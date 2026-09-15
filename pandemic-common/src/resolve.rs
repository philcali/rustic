//! Registry resolution (ideas/deployments.md, phase 7) — the async
//! "fetch + materialize" step that runs *before* the pure, offline plan
//! builder in [`crate::apply`].
//!
//! A registry *atom* (an `infection-spec` or a `deployment`) is published as
//! a single checksummed tar.gz bundle. Resolving an install `target` (a
//! registry name) means:
//!
//! 1. look the atom up in the index and confirm its `type` (routing guard —
//!    the CLI noun is authoritative, so a mismatch is a clear error);
//! 2. fetch the bundle, sha256-verify it, and untar it into a temp tree
//!    (path-traversal-guarded, see [`crate::registry::extract_bundle`]);
//! 3. for a deployment, repeat for every bare-named infection it references;
//! 4. hand that tree to the pure builder, which renders the fully concrete
//!    plan (all file/unit content in-band).
//!
//! The temp tree is dropped when this function returns; because [`Plan`]
//! carries its content in-band, nothing on disk is needed afterwards. Local
//! specs (a `target` path) never enter this path — they stay offline.

use std::collections::BTreeMap;

use anyhow::{bail, Context, Result};
use pandemic_protocol::spec::{parse_deployment_spec, parse_infection_spec};
use pandemic_protocol::Plan;

use crate::apply::{build_deployment_plan_from, build_plan, validate_set, DeploymentPlan};
use crate::registry::RegistryClient;

const INFECTION_SPEC: &str = "infection-spec";
const DEPLOYMENT: &str = "deployment";

/// Confirm `target` is an atom of `expected` type; on a mismatch, point the
/// user at the command that installs *that* kind of atom.
fn expect_atom_type(actual: &str, expected: &str, target: &str) -> Result<()> {
    if actual == expected {
        return Ok(());
    }
    let correct = match actual {
        "infection" => format!("pandemic-cli registry install {target}"),
        INFECTION_SPEC => format!("pandemic-cli infection install {target}"),
        DEPLOYMENT => format!("pandemic-cli deployment install {target}"),
        other => format!("(atom type `{other}`)"),
    };
    bail!("'{target}' is a `{actual}` atom, not a `{expected}` atom.\n  to install it: {correct}")
}

/// Resolve a registry *infection-spec* atom into a concrete [`Plan`].
///
/// `target` is the registry name; `set` holds the `--set`/`vars` values (they
/// are validated against the atom's declared variables once the spec is
/// materialized).
pub async fn resolve_infection_target(
    client: &RegistryClient,
    target: &str,
    set: &BTreeMap<String, String>,
) -> Result<Plan> {
    let (base, summary) = client.get_infection_summary(target).await?;
    expect_atom_type(&summary.type_, INFECTION_SPEC, target)?;

    let root = tempfile::tempdir().context("creating a temp dir for the registry bundle")?;
    let dir = client
        .fetch_bundle_into(&base, &summary, root.path())
        .await?;

    let text = std::fs::read_to_string(dir.join("infection.toml")).with_context(|| {
        format!("infection-spec bundle for '{target}' is missing infection.toml")
    })?;
    let spec = parse_infection_spec(&text)
        .with_context(|| format!("parsing infection spec for '{target}'"))?;

    let declared: Vec<String> = spec.variables.keys().cloned().collect();
    validate_set(set, &declared, &format!("infection '{}'", spec.meta.name))?;

    // Standalone: no deployment bindings or shared variables.
    build_plan(&spec, &dir, &BTreeMap::new(), &BTreeMap::new(), set)
        .with_context(|| format!("building infection plan for '{target}'"))
}

/// Resolve a registry *deployment* atom into a concrete [`DeploymentPlan`].
///
/// The deployment bundle is fetched first; every bare-named infection it
/// references is then fetched and materialized next to it, so the pure
/// builder can resolve each `source` relative to the shared temp root.
pub async fn resolve_deployment_target(
    client: &RegistryClient,
    target: &str,
    set: &BTreeMap<String, String>,
) -> Result<DeploymentPlan> {
    let (base, summary) = client.get_infection_summary(target).await?;
    expect_atom_type(&summary.type_, DEPLOYMENT, target)?;

    let root = tempfile::tempdir().context("creating a temp dir for the registry bundles")?;

    // 1. Fetch + extract the deployment atom.
    let dep_dir = client
        .fetch_bundle_into(&base, &summary, root.path())
        .await?;
    let dtext = std::fs::read_to_string(dep_dir.join("deployment.toml"))
        .with_context(|| format!("deployment bundle for '{target}' is missing deployment.toml"))?;
    let spec = parse_deployment_spec(&dtext)
        .with_context(|| format!("parsing deployment spec for '{target}'"))?;

    // Validate the caller's --set against the deployment's declared variables.
    let declared: Vec<String> = spec.variables.keys().cloned().collect();
    validate_set(set, &declared, &format!("deployment '{}'", spec.meta.name))?;

    // 2. Fetch + extract every bare-named infection this deployment references.
    for entry in &spec.infections {
        if entry.source.contains('/') {
            // A local path (absolute or relative). Not a registry atom; let the
            // builder resolve it against the tree (it will report if missing).
            continue;
        }
        let (inf_base, inf) = client.get_infection_summary(&entry.source).await?;
        if inf.type_ != INFECTION_SPEC {
            bail!(
                "deployment '{}' references '{}', which is a {} atom, not an infection-spec",
                spec.meta.name,
                entry.source,
                inf.type_
            );
        }
        client
            .fetch_bundle_into(&inf_base, &inf, root.path())
            .await?;
    }

    // 3. Rewrite bare sources to `<name>/infection.toml` so the builder's
    //    `resolve_source` (relative to the shared root) finds each spec.
    let mut materialized = spec.clone();
    for entry in &mut materialized.infections {
        if !entry.source.contains('/') {
            entry.source = format!("{}/infection.toml", entry.source);
        }
    }

    build_deployment_plan_from(&materialized, root.path(), set)
        .with_context(|| format!("building deployment plan for '{target}'"))
}
