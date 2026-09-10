//! Spec-driven infection lifecycle (ideas/deployments.md, phase 3).
//!
//! `infection install` resolves the spec's variables, renders every template,
//! and applies the plan through the shared steps in [`crate::apply`] —
//! packages, groups/users, files, unit or attach, health check, then a
//! recorded state entry. `infection status` lists installed infections or
//! shows one in detail; `infection uninstall` reverses an install from its
//! recorded state.
//!
//! Installs here are *standalone* (recorded `owner` is `None`). Installing
//! over an infection a deployment owns is refused — remove the deployment
//! first.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use pandemic_protocol::spec::{parse_infection_spec, InfectionSpec};
use pandemic_protocol::AgentRequest;

use crate::apply::{build_plan_from_spec, parse_set_args};
use crate::service::agent_action;

pub async fn handle_infection_command(
    action: crate::InfectionAction,
    agent_secret: Option<String>,
    agent_secret_path: Option<PathBuf>,
) -> Result<()> {
    match action {
        crate::InfectionAction::Install { path, set } => {
            install(&path, &set, agent_secret, agent_secret_path).await
        }
        crate::InfectionAction::Status { name } => {
            status(name.as_deref(), agent_secret, agent_secret_path).await
        }
        crate::InfectionAction::Uninstall { name } => {
            uninstall(&name, agent_secret, agent_secret_path).await
        }
    }
}

async fn install(
    spec_path: &Path,
    set_args: &[String],
    agent_secret: Option<String>,
    agent_secret_path: Option<PathBuf>,
) -> Result<()> {
    let text = std::fs::read_to_string(spec_path)
        .with_context(|| format!("reading infection spec {}", spec_path.display()))?;
    let spec: InfectionSpec = parse_infection_spec(&text)
        .with_context(|| format!("parsing infection spec {}", spec_path.display()))?;

    let declared: Vec<String> = spec.variables.keys().cloned().collect();
    let set = parse_set_args(
        set_args,
        &declared,
        &format!("infection '{}'", spec.meta.name),
    )?;

    // Resolve + render everything (pure, local). The agent runs the Apply step.
    let plan = build_plan_from_spec(spec_path, &set)?;

    // Ownership guard: never install over an infection a deployment owns.
    let existing = agent_action(
        &AgentRequest::GetInfectionStatus {
            name: plan.name.clone(),
        },
        agent_secret.clone(),
        agent_secret_path.clone(),
    )
    .await;
    if let Ok(data) = &existing {
        if let Some(owner) = data
            .get("state")
            .and_then(|s| s.get("owner"))
            .and_then(|v| v.as_str())
        {
            bail!(
                "infection '{}' is owned by deployment '{owner}'. Remove the deployment first:\n  pandemic-cli deploy remove {owner}",
                plan.name
            );
        }
    }

    println!(
        "Installing infection '{}' v{} ({} file(s){}{})",
        plan.name,
        plan.version,
        plan.files.len() + usize::from(plan.unit.is_some()),
        plan.unit
            .as_ref()
            .map(|u| format!(", unit '{}'", u.name))
            .unwrap_or_default(),
        plan.attach
            .as_ref()
            .map(|a| format!(", attach '{a}'"))
            .unwrap_or_default(),
    );

    let name = plan.name.clone();
    agent_action(
        &AgentRequest::ApplyInfection { plan, owner: None },
        agent_secret,
        agent_secret_path,
    )
    .await?;

    println!("✅ Installed infection '{name}' (standalone)");
    Ok(())
}

async fn status(
    name: Option<&str>,
    agent_secret: Option<String>,
    agent_secret_path: Option<PathBuf>,
) -> Result<()> {
    match name {
        None => list(agent_secret, agent_secret_path).await,
        Some(name) => detail(name, agent_secret, agent_secret_path).await,
    }
}

