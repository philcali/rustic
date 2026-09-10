//! The privileged `Apply` step of the Plan/Apply boundary
//! (ideas/deployments.md, phase 5).
//!
//! The client (CLI or REST) does the pure `Plan` step — resolve variables,
//! render every template into a concrete [`pandemic_protocol::Plan`] — and
//! sends it via `ApplyInfection`/`ApplyDeployment`. This module executes that
//! concrete plan with the agent's privileged primitives, in order, and is
//! idempotent and safe to re-run:
//!
//! 1. packages (manager selected for *this* host),
//! 2. groups (skip existing),
//! 3. users (skip existing),
//! 4. rendered files,
//! 5. unit (write, daemon-reload, enable, restart-if-reapply) or attach,
//! 6. health check,
//! 7. record state (`owner` is `None` for standalone, the deployment name
//!    otherwise — that is what makes `RemoveDeployment` precise).
//!
//! Because the agent runs on the target host, package-manager selection and
//! the health check both run here, against the real host.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use anyhow::{anyhow, bail, Result};
use pandemic_common::sha256_hex;
use pandemic_protocol::spec::{
    select_packages, DeploymentRecordedInfection, DeploymentState, InfectionRecordedFile,
    InfectionState,
};
use pandemic_protocol::{ApplyDeploymentInfection, Plan};

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

/// Wrap a step's result: record the label on success, attach context on failure.
fn step<R>(applied: &mut Applied, label: String, result: Result<R>) -> Result<R> {
    match result {
        Ok(value) => {
            applied.record(label);
            Ok(value)
        }
        Err(err) => Err(applied.fail(label, err)),
    }
}

/// Resolve a bare command name via PATH (health checks run on this host).
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

/// Health check: up to 5 attempts, 2s apart, on this host.
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

/// Apply one concrete infection plan and record it. `owner` is `None` for a
/// standalone install, the deployment name when applied as part of one.
pub async fn apply_infection(plan: &Plan, owner: Option<&str>) -> Result<serde_json::Value> {
    let mut applied = Applied::new();

    // 1. Packages — pick the manager this host supports.
    if !plan.declared_packages.is_empty() {
        let supported: Vec<String> = crate::packages::detect_package_managers()
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        let (manager, packages) = select_packages(&plan.declared_packages, &supported)?
            .ok_or_else(|| anyhow!("no packages selected"))?;
        let label = format!("package install via {manager}: {}", packages.join(", "));
        step(
            &mut applied,
            label,
            crate::packages::install_packages(&manager, &packages).await,
        )?;
    }

    // 2. Groups (skip ones that already exist).
    let mut created_groups = Vec::new();
    if !plan.groups.is_empty() {
        let existing = crate::users::list_groups().await?;
        for group in &plan.groups {
            if existing.iter().any(|g| g == group) {
                tracing::info!("group '{group}' already exists, skipping");
                continue;
            }
            step(
                &mut applied,
                format!("create group '{group}'"),
                crate::users::create_group(group).await,
            )?;
            created_groups.push(group.clone());
        }
    }

    // 3. Users (skip ones that already exist).
    let mut created_users = Vec::new();
    if !plan.users.is_empty() {
        let existing = crate::users::list_users().await?;
        for (username, config) in &plan.users {
            if existing.iter().any(|u| u == username) {
                tracing::info!("user '{username}' already exists, skipping");
                continue;
            }
            step(
                &mut applied,
                format!("create user '{username}'"),
                crate::users::create_user(username, config).await,
            )?;
            created_users.push(username.clone());
        }
    }

    // 4. Rendered files.
    for file in &plan.files {
        let label = format!("write {}", file.target);
        step(
            &mut applied,
            label,
            crate::files::write_file(&file.target, &file.content, &file.owner, &file.mode).await,
        )?;
    }

    // 5. Unit this infection owns, or the attach flow.
    if let Some(unit) = &plan.unit {
        step(
            &mut applied,
            format!("write {}", unit.target),
            crate::files::write_file(&unit.target, &unit.content, "root", "0644").await,
        )?;
        step(
            &mut applied,
            "systemd daemon-reload".into(),
            crate::systemd::daemon_reload().await,
        )?;
        if unit.enable {
            step(
                &mut applied,
                format!("enable unit '{}'", unit.name),
                crate::systemd::execute_systemctl("enable", &unit.name)
                    .await
                    .map(|_| ()),
            )?;
        }
        // Re-applying over an existing install must restart, not fail to start.
        let action = if crate::state::is_installed(&plan.name) {
            tracing::info!("infection already installed — restarting instead of start");
            "restart"
        } else {
            "start"
        };
        step(
            &mut applied,
            format!("{action} unit '{}'", unit.name),
            crate::systemd::execute_systemctl(action, &unit.name)
                .await
                .map(|_| ()),
        )?;
    } else if let Some(attach) = &plan.attach {
        step(
            &mut applied,
            format!("attach unit '{attach}'"),
            crate::infection::attach_infection(&crate::infection::AttachParams {
                unit: attach.clone(),
                name: Some(plan.name.clone()),
                version: Some(plan.version.clone()),
                description: (!plan.description.is_empty()).then(|| plan.description.clone()),
                health_check: Some(plan.health_check.clone()),
                health_interval: Some(plan.health_interval),
                proxy_path: None,
            })
            .await,
        )?;
    }

    // 6. Health check (before the state is recorded as installed).
    if !plan.health_check.is_empty() {
        let label = format!("health check '{}'", plan.health_check.join(" "));
        step(
            &mut applied,
            label,
            run_health_check(&plan.health_check).await,
        )?;
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
        installed_at: None, // the record helper stamps it
        owner: owner.map(String::from),
    };
    step(
        &mut applied,
        "record infection state".into(),
        crate::state::record_infection(&plan.name, &state),
    )?;

    Ok(serde_json::json!({
        "name": plan.name,
        "owner": owner,
    }))
}

