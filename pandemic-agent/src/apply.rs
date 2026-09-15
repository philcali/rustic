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

/// Steps applied so far, plus the context needed to write the audit entry
/// (ideas/deployments.md, phase 8) when the operation ends — success or
/// failure. Written on drop, so *every* exit path is covered: the audit
/// log is exactly why a failed install still tells you which users/groups
/// it left behind.
struct Applied {
    /// (label, ok) in execution order.
    steps: Vec<(String, bool)>,
    created_users: Vec<String>,
    created_groups: Vec<String>,
    outcome: &'static str,
    error: Option<String>,
    name: String,
    version: String,
    owner: Option<String>,
    /// Audit log path (overridden in tests).
    pub(crate) audit_path: String,
    audited: bool,
}

impl Applied {
    fn new(plan: &Plan, owner: Option<&str>) -> Self {
        Self {
            steps: Vec::new(),
            created_users: Vec::new(),
            created_groups: Vec::new(),
            outcome: "failed", // optimistic only via `complete()`
            error: None,
            name: plan.name.clone(),
            version: plan.version.clone(),
            owner: owner.map(String::from),
            audit_path: pandemic_common::audit::AUDIT_FILE.to_string(),
            audited: false,
        }
    }

    fn record(&mut self, step: impl Into<String>) {
        self.steps.push((step.into(), true));
    }

    fn mark_failed(&mut self, step: &str, err: &anyhow::Error) {
        self.steps.push((step.into(), false));
        self.outcome = "failed";
        self.error = Some(err.to_string());
    }

    fn created_user(&mut self, name: String) {
        self.created_users.push(name);
    }

    fn created_group(&mut self, name: String) {
        self.created_groups.push(name);
    }

    /// The operation succeeded: write the audit entry now.
    fn complete(&mut self) {
        self.outcome = "ok";
        self.error = None;
        self.write_audit();
    }

    /// Enrich a failure with the steps already applied.
    fn error_message(&self, step: &str, err: &anyhow::Error) -> anyhow::Error {
        let mut msg = format!("install of this infection failed at step '{step}': {err}");
        let applied: Vec<&String> = self
            .steps
            .iter()
            .filter(|(_, ok)| *ok)
            .map(|(label, _)| label)
            .collect();
        if applied.is_empty() {
            msg.push_str("\n  no steps were applied");
        } else {
            msg.push_str("\n  steps already applied:");
            for s in &applied {
                msg.push_str(&format!("\n    - {s}"));
            }
        }
        msg.push_str("\n  re-run the install to retry — completed steps are idempotent");
        anyhow::anyhow!("{msg}")
    }

    /// Append the audit entry (best-effort: a log failure must not change
    /// the operation's outcome).
    fn write_audit(&mut self) {
        if self.audited {
            return;
        }
        self.audited = true;
        let entry = serde_json::json!({
            "ts": pandemic_common::audit::now_rfc3339(),
            "event": "apply_infection",
            "name": self.name,
            "version": self.version,
            "owner": self.owner,
            "outcome": self.outcome,
            "error": self.error,
            "steps": self
                .steps
                .iter()
                .map(|(label, ok)| {
                    serde_json::json!({ "step": label, "result": if *ok { "ok" } else { "failed" } })
                })
                .collect::<Vec<_>>(),
            "created_users": self.created_users,
            "created_groups": self.created_groups,
        });
        pandemic_common::audit::record_best_effort_in(&self.audit_path, &entry);
    }
}

impl Drop for Applied {
    fn drop(&mut self) {
        self.write_audit();
    }
}

