//! Recorded infection state (ideas/deployments.md, phase 3).
//!
//! After a successful `infection install`, the CLI tells the agent to
//! [`record_infection`], which writes `/etc/pandemic/infections/<name>/state.toml`
//! (dir 0700, file 0600 — the record holds resolved variable values that may
//! be secrets). [`load_state_in`] reads it back, falling back to the legacy
//! `<name>.toml` written by `service attach`, so status/uninstall cover both
//! install paths.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use pandemic_protocol::spec::InfectionState;

use crate::infection::{attach_service_name, validate_infection_name, INFECTIONS_DIR};

/// State file name inside a recorded infection's directory.
const STATE_FILE: &str = "state.toml";

/// Record an installed infection's state under the default root.
pub fn record_infection(name: &str, state: &InfectionState) -> Result<()> {
    record_infection_in(INFECTIONS_DIR, name, state)
}

/// Record `state` at `<root>/<name>/state.toml`, stamping `installed_at`.
///
/// The state dir is forced to 0700 and the file to 0600 on every write, so a
/// directory left behind by an older version is also tightened up.
pub fn record_infection_in(root: &str, name: &str, state: &InfectionState) -> Result<()> {
    validate_infection_name(name)?;
    let mut state = state.clone();
    state.name = name.to_string();
    state.installed_at = Some(Utc::now().to_rfc3339());

    let dir = Path::new(root).join(name);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
        .with_context(|| format!("setting mode 0700 on {}", dir.display()))?;

    let path = dir.join(STATE_FILE);
    let toml = toml::to_string_pretty(&state)
        .with_context(|| format!("serializing infection state for '{name}'"))?;
    std::fs::write(&path, toml).with_context(|| format!("writing {}", path.display()))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("setting mode 0600 on {}", path.display()))?;
    Ok(())
}

/// List all installed infections (recorded + legacy) under the default root.
pub fn list_infections() -> Result<Vec<InfectionState>> {
    list_infections_in(INFECTIONS_DIR)
}

/// List installed infections under `root`, sorted by name.
///
/// A missing root is not an error — the host simply has no infections yet.
/// Recorded infections come from `<root>/<name>/state.toml`; legacy
/// `service attach` configs from `<root>/<name>.toml`.
pub fn list_infections_in(root: &str) -> Result<Vec<InfectionState>> {
    let root_path = Path::new(root);
    if !root_path.is_dir() {
        return Ok(Vec::new());
    }
    let mut states = Vec::new();
    let entries =
        std::fs::read_dir(root_path).with_context(|| format!("reading {}", root_path.display()))?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            let state_file = path.join(STATE_FILE);
            if state_file.is_file() {
                states.push(read_state_file(&state_file)?);
            }
            // A state dir without state.toml is corrupt; `load_state_in`
            // reports it for the specific name, so listing stays usable.
        } else if path.is_file() && entry.file_name().to_string_lossy().ends_with(".toml") {
            states.push(legacy_state(&path)?);
        }
    }
    states.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(states)
}

fn read_state_file(path: &Path) -> Result<InfectionState> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let state: InfectionState = toml::from_str(&text)
        .with_context(|| format!("parsing infection state in {}", path.display()))?;
    Ok(state)
}

/// Load state for `name` from `root`, preferring the recorded
/// `<root>/<name>/state.toml` over the legacy `<root>/<name>.toml`.
pub fn load_state_in(root: &str, name: &str) -> Result<InfectionState> {
    validate_infection_name(name)?;
    let root_path = Path::new(root);
    let state_file = root_path.join(name).join(STATE_FILE);
    if state_file.is_file() {
        return read_state_file(&state_file);
    }
    let legacy = root_path.join(format!("{name}.toml"));
    if legacy.is_file() {
        return legacy_state(&legacy);
    }
    bail!("infection '{name}' is not installed")
}

/// True if `name` is installed (recorded or legacy) under the default root.
pub fn is_installed(name: &str) -> bool {
    is_installed_in(INFECTIONS_DIR, name)
}

/// True if `name` is installed (recorded or legacy) under `root`.
pub fn is_installed_in(root: &str, name: &str) -> bool {
    validate_infection_name(name).is_ok()
        && (Path::new(root).join(name).join(STATE_FILE).is_file()
            || Path::new(root).join(format!("{name}.toml")).is_file())
}

