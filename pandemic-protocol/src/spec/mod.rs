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
    use super::{is_variable_name, validate_infection_name};

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