async fn list(agent_secret: Option<String>, agent_secret_path: Option<PathBuf>) -> Result<()> {
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
    if infections.is_empty() {
        println!("No infections installed.");
        return Ok(());
    }
    println!(
        "{:<18} {:<10} {:<8} {:<24} {:<14} installed",
        "NAME", "VERSION", "KIND", "TARGET", "OWNER"
    );
    for inf in &infections {
        let kind = if inf.get("attach").is_some_and(|v| v.is_string()) {
            "attach"
        } else if inf.get("unit").is_some_and(|v| v.is_string()) {
            "unit"
        } else {
            "config"
        };
        let target = inf
            .get("attach")
            .and_then(|v| v.as_str())
            .or_else(|| inf.get("unit").and_then(|v| v.as_str()))
            .unwrap_or("-");
        let owner = inf
            .get("owner")
            .and_then(|v| v.as_str())
            .map(|o| format!("[{o}]"))
            .unwrap_or_else(|| "-".to_string());
        println!(
            "{:<18} {:<10} {:<8} {:<24} {:<14} {}",
            inf.get("name").and_then(|v| v.as_str()).unwrap_or("?"),
            inf.get("version").and_then(|v| v.as_str()).unwrap_or("?"),
            kind,
            target,
            owner,
            inf.get("installed_at")
                .and_then(|v| v.as_str())
                .unwrap_or("-"),
        );
    }
    Ok(())
}

pub(crate) fn active_str(value: Option<&serde_json::Value>) -> String {
    match value.and_then(|v| v.as_bool()) {
        Some(true) => "active".to_string(),
        Some(false) => "inactive".to_string(),
        None => "n/a".to_string(),
    }
}

async fn detail(
    name: &str,
    agent_secret: Option<String>,
    agent_secret_path: Option<PathBuf>,
) -> Result<()> {
    let data = agent_action(
        &AgentRequest::GetInfectionStatus {
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
    if let Some(description) = state.get("description").and_then(|v| v.as_str()) {
        if !description.is_empty() {
            println!("   {description}");
        }
    }
    if let Some(owner) = state.get("owner").and_then(|v| v.as_str()) {
        println!("   owner:     deployment '{owner}'");
    }
    if let Some(unit) = state.get("unit").and_then(|v| v.as_str()) {
        println!(
            "   unit:      {unit} ({})",
            active_str(data.get("unit_active"))
        );
    }
    if let Some(attach) = state.get("attach").and_then(|v| v.as_str()) {
        let sidecar = if name.starts_with("pandemic") {
            name.to_string()
        } else {
            format!("pandemic-{name}")
        };
        println!(
            "   attach:    {attach} ({})",
            active_str(data.get("target_active"))
        );
        println!(
            "   sidecar:   {sidecar} ({})",
            active_str(data.get("sidecar_active"))
        );
    }

    if let Some(files) = data.get("files").and_then(|v| v.as_array()) {
        if !files.is_empty() {
            println!("\n   files:");
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
                    "   [{mark}] {} ({}:{})",
                    file.get("target").and_then(|v| v.as_str()).unwrap_or("?"),
                    file.get("owner").and_then(|v| v.as_str()).unwrap_or("?"),
                    file.get("mode").and_then(|v| v.as_str()).unwrap_or("?"),
                );
            }
        }
    }

    let variables = state.get("variables").and_then(|v| v.as_object());
    if let Some(variables) = variables {
        if !variables.is_empty() {
            println!("\n   variables:");
            for (key, value) in variables {
                println!("   {key} = {value}");
            }
        }
    }

    let health = state.get("health_check").and_then(|v| v.as_array());
    if let Some(health) = health {
        if !health.is_empty() {
            println!(
                "\n   health:    {}",
                health
                    .iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            println!(
                "   interval:  {}s",
                state
                    .get("health_interval")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(30)
            );
        }
    }
    println!(
        "\n   installed: {}",
        state
            .get("installed_at")
            .and_then(|v| v.as_str())
            .unwrap_or("-")
    );
    Ok(())
}

async fn uninstall(
    name: &str,
    agent_secret: Option<String>,
    agent_secret_path: Option<PathBuf>,
) -> Result<()> {
    let data = agent_action(
        &AgentRequest::UninstallInfection {
            name: name.to_string(),
        },
        agent_secret,
        agent_secret_path,
    )
    .await?;

    println!("✅ Uninstalled infection '{name}'");
    let removed = data
        .get("removed_files")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if removed.is_empty() {
        println!("   (no recorded files to remove)");
    } else {
        for path in &removed {
            println!("   removed {}", path.as_str().unwrap_or("?"));
        }
    }
    if let Some(notes) = data.get("notes").and_then(|v| v.as_array()) {
        for note in notes {
            println!("   note: {}", note.as_str().unwrap_or("?"));
        }
    }
    Ok(())
}
