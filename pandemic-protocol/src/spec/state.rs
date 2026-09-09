//! Recorded install state (ideas/deployments.md, phases 3-4).
//!
//! After a successful `infection install`, the agent records an
//! [`InfectionState`] at `/etc/pandemic/infections/<name>/state.toml`
//! (0600, root-only). The record is what `infection status` reports and
//! what `infection uninstall` reverses — ownership is data, not inference.
//!
//! After a successful `deploy install`, the agent records a
//! [`DeploymentState`] at `/etc/pandemic/deployments/<name>/state.toml`
//! (0600, root-only): the resolved shared variables and the ordered
//! infections it owns, so `deploy remove` can uninstall them in reverse
//! and leave standalone infections alone.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A file an infection wrote, with the hash of what was written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InfectionRecordedFile {
    /// Absolute host path.
    pub target: String,
    /// sha256 (hex) of the content that was written.
    pub sha256: String,
    /// Owning user.
    pub owner: String,
    /// File mode (octal string).
    pub mode: String,
}

/// Recorded state of an installed infection.
///
/// Only `name` and `version` are required; every other field defaults so
/// that state files written by older versions (or hand-written) still load.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InfectionState {
    /// Infection name.
    pub name: String,
    /// Infection version.
    pub version: String,
    /// Human description.
    #[serde(default)]
    pub description: String,
    /// Resolved variable values (may contain secrets — keep the record
    /// 0600 root-only).
    #[serde(default)]
    pub variables: BTreeMap<String, String>,
    /// Groups the infection created (left in place on uninstall — they
    /// may be shared).
    #[serde(default)]
    pub groups: Vec<String>,
    /// Users the infection created (left in place on uninstall).
    #[serde(default)]
    pub users: Vec<String>,
    /// Files the infection wrote, with content hashes.
    #[serde(default)]
    pub files: Vec<InfectionRecordedFile>,
    /// The unit this infection owns (canonical name), if any.
    #[serde(default)]
    pub unit: Option<String>,
    /// The existing unit this infection attached, if any.
    #[serde(default)]
    pub attach: Option<String>,
    /// Health check command (rendered).
    #[serde(default)]
    pub health_check: Vec<String>,
    /// Health check interval, seconds.
    #[serde(default = "default_state_interval")]
    pub health_interval: u64,
    /// Install time (RFC 3339); stamped by the agent at record time.
    #[serde(default)]
    pub installed_at: Option<String>,
    /// Owning deployment (None = installed standalone).
    #[serde(default)]
    pub owner: Option<String>,
}

fn default_state_interval() -> u64 {
    30
}

/// One infection a deployment installed, in install order.
///
/// The infection's own recorded state (files, unit, variables) lives in
/// `/etc/pandemic/infections/<name>/state.toml`; the deployment record only
/// needs to know what it owns and in what order, so removal can run in
/// reverse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentRecordedInfection {
    /// Infection name (must match the infection's recorded state).
    pub name: String,
    /// Infection version as recorded at install time.
    pub version: String,
    /// Position in the deployment's `order` (lower = installed first).
    pub order: u64,
    /// Where the infection spec came from (path or registry name), for
    /// reporting.
    pub source: String,
}

