//! Epidemic: spread a resolved deployment to a group of nodes.
//!
//! The **coordinator** (this command) is the third member of the node / group /
//! coordinator primitive. It holds a group's roster + epidemic secret, runs the
//! pure `Plan` step locally (shared with `deployment install`), then applies the
//! identical concrete plan to every node in the roster through a
//! [`pandemic-node`] receiver — each of which forwards to its own local agent.
//!
//! Two distinct secrets are in play, exactly as on the node:
//!   * the **epidemic** secret guards the coordinator -> node handshake;
//!   * the **agent** secret is the node's business (it holds it; the coordinator
//!     never sees it).
//!
//! `--discover` (increment 2) probes the LAN over mDNS: `epidemic nodes
//! --discover` lists the nodes advertising `_pandemic-node._tcp.local`, and
//! `epidemic spread --discover` unions them with the group/ad-hoc roster
//! (deduplicated by address, same as everywhere else).
//!
//! **Broadcast (increment 3):** `epidemic spread --broadcast` carries only a
//! small *intent* datagram over the multicast group (default
//! `239.255.77.11:7712`, TTL 1 — it stays on the link). No payload and no
//! secret travel in cleartext: the intent carries a
//! `token = HMAC-SHA256(epidemic_secret, spread_id)` (membership proof) and a
//! `sha256` of the concrete plan. Nodes self-select on `key=value` criteria
//! and, for canary spreads, on a stateless cohort; a selected node dials the
//! coordinator's callback listener, signs the handshake, receives the
//! `ApplyDeployment` (verifying its digest), and applies it through the local
//! agent. `--canary` + `--promote --spread-id` give the staged rollout;
//! `epidemic spreads` lists recent broadcast history.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use pandemic_common::discovery::{self, DiscoveredNode};
use pandemic_common::groups::{
    default_groups_path, find_group, load_groups_or_default, merge_roster,
};
use pandemic_common::{
    auth, generate_spread_id, history, intent_token, parse_criteria, send_intent, sha256_hex,
    Criterion, GroupConfig, NodeConfig, RemoteClient, TlsClient, DEFAULT_MULTICAST_GROUP,
    DEFAULT_MULTICAST_PORT, INTENT_RESENDS, INTENT_VERSION,
};
use pandemic_protocol::{
    AgentRequest, AuthChallenge, AuthResponse, Canary, PlanIdentity, Response, SpreadIntent,
    SpreadMode, SpreadNodeResult, SpreadRecord, SpreadStage,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

use crate::apply::deployment_apply_infections;
use crate::deployment::{print_dry_run, resolve_deployment_plan};

/// The epidemic (network) secret as supplied on the `epidemic` verb: an inline
/// value and/or a path to a file holding it. Bundled so it travels as one
/// unit through the coordinator's call chain.
#[derive(Default, Clone)]
struct EpidemicSecret {
    inline: Option<String>,
    path: Option<PathBuf>,
}

/// The `--discover` probe options, shared by `spread` and `nodes`. Bundled so
/// the three related flags travel as one unit through the call chain.
#[derive(Clone, Copy)]
struct Discovery {
    enabled: bool,
    /// Interface to probe on (an IPv4 address), or `None` for all interfaces.
    interface: Option<Ipv4Addr>,
    /// Probe timeout in seconds.
    timeout_secs: u64,
}

impl Discovery {
    fn duration(self) -> Duration {
        Duration::from_secs(self.timeout_secs)
    }
}

/// Resolved `--broadcast` options (defaults already applied), shared by the
/// spread handler and the intent builder. Bundled so the related flags travel
/// as one unit through the call chain (the same pattern as [`Discovery`]).
#[derive(Clone)]
struct Broadcast {
    /// Targeting criteria, `key=value`, AND semantics (empty = every node).
    criteria: Vec<String>,
    /// The canary cohort, or `None` for a plain full spread.
    canary: Option<Canary>,
    /// Promote an existing canary: re-broadcast the same `spread_id` at Full.
    promote: bool,
    /// A fixed spread id (required by `--promote`; fresh ones generated
    /// otherwise). See `epidemic spreads` for recent ids.
    spread_id: Option<String>,
    /// Multicast group (default [`DEFAULT_MULTICAST_GROUP`]).
    group: Ipv4Addr,
    /// Multicast UDP port (default [`DEFAULT_MULTICAST_PORT`]).
    port: u16,
    /// How long to wait for node callbacks before reporting the result.
    wait_secs: u64,
}

#[derive(clap::Subcommand)]
// clap CLI enums are large by nature (every option per subcommand), and the
// large fields can't be boxed — clap's derive needs the concrete types.
#[allow(clippy::large_enum_variant)]
pub enum EpidemicAction {
    /// Spread a deployment to a group of nodes (or an ad-hoc node list)
    Spread {
        /// Path to the deployment spec (deployment.toml), or a registry deployment name
        target: String,
        /// Named group (from the groups file) whose roster is the target list
        #[arg(long)]
        group: Option<String>,
        /// Ad-hoc node endpoint `host:port` (repeatable); merged with `--group`
        #[arg(long = "node")]
        node: Vec<String>,
        /// Registry URL to use (when `target` is a registry name)
        #[arg(long)]
        registry_url: Option<String>,
        /// Shared variable values: --set key=value (repeatable)
        #[arg(long = "set")]
        set: Vec<String>,
        /// Show the resolved plan + node list without applying anywhere
        #[arg(long)]
        dry_run: bool,
        /// Also target nodes found on the LAN via mDNS discovery
        #[arg(long)]
        discover: bool,
        /// mDNS probe interface (an IPv4 address, e.g. 127.0.0.1); default: all
        #[arg(long)]
        interface: Option<Ipv4Addr>,
        /// mDNS probe timeout in seconds
        #[arg(long, default_value_t = 3)]
        timeout: u64,

        // ── Broadcast (increment 3): multicast intent + node self-selection ──
        /// Spread over the multicast intent group instead of a roster. Nodes
        /// holding the group secret self-select on the criteria and dial back.
        #[arg(long)]
        broadcast: bool,
        /// Targeting criterion `KEY=VALUE` (repeatable, AND semantics).
        /// `name=X`, a node label, or `cap:NAME=true`; empty = every node.
        #[arg(long = "criteria", value_name = "KEY=VALUE")]
        criteria: Vec<String>,
        /// Canary cohort: a percentage (`25`) or a `KEY=VALUE` criterion
        /// (`role=canary`). Only matching nodes apply at the canary stage.
        #[arg(long)]
        canary: Option<String>,
        /// Promote a prior canary: re-broadcast `--spread-id` at Full so every
        /// matching node that has not applied yet does so.
        #[arg(long)]
        promote: bool,
        /// A fixed spread id (required for `--promote`; generated otherwise).
        /// See `epidemic spreads` for recent ids.
        #[arg(long = "spread-id")]
        spread_id: Option<String>,
        /// Multicast group to broadcast on (site-local; default 239.255.77.11)
        #[arg(long)]
        multicast_group: Option<Ipv4Addr>,
        /// UDP port to broadcast on (default 7712)
        #[arg(long)]
        multicast_port: Option<u16>,
        /// Seconds to wait for node callbacks before reporting the result
        #[arg(long, default_value_t = 15)]
        wait: u64,

        // ── TLS (increment 4): encrypt the coordinator -> node hop ──────────
        /// Encrypt the coordinator -> node hop with TLS (requires `--tls-ca`).
        #[arg(long)]
        tls: bool,
        /// Root CA (PEM) used to verify each node's TLS certificate.
        #[arg(long)]
        tls_ca: Option<PathBuf>,
        /// Server name each node's TLS cert must present. Default: the node's
        /// host (the `host` of its `host:port` address).
        #[arg(long)]
        tls_server_name: Option<String>,
    },
    /// List recent spreads (roster + broadcast, newest first)
    Spreads {
        /// How many recent spreads to show
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// List configured groups and their node rosters
    Nodes {
        /// Only show this group
        #[arg(long)]
        group: Option<String>,
        /// Also list nodes found on the LAN via mDNS discovery
        #[arg(long)]
        discover: bool,
        /// mDNS probe interface (an IPv4 address, e.g. 127.0.0.1); default: all
        #[arg(long)]
        interface: Option<Ipv4Addr>,
        /// mDNS probe timeout in seconds
        #[arg(long, default_value_t = 3)]
        timeout: u64,
    },
}

pub async fn handle_epidemic_command(
    action: EpidemicAction,
    epidemic_secret: Option<String>,
    epidemic_secret_path: Option<PathBuf>,
) -> Result<()> {
    match action {
        EpidemicAction::Spread {
            target,
            group,
            node,
            registry_url,
            set,
            dry_run,
            discover,
            interface,
            timeout,
            broadcast,
            criteria,
            canary,
            promote,
            spread_id,
            multicast_group,
            multicast_port,
            wait,
            tls,
            tls_ca,
            tls_server_name,
        } => {
            let secret = EpidemicSecret {
                inline: epidemic_secret,
                path: epidemic_secret_path,
            };
            let probe = Discovery {
                enabled: discover,
                interface,
                timeout_secs: timeout,
            };
            spread(
                &target,
                group.as_deref(),
                &node,
                registry_url,
                &set,
                dry_run,
                secret,
                probe,
                broadcast,
                criteria,
                canary.as_deref().map(parse_canary_arg).transpose()?,
                promote,
                spread_id,
                multicast_group.unwrap_or(DEFAULT_MULTICAST_GROUP),
                multicast_port.unwrap_or(DEFAULT_MULTICAST_PORT),
                wait,
                tls,
                tls_ca,
                tls_server_name,
            )
            .await
        }
        EpidemicAction::Spreads { limit } => spreads(limit).await,
        EpidemicAction::Nodes {
            group,
            discover,
            interface,
            timeout,
        } => {
            let probe = Discovery {
                enabled: discover,
                interface,
                timeout_secs: timeout,
            };
            nodes(group.as_deref(), probe).await
        }
    }
}

#[allow(clippy::too_many_arguments)]
// Top-level `epidemic spread` handler: it carries the full CLI flag set
// (target, roster sources, plan inputs, dry-run, secret, the mDNS probe, and
// the broadcast options). `--broadcast` routes to the multicast path.
async fn spread(
    target: &str,
    group: Option<&str>,
    adhoc: &[String],
    registry_url: Option<String>,
    set_args: &[String],
    dry_run: bool,
    epidemic_secret: EpidemicSecret,
    probe: Discovery,
    broadcast: bool,
    criteria: Vec<String>,
    canary: Option<Canary>,
    promote: bool,
    spread_id: Option<String>,
    mcast_group: Ipv4Addr,
    mcast_port: u16,
    wait_secs: u64,
    tls: bool,
    tls_ca: Option<PathBuf>,
    tls_server_name: Option<String>,
) -> Result<()> {
    // Broadcast flag validation (cheap, up front — before any network I/O).
    validate_broadcast_flags(
        broadcast,
        group,
        adhoc,
        probe.enabled,
        &criteria,
        canary.as_ref(),
        promote,
        spread_id.as_deref(),
    )?;

    // The `--broadcast` path: multicast intent + node self-selection. The
    // roster/discovery flags are excluded by validation above.
    if broadcast {
        return run_broadcast(
            target,
            set_args,
            registry_url,
            dry_run,
            epidemic_secret,
            probe.interface,
            &Broadcast {
                criteria,
                canary,
                promote,
                spread_id,
                group: mcast_group,
                port: mcast_port,
                wait_secs,
            },
        )
        .await;
    }

    // 1. Resolve the target roster: the named group's nodes merged with any
    //    ad-hoc `--node` endpoints (de-duplicated by addr, group first).
    let (mut roster, group_secret_path) = resolve_roster(group, adhoc)?;

    // 1b. `--discover`: probe the LAN over mDNS and union the result into the
    //     roster (deduped by addr; roster entries keep their configured names).
    //     A probe failure is a warning, not an error — the explicit roster is
    //     still spread to; if the roster ends up empty the check below fires.
    if probe.enabled {
        match discovery::discover_nodes(probe.interface, probe.duration()).await {
            Ok(discovered) => {
                let added = merge_discovered(&mut roster, &discovered);
                println!(
                    "discovered {} node(s) via mDNS ({}s probe), {} added to roster:",
                    discovered.len(),
                    probe.timeout_secs,
                    added
                );
                for d in &discovered {
                    println!("  {:<16} {}", d.name, d.addr);
                }
            }
            Err(e) => {
                eprintln!(
                    "WARN: mDNS discovery failed: {e}; spreading only to the explicit roster"
                );
            }
        }
    }

    if roster.is_empty() && !dry_run {
        return Err(anyhow::anyhow!(
            "no nodes to spread to — pass --node host:port, or --group NAME, or --discover (see `epidemic nodes`)"
        ));
    }

    // 2. Pure Plan step (shared with `deployment install`) — resolved, rendered,
    //    and validated once, locally. The same concrete plan goes to every node.
    let dp = resolve_deployment_plan(target, set_args, registry_url).await?;

    if dry_run {
        println!("nodes ({}):", roster.len());
        for n in &roster {
            println!("  {:<16} {}", n.name, n.addr);
        }
        println!();
        print_dry_run(&dp);
        return Ok(());
    }

    // 3. Resolve the epidemic (network) secret for the coordinator -> node hop.
    let secret = resolve_epidemic_secret(&epidemic_secret, group_secret_path.as_deref())?;

    // 3b. TLS (increment 4): build the shared trust config once (if --tls). The
    //     server name is attached per node below (default: the node's host),
    //     since a roster may mix endpoints.
    let tls_trust = if tls {
        let ca = tls_ca
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("--tls requires --tls-ca"))?;
        Some(TlsClient::trust_from_paths(ca)?)
    } else {
        None
    };

    // 4. Apply the identical plan to every node, collecting per-node results.
    let base_request = AgentRequest::ApplyDeployment {
        name: dp.spec.meta.name.clone(),
        version: dp.spec.meta.version.clone(),
        variables: dp.shared.clone(),
        infections: deployment_apply_infections(&dp),
    };

    let name = dp.spec.meta.name.clone();
    let version = dp.spec.meta.version.clone();
    let mut results: Vec<(NodeConfig, Result<()>)> = Vec::new();
    for node in &roster {
        let node_tls = if let Some(trust) = tls_trust.as_ref() {
            let server_name = tls_server_name
                .clone()
                .unwrap_or_else(|| host_of(&node.addr));
            Some(TlsClient::with_server_name(trust.clone(), server_name)?)
        } else {
            None
        };
        let outcome = apply_to_node(&node.addr, &secret, &base_request, node_tls.as_ref()).await;
        results.push((node.clone(), outcome));
    }

    // 5. Record the spread (5a: the roster path is audited like the broadcast
    //    path — which nodes, which plan hash, per-node outcomes), then report.
    //    A partial spread is recorded *as a failure* and then reported as one.
    let request_line =
        serde_json::to_string(&base_request).with_context(|| "encoding the plan for the record")?;
    record_spread(&roster_record(
        &generate_spread_id(),
        group,
        &name,
        &version,
        &sha256_hex(&request_line),
        &results,
    ))?;

    print_results(&name, &results)?;
    Ok(())
}

/// The audit record for a roster spread (5a): which nodes, which plan hash,
/// and each node's outcome. Pure, so the shape is testable without a network.
fn roster_record(
    spread_id: &str,
    group: Option<&str>,
    name: &str,
    version: &str,
    plan_sha: &str,
    results: &[(NodeConfig, Result<()>)],
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
fn roster_node_results(results: &[(NodeConfig, Result<()>)]) -> Vec<SpreadNodeResult> {
    results
        .iter()
        .map(|(node, outcome)| match outcome {
            Ok(()) => SpreadNodeResult {
                name: node.name.clone(),
                addr: node.addr.clone(),
                ok: true,
                error: None,
            },
            Err(e) => SpreadNodeResult {
                name: node.name.clone(),
                addr: node.addr.clone(),
                ok: false,
                error: Some(e.to_string()),
            },
        })
        .collect()
}

/// Per-node results from the broadcast path (the coordinator only learns each
/// node's callback peer address — never its name).
fn broadcast_node_results(callbacks: &[(SocketAddr, Response)]) -> Vec<SpreadNodeResult> {
    callbacks
        .iter()
        .map(|(peer, response)| match response {
            Response::Success { .. } => SpreadNodeResult {
                name: peer.to_string(),
                addr: peer.to_string(),
                ok: true,
                error: None,
            },
            Response::Error { message } | Response::NotFound { message } => SpreadNodeResult {
                name: peer.to_string(),
                addr: peer.to_string(),
                ok: false,
                error: Some(message.clone()),
            },
        })
        .collect()
}

/// Resolve the coordinator's target list: the named group's roster plus any
/// ad-hoc endpoints. Also surfaces the group's optional `secret_path` so the
/// secret resolution can fall back to it.
fn resolve_roster(
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
fn merge_discovered(roster: &mut Vec<NodeConfig>, discovered: &[DiscoveredNode]) -> usize {
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
fn host_of(addr: &str) -> String {
    addr.split(':').next().unwrap_or(addr).to_string()
}

/// Ping a node, then apply the deployment. A failed ping is reported as
/// "unreachable"; an apply error is reported with the node's own message. When
/// `tls` is set (increment 4), the connection is wrapped in TLS with the
/// shared trust config before the shared handshake.
async fn apply_to_node(
    addr: &str,
    secret: &str,
    request: &AgentRequest,
    tls: Option<&TlsClient>,
) -> Result<()> {
    let client = match tls {
        Some(tls) => RemoteClient::new(addr, secret).with_tls(tls.clone()),
        None => RemoteClient::new(addr, secret),
    };
    client
        .ping()
        .await
        .with_context(|| format!("node {addr} unreachable"))?;
    let response = client
        .send_agent_request(request)
        .await
        .with_context(|| format!("node {addr} failed to apply"))?;
    match response {
        Response::Success { .. } => Ok(()),
        Response::Error { message } => Err(anyhow::anyhow!("{message}")),
        Response::NotFound { message } => Err(anyhow::anyhow!("{message}")),
    }
}

/// Print the per-node results table; return an error if any node failed so the
/// coordinator exits non-zero (a partial spread must not look like success).
fn print_results(name: &str, results: &[(NodeConfig, Result<()>)]) -> Result<()> {
    let mut failures: Vec<(&NodeConfig, String)> = Vec::new();
    for (node, outcome) in results {
        match outcome {
            Ok(()) => println!("  ✓  {}/{}  applied", node.name, node.addr),
            Err(e) => {
                println!("  ✗  {}/{}  FAILED", node.name, node.addr);
                failures.push((node, e.to_string()));
            }
        }
    }

    if failures.is_empty() {
        println!(
            "\n✅ spread deployment '{name}' to {} node(s)",
            results.len()
        );
        Ok(())
    } else {
        println!(
            "\n⚠️  spread of '{name}' failed on {} of {} node(s):",
            failures.len(),
            results.len()
        );
        for (node, err) in &failures {
            println!("  ✗  {}: {err}", node.addr);
        }
        Err(anyhow::anyhow!(
            "spread of '{name}' failed on {} of {} node(s)",
            failures.len(),
            results.len()
        ))
    }
}

/// Resolve the epidemic (network) secret.
///
/// Precedence: `--secret` inline > `--secret-path` file > the group's
/// `secret_path` (if any) > the default path > freshly generated (a bootstrap
/// aid — every node must be given the same value, or handshakes fail).
fn resolve_epidemic_secret(
    secret: &EpidemicSecret,
    group_secret_path: Option<&str>,
) -> Result<String> {
    if let Some(secret) = &secret.inline {
        return Ok(secret.clone());
    }
    if let Some(path) = &secret.path {
        let content = std::fs::read_to_string(path.as_path())
            .with_context(|| format!("reading epidemic secret {}", path.display()))?;
        return Ok(content.trim().to_string());
    }
    if let Some(path) = group_secret_path {
        if let Ok(content) = std::fs::read_to_string(path) {
            let trimmed = content.trim().to_string();
            if !trimmed.is_empty() {
                return Ok(trimmed);
            }
        }
    }
    if let Ok(content) = std::fs::read_to_string(auth::EPIDEMIC_SECRET_PATH) {
        let trimmed = content.trim().to_string();
        if !trimmed.is_empty() {
            return Ok(trimmed);
        }
    }
    let secret = auth::generate_secret();
    eprintln!("WARN: no epidemic secret configured; generated one. Give every node the same value (--secret/--secret-path), or handshakes will fail.\n  epidemic secret: {secret}");
    Ok(secret)
}

// ── Broadcast (increment 3) ────────────────────────────────────────────────

/// Validate the broadcast flag combinations up front (before any network I/O):
/// `--broadcast` is the roster-free path (no `--node`/`--group`/`--discover`),
/// and the canary/promote/spread-id/criteria options are broadcast-only.
#[allow(clippy::too_many_arguments)]
fn validate_broadcast_flags(
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
fn parse_canary_arg(raw: &str) -> Result<Canary> {
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
fn now_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Build a spread intent from the concrete plan's identity. `plan_sha` is the
/// sha256 of the exact `ApplyDeployment` request line the callback will send
/// (the node trims the trailing newline before hashing, so it matches).
#[allow(clippy::too_many_arguments)]
fn build_intent(
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
/// primary LAN address (used as the intent's `origin` when `--interface` is
/// not given). A UDP `connect` sends no packets.
fn primary_lan_ip() -> Option<Ipv4Addr> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    match sock.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(v4) => Some(v4),
        std::net::IpAddr::V6(_) => None,
    }
}

/// The broadcast path: resolve the plan, sign an intent, run a callback
/// listener, broadcast the intent (with re-sends), and wait for the nodes that
/// self-select to dial back, apply, and report. Roles are flipped vs the TCP
/// path — the node is the one that dials and sends the `Response`.
async fn run_broadcast(
    target: &str,
    set_args: &[String],
    registry_url: Option<String>,
    dry_run: bool,
    epidemic_secret: EpidemicSecret,
    interface: Option<Ipv4Addr>,
    bcast: &Broadcast,
) -> Result<()> {
    // 1. Pure Plan step (shared with the roster path) — one concrete plan.
    let dp = resolve_deployment_plan(target, set_args, registry_url).await?;
    let name = dp.spec.meta.name.clone();
    let version = dp.spec.meta.version.clone();

    let request = AgentRequest::ApplyDeployment {
        name: name.clone(),
        version: version.clone(),
        variables: dp.shared.clone(),
        infections: deployment_apply_infections(&dp),
    };
    // Serialize once; the callback sends exactly this line and the intent's
    // `plan.sha256` is the digest of it (no terminator). Re-serializing would
    // be safe here (BTreeMap/Vec keep it deterministic) but we avoid it.
    let request_line = serde_json::to_string(&request)?;
    let plan_sha = sha256_hex(&request_line);

    let secret = resolve_epidemic_secret(&epidemic_secret, None)?;

    let spread_id = bcast.spread_id.clone().unwrap_or_else(generate_spread_id);
    let stage = if bcast.promote {
        SpreadStage::Full
    } else if bcast.canary.is_some() {
        SpreadStage::Canary
    } else {
        SpreadStage::Full
    };

    let origin = interface.or_else(primary_lan_ip).ok_or_else(|| {
        anyhow!("no LAN IPv4 to advertise as the callback origin — pass --interface")
    })?;

    if dry_run {
        println!(
            "DRY RUN — broadcast deployment '{}' v{} over {}:{} (nothing will be applied)",
            name, version, bcast.group, bcast.port
        );
        println!("  stage:       {:?}", stage);
        println!("  spread_id:   {spread_id}");
        println!("  origin:      {origin}");
        if bcast.criteria.is_empty() {
            println!("  criteria:    (all matching nodes)");
        } else {
            println!("  criteria:    {}", bcast.criteria.join(", "));
        }
        match &bcast.canary {
            Some(Canary::Percentage { pct }) => println!("  canary:      {pct}% cohort"),
            Some(Canary::Subset { criterion }) => println!("  canary:      subset {criterion}"),
            None => println!("  canary:      (none)"),
        }
        println!("  plan sha256: {plan_sha}");
        print_dry_run(&dp);
        return Ok(());
    }

    // 2. Callback listener on an ephemeral port (the node dials this).
    let listener = TcpListener::bind("0.0.0.0:0").await?;
    let callback_port = listener.local_addr()?.port();

    let intent = build_intent(
        &secret,
        &spread_id,
        stage,
        &name,
        &version,
        &plan_sha,
        bcast.criteria.clone(),
        bcast.canary.clone(),
        origin.to_string(),
        callback_port,
        now_unix_secs(),
    );

    // 3. Spawn the accept loop; each selected node becomes one connection.
    let results: Arc<Mutex<Vec<(SocketAddr, Response)>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let results = results.clone();
        let secret = secret.clone();
        let request_line = request_line.clone();
        tokio::spawn(async move {
            loop {
                let (stream, peer) = match listener.accept().await {
                    Ok(x) => x,
                    Err(_) => break,
                };
                let results = results.clone();
                let secret = secret.clone();
                let request_line = request_line.clone();
                tokio::spawn(async move {
                    let response = serve_one_callback(stream, &secret, &request_line).await;
                    results.lock().await.push((peer, response));
                });
            }
        });
    }

    // 4. Broadcast the intent. UDP is best-effort, so re-send a few times;
    //    nodes de-duplicate by spread_id, so repeats are harmless.
    for _ in 0..INTENT_RESENDS {
        send_intent(bcast.group, bcast.port, interface, &intent)?;
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    // 5. Wait for callbacks: stop after a short quiescence once at least one
    //    node has answered, or at the `--wait` deadline (whichever first).
    let deadline = std::time::Instant::now() + Duration::from_secs(bcast.wait_secs);
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

    // 6. Report per-node results and record history.
    let collected = std::mem::take(&mut *results.lock().await);
    report_and_record(&name, stage, &spread_id, &intent, &collected, bcast).await
}

/// One callback connection: the coordinator is the server here. Handshake with
/// the epidemic secret, send the exact plan line (the intent's digest is over
/// it, no terminator), and read the node's `Response`.
async fn serve_one_callback(stream: TcpStream, secret: &str, request_line: &str) -> Response {
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

/// Evaluate the collected callbacks for this stage, print the per-node results,
/// record the spread in history, and return `Err` when the spread did not
/// land (a failed or no-op spread must not look like success). A promote that
/// reaches no new node is a benign no-op (they already applied that id).
async fn report_and_record(
    name: &str,
    stage: SpreadStage,
    spread_id: &str,
    intent: &SpreadIntent,
    callbacks: &[(SocketAddr, Response)],
    bcast: &Broadcast,
) -> Result<()> {
    let mut applied = 0u32;
    let mut failed = 0u32;
    for (peer, response) in callbacks {
        match response {
            Response::Success { .. } => {
                println!("  ✓  {peer}  applied");
                applied += 1;
            }
            Response::Error { message } => {
                println!("  ✗  {peer}  FAILED: {message}");
                failed += 1;
            }
            Response::NotFound { message } => {
                println!("  ✗  {peer}  FAILED: {message}");
                failed += 1;
            }
        }
    }
    let stage_str = match stage {
        SpreadStage::Canary => "canary",
        SpreadStage::Full if bcast.promote => "promote",
        SpreadStage::Full => "full",
    };

    let ok = failed == 0
        && if applied == 0 {
            // A no-op is only fine when it is a promote (idempotent) or a
            // canary the operator may retry — otherwise nothing landed.
            bcast.promote
        } else {
            true
        };

    record_spread(&SpreadRecord {
        timestamp: intent.issued_at,
        spread_id: spread_id.to_string(),
        mode: SpreadMode::Broadcast,
        stage: stage_str.to_string(),
        name: name.to_string(),
        version: intent.plan.version.clone(),
        sha256: intent.plan.sha256.clone(),
        group: None,
        origin: Some(intent.origin.clone()),
        criteria: intent.criteria.clone(),
        canary: bcast.canary.clone(),
        nodes: broadcast_node_results(callbacks),
        applied,
        failed,
        ok,
    })?;

    if !ok {
        if applied == 0 {
            return Err(anyhow::anyhow!(
                "no node applied the {stage_str} spread ({} callback(s)) — check the epidemic secret, the group/interface, and that nodes joined the group",
                callbacks.len()
            ));
        }
        return Err(anyhow::anyhow!(
            "'{name}' {stage_str} spread: {applied} applied but {failed} callback(s) failed"
        ));
    }

    if applied == 0 {
        println!("\n✅ promote of {spread_id}: no new node applied (likely already applied)");
    } else {
        println!(
            "\n✅ broadcast '{name}' as {stage_str} — {applied} applied, {failed} failed (spread_id={spread_id})"
        );
    }
    Ok(())
}

/// A human-readable form of the canary cohort for the history log.
fn describe_canary(canary: Option<&Canary>) -> String {
    match canary {
        Some(Canary::Percentage { pct }) => format!("pct={pct}"),
        Some(Canary::Subset { criterion }) => format!("subset={criterion}"),
        None => String::new(),
    }
}

// ── Spread history (`epidemic spreads`) ────────────────────────────────────
//
// One record per spread (roster *and* broadcast), one JSON line each (5a).
// The record type is shared with the console's REST surface (5b) in
// `pandemic-protocol`; the history file's location + format live in
// `pandemic_common::history`, which `epidemic spreads` and the REST
// `GET /api/epidemic/spreads` both read through.

/// Append one record to the default history file (shared with the console's
/// REST read surface — `pandemic_common::history`).
fn record_spread(record: &SpreadRecord) -> Result<()> {
    history::append_record(&history::default_history_path(), record)
}

/// `epidemic spreads`: print recent spread history (newest first), with each
/// node's outcome where the record carries one.
async fn spreads(limit: u32) -> Result<()> {
    let path = history::default_history_path();
    let records = history::load_spreads(&path, limit as usize);
    if records.is_empty() {
        println!("No spreads recorded yet at {}.", path.display());
        println!("  Run `epidemic spread <target>` to create one.");
        return Ok(());
    }
    println!("Recent spreads ({} shown, newest first):\n", records.len());
    for r in &records {
        println!(
            "  {}  {}  {} v{}  [{} {}]  applied={} failed={}",
            human_timestamp(r.timestamp),
            if r.ok { "✓" } else { "✗" },
            r.name,
            r.version,
            match r.mode {
                SpreadMode::Roster => "roster",
                SpreadMode::Broadcast => "broadcast",
            },
            r.stage,
            r.applied,
            r.failed
        );
        println!("      spread_id={}", r.spread_id);
        if let Some(g) = &r.group {
            println!("      group={g}");
        }
        if let Some(o) = &r.origin {
            println!("      origin={o}");
        }
        if !r.criteria.is_empty() {
            println!("      criteria={}", r.criteria.join(";"));
        }
        if let Some(c) = &r.canary {
            println!("      canary={}", describe_canary(Some(c)));
        }
        for n in &r.nodes {
            if n.ok {
                println!("      ✓  {}/{}", n.name, n.addr);
            } else {
                println!(
                    "      ✗  {}/{}  {}",
                    n.name,
                    n.addr,
                    n.error.as_deref().unwrap_or("failed")
                );
            }
        }
        println!();
    }
    Ok(())
}

/// Render a unix-seconds timestamp as `YYYY-MM-DD HH:MM:SS` (UTC), without a
/// chrono dependency (the CLI does not link chrono).
fn human_timestamp(secs: i64) -> String {
    if secs <= 0 {
        return "-".to_string();
    }
    // Days since the Unix epoch → calendar date (Hinnant civil-from-days).
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400) as u32;
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // Shift from the 1970 epoch to the proleptic Gregorian year-0 epoch.
    let z = days + 719_468;
    let era = z.div_euclid(146097).max(0);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let yy = if m <= 2 { y + 1 } else { y };
    format!("{yy:04}-{m:02}-{d:02} {hh:02}:{mm:02}:{ss:02}Z")
}

async fn nodes(group: Option<&str>, probe: Discovery) -> Result<()> {
    // `--discover`: probe the LAN over mDNS. Discovery is the point of this
    // flag, so a probe failure is a hard error (not a warning as in `spread`).
    if probe.enabled {
        let discovered = discovery::discover_nodes(probe.interface, probe.duration())
            .await
            .with_context(|| {
                format!(
                    "mDNS discovery (interface={:?}, {}s probe)",
                    probe.interface, probe.timeout_secs
                )
            })?;
        println!("discovered via mDNS ({}s probe):", probe.timeout_secs);
        if discovered.is_empty() {
            println!("  (no nodes advertising {})", discovery::SERVICE_FQDN);
        } else {
            for d in &discovered {
                println!("  {:<16} {}", d.name, d.addr);
            }
        }
        println!();
    }

    let groups = load_groups_or_default(None)?;
    if groups.is_empty() {
        println!(
            "No named groups configured at {}",
            default_groups_path().display()
        );
        println!(
            "  Add [[group]] entries, or spread ad-hoc with `epidemic spread --node host:port`."
        );
        return Ok(());
    }
    let shown: Vec<&GroupConfig> = match group {
        Some(name) => {
            let g = find_group(&groups, name)
                .ok_or_else(|| anyhow::anyhow!("group '{name}' not found"))?;
            vec![g]
        }
        None => groups.iter().collect(),
    };
    for g in shown {
        println!("group '{}':", g.name);
        if let Some(sp) = &g.secret_path {
            println!("  secret_path: {sp}");
        } else {
            println!("  secret:      (--secret / --secret-path / default path)");
        }
        if g.node.is_empty() {
            println!("  nodes:       (none — use --node on `epidemic spread`)");
        } else {
            println!("  nodes:");
            for n in &g.node {
                println!("    {:<16} {}", n.name, n.addr);
            }
        }
        println!();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn validate_broadcast_flags_combos() {
        // A plain broadcast (no roster, no canary/promote) is fine.
        assert!(validate_broadcast_flags(true, None, &[], false, &[], None, false, None).is_ok());
        // Broadcast takes no roster.
        assert!(
            validate_broadcast_flags(true, Some("edge"), &[], false, &[], None, false, None)
                .is_err()
        );
        assert!(validate_broadcast_flags(
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
        assert!(validate_broadcast_flags(true, None, &[], true, &[], None, false, None).is_err());
        // --promote and --canary are mutually exclusive; --promote needs an id.
        assert!(validate_broadcast_flags(
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
        assert!(validate_broadcast_flags(true, None, &[], false, &[], None, true, None).is_err());
        // A bad criterion is rejected even in broadcast mode.
        assert!(validate_broadcast_flags(
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
        assert!(validate_broadcast_flags(
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
        assert!(validate_broadcast_flags(false, None, &[], false, &[], None, true, None).is_err());
        assert!(
            validate_broadcast_flags(false, None, &[], false, &[], None, false, Some("id"))
                .is_err()
        );
        assert!(validate_broadcast_flags(
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
        assert!(pandemic_common::verify_intent_token(secret, &intent));
        assert!(!pandemic_common::verify_intent_token(
            "other-secret",
            &intent
        ));
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
                Ok(()),
            ),
            (
                NodeConfig {
                    name: "edge-2".to_string(),
                    addr: "10.0.0.2:7711".to_string(),
                },
                Err(anyhow::anyhow!("node 10.0.0.2:7711 failed to apply")),
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
        assert!(rec.nodes[1].error.is_some());
        assert_eq!(rec.applied, 1);
        assert_eq!(rec.failed, 1);
        assert!(!rec.ok, "a partial spread must not look like success");

        // All nodes ok → ok, and an ad-hoc roster has no group.
        let all_ok: Vec<(NodeConfig, Result<()>)> =
            results.iter().map(|(n, _)| (n.clone(), Ok(()))).collect();
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
    fn human_timestamp_formats_utc() {
        assert_eq!(human_timestamp(1_704_067_200), "2024-01-01 00:00:00Z");
        // Non-positive is a placeholder, not a date.
        assert_eq!(human_timestamp(0), "-");
        assert_eq!(human_timestamp(-5), "-");
    }

    #[test]
    fn describe_canary_formats() {
        assert_eq!(describe_canary(None), "");
        assert_eq!(
            describe_canary(Some(&Canary::Percentage { pct: 25 })),
            "pct=25"
        );
        assert_eq!(
            describe_canary(Some(&Canary::Subset {
                criterion: "role=canary".to_string()
            })),
            "subset=role=canary"
        );
    }
}
