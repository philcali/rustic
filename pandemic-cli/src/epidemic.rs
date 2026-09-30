//! Epidemic: spread a resolved deployment to a group of nodes.
//!
//! The **coordinator** (this command) is the third member of the node / group /
//! coordinator primitive. It holds a group's roster + epidemic secret, runs the
//! pure `Plan` step locally (shared with `deployment install`), then applies the
//! identical concrete plan to every node in the roster through a
//! [`pandemic-node`] receiver — each of which forwards to its own local agent.
//! The spread machinery itself (handshake, broadcast intent, results, audit
//! record) is shared with the console's REST surface in
//! `pandemic_common::coordinator`, so the CLI and the API cannot diverge.
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
//!
//! **Console trigger (increment 5c):** the REST `POST /api/epidemic/spread`
//! runs the very same coordinator with a live-progress sink; this CLI passes
//! no sink and prints the shared audit record.

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use pandemic_common::discovery;
use pandemic_common::groups::{
    default_groups_path, find_group, load_groups_or_default, GroupConfig,
};
use pandemic_common::{
    deployment_apply_infections, generate_spread_id, history, merge_discovered, parse_canary_arg,
    primary_lan_ip, resolve_roster, run_broadcast_spread, run_roster_spread, sha256_hex,
    validate_broadcast, BroadcastSpread, RosterSpread, SecretSource, TlsOptions,
    DEFAULT_APPLY_RETRIES, DEFAULT_APPLY_TIMEOUT, DEFAULT_MULTICAST_GROUP, DEFAULT_MULTICAST_PORT,
};
use pandemic_protocol::{AgentRequest, Canary, SpreadMode, SpreadRecord, SpreadStage};

use crate::deployment::{print_dry_run, resolve_deployment_plan};

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
struct BroadcastCli {
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
    multicast_group: Ipv4Addr,
    /// Multicast UDP port (default [`DEFAULT_MULTICAST_PORT`]).
    multicast_port: u16,
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

