//! `pandemic-node` — the *node* primitive of the epidemic feature.
//!
//! A node is a host that runs a small TCP receiver exposing a **narrow**
//! deployment surface (see [`allowlist`]) to a coordinator, and forwards each
//! approved request to the local privileged agent over the Unix admin socket.
//!
//! Two distinct secrets are in play, matching the two hops:
//!   * the **epidemic** secret guards the network handshake (coordinator ->
//!     node);
//!   * the **agent** secret guards the local hop (node -> agent).
//!
//! The handshake crypto, framing, and secret handling are all shared with the
//! agent and the CLI via `pandemic_common`, so a node, an agent, and a
//! coordinator speak the identical protocol.
//!
//! **Discovery (increment 2):** alongside the TCP listener, the node
//! advertises itself over mDNS as `<name>._pandemic-node._tcp.local` (on the
//! interface matching `--listen`), so a coordinator on the same link can find
//! it with `pandemic-cli epidemic nodes --discover` instead of a hand-typed
//! roster. Disable with `--no-advertise`; the TCP surface is unaffected
//! either way.

mod allowlist;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tracing::{error, info, warn};

use anyhow::Result;
use clap::Parser;
use pandemic_common::{auth, discovery, AgentClient};
use pandemic_protocol::{AgentRequest, AuthChallenge, AuthResponse, Response};

use allowlist::request_allowed;

const DEFAULT_LISTEN: &str = "127.0.0.1:7711";

#[derive(Parser)]
#[command(name = "pandemic-node")]
#[command(
    about = "Epidemic node receiver: a narrow agent surface over TCP, forwarded to the local agent"
)]
pub struct Args {
    /// TCP address to listen on (the coordinator -> node network hop).
    #[arg(long, default_value = DEFAULT_LISTEN)]
    pub listen: String,

    /// Node identity advertised over mDNS (the `_pandemic-node._tcp.local`
    /// instance name; letters/digits/hyphens). Defaults to the hostname.
    #[arg(long)]
    pub name: Option<String>,

    /// Do not advertise over mDNS (the TCP surface is unaffected).
    #[arg(long)]
    pub no_advertise: bool,

    /// Epidemic (network) shared secret, inline.
    #[arg(long)]
    pub secret: Option<String>,

    /// Path to a file holding the epidemic (network) shared secret.
    #[arg(long)]
    pub secret_path: Option<PathBuf>,

    /// Path to the local agent admin socket to forward approved requests to.
    #[arg(long, default_value = "/var/run/pandemic/admin.sock")]
    pub agent_socket: PathBuf,

    /// Local agent shared secret, inline (the node -> agent hop).
    #[arg(long)]
    pub agent_secret: Option<String>,

    /// Path to a file holding the local agent shared secret.
    #[arg(long)]
    pub agent_secret_path: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    // The node forwards to a root-only agent, so it must itself be root.
    if unsafe { libc::getuid() } != 0 {
        return Err(anyhow::anyhow!("pandemic-node must run as root"));
    }

    let epidemic_secret = resolve_secret(
        args.secret.as_deref(),
        args.secret_path.as_deref(),
        auth::EPIDEMIC_SECRET_PATH,
        "epidemic",
    )?;

    let agent_secret = resolve_secret(
        args.agent_secret.as_deref(),
        args.agent_secret_path.as_deref(),
        pandemic_common::AGENT_SECRET_PATH,
        "agent",
    )?;

    let listener = TcpListener::bind(&args.listen).await?;
    info!(
        "pandemic-node listening on {} (deployment surface -> agent at {:?})",
        args.listen, args.agent_socket
    );

    // Discovery (increment 2): advertise alongside the TCP listener, on the
    // interface matching --listen, so a coordinator on the same link finds
    // this node. Advertise failure is not fatal — the TCP surface keeps
    // working and an explicit `--node` roster still reaches this node.
    // (The binding is held for the process lifetime: dropping it stops
    // the advertisement.)
    let _advertiser = if args.no_advertise {
        None
    } else {
        let name = args.name.clone().unwrap_or_else(default_node_name);
        match advertise_for_listen(&args.listen, &name).await {
            Ok(server) => Some(server),
            Err(e) => {
                warn!("mDNS advertise failed; node will not be discoverable: {e}");
                None
            }
        }
    };

    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let epidemic_secret = epidemic_secret.clone();
                let agent_secret = agent_secret.clone();
                let agent_socket = args.agent_socket.clone();
                tokio::spawn(async move {
                    let agent_client =
                        AgentClient::with_socket_path(agent_socket).with_secret(agent_secret);
                    if let Err(e) = serve_node(stream, &epidemic_secret, &agent_client).await {
                        warn!("connection from {peer} ended: {e}");
                    }
                });
            }
            Err(e) => error!("Failed to accept connection: {e}"),
        }
    }
}

