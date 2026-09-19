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

use std::path::PathBuf;

use anyhow::{Context, Result};
use pandemic_common::groups::{
    default_groups_path, find_group, load_groups_or_default, merge_roster,
};
use pandemic_common::{auth, GroupConfig, NodeConfig, RemoteClient};
use pandemic_protocol::{AgentRequest, Response};

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

#[derive(clap::Subcommand)]
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
    },
    /// List configured groups and their node rosters
    Nodes {
        /// Only show this group
        #[arg(long)]
        group: Option<String>,
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
        } => {
            let secret = EpidemicSecret {
                inline: epidemic_secret,
                path: epidemic_secret_path,
            };
            spread(
                &target,
                group.as_deref(),
                &node,
                registry_url,
                &set,
                dry_run,
                secret,
            )
            .await
        }
        EpidemicAction::Nodes { group } => nodes(group.as_deref()).await,
    }
}

async fn spread(
    target: &str,
    group: Option<&str>,
    adhoc: &[String],
    registry_url: Option<String>,
    set_args: &[String],
    dry_run: bool,
    epidemic_secret: EpidemicSecret,
) -> Result<()> {
    // 1. Resolve the target roster: the named group's nodes merged with any
    //    ad-hoc `--node` endpoints (de-duplicated by addr, group first).
    let (roster, group_secret_path) = resolve_roster(group, adhoc)?;
    if roster.is_empty() && !dry_run {
        return Err(anyhow::anyhow!(
            "no nodes to spread to — pass --node host:port, or --group NAME (see `epidemic nodes`)"
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

    // 4. Apply the identical plan to every node, collecting per-node results.
    let base_request = AgentRequest::ApplyDeployment {
        name: dp.spec.meta.name.clone(),
        version: dp.spec.meta.version.clone(),
        variables: dp.shared.clone(),
        infections: deployment_apply_infections(&dp),
    };

    let name = dp.spec.meta.name.clone();
    let mut results: Vec<(NodeConfig, Result<()>)> = Vec::new();
    for node in &roster {
        let outcome = apply_to_node(&node.addr, &secret, &base_request).await;
        results.push((node.clone(), outcome));
    }

    print_results(&name, &results)?;
    Ok(())
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

/// Ping a node, then apply the deployment. A failed ping is reported as
/// "unreachable"; an apply error is reported with the node's own message.
async fn apply_to_node(addr: &str, secret: &str, request: &AgentRequest) -> Result<()> {
    let client = RemoteClient::new(addr, secret);
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

async fn nodes(group: Option<&str>) -> Result<()> {
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
    fn unknown_group_is_an_error() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("groups.toml");
        std::fs::write(&path, "").unwrap();
        let groups = load_groups_or_default(Some(path.as_path())).unwrap();
        assert!(find_group(&groups, "nope").is_none());
    }
}