        // ── Retries (increment 4b): a dropped node is retried, not lost ─────
        /// How many times to retry a node after a failed attempt (transport
        /// error or per-attempt timeout). Default: [`DEFAULT_APPLY_RETRIES`].
        /// A node that *answers* an error (a verdict) is not retried.
        #[arg(long, default_value_t = DEFAULT_APPLY_RETRIES)]
        retries: u32,
        /// Per-attempt timeout in seconds: a node that never answers is
        /// retried after this long. Default: 30.
        #[arg(long, default_value_t = DEFAULT_APPLY_TIMEOUT.as_secs())]
        apply_timeout: u64,
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
            retries,
            apply_timeout,
        } => {
            let secret = SecretSource::new(epidemic_secret, epidemic_secret_path);
            let probe = Discovery {
                enabled: discover,
                interface,
                timeout_secs: timeout,
            };
            let tls_opts = TlsOptions {
                enabled: tls,
                ca: tls_ca,
                server_name: tls_server_name,
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
                tls_opts,
                retries,
                Duration::from_secs(apply_timeout),
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
    epidemic_secret: SecretSource,
    probe: Discovery,
    broadcast: bool,
    criteria: Vec<String>,
    canary: Option<Canary>,
    promote: bool,
    spread_id: Option<String>,
    mcast_group: Ipv4Addr,
    mcast_port: u16,
    wait_secs: u64,
    tls: TlsOptions,
    retries: u32,
    apply_timeout: Duration,
) -> Result<()> {
    // Broadcast flag validation (cheap, up front — before any network I/O).
    validate_broadcast(
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
        return run_broadcast_cli(
            target,
            set_args,
            registry_url,
            dry_run,
            epidemic_secret,
            probe.interface,
            &BroadcastCli {
                criteria,
                canary,
                promote,
                spread_id,
                multicast_group: mcast_group,
                multicast_port: mcast_port,
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

    // 3. TLS (increment 4) needs a CA; fail up front with the CLI's message
    //    (the shared coordinator carries the same check).
    if tls.enabled && tls.ca.is_none() {
        return Err(anyhow::anyhow!("--tls requires --tls-ca"));
    }

    // 4. Spread + record (the shared coordinator; the CLI streams no progress).
    let result = run_roster_spread(&RosterSpread {
        roster,
        group: group.map(str::to_string),
        group_secret_path,
        plan: dp,
        secret: epidemic_secret,
        tls,
        retries,
        timeout: apply_timeout,
        spread_id: None,
        history_path: None,
        progress: None,
    })
    .await?;

    if let Some(secret) = &result.generated_secret {
        eprintln!("WARN: no epidemic secret configured; generated one. Give every node the same value (--secret/--secret-path), or handshakes will fail.\n  epidemic secret: {secret}");
    }
    print_roster_results(&result.record)
}

/// Print the per-node results of a roster spread; return an error if any node
/// failed so the coordinator exits non-zero (a partial spread must not look
/// like success).
fn print_roster_results(rec: &SpreadRecord) -> Result<()> {
    let failed_nodes = rec.nodes.iter().filter(|n| !n.ok).collect::<Vec<_>>();
    for n in &rec.nodes {
        if n.ok {
            println!("  ✓  {}/{}  applied", n.name, n.addr);
        } else {
            println!("  ✗  {}/{}  FAILED", n.name, n.addr);
        }
    }

    if failed_nodes.is_empty() {
        println!(
            "\n✅ spread deployment '{}' to {} node(s)",
            rec.name,
            rec.nodes.len()
        );
        Ok(())
    } else {
        println!(
            "\n⚠️  spread of '{}' failed on {} of {} node(s):",
            rec.name,
            rec.failed,
            rec.nodes.len()
        );
        for n in &failed_nodes {
            println!(
                "  ✗  {}: {}",
                n.addr,
                n.error.as_deref().unwrap_or("failed")
            );
        }
        Err(anyhow::anyhow!(
            "spread of '{}' failed on {} of {} node(s)",
            rec.name,
            rec.failed,
            rec.nodes.len()
        ))
    }
}

/// The broadcast path (CLI wrapper): resolve the plan, optionally dry-run,
/// then run the shared broadcast coordinator and print its audit record.
async fn run_broadcast_cli(
    target: &str,
    set_args: &[String],
    registry_url: Option<String>,
    dry_run: bool,
    epidemic_secret: SecretSource,
    interface: Option<Ipv4Addr>,
    bcast: &BroadcastCli,
) -> Result<()> {
    // 1. Pure Plan step (shared with the roster path) — one concrete plan.
    let dp = resolve_deployment_plan(target, set_args, registry_url).await?;
    let name = dp.spec.meta.name.clone();
    let version = dp.spec.meta.version.clone();

    if dry_run {
        let request = AgentRequest::ApplyDeployment {
            name: name.clone(),
            version: version.clone(),
            variables: dp.shared.clone(),
            infections: deployment_apply_infections(&dp),
        };
        let plan_sha = sha256_hex(&serde_json::to_string(&request)?);
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

        println!(
            "DRY RUN — broadcast deployment '{}' v{} over {}:{} (nothing will be applied)",
            name, version, bcast.multicast_group, bcast.multicast_port
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

    // 2. Broadcast + record (the shared coordinator; the CLI streams no
    //    progress).
    let result = run_broadcast_spread(&BroadcastSpread {
        plan: dp,
        secret: epidemic_secret,
        criteria: bcast.criteria.clone(),
        canary: bcast.canary.clone(),
        promote: bcast.promote,
        spread_id: bcast.spread_id.clone(),
        multicast_group: bcast.multicast_group,
        multicast_port: bcast.multicast_port,
        wait_secs: bcast.wait_secs,
        interface,
        history_path: None,
        progress: None,
    })
    .await?;

    if let Some(secret) = &result.generated_secret {
        eprintln!("WARN: no epidemic secret configured; generated one. Give every node the same value (--secret/--secret-path), or handshakes will fail.\n  epidemic secret: {secret}");
    }
    print_broadcast_results(&result.record)
}

/// Print the broadcast result (the coordinator's audit record). A failed or
/// no-op spread returns an error (it must not look like success); a promote
/// that reaches no new node is a benign no-op (they already applied that id).
fn print_broadcast_results(rec: &SpreadRecord) -> Result<()> {
    for n in &rec.nodes {
        if n.ok {
            println!("  ✓  {}  applied", n.addr);
        } else {
            println!(
                "  ✗  {}  FAILED: {}",
                n.addr,
                n.error.as_deref().unwrap_or("failed")
            );
        }
    }

    if !rec.ok {
        if rec.applied == 0 {
            return Err(anyhow::anyhow!(
                "no node applied the {} spread ({} callback(s)) — check the epidemic secret, the group/interface, and that nodes joined the group",
                rec.stage,
                rec.nodes.len()
            ));
        }
        return Err(anyhow::anyhow!(
            "'{}' {} spread: {} applied but {} callback(s) failed",
            rec.name,
            rec.stage,
            rec.applied,
            rec.failed
        ));
    }

    if rec.applied == 0 {
        println!(
            "\n✅ promote of {}: no new node applied (likely already applied)",
            rec.spread_id
        );
    } else {
        println!(
            "\n✅ broadcast '{}' as {} — {} applied, {} failed (spread_id={})",
            rec.name, rec.stage, rec.applied, rec.failed, rec.spread_id
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
