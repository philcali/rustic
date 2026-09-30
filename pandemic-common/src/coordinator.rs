//! The epidemic coordinator: shared spread logic for the CLI and the REST API.
//!
//! The *coordinator* is the third member of the node / group / coordinator
//! primitive (the node's `pandemic-node` receiver and the group in
//! `groups.toml` being the other two). Its job — resolving the roster,
//! signing a broadcast intent, driving the epidemic handshake, collecting
//! per-node results, and writing the audit record — lives here so that the
//! CLI (`pandemic epidemic spread`) and the REST surface
//! (`POST /api/epidemic/spread`, increment 5c) share one implementation and
//! cannot diverge.
//!
//! Two distinct secrets are in play, exactly as on the node:
//!   * the **epidemic** secret guards the coordinator -> node handshake;
//!   * the **agent** secret is the node's business (the coordinator never
//!     sees it).
//!
//! **Live progress (5c):** both `run_*_spread` take an optional
//! [`mpsc::UnboundedSender`] sink of [`SpreadProgress`] events. The CLI
//! passes `None`; the REST layer forwards each event to the daemon event
//! bus (topic `epidemic.spread`), and the console renders them over the
//! existing `/api/events/stream` websocket.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use pandemic_protocol::{
    AgentRequest, AuthChallenge, AuthResponse, Canary, PlanIdentity, Response, SpreadIntent,
    SpreadMode, SpreadNodeResult, SpreadProgress, SpreadRecord, SpreadStage, SpreadTarget,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex};

use crate::discovery::DiscoveredNode;
use crate::groups::{find_group, load_groups_or_default, merge_roster, NodeConfig};
use crate::{
    auth, deployment_apply_infections, generate_spread_id, history, intent_token, parse_criteria,
    send_intent, sha256_hex, Criterion, DeploymentPlan, RemoteClient, SpreadRateLimit, TlsClient,
    INTENT_RESENDS, INTENT_VERSION,
};

/// The epidemic (network) secret as supplied by the caller: an inline value
/// and/or a path to a file holding it. Bundled so it travels as one unit
/// through the coordinator's call chain.
#[derive(Default, Clone)]
pub struct SecretSource {
    pub inline: Option<String>,
    pub path: Option<PathBuf>,
}

impl SecretSource {
    /// Build a source from an inline value and/or a file path.
    pub fn new(inline: Option<String>, path: Option<PathBuf>) -> Self {
        Self { inline, path }
    }
}

/// A resolved epidemic secret, plus whether it was freshly generated (a
/// bootstrap aid — every node must then be given the same value).
#[derive(Clone)]
pub struct ResolvedSecret {
    /// The secret value (trimmed).
    pub value: String,
    /// True when nothing was configured and a secret was generated on the
    /// spot (the caller should surface the value so it can be handed out).
    pub generated: bool,
}

/// TLS options for the coordinator -> node hop (increment 4). `ca` is the
/// root CA (PEM) used to verify each node's certificate; `server_name`
/// overrides the server name each node's cert must present (default: the
/// node's host).
#[derive(Default, Clone)]
pub struct TlsOptions {
    /// Whether the hop is encrypted (requires `ca`).
    pub enabled: bool,
    /// Root CA (PEM) trusting the nodes' certificates.
    pub ca: Option<PathBuf>,
    /// Server name each node's cert must present (default: the node's host).
    pub server_name: Option<String>,
}

/// The outcome of a completed spread: the audit record (already appended to
/// the history) plus a freshly generated secret, when one was.
#[derive(Clone)]
pub struct SpreadResult {
    /// The audit record (identical to the line appended to the history).
    pub record: SpreadRecord,
    /// The generated epidemic secret, when nothing was configured (a
    /// bootstrap aid — hand it to every node). Absent otherwise.
    pub generated_secret: Option<String>,
}

/// Resolve the epidemic (network) secret.
///
/// Precedence: inline `--secret` > `--secret-path` file > the group's
/// `secret_path` (if any) > the default path > freshly generated (a
/// bootstrap aid — every node must be given the same value, or handshakes
/// fail; the `generated` flag lets the caller say so).
pub fn resolve_epidemic_secret(
    secret: &SecretSource,
    group_secret_path: Option<&str>,
) -> Result<ResolvedSecret> {
    if let Some(secret) = &secret.inline {
        return Ok(ResolvedSecret {
            value: secret.clone(),
            generated: false,
        });
    }
    if let Some(path) = &secret.path {
        let content = std::fs::read_to_string(path.as_path())
            .with_context(|| format!("reading epidemic secret {}", path.display()))?;
        return Ok(ResolvedSecret {
            value: content.trim().to_string(),
            generated: false,
        });
    }
    if let Some(path) = group_secret_path {
        if let Ok(content) = std::fs::read_to_string(path) {
            let trimmed = content.trim().to_string();
            if !trimmed.is_empty() {
                return Ok(ResolvedSecret {
                    value: trimmed,
                    generated: false,
                });
            }
        }
    }
    if let Ok(content) = std::fs::read_to_string(auth::EPIDEMIC_SECRET_PATH) {
        let trimmed = content.trim().to_string();
        if !trimmed.is_empty() {
            return Ok(ResolvedSecret {
                value: trimmed,
                generated: false,
            });
        }
    }
    let value = auth::generate_secret();
    Ok(ResolvedSecret {
        value,
        generated: true,
    })
}

/// The TLS client for one node (increment 4): the shared trust config with
/// this node's server name attached (default: the node's host), since a
/// roster may mix endpoints. `Ok(None)` when TLS is disabled.
fn node_tls_client(tls: &TlsOptions, addr: &str) -> Result<Option<TlsClient>> {
    if !tls.enabled {
        return Ok(None);
    }
    let ca = tls.ca.as_deref().ok_or_else(|| {
        anyhow!("TLS is enabled but no root CA was provided (tls_ca / --tls-ca is required)")
    })?;
    let trust = TlsClient::trust_from_paths(ca)?;
    let server_name = tls.server_name.clone().unwrap_or_else(|| host_of(addr));
    Ok(Some(TlsClient::with_server_name(trust, server_name)?))
}

/// Resolve the coordinator's target list: the named group's roster plus any
/// ad-hoc endpoints. Also surfaces the group's optional `secret_path` so the
/// secret resolution can fall back to it.
pub fn resolve_roster(
    group: Option<&str>,
    adhoc: &[String],
) -> Result<(Vec<NodeConfig>, Option<String>)> {
    match group {
        Some(name) => {
            let groups = load_groups_or_default(None)?;
            let g = find_group(&groups, name)
                .with_context(|| format!("group '{name}' not found (see `epidemic nodes`)"))?;
            Ok((merge_roster(Some(g), adhoc), g.secret_path.clone()))
        }
        None => Ok((merge_roster(None, adhoc), None)),
    }
}