/// Refuse to install over infections that belong to someone else.
///
/// - a recorded deployment with the same name must install the *same*
///   infection set (re-apply/upgrade), otherwise refuse;
/// - each infection must be absent, or owned by this deployment. Standalone
///   and foreign-owned infections are never adopted.
fn preflight_ownership(dep_name: &str, infections: &[ApplyDeploymentInfection]) -> Result<()> {
    let current: Vec<String> = infections.iter().map(|i| i.name.clone()).collect();
    let recorded = crate::deployments::list_deployments()?;
    if let Some(rec) = recorded.iter().find(|d| d.name == dep_name) {
        let rec_infections: Vec<String> = rec.infections.iter().map(|i| i.name.clone()).collect();
        if rec_infections != current {
            bail!(
                "deployment '{dep_name}' is already installed with infections [{}]\nbut this spec installs [{}]. Remove it first:\n  pandemic-cli deploy remove {dep_name}",
                rec_infections.join(", "),
                current.join(", ")
            );
        }
    }

    let installed = crate::state::list_infections()?;
    for inf in infections {
        if let Some(rec) = installed.iter().find(|i| i.name == inf.name) {
            match &rec.owner {
                Some(owner) if owner == dep_name => {}
                Some(owner) => bail!(
                    "infection '{}' is already installed and owned by deployment '{owner}'.\nRemove it first:\n  pandemic-cli deploy remove {owner}",
                    inf.name
                ),
                None => bail!(
                    "infection '{}' is already installed standalone.\nRemove it first:\n  pandemic-cli infection uninstall {}",
                    inf.name,
                    inf.name
                ),
            }
        }
    }
    Ok(())
}

/// Apply a whole deployment: ownership pre-flight, each infection in order
/// (recorded with this deployment as owner), then the deployment record.
/// Mirrors [`crate::deployments::remove_deployment`].
pub async fn apply_deployment(
    name: &str,
    version: &str,
    variables: &BTreeMap<String, String>,
    infections: &[ApplyDeploymentInfection],
) -> Result<serde_json::Value> {
    preflight_ownership(name, infections)?;

    let mut applied: Vec<String> = Vec::new();
    for inf in infections {
        apply_infection(&inf.plan, Some(name))
            .await
            .map_err(|err| {
                anyhow!(
                    "deployment '{}' failed on infection '{}':\n\n{err}\n\n  no deployment record was written; infections already applied remain installed",
                    name, inf.name
                )
            })?;
        applied.push(inf.name.clone());
    }

    let state = DeploymentState {
        name: name.to_string(),
        version: version.to_string(),
        variables: variables.clone(),
        infections: infections
            .iter()
            .map(|inf| DeploymentRecordedInfection {
                name: inf.name.clone(),
                version: inf.version.clone(),
                order: inf.order,
                source: inf.source.clone(),
            })
            .collect(),
        installed_at: None, // the record helper stamps it
    };
    crate::deployments::record_deployment(name, &state)?;

    Ok(serde_json::json!({
        "name": name,
        "applied": applied,
        "record_written": true
    }))
}
