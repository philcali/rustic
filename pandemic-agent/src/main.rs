mod files;
mod handlers;
mod infection;
mod packages;
mod socket;
mod systemd;
mod users;

use anyhow::Result;
use clap::Parser;
use hmac::{Hmac, Mac};
use pandemic_protocol::{AgentRequest, AuthChallenge, AuthResponse, Response};
use sha2::Sha256;
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tracing::{error, info, warn};

use handlers::handle_agent_request;
use socket::setup_socket_permissions;

#[derive(Parser)]
#[command(name = "pandemic-agent")]
#[command(about = "Privileged agent for pandemic system management")]
pub struct Args {
    #[arg(long, default_value = "/var/run/pandemic/admin.sock")]
    pub socket_path: PathBuf,

    #[arg(long, default_value = "pandemic")]
    pub user: String,

    #[arg(long, default_value = "pandemic")]
    pub group: String,

    #[arg(long)]
    pub secret: Option<String>,

    #[arg(long)]
    pub secret_path: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    // Ensure we're running as root
    if unsafe { libc::getuid() } != 0 {
        return Err(anyhow::anyhow!("pandemic-agent must run as root"));
    }

    info!("Starting pandemic-agent as root");

    // Remove existing socket if it exists
    if args.socket_path.exists() {
        std::fs::remove_file(&args.socket_path)?;
    }

    // Create socket directory if it doesn't exist
    if let Some(parent) = args.socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Bind to Unix socket
    let listener = UnixListener::bind(&args.socket_path)?;

    // Set socket permissions and ownership
    setup_socket_permissions(&args)?;

    info!("Agent listening on {:?}", args.socket_path);

    // Resolve shared secret: --secret > --secret-path > default path > generate
    let secret = resolve_secret(
        args.secret.as_deref(),
        args.secret_path.as_deref(),
        pandemic_common::AGENT_SECRET_PATH,
    )?;

    // Accept connections
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let secret = secret.clone();
                tokio::spawn(handle_connection(stream, secret));
            }
            Err(e) => {
                error!("Failed to accept connection: {}", e);
            }
        }
    }
}

/// Resolve the shared secret.
///
/// Precedence: `--secret` inline value > `--secret-path` file > the default
/// secret path (installed by `pandemic-cli bootstrap install --with-agent`)
/// > freshly generated (last resort, printed to the log).
fn resolve_secret(
    secret: Option<&str>,
    secret_path: Option<&std::path::Path>,
    default_secret_path: &str,
) -> Result<String> {
    if let Some(secret) = secret {
        return Ok(secret.to_string());
    }

    if let Some(path) = secret_path {
        return Ok(std::fs::read_to_string(path)?.trim().to_string());
    }

    if let Ok(content) = std::fs::read_to_string(default_secret_path) {
        let trimmed = content.trim().to_string();
        if !trimmed.is_empty() {
            info!("Using agent secret from {}", default_secret_path);
            return Ok(trimmed);
        }
    }

    let secret = hex::encode(rand::random::<[u8; 32]>());
    error!(
        "!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!
 WARN: No agent secret configured. A random secret was generated.
       You MUST save this secret and pass it via --secret or --secret-path
       on subsequent runs, or clients will be unable to authenticate.

 agent secret: {}
!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!",
        secret
    );
    Ok(secret)
}

async fn handle_connection(mut stream: UnixStream, secret: String) -> Result<()> {
    let (reader, mut writer) = stream.split();
    let mut buf_reader = BufReader::new(reader);
    let mut line = String::new();

    // Send auth challenge
    let nonce = hex::encode(rand::random::<[u8; 16]>());
    let challenge = AuthChallenge {
        nonce: nonce.clone(),
    };
    let challenge_json = serde_json::to_string(&challenge)?;
    writer.write_all(challenge_json.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;

    // Wait for auth response
    line.clear();
    buf_reader.read_line(&mut line).await?;
    let trimmed = line.trim().to_string();
    line.clear();

    let auth_response = match serde_json::from_str::<AuthResponse>(&trimmed) {
        Ok(resp) => resp,
        Err(e) => {
            warn!("Authentication failed: expected AuthResponse ({e})");
            return Ok(());
        }
    };

    // Verify HMAC-SHA256 signature
    let mut mac: Hmac<Sha256> = Hmac::new_from_slice(secret.as_bytes())?;
    mac.update(auth_response.nonce.as_bytes());
    let expected = mac.finalize().into_bytes();

    // Decode the received signature from hex
    let sig_bytes = match hex::decode(&auth_response.signature) {
        Ok(b) => b,
        Err(_) => {
            warn!("Authentication failed: invalid hex signature");
            return Ok(());
        }
    };

    // Constant-time comparison to prevent timing attacks
    let equal: bool = subtle::ConstantTimeEq::ct_eq(&expected[..], &sig_bytes).into();
    if !equal {
        warn!("Authentication failed: invalid signature");
        return Ok(());
    }

    info!("Authenticated client connected");

    // Process normal requests
    while buf_reader.read_line(&mut line).await? > 0 {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            line.clear();
            continue;
        }

        let response = match serde_json::from_str::<AgentRequest>(trimmed) {
            Ok(request) => handle_agent_request(request).await,
            Err(e) => {
                warn!("Failed to parse message: {}", e);
                Response::error("Invalid message format")
            }
        };

        let response_json = serde_json::to_string(&response)?;
        writer.write_all(response_json.as_bytes()).await?;
        writer.write_all(b"\n").await?;

        line.clear();
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::resolve_secret;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pandemic-agent-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn inline_secret_wins() {
        let dir = temp_dir("inline");
        let path = dir.join("secret");
        std::fs::write(&path, "file-secret\n").unwrap();
        let resolved = resolve_secret(
            Some("inline-secret"),
            Some(path.as_path()),
            dir.join("default").to_str().unwrap(),
        )
        .unwrap();
        assert_eq!(resolved, "inline-secret");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn secret_path_is_trimmed() {
        let dir = temp_dir("path");
        let path = dir.join("secret");
        std::fs::write(&path, "file-secret\n").unwrap();
        let resolved =
            resolve_secret(None, Some(path.as_path()), "nonexistent-secret-path").unwrap();
        assert_eq!(resolved, "file-secret");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_path_used_when_no_flags() {
        let dir = temp_dir("default");
        std::fs::write(dir.join("agent-secret"), "default-secret\n").unwrap();
        let resolved =
            resolve_secret(None, None, dir.join("agent-secret").to_str().unwrap()).unwrap();
        assert_eq!(resolved, "default-secret");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn generates_when_nothing_configured() {
        let resolved = resolve_secret(None, None, "nonexistent-secret-path").unwrap();
        assert_eq!(resolved.len(), 64, "expected 32 random bytes hex-encoded");
        assert!(resolved.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