/// The on-disk shape written by `service attach` (pandemic-agent 0.4).
#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct LegacyRecord {
    infection: LegacyMeta,
    runtime: LegacyRuntime,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct LegacyMeta {
    name: String,
    version: String,
    description: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct LegacyRuntime {
    attach: String,
    health_check: Vec<String>,
    #[serde(default = "legacy_health_interval")]
    health_interval: u64,
}

fn legacy_health_interval() -> u64 {
    30
}

/// Interpret a legacy `<name>.toml` (from `service attach`) as state.
fn legacy_state(path: &Path) -> Result<InfectionState> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let legacy: LegacyRecord = toml::from_str(&text)
        .with_context(|| format!("parsing legacy infection config {}", path.display()))?;

    let name = if legacy.infection.name.is_empty() {
        path.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        legacy.infection.name.clone()
    };
    let attached = if legacy.runtime.attach.is_empty() {
        None
    } else {
        Some(legacy.runtime.attach.clone())
    };

    // Best effort: the file mtime is when it was attached.
    let installed_at = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .map(|t| DateTime::<Utc>::from(t).to_rfc3339());

    Ok(InfectionState {
        name,
        version: legacy.infection.version,
        description: legacy.infection.description,
        variables: BTreeMap::new(),
        groups: Vec::new(),
        users: Vec::new(),
        files: Vec::new(),
        unit: None,
        attach: attached,
        health_check: legacy.runtime.health_check,
        health_interval: legacy.runtime.health_interval,
        installed_at,
        owner: None,
    })
}

/// Full status of an installed infection: recorded state, live unit states,
/// and a per-file integrity check (exists + sha256 match).
pub async fn infection_status(name: &str) -> Result<serde_json::Value> {
    infection_status_in(INFECTIONS_DIR, name).await
}

/// Full status of infection `name` recorded under `root`: recorded state,
/// live unit states, and a per-file integrity check (exists + sha256 match).
pub async fn infection_status_in(root: &str, name: &str) -> Result<serde_json::Value> {
    let state = load_state_in(root, name)?;

    let unit_active = state.unit.as_deref().map(is_active);
    let sidecar_active = state
        .attach
        .as_ref()
        .map(|_| is_active(&attach_service_name(name)));
    let target_active = state.attach.as_deref().map(is_active);

    let files: Vec<serde_json::Value> = state
        .files
        .iter()
        .map(|f| {
            let exists = Path::new(&f.target).is_file();
            let hash_ok = exists
                && sha256_file(&f.target)
                    .map(|actual| actual == f.sha256)
                    .unwrap_or(false);
            serde_json::json!({
                "target": f.target,
                "owner": f.owner,
                "mode": f.mode,
                "exists": exists,
                "hash_ok": hash_ok,
            })
        })
        .collect();

    Ok(serde_json::json!({
        "state": state,
        "unit_active": unit_active,
        "sidecar_active": sidecar_active,
        "target_active": target_active,
        "files": files,
    }))
}

/// `systemctl is-active <unit>` as a bool (false when systemctl is missing).
pub fn is_active(unit: &str) -> bool {
    let output = Command::new("systemctl")
        .args(["is-active", unit])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    output.as_deref() == Some("active")
}

/// sha256 (hex) of a byte slice.
pub fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// sha256 (hex) of a file's contents; `None` when unreadable.
pub fn sha256_file(path: &str) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(sha256_bytes(&bytes))
}

/// Uninstall an infection under the default root.
pub async fn uninstall_infection(name: &str) -> Result<serde_json::Value> {
    uninstall_infection_in(INFECTIONS_DIR, name).await
}

/// Uninstall `name` from `root`: stop and remove what it owns, delete the
/// files it wrote (reverse order), and drop its state record.
///
/// Users and groups it created are left in place — they may be shared — and
/// reported in `notes`.
pub async fn uninstall_infection_in(root: &str, name: &str) -> Result<serde_json::Value> {
    uninstall_infection_in_impl(root, name, false).await
}

/// As [`uninstall_infection_in`], but without the "owned by deployment —
/// consider `deployment remove`" note: used by `RemoveDeployment`, whose
/// caller *is* the owner, so the hint is noise there.
pub async fn uninstall_owned_infection_in(root: &str, name: &str) -> Result<serde_json::Value> {
    uninstall_infection_in_impl(root, name, true).await
}

