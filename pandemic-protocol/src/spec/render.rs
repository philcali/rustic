//! Variable resolution and `{{name}}` rendering.
//!
//! Pure logic: given parsed specs and raw values, produce the resolved
//! variable maps and rendered strings that a later phase applies.
//! Template *contents* are supplied by the caller — this module never
//! reads files.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};

use super::deployment::DeploymentSpec;
use super::infection::InfectionSpec;
use super::is_variable_name;

/// Render one `{{name}}` occurrence, given a resolver for names.
fn render_with<F>(template: &str, mut resolve: F) -> Result<String>
where
    F: FnMut(&str) -> Result<String>,
{
    let mut out = String::new();
    let mut rest = template;
    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let inner = &rest[open + 2..];
        let close = inner
            .find("}}")
            .ok_or_else(|| anyhow!("unterminated template variable: '{{' without '}}'"))?;
        let name = inner[..close].trim();
        if !is_variable_name(name) {
            bail!("invalid template variable '{{{name}}}'");
        }
        out.push_str(&resolve(name)?);
        rest = &inner[close + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Render a `{{name}}` template against a variable map.
///
/// Single pass: substituted values are not re-scanned, so a value that
/// itself contains `{{...}}` lands literally. An unknown variable is an
/// error — a missing required variable fails before anything is applied.
pub fn render_template(template: &str, vars: &BTreeMap<String, String>) -> Result<String> {
    render_with(template, |name| {
        vars.get(name)
            .cloned()
            .ok_or_else(|| anyhow!("missing required variable '{name}'"))
    })
}

/// Resolve the deployment's shared variables.
///
/// `set` (CLI `--set`) overrides `[variables]`; a value may reference
/// other shared variables via `{{name}}` and is resolved transitively.
/// Circular references and references to unknown variables are errors.
pub fn resolve_deployment_variables(
    variables: &BTreeMap<String, String>,
    set: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>> {
    let merged: BTreeMap<String, String> = variables
        .iter()
        .chain(set.iter())
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let mut resolved: BTreeMap<String, String> = BTreeMap::new();
    let mut visiting: BTreeSet<String> = BTreeSet::new();
    for name in merged.keys() {
        resolve_shared(name, &merged, &mut resolved, &mut visiting)?;
    }
    Ok(resolved)
}

fn resolve_shared(
    name: &str,
    merged: &BTreeMap<String, String>,
    resolved: &mut BTreeMap<String, String>,
    visiting: &mut BTreeSet<String>,
) -> Result<String> {
    if let Some(value) = resolved.get(name) {
        return Ok(value.clone());
    }
    if !visiting.insert(name.to_string()) {
        bail!("circular reference in deployment variables involving '{name}'");
    }
    let raw = merged
        .get(name)
        .ok_or_else(|| anyhow!("missing required variable '{name}'"))?;
    let rendered = render_with(raw, |ref_name| {
        resolve_shared(ref_name, merged, resolved, visiting)
    })?;
    visiting.remove(name);
    resolved.insert(name.to_string(), rendered.clone());
    Ok(rendered)
}

/// Resolve an infection's declared variables, most specific source first:
///
/// 1. standalone `--set` values (infection namespace),
/// 2. the deployment entry's `vars` bindings, rendered against the shared
///    variables — this is where cross-infection wiring lives,
/// 3. a same-name shared (deployment) variable,
/// 4. the infection's declared `default`.
///
/// `shared` must already be resolved (see
/// [`resolve_deployment_variables`]). A variable that resolves nowhere is
/// an error — before anything is applied.
pub fn resolve_infection(
    spec: &InfectionSpec,
    bindings: &BTreeMap<String, String>,
    shared: &BTreeMap<String, String>,
    set: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for (name, variable) in &spec.variables {
        let value = if let Some(value) = set.get(name) {
            value.clone()
        } else if let Some(value) = bindings.get(name) {
            render_template(value, shared)?
        } else if let Some(value) = shared.get(name) {
            value.clone()
        } else if let Some(default) = &variable.default {
            default.clone()
        } else {
            bail!(
                "infection '{}' is missing required variable '{name}': \
                 no --set value, deployment binding, shared variable, or default",
                spec.meta.name
            );
        };
        out.insert(name.clone(), value);
    }
    Ok(out)
}

/// Canonical unit name for a `.service` unit: the component before the
/// first dot (`rest.service` → `rest`).
pub fn canonical_unit_name(unit: &str) -> String {
    unit.split('.').next().unwrap_or(unit).to_string()
}

/// A rendered file, ready to apply.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderedFile {
    /// The template path as listed in the spec.
    pub source: String,
    /// The rendered content.
    pub content: String,
    /// The absolute host path to write to.
    pub target: String,
    /// The owning user (specs without one render as `root`).
    pub owner: String,
    /// The file mode, e.g. `"0600"` (specs without one render as `"0644"`).
    pub mode: String,
}

/// A unit an infection owns (from `systemd.unit_file`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderedUnit {
    /// The canonical unit name (`rest`).
    pub name: String,
    /// The unit file name (`rest.service`), rendered to
    /// `/etc/systemd/system/`.
    pub file: String,
    /// Enable on boot.
    pub enable: bool,
}

/// An infection after variable resolution and template rendering — the
/// unit a later phase applies, in order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderedInfection {
    /// The infection name (deployment-scoped when deployed).
    pub name: String,
    /// The resolved variable values.
    pub variables: BTreeMap<String, String>,
    /// The rendered files to write.
    pub files: Vec<RenderedFile>,
    /// The unit this infection owns, if any.
    pub unit: Option<RenderedUnit>,
    /// The existing unit this infection attaches to, if any.
    pub attach: Option<String>,
    /// The rendered health check command.
    pub health_check: Vec<String>,
    /// The health check interval, seconds.
    pub health_interval: u64,
}

