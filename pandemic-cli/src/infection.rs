//! Spec-driven infection lifecycle (ideas/deployments.md, phase 3).
//!
//! `infection install` resolves the spec's variables, renders every template,
//! and applies the plan through individual agent primitives — packages,
//! groups/users, files, unit or attach, health check, then a recorded state
//! entry. `infection status` lists installed infections or shows one in
//! detail; `infection uninstall` reverses an install from its recorded state.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use pandemic_protocol::spec::{
    canonical_unit_name, is_variable_name, parse_infection_spec, render_template,
    resolve_infection, InfectionRecordedFile, InfectionSpec, InfectionState,
    KNOWN_PACKAGE_MANAGERS,
};
use pandemic_protocol::{AgentRequest, UserConfig};
use sha2::{Digest, Sha256};

use crate::service::agent_action;

/// The fully rendered, pre-flight plan for one infection install.
struct Plan {
    name: String,
    version: String,
    description: String,
    variables: BTreeMap<String, String>,
    files: Vec<RenderedFile>,
    unit: Option<PlanUnit>,
    attach: Option<String>,
    health_check: Vec<String>,
    health_interval: u64,
    packages: Option<(String, Vec<String>)>,
    groups: Vec<String>,
    users: Vec<(String, UserConfig)>,
}

/// A rendered file ready to write (rendered content + host placement).
struct RenderedFile {
    target: String,
    content: String,
    owner: String,
    mode: String,
}

/// The unit an install owns, rendered to /etc/systemd/system/.
struct PlanUnit {
    name: String,
    target: String,
    content: String,
    enable: bool,
}

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

