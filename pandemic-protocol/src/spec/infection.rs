//! Infection spec: types, TOML parsing, and validation.

use std::collections::BTreeMap;

use anyhow::{bail, Result};
use serde::Deserialize;

use super::{is_variable_name, validate_infection_name, KNOWN_PACKAGE_MANAGERS};

/// Top-level infection spec (one infection's `spec.toml`).
///
/// A strict superset of today's attach/service fields: identity,
/// variables, packages, rendered files, users/groups, a unit (or an
/// attach of an existing unit), and a health check.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InfectionSpec {
    #[serde(rename = "infection")]
    pub meta: InfectionMeta,
    /// Variables the infection is templated with.
    ///
    /// A variable without a `default` is *required*: it must be supplied
    /// by `--set` or a deployment binding, or resolution fails.
    #[serde(default)]
    pub variables: BTreeMap<String, InfectionVariable>,
    /// Packages to install, keyed by package-manager name.
    ///
    /// The agent picks the key its host supports and errors if none match.
    #[serde(default)]
    pub packages: BTreeMap<String, Vec<String>>,
    /// Files to render onto the host, keyed by template path (relative to
    /// the spec or in `files/`).
    #[serde(default)]
    pub files: BTreeMap<String, FilePlacement>,
    /// Groups to create, keyed by group name.
    #[serde(default)]
    pub groups: BTreeMap<String, GroupSpec>,
    /// Users to create, keyed by user name.
    #[serde(default)]
    pub users: BTreeMap<String, UserSpec>,
    /// The service this infection installs: a new unit or an attach.
    ///
    /// `None` for a config-only infection — files/attach only, no unit of
    /// its own is the normal shape, not a special case.
    pub systemd: Option<SystemdSpec>,
    /// Health check for the installed service.
    pub health: HealthSpec,
}

/// Identity block: `[infection]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InfectionMeta {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
}

/// A declared variable: `[variables] name = { default = "..." }`.
///
/// `default` is optional — omit it to make the variable required.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InfectionVariable {
    #[serde(default)]
    pub default: Option<String>,
}

/// Where a template file lands on the host:
/// `[files] "template" = { target = "...", owner = "...", mode = "0600" }`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilePlacement {
    /// Absolute host path to write the rendered file to.
    pub target: String,
    /// Owning user; `None` renders as `root`.
    #[serde(default)]
    pub owner: Option<String>,
    /// File mode, octal string (e.g. `"0600"`); `None` renders as `"0644"`.
    #[serde(default)]
    pub mode: Option<String>,
}

/// A group to create; an empty table `{}` is valid.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupSpec {}

/// A user to create: `[users] name = { group = "...", system = true }`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserSpec {
    /// Primary group; `None` means a group of the same name.
    #[serde(default)]
    pub group: Option<String>,
    /// Create as a system user.
    #[serde(default)]
    pub system: bool,
}

/// The service this infection installs: `[systemd]`.
///
/// Exactly one of `unit_file` / `attach`; a spec with no `[systemd]` at
/// all is a config-only infection.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemdSpec {
    /// A new unit file, rendered to `/etc/systemd/system/`.
    #[serde(default)]
    pub unit_file: Option<String>,
    /// An existing unit to wrap (existing attach flow).
    #[serde(default)]
    pub attach: Option<String>,
    /// Enable the unit on boot.
    #[serde(default)]
    pub enable: bool,
}

/// Health check: `[health]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthSpec {
    /// Command to run; success is exit 0. Must be non-empty.
    pub check: Vec<String>,
    /// Seconds between checks.
    #[serde(default = "default_interval")]
    pub interval: u64,
}

fn default_interval() -> u64 {
    30
}

/// Parse and validate an infection spec from TOML.
///
/// Fails on any schema, charset, or shape violation — before anything is
/// applied.
pub fn parse_infection_spec(toml: &str) -> Result<InfectionSpec> {
    let spec: InfectionSpec =
        toml::from_str(toml).map_err(|e| anyhow::anyhow!("invalid infection spec: {e}"))?;
    validate_infection(&spec)?;
    Ok(spec)
}

