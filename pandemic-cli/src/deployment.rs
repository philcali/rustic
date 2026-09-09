//! Spec-driven deployment lifecycle (ideas/deployments.md, phase 4).
//!
//! `deploy install` resolves the deployment's shared variables, renders
//! every infection's templates (all before anything is applied), checks
//! ownership, applies the infections in `order` through the shared steps
//! in [`crate::apply`], and records the deployment as the *owner* of each.
//!
//! `deploy remove` uninstalls the owned infections in **reverse** order
//! and drops the record; infections that are not owned by the deployment
//! are left untouched. Re-running `deploy install` under an existing name
//! is an idempotent re-apply/upgrade.
//!
//! Registry `source` names and `--registry-url` arrive with phase 5 —
//! `source` must be a local path for now.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use pandemic_protocol::spec::{
    parse_deployment_spec, parse_infection_spec, resolve_deployment_variables, validate_deployment,
    DeploymentRecordedInfection, DeploymentSpec, DeploymentState, InfectionSpec,
};
use pandemic_protocol::AgentRequest;

use crate::apply::{
    apply_infection, build_plan, fetch_supported_managers, parse_set_args, select_packages,
};
use crate::infection::active_str;
use crate::service::agent_action;

pub async fn handle_deploy_command(
    action: crate::DeployAction,
    agent_secret: Option<String>,
    agent_secret_path: Option<PathBuf>,
) -> Result<()> {
    match action {
        crate::DeployAction::Install { path, set, dry_run } => {
            install(&path, &set, dry_run, agent_secret, agent_secret_path).await
        }
        crate::DeployAction::List => list(agent_secret, agent_secret_path).await,
        crate::DeployAction::Status { name } => {
            status(name.as_deref(), agent_secret, agent_secret_path).await
        }
        crate::DeployAction::Remove { name } => {
            remove(&name, agent_secret, agent_secret_path).await
        }
    }
}

/// One deployment infection, fully resolved and rendered.
struct Resolved {
    name: String,
    order: u64,
    source: String,
    spec: InfectionSpec,
    plan: crate::apply::Plan,
}

/// Resolve a deployment `source` to a local infection spec file.
fn resolve_source(spec_dir: &Path, source: &str) -> Result<PathBuf> {
    let candidate = if source.contains('/') {
        let p = Path::new(source);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            spec_dir.join(p)
        }
    } else {
        bail!(
            "source '{source}' is not a local path.\n  registry resolution arrives in a later phase (ideas/deployments.md, phase 5)"
        )
    };
    if !candidate.is_file() {
        bail!(
            "infection spec '{source}' not found (resolved to {})",
            candidate.display()
        );
    }
    Ok(candidate)
}

async fn install(
    spec_path: &Path,
    set_args: &[String],
    dry_run: bool,
    agent_secret: Option<String>,
    agent_secret_path: Option<PathBuf>,
) -> Result<()> {
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
    let shared = resolve_deployment_variables(&spec.variables, &set)
        .with_context(|| "resolving deployment variables")?;

    // Resolve + render every infection before touching the host.
    let mut resolved: Vec<Resolved> = Vec::new();
    for entry in &spec.infections {
        let source_path = resolve_source(&spec_dir, &entry.source)
            .with_context(|| format!("resolving source for infection '{}'", entry.name))?;
        let itext = std::fs::read_to_string(&source_path)
            .with_context(|| format!("reading infection spec {}", source_path.display()))?;
        let ispec = parse_infection_spec(&itext)
            .with_context(|| format!("parsing infection spec {}", source_path.display()))?;
        let idir = source_path
            .parent()
            .map(Path::to_path_buf)
            .filter(|p| p != Path::new(""))
            .unwrap_or_else(|| PathBuf::from("."));
        let plan = build_plan(&ispec, &idir, &entry.vars, &shared, &BTreeMap::new())
            .with_context(|| format!("building plan for infection '{}'", entry.name))?;
        resolved.push(Resolved {
            name: entry.name.clone(),
            order: entry.order,
            source: entry.source.clone(),
            spec: ispec,
            plan,
        });
    }
    resolved.sort_by_key(|r| r.order);

    let specs: Vec<InfectionSpec> = resolved.iter().map(|r| r.spec.clone()).collect();
    validate_deployment(&spec, &specs)?;

    if dry_run {
        print_dry_run(&spec, &shared, &resolved);
        return Ok(());
    }

    // One capabilities fetch shared by every infection that declares packages.
    if resolved
        .iter()
        .any(|r| !r.plan.declared_packages.is_empty())
    {
        let supported =
            fetch_supported_managers(agent_secret.clone(), agent_secret_path.clone()).await?;
        for r in &mut resolved {
            if !r.plan.declared_packages.is_empty() {
                r.plan.packages = select_packages(&r.plan.declared_packages, &supported)?;
            }
        }
    }

    // Ownership pre-flight: refuse to adopt infections that are installed
    // but not owned by this deployment.
    preflight_ownership(
        &spec.meta.name,
        &resolved,
        agent_secret.clone(),
        agent_secret_path.clone(),
    )
    .await?;

    // Apply the infections in order; record the deployment only when every
    // one succeeded (a failed apply leaves the record un-written).
    for r in &resolved {
        println!("Applying infection '{}' v{} ...", r.name, r.plan.version);
        if let Err(err) = apply_infection(
            &r.plan,
            Some(&spec.meta.name),
            agent_secret.clone(),
            agent_secret_path.clone(),
        )
        .await
        {
            bail!(
                "deployment '{}' failed on infection '{}':\n\n{err}\n\n  no deployment record was written; infections already applied remain installed",
                spec.meta.name,
                r.name
            );
        }
        println!("   ✅ applied '{}'", r.name);
    }

    let state = DeploymentState {
        name: spec.meta.name.clone(),
        version: spec.meta.version.clone(),
        variables: shared,
        infections: resolved
            .iter()
            .map(|r| DeploymentRecordedInfection {
                name: r.name.clone(),
                version: r.plan.version.clone(),
                order: r.order,
                source: r.source.clone(),
            })
            .collect(),
        installed_at: None, // the agent stamps it
    };
    agent_action(
        &AgentRequest::RecordDeployment {
            name: spec.meta.name.clone(),
            state,
        },
        agent_secret,
        agent_secret_path,
    )
    .await?;

    println!("✅ Installed deployment '{}'", spec.meta.name);
    Ok(())
}