/// Parse `--set key=value` pairs against the spec's declared variables.
fn parse_set(args: &[String], spec: &InfectionSpec) -> Result<BTreeMap<String, String>> {
    let mut set = BTreeMap::new();
    for arg in args {
        let (key, value) = arg
            .split_once('=')
            .ok_or_else(|| anyhow!("invalid --set '{arg}': expected key=value"))?;
        if !is_variable_name(key) {
            bail!("invalid variable name '{key}' in --set '{arg}'");
        }
        if !spec.variables.contains_key(key) {
            bail!(
                "unknown variable '{key}' for infection '{}': declared variables are [{}]",
                spec.meta.name,
                spec.variables
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        set.insert(key.to_string(), value.to_string());
    }
    Ok(set)
}

/// Locate a template: next to the spec first, then in `files/`.
fn find_template(spec_dir: &Path, key: &str) -> Option<PathBuf> {
    [spec_dir.join(key), spec_dir.join("files").join(key)]
        .into_iter()
        .find(|candidate| candidate.is_file())
}

/// Resolve a bare command name via PATH (health checks run on the CLI host).
fn resolve_command(name: &str) -> Result<String> {
    if name.contains('/') {
        return Ok(name.to_string());
    }
    let path_var = std::env::var("PATH").unwrap_or_default();
    for dir in path_var.split(':') {
        if dir.is_empty() {
            continue;
        }
        let candidate = Path::new(dir).join(name);
        if candidate.is_file() {
            let mode = std::fs::metadata(&candidate)
                .map(|m| m.permissions().mode())
                .unwrap_or(0);
            if mode & 0o111 != 0 {
                return Ok(candidate.to_string_lossy().into_owned());
            }
        }
    }
    bail!("health check command '{name}' not found in PATH")
}

fn sha256_hex(content: &str) -> String {
    hex::encode(Sha256::digest(content.as_bytes()))
}

/// Resolve variables and render every template. Fails before anything is
/// applied: unknown variables, missing templates, or unresolvable packages.
async fn build_plan(
    spec: &InfectionSpec,
    spec_dir: &Path,
    set: &BTreeMap<String, String>,
    agent_secret: Option<String>,
    agent_secret_path: Option<PathBuf>,
) -> Result<Plan> {
    let variables = resolve_infection(spec, &BTreeMap::new(), &BTreeMap::new(), set)
        .with_context(|| format!("resolving variables for infection '{}'", spec.meta.name))?;

    let mut files = Vec::new();
    for (source, placement) in &spec.files {
        let template_path = find_template(spec_dir, source).ok_or_else(|| {
            anyhow!(
                "template '{source}' not found next to the spec (in {}) or in {}/files/",
                spec_dir.display(),
                spec_dir.display()
            )
        })?;
        let raw = std::fs::read_to_string(&template_path)
            .with_context(|| format!("reading template {}", template_path.display()))?;
        let content = render_template(&raw, &variables)
            .with_context(|| format!("rendering template '{source}'"))?;
        files.push(RenderedFile {
            target: placement.target.clone(),
            content,
            owner: placement.owner.clone().unwrap_or_else(|| "root".into()),
            mode: placement.mode.clone().unwrap_or_else(|| "0644".into()),
        });
    }

    let mut unit = None;
    let mut attach = None;
    if let Some(systemd) = &spec.systemd {
        if let Some(unit_file) = &systemd.unit_file {
            let template_path = find_template(spec_dir, unit_file).ok_or_else(|| {
                anyhow!(
                    "unit file template '{unit_file}' not found next to the spec (in {}) or in {}/files/",
                    spec_dir.display(),
                    spec_dir.display()
                )
            })?;
            let raw = std::fs::read_to_string(&template_path)
                .with_context(|| format!("reading unit template {}", template_path.display()))?;
            let content = render_template(&raw, &variables)
                .with_context(|| format!("rendering unit template '{unit_file}'"))?;
            let file = if unit_file.contains('.') {
                unit_file.clone()
            } else {
                format!("{unit_file}.service")
            };
            let target = format!("/etc/systemd/system/{file}");
            unit = Some(PlanUnit {
                name: canonical_unit_name(&file),
                target,
                content,
                enable: systemd.enable,
            });
        }
        if let Some(target) = &systemd.attach {
            attach = Some(target.clone());
        }
    }

    let health_check: Vec<String> = spec
        .health
        .check
        .iter()
        .map(|part| render_template(part, &variables))
        .collect::<Result<_>>()
        .with_context(|| "rendering health check command")?;
    let health_interval = spec.health.interval;

    let packages = if spec.packages.values().any(|v| !v.is_empty()) {
        let caps = agent_action(
            &AgentRequest::GetCapabilities,
            agent_secret.clone(),
            agent_secret_path.clone(),
        )
        .await?;
        let supported: Vec<String> = caps
            .get("package_managers")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let chosen = KNOWN_PACKAGE_MANAGERS
            .iter()
            .copied()
            .find(|manager| {
                spec.packages
                    .get(*manager)
                    .map(|list| {
                        !list.is_empty() && supported.iter().any(|s| s.as_str() == *manager)
                    })
                    .unwrap_or(false)
            })
            .map(|manager| (manager.to_string(), spec.packages[manager].clone()));
        Some(chosen.ok_or_else(|| {
            anyhow!(
                "spec lists packages for [{}] but this host supports [{}]",
                spec.packages.keys().cloned().collect::<Vec<_>>().join(", "),
                if supported.is_empty() {
                    "no known manager (apt, dnf, pacman, apk, zypper)".to_string()
                } else {
                    supported.join(", ")
                }
            )
        })?)
    } else {
        None
    };

    let groups: Vec<String> = spec.groups.keys().cloned().collect();
    let users: Vec<(String, UserConfig)> = spec
        .users
        .iter()
        .map(|(username, user_spec)| {
            (
                username.clone(),
                UserConfig {
                    shell: None,
                    home_dir: None,
                    groups: Some(vec![user_spec
                        .group
                        .clone()
                        .unwrap_or_else(|| username.clone())]),
                    system_user: Some(user_spec.system),
                },
            )
        })
        .collect();

    Ok(Plan {
        name: spec.meta.name.clone(),
        version: spec.meta.version.clone(),
        description: spec.meta.description.clone(),
        variables,
        files,
        unit,
        attach,
        health_check,
        health_interval,
        packages,
        groups,
        users,
    })
}

/// Steps already applied — reported when a later step fails.
struct Applied {
    steps: Vec<String>,
}

impl Applied {
    fn new() -> Self {
        Self { steps: Vec::new() }
    }

    fn record(&mut self, step: impl Into<String>) {
        self.steps.push(step.into());
    }

    fn fail(&self, step: impl Into<String>, err: anyhow::Error) -> anyhow::Error {
        let step = step.into();
        let mut msg = format!("install of this infection failed at step '{step}': {err}");
        if self.steps.is_empty() {
            msg.push_str("\n  no steps were applied");
        } else {
            msg.push_str("\n  steps already applied:");
            for s in &self.steps {
                msg.push_str(&format!("\n    - {s}"));
            }
        }
        msg.push_str("\n  re-run the install to retry — completed steps are idempotent");
        anyhow::anyhow!("{msg}")
    }
}

/// Run one agent step, recording it on success and reporting context on failure.
async fn do_step(
    applied: &mut Applied,
    label: impl Into<String>,
    request: &AgentRequest,
    agent_secret: Option<String>,
    agent_secret_path: Option<PathBuf>,
) -> Result<serde_json::Value> {
    let label = label.into();
    match agent_action(request, agent_secret, agent_secret_path).await {
        Ok(data) => {
            applied.record(label.clone());
            Ok(data)
        }
        Err(err) => Err(applied.fail(label, err)),
    }
}

/// Health check: up to 5 attempts, 2s apart, on the CLI host.
async fn run_health_check(cmd: &[String]) -> Result<()> {
    let (head, rest) = cmd
        .split_first()
        .ok_or_else(|| anyhow!("empty health check command"))?;
    let head = resolve_command(head)?;
    let mut last = String::new();
    for attempt in 1..=5u32 {
        match Command::new(&head).args(rest).status() {
            Ok(s) if s.success() => return Ok(()),
            Ok(s) => last = format!("exited {s:?}"),
            Err(e) => last = e.to_string(),
        }
        if attempt < 5 {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    }
    bail!(
        "health check '{}' failed after 5 attempts: {last}",
        cmd.join(" ")
    )
}

async fn install(
    spec_path: &Path,
    set_args: &[String],
    agent_secret: Option<String>,
    agent_secret_path: Option<PathBuf>,
) -> Result<()> {
    let text = std::fs::read_to_string(spec_path)
        .with_context(|| format!("reading infection spec {}", spec_path.display()))?;
    let spec = parse_infection_spec(&text)
        .with_context(|| format!("parsing infection spec {}", spec_path.display()))?;
    let spec_dir = spec_path
        .parent()
        .map(Path::to_path_buf)
        .filter(|p| p != Path::new(""))
        .unwrap_or_else(|| PathBuf::from("."));
    let set = parse_set(set_args, &spec)?;

    // Resolve + render everything before touching the host.
    let plan = build_plan(
        &spec,
        &spec_dir,
        &set,
        agent_secret.clone(),
        agent_secret_path.clone(),
    )
    .await?;
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

    let mut applied = Applied::new();

    // 1. Packages.
    if let Some((manager, packages)) = &plan.packages {
        do_step(
            &mut applied,
            format!("package install via {manager}: {}", packages.join(", ")),
            &AgentRequest::PackageInstall {
                manager: manager.clone(),
                packages: packages.clone(),
            },
            agent_secret.clone(),
            agent_secret_path.clone(),
        )
        .await?;
    }

    // 2. Groups (skip ones that already exist).
    let mut created_groups = Vec::new();
    if !plan.groups.is_empty() {
        let data = do_step(
            &mut applied,
            "list groups",
            &AgentRequest::ListGroups,
            agent_secret.clone(),
            agent_secret_path.clone(),
        )
        .await?;
        let existing: Vec<String> = data
            .get("groups")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        for group in &plan.groups {
            if existing.iter().any(|g| g == group) {
                println!("   group '{group}' already exists, skipping");
                continue;
            }
            do_step(
                &mut applied,
                format!("create group '{group}'"),
                &AgentRequest::GroupCreate {
                    groupname: group.clone(),
                },
                agent_secret.clone(),
                agent_secret_path.clone(),
            )
            .await?;
            created_groups.push(group.clone());
        }
    }

    // 3. Users (skip ones that already exist).
    let mut created_users = Vec::new();
    if !plan.users.is_empty() {
        let data = do_step(
            &mut applied,
            "list users",
            &AgentRequest::ListUsers,
            agent_secret.clone(),
            agent_secret_path.clone(),
        )
        .await?;
        let existing: Vec<String> = data
            .get("users")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        for (username, config) in &plan.users {
            if existing.iter().any(|u| u == username) {
                println!("   user '{username}' already exists, skipping");
                continue;
            }
            do_step(
                &mut applied,
                format!("create user '{username}'"),
                &AgentRequest::UserCreate {
                    username: username.clone(),
                    config: config.clone(),
                },
                agent_secret.clone(),
                agent_secret_path.clone(),
            )
            .await?;
            created_users.push(username.clone());
        }
    }

    // 4. Rendered files.
    for file in &plan.files {
        do_step(
            &mut applied,
            format!("write {}", file.target),
            &AgentRequest::WriteFile {
                path: file.target.clone(),
                content: file.content.clone(),
                owner: file.owner.clone(),
                mode: file.mode.clone(),
            },
            agent_secret.clone(),
            agent_secret_path.clone(),
        )
        .await?;
    }

    // 5. Unit this infection owns, or the attach flow.
    let mut action = "start".to_string();
    if let Some(unit) = &plan.unit {
        do_step(
            &mut applied,
            format!("write {}", unit.target),
            &AgentRequest::WriteFile {
                path: unit.target.clone(),
                content: unit.content.clone(),
                owner: "root".into(),
                mode: "0644".into(),
            },
            agent_secret.clone(),
            agent_secret_path.clone(),
        )
        .await?;
        do_step(
            &mut applied,
            "systemd daemon-reload",
            &AgentRequest::SystemdControl {
                action: "daemon-reload".into(),
                service: String::new(),
            },
            agent_secret.clone(),
            agent_secret_path.clone(),
        )
        .await?;
        if unit.enable {
            do_step(
                &mut applied,
                format!("enable unit '{}'", unit.name),
                &AgentRequest::SystemdControl {
                    action: "enable".into(),
                    service: unit.name.clone(),
                },
                agent_secret.clone(),
                agent_secret_path.clone(),
            )
            .await?;
        }
        // Re-applying over an existing install must restart, not fail to start.
        let already = agent_action(
            &AgentRequest::GetInfectionStatus {
                name: plan.name.clone(),
            },
            agent_secret.clone(),
            agent_secret_path.clone(),
        )
        .await
        .is_ok();
        if already {
            action = "restart".into();
            println!("   infection already installed — restarting instead of start");
        }
        do_step(
            &mut applied,
            format!("{action} unit '{}'", unit.name),
            &AgentRequest::SystemdControl {
                action: action.clone(),
                service: unit.name.clone(),
            },
            agent_secret.clone(),
            agent_secret_path.clone(),
        )
        .await?;
    } else if let Some(attach) = &plan.attach {
        do_step(
            &mut applied,
            format!("attach unit '{attach}'"),
            &AgentRequest::AttachInfection {
                unit: attach.clone(),
                name: Some(plan.name.clone()),
                version: Some(plan.version.clone()),
                description: (!plan.description.is_empty()).then(|| plan.description.clone()),
                health_check: Some(plan.health_check.clone()),
                health_interval: Some(plan.health_interval),
                proxy_path: None,
            },
            agent_secret.clone(),
            agent_secret_path.clone(),
        )
        .await?;
    }

    // 6. Health check on the CLI host (before the state is recorded as
    //    installed; a failing check is reported with the steps already taken).
    if !plan.health_check.is_empty() {
        if let Err(err) = run_health_check(&plan.health_check).await {
            return Err(applied.fail(
                format!("health check '{}'", plan.health_check.join(" ")),
                err,
            ));
        }
        applied.record(format!("health check '{}'", plan.health_check.join(" ")));
    }

    // 7. Record state so status/uninstall are data-driven.
    let mut recorded_files: Vec<InfectionRecordedFile> = plan
        .files
        .iter()
        .map(|f| InfectionRecordedFile {
            target: f.target.clone(),
            sha256: sha256_hex(&f.content),
            owner: f.owner.clone(),
            mode: f.mode.clone(),
        })
        .collect();
    if let Some(unit) = &plan.unit {
        recorded_files.push(InfectionRecordedFile {
            target: unit.target.clone(),
            sha256: sha256_hex(&unit.content),
            owner: "root".into(),
            mode: "0644".into(),
        });
    }

    let state = InfectionState {
        name: plan.name.clone(),
        version: plan.version.clone(),
        description: plan.description.clone(),
        variables: plan.variables.clone(),
        groups: created_groups,
        users: created_users,
        files: recorded_files,
        unit: plan.unit.as_ref().map(|u| u.name.clone()),
        attach: plan.attach.clone(),
        health_check: plan.health_check.clone(),
        health_interval: plan.health_interval,
        installed_at: None, // the agent stamps it
        owner: None,        // standalone install
    };
    do_step(
        &mut applied,
        "record infection state",
        &AgentRequest::RecordInfection {
            name: plan.name.clone(),
            state,
        },
        agent_secret,
        agent_secret_path,
    )
    .await?;

    println!("✅ Installed infection '{}'", plan.name);
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
        "{:<18} {:<10} {:<8} {:<24} installed",
        "NAME", "VERSION", "KIND", "TARGET"
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
        println!(
            "{:<18} {:<10} {:<8} {:<24} {}",
            inf.get("name").and_then(|v| v.as_str()).unwrap_or("?"),
            inf.get("version").and_then(|v| v.as_str()).unwrap_or("?"),
            kind,
            target,
            inf.get("installed_at")
                .and_then(|v| v.as_str())
                .unwrap_or("-"),
        );
    }
    Ok(())
}

fn active_str(value: Option<&serde_json::Value>) -> String {
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
