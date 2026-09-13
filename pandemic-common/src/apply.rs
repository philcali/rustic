//! Pure `Plan` step of the Plan/Apply boundary (ideas/deployments.md, phase 5).
//!
//! Shared by `pandemic-cli` and `pandemic-rest`: read a local infection spec
//! plus its templates, resolve variables, and render everything into a
//! concrete [`pandemic_protocol::Plan`]. No network and no privileged action
//! here — the agent performs the `Apply` step (executes the plan with its
//! privileged primitives). Keeping this in the common crate is what lets the
//! CLI and the REST API drive the *same* plan without drifting.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use pandemic_protocol::spec::{
    canonical_unit_name, is_variable_name, parse_deployment_spec, parse_infection_spec,
    render_template, resolve_deployment_variables, resolve_infection, validate_deployment,
    DeploymentSpec, InfectionSpec,
};
use pandemic_protocol::{ApplyDeploymentInfection, Plan, PlanUnit, RenderedFile, UserConfig};
use sha2::{Digest, Sha256};

/// Parse `--set key=value` pairs into a map, validating each key is a valid
/// variable name (but *not* against a declared set — see [`validate_set`]).
///
/// Split from [`parse_set_args`] so a registry target — whose declared
/// variables are only known after the bundle is materialized — can parse the
/// raw `--set` values first and validate them later.
pub fn parse_set_values(args: &[String]) -> Result<BTreeMap<String, String>> {
    let mut set = BTreeMap::new();
    for arg in args {
        let (key, value) = arg
            .split_once('=')
            .ok_or_else(|| anyhow!("invalid --set '{arg}': expected key=value"))?;
        if !is_variable_name(key) {
            bail!("invalid variable name '{key}' in --set '{arg}'");
        }
        set.insert(key.to_string(), value.to_string());
    }
    Ok(set)
}

/// Check every key in an already-parsed `set` map against declared variable
/// names. Shared by the local flow (after reading the spec) and the registry
/// flow (after materializing the bundle).
pub fn validate_set(
    set: &BTreeMap<String, String>,
    declared: &[String],
    context: &str,
) -> Result<()> {
    for key in set.keys() {
        if !declared.iter().any(|d| d == key) {
            bail!(
                "unknown variable '{key}' for {context}: declared variables are [{}]",
                declared.join(", ")
            );
        }
    }
    Ok(())
}

/// Parse `--set key=value` pairs against a set of declared variable names.
pub fn parse_set_args(
    args: &[String],
    declared: &[String],
    context: &str,
) -> Result<BTreeMap<String, String>> {
    let set = parse_set_values(args)?;
    validate_set(&set, declared, context)?;
    Ok(set)
}

/// Locate a template: next to the spec first, then in `files/`.
pub fn find_template(spec_dir: &Path, key: &str) -> Option<PathBuf> {
    [spec_dir.join(key), spec_dir.join("files").join(key)]
        .into_iter()
        .find(|candidate| candidate.is_file())
}

pub fn sha256_hex(content: &str) -> String {
    hex::encode(Sha256::digest(content.as_bytes()))
}

