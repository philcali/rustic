//! Spec-driven install: infection and deployment specs.
//!
//! See `ideas/deployments.md` for the design. Two layers:
//!
//! - [`InfectionSpec`] — a declarative description of one piece of software
//!   installed and configured on a host: packages, rendered files,
//!   users/groups, a systemd unit (or an attach of an existing unit), and a
//!   health check. Templated with variables. The atomic,
//!   registry-publishable unit.
//! - [`DeploymentSpec`] — a composition of infection specs with shared
//!   variables, explicit ordering, and cross-infection wiring.
//!
//! This module is pure logic — parsing, validation, variable resolution, and
//! `{{name}}` rendering. It never touches the network or the filesystem:
//! template contents are supplied by the caller (see [`resolve_infection`]).
//!
//! ## Variable resolution
//!
//! An infection variable's value comes from the first hit in:
//!
//! 1. standalone `--set` values (infection namespace),
//! 2. the deployment entry's `vars` bindings, rendered against the shared
//!    variables — this is where cross-infection wiring lives,
//! 3. a same-name shared (deployment) variable,
//! 4. the infection's declared `default`.
//!
//! A variable that is declared without a default, or referenced by a template
//! or the health check, and resolves nowhere is an error — before anything
//! is applied.
//!
//! Shared deployment variables are `[variables]` overridden by `--set`; a
//! shared value may reference other shared variables (resolved iteratively,
//! circular references are an error).

mod deployment;
mod infection;
mod render;
mod state;

pub use deployment::{parse_deployment_spec, DeploymentInfection, DeploymentMeta, DeploymentSpec};
pub use infection::{
    parse_infection_spec, FilePlacement, GroupSpec, HealthSpec, InfectionMeta, InfectionSpec,
    InfectionVariable, SystemdSpec, UserSpec,
};
pub use render::{
    canonical_unit_name, check_infection_collisions, render_template, resolve_deployment_variables,
    resolve_infection, validate_deployment, RenderedFile, RenderedInfection, RenderedUnit,
};
pub use state::{
    DeploymentRecordedInfection, DeploymentState, InfectionRecordedFile, InfectionState,
};

use anyhow::{bail, Result};

/// Package managers that may appear as `[packages]` keys.
///
/// A spec lists packages per manager; at apply time the agent picks the key
/// its host supports and errors if none match.
pub const KNOWN_PACKAGE_MANAGERS: &[&str] = &["apt", "dnf", "pacman", "apk", "zypper"];

/// Pick the package manager + packages for a host from a spec's declared
/// `[packages]` and the host's supported managers.
///
/// Pure — the caller supplies `supported` (the agent uses its own detected
/// managers; a client uses a `GetCapabilities` fetch for dry-run reporting).
/// Errors if the spec declares packages but the host supports none of them.
pub fn select_packages(
    declared: &std::collections::BTreeMap<String, Vec<String>>,
    supported: &[String],
) -> Result<Option<(String, Vec<String>)>> {
    if declared.is_empty() {
        return Ok(None);
    }
    let chosen = KNOWN_PACKAGE_MANAGERS.iter().copied().find(|manager| {
        declared
            .get(*manager)
            .map(|list| !list.is_empty() && supported.iter().any(|s| s == *manager))
            .unwrap_or(false)
    });
    match chosen {
        Some(manager) => Ok(Some((manager.to_string(), declared[manager].clone()))),
        None => Err(anyhow::anyhow!(
            "spec lists packages for [{}] but this host supports [{}]",
            declared.keys().cloned().collect::<Vec<_>>().join(", "),
            if supported.is_empty() {
                "no known manager (apt, dnf, pacman, apk, zypper)".to_string()
            } else {
                supported.join(", ")
            }
        )),
    }
}

/// Infection and deployment names share one charset rule: 1-63 lowercase
/// ASCII letters, digits, or hyphens, with no leading or trailing hyphen.
pub fn validate_infection_name(name: &str) -> Result<()> {
    let valid = !name.is_empty()
        && name.len() <= 63
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if valid {
        Ok(())
    } else {
        bail!(
            "invalid infection name '{name}': use 1-63 lowercase letters, digits or hyphens (no leading/trailing hyphen)"
        )
    }
}

/// Variable names are non-empty ASCII alphanumerics and underscores.
pub fn is_variable_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::{
        is_variable_name, parse_deployment_spec, parse_infection_spec, validate_infection_name,
    };

    /// The shipped registry source tree (`registry-src/`) must contain valid
    /// specs — a guard against publishing a broken atom. The deployment's
    /// `source` entries are bare registry names (no local path), per the
    /// phase-7 resolution model (deployment → infection-spec only).
    #[test]
    fn shipped_registry_specs_are_valid() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("registry-src");
        let read = |p: &std::path::Path| {
            std::fs::read_to_string(p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
        };

        let mosq = parse_infection_spec(&read(&root.join("infections/mosquitto/infection.toml")))
            .expect("mosquitto infection-spec parses");
        assert_eq!(mosq.meta.name, "mosquitto");
        assert!(
            mosq.systemd.as_ref().unwrap().attach.is_some(),
            "mosquitto is an attach"
        );

        let rest = parse_infection_spec(&read(&root.join("infections/rest/infection.toml")))
            .expect("rest infection-spec parses");
        assert_eq!(rest.meta.name, "rest");
        assert!(
            rest.systemd.as_ref().unwrap().unit_file.is_some(),
            "rest has a unit"
        );

        let dep = parse_deployment_spec(&read(&root.join("deployments/rest-mqtt/deployment.toml")))
            .expect("rest-mqtt deployment parses");
        assert_eq!(dep.meta.name, "rest-mqtt");
        assert_eq!(dep.infections.len(), 2);
        for inf in &dep.infections {
            assert!(
                !inf.source.contains('/'),
                "registry deployment sources must be bare names, got '{}'",
                inf.source
            );
        }
    }

    #[test]
    fn name_charset() {
        assert!(validate_infection_name("mosquitto").is_ok());
        assert!(validate_infection_name("a-b-2").is_ok());
        assert!(validate_infection_name("").is_err());
        assert!(validate_infection_name("Has-Upper").is_err());
        assert!(validate_infection_name("-lead").is_err());
        assert!(validate_infection_name("trail-").is_err());
        assert!(validate_infection_name("with space").is_err());
        assert!(validate_infection_name(&"a".repeat(64)).is_err());
    }

    #[test]
    fn variable_name_charset() {
        assert!(is_variable_name("socket_path"));
        assert!(is_variable_name("a1"));
        assert!(!is_variable_name(""));
        assert!(!is_variable_name("a b"));
        assert!(!is_variable_name("a-b"));
        assert!(!is_variable_name("-a"));
    }
}