async fn uninstall_infection_in_impl(
    root: &str,
    name: &str,
    suppress_owner_note: bool,
) -> Result<serde_json::Value> {
    let state = load_state_in(root, name)?;
    let mut notes = Vec::new();

    if !suppress_owner_note {
        if let Some(owner) = &state.owner {
            notes.push(format!(
                "'{name}' is owned by deployment '{owner}' — consider `pandemic-cli deployment remove {owner}` instead"
            ));
        }
    }

    if state.attach.is_some() {
        // Attached infection: stop the sidecar, remove its unit + config.
        crate::infection::detach_infection(name).await?;
    } else if let Some(unit) = state.unit.as_deref() {
        // Unit this infection owns: stop it and drop its unit file.
        let unit_name = if unit.contains('.') {
            unit.to_string()
        } else {
            format!("{unit}.service")
        };
        let stopped = Command::new("systemctl")
            .args(["disable", "--now", &unit_name])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !stopped {
            notes.push(format!("could not stop unit '{unit_name}' (continuing)"));
        }
        let unit_path = PathBuf::from(format!("/etc/systemd/system/{unit_name}"));
        if unit_path.is_file() && std::fs::remove_file(&unit_path).is_ok() {
            notes.push(format!("removed unit file {unit_name}"));
        }
        crate::systemd::daemon_reload().await.ok();
    }

    // Remove the files the infection wrote, in reverse install order.
    let mut removed_files = Vec::new();
    for file in state.files.iter().rev() {
        let path = Path::new(&file.target);
        if path.is_file() && std::fs::remove_file(path).is_ok() {
            removed_files.push(file.target.clone());
        }
    }

    // Drop the state record: recorded dir, or the legacy single file.
    let state_dir = Path::new(root).join(name);
    let removed_state = if state_dir.join(STATE_FILE).is_file() {
        std::fs::remove_dir_all(&state_dir).is_ok()
    } else {
        let legacy = Path::new(root).join(format!("{name}.toml"));
        std::fs::remove_file(&legacy).is_ok()
    };

    if !state.users.is_empty() || !state.groups.is_empty() {
        notes.push(format!(
            "left users [{}] and groups [{}] in place (they may be shared)",
            state.users.join(", "),
            state.groups.join(", ")
        ));
    }

    Ok(serde_json::json!({
        "name": name,
        "removed_files": removed_files,
        "removed_state": removed_state,
        "notes": notes,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pandemic_protocol::spec::InfectionRecordedFile;

    fn temp_root(label: &str) -> String {
        let dir = std::env::temp_dir().join(format!(
            "pandemic-agent-state-{}-{label}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.to_string_lossy().into_owned()
    }

    fn cleanup(root: &str) {
        let _ = std::fs::remove_dir_all(root);
    }

    fn sample_state(name: &str) -> InfectionState {
        InfectionState {
            name: name.into(),
            version: "1.2.3".into(),
            description: "test infection".into(),
            variables: {
                let mut m = BTreeMap::new();
                m.insert("token".to_string(), "s3cret".to_string());
                m
            },
            groups: vec!["rest".into()],
            users: vec!["rest".into()],
            files: Vec::new(),
            unit: None,
            attach: None,
            health_check: vec!["true".into()],
            health_interval: 15,
            installed_at: None,
            owner: None,
        }
    }

    #[test]
    fn record_then_list_then_load() {
        let root = temp_root("roundtrip");

        record_infection_in(&root, "rest", &sample_state("rest")).unwrap();

        let dir_mode = std::fs::metadata(format!("{root}/rest"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(dir_mode & 0o777, 0o700, "state dir must be 0700");
        let file_mode = std::fs::metadata(format!("{root}/rest/state.toml"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(file_mode & 0o666, 0o600, "state file must be 0600");

        let listed = list_infections_in(&root).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "rest");
        assert!(
            listed[0].installed_at.is_some(),
            "agent stamps installed_at"
        );

        let loaded = load_state_in(&root, "rest").unwrap();
        assert_eq!(loaded.version, "1.2.3");
        assert_eq!(loaded.variables["token"], "s3cret");
        assert!(loaded.installed_at.is_some());

        cleanup(&root);
    }

    #[test]
    fn list_missing_root_is_empty() {
        let root = std::env::temp_dir().join(format!(
            "pandemic-agent-state-{}-missing",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        assert!(list_infections_in(&root.to_string_lossy())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn load_missing_name_errors() {
        let root = temp_root("missing-name");
        let err = load_state_in(&root, "nope").unwrap_err();
        assert!(err.to_string().contains("not installed"));
        cleanup(&root);
    }

    #[test]
    fn record_rejects_invalid_names() {
        let root = temp_root("badname");
        assert!(record_infection_in(&root, "../evil", &sample_state("x")).is_err());
        assert!(record_infection_in(&root, "UPPER", &sample_state("x")).is_err());
        assert!(!Path::new(&root).join("UPPER").exists());
        cleanup(&root);
    }

    #[test]
    fn legacy_config_loads_as_attach_state() {
        let root = temp_root("legacy");
        std::fs::write(
            format!("{root}/mosquitto.toml"),
            r#"
[infection]
name = "mosquitto"
version = "0.0.0"
description = "attached"

[runtime]
attach = "mosquitto"
health_interval = 45
"#,
        )
        .unwrap();

        assert!(is_installed_in(&root, "mosquitto"));
        let state = load_state_in(&root, "mosquitto").unwrap();
        assert_eq!(state.attach.as_deref(), Some("mosquitto"));
        assert!(state.unit.is_none());
        assert_eq!(state.health_interval, 45);
        assert!(
            state.installed_at.is_some(),
            "legacy install time comes from mtime"
        );

        let listed = list_infections_in(&root).unwrap();
        assert_eq!(
            listed.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["mosquitto"]
        );
        cleanup(&root);
    }

    #[test]
    fn list_merges_recorded_and_legacy_sorted() {
        let root = temp_root("mixed");
        std::fs::create_dir_all(format!("{root}/zeta")).unwrap();
        std::fs::write(
            format!("{root}/zeta/state.toml"),
            "name = \"zeta\"\nversion = \"1\"\n",
        )
        .unwrap();
        std::fs::write(
            format!("{root}/alpha.toml"),
            "[infection]\nname = \"alpha\"\nversion = \"0\"\n\n[runtime]\nattach = \"alpha\"\n",
        )
        .unwrap();

        let names: Vec<String> = list_infections_in(&root)
            .unwrap()
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(names, vec!["alpha".to_string(), "zeta".to_string()]);
        cleanup(&root);
    }

    #[tokio::test]
    async fn uninstall_removes_files_and_state() {
        let root = temp_root("uninstall");
        let target = std::env::temp_dir().join(format!(
            "pandemic-agent-state-{}-written.conf",
            std::process::id()
        ));
        std::fs::write(&target, "rendered content").unwrap();

        let mut state = sample_state("rest");
        state.files = vec![InfectionRecordedFile {
            target: target.to_string_lossy().into_owned(),
            sha256: "abc".into(),
            owner: "root".into(),
            mode: "0644".into(),
        }];
        record_infection_in(&root, "rest", &state).unwrap();

        let result = uninstall_infection_in(&root, "rest").await.unwrap();
        assert!(!target.exists(), "recorded file must be removed");
        assert!(
            !Path::new(&root).join("rest").exists(),
            "state dir must be removed"
        );
        assert_eq!(result["removed_files"][0], target.display().to_string());
        assert!(result["notes"][0].as_str().unwrap().contains("left users"));

        let _ = std::fs::remove_file(&target);
        cleanup(&root);
    }

    #[tokio::test]
    async fn uninstall_unknown_errors() {
        let root = temp_root("uninstall-unknown");
        let err = uninstall_infection_in(&root, "ghost").await.unwrap_err();
        assert!(err.to_string().contains("not installed"));
        cleanup(&root);
    }

    #[tokio::test]
    async fn owned_uninstall_suppresses_owner_hint() {
        let root = temp_root("owned-hint");
        let mut state = sample_state("rest");
        state.owner = Some("rest-mqtt".to_string());
        record_infection_in(&root, "rest", &state).unwrap();

        // The owning deployment removes it: the "consider `deployment remove`"
        // hint is noise (the caller *is* that deployment) and must not show.
        let owned: serde_json::Value = uninstall_owned_infection_in(&root, "rest").await.unwrap();
        let owned_notes = owned["notes"].as_array().unwrap();
        assert!(
            !owned_notes
                .iter()
                .any(|n| n.as_str().unwrap_or("").contains("consider")),
            "owner hint must be suppressed for the owning deployment: {owned_notes:?}"
        );

        // A standalone uninstall still gets the hint.
        let mut web = sample_state("web");
        web.owner = Some("other-deployment".to_string());
        record_infection_in(&root, "web", &web).unwrap();
        let standalone: serde_json::Value = uninstall_infection_in(&root, "web").await.unwrap();
        let standalone_notes = standalone["notes"].as_array().unwrap();
        assert!(
            standalone_notes.iter().any(|n| {
                n.as_str()
                    .unwrap_or("")
                    .contains("consider `pandemic-cli deployment remove other-deployment`")
            }),
            "owner hint must remain for standalone uninstalls: {standalone_notes:?}"
        );
        cleanup(&root);
    }
}