/// Resolve variables and render every template into a concrete [`Plan`].
/// Pure local work (filesystem reads of the spec + templates only) — it never
/// talks to the agent, so `--dry-run` stays offline. Fails before anything is
/// applied: unknown variables, missing templates, or unresolvable references.
pub fn build_plan(
    spec: &InfectionSpec,
    spec_dir: &Path,
    bindings: &BTreeMap<String, String>,
    shared: &BTreeMap<String, String>,
    set: &BTreeMap<String, String>,
) -> Result<Plan> {
    let variables = resolve_infection(spec, bindings, shared, set)
        .with_context(|| format!("resolving variables for infection '{}'", spec.meta.name))?;

    let mut files = Vec::new();
    for (source, placement) in &spec.files {
        let template_path = find_template(spec_dir, source).ok_or_else(|| {
            anyhow!(
                "template '{source}' not found next to the spec (in {}) or in {}/files/",
                spec_dir.display(),
                spec_dir.display()
            )
        })?;
        let raw = std::fs::read_to_string(&template_path)
            .with_context(|| format!("reading template {}", template_path.display()))?;
        let content = render_template(&raw, &variables)
            .with_context(|| format!("rendering template '{source}'"))?;
        files.push(RenderedFile {
            target: placement.target.clone(),
            content,
            owner: placement.owner.clone().unwrap_or_else(|| "root".into()),
            mode: placement.mode.clone().unwrap_or_else(|| "0644".into()),
        });
    }

    let mut unit = None;
    let mut attach = None;
    if let Some(systemd) = &spec.systemd {
        if let Some(unit_file) = &systemd.unit_file {
            let template_path = find_template(spec_dir, unit_file).ok_or_else(|| {
                anyhow!(
                    "unit file template '{unit_file}' not found next to the spec (in {}) or in {}/files/",
                    spec_dir.display(),
                    spec_dir.display()
                )
            })?;
            let raw = std::fs::read_to_string(&template_path)
                .with_context(|| format!("reading unit template {}", template_path.display()))?;
            let content = render_template(&raw, &variables)
                .with_context(|| format!("rendering unit template '{unit_file}'"))?;
            let file = if unit_file.contains('.') {
                unit_file.clone()
            } else {
                format!("{unit_file}.service")
            };
            let target = format!("/etc/systemd/system/{file}");
            unit = Some(PlanUnit {
                name: canonical_unit_name(&file),
                target,
                content,
                enable: systemd.enable,
            });
        }
        if let Some(target) = &systemd.attach {
            attach = Some(target.clone());
        }
    }

    let health_check: Vec<String> = spec
        .health
        .check
        .iter()
        .map(|part| render_template(part, &variables))
        .collect::<Result<_>>()
        .with_context(|| "rendering health check command")?;
    let health_interval = spec.health.interval;

    let declared_packages: BTreeMap<String, Vec<String>> = spec
        .packages
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    let groups: Vec<String> = spec.groups.keys().cloned().collect();
    let users: Vec<(String, UserConfig)> = spec
        .users
        .iter()
        .map(|(username, user_spec)| {
            (
                username.clone(),
                UserConfig {
                    shell: None,
                    home_dir: None,
                    groups: Some(vec![user_spec
                        .group
                        .clone()
                        .unwrap_or_else(|| username.clone())]),
                    system_user: Some(user_spec.system),
                },
            )
        })
        .collect();

    Ok(Plan {
        name: spec.meta.name.clone(),
        version: spec.meta.version.clone(),
        description: spec.meta.description.clone(),
        variables,
        files,
        unit,
        attach,
        health_check,
        health_interval,
        declared_packages,
        groups,
        users,
    })
}

/// Read + parse an infection spec file and build its concrete [`Plan`].
///
/// The convenience entry point for a standalone infection (no deployment
/// bindings or shared variables).
pub fn build_plan_from_spec(spec_path: &Path, set: &BTreeMap<String, String>) -> Result<Plan> {
    let empty = BTreeMap::new();
    build_plan_from_spec_with(spec_path, &empty, &empty, set)
}

/// Read + parse an infection spec file and build its concrete [`Plan`], with
/// deployment `bindings` and shared `variables`.
pub fn build_plan_from_spec_with(
    spec_path: &Path,
    bindings: &BTreeMap<String, String>,
    shared: &BTreeMap<String, String>,
    set: &BTreeMap<String, String>,
) -> Result<Plan> {
    let text = std::fs::read_to_string(spec_path)
        .with_context(|| format!("reading infection spec {}", spec_path.display()))?;
    let spec = parse_infection_spec(&text)
        .with_context(|| format!("parsing infection spec {}", spec_path.display()))?;
    let spec_dir = spec_path
        .parent()
        .map(Path::to_path_buf)
        .filter(|p| p != Path::new(""))
        .unwrap_or_else(|| PathBuf::from("."));
    build_plan(&spec, &spec_dir, bindings, shared, set)
}

/// One infection in a [`DeploymentPlan`], fully resolved and rendered.
pub struct ResolvedInfection {
    /// The infection's name within the deployment.
    pub name: String,
    /// Explicit install order (lower first).
    pub order: u64,
    /// The `source` as declared in the deployment spec.
    pub source: String,
    /// The parsed infection spec (kept for validation / tooling).
    pub spec: InfectionSpec,
    /// The concrete rendered plan the agent will apply.
    pub plan: Plan,
}

