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
    canonical_unit_name, is_variable_name, parse_infection_spec, render_template,
    resolve_infection, InfectionSpec,
};
use pandemic_protocol::{Plan, PlanUnit, RenderedFile, UserConfig};
use sha2::{Digest, Sha256};

/// Parse `--set key=value` pairs against a set of declared variable names.
pub fn parse_set_args(
    args: &[String],
    declared: &[String],
    context: &str,
) -> Result<BTreeMap<String, String>> {
    let mut set = BTreeMap::new();
    for arg in args {
        let (key, value) = arg
            .split_once('=')
            .ok_or_else(|| anyhow!("invalid --set '{arg}': expected key=value"))?;
        if !is_variable_name(key) {
            bail!("invalid variable name '{key}' in --set '{arg}'");
        }
        if !declared.iter().any(|d| d == key) {
            bail!(
                "unknown variable '{key}' for {context}: declared variables are [{}]",
                declared.join(", ")
            );
        }
        set.insert(key.to_string(), value.to_string());
    }
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