/// Recorded state of an installed deployment (ideas/deployments.md, phase 4).
///
/// Stored at `/etc/pandemic/deployments/<name>/state.toml` (0600,
/// root-only). `variables` holds the *resolved* shared variables and may
/// contain secrets — the record stays 0600 root-only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentState {
    /// Deployment name.
    pub name: String,
    /// Deployment version.
    pub version: String,
    /// Resolved shared variables (may contain secrets).
    #[serde(default)]
    pub variables: BTreeMap<String, String>,
    /// Infections owned by this deployment, in install order.
    pub infections: Vec<DeploymentRecordedInfection>,
    /// Install time (RFC 3339); stamped by the agent at record time.
    #[serde(default)]
    pub installed_at: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> InfectionState {
        InfectionState {
            name: "rest".into(),
            version: "0.4.0".into(),
            description: "Pandemic REST API".into(),
            variables: {
                let mut m = BTreeMap::new();
                m.insert("port".to_string(), "8080".to_string());
                m
            },
            groups: vec!["rest".into()],
            users: vec!["rest".into()],
            files: vec![InfectionRecordedFile {
                target: "/etc/pandemic/rest-auth.toml".into(),
                sha256: "abc123".into(),
                owner: "root".into(),
                mode: "0600".into(),
            }],
            unit: Some("rest".into()),
            attach: None,
            health_check: vec![
                "curl".into(),
                "-sf".into(),
                "http://127.0.0.1:8080/health".into(),
            ],
            health_interval: 15,
            installed_at: Some("2026-09-06T00:00:00Z".into()),
            owner: None,
        }
    }

    #[test]
    fn state_toml_roundtrip() {
        let state = sample();
        let toml = toml::to_string(&state).unwrap();
        let back: InfectionState = toml::from_str(&toml).unwrap();
        assert_eq!(back.name, "rest");
        assert_eq!(back.version, "0.4.0");
        assert_eq!(back.variables["port"], "8080");
        assert_eq!(back.files[0].sha256, "abc123");
        assert_eq!(back.unit.as_deref(), Some("rest"));
        assert_eq!(back.health_interval, 15);
        assert_eq!(back.installed_at.as_deref(), Some("2026-09-06T00:00:00Z"));
    }

    #[test]
    fn state_json_roundtrip() {
        let state = sample();
        let json = serde_json::to_string(&state).unwrap();
        let back: InfectionState = serde_json::from_str(&json).unwrap();
        assert_eq!(back, state);
    }

    #[test]
    fn state_tolerates_missing_new_fields() {
        // A state file written before `owner` existed must still load.
        let toml = r#"
name = "old"
version = "1.0.0"
description = ""
variables = {}
files = []
health_check = []
health_interval = 30
"#;
        let state: InfectionState = toml::from_str(toml).unwrap();
        assert_eq!(state.name, "old");
        assert!(state.owner.is_none());
        assert!(state.groups.is_empty());
    }

    fn deployment_sample() -> DeploymentState {
        DeploymentState {
            name: "rest-stack".into(),
            version: "1.2.0".into(),
            variables: {
                let mut m = BTreeMap::new();
                m.insert("port".to_string(), "8080".to_string());
                m.insert("api_key".to_string(), "s3cr3t".to_string());
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
            installed_at: Some("2026-09-06T00:00:00Z".into()),
        }
    }

    #[test]
    fn deployment_state_toml_roundtrip() {
        let state = deployment_sample();
        let toml = toml::to_string(&state).unwrap();
        let back: DeploymentState = toml::from_str(&toml).unwrap();
        assert_eq!(back, state);
        assert_eq!(back.name, "rest-stack");
        assert_eq!(back.infections.len(), 2);
        assert_eq!(back.infections[1].name, "rest-metrics");
        assert_eq!(back.variables["api_key"], "s3cr3t");
    }

    #[test]
    fn deployment_state_json_roundtrip() {
        let state = deployment_sample();
        let json = serde_json::to_string(&state).unwrap();
        let back: DeploymentState = serde_json::from_str(&json).unwrap();
        assert_eq!(back, state);
    }

    #[test]
    fn deployment_state_tolerates_missing_optional_fields() {
        // A record written before `variables`/`installed_at` existed loads.
        let toml = r#"
name = "old"
version = "1.0.0"
infections = [
    { name = "rest", version = "0.4.0", order = 1, source = "rest" }
]
"#;
        let state: DeploymentState = toml::from_str(toml).unwrap();
        assert_eq!(state.name, "old");
        assert!(state.variables.is_empty());
        assert!(state.installed_at.is_none());
        assert_eq!(state.infections[0].source, "rest");
    }
}
