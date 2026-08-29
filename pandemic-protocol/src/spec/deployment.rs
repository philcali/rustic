//! Deployment spec: types, TOML parsing, and validation.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{bail, Result};
use serde::Deserialize;

use super::is_variable_name;
use crate::spec::validate_infection_name;

/// Top-level deployment spec (`pandemic-full.toml`).
///
/// A composition of infection specs: shared variables, explicit ordering,
/// and per-infection variable bindings (the cross-infection wiring).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentSpec {
    #[serde(rename = "deployment")]
    pub meta: DeploymentMeta,
    /// Shared variables, available to every infection in the deployment.
    ///
    /// Values may reference other shared variables via `{{name}}` and are
    /// resolved iteratively (see [`super::resolve_deployment_variables`]).
    /// Overridden by CLI `--set`.
    #[serde(default)]
    pub variables: BTreeMap<String, String>,
    /// The infections this deployment installs, with ordering and wiring.
    pub infections: Vec<DeploymentInfection>,
}

/// Identity block: `[deployment]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentMeta {
    pub name: String,
    pub version: String,
}

/// One entry of `[[infections]]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentInfection {
    /// The infection's name within this deployment.
    pub name: String,
    /// A local spec path or a registry infection name.
    pub source: String,
    /// Explicit install order; lower installs first. Ties are an error.
    pub order: u64,
    /// Variable bindings for this infection. Values may reference shared
    /// variables: `vars = { api_url = "http://{{host}}:{{rest_port}}" }`.
    #[serde(default)]
    pub vars: BTreeMap<String, String>,
}

/// Parse and validate a deployment spec from TOML.
pub fn parse_deployment_spec(toml: &str) -> Result<DeploymentSpec> {
    let spec: DeploymentSpec =
        toml::from_str(toml).map_err(|e| anyhow::anyhow!("invalid deployment spec: {e}"))?;
    validate_deployment_spec(&spec)?;
    Ok(spec)
}

fn validate_deployment_spec(spec: &DeploymentSpec) -> Result<()> {
    validate_infection_name(&spec.meta.name)?;
    if spec.infections.is_empty() {
        bail!("deployment '{}' has no infections", spec.meta.name);
    }

    for name in spec.variables.keys() {
        if !is_variable_name(name) {
            bail!("invalid variable name '{name}': use letters, digits or underscores");
        }
    }

    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut orders: BTreeSet<u64> = BTreeSet::new();
    for infection in &spec.infections {
        validate_infection_name(&infection.name)?;
        if !seen.insert(infection.name.clone()) {
            bail!(
                "duplicate infection name '{}' in deployment",
                infection.name
            );
        }
        if !orders.insert(infection.order) {
            bail!(
                "duplicate order {} in deployment: every infection needs a distinct order",
                infection.order
            );
        }
        if infection.source.is_empty() {
            bail!("infection '{}' has an empty source", infection.name);
        }
        for name in infection.vars.keys() {
            if !is_variable_name(name) {
                bail!(
                    "infection '{}' has invalid variable name '{name}': \
                     use letters, digits or underscores",
                    infection.name
                );
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The canonical `pandemic-full` deployment from `ideas/deployments.md`.
    const FULL: &str = r#"
[deployment]
name = "pandemic-full"
version = "0.4.0"

[variables]
host = "127.0.0.1"
socket_path = "/var/run/pandemic/pandemic.sock"
rest_port = "8080"
console_port = "3000"
mqtt_port = "1883"
topic_prefix = "pandemic"

[[infections]]
name = "mosquitto"
source = "./infections/mosquitto.toml"
order = 1
vars = { port = "{{mqtt_port}}" }

[[infections]]
name = "rest"
source = "./infections/rest.toml"
order = 2
vars = { socket_path = "{{socket_path}}", port = "{{rest_port}}" }

[[infections]]
name = "console"
source = "./infections/console.toml"
order = 3
vars = { socket_path = "{{socket_path}}", port = "{{console_port}}", api_url = "http://{{host}}:{{rest_port}}" }

[[infections]]
name = "mqtt"
source = "./infections/mqtt.toml"
order = 4
vars = { socket_path = "{{socket_path}}", broker_url = "mqtt://{{host}}:{{mqtt_port}}", topic_prefix = "{{topic_prefix}}" }
"#;

    #[test]
    fn parses_pandemic_full() {
        let spec = parse_deployment_spec(FULL).unwrap();
        assert_eq!(spec.meta.name, "pandemic-full");
        assert_eq!(spec.variables.len(), 6);
        assert_eq!(spec.variables["host"], "127.0.0.1");
        assert_eq!(spec.infections.len(), 4);
        let mosquitto = &spec.infections[0];
        assert_eq!(mosquitto.name, "mosquitto");
        assert_eq!(mosquitto.source, "./infections/mosquitto.toml");
        assert_eq!(mosquitto.order, 1);
        assert_eq!(mosquitto.vars["port"], "{{mqtt_port}}");
        let console = &spec.infections[2];
        assert_eq!(console.vars["api_url"], "http://{{host}}:{{rest_port}}");
    }

    fn base() -> String {
        r#"
[deployment]
name = "app"
version = "0.1.0"

[[infections]]
name = "rest"
source = "./rest.toml"
order = 1
"#
        .to_string()
    }

    #[test]
    fn rejects_duplicate_infection_names() {
        let toml = base().replace(
            "order = 1",
            "order = 1\n\n[[infections]]\nname = \"rest\"\nsource = \"./other.toml\"\norder = 2",
        );
        assert!(parse_deployment_spec(&toml).is_err());
    }

    #[test]
    fn rejects_order_ties() {
        let toml = base().replace(
            "order = 1",
            "order = 1\n\n[[infections]]\nname = \"console\"\nsource = \"./console.toml\"\norder = 1",
        );
        assert!(parse_deployment_spec(&toml).is_err());
    }

    #[test]
    fn rejects_empty_infection_list() {
        let toml = r#"
[deployment]
name = "app"
version = "0.1.0"
"#;
        assert!(parse_deployment_spec(toml).is_err());
    }

    #[test]
    fn rejects_invalid_deployment_name() {
        let toml = base().replace("name = \"app\"", "name = \"App\"");
        assert!(parse_deployment_spec(&toml).is_err());
    }

    #[test]
    fn rejects_unknown_field() {
        let toml = base().replace(
            "[deployment]",
            "[dependencies]\nrest = \"console\"\n\n[deployment]",
        );
        assert!(parse_deployment_spec(&toml).is_err());
    }
}
