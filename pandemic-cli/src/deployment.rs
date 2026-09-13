//! Spec-driven deployment lifecycle (ideas/deployments.md, phase 4).
//!
//! `deployment install` does the pure `Plan` step — resolve the deployment's
//! shared variables and render every infection's templates (all in the shared
//! [`pandemic_common::apply`] builder) — then hands the concrete plans to the
//! agent, which runs the privileged `Apply` step: ownership pre-flight, apply
//! in `order`, and record the deployment as the *owner* of each.
//!
//! `deployment remove` uninstalls the owned infections in **reverse** order
//! and drops the record; infections that are not owned by the deployment
//! are left untouched. Re-running `deployment install` under an existing name
//! is an idempotent re-apply/upgrade.
//!
//! The install target is either a local `deployment.toml` path (fully offline)
//! or a registry **deployment** name (fetched + sha256-verified, its infection-spec
//! atoms pulled from the same registry). `--registry-url` overrides the registry
//! for the by-name path.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use pandemic_protocol::spec::parse_deployment_spec;
use pandemic_protocol::AgentRequest;

use crate::apply::{
    build_deployment_plan_from, deployment_apply_infections, parse_set_args, parse_set_values,
    resolve_deployment_target, DeploymentPlan,
};
use crate::infection::{active_str, target_is_local};
use crate::registry::registry_client;
use crate::service::agent_action;

pub async fn handle_deployment_command(
    action: crate::DeploymentAction,
    agent_secret: Option<String>,
    agent_secret_path: Option<PathBuf>,
) -> Result<()> {
    match action {
        crate::DeploymentAction::Install {
            target,
            set,
            registry_url,
            dry_run,
        } => install(&target, &set, registry_url, dry_run, agent_secret, agent_secret_path).await,
        crate::DeploymentAction::List => list(agent_secret, agent_secret_path).await,
        crate::DeploymentAction::Status { name } => {
            status(name.as_deref(), agent_secret, agent_secret_path).await
        }
        crate::DeploymentAction::Remove { name } => {
            remove(&name, agent_secret, agent_secret_path).await
        }
    }
}

async fn install(
    target: &str,
    set_args: &[String],
    registry_url: Option<String>,
    dry_run: bool,
    agent_secret: Option<String>,
    agent_secret_path: Option<PathBuf>,
) -> Result<()> {
    // Pure Plan step (shared with the REST API). A local spec path stays fully
    // offline; a registry name fetches + sha256-verifies the deployment bundle
    // and its infection-spec atoms, then resolves + renders + validates them.
    let dp = if target_is_local(target) {
        let spec_path = Path::new(target);
        let text = std::fs::read_to_string(spec_path)
            .with_context(|| format!("reading deployment spec {}", spec_path.display()))?;
        let spec = parse_deployment_spec(&text)
            .with_context(|| format!("parsing deployment spec {}", spec_path.display()))?;
        let spec_dir = spec_path
            .parent()
            .map(Path::to_path_buf)
            .filter(|p| p != Path::new(""))
            .unwrap_or_else(|| PathBuf::from("."));

        let declared: Vec<String> = spec.variables.keys().cloned().collect();
        let set = parse_set_args(
            set_args,
            &declared,
            &format!("deployment '{}'", spec.meta.name),
        )?;
        build_deployment_plan_from(&spec, &spec_dir, &set)?
    } else {
        // Registry name: --set values parsed raw, then validated against the
        // deployment bundle's declared variables inside the resolver.
        let set = parse_set_values(set_args)?;
        let client = registry_client(registry_url);
        resolve_deployment_target(&client, target, &set).await?
    };

    if dry_run {
        print_dry_run(&dp);
        return Ok(());
    }

    // The agent runs the Apply step: ownership pre-flight, each infection in
    // order (recorded with this deployment as owner), then the record.
    let infections = deployment_apply_infections(&dp);
    let data = agent_action(
        &AgentRequest::ApplyDeployment {
            name: dp.spec.meta.name.clone(),
            version: dp.spec.meta.version.clone(),
            variables: dp.shared.clone(),
            infections,
        },
        agent_secret,
        agent_secret_path,
    )
    .await?;

    let applied_count = data
        .get("applied")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    println!(
        "✅ Installed deployment '{}' ({} infection(s) applied)",
        dp.spec.meta.name, applied_count
    );
    Ok(())
}