fn validate_infection(spec: &InfectionSpec) -> Result<()> {
    validate_infection_name(&spec.meta.name)?;

    for name in spec.variables.keys() {
        if !is_variable_name(name) {
            bail!("invalid variable name '{name}': use letters, digits or underscores");
        }
    }

    for (manager, packages) in &spec.packages {
        if !KNOWN_PACKAGE_MANAGERS.contains(&manager.as_str()) {
            bail!(
                "unknown package manager '{manager}' in [packages]: expected one of {}",
                KNOWN_PACKAGE_MANAGERS.join(", ")
            );
        }
        if packages.iter().any(|p| p.is_empty()) {
            bail!("empty package name in [packages] '{manager}'");
        }
    }

    for (template, placement) in &spec.files {
        if !placement.target.starts_with('/') {
            bail!(
                "file '{template}' target must be an absolute path, got '{}'",
                placement.target
            );
        }
    }

    if spec.groups.keys().any(|k| k.is_empty()) {
        bail!("[groups] key must not be empty");
    }
    if spec.users.keys().any(|k| k.is_empty()) {
        bail!("[users] key must not be empty");
    }

    if let Some(systemd) = &spec.systemd {
        let (unit_file, attach) = (&systemd.unit_file, &systemd.attach);
        match (unit_file.is_some(), attach.is_some()) {
            (true, true) => {
                bail!("[systemd] requires exactly one of unit_file or attach, not both")
            }
            (false, false) => bail!(
                "[systemd] requires one of unit_file or attach \
                 (omit [systemd] entirely for a config-only infection)"
            ),
            _ => {
                let unit = unit_file.as_deref().or(attach.as_deref()).unwrap();
                if !unit.ends_with(".service") {
                    bail!("[systemd] unit '{unit}' must be a .service unit in v1");
                }
            }
        }
    }

    if spec.health.check.is_empty() {
        bail!("[health].check must not be empty");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The canonical REST fixture from `ideas/deployments.md`.
    const REST: &str = r#"
[infection]
name = "rest"
version = "0.4.0"
description = "Pandemic REST API"

[variables]
socket_path  = { default = "/var/run/pandemic/pandemic.sock" }
port         = { default = "8080" }
bind_address = { default = "127.0.0.1" }

[packages]
apt = []
dnf = []
pacman = []

[groups]
rest = {}

[users]
rest = { group = "rest", system = true }

[files]
"rest-auth.toml" = { target = "/etc/pandemic/rest-auth.toml", owner = "root", mode = "0600" }

[systemd]
unit_file = "rest.service"
enable = true

[health]
check = ["curl", "-sf", "http://127.0.0.1:{{port}}/health"]
interval = 15
"#;

    /// The canonical mosquitto attach fixture from `ideas/deployments.md`.
    const MOSQUITTO: &str = r#"
[infection]
name = "mosquitto"
version = "2.0"
description = "MQTT broker (attached)"

[variables]
port = { default = "1883" }

[files]
"mosquitto.conf" = { target = "/etc/mosquitto/mosquitto.conf", owner = "mosquitto", mode = "0640" }

[systemd]
attach = "mosquitto.service"

[health]
check = ["systemctl", "is-active", "mosquitto"]
interval = 15
"#;

    #[test]
    fn parses_rest_spec() {
        let spec = parse_infection_spec(REST).unwrap();
        assert_eq!(spec.meta.name, "rest");
        assert_eq!(spec.meta.version, "0.4.0");
        assert_eq!(spec.variables.len(), 3);
        assert_eq!(spec.variables["port"].default.as_deref(), Some("8080"));
        assert_eq!(
            spec.variables["socket_path"].default.as_deref(),
            Some("/var/run/pandemic/pandemic.sock")
        );
        assert_eq!(spec.packages.len(), 3);
        assert!(spec.packages.contains_key("apt"));
        assert!(spec.groups.contains_key("rest"));
        assert!(spec.users["rest"].system);
        assert_eq!(spec.users["rest"].group.as_deref(), Some("rest"));
        let placement = &spec.files["rest-auth.toml"];
        assert_eq!(placement.target, "/etc/pandemic/rest-auth.toml");
        assert_eq!(placement.owner.as_deref(), Some("root"));
        assert_eq!(placement.mode.as_deref(), Some("0600"));
        let systemd = spec.systemd.as_ref().unwrap();
        assert_eq!(systemd.unit_file.as_deref(), Some("rest.service"));
        assert!(systemd.attach.is_none());
        assert!(systemd.enable);
        assert_eq!(spec.health.interval, 15);
        assert_eq!(
            spec.health.check,
            vec!["curl", "-sf", "http://127.0.0.1:{{port}}/health"]
        );
    }

    #[test]
    fn parses_mosquitto_attach_spec() {
        let spec = parse_infection_spec(MOSQUITTO).unwrap();
        assert_eq!(spec.meta.name, "mosquitto");
        let systemd = spec.systemd.as_ref().unwrap();
        assert_eq!(systemd.attach.as_deref(), Some("mosquitto.service"));
        assert!(systemd.unit_file.is_none());
        assert!(!systemd.enable);
        assert!(spec.packages.is_empty());
        assert!(spec.groups.is_empty());
        assert!(spec.users.is_empty());
        assert_eq!(spec.files.len(), 1);
    }

    #[test]
    fn config_only_spec_has_no_systemd() {
        let toml = r#"
[infection]
name = "tls"
version = "0.1.0"

[files]
"certs.conf" = { target = "/etc/pandemic/certs.conf", owner = "root", mode = "0644" }

[health]
check = ["true"]
"#;
        let spec = parse_infection_spec(toml).unwrap();
        assert!(spec.systemd.is_none());
        assert_eq!(spec.health.interval, 30); // default
    }

    fn base() -> String {
        r#"
[infection]
name = "rest"
version = "0.1.0"

[systemd]
unit_file = "rest.service"

[health]
check = ["true"]
"#
        .to_string()
    }

    #[test]
    fn rejects_both_unit_and_attach() {
        let toml = base().replace(
            "unit_file = \"rest.service\"",
            "unit_file = \"rest.service\"\nattach = \"mosquitto.service\"",
        );
        assert!(parse_infection_spec(&toml).is_err());
    }

    #[test]
    fn rejects_empty_systemd_table() {
        let toml = base().replace("unit_file = \"rest.service\"\n", "");
        assert!(parse_infection_spec(&toml).is_err());
    }

    #[test]
    fn rejects_non_service_unit() {
        let toml = base().replace("rest.service", "rest.target");
        assert!(parse_infection_spec(&toml).is_err());
    }

    #[test]
    fn rejects_unknown_package_manager() {
        let toml = base().replace("[health]", "[packages]\nyum = [\"rest\"]\n\n[health]");
        assert!(parse_infection_spec(&toml).is_err());
    }

    #[test]
    fn rejects_invalid_name() {
        let toml = base().replace("name = \"rest\"", "name = \"Rest!\"");
        assert!(parse_infection_spec(&toml).is_err());
    }

    #[test]
    fn rejects_relative_file_target() {
        let toml = base().replace(
            "[health]",
            "[files]\n\"a.conf\" = { target = \"etc/a.conf\" }\n\n[health]",
        );
        assert!(parse_infection_spec(&toml).is_err());
    }

    #[test]
    fn rejects_empty_health_check() {
        let toml = base().replace("check = [\"true\"]", "check = []");
        assert!(parse_infection_spec(&toml).is_err());
    }

    #[test]
    fn rejects_unknown_top_level_field() {
        let toml = base().replace("[health]", "[commands]\nrun = [\"echo\"]\n\n[health]");
        assert!(parse_infection_spec(&toml).is_err());
    }

    #[test]
    fn required_variable_has_no_default() {
        let toml = r#"
[infection]
name = "rest"
version = "0.1.0"

[variables]
token = {}

[health]
check = ["true"]
"#;
        let spec = parse_infection_spec(toml).unwrap();
        assert!(spec.variables["token"].default.is_none());
    }
}