/// Union `--discover` results into the roster: append each discovered node
/// that is not already present by address (roster entries keep their configured
/// names; discovered nodes use their mDNS instance name). Returns how many
/// were added.
pub fn merge_discovered(roster: &mut Vec<NodeConfig>, discovered: &[DiscoveredNode]) -> usize {
    let mut added = 0;
    for d in discovered {
        if roster.iter().any(|n| n.addr == d.addr) {
            continue;
        }
        roster.push(NodeConfig {
            name: d.name.clone(),
            addr: d.addr.clone(),
        });
        added += 1;
    }
    added
}

/// The host portion of a `host:port` node address (used as the default TLS
/// server name). Roster endpoints are IPv4 / hostname, so the first `:`
/// separates host from port.
pub fn host_of(addr: &str) -> String {
    addr.split(':').next().unwrap_or(addr).to_string()
}

/// Default retry policy (increment 4b): a dropped node is retried and
/// reported, not silently lost. `retries` is the number of *re*-attempts
/// after the first one, and the timeout bounds each attempt (a hung node
/// becomes a retriable failure instead of wedging the spread).
pub const DEFAULT_APPLY_RETRIES: u32 = 2;
pub const DEFAULT_APPLY_TIMEOUT: Duration = Duration::from_secs(30);
/// Backoff base between attempts; doubles per retry (500ms, 1s, 2s, …).
const RETRY_BACKOFF_BASE: Duration = Duration::from_millis(500);

/// The result of one apply attempt against one node.
enum ApplyResult {
    /// The node applied the deployment.
    Ok,
    /// A transport-level failure (dial, handshake, timeout): the node never
    /// gave a verdict, so asking again is safe and useful. Retried.
    Transient(String),
    /// The node answered with an error verdict: re-asking with the same plan
    /// gets the same verdict, so the retry loop stops here. Not retried.
    Verdict(String),
}

/// The final outcome of applying to one node (after any retries).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyOutcome {
    /// Did the node apply the deployment?
    pub ok: bool,
    /// The failure message (when `ok` is false).
    pub error: Option<String>,
    /// How many attempts were made (1..=`retries`+1).
    pub attempts: u32,
}

/// One attempt against a node (ping + apply), bounded by `timeout`.
///
/// A failed ping, a dropped connection, or a timeout is a **transient**
/// failure — the node never reached a verdict. A `Response::Error` /
/// `NotFound` from the node is a **verdict** — the node said no, and the
/// same plan will get the same answer.
///
/// When `tls` is set (increment 4a) the connection is wrapped in TLS with
/// the shared trust config before the shared handshake. Re-applying is safe:
/// the agent's install steps are idempotent by design, so a retry after a
/// partial apply resumes where it left off (4b idempotency).
async fn apply_once(
    addr: &str,
    secret: &str,
    request: &AgentRequest,
    tls: Option<&TlsClient>,
    timeout: Duration,
) -> ApplyResult {
    let client = match tls {
        Some(tls) => RemoteClient::new(addr, secret).with_tls(tls.clone()),
        None => RemoteClient::new(addr, secret),
    };
    let attempt = async {
        if let Err(e) = client.ping().await {
            return ApplyResult::Transient(format!("node {addr} unreachable: {e:#}"));
        }
        match client.send_agent_request(request).await {
            Ok(Response::Success { .. }) => ApplyResult::Ok,
            Ok(Response::Error { message }) => ApplyResult::Verdict(message),
            Ok(Response::NotFound { message }) => ApplyResult::Verdict(message),
            Err(e) => ApplyResult::Transient(format!("node {addr} failed to apply: {e:#}")),
        }
    };
    match tokio::time::timeout(timeout, attempt).await {
        Ok(result) => result,
        Err(_) => ApplyResult::Transient(format!(
            "node {addr} timed out after {timeout:?} (no verdict)"
        )),
    }
}

/// Apply to one node with the retry policy: transient failures are retried
/// up to `retries` more times with exponential backoff (500ms, 1s, 2s, …);
/// a node verdict is final. The outcome always reports how many attempts
/// were made, so a dropped node is *retried and reported*, not silently
/// lost (increment 4b).
pub async fn apply_with_retries(
    addr: &str,
    secret: &str,
    request: &AgentRequest,
    tls: Option<&TlsClient>,
    retries: u32,
    timeout: Duration,
) -> ApplyOutcome {
    let mut attempts = 0u32;
    loop {
        attempts += 1;
        match apply_once(addr, secret, request, tls, timeout).await {
            ApplyResult::Ok => {
                return ApplyOutcome {
                    ok: true,
                    error: None,
                    attempts,
                }
            }
            ApplyResult::Verdict(message) => {
                return ApplyOutcome {
                    ok: false,
                    error: Some(message),
                    attempts,
                }
            }
            ApplyResult::Transient(message) => {
                if attempts > retries {
                    return ApplyOutcome {
                        ok: false,
                        error: Some(message),
                        attempts,
                    };
                }
                let backoff = RETRY_BACKOFF_BASE * 2u32.saturating_pow(attempts - 1);
                tokio::time::sleep(backoff).await;
            }
        }
    }
}

/// The audit record for a roster spread (5a): which nodes, which plan hash,
/// and each node's outcome. Pure, so the shape is testable without a network.
pub fn roster_record(
    spread_id: &str,
    group: Option<&str>,
    name: &str,
    version: &str,
    plan_sha: &str,
    results: &[(NodeConfig, ApplyOutcome)],
) -> SpreadRecord {
    let nodes = roster_node_results(results);
    let applied = nodes.iter().filter(|n| n.ok).count() as u32;
    let failed = (nodes.len() - applied as usize) as u32;
    let ok = !nodes.is_empty() && failed == 0;
    SpreadRecord {
        timestamp: now_unix_secs(),
        spread_id: spread_id.to_string(),
        mode: SpreadMode::Roster,
        stage: "full".to_string(),
        name: name.to_string(),
        version: version.to_string(),
        sha256: plan_sha.to_string(),
        group: group.map(str::to_string),
        origin: None,
        criteria: Vec::new(),
        canary: None,
        nodes,
        applied,
        failed,
        ok,
    }
}

/// Per-node results from the roster path (the coordinator knows each node's
/// name and address from the roster).
pub fn roster_node_results(results: &[(NodeConfig, ApplyOutcome)]) -> Vec<SpreadNodeResult> {
    results
        .iter()
        .map(|(node, outcome)| SpreadNodeResult {
            name: node.name.clone(),
            addr: node.addr.clone(),
            ok: outcome.ok,
            error: outcome.error.clone(),
            attempts: outcome.attempts,
        })
        .collect()
}