/// A fully resolved + rendered deployment, ready to dry-run or apply.
///
/// This is the shared artifact of the deployment `Plan` step: the CLI and the
/// REST API both build one of these and then either print it (dry-run) or send
/// its concrete plans to the agent as an `ApplyDeployment` request.
pub struct DeploymentPlan {
    /// The parsed deployment spec (name, version, declared wiring).
    pub spec: DeploymentSpec,
    /// Resolved shared variables.
    pub shared: BTreeMap<String, String>,
    /// Every infection, sorted by install `order`, each with its plan.
    pub infections: Vec<ResolvedInfection>,
}

/// The concrete plans for an `ApplyDeployment` request.
pub fn deployment_apply_infections(dp: &DeploymentPlan) -> Vec<ApplyDeploymentInfection> {
    dp.infections
        .iter()
        .map(|r| ApplyDeploymentInfection {
            name: r.name.clone(),
            version: r.plan.version.clone(),
            order: r.order,
            source: r.source.clone(),
            plan: r.plan.clone(),
        })
        .collect()
}

/// Resolve a deployment `source` to a local infection spec file.
fn resolve_source(spec_dir: &Path, source: &str) -> Result<PathBuf> {
    let candidate = if source.contains('/') {
        let p = Path::new(source);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            spec_dir.join(p)
        }
    } else {
        bail!(
            "source '{source}' is not a local path.\n  registry resolution arrives in a later phase (ideas/deployments.md, phase 5)"
        )
    };
    if !candidate.is_file() {
        bail!(
            "infection spec '{source}' not found (resolved to {})",
            candidate.display()
        );
    }
    Ok(candidate)
}

/// Resolve + render every infection in a deployment spec into a concrete
/// [`DeploymentPlan`].
///
/// Pure local work: reads the deployment spec, each infection spec, and every
/// template; resolves shared + per-infection variables; renders; validates.
/// No network, no privileged action. Shared by the CLI and REST API so they
/// drive the identical plan.
pub fn build_deployment_plan_from(
    spec: &DeploymentSpec,
    spec_dir: &Path,
    set: &BTreeMap<String, String>,
) -> Result<DeploymentPlan> {
    let shared = resolve_deployment_variables(&spec.variables, set)
        .with_context(|| "resolving deployment variables")?;

    let mut infections: Vec<ResolvedInfection> = Vec::new();
    for entry in &spec.infections {
        let source_path = resolve_source(spec_dir, &entry.source)
            .with_context(|| format!("resolving source for infection '{}'", entry.name))?;
        let itext = std::fs::read_to_string(&source_path)
            .with_context(|| format!("reading infection spec {}", source_path.display()))?;
        let ispec = parse_infection_spec(&itext)
            .with_context(|| format!("parsing infection spec {}", source_path.display()))?;
        let idir = source_path
            .parent()
            .map(Path::to_path_buf)
            .filter(|p| p != Path::new(""))
            .unwrap_or_else(|| PathBuf::from("."));
        let plan = build_plan(&ispec, &idir, &entry.vars, &shared, &BTreeMap::new())
            .with_context(|| format!("building plan for infection '{}'", entry.name))?;
        infections.push(ResolvedInfection {
            name: entry.name.clone(),
            order: entry.order,
            source: entry.source.clone(),
            spec: ispec,
            plan,
        });
    }
    infections.sort_by_key(|i| i.order);

    let specs: Vec<InfectionSpec> = infections.iter().map(|i| i.spec.clone()).collect();
    validate_deployment(spec, &specs)?;

    Ok(DeploymentPlan {
        spec: spec.clone(),
        shared,
        infections,
    })
}

/// Read + parse a deployment spec file and build its concrete
/// [`DeploymentPlan`]. Convenience entry point used by the REST API.
pub fn build_deployment_plan(
    spec_path: &Path,
    set: &BTreeMap<String, String>,
) -> Result<DeploymentPlan> {
    let text = std::fs::read_to_string(spec_path)
        .with_context(|| format!("reading deployment spec {}", spec_path.display()))?;
    let spec = parse_deployment_spec(&text)
        .with_context(|| format!("parsing deployment spec {}", spec_path.display()))?;
    let spec_dir = spec_path
        .parent()
        .map(Path::to_path_buf)
        .filter(|p| p != Path::new(""))
        .unwrap_or_else(|| PathBuf::from("."));
    build_deployment_plan_from(&spec, &spec_dir, set)
}