/// Wrap a step's result: record the label on success, attach context on failure.
fn step<R>(applied: &mut Applied, label: String, result: Result<R>) -> Result<R> {
    match result {
        Ok(value) => {
            applied.record(label);
            Ok(value)
        }
        Err(err) => {
            applied.mark_failed(&label, &err);
            Err(applied.error_message(&label, &err))
        }
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
    let mut applied = Applied::new(plan, owner);

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
            applied.created_group(group.clone());
        }
    }

    // 3. Users (skip ones that already exist).
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
            applied.created_user(username.clone());
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
        groups: applied.created_groups.clone(),
        users: applied.created_users.clone(),
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

    applied.complete();
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
                "deployment '{dep_name}' is already installed with infections [{}]\nbut this spec installs [{}]. Remove it first:\n  pandemic-cli deployment remove {dep_name}",
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
                    "infection '{}' is already installed and owned by deployment '{owner}'.\nRemove it first:\n  pandemic-cli deployment remove {owner}",
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
///
/// **Rollback (phase 8):** if the run fails (an infection's apply fails, or
/// the deployment record cannot be written), the infections this run
/// *freshly* installed are best-effort uninstalled — files, unit, state
/// record; users/groups stay. Infections that already existed (re-applies)
/// are left in place. Both lists are reported in the error and the audit
/// entry. Standalone failed installs keep the idempotent-retry model.
///
/// Each infection writes its own `apply_infection` audit entry; this
/// wrapper adds the deployment-level summary (which infections made it,
/// where it stopped, what was rolled back / left applied).
pub async fn apply_deployment(
    name: &str,
    version: &str,
    variables: &BTreeMap<String, String>,
    infections: &[ApplyDeploymentInfection],
) -> Result<serde_json::Value> {
    let (applied, rolled_back, left_applied, rollback_errors, result) =
        apply_deployment_inner(name, version, variables, infections).await;

    let entry = serde_json::json!({
        "ts": pandemic_common::audit::now_rfc3339(),
        "event": "apply_deployment",
        "name": name,
        "version": version,
        "outcome": if result.is_ok() { "ok" } else { "failed" },
        "applied": applied,
        "rolled_back": rolled_back,
        "left_applied": left_applied,
        "rollback_errors": rollback_errors,
        "error": result.as_ref().err().map(|e| e.to_string()),
    });
    pandemic_common::audit::record_best_effort(&entry);

    result
}

/// Best-effort uninstall of the infections this run freshly installed, in
/// reverse order (users/groups are left — they may be shared, and a
/// half-finished install is not the moment to delete identity).
/// Returns `(rolled_back, left_applied, rollback_errors)`.
async fn rollback_fresh_installs_in(
    root: &str,
    fresh: &[String],
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut rolled_back = Vec::new();
    let mut left_applied = Vec::new();
    let mut rollback_errors = Vec::new();
    for name in fresh.iter().rev() {
        match crate::state::uninstall_owned_infection_in(root, name, false).await {
            Ok(_) => rolled_back.push(name.clone()),
            Err(e) => {
                left_applied.push(name.clone());
                rollback_errors.push(format!("'{name}': {e}"));
            }
        }
    }
    (rolled_back, left_applied, rollback_errors)
}

/// Rollback under the default infection root.
async fn rollback_fresh_installs(fresh: &[String]) -> (Vec<String>, Vec<String>, Vec<String>) {
    rollback_fresh_installs_in(crate::infection::INFECTIONS_DIR, fresh).await
}

/// Outcome of `apply_deployment_inner`: infections applied this run,
/// rolled back on failure, left applied on failure, rollback errors,
/// and the JSON result (or error).
type DeploymentOutcome = (
    Vec<String>,
    Vec<String>,
    Vec<String>,
    Vec<String>,
    Result<serde_json::Value>,
);

/// Failure bookkeeping for `apply_deployment_inner`: the rollback outcome
/// is folded into the error message so the operator sees it in the CLI/REST
/// response, not only in the audit log.
fn failure(
    applied: Vec<String>,
    rolled_back: Vec<String>,
    left_applied: Vec<String>,
    rollback_errors: Vec<String>,
    base_msg: String,
) -> DeploymentOutcome {
    let mut msg = base_msg;
    msg.push_str(&format!(
        "\n\n  rolled back (installed by this run): {}",
        if rolled_back.is_empty() {
            "(none)".to_string()
        } else {
            rolled_back.join(", ")
        }
    ));
    msg.push_str(&format!(
        "\n  left applied: {}",
        if left_applied.is_empty() {
            "(none)".to_string()
        } else {
            left_applied.join(", ")
        }
    ));
    for e in &rollback_errors {
        msg.push_str(&format!("\n  rollback failed: {e}"));
    }
    msg.push_str(
        "\n  re-run the install after fixing the failure — completed steps are idempotent",
    );
    (
        applied,
        rolled_back,
        left_applied,
        rollback_errors,
        Err(anyhow::anyhow!("{msg}")),
    )
}