/// Per-node results from the broadcast path (the coordinator only learns each
/// node's callback peer address — never its name).
pub fn broadcast_node_results(callbacks: &[(SocketAddr, Response)]) -> Vec<SpreadNodeResult> {
    callbacks
        .iter()
        .map(|(peer, response)| match response {
            Response::Success { .. } => SpreadNodeResult {
                name: peer.to_string(),
                addr: peer.to_string(),
                ok: true,
                error: None,
                // Broadcast has no coordinator-side retry: the node applies
                // once (deduped by spread_id) and a lost callback is a
                // reported miss, not a silent one.
                attempts: 1,
            },
            Response::Error { message } | Response::NotFound { message } => SpreadNodeResult {
                name: peer.to_string(),
                addr: peer.to_string(),
                ok: false,
                error: Some(message.clone()),
                attempts: 1,
            },
        })
        .collect()
}

/// Validate the broadcast flag combinations up front (before any network I/O):
/// `--broadcast` is the roster-free path (no `--node`/`--group`/`--discover`),
/// and the canary/promote/spread-id/criteria options are broadcast-only.
#[allow(clippy::too_many_arguments)]
pub fn validate_broadcast(
    broadcast: bool,
    group: Option<&str>,
    adhoc: &[String],
    discover: bool,
    criteria: &[String],
    canary: Option<&Canary>,
    promote: bool,
    spread_id: Option<&str>,
) -> Result<()> {
    if broadcast {
        if group.is_some() || !adhoc.is_empty() || discover {
            bail!(
                "--broadcast spreads over the multicast group and takes no roster — drop --node/--group/--discover"
            );
        }
        if promote && canary.is_some() {
            bail!("--promote and --canary are mutually exclusive (promote re-broadcasts an existing spread at Full)");
        }
        if promote && spread_id.is_none() {
            bail!("--promote requires --spread-id (see `epidemic spreads` for recent ids)");
        }
        // The criteria must be parseable (the node re-parses them, but we fail
        // early with a clear message instead of broadcasting garbage).
        parse_criteria(criteria)?;
        return Ok(());
    }
    // Not broadcast: the broadcast-only flags must be absent.
    if canary.is_some() {
        bail!("--canary requires --broadcast");
    }
    if promote {
        bail!("--promote requires --broadcast");
    }
    if spread_id.is_some() {
        bail!("--spread-id is a --broadcast flag");
    }
    if !criteria.is_empty() {
        bail!("--criteria is a --broadcast flag (a roster spread targets its nodes explicitly)");
    }
    Ok(())
}

/// Parse a `--canary` argument: a percentage (`25` or `25%`) →
/// [`Canary::Percentage`]; a `KEY=VALUE` criterion (`role=canary`) →
/// [`Canary::Subset`]. Anything else is an error.
pub fn parse_canary_arg(raw: &str) -> Result<Canary> {
    let s = raw.trim();
    if s.is_empty() {
        bail!("--canary expects a percentage (e.g. 25) or a criterion (e.g. role=canary)");
    }
    let pct_str = s.strip_suffix('%').unwrap_or(s);
    if let Ok(pct) = pct_str.parse::<u8>() {
        return Ok(Canary::Percentage { pct });
    }
    if s.contains('=') {
        Criterion::parse(s)?; // validate the key=value form
        return Ok(Canary::Subset {
            criterion: s.to_string(),
        });
    }
    bail!(
        "--canary expects a percentage (e.g. 25) or a key=value criterion (e.g. role=canary), got '{raw}'"
    )
}

/// The unix-seconds timestamp for an intent's `issued_at`.
pub fn now_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Build a spread intent from the concrete plan's identity. `plan_sha` is the
/// sha256 of the exact `ApplyDeployment` request line the callback will send
/// (the node trims the trailing newline before hashing, so it matches).
#[allow(clippy::too_many_arguments)]
pub fn build_intent(
    secret: &str,
    spread_id: &str,
    stage: SpreadStage,
    name: &str,
    version: &str,
    plan_sha: &str,
    criteria: Vec<String>,
    canary: Option<Canary>,
    origin: String,
    callback_port: u16,
    issued_at: i64,
) -> SpreadIntent {
    SpreadIntent {
        version: INTENT_VERSION,
        spread_id: spread_id.to_string(),
        stage,
        issued_at,
        token: intent_token(secret, spread_id),
        group: None,
        plan: PlanIdentity {
            name: name.to_string(),
            version: version.to_string(),
            sha256: plan_sha.to_string(),
        },
        criteria,
        canary,
        origin,
        callback_port,
    }
}

/// The local IPv4 the kernel would route to a public destination — i.e. the
/// primary LAN address (used as the intent's `origin` when no interface is
/// given). A UDP `connect` sends no packets.
pub fn primary_lan_ip() -> Option<Ipv4Addr> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    match sock.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(v4) => Some(v4),
        std::net::IpAddr::V6(_) => None,
    }
}

/// Send one progress event (a no-op when there is no sink; a dropped receiver
/// — e.g. the REST handler's forwarder aborted — must never fail the spread).
fn emit(progress: Option<&mpsc::UnboundedSender<SpreadProgress>>, event: SpreadProgress) {
    if let Some(tx) = progress {
        let _ = tx.send(event);
    }
}

/// One callback connection: the coordinator is the server here. Handshake with
/// the epidemic secret, send the exact plan line (the intent's digest is over
/// it, no terminator), and read the node's `Response`.
pub async fn serve_one_callback(stream: TcpStream, secret: &str, request_line: &str) -> Response {
    let (reader, mut writer) = tokio::io::split(stream);
    let mut br = BufReader::new(reader);
    let mut line = String::new();

    // Handshake: challenge, then verify the node's signature.
    let challenge = AuthChallenge {
        nonce: auth::generate_nonce(),
    };
    let mut payload = match serde_json::to_string(&challenge) {
        Ok(p) => p,
        Err(e) => return Response::error(format!("callback: encoding challenge: {e}")),
    };
    payload.push('\n');
    if writer.write_all(payload.as_bytes()).await.is_err() || writer.flush().await.is_err() {
        return Response::error("callback: sending challenge failed");
    }

    if br.read_line(&mut line).await.is_err() {
        return Response::error("callback: reading AuthResponse failed");
    }
    let auth_response: AuthResponse = match serde_json::from_str(line.trim()) {
        Ok(r) => r,
        Err(e) => return Response::error(format!("callback: bad AuthResponse: {e}")),
    };
    if !auth::verify(secret, &auth_response.nonce, &auth_response.signature) {
        return Response::error("callback handshake: bad signature");
    }

    // Send the plan (the exact line the intent's sha256 is over, + terminator).
    let mut payload = request_line.to_string();
    payload.push('\n');
    if writer.write_all(payload.as_bytes()).await.is_err() || writer.flush().await.is_err() {
        return Response::error("callback: sending the plan failed");
    }

    line.clear();
    if br.read_line(&mut line).await.is_err() {
        return Response::error("callback: reading the node response failed");
    }
    match serde_json::from_str(line.trim()) {
        Ok(r) => r,
        Err(e) => Response::error(format!("callback: bad response line: {e}")),
    }
}