/// Refuse to install over infections that belong to someone else.
///
/// - a recorded deployment with the same name must install the *same*
///   infection set (re-apply/upgrade), otherwise refuse;
/// - each infection must be absent, or owned by this deployment. Standalone
///   and foreign-owned infections are never adopted.
async fn preflight_ownership(
    dep_name: &str,
    resolved: &[Resolved],
    agent_secret: Option<String>,
    agent_secret_path: Option<PathBuf>,
) -> Result<()> {
    let data = agent_action(
        &AgentRequest::ListDeployments,
        agent_secret.clone(),
        agent_secret_path.clone(),
    )
    .await?;
    if let Some(deps) = data.get("deployments").and_then(|v| v.as_array()) {
        if let Some(rec) = deps
            .iter()
            .find(|d| d.get("name").and_then(|v| v.as_str()) == Some(dep_name))
        {
            let recorded: Vec<String> = rec
                .get("infections")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|i| i.get("name").and_then(|v| v.as_str()).map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let current: Vec<String> = resolved.iter().map(|r| r.name.clone()).collect();
            if recorded != current {
                bail!(
                    "deployment '{dep_name}' is already installed with infections [{}]\nbut this spec installs [{}]. Remove it first:\n  pandemic-cli deploy remove {dep_name}",
                    recorded.join(", "),
                    current.join(", ")
                );
            }
        }
    }

    let data = agent_action(
        &AgentRequest::ListInfections,
        agent_secret,
        agent_secret_path,
    )
    .await?;
    let infections = data
        .get("infections")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    for r in resolved {
        if let Some(rec) = infections
            .iter()
            .find(|i| i.get("name").and_then(|v| v.as_str()) == Some(r.name.as_str()))
        {
            match rec.get("owner").and_then(|v| v.as_str()) {
                Some(owner) if owner == dep_name => {}
                Some(owner) => bail!(
                    "infection '{}' is already installed and owned by deployment '{owner}'.\nRemove it first:\n  pandemic-cli deploy remove {owner}",
                    r.name
                ),
                None => bail!(
                    "infection '{}' is already installed standalone.\nRemove it first:\n  pandemic-cli infection uninstall {}",
                    r.name,
                    r.name
                ),
            }
        }
    }
    Ok(())
}

/// The `--dry-run` view: everything resolved and rendered, nothing applied.
fn print_dry_run(spec: &DeploymentSpec, shared: &BTreeMap<String, String>, resolved: &[Resolved]) {
    println!(
        "DRY RUN — deployment '{}' v{} (nothing will be applied)",
        spec.meta.name, spec.meta.version
    );
    if !shared.is_empty() {
        println!("\nshared variables:");
        for (key, value) in shared {
            println!("  {key} = {value}");
        }
    }
    for r in resolved {
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
            "\n  re-run `pandemic-cli deploy remove {name}` to finish — already-removed infections are skipped"
        );
    }
    Ok(())
}