async fn apply_deployment_inner(
    name: &str,
    version: &str,
    variables: &BTreeMap<String, String>,
    infections: &[ApplyDeploymentInfection],
) -> DeploymentOutcome {
    let none = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    if let Err(e) = preflight_ownership(name, infections) {
        return (none.0, none.1, none.2, none.3, Err(e));
    }

    let mut applied: Vec<String> = Vec::new();
    let mut fresh: Vec<String> = Vec::new(); // no state record before this run
    let mut preexisting: Vec<String> = Vec::new(); // had a record (re-apply)
    for inf in infections {
        let existed = crate::state::is_installed(&inf.name);
        if let Err(err) = apply_infection(&inf.plan, Some(name)).await {
            let (rolled_back, rollback_left, rollback_errors) =
                rollback_fresh_installs(&fresh).await;
            let mut left_applied = preexisting.clone();
            left_applied.extend(rollback_left);
            return failure(
                applied,
                rolled_back,
                left_applied,
                rollback_errors,
                format!(
                    "deployment '{}' failed on infection '{}':\n\n{err}",
                    name, inf.name
                ),
            );
        }
        applied.push(inf.name.clone());
        if existed {
            preexisting.push(inf.name.clone());
        } else {
            fresh.push(inf.name.clone());
        }
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
    if let Err(e) = crate::deployments::record_deployment(name, &state) {
        // The infections are installed but the record that would make them
        // removable is not — roll the fresh ones back rather than orphaning
        // them under a deployment name with no record.
        let (rolled_back, rollback_left, rollback_errors) = rollback_fresh_installs(&fresh).await;
        let mut left_applied = preexisting.clone();
        left_applied.extend(rollback_left);
        return failure(
            applied,
            rolled_back,
            left_applied,
            rollback_errors,
            format!(
                "deployment '{}': infections applied but the deployment record could not be written: {e}",
                name
            ),
        );
    }

    (
        applied.clone(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Ok(serde_json::json!({
            "name": name,
            "applied": applied,
            "record_written": true
        })),
    )
}

#[cfg(test)]
mod audit_tests {
    use super::*;

    fn plan(name: &str) -> Plan {
        Plan {
            name: name.into(),
            version: "9.9.9".into(),
            description: String::new(),
            variables: BTreeMap::new(),
            files: Vec::new(),
            unit: None,
            attach: None,
            health_check: Vec::new(),
            health_interval: 30,
            declared_packages: BTreeMap::new(),
            groups: Vec::new(),
            users: Vec::new(),
        }
    }

    fn temp_audit(label: &str) -> String {
        let dir = std::env::temp_dir().join(format!(
            "pandemic-agent-audit-{}-{label}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("audit.jsonl").to_string_lossy().into_owned()
    }

    fn read_entry(path: &str) -> serde_json::Value {
        let raw = std::fs::read_to_string(path).unwrap();
        serde_json::from_str(raw.lines().next().unwrap()).unwrap()
    }

    fn cleanup(path: &str) {
        let _ = std::fs::remove_dir_all(std::path::Path::new(path).parent().unwrap());
    }

    #[test]
    fn success_writes_ok_entry_with_steps_and_created() {
        let path = temp_audit("ok");
        let mut a = Applied::new(&plan("alpha"), Some("web-tier"));
        a.audit_path = path.clone();
        a.record("create group 'alpha'");
        a.created_group("alpha".into());
        a.record("record infection state");
        a.complete();

        let entry = read_entry(&path);
        assert_eq!(entry["event"], "apply_infection");
        assert_eq!(entry["name"], "alpha");
        assert_eq!(entry["owner"], "web-tier");
        assert_eq!(entry["outcome"], "ok");
        assert!(entry["error"].is_null());
        let steps = entry["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0]["step"], "create group 'alpha'");
        assert_eq!(steps[0]["result"], "ok");
        assert_eq!(entry["created_groups"], serde_json::json!(["alpha"]));
        assert!(
            entry["ts"].as_str().unwrap().len() >= 20,
            "RFC3339 timestamp"
        );
        cleanup(&path);
    }

    #[test]
    fn failure_writes_failed_entry_on_drop() {
        let path = temp_audit("fail");
        {
            let mut a = Applied::new(&plan("beta"), None);
            a.audit_path = path.clone();
            a.record("create user 'beta'");
            a.created_user("beta".into());
            a.mark_failed("write /etc/beta.conf", &anyhow::anyhow!("disk on fire"));
            // Simulates an early `?` return: no `complete()`, guard dropped.
        }

        let entry = read_entry(&path);
        assert_eq!(entry["outcome"], "failed");
        assert_eq!(entry["error"], "disk on fire");
        let steps = entry["steps"].as_array().unwrap();
        assert_eq!(steps[0]["result"], "ok");
        assert_eq!(steps[1]["step"], "write /etc/beta.conf");
        assert_eq!(steps[1]["result"], "failed");
        assert_eq!(
            entry["created_users"],
            serde_json::json!(["beta"]),
            "a failed install must still report what it created"
        );
        cleanup(&path);
    }

    #[test]
    fn drop_does_not_double_write() {
        let path = temp_audit("double");
        let mut a = Applied::new(&plan("gamma"), None);
        a.audit_path = path.clone();
        a.complete(); // writes once here
        drop(a); // Drop must not write again

        let raw = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            raw.lines().count(),
            1,
            "exactly one audit line per operation"
        );
        cleanup(&path);
    }
}

#[cfg(test)]
mod rollback_tests {
    use super::*;
    use pandemic_protocol::spec::{InfectionRecordedFile, InfectionState};

    fn temp_root(label: &str) -> String {
        let dir = std::env::temp_dir().join(format!(
            "pandemic-agent-rollback-{}-{label}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.to_string_lossy().into_owned()
    }

    fn record_with_file(root: &str, name: &str, target: &std::path::Path) {
        let state = InfectionState {
            name: name.into(),
            version: "1.0.0".into(),
            description: "test".into(),
            variables: BTreeMap::new(),
            groups: Vec::new(),
            users: Vec::new(),
            files: vec![InfectionRecordedFile {
                target: target.to_string_lossy().into_owned(),
                sha256: "abc".into(),
                owner: "root".into(),
                mode: "0644".into(),
            }],
            unit: None,
            attach: None,
            health_check: Vec::new(),
            health_interval: 30,
            installed_at: None,
            owner: Some("web-tier".into()),
        };
        crate::state::record_infection_in(root, name, &state).unwrap();
    }

    #[tokio::test]
    async fn rollback_uninstalls_fresh_infections_in_reverse() {
        let root = temp_root("fresh");
        let f1 = std::env::temp_dir().join(format!(
            "pandemic-agent-rollback-{}-one.conf",
            std::process::id()
        ));
        let f2 = std::env::temp_dir().join(format!(
            "pandemic-agent-rollback-{}-two.conf",
            std::process::id()
        ));
        std::fs::write(&f1, "one").unwrap();
        std::fs::write(&f2, "two").unwrap();
        record_with_file(&root, "alpha", &f1);
        record_with_file(&root, "beta", &f2);

        let fresh = vec!["alpha".to_string(), "beta".to_string()];
        let (rolled_back, left_applied, errors) = rollback_fresh_installs_in(&root, &fresh).await;

        assert_eq!(rolled_back, vec!["beta", "alpha"], "reverse install order");
        assert!(left_applied.is_empty(), "{left_applied:?}");
        assert!(errors.is_empty(), "{errors:?}");
        assert!(!f1.exists() && !f2.exists(), "recorded files removed");
        assert!(
            !crate::state::is_installed_in(&root, "alpha")
                && !crate::state::is_installed_in(&root, "beta"),
            "state records removed"
        );

        for f in [&f1, &f2] {
            let _ = std::fs::remove_file(f);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn rollback_reports_unremovable_installs_as_left() {
        let root = temp_root("left");
        // No state record: uninstall fails, the name lands in left_applied
        // with an error — never silently dropped.
        let fresh = vec!["ghost".to_string()];
        let (rolled_back, left_applied, errors) = rollback_fresh_installs_in(&root, &fresh).await;

        assert!(rolled_back.is_empty());
        assert_eq!(left_applied, vec!["ghost"]);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("ghost"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