/// A roster spread: apply the resolved plan to each roster node in turn over
/// the epidemic handshake. The caller has already resolved the roster (group
/// nodes, ad-hoc endpoints, and any `--discover` union), the concrete plan,
/// and any dry-run output — this function only does the spread and audit.
pub struct RosterSpread {
    /// The roster to spread to (group nodes + ad-hoc endpoints, deduped).
    pub roster: Vec<NodeConfig>,
    /// The named target group (for the audit record).
    pub group: Option<String>,
    /// The group's optional `secret_path` (secret-resolution fallback).
    pub group_secret_path: Option<String>,
    /// The resolved concrete plan (identical for every node).
    pub plan: DeploymentPlan,
    /// The epidemic secret source (inline / file), or `Default` for the
    /// group/default-path/generated fallback chain.
    pub secret: SecretSource,
    /// TLS for the coordinator -> node hop (disabled by default).
    pub tls: TlsOptions,
    /// Retries per node after a transient (transport) failure (4b;
    /// default [`DEFAULT_APPLY_RETRIES`]). A node verdict is never retried.
    pub retries: u32,
    /// Per-attempt timeout so a hung node is a retriable failure instead of
    /// wedging the spread (4b; default [`DEFAULT_APPLY_TIMEOUT`]).
    pub timeout: Duration,
    /// Shared rate limit on spread *starts* (4c; default
    /// [`ratelimit::DEFAULT_RATE_LIMIT_MAX`]/window). Consulted before any
    /// node is touched; a `RateLimited` refusal is returned before the fan-out.
    pub rate_limit: SpreadRateLimit,
    /// A fixed spread id (caller-generated when it must be known up front,
    /// e.g. before streaming progress); a fresh one is generated otherwise.
    pub spread_id: Option<String>,
    /// Where to append the audit record
    /// (default: [`history::default_history_path`]).
    pub history_path: Option<PathBuf>,
    /// Live progress sink (the REST layer forwards each event to the daemon
    /// event bus; the CLI passes `None`).
    pub progress: Option<mpsc::UnboundedSender<SpreadProgress>>,
}

/// Run a roster spread: apply the plan to every node, stream per-node results,
/// append the audit record to the history, and report the outcome.
pub async fn run_roster_spread(opts: &RosterSpread) -> Result<SpreadResult> {
    // 4c: consult the shared rate limit on spread *starts* before touching any
    // node. A refusal is returned here — reported (CLI non-zero exit / REST 429
    // with retry-after) — without emitting progress or dialing a single node.
    opts.rate_limit.record()?;

    let spread_id = opts.spread_id.clone().unwrap_or_else(generate_spread_id);

    // The identical concrete plan goes to every node; serialize it once so
    // the record's sha256 is the digest of exactly what is sent.
    let name = opts.plan.spec.meta.name.clone();
    let version = opts.plan.spec.meta.version.clone();
    let request = AgentRequest::ApplyDeployment {
        name: name.clone(),
        version: version.clone(),
        variables: opts.plan.shared.clone(),
        infections: deployment_apply_infections(&opts.plan),
    };
    let request_line =
        serde_json::to_string(&request).with_context(|| "encoding the plan for the nodes")?;
    let plan_sha = sha256_hex(&request_line);

    // 5c: announce the spread (what is being applied where).
    emit(
        opts.progress.as_ref(),
        SpreadProgress::Started {
            spread_id: spread_id.clone(),
            mode: SpreadMode::Roster,
            stage: "full".to_string(),
            name: name.clone(),
            version: version.clone(),
            sha256: plan_sha.clone(),
            group: opts.group.clone(),
            nodes: opts
                .roster
                .iter()
                .map(|n| SpreadTarget {
                    name: n.name.clone(),
                    addr: n.addr.clone(),
                })
                .collect(),
            criteria: Vec::new(),
            canary: None,
        },
    );

    // Resolve the epidemic (network) secret for the coordinator -> node hop.
    let secret = resolve_epidemic_secret(&opts.secret, opts.group_secret_path.as_deref())?;

    // Apply to each node in turn, streaming each outcome as it arrives.
    // Transient failures are retried with backoff (4b); a node verdict is
    // final. The outcome carries the attempt count for the record + progress.
    let mut results: Vec<(NodeConfig, ApplyOutcome)> = Vec::new();
    for node in &opts.roster {
        let node_tls = node_tls_client(&opts.tls, &node.addr)?;
        let outcome = apply_with_retries(
            &node.addr,
            &secret.value,
            &request,
            node_tls.as_ref(),
            opts.retries,
            opts.timeout,
        )
        .await;
        emit(
            opts.progress.as_ref(),
            SpreadProgress::Node {
                spread_id: spread_id.clone(),
                name: node.name.clone(),
                addr: node.addr.clone(),
                ok: outcome.ok,
                error: outcome.error.clone(),
                attempts: outcome.attempts,
            },
        );
        results.push((node.clone(), outcome));
    }

    // The audit record: a partial spread is recorded *as a failure* and then
    // reported as one (the CLI / REST decide the exit status and message).
    let record = roster_record(
        &spread_id,
        opts.group.as_deref(),
        &name,
        &version,
        &plan_sha,
        &results,
    );
    emit(
        opts.progress.as_ref(),
        SpreadProgress::Finished {
            spread_id: spread_id.clone(),
            applied: record.applied,
            failed: record.failed,
            ok: record.ok,
        },
    );

    // Append the record (a failure here is an error: the spread happened but
    // the audit trail did not land).
    let history_path = opts
        .history_path
        .clone()
        .unwrap_or_else(history::default_history_path);
    history::append_record(&history_path, &record)?;

    Ok(SpreadResult {
        record,
        generated_secret: secret.generated.then(|| secret.value.clone()),
    })
}

