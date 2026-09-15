//! Host preview / diff for the dry-run (ideas/deployments.md, phase 8).
//!
//! Read-only: reports what applying a concrete [`Plan`] would change —
//! per-file absent/unchanged/modified (by sha256), unit file + active
//! state, attach target active, groups/users already present, the package
//! manager this host would select, and whether a state record already
//! exists. No writes, no privileged mutation: safe to call before the
//! operator has approved anything.

use std::collections::BTreeMap;
use std::path::Path;

use pandemic_protocol::spec::select_packages;
use pandemic_protocol::{ApplyDeploymentInfection, Plan};

use crate::infection::INFECTIONS_DIR;
use crate::packages::detect_package_managers;
use crate::state::{is_active, is_installed_in, sha256_bytes, sha256_file};
use crate::users::{group_exists, user_exists};

/// Per-file diff states.
pub const FILE_ABSENT: &str = "absent";
pub const FILE_UNCHANGED: &str = "unchanged";
pub const FILE_MODIFIED: &str = "modified";

/// The unit name as systemd knows it (`.service` suffix implied).
fn unit_name(unit: &str) -> String {
    if unit.contains('.') {
        unit.to_string()
    } else {
        format!("{unit}.service")
    }
}

/// The package manager this host would use for the plan's declared
/// packages (canonical order, first supported), or `None` when the plan
/// declares none or the host supports none of them.
fn selected_package_manager(declared: &BTreeMap<String, Vec<String>>) -> Option<String> {
    let supported: Vec<String> = detect_package_managers()
        .into_iter()
        .map(String::from)
        .collect();
    select_packages(declared, &supported)
        .ok()
        .flatten()
        .map(|(manager, _)| manager)
}

/// Preview `plan` against the host, comparing against state in `inf_root`
/// (zero writes).
pub async fn preview_infection_in(plan: &Plan, inf_root: &str) -> serde_json::Value {
    let files = plan
        .files
        .iter()
        .map(|f| {
            let state = if !Path::new(&f.target).is_file() {
                FILE_ABSENT
            } else {
                match sha256_file(&f.target) {
                    Some(on_disk) if on_disk == sha256_bytes(f.content.as_bytes()) => {
                        FILE_UNCHANGED
                    }
                    _ => FILE_MODIFIED,
                }
            };
            serde_json::json!({ "target": f.target, "state": state })
        })
        .collect::<Vec<_>>();

    let unit = plan.unit.as_ref().map(|u| {
        serde_json::json!({
            "name": u.name,
            "file_exists": Path::new(&u.target).is_file(),
            "active": is_active(&unit_name(&u.name)),
        })
    });

    let attach = plan
        .attach
        .as_ref()
        .map(|t| serde_json::json!({ "target": t, "active": is_active(&unit_name(t)) }));

    let split = |wanted: &[String], exists: &dyn Fn(&str) -> bool| {
        let present: Vec<&String> = wanted.iter().filter(|w| exists(w)).collect();
        let missing: Vec<&String> = wanted.iter().filter(|w| !exists(w)).collect();
        serde_json::json!({ "present": present, "missing": missing })
    };

    serde_json::json!({
        "name": plan.name,
        "already_recorded": is_installed_in(inf_root, &plan.name),
        "selected_package_manager": selected_package_manager(&plan.declared_packages),
        "files": files,
        "unit": unit,
        "attach": attach,
        "groups": split(&plan.groups, &group_exists),
        "users": split(
            &plan.users.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(),
            &user_exists,
        ),
    })
}

/// Preview one concrete plan against the default state root.
pub async fn preview_infection(plan: &Plan) -> serde_json::Value {
    preview_infection_in(plan, INFECTIONS_DIR).await
}

