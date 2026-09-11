//! Recorded deployment state (ideas/deployments.md, phase 4).
//!
//! After a successful `deployment install`, the CLI tells the agent to
//! [`record_deployment`], which writes
//! `/etc/pandemic/deployments/<name>/state.toml` (dir 0700, file 0600 —
//! the record holds resolved shared variables that may be secrets).
//!
//! [`remove_deployment`] uninstalls the deployment's owned infections in
//! reverse install order and drops the record. Infections that are not
//! owned by the deployment (standalone or owned by another deployment)
//! are left untouched and reported.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use anyhow::{bail, Context, Result};
use chrono::Utc;
use pandemic_protocol::spec::DeploymentState;

use crate::infection::{validate_infection_name, INFECTIONS_DIR};
use crate::state::{infection_status_in, is_installed_in, uninstall_infection_in};

/// Where deployment records live on the host.
pub const DEPLOYMENTS_DIR: &str = "/etc/pandemic/deployments";

/// State file name inside a recorded deployment's directory.
const STATE_FILE: &str = "state.toml";

/// Record an installed deployment's state under the default root.
pub fn record_deployment(name: &str, state: &DeploymentState) -> Result<()> {
    record_deployment_in(DEPLOYMENTS_DIR, name, state)
}

/// Record `state` at `<root>/<name>/state.toml`, stamping `installed_at`.
///
/// The state dir is forced to 0700 and the file to 0600 on every write, so
/// a directory left behind by an older version is also tightened up.
pub fn record_deployment_in(root: &str, name: &str, state: &DeploymentState) -> Result<()> {
    validate_infection_name(name)?;
    for inf in &state.infections {
        validate_infection_name(&inf.name)
            .with_context(|| format!("infection '{}' in deployment '{name}'", inf.name))?;
    }
    let mut state = state.clone();
    state.name = name.to_string();
    state.installed_at = Some(Utc::now().to_rfc3339());

    let dir = Path::new(root).join(name);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
        .with_context(|| format!("setting mode 0700 on {}", dir.display()))?;

    let path = dir.join(STATE_FILE);
    let toml = toml::to_string_pretty(&state)
        .with_context(|| format!("serializing deployment state for '{name}'"))?;
    std::fs::write(&path, toml).with_context(|| format!("writing {}", path.display()))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("setting mode 0600 on {}", path.display()))?;
    Ok(())
}

/// List all installed deployments under the default root.
pub fn list_deployments() -> Result<Vec<DeploymentState>> {
    list_deployments_in(DEPLOYMENTS_DIR)
}

/// List installed deployments under `root`, sorted by name.
///
/// A missing root is not an error — the host simply has no deployments yet.
pub fn list_deployments_in(root: &str) -> Result<Vec<DeploymentState>> {
    let root_path = Path::new(root);
    if !root_path.is_dir() {
        return Ok(Vec::new());
    }
    let mut states: Vec<DeploymentState> = Vec::new();
    let entries =
        std::fs::read_dir(root_path).with_context(|| format!("reading {}", root_path.display()))?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            let state_file = path.join(STATE_FILE);
            if state_file.is_file() {
                let text = std::fs::read_to_string(&state_file)
                    .with_context(|| format!("reading {}", state_file.display()))?;
                states.push(toml::from_str(&text).with_context(|| {
                    format!("parsing deployment state in {}", state_file.display())
                })?);
            }
            // A state dir without state.toml is corrupt; listing stays usable.
        }
    }
    states.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(states)
}

/// True if `name` is recorded as a deployment under the default root.
pub fn is_deployed(name: &str) -> bool {
    is_deployed_in(DEPLOYMENTS_DIR, name)
}

/// True if `name` is recorded as a deployment under `root`.
pub fn is_deployed_in(root: &str, name: &str) -> bool {
    validate_infection_name(name).is_ok() && Path::new(root).join(name).join(STATE_FILE).is_file()
}

/// Load state for deployment `name` from `root`.
pub fn load_deployment_in(root: &str, name: &str) -> Result<DeploymentState> {
    validate_infection_name(name)?;
    let state_file = Path::new(root).join(name).join(STATE_FILE);
    if !state_file.is_file() {
        bail!("deployment '{name}' is not installed")
    }
    let text = std::fs::read_to_string(&state_file)
        .with_context(|| format!("reading {}", state_file.display()))?;
    toml::from_str(&text)
        .with_context(|| format!("parsing deployment state in {}", state_file.display()))
}

/// Status of one deployment under the default roots: recorded state plus
/// each owned infection's live status.
pub async fn deployment_status(name: &str) -> Result<serde_json::Value> {
    deployment_status_in(DEPLOYMENTS_DIR, INFECTIONS_DIR, name).await
}