/// Start the mDNS advertiser for the `--listen` address:
///
///   * `127.0.0.1:7711` — advertise `127.0.0.1:7711` on loopback (the
///     e2e spread-to-self path);
///   * `0.0.0.0:7711`   — advertise the primary LAN IPv4 on that interface;
///   * `<ip>:7711`      — advertise that IP on its interface.
async fn advertise_for_listen(listen: &str, name: &str) -> Result<discovery::Advertiser> {
    let addr: SocketAddr = listen
        .parse()
        .map_err(|e| anyhow::anyhow!("parsing --listen '{listen}': {e}"))?;
    let port = addr.port();
    match addr.ip() {
        IpAddr::V4(v4) if v4.is_unspecified() => {
            let lan = primary_lan_ip().ok_or_else(|| {
                anyhow::anyhow!("--listen 0.0.0.0 but no LAN IPv4 interface to advertise on")
            })?;
            discovery::advertise_node(name, IpAddr::V4(lan), port, lan).await
        }
        IpAddr::V4(v4) => discovery::advertise_node(name, IpAddr::V4(v4), port, v4).await,
        IpAddr::V6(_) => {
            anyhow::bail!(
                "mDNS advertise is IPv4-only; use an IPv4 --listen (the TCP surface is unaffected)"
            )
        }
    }
}

/// The local IPv4 the kernel would route to a public destination — i.e. the
/// primary LAN address. (UDP `connect` sends no packets.)
fn primary_lan_ip() -> Option<Ipv4Addr> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    match sock.local_addr().ok()?.ip() {
        IpAddr::V4(v4) => Some(v4),
        IpAddr::V6(_) => None,
    }
}

/// The default node identity for the mDNS instance name: the hostname.
fn default_node_name() -> String {
    let mut buf = [0u8; 256];
    let ret = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if ret == 0 {
        let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        let name = String::from_utf8_lossy(&buf[..len]).into_owned();
        if !name.is_empty() {
            return name;
        }
    }
    "pandemic-node".to_string()
}

/// Run one coordinator connection: authenticate it with the epidemic secret,
/// then forward each allowed request to the local agent. Denied requests get a
/// `Response::Error`; the connection ends when the client closes the stream.
pub async fn serve_node<S>(
    stream: S,
    epidemic_secret: &str,
    agent_client: &AgentClient,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (reader, mut writer) = tokio::io::split(stream);
    let mut buf_reader = BufReader::new(reader);
    let mut line = String::new();

    // Handshake: challenge, then verify the coordinator's signature.
    let challenge = AuthChallenge {
        nonce: auth::generate_nonce(),
    };
    let mut payload = serde_json::to_string(&challenge)?;
    payload.push('\n');
    writer.write_all(payload.as_bytes()).await?;
    writer.flush().await?;

    buf_reader.read_line(&mut line).await?;
    let auth_response: AuthResponse = match serde_json::from_str(line.trim()) {
        Ok(resp) => resp,
        Err(e) => {
            warn!("Handshake failed: expected AuthResponse ({e})");
            return Ok(());
        }
    };
    if !auth::verify(
        epidemic_secret,
        &auth_response.nonce,
        &auth_response.signature,
    ) {
        warn!("Handshake failed: invalid signature");
        return Ok(());
    }
    info!("Coordinator authenticated");

    // Request loop: one line at a time, until the client disconnects.
    loop {
        line.clear();
        if buf_reader.read_line(&mut line).await? == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let request = match serde_json::from_str::<AgentRequest>(trimmed) {
            Ok(req) => req,
            Err(e) => {
                send_response(
                    &mut writer,
                    &Response::error(format!("invalid message: {e}")),
                )
                .await?;
                continue;
            }
        };

        let response = if request_allowed(&request) {
            match agent_client.send_agent_request(&request).await {
                Ok(resp) => resp,
                Err(e) => Response::error(format!("agent error: {e}")),
            }
        } else {
            warn!("Denied request (deployment surface only): {request:?}");
            Response::error("request not allowed on a node (deployment surface only)")
        };

        send_response(&mut writer, &response).await?;
    }

    Ok(())
}

async fn send_response<W>(writer: &mut W, response: &Response) -> std::io::Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let mut payload = serde_json::to_string(response)?;
    payload.push('\n');
    writer.write_all(payload.as_bytes()).await?;
    writer.flush().await?;
    Ok(())
}