/// Preview every infection of a deployment (in the given order).
pub async fn preview_deployment(infections: &[ApplyDeploymentInfection]) -> serde_json::Value {
    let mut previews = Vec::with_capacity(infections.len());
    for infection in infections {
        previews.push(preview_infection_in(&infection.plan, INFECTIONS_DIR).await);
    }
    serde_json::json!({ "infections": previews })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pandemic_protocol::{Plan, PlanUnit, RenderedFile, UserConfig};

    fn temp_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pandemic-agent-preview-{}-{label}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn plan_with_file(target: &str, content: &str, name: &str) -> Plan {
        Plan {
            name: name.into(),
            version: "1.0.0".into(),
            description: "preview test".into(),
            variables: BTreeMap::new(),
            files: vec![RenderedFile {
                target: target.into(),
                content: content.into(),
                owner: "root".into(),
                mode: "0644".into(),
            }],
            unit: None,
            attach: None,
            health_check: vec!["true".into()],
            health_interval: 30,
            declared_packages: BTreeMap::new(),
            groups: vec![],
            users: vec![(
                "root".into(),
                UserConfig {
                    shell: None,
                    home_dir: None,
                    groups: None,
                    system_user: None,
                },
            )],
        }
    }

    #[tokio::test]
    async fn file_diff_states_absent_unchanged_modified() {
        let dir = temp_dir("files");
        let target = dir.join("rendered.toml");
        let target = target.to_string_lossy().into_owned();
        let inf_root = temp_dir("files-root").to_string_lossy().into_owned();

        let content = "key = \"value\"";

        let absent =
            preview_infection_in(&plan_with_file(&target, content, "pv1"), &inf_root).await;
        assert_eq!(absent["files"][0]["state"], FILE_ABSENT);
        assert_eq!(absent["already_recorded"], false);

        std::fs::write(&target, content).unwrap();
        let unchanged =
            preview_infection_in(&plan_with_file(&target, content, "pv1"), &inf_root).await;
        assert_eq!(unchanged["files"][0]["state"], FILE_UNCHANGED);

        std::fs::write(&target, "key = \"CHANGED\"").unwrap();
        let modified =
            preview_infection_in(&plan_with_file(&target, content, "pv1"), &inf_root).await;
        assert_eq!(modified["files"][0]["state"], FILE_MODIFIED);

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&inf_root);
    }

    #[tokio::test]
    async fn groups_users_split_present_and_missing() {
        let inf_root = temp_dir("gu-root").to_string_lossy().into_owned();
        let mut plan = plan_with_file("/nonexistent-pv", "x", "pv2");
        plan.groups = vec!["root".into(), "definitely-not-a-pandemic-group".into()];

        let preview = preview_infection_in(&plan, &inf_root).await;
        assert_eq!(preview["groups"]["present"], serde_json::json!(["root"]));
        assert_eq!(
            preview["groups"]["missing"],
            serde_json::json!(["definitely-not-a-pandemic-group"])
        );
        assert_eq!(preview["users"]["present"], serde_json::json!(["root"]));

        let _ = std::fs::remove_dir_all(&inf_root);
    }

    #[tokio::test]
    async fn selected_manager_is_supported_or_none() {
        let inf_root = temp_dir("mgr-root").to_string_lossy().into_owned();
        let mut plan = plan_with_file("/nonexistent-pv", "x", "pv3");
        plan.declared_packages = BTreeMap::from([
            ("apt".into(), vec!["curl".into()]),
            ("dnf".into(), vec!["curl".into()]),
            ("apk".into(), vec!["curl".into()]),
            ("pacman".into(), vec!["curl".into()]),
            ("zypper".into(), vec!["curl".into()]),
        ]);

        let preview = preview_infection_in(&plan, &inf_root).await;
        let selected = preview["selected_package_manager"]
            .as_str()
            .map(String::from);
        let supported = detect_package_managers();
        match selected {
            Some(m) => assert!(
                supported.iter().any(|s| *s == m),
                "selected '{m}' must be a supported manager on this host ({supported:?})"
            ),
            None => assert!(
                supported.is_empty(),
                "host supports {supported:?} but none selected"
            ),
        }

        let _ = std::fs::remove_dir_all(&inf_root);
    }

    #[tokio::test]
    async fn unit_preview_reports_file_and_active() {
        let dir = temp_dir("unit");
        let unit_target = dir.join("pv.service");
        let inf_root = temp_dir("unit-root").to_string_lossy().into_owned();

        let mut plan = plan_with_file("/nonexistent-pv", "x", "pv4");
        plan.unit = Some(PlanUnit {
            name: "pv-unit".into(),
            target: unit_target.to_string_lossy().into_owned(),
            content: "[Service]\nExecStart=/bin/true\n".into(),
            enable: true,
        });

        let preview = preview_infection_in(&plan, &inf_root).await;
        assert_eq!(preview["unit"]["name"], "pv-unit");
        assert_eq!(preview["unit"]["file_exists"], false);
        assert_eq!(preview["unit"]["active"], false);

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&inf_root);
    }

    #[tokio::test]
    async fn already_recorded_reflects_state_root() {
        let inf_root = temp_dir("recorded-root");
        let inf_root = inf_root.to_string_lossy().into_owned();
        let plan = plan_with_file("/nonexistent-pv", "x", "pv5");

        assert_eq!(
            preview_infection_in(&plan, &inf_root).await["already_recorded"],
            false
        );

        let state = pandemic_protocol::spec::InfectionState {
            name: "pv5".into(),
            version: "1.0.0".into(),
            description: "test".into(),
            variables: BTreeMap::new(),
            groups: vec![],
            users: vec![],
            files: vec![],
            unit: None,
            attach: None,
            health_check: vec!["true".into()],
            health_interval: 30,
            installed_at: None,
            owner: None,
        };
        crate::state::record_infection_in(&inf_root, "pv5", &state).unwrap();
        assert_eq!(
            preview_infection_in(&plan, &inf_root).await["already_recorded"],
            true
        );

        let _ = std::fs::remove_dir_all(&inf_root);
    }
}