/// Status of deployment `name`: its recorded state, and for each owned
/// infection (in install order) its live status — or `present: false` when
/// the infection is no longer installed. Missing infections are reported,
/// not an error, so a partially uninstalled deployment still shows.
pub async fn deployment_status_in(
    dep_root: &str,
    inf_root: &str,
    name: &str,
) -> Result<serde_json::Value> {
    let state = load_deployment_in(dep_root, name)?;

    let mut infections = Vec::new();
    for inf in &state.infections {
        match infection_status_in(inf_root, &inf.name).await {
            Ok(status) => infections.push(serde_json::json!({
                "name": inf.name,
                "present": true,
                "status": status,
            })),
            Err(_) => infections.push(serde_json::json!({
                "name": inf.name,
                "present": false,
            })),
        }
    }

    Ok(serde_json::json!({
        "state": state,
        "infections": infections,
    }))
}

/// Remove a deployment under the default roots.
pub async fn remove_deployment(name: &str) -> Result<serde_json::Value> {
    remove_deployment_in(DEPLOYMENTS_DIR, INFECTIONS_DIR, name).await
}

/// Remove deployment `name`: uninstall its owned infections in **reverse**
/// install order, then drop the record.
///
/// Ownership rules (ideas/deployments.md, phase 4):
/// - the infection's recorded `owner` is this deployment → uninstall it;
/// - the infection is missing on the host → skip with a note;
/// - the infection is standalone, or owned by another deployment → skip
///   with a note; it is never touched.
///
/// If any uninstall fails, the deployment record is **kept** (so
/// `deployment remove` can be re-run — already-removed infections are skipped
/// as missing) and `record_removed` is false.
pub async fn remove_deployment_in(
    dep_root: &str,
    inf_root: &str,
    name: &str,
) -> Result<serde_json::Value> {
    let state = load_deployment_in(dep_root, name)?;

    let mut removed = Vec::new();
    let mut skipped = Vec::new();
    let mut failed = Vec::new();
    let mut notes = Vec::new();

    for inf in state.infections.iter().rev() {
        if !is_installed_in(inf_root, &inf.name) {
            skipped.push(inf.name.clone());
            notes.push(format!(
                "'{}' is not installed on this host — skipped",
                inf.name
            ));
            continue;
        }

        let inf_state = match crate::state::load_state_in(inf_root, &inf.name) {
            Ok(s) => s,
            Err(e) => {
                failed.push(inf.name.clone());
                notes.push(format!(
                    "'{}': could not read recorded state: {e}",
                    inf.name
                ));
                continue;
            }
        };

        match inf_state.owner.as_deref() {
            Some(owner) if owner == name => match uninstall_infection_in(inf_root, &inf.name).await
            {
                Ok(result) => {
                    removed.push(inf.name.clone());
                    if let Some(inf_notes) = result.get("notes").and_then(|v| v.as_array()) {
                        for n in inf_notes {
                            if let Some(n) = n.as_str() {
                                notes.push(format!("{}: {n}", inf.name));
                            }
                        }
                    }
                }
                Err(e) => {
                    failed.push(inf.name.clone());
                    notes.push(format!("'{}': uninstall failed: {e}", inf.name));
                }
            },
            Some(owner) => {
                skipped.push(inf.name.clone());
                notes.push(format!(
                    "'{}' is owned by deployment '{owner}' — left untouched",
                    inf.name
                ));
            }
            None => {
                skipped.push(inf.name.clone());
                notes.push(format!(
                    "'{}' was installed standalone — left untouched",
                    inf.name
                ));
            }
        }
    }

    // Keep the record when anything failed, so removal can be retried.
    let record_removed = if failed.is_empty() {
        std::fs::remove_dir_all(Path::new(dep_root).join(name)).is_ok()
    } else {
        false
    };

    Ok(serde_json::json!({
        "name": name,
        "removed": removed,
        "skipped": skipped,
        "failed": failed,
        "notes": notes,
        "record_removed": record_removed,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pandemic_protocol::spec::{
        DeploymentRecordedInfection, DeploymentState, InfectionRecordedFile, InfectionState,
    };
    use std::collections::BTreeMap;
    use std::os::unix::fs::PermissionsExt;

    fn temp_root(label: &str) -> String {
        let dir = std::env::temp_dir().join(format!(
            "pandemic-agent-deploy-{}-{label}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.to_string_lossy().into_owned()
    }

    fn cleanup(root: &str) {
        let _ = std::fs::remove_dir_all(root);
    }

    fn sample_state(name: &str) -> DeploymentState {
        DeploymentState {
            name: name.into(),
            version: "1.0.0".into(),
            variables: {
                let mut m = BTreeMap::new();
                m.insert("port".to_string(), "8080".to_string());
                m
            },
            infections: vec![
                DeploymentRecordedInfection {
                    name: "rest".into(),
                    version: "0.4.0".into(),
                    order: 1,
                    source: "rest/infection.toml".into(),
                },
                DeploymentRecordedInfection {
                    name: "rest-metrics".into(),
                    version: "0.1.0".into(),
                    order: 2,
                    source: "metrics/infection.toml".into(),
                },
            ],
            installed_at: None,
        }
    }

    fn infection_state(name: &str, owner: Option<&str>) -> InfectionState {
        InfectionState {
            name: name.into(),
            version: "1.0.0".into(),
            description: "test".into(),
            variables: BTreeMap::new(),
            groups: Vec::new(),
            users: Vec::new(),
            files: Vec::new(),
            unit: None,
            attach: None,
            health_check: Vec::new(),
            health_interval: 30,
            installed_at: None,
            owner: owner.map(String::from),
        }
    }

    #[test]
    fn record_list_load_roundtrip() {
        let root = temp_root("roundtrip");

        record_deployment_in(&root, "rest-stack", &sample_state("rest-stack")).unwrap();

        let dir_mode = std::fs::metadata(format!("{root}/rest-stack"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(dir_mode & 0o777, 0o700, "state dir must be 0700");
        let file_mode = std::fs::metadata(format!("{root}/rest-stack/state.toml"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(file_mode & 0o666, 0o600, "state file must be 0600");

        let listed = list_deployments_in(&root).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "rest-stack");
        assert!(
            listed[0].installed_at.is_some(),
            "agent stamps installed_at"
        );

        let loaded = load_deployment_in(&root, "rest-stack").unwrap();
        assert_eq!(loaded.infections.len(), 2);
        assert_eq!(loaded.infections[0].name, "rest");
        assert_eq!(loaded.variables["port"], "8080");

        assert!(is_deployed_in(&root, "rest-stack"));
        assert!(!is_deployed_in(&root, "nope"));

        cleanup(&root);
    }

    #[test]
    fn list_missing_root_is_empty() {
        let root = std::env::temp_dir().join(format!(
            "pandemic-agent-deploy-{}-missing",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        assert!(list_deployments_in(&root.to_string_lossy())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn load_missing_name_errors() {
        let root = temp_root("missing-name");
        let err = load_deployment_in(&root, "nope").unwrap_err();
        assert!(err.to_string().contains("not installed"));
        cleanup(&root);
    }

    #[test]
    fn record_rejects_invalid_names() {
        let root = temp_root("badname");
        let state = sample_state("rest-stack");
        assert!(record_deployment_in(&root, "../evil", &state).is_err());
        assert!(record_deployment_in(&root, "UPPER", &state).is_err());

        let mut bad_infection = sample_state("rest-stack");
        bad_infection.infections[0].name = "Bad/Name".to_string();
        assert!(record_deployment_in(&root, "rest-stack", &bad_infection).is_err());
        assert!(!Path::new(&root).join("rest-stack").exists());
        cleanup(&root);
    }

    #[tokio::test]
    async fn status_reports_missing_infections_without_erroring() {
        let dep = temp_root("status-dep");
        let inf = temp_root("status-inf");

        record_deployment_in(&dep, "rest-stack", &sample_state("rest-stack")).unwrap();
        // Only one of the two owned infections is actually installed.
        crate::state::record_infection_in(
            &inf,
            "rest",
            &infection_state("rest", Some("rest-stack")),
        )
        .unwrap();

        let status = deployment_status_in(&dep, &inf, "rest-stack")
            .await
            .unwrap();
        let infections = status["infections"].as_array().unwrap();
        assert_eq!(infections.len(), 2);
        assert_eq!(infections[0]["name"], "rest");
        assert_eq!(infections[0]["present"], true);
        assert!(infections[0]["status"].get("state").is_some());
        assert_eq!(infections[1]["name"], "rest-metrics");
        assert_eq!(infections[1]["present"], false);

        cleanup(&dep);
        cleanup(&inf);
    }

    #[tokio::test]
    async fn remove_uninstalls_owned_infections_in_reverse_and_drops_record() {
        let dep = temp_root("remove-dep");
        let inf = temp_root("remove-inf");

        // Two real files, one per infection, so we can see what got removed.
        let f1 = std::env::temp_dir().join(format!(
            "pandemic-agent-deploy-{}-rest.conf",
            std::process::id()
        ));
        let f2 = std::env::temp_dir().join(format!(
            "pandemic-agent-deploy-{}-metrics.conf",
            std::process::id()
        ));
        std::fs::write(&f1, "one").unwrap();
        std::fs::write(&f2, "two").unwrap();

        let mut s1 = infection_state("rest", Some("rest-stack"));
        s1.files = vec![InfectionRecordedFile {
            target: f1.to_string_lossy().into_owned(),
            sha256: "abc".into(),
            owner: "root".into(),
            mode: "0644".into(),
        }];
        let mut s2 = infection_state("rest-metrics", Some("rest-stack"));
        s2.files = vec![InfectionRecordedFile {
            target: f2.to_string_lossy().into_owned(),
            sha256: "def".into(),
            owner: "root".into(),
            mode: "0644".into(),
        }];
        crate::state::record_infection_in(&inf, "rest", &s1).unwrap();
        crate::state::record_infection_in(&inf, "rest-metrics", &s2).unwrap();

        record_deployment_in(&dep, "rest-stack", &sample_state("rest-stack")).unwrap();

        let result = remove_deployment_in(&dep, &inf, "rest-stack")
            .await
            .unwrap();
        assert_eq!(result["removed"][0], "rest-metrics");
        assert_eq!(result["removed"][1], "rest");
        assert!(result["skipped"].as_array().unwrap().is_empty());
        assert!(result["failed"].as_array().unwrap().is_empty());
        assert_eq!(result["record_removed"], true);
        assert!(
            !f1.exists() && !f2.exists(),
            "owned infection files must be removed"
        );
        assert!(
            !is_deployed_in(&dep, "rest-stack"),
            "record must be dropped"
        );

        for f in [&f1, &f2] {
            let _ = std::fs::remove_file(f);
        }
        cleanup(&dep);
        cleanup(&inf);
    }

    #[tokio::test]
    async fn remove_leaves_standalone_and_foreign_infections_untouched() {
        let dep = temp_root("remove-foreign");
        let inf = temp_root("remove-foreign-inf");

        let mut s1 = infection_state("rest", Some("other-deployment"));
        s1.files = vec![InfectionRecordedFile {
            target: "/nonexistent/other.conf".into(),
            sha256: "x".into(),
            owner: "root".into(),
            mode: "0644".into(),
        }];
        crate::state::record_infection_in(&inf, "rest", &s1).unwrap();
        // "rest-metrics" is standalone (no owner).
        crate::state::record_infection_in(
            &inf,
            "rest-metrics",
            &infection_state("rest-metrics", None),
        )
        .unwrap();

        record_deployment_in(&dep, "rest-stack", &sample_state("rest-stack")).unwrap();

        let result = remove_deployment_in(&dep, &inf, "rest-stack")
            .await
            .unwrap();
        assert!(result["removed"].as_array().unwrap().is_empty());
        assert_eq!(result["skipped"].as_array().unwrap().len(), 2);
        assert_eq!(result["record_removed"], true);
        assert!(
            is_installed_in(&inf, "rest"),
            "foreign-owned infection must be untouched"
        );
        assert!(
            is_installed_in(&inf, "rest-metrics"),
            "standalone infection must be untouched"
        );
        let notes = result["notes"].as_array().unwrap();
        assert!(notes.iter().any(|n| n
            .as_str()
            .unwrap()
            .contains("owned by deployment 'other-deployment'")));
        assert!(notes
            .iter()
            .any(|n| n.as_str().unwrap().contains("installed standalone")));

        cleanup(&dep);
        cleanup(&inf);
    }

    #[tokio::test]
    async fn remove_skips_infections_missing_on_host() {
        let dep = temp_root("remove-missing");
        let inf = temp_root("remove-missing-inf");

        record_deployment_in(&dep, "rest-stack", &sample_state("rest-stack")).unwrap();

        let result = remove_deployment_in(&dep, &inf, "rest-stack")
            .await
            .unwrap();
        assert_eq!(result["skipped"].as_array().unwrap().len(), 2);
        assert_eq!(result["record_removed"], true);

        cleanup(&dep);
        cleanup(&inf);
    }

    #[tokio::test]
    async fn remove_unknown_errors() {
        let dep = temp_root("remove-unknown");
        let inf = temp_root("remove-unknown-inf");
        let err = remove_deployment_in(&dep, &inf, "ghost").await.unwrap_err();
        assert!(err.to_string().contains("not installed"));
        cleanup(&dep);
        cleanup(&inf);
    }
}