/// A broadcast spread (increment 3): sign a multicast intent, run the callback
/// listener, and collect the nodes that self-select and dial back.
pub struct BroadcastSpread {
    /// The resolved concrete plan (identical for every node).
    pub plan: DeploymentPlan,
    /// The epidemic secret source, or `Default` for the fallback chain.
    pub secret: SecretSource,
    /// Targeting criteria, `key=value`, AND semantics (empty = every node).
    pub criteria: Vec<String>,
    /// The canary cohort (absent for a plain full spread).
    pub canary: Option<Canary>,
    /// Promote an existing canary: re-broadcast the same spread id at Full.
    pub promote: bool,
    /// A fixed spread id (required by `promote`; a fresh one otherwise).
    pub spread_id: Option<String>,
    /// Multicast group to broadcast on (site-local).
    pub multicast_group: Ipv4Addr,
    /// Multicast UDP port.
    pub multicast_port: u16,
    /// How long to wait for node callbacks before reporting the result.
    pub wait_secs: u64,
    /// The LAN interface to advertise as the callback origin
    /// (absent: the primary LAN address is inferred).
    pub interface: Option<Ipv4Addr>,
    /// Shared rate limit on spread *starts* (4c; default
    /// [`ratelimit::DEFAULT_RATE_LIMIT_MAX`]/window). Consulted before the
    /// intent is sent; a `RateLimited` refusal is returned before any node.
    pub rate_limit: SpreadRateLimit,
    /// Where to append the audit record
    /// (default: [`history::default_history_path`]).
    pub history_path: Option<PathBuf>,
    /// Live progress sink (the REST layer forwards each event to the daemon
    /// event bus; the CLI passes `None`).
    pub progress: Option<mpsc::UnboundedSender<SpreadProgress>>,
}