/// The `--dry-run` view: everything resolved and rendered, nothing applied.
fn print_dry_run(dp: &DeploymentPlan) {
    println!(
        "DRY RUN — deployment '{}' v{} (nothing will be applied)",
        dp.spec.meta.name, dp.spec.meta.version
    );
    if !dp.shared.is_empty() {
        println!("\nshared variables:");
        for (key, value) in &dp.shared {
            println!("  {key} = {value}");
        }
    }
    for r in &dp.infections {
        println!(
            "\ninfection '{}' v{} (order {}, source {})",
            r.name, r.plan.version, r.order, r.source
        );
        if !r.plan.declared_packages.is_empty() {
            let pkgs = r
                .plan
                .declared_packages
                .iter()
                .map(|(manager, packages)| format!("{manager}: [{}]", packages.join(", ")))
                .collect::<Vec<_>>()
                .join(", ");
            println!("  packages:  {pkgs} (manager picked at apply time)");
        }
        if !r.plan.groups.is_empty() {
            println!("  groups:    {}", r.plan.groups.join(", "));
        }
        if !r.plan.users.is_empty() {
            println!(
                "  users:     {}",
                r.plan
                    .users
                    .iter()
                    .map(|(u, _)| u.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        for file in &r.plan.files {
            println!(
                "  file:      {} ({}:{})",
                file.target, file.owner, file.mode
            );
        }
        if let Some(unit) = &r.plan.unit {
            println!(
                "  unit:      {} -> {}{}",
                unit.name,
                unit.target,
                if unit.enable { " (enabled)" } else { "" }
            );
        }
        if let Some(attach) = &r.plan.attach {
            println!("  attach:    {attach}");
        }
        if !r.plan.health_check.is_empty() {
            println!("  health:    {}", r.plan.health_check.join(" "));
        }
        if !r.plan.variables.is_empty() {
            println!("  variables:");
            for (key, value) in &r.plan.variables {
                println!("    {key} = {value}");
            }
        }
    }
}

async fn list(agent_secret: Option<String>, agent_secret_path: Option<PathBuf>) -> Result<()> {
    let data = agent_action(
        &AgentRequest::ListDeployments,
        agent_secret,
        agent_secret_path,
    )
    .await?;
    let deployments = data
        .get("deployments")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if deployments.is_empty() {
        println!("No deployments installed.");
        return Ok(());
    }
    println!(
        "{:<20} {:<10} {:<36} installed",
        "NAME", "VERSION", "INFECTIONS"
    );
    for dep in &deployments {
        let infections = dep
            .get("infections")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .map(|i| {
                        format!(
                            "{} (order {})",
                            i.get("name").and_then(|v| v.as_str()).unwrap_or("?"),
                            i.get("order").and_then(|v| v.as_u64()).unwrap_or(0)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        println!(
            "{:<20} {:<10} {:<36} {}",
            dep.get("name").and_then(|v| v.as_str()).unwrap_or("?"),
            dep.get("version").and_then(|v| v.as_str()).unwrap_or("?"),
            infections,
            dep.get("installed_at")
                .and_then(|v| v.as_str())
                .unwrap_or("-"),
        );
    }
    Ok(())
}

async fn status(
    name: Option<&str>,
    agent_secret: Option<String>,
    agent_secret_path: Option<PathBuf>,
) -> Result<()> {
    match name {
        Some(name) => show_one(name, agent_secret, agent_secret_path).await,
        None => {
            let data = agent_action(
                &AgentRequest::ListDeployments,
                agent_secret.clone(),
                agent_secret_path.clone(),
            )
            .await?;
            let deployments = data
                .get("deployments")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            if deployments.is_empty() {
                println!("No deployments installed.");
                return Ok(());
            }
            for dep in &deployments {
                let name = dep
                    .get("name")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("deployment entry without a name"))?;
                show_one(name, agent_secret.clone(), agent_secret_path.clone()).await?;
                println!();
            }
            Ok(())
        }
    }
}

/// One deployment: recorded state, shared variables, and each owned
/// infection's live status (missing ones flagged, not an error).
async fn show_one(
    name: &str,
    agent_secret: Option<String>,
    agent_secret_path: Option<PathBuf>,
) -> Result<()> {
    let data = agent_action(
        &AgentRequest::GetDeploymentStatus {
            name: name.to_string(),
        },
        agent_secret,
        agent_secret_path,
    )
    .await?;
    let state = &data["state"];

    println!(
        "{}  v{}",
        state.get("name").and_then(|v| v.as_str()).unwrap_or(name),
        state.get("version").and_then(|v| v.as_str()).unwrap_or("?")
    );
    println!(
        "   installed: {}",
        state
            .get("installed_at")
            .and_then(|v| v.as_str())
            .unwrap_or("-")
    );

    if let Some(variables) = state.get("variables").and_then(|v| v.as_object()) {
        if !variables.is_empty() {
            println!("\n   shared variables:");
            for (key, value) in variables {
                println!("   {key} = {value}");
            }
        }
    }

    if let Some(infections) = data.get("infections").and_then(|v| v.as_array()) {
        println!("\n   infections (install order):");
        for inf in infections {
            let iname = inf.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            if inf
                .get("present")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                let st = &inf["status"];
                let s = &st["state"];
                println!(
                    "   {iname}  v{}  [installed]",
                    s.get("version").and_then(|v| v.as_str()).unwrap_or("?")
                );
                if let Some(unit) = s.get("unit").and_then(|v| v.as_str()) {
                    println!(
                        "      unit:      {unit} ({})",
                        active_str(st.get("unit_active"))
                    );
                }
                if let Some(attach) = s.get("attach").and_then(|v| v.as_str()) {
                    println!(
                        "      attach:    {attach} ({})",
                        active_str(st.get("target_active"))
                    );
                }
                if let Some(files) = st.get("files").and_then(|v| v.as_array()) {
                    for file in files {
                        let exists = file
                            .get("exists")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let hash_ok = file
                            .get("hash_ok")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let mark = if exists && hash_ok {
                            "✓"
                        } else if exists {
                            "modified"
                        } else {
                            "missing"
                        };
                        println!(
                            "      [{mark}] {}",
                            file.get("target").and_then(|v| v.as_str()).unwrap_or("?")
                        );
                    }
                }
            } else {
                println!("   {iname}  [not installed on host]");
            }
        }
    }
    Ok(())
}

async fn remove(
    name: &str,
    agent_secret: Option<String>,
    agent_secret_path: Option<PathBuf>,
) -> Result<()> {
    let data = agent_action(
        &AgentRequest::RemoveDeployment {
            name: name.to_string(),
        },
        agent_secret,
        agent_secret_path,
    )
    .await?;

    let list_of = |key: &str| {
        data.get(key)
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default()
    };
    let removed = list_of("removed");
    let skipped = list_of("skipped");
    let failed = list_of("failed");
    let record_removed = data
        .get("record_removed")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if record_removed {
        println!("✅ Removed deployment '{name}'");
    } else {
        println!("⚠️  Removal of deployment '{name}' partially failed — record kept");
    }
    if !removed.is_empty() {
        println!("   removed: {removed}");
    }
    if !skipped.is_empty() {
        println!("   skipped: {skipped}");
    }
    if !failed.is_empty() {
        println!("   failed:  {failed}");
    }
    if let Some(notes) = data.get("notes").and_then(|v| v.as_array()) {
        for note in notes {
            println!("   note: {}", note.as_str().unwrap_or("?"));
        }
    }
    if !record_removed {
        println!(
            "\n  re-run `pandemic-cli deployment remove {name}` to finish — already-removed infections are skipped"
        );
    }
    Ok(())
}