/// Resolve a secret: `inline > path file > default path > generated`.
///
/// Mirrors the agent's resolution so operators keep one mental model. The last
/// resort generates a fresh secret and prints it — a bootstrap aid, never the
/// steady state (the peer must be told the same value or handshakes fail).
fn resolve_secret(
    secret: Option<&str>,
    secret_path: Option<&Path>,
    default_path: &str,
    label: &str,
) -> Result<String> {
    if let Some(secret) = secret {
        return Ok(secret.to_string());
    }
    if let Some(path) = secret_path {
        let content = std::fs::read_to_string(path)?;
        return Ok(content.trim().to_string());
    }
    if let Ok(content) = std::fs::read_to_string(default_path) {
        let trimmed = content.trim().to_string();
        if !trimmed.is_empty() {
            info!("Using {label} secret from {default_path}");
            return Ok(trimmed);
        }
    }
    let secret = auth::generate_secret();
    error!(
        "WARN: no {label} secret configured; generated one. Save it and pass the same value to the peer (--secret/--secret-path), or handshakes will fail. {label} secret: {secret}"
    );
    Ok(secret)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use pandemic_common::RemoteClient;
    use serde_json::json;
    use tempfile::tempdir;
    use tokio::net::UnixListener;

    /// A stand-in local agent: does the HMAC handshake with `agent_secret`,
    /// reads one request, and answers. Lets us exercise the node's handshake,
    /// allowlist, and forwarding without a real privileged agent.
    async fn spawn_fake_agent(socket_path: &Path, agent_secret: &str) {
        let listener = UnixListener::bind(socket_path).unwrap();
        let secret = agent_secret.to_string();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = tokio::io::split(stream);
            let mut br = BufReader::new(reader);
            let mut line = String::new();

            let challenge = AuthChallenge {
                nonce: auth::generate_nonce(),
            };
            let mut payload = serde_json::to_string(&challenge).unwrap();
            payload.push('\n');
            writer.write_all(payload.as_bytes()).await.unwrap();
            writer.flush().await.unwrap();

            br.read_line(&mut line).await.unwrap();
            let resp: AuthResponse = serde_json::from_str(line.trim()).unwrap();
            line.clear();
            if !auth::verify(&secret, &resp.nonce, &resp.signature) {
                return;
            }

            br.read_line(&mut line).await.unwrap();
            let request: AgentRequest = serde_json::from_str(line.trim()).unwrap();
            let response = match &request {
                AgentRequest::GetCapabilities => {
                    Response::success_with_data(json!({"capabilities": ["fake-cap"]}))
                }
                _ => Response::success_with_data(json!({"reached": true})),
            };
            let mut p = serde_json::to_string(&response).unwrap();
            p.push('\n');
            writer.write_all(p.as_bytes()).await.unwrap();
            writer.flush().await.unwrap();
        });
    }

    /// Start a node (handshake + forwarding) on an ephemeral port, wired to
    /// `agent_socket`/`agent_secret`; returns the `host:port` to dial.
    async fn bind_node(epidemic_secret: &str, agent_socket: &Path, agent_secret: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let es = epidemic_secret.to_string();
        let agent_client =
            AgentClient::with_socket_path(agent_socket).with_secret(agent_secret.to_string());
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let _ = serve_node(stream, &es, &agent_client).await;
        });
        addr.to_string()
    }

    #[tokio::test]
    async fn allowed_request_is_forwarded_to_agent() {
        let dir = tempdir().unwrap();
        let agent_sock = dir.path().join("agent.sock");
        spawn_fake_agent(&agent_sock, "agent-secret").await;

        let node_addr = bind_node("epi-secret", &agent_sock, "agent-secret").await;

        let client = RemoteClient::new(node_addr, "epi-secret");
        let caps = client.ping().await.unwrap();
        assert_eq!(caps, vec!["fake-cap".to_string()]);
    }

    #[tokio::test]
    async fn denied_request_is_rejected() {
        // No agent needed: a denied request must never reach it.
        let node_addr = bind_node(
            "epi-secret",
            Path::new("/nonexistent/agent.sock"),
            "agent-secret",
        )
        .await;

        let client = RemoteClient::new(node_addr, "epi-secret");
        let resp = client
            .send_agent_request(&AgentRequest::GetHealth)
            .await
            .unwrap();
        assert!(
            matches!(&resp, Response::Error { message } if message.contains("not allowed")),
            "expected a denial, got {resp:?}"
        );
    }

    #[tokio::test]
    async fn advertise_for_listen_uses_the_loopback_interface() {
        let server = advertise_for_listen("127.0.0.1:7711", "epi-node-test")
            .await
            .expect("advertiser starts");
        // And a coordinator probing loopback finds it, with the right port.
        let found = pandemic_common::discover_nodes(
            Some(Ipv4Addr::new(127, 0, 0, 1)),
            Duration::from_secs(3),
        )
        .await
        .unwrap();
        assert!(
            found
                .iter()
                .any(|n| n.name == "epi-node-test" && n.addr == "127.0.0.1:7711"),
            "expected epi-node-test@127.0.0.1:7711 in {found:?}"
        );
        server.shutdown().await;
    }

    #[tokio::test]
    async fn advertise_for_listen_rejects_ipv6_listens() {
        let err = match advertise_for_listen("[::1]:7711", "epi-node-test").await {
            Err(e) => e,
            Ok(_) => panic!("IPv6 listen must not advertise"),
        };
        assert!(err.to_string().contains("IPv4-only"));
    }

    #[tokio::test]
    async fn wrong_secret_fails_handshake() {
        let dir = tempdir().unwrap();
        let agent_sock = dir.path().join("agent.sock");
        spawn_fake_agent(&agent_sock, "agent-secret").await;

        let node_addr = bind_node("correct-epi-secret", &agent_sock, "agent-secret").await;

        // The coordinator signs with the wrong epidemic secret, so the node
        // drops the connection and the first request round-trip fails.
        let client = RemoteClient::new(node_addr, "wrong-epi-secret");
        assert!(client.ping().await.is_err());
    }
}