/// Run a broadcast spread: sign + send the intent, serve the callbacks,
/// stream per-node results, append the audit record, and report the outcome.
pub async fn run_broadcast_spread(opts: &BroadcastSpread) -> Result<SpreadResult> {
    // 4c: consult the shared rate limit on spread *starts* before sending the
    // intent — a refusal is reported before any node self-selects.
    opts.rate_limit.record()?;

    let name = opts.plan.spec.meta.name.clone();
    let version = opts.plan.spec.meta.version.clone();

    // One concrete plan; the callback sends exactly this line and the
    // intent's `plan.sha256` is the digest of it (no terminator).
    let request = AgentRequest::ApplyDeployment {
        name: name.clone(),
        version: version.clone(),
        variables: opts.plan.shared.clone(),
        infections: deployment_apply_infections(&opts.plan),
    };
    let request_line = serde_json::to_string(&request)?;
    let plan_sha = sha256_hex(&request_line);

    let secret = resolve_epidemic_secret(&opts.secret, None)?;

    let spread_id = opts.spread_id.clone().unwrap_or_else(generate_spread_id);
    let stage = if opts.promote {
        SpreadStage::Full
    } else if opts.canary.is_some() {
        SpreadStage::Canary
    } else {
        SpreadStage::Full
    };
    let stage_str = match (stage, opts.promote) {
        (SpreadStage::Full, true) => "promote",
        (SpreadStage::Full, false) => "full",
        (SpreadStage::Canary, _) => "canary",
    };

    let origin = opts.interface.or_else(primary_lan_ip).ok_or_else(|| {
        anyhow!("no LAN IPv4 to advertise as the callback origin — specify the interface")
    })?;

    // 5c: announce the spread (broadcast has no roster — nodes self-select).
    emit(
        opts.progress.as_ref(),
        SpreadProgress::Started {
            spread_id: spread_id.clone(),
            mode: SpreadMode::Broadcast,
            stage: stage_str.to_string(),
            name: name.clone(),
            version: version.clone(),
            sha256: plan_sha.clone(),
            group: None,
            nodes: Vec::new(),
            criteria: opts.criteria.clone(),
            canary: opts.canary.clone(),
        },
    );

    // Callback listener on an ephemeral port (the node dials this).
    let listener = TcpListener::bind("0.0.0.0:0").await?;
    let callback_port = listener.local_addr()?.port();

    let intent = build_intent(
        &secret.value,
        &spread_id,
        stage,
        &name,
        &version,
        &plan_sha,
        opts.criteria.clone(),
        opts.canary.clone(),
        origin.to_string(),
        callback_port,
        now_unix_secs(),
    );

    // Spawn the accept loop; each selected node becomes one connection.
    let results: Arc<Mutex<Vec<(SocketAddr, Response)>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let results = results.clone();
        let secret = secret.value.clone();
        let request_line = request_line.clone();
        let progress = opts.progress.clone();
        let spread_id = spread_id.clone();
        tokio::spawn(async move {
            loop {
                let (stream, peer) = match listener.accept().await {
                    Ok(x) => x,
                    Err(_) => break,
                };
                let results = results.clone();
                let secret = secret.clone();
                let request_line = request_line.clone();
                let progress = progress.clone();
                let spread_id = spread_id.clone();
                tokio::spawn(async move {
                    let response = serve_one_callback(stream, &secret, &request_line).await;
                    // 5c: stream this node's outcome as it arrives.
                    if let Some(tx) = &progress {
                        let (ok, error) = match &response {
                            Response::Success { .. } => (true, None),
                            Response::Error { message } => (false, Some(message.clone())),
                            Response::NotFound { message } => (false, Some(message.clone())),
                        };
                        let _ = tx.send(SpreadProgress::Node {
                            spread_id: spread_id.clone(),
                            name: peer.to_string(),
                            addr: peer.to_string(),
                            ok,
                            error,
                            // Broadcast has no coordinator-side retry: one
                            // callback per node, so a single attempt.
                            attempts: 1,
                        });
                    }
                    results.lock().await.push((peer, response));
                });
            }
        });
    }

    // Broadcast the intent. UDP is best-effort, so re-send a few times;
    // nodes de-duplicate by spread_id, so repeats are harmless.
    for _ in 0..INTENT_RESENDS {
        send_intent(
            opts.multicast_group,
            opts.multicast_port,
            opts.interface,
            &intent,
        )?;
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    // Wait for callbacks: stop after a short quiescence once at least one
    // node has answered, or at the deadline (whichever comes first).
    let deadline = std::time::Instant::now() + Duration::from_secs(opts.wait_secs);
    let mut last_count = 0usize;
    let mut last_change: Option<std::time::Instant> = None;
    loop {
        let count = results.lock().await.len();
        if count != last_count {
            last_count = count;
            last_change = Some(std::time::Instant::now());
        }
        let settled = last_change
            .map(|t| t.elapsed() >= Duration::from_secs(2))
            .unwrap_or(false);
        if settled || std::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Evaluate the callbacks for this stage and shape the audit record.
    let callbacks = std::mem::take(&mut *results.lock().await);
    let nodes = broadcast_node_results(&callbacks);
    let applied = nodes.iter().filter(|n| n.ok).count() as u32;
    let failed = (nodes.len() - applied as usize) as u32;
    let ok = failed == 0 && (applied > 0 || opts.promote);

    let record = SpreadRecord {
        timestamp: intent.issued_at,
        spread_id: spread_id.clone(),
        mode: SpreadMode::Broadcast,
        stage: stage_str.to_string(),
        name,
        version: intent.plan.version.clone(),
        sha256: intent.plan.sha256.clone(),
        group: None,
        origin: Some(intent.origin.clone()),
        criteria: intent.criteria.clone(),
        canary: opts.canary.clone(),
        nodes,
        applied,
        failed,
        ok,
    };

    emit(
        opts.progress.as_ref(),
        SpreadProgress::Finished {
            spread_id: spread_id.clone(),
            applied: record.applied,
            failed: record.failed,
            ok: record.ok,
        },
    );

    let history_path = opts
        .history_path
        .clone()
        .unwrap_or_else(history::default_history_path);
    history::append_record(&history_path, &record)?;

    Ok(SpreadResult {
        record,
        generated_secret: secret.generated.then(|| secret.value.clone()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verify_intent_token;
    use tempfile::tempdir;

    #[test]
    fn roster_is_adhoc_when_no_group() {
        let (roster, secret_path) = resolve_roster(
            None,
            &["10.0.0.1:7711".to_string(), "10.0.0.2:7711".to_string()],
        )
        .unwrap();
        assert_eq!(roster.len(), 2);
        assert_eq!(roster[0].addr, "10.0.0.1:7711");
        // Ad-hoc nodes name themselves by address.
        assert_eq!(roster[0].name, "10.0.0.1:7711");
        assert!(secret_path.is_none());
    }

    #[test]
    fn roster_merges_group_and_adhoc_deduped() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("groups.toml");
        std::fs::write(
            &path,
            r#"
[[group]]
name = "edge"
secret_path = "/tmp/edge-secret"

[[group.node]]
name = "edge-1"
addr = "10.0.0.1:7711"
"#,
        )
        .unwrap();

        // Reuse the shared loader/merge so the coordinator path is what's tested.
        let groups = load_groups_or_default(Some(path.as_path())).unwrap();
        let g = find_group(&groups, "edge").unwrap();
        let roster = merge_roster(
            Some(g),
            &["10.0.0.1:7711".to_string(), "10.0.0.9:7711".to_string()],
        );
        // edge-1 (group) + 10.0.0.9 (ad-hoc); the duplicate 10.0.0.1 is deduped.
        assert_eq!(roster.len(), 2);
        assert_eq!(roster[0].name, "edge-1");
        assert_eq!(roster[1].name, "10.0.0.9:7711");
        assert_eq!(g.secret_path.as_deref(), Some("/tmp/edge-secret"));
    }

    #[test]
    fn discovered_nodes_merge_deduped_by_addr() {
        let mut roster = vec![
            NodeConfig {
                name: "edge-1".to_string(),
                addr: "10.0.0.1:7711".to_string(),
            },
            NodeConfig {
                name: "10.0.0.2:7711".to_string(),
                addr: "10.0.0.2:7711".to_string(),
            },
        ];
        let discovered = vec![
            // Same address as a roster entry → not added (roster name wins).
            DiscoveredNode {
                name: "edge-1".to_string(),
                addr: "10.0.0.1:7711".to_string(),
            },
            // New address → added, keeping its mDNS instance name.
            DiscoveredNode {
                name: "edge-9".to_string(),
                addr: "10.0.0.9:7711".to_string(),
            },
        ];

        assert_eq!(merge_discovered(&mut roster, &discovered), 1);
        assert_eq!(roster.len(), 3);
        assert_eq!(roster[0].name, "edge-1");
        assert_eq!(roster[2].name, "edge-9");
        assert_eq!(roster[2].addr, "10.0.0.9:7711");
    }

    #[test]
    fn discovered_nodes_merge_is_idempotent() {
        let mut roster = vec![NodeConfig {
            name: "edge-1".to_string(),
            addr: "10.0.0.1:7711".to_string(),
        }];
        let discovered = vec![DiscoveredNode {
            name: "edge-1".to_string(),
            addr: "10.0.0.1:7711".to_string(),
        }];

        assert_eq!(merge_discovered(&mut roster, &discovered), 0);
        assert_eq!(merge_discovered(&mut roster, &discovered), 0);
        assert_eq!(roster.len(), 1);
    }

    #[test]
    fn unknown_group_is_an_error() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("groups.toml");
        std::fs::write(&path, "").unwrap();
        let groups = load_groups_or_default(Some(path.as_path())).unwrap();
        assert!(find_group(&groups, "nope").is_none());
    }

    // ── Broadcast (increment 3) ──────────────────────────────────────────

    #[test]
    fn parse_canary_arg_percentage_and_subset() {
        assert_eq!(
            parse_canary_arg("25").unwrap(),
            Canary::Percentage { pct: 25 }
        );
        assert_eq!(
            parse_canary_arg("25%").unwrap(),
            Canary::Percentage { pct: 25 }
        );
        assert_eq!(
            parse_canary_arg(" 25 ").unwrap(),
            Canary::Percentage { pct: 25 }
        );
        assert_eq!(
            parse_canary_arg("role=canary").unwrap(),
            Canary::Subset {
                criterion: "role=canary".to_string()
            }
        );
        // A capability criterion is also a valid named subset.
        matches!(
            parse_canary_arg("cap:mqtt=true").unwrap(),
            Canary::Subset { .. }
        );
        // Errors.
        assert!(parse_canary_arg("").is_err());
        assert!(parse_canary_arg("256").is_err(), "255 is the u8 ceiling");
        assert!(parse_canary_arg("nonsense").is_err());
        assert!(parse_canary_arg("noequals").is_err());
    }

    #[test]
    fn validate_broadcast_combos() {
        // A plain broadcast (no roster, no canary/promote) is fine.
        assert!(validate_broadcast(true, None, &[], false, &[], None, false, None).is_ok());
        // Broadcast takes no roster.
        assert!(
            validate_broadcast(true, Some("edge"), &[], false, &[], None, false, None).is_err()
        );
        assert!(validate_broadcast(
            true,
            None,
            &["1.1.1.1:7711".to_string()],
            false,
            &[],
            None,
            false,
            None
        )
        .is_err());
        assert!(validate_broadcast(true, None, &[], true, &[], None, false, None).is_err());
        // --promote and --canary are mutually exclusive; --promote needs an id.
        assert!(validate_broadcast(
            true,
            None,
            &[],
            false,
            &[],
            Some(&Canary::Percentage { pct: 10 }),
            true,
            Some("id")
        )
        .is_err());
        assert!(validate_broadcast(true, None, &[], false, &[], None, true, None).is_err());
        // A bad criterion is rejected even in broadcast mode.
        assert!(validate_broadcast(
            true,
            None,
            &[],
            false,
            &["noequals".to_string()],
            None,
            false,
            None
        )
        .is_err());
        // Non-broadcast spreads reject the broadcast-only flags.
        assert!(validate_broadcast(
            false,
            None,
            &[],
            false,
            &[],
            Some(&Canary::Percentage { pct: 10 }),
            false,
            None
        )
        .is_err());
        assert!(validate_broadcast(false, None, &[], false, &[], None, true, None).is_err());
        assert!(validate_broadcast(false, None, &[], false, &[], None, false, Some("id")).is_err());
        assert!(validate_broadcast(
            false,
            None,
            &[],
            false,
            &["role=edge".to_string()],
            None,
            false,
            None
        )
        .is_err());
    }

    #[test]
    fn build_intent_sets_token_stage_and_sha() {
        let secret = "epi-secret";
        let spread_id = "8f3a1c02d4e5b6a7";
        let sha = "cd".repeat(32);
        let intent = build_intent(
            secret,
            spread_id,
            SpreadStage::Canary,
            "webapp",
            "1.2.3",
            &sha,
            vec!["role=edge".to_string()],
            Some(Canary::Percentage { pct: 25 }),
            "192.168.1.5".to_string(),
            41731,
            1_750_000_000,
        );
        assert_eq!(intent.version, INTENT_VERSION);
        assert_eq!(intent.spread_id, spread_id);
        assert_eq!(intent.stage, SpreadStage::Canary);
        assert_eq!(intent.token, intent_token(secret, spread_id));
        // The token is the membership proof: it verifies against the shared
        // secret and against nothing else.
        assert!(verify_intent_token(secret, &intent));
        assert!(!verify_intent_token("other-secret", &intent));
        assert_eq!(intent.plan.name, "webapp");
        assert_eq!(intent.plan.version, "1.2.3");
        assert_eq!(intent.plan.sha256, sha);
        assert_eq!(intent.criteria, vec!["role=edge".to_string()]);
        assert_eq!(intent.canary, Some(Canary::Percentage { pct: 25 }));
        assert_eq!(intent.origin, "192.168.1.5");
        assert_eq!(intent.callback_port, 41731);
        assert_eq!(intent.issued_at, 1_750_000_000);
        assert!(intent.group.is_none());
    }

    #[test]
    fn roster_record_shapes_the_audit_entry() {
        let results = vec![
            (
                NodeConfig {
                    name: "edge-1".to_string(),
                    addr: "10.0.0.1:7711".to_string(),
                },
                ApplyOutcome {
                    ok: true,
                    error: None,
                    attempts: 1,
                },
            ),
            (
                NodeConfig {
                    name: "edge-2".to_string(),
                    addr: "10.0.0.2:7711".to_string(),
                },
                ApplyOutcome {
                    ok: false,
                    error: Some("node 10.0.0.2:7711 failed to apply".to_string()),
                    attempts: 3,
                },
            ),
        ];

        let rec = roster_record("sid-1", Some("edge"), "webapp", "1.2.3", "cdcd", &results);
        assert_eq!(rec.mode, SpreadMode::Roster);
        assert_eq!(rec.stage, "full");
        assert_eq!(rec.spread_id, "sid-1");
        assert_eq!(rec.group.as_deref(), Some("edge"));
        assert_eq!(rec.sha256, "cdcd");
        assert!(rec.origin.is_none());
        assert!(rec.canary.is_none());
        assert!(rec.criteria.is_empty());
        assert_eq!(rec.nodes.len(), 2);
        assert_eq!(rec.nodes[0].name, "edge-1");
        assert!(rec.nodes[0].ok);
        assert_eq!(rec.nodes[0].attempts, 1);
        assert!(rec.nodes[1].error.is_some());
        assert_eq!(
            rec.nodes[1].attempts, 3,
            "a retried node reports its attempts"
        );
        assert_eq!(rec.applied, 1);
        assert_eq!(rec.failed, 1);
        assert!(!rec.ok, "a partial spread must not look like success");

        // All nodes ok → ok, and an ad-hoc roster has no group.
        let all_ok: Vec<(NodeConfig, ApplyOutcome)> = results
            .iter()
            .map(|(n, _)| {
                (
                    n.clone(),
                    ApplyOutcome {
                        ok: true,
                        error: None,
                        attempts: 1,
                    },
                )
            })
            .collect();
        let rec2 = roster_record("sid-2", None, "webapp", "1.2.3", "cdcd", &all_ok);
        assert!(rec2.ok);
        assert_eq!(rec2.failed, 0);
        assert!(rec2.group.is_none());

        // An empty result set is not a success either.
        let rec3 = roster_record("sid-3", None, "webapp", "1.2.3", "cdcd", &[]);
        assert!(!rec3.ok);
    }

    #[test]
    fn broadcast_node_results_maps_each_callback() {
        let callbacks: Vec<(SocketAddr, Response)> = vec![
            (
                "10.0.0.2:53211".parse().unwrap(),
                Response::success_with_data(serde_json::Value::Null),
            ),
            ("10.0.0.3:53212".parse().unwrap(), Response::error("boom")),
            (
                "10.0.0.4:53213".parse().unwrap(),
                Response::not_found("missing"),
            ),
        ];

        let nodes = broadcast_node_results(&callbacks);
        assert_eq!(nodes.len(), 3);
        assert!(nodes[0].ok);
        assert!(nodes[0].error.is_none());
        assert!(!nodes[1].ok);
        assert_eq!(nodes[1].error.as_deref(), Some("boom"));
        assert!(!nodes[2].ok);
        assert_eq!(nodes[2].error.as_deref(), Some("missing"));
        // The coordinator only knows the peer address on this path.
        assert_eq!(nodes[1].name, "10.0.0.3:53212");
        assert_eq!(nodes[1].addr, "10.0.0.3:53212");
    }

    #[test]
    fn host_of_splits_host_and_port() {
        assert_eq!(host_of("10.0.0.1:7711"), "10.0.0.1");
        assert_eq!(host_of("edge-1.local:8080"), "edge-1.local");
        // A bare host (no port) is returned as-is.
        assert_eq!(host_of("edge-1.local"), "edge-1.local");
    }

    // ── 4b retry engine: a fake node speaking the real wire protocol ────────

    use std::sync::atomic::{AtomicUsize, Ordering};

    /// What the fake node answers to an `ApplyDeployment` request.
    #[derive(Clone, Copy)]
    enum FakeApply {
        /// `Success` — the node applied the deployment.
        Succeed,
        /// `Error { message }` — a verdict (the retry loop must not re-ask).
        Verdict(&'static str),
        /// Accept the request but never reply — the coordinator's per-attempt
        /// timeout is what turns this into a transient failure.
        Hang,
    }

    /// A fake node: a TCP listener speaking the real
    /// `AuthChallenge`/`AuthResponse` + request/response protocol. While the
    /// connection count is below `drop_first`, it accepts and immediately
    /// disconnects (a *dropped* node — a transient failure). Otherwise it
    /// completes the handshake (verifying the client's signature, like a real
    /// node would), answers `GetCapabilities` with a capability list, and
    /// answers `ApplyDeployment` per `apply`. Returns the endpoint and a
    /// live count of accepted connections.
    async fn spawn_fake_node(
        secret: &str,
        apply: FakeApply,
        drop_first: u32,
    ) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let conns = Arc::new(AtomicUsize::new(0));
        let conns_c = conns.clone();
        let secret = secret.to_string();
        tokio::spawn(async move {
            loop {
                let (stream, _peer) = match listener.accept().await {
                    Ok(s) => s,
                    Err(_) => break,
                };
                let n = conns_c.fetch_add(1, Ordering::SeqCst) as u32;
                let secret = secret.clone();
                if n < drop_first {
                    // A dropped node: accept, then close before the handshake.
                    drop(stream);
                    continue;
                }
                tokio::spawn(async move { serve_fake_conn(stream, &secret, apply).await });
            }
        });
        (addr, conns)
    }

    /// One fake-node connection: challenge → verify signed response → read the
    /// request → reply (`GetCapabilities` or `ApplyDeployment` per policy).
    async fn serve_fake_conn(stream: TcpStream, secret: &str, apply: FakeApply) {
        // 1. Send the challenge (the client reads it first).
        let nonce = format!("fake-nonce-{}", std::process::id());
        let mut challenge = serde_json::to_string(&AuthChallenge {
            nonce: nonce.clone(),
        })
        .unwrap();
        challenge.push('\n');
        let mut out = stream;
        if out.write_all(challenge.as_bytes()).await.is_err() {
            return;
        }
        if out.flush().await.is_err() {
            return;
        }
        // 2. Read the client's signed response; verify it (a real node would).
        let mut reader = BufReader::new(out);
        let mut line = String::new();
        if reader.read_line(&mut line).await.ok().unwrap_or(0) == 0 {
            return;
        }
        let resp: AuthResponse = match serde_json::from_str(line.trim()) {
            Ok(r) => r,
            Err(_) => return,
        };
        if resp.nonce != nonce || !auth::verify(secret, &nonce, &resp.signature) {
            // A wrong signature is refused exactly like a forged deployment.
            return;
        }
        // 3. Read the request.
        let mut req_line = String::new();
        if reader.read_line(&mut req_line).await.ok().unwrap_or(0) == 0 {
            return;
        }
        let req: AgentRequest = match serde_json::from_str(req_line.trim()) {
            Ok(r) => r,
            Err(_) => return,
        };
        let reply = match req {
            AgentRequest::GetCapabilities => Response::Success {
                data: Some(serde_json::json!({ "capabilities": ["systemd"] })),
            },
            AgentRequest::ApplyDeployment { .. } => match apply {
                FakeApply::Succeed => Response::success(),
                FakeApply::Verdict(msg) => Response::Error {
                    message: msg.to_string(),
                },
                FakeApply::Hang => {
                    // Hold the connection open (reader still owns the stream)
                    // and never reply: the coordinator's per-attempt timeout
                    // is what turns this into a transient failure.
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    return;
                }
            },
            _ => Response::success(),
        };
        // 4. Reply on the same stream.
        let mut out = reader.into_inner();
        let mut payload = serde_json::to_string(&reply).unwrap();
        payload.push('\n');
        let _ = out.write_all(payload.as_bytes()).await;
        let _ = out.flush().await;
    }

    fn apply_request() -> AgentRequest {
        AgentRequest::ApplyDeployment {
            name: "web".to_string(),
            version: "1.0.0".to_string(),
            variables: std::collections::BTreeMap::new(),
            infections: Vec::new(),
        }
    }

    #[tokio::test]
    async fn retry_recovers_after_transient_failures() {
        // The node drops the first two attempts (a flapping connection), then
        // serves normally: the spread must succeed on the third attempt.
        let (addr, _conns) = spawn_fake_node("s3cr3t", FakeApply::Succeed, 2).await;
        let out = apply_with_retries(
            &addr,
            "s3cr3t",
            &apply_request(),
            None,
            2,
            Duration::from_secs(5),
        )
        .await;
        assert!(out.ok, "expected recovery: {out:?}");
        assert_eq!(out.attempts, 3, "two drops + one success");
    }

    #[tokio::test]
    async fn retry_gives_up_after_max_retries() {
        // A node that never answers (always dropped) is retried `retries`
        // more times and then reported as a failure — not silently lost.
        let (addr, _conns) = spawn_fake_node("s3cr3t", FakeApply::Succeed, 1_000_000).await;
        let out = apply_with_retries(
            &addr,
            "s3cr3t",
            &apply_request(),
            None,
            2,
            Duration::from_secs(2),
        )
        .await;
        assert!(!out.ok);
        assert!(out.error.is_some(), "a failed apply reports why it failed");
        assert_eq!(out.attempts, 3, "1 initial + 2 retries");
    }

    #[tokio::test]
    async fn verdict_is_not_retried() {
        // A node that *answers* "no" (a verdict) gets the same answer every
        // time, so the retry loop stops after the very first attempt.
        let (addr, _conns) =
            spawn_fake_node("s3cr3t", FakeApply::Verdict("unit is masked"), 0).await;
        let out = apply_with_retries(
            &addr,
            "s3cr3t",
            &apply_request(),
            None,
            2,
            Duration::from_secs(5),
        )
        .await;
        assert!(!out.ok);
        assert_eq!(out.error.as_deref(), Some("unit is masked"));
        assert_eq!(out.attempts, 1, "a verdict is final — never re-asked");
    }

    #[tokio::test]
    async fn timeout_is_transient_and_retried() {
        // A node that accepts but never replies is a *hung* node: the
        // per-attempt timeout turns it into a transient failure, so it is
        // retried and then reported (rather than wedging the spread).
        let (addr, _conns) = spawn_fake_node("s3cr3t", FakeApply::Hang, 0).await;
        let out = apply_with_retries(
            &addr,
            "s3cr3t",
            &apply_request(),
            None,
            1,
            Duration::from_millis(150),
        )
        .await;
        assert!(!out.ok);
        assert!(
            out.error.as_deref().unwrap().contains("timed out"),
            "got: {out:?}"
        );
        assert_eq!(
            out.attempts, 2,
            "1 initial + 1 retry, each bounded by the timeout"
        );
    }
}