/// Check infection specs for cross-infection collisions: two infections
/// writing the same file target, or two infections owning the same unit.
///
/// Attaching the same existing unit from several infections is allowed —
/// attach is additive.
pub fn check_infection_collisions(infections: &[InfectionSpec]) -> Result<()> {
    let mut file_owners: BTreeMap<&str, &str> = BTreeMap::new();
    let mut unit_owners: BTreeMap<String, String> = BTreeMap::new();
    for infection in infections {
        let name: &str = infection.meta.name.as_str();
        for placement in infection.files.values() {
            if let Some(owner) = file_owners.insert(placement.target.as_str(), name) {
                bail!(
                    "file target '{}' claimed by both '{}' and '{}'",
                    placement.target,
                    owner,
                    name
                );
            }
        }
        if let Some(systemd) = &infection.systemd {
            if let Some(unit_file) = &systemd.unit_file {
                let unit = canonical_unit_name(unit_file);
                if let Some(owner) = unit_owners.insert(unit.clone(), name.to_string()) {
                    bail!("unit '{unit}' owned by both '{owner}' and '{name}'");
                }
            }
        }
    }
    Ok(())
}

/// Validate a deployment against the infection specs it composes:
/// duplicate infection names, order ties, and cross-infection collisions.
///
/// Field-level rules (name charset, unit/attach exclusivity) are enforced
/// at parse time by the individual specs.
pub fn validate_deployment(spec: &DeploymentSpec, infections: &[InfectionSpec]) -> Result<()> {
    let mut names: BTreeSet<String> = BTreeSet::new();
    let mut orders: BTreeSet<u64> = BTreeSet::new();
    for infection in &spec.infections {
        if !names.insert(infection.name.clone()) {
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
    }
    check_infection_collisions(infections)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn vars(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn renders_simple_substitution() {
        let out = render_template(
            "http://{{host}}:{{port}}/health",
            &vars(&[("host", "127.0.0.1"), ("port", "8080")]),
        )
        .unwrap();
        assert_eq!(out, "http://127.0.0.1:8080/health");
    }

    #[test]
    fn renders_no_variables() {
        assert_eq!(
            render_template("plain text", &vars(&[])).unwrap(),
            "plain text"
        );
    }

    #[test]
    fn values_are_not_rescanned() {
        // a="{{b}}" and b="x": single pass, so the output is the literal
        // value of a, which happens to contain {{b}}.
        let out = render_template("[{{a}}]", &vars(&[("a", "{{b}}"), ("b", "x")])).unwrap();
        assert_eq!(out, "[{{b}}]");
    }

    #[test]
    fn missing_variable_is_an_error() {
        let err = render_template("{{nope}}", &vars(&[])).unwrap_err();
        assert!(err.to_string().contains("missing required variable 'nope'"));
    }

    #[test]
    fn unterminated_variable_is_an_error() {
        assert!(render_template("{{nope", &vars(&[])).is_err());
    }

    #[test]
    fn invalid_variable_name_is_an_error() {
        assert!(render_template("{{a b}}", &vars(&[("a b", "x")])).is_err());
    }

    #[test]
    fn resolves_shared_variable_references() {
        let shared = vars(&[
            ("base", "http://127.0.0.1"),
            ("api_url", "{{base}}:8080/api"),
        ]);
        let out = resolve_deployment_variables(&shared, &vars(&[])).unwrap();
        assert_eq!(out["api_url"], "http://127.0.0.1:8080/api");
        assert_eq!(out["base"], "http://127.0.0.1");
    }

    #[test]
    fn set_overrides_shared() {
        let shared = vars(&[("host", "127.0.0.1")]);
        let set = vars(&[("host", "10.0.1.5")]);
        let out = resolve_deployment_variables(&shared, &set).unwrap();
        assert_eq!(out["host"], "10.0.1.5");
    }

    #[test]
    fn circular_reference_is_an_error() {
        let shared = vars(&[("a", "{{b}}"), ("b", "{{a}}")]);
        let err = resolve_deployment_variables(&shared, &vars(&[])).unwrap_err();
        assert!(err.to_string().contains("circular"));
    }

    #[test]
    fn self_reference_is_an_error() {
        let shared = vars(&[("a", "{{a}}")]);
        let err = resolve_deployment_variables(&shared, &vars(&[])).unwrap_err();
        assert!(err.to_string().contains("circular"));
    }

    #[test]
    fn unknown_shared_reference_is_an_error() {
        let shared = vars(&[("a", "{{ghost}}")]);
        let err = resolve_deployment_variables(&shared, &vars(&[])).unwrap_err();
        assert!(err
            .to_string()
            .contains("missing required variable 'ghost'"));
    }

    fn infection(
        name: &str,
        variables: &[(&str, Option<&str>)],
        file_target: Option<&str>,
        unit_file: Option<&str>,
        attach: Option<&str>,
    ) -> InfectionSpec {
        let systemd = (unit_file.is_some() || attach.is_some()).then(|| {
            super::super::infection::SystemdSpec {
                unit_file: unit_file.map(String::from),
                attach: attach.map(String::from),
                enable: false,
            }
        });
        InfectionSpec {
            meta: super::super::infection::InfectionMeta {
                name: name.into(),
                version: "0.1.0".into(),
                description: String::new(),
            },
            variables: variables
                .iter()
                .map(|(n, d)| {
                    (
                        n.to_string(),
                        super::super::infection::InfectionVariable {
                            default: d.map(String::from),
                        },
                    )
                })
                .collect(),
            packages: BTreeMap::new(),
            files: file_target
                .map(|t| {
                    [(
                        "a.conf".to_string(),
                        super::super::infection::FilePlacement {
                            target: t.to_string(),
                            owner: None,
                            mode: None,
                        },
                    )]
                    .into_iter()
                    .collect()
                })
                .unwrap_or_default(),
            groups: BTreeMap::new(),
            users: BTreeMap::new(),
            systemd,
            health: super::super::infection::HealthSpec {
                check: vec!["true".into()],
                interval: 30,
            },
        }
    }

    #[test]
    fn resolve_infection_prefers_set_over_all() {
        let spec = infection("rest", &[("port", Some("8080"))], None, None, None);
        let shared = vars(&[("port", "9999")]);
        let bindings = vars(&[("port", "{{port}}")]);
        let set = vars(&[("port", "1234")]);
        let out = resolve_infection(&spec, &bindings, &shared, &set).unwrap();
        assert_eq!(out["port"], "1234");
    }

    #[test]
    fn resolve_infection_bindings_win_over_shared_and_render() {
        let spec = infection(
            "rest",
            &[("port", Some("8080")), ("api_url", None)],
            None,
            None,
            None,
        );
        let shared = vars(&[
            ("port", "9999"),
            ("rest_port", "8081"),
            ("host", "127.0.0.1"),
        ]);
        let bindings = vars(&[
            ("port", "{{rest_port}}"),
            ("api_url", "http://{{host}}:{{rest_port}}"),
        ]);
        let out = resolve_infection(&spec, &bindings, &shared, &vars(&[])).unwrap();
        assert_eq!(out["port"], "8081");
        assert_eq!(out["api_url"], "http://127.0.0.1:8081");
    }

    #[test]
    fn resolve_infection_falls_back_to_shared_then_default() {
        let spec = infection(
            "rest",
            &[("port", Some("8080")), ("bind", Some("127.0.0.1"))],
            None,
            None,
            None,
        );
        let shared = vars(&[("port", "9999")]);
        let out = resolve_infection(&spec, &vars(&[]), &shared, &vars(&[])).unwrap();
        assert_eq!(out["port"], "9999");
        assert_eq!(out["bind"], "127.0.0.1");
    }

    #[test]
    fn resolve_infection_missing_required_variable_errors() {
        let spec = infection("rest", &[("token", None)], None, None, None);
        let err = resolve_infection(&spec, &vars(&[]), &vars(&[]), &vars(&[])).unwrap_err();
        assert!(err
            .to_string()
            .contains("missing required variable 'token'"));
    }

    #[test]
    fn canonical_unit_names() {
        assert_eq!(canonical_unit_name("rest.service"), "rest");
        assert_eq!(canonical_unit_name("mosquitto.service"), "mosquitto");
    }

    #[test]
    fn rejects_duplicate_file_targets() {
        let a = infection("a", &[], Some("/etc/app/a.conf"), None, None);
        let b = infection("b", &[], Some("/etc/app/a.conf"), None, None);
        let err = check_infection_collisions(&[a, b]).unwrap_err();
        assert!(err.to_string().contains("/etc/app/a.conf"));
    }

    #[test]
    fn rejects_duplicate_owned_units() {
        let a = infection("a", &[], None, Some("app.service"), None);
        let b = infection("b", &[], None, Some("app.service"), None);
        let err = check_infection_collisions(&[a, b]).unwrap_err();
        assert!(err.to_string().contains("unit 'app'"));
    }

    #[test]
    fn allows_shared_attach_targets() {
        let a = infection(
            "a",
            &[],
            Some("/etc/mosquitto/a.conf"),
            None,
            Some("mosquitto.service"),
        );
        let b = infection(
            "b",
            &[],
            Some("/etc/mosquitto/b.conf"),
            None,
            Some("mosquitto.service"),
        );
        assert!(check_infection_collisions(&[a, b]).is_ok());
    }

    #[test]
    fn validate_deployment_rejects_ties_and_duplicates() {
        let spec = DeploymentSpec {
            meta: super::super::deployment::DeploymentMeta {
                name: "app".into(),
                version: "0.1.0".into(),
            },
            variables: BTreeMap::new(),
            infections: vec![
                super::super::deployment::DeploymentInfection {
                    name: "a".into(),
                    source: "./a.toml".into(),
                    order: 1,
                    vars: BTreeMap::new(),
                },
                super::super::deployment::DeploymentInfection {
                    name: "a".into(),
                    source: "./b.toml".into(),
                    order: 2,
                    vars: BTreeMap::new(),
                },
            ],
        };
        let err = validate_deployment(&spec, &[]).unwrap_err();
        assert!(err.to_string().contains("duplicate infection name"));

        let spec = DeploymentSpec {
            infections: vec![
                super::super::deployment::DeploymentInfection {
                    name: "a".into(),
                    source: "./a.toml".into(),
                    order: 1,
                    vars: BTreeMap::new(),
                },
                super::super::deployment::DeploymentInfection {
                    name: "b".into(),
                    source: "./b.toml".into(),
                    order: 1,
                    vars: BTreeMap::new(),
                },
            ],
            ..spec
        };
        let err = validate_deployment(&spec, &[]).unwrap_err();
        assert!(err.to_string().contains("duplicate order"));
    }

    #[test]
    fn validate_deployment_catches_collisions() {
        let spec = DeploymentSpec {
            meta: super::super::deployment::DeploymentMeta {
                name: "app".into(),
                version: "0.1.0".into(),
            },
            variables: BTreeMap::new(),
            infections: vec![
                super::super::deployment::DeploymentInfection {
                    name: "a".into(),
                    source: "./a.toml".into(),
                    order: 1,
                    vars: BTreeMap::new(),
                },
                super::super::deployment::DeploymentInfection {
                    name: "b".into(),
                    source: "./b.toml".into(),
                    order: 2,
                    vars: BTreeMap::new(),
                },
            ],
        };
        let a = infection("a", &[], Some("/etc/app/conf"), None, None);
        let b = infection("b", &[], Some("/etc/app/conf"), None, None);
        assert!(validate_deployment(&spec, &[a, b]).is_err());
    }
}
