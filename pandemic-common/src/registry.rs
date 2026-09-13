use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InfectionManifest {
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    pub homepage: Option<String>,
    pub license: Option<String>,
    pub keywords: Vec<String>,
    pub dependencies: Vec<String>,
    pub platforms: Vec<Platform>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Platform {
    pub os: String,
    pub arch: String,
    pub binary_url: String,
    pub checksum: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryIndex {
    pub name: String,
    pub description: String,
    pub infections: HashMap<String, InfectionSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InfectionSummary {
    pub name: String,
    pub latest_version: String,
    /// Discriminant: `infection` (binary), `infection-spec`, or `deployment`.
    /// The wire key is `type`; `type_` is the Rust name (reserved word).
    #[serde(rename = "type")]
    pub type_: String,
    pub description: String,
    /// Binary entries: URL of the per-name JSON manifest (platforms + binaries).
    #[serde(default)]
    pub manifest_url: Option<String>,
    /// Spec/deployment entries: URL of the spec+files bundle (tar.gz).
    #[serde(default)]
    pub bundle_url: Option<String>,
    /// sha256 of the bundle (spec/deployment entries).
    #[serde(default)]
    pub checksum: Option<String>,
}

pub struct RegistryClient {
    registries: Vec<String>,
    client: reqwest::Client,
}

impl RegistryClient {
    pub fn new() -> Self {
        let default_url = "https://philcali.github.io/rustic/registry/".to_string();
        let registry_url = std::env::var("PANDEMIC_REGISTRY_URL").unwrap_or(default_url);

        Self {
            registries: vec![registry_url],
            client: reqwest::Client::new(),
        }
    }

    pub fn with_registries(registries: Vec<String>) -> Self {
        Self {
            registries,
            client: reqwest::Client::new(),
        }
    }

    pub fn with_registry_url(url: String) -> Self {
        Self {
            registries: vec![url],
            client: reqwest::Client::new(),
        }
    }

    pub async fn search_infections(&self, query: &str) -> Result<Vec<InfectionSummary>> {
        let mut results = Vec::new();

        for registry_url in &self.registries {
            match self.fetch_registry_index(registry_url).await {
                Ok(index) => {
                    for (_, infection) in index.infections {
                        if infection.name.contains(query) || infection.description.contains(query) {
                            results.push(infection);
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to fetch registry {}: {}", registry_url, e);
                }
            }
        }

        Ok(results)
    }

    pub async fn get_infection_manifest(&self, name: &str) -> Result<InfectionManifest> {
        for registry_url in &self.registries {
            if let Ok(index) = self.fetch_registry_index(registry_url).await {
                if let Some(summary) = index.infections.get(name) {
                    let manifest_url = summary.manifest_url.clone().ok_or_else(|| {
                        anyhow::anyhow!(
                            "'{name}' has no binary manifest (type {}); use its spec bundle instead",
                            summary.type_
                        )
                    })?;
                    let manifest = self
                        .client
                        .get(&manifest_url)
                        .send()
                        .await?
                        .json::<InfectionManifest>()
                        .await?;
                    return Ok(manifest);
                }
            }
        }
        Err(anyhow::anyhow!(
            "Infection '{}' not found in any registry",
            name
        ))
    }

    pub async fn download_infection(
        &self,
        manifest: &InfectionManifest,
        target_path: &str,
    ) -> Result<()> {
        let platform = self.get_current_platform(manifest)?;

        let response = self.client.get(&platform.binary_url).send().await?;

        let bytes = response.bytes().await?;

        // Verify checksum
        let actual_checksum = sha256::digest(&*bytes);
        if actual_checksum != platform.checksum {
            return Err(anyhow::anyhow!("Checksum mismatch for {}", manifest.name));
        }

        std::fs::write(target_path, bytes)?;

        // Make executable
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(target_path)?.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(target_path, perms)?;
        }

        Ok(())
    }

    /// Look up a single atom (binary, infection-spec, or deployment) by name
    /// and return the registry base URL it was found under, plus its index
    /// summary — including the `type` discriminant, the spec-bundle URL, and
    /// its sha256. The base URL is needed to resolve a relative `bundle_url`
    /// against the registry that actually served the index.
    pub async fn get_infection_summary(
        &self,
        name: &str,
    ) -> Result<(String, InfectionSummary)> {
        for registry_url in &self.registries {
            if let Ok(index) = self.fetch_registry_index(registry_url).await {
                if let Some(summary) = index.infections.get(name) {
                    return Ok((registry_url.clone(), summary.clone()));
                }
            }
        }
        Err(anyhow::anyhow!(
            "'{name}' not found in any registry (tried {})",
            self.registries.len()
        ))
    }

    /// Fetch an atom's spec/deployment bundle, verify its sha256, and extract
    /// it (tar.gz) into `root`. Returns the extracted top-level directory
    /// (`root/<name>/`).
    ///
    /// A relative `bundle_url` is resolved against `base_url` (the registry
    /// that served the index), so `PANDEMIC_REGISTRY_URL` / `--registry-url`
    /// control both the index and the bundles. The download is
    /// integrity-checked against the index's `checksum`, and extraction uses
    /// `Entry::unpack_in`, which refuses absolute paths and `..` components so
    /// nothing can land outside `root` (tar-slip guard).
    pub async fn fetch_bundle_into(
        &self,
        base_url: &str,
        summary: &InfectionSummary,
        root: &Path,
    ) -> Result<PathBuf> {
        let bundle_url = summary.bundle_url.clone().ok_or_else(|| {
            anyhow::anyhow!(
                "'{}' has no spec bundle (it is a {} atom with no bundle_url)",
                summary.name,
                summary.type_
            )
        })?;
        let bundle_url = resolve_bundle_url(base_url, &bundle_url);
        let expected = summary.checksum.clone().ok_or_else(|| {
            anyhow::anyhow!("'{}' has no bundle checksum; cannot verify integrity", summary.name)
        })?;

        let bytes = self
            .client
            .get(&bundle_url)
            .send()
            .await
            .with_context(|| format!("downloading bundle for '{}'", summary.name))?
            .bytes()
            .await?;

        verify_sha256(&bytes, &expected, &summary.name)?;
        extract_bundle(&bytes, &summary.name, root)
    }

    async fn fetch_registry_index(&self, registry_url: &str) -> Result<RegistryIndex> {
        let index_url = format!("{}/index.json", registry_url);
        let index = self
            .client
            .get(&index_url)
            .send()
            .await?
            .json::<RegistryIndex>()
            .await?;
        Ok(index)
    }

    fn get_current_platform<'a>(&self, manifest: &'a InfectionManifest) -> Result<&'a Platform> {
        let os = std::env::consts::OS;
        let arch = std::env::consts::ARCH;

        manifest
            .platforms
            .iter()
            .find(|p| p.os == os && p.arch == arch)
            .ok_or_else(|| anyhow::anyhow!("No binary available for {}-{}", os, arch))
    }
}

impl Default for RegistryClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolve a bundle URL against a registry base URL.
///
/// Absolute URLs (`http(s)://...`) are used as-is (backward compatible with
/// older indices). Otherwise the path is joined onto `base_url`, which is the
/// registry that served the index — so overriding the registry base controls
/// where bundles are fetched from.
///
/// Pure and side-effect-free so it can be unit-tested without a network.
pub fn resolve_bundle_url(base_url: &str, bundle_url: &str) -> String {
    if bundle_url.starts_with("http://") || bundle_url.starts_with("https://") {
        return bundle_url.to_string();
    }
    let base = if base_url.ends_with('/') {
        base_url.to_string()
    } else {
        format!("{base_url}/")
    };
    format!("{base}{}", bundle_url.trim_start_matches('/'))
}

/// Verify a downloaded blob's sha256 against the index-published digest.
///
/// Pure and side-effect-free so it can be unit-tested without a network.
pub fn verify_sha256(bytes: &[u8], expected: &str, name: &str) -> Result<()> {
    let actual = sha256::digest(bytes);
    if actual != expected {
        bail!(
            "checksum mismatch for '{name}' (expected {expected}, got {actual})"
        )
    }
    Ok(())
}

/// Extract a tar.gz spec bundle into `root`, guarded against path traversal.
///
/// The bundle's top-level directory is the atom's name (that is how
/// `scripts/generate-registry.sh` lays it out), so the extracted content lands
/// at `root/<name>/`. Each entry is extracted with `Entry::unpack_in` —
/// unlike `unpack`, it refuses absolute paths and `..` components, so a
/// hostile bundle cannot write outside `root` (the tar-slip / path-traversal
/// guard).
pub fn extract_bundle(bytes: &[u8], name: &str, root: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(root)
        .with_context(|| format!("creating extraction dir {}", root.display()))?;

    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    for entry in archive
        .entries()
        .with_context(|| format!("reading bundle entries for '{name}'"))?
    {
        let mut entry = entry.with_context(|| format!("reading a bundle entry for '{name}'"))?;
        entry
            .unpack_in(root)
            .with_context(|| {
                format!("extracting a bundle entry for '{name}' into {}", root.display())
            })?;
    }

    let dir = root.join(name);
    if !dir.is_dir() {
        bail!(
            "bundle '{name}' did not extract to the expected directory {}",
            dir.display()
        );
    }
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape `scripts/generate-registry.sh` emits today: a `type` key (not
    /// `type_`) and a `manifest_url`. This is the bug the `#[serde(rename)]`
    /// fix addresses — before it, this failed to deserialize.
    #[test]
    fn parses_binary_index_shape() {
        let json = r#"{
            "name": "Pandemic Infections Registry",
            "description": "test",
            "infections": {
                "mosquitto": {
                    "name": "mosquitto",
                    "latest_version": "2.0",
                    "description": "MQTT broker",
                    "type": "infection",
                    "manifest_url": "https://x/registry/mosquitto.json"
                },
                "pandemic-cli": {
                    "name": "pandemic-cli",
                    "latest_version": "0.4.0",
                    "description": "core",
                    "type": "core",
                    "manifest_url": "https://x/registry/pandemic-cli.json"
                }
            }
        }"#;

        let index: RegistryIndex = serde_json::from_str(json).expect("binary index parses");
        let m = &index.infections["mosquitto"];
        assert_eq!(m.type_, "infection");
        assert_eq!(m.manifest_url.as_deref(), Some("https://x/registry/mosquitto.json"));
        assert_eq!(m.bundle_url, None);
        assert_eq!(m.checksum, None);
        assert_eq!(index.infections["pandemic-cli"].type_, "core");
    }

    /// Spec/deployment atoms: `type` discriminant, a `bundle_url` + `checksum`,
    /// and *no* `manifest_url` (all three optional).
    #[test]
    fn parses_spec_and_deployment_entries() {
        let json = r#"{
            "name": "Pandemic Registry",
            "description": "test",
            "infections": {
                "rest": {
                    "name": "rest",
                    "latest_version": "0.4.0",
                    "description": "REST API spec",
                    "type": "infection-spec",
                    "bundle_url": "https://x/registry/specs/infections/rest.tar.gz",
                    "checksum": "deadbeef"
                },
                "pandemic-full": {
                    "name": "pandemic-full",
                    "latest_version": "0.4.0",
                    "description": "full stack",
                    "type": "deployment",
                    "bundle_url": "https://x/registry/specs/deployments/pandemic-full.tar.gz",
                    "checksum": "cafe0123"
                }
            }
        }"#;

        let index: RegistryIndex = serde_json::from_str(json).expect("spec index parses");
        let rest = &index.infections["rest"];
        assert_eq!(rest.type_, "infection-spec");
        assert_eq!(rest.manifest_url, None);
        assert_eq!(
            rest.bundle_url.as_deref(),
            Some("https://x/registry/specs/infections/rest.tar.gz")
        );
        assert_eq!(rest.checksum.as_deref(), Some("deadbeef"));

        let dep = &index.infections["pandemic-full"];
        assert_eq!(dep.type_, "deployment");
        assert_eq!(dep.bundle_url.as_deref(), Some("https://x/registry/specs/deployments/pandemic-full.tar.gz"));
        assert_eq!(dep.checksum.as_deref(), Some("cafe0123"));
    }

    /// Serde round-trip is stable (Serialize emits the `type` key, Deserialize
    /// reads it back).
    #[test]
    fn summary_round_trips() {
        let summary = InfectionSummary {
            name: "rest".into(),
            latest_version: "0.4.0".into(),
            type_: "infection-spec".into(),
            description: "d".into(),
            manifest_url: None,
            bundle_url: Some("https://x/b.tar.gz".into()),
            checksum: Some("abc".into()),
        };
        let json = serde_json::to_string(&summary).unwrap();
        assert!(json.contains("\"type\":\"infection-spec\""), "emits `type` key, got {json}");
        let back: InfectionSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(back.type_, "infection-spec");
        assert_eq!(back.bundle_url.as_deref(), Some("https://x/b.tar.gz"));
    }

    /// Build a real `.tar.gz` in memory: a top-level `<name>/` dir holding the
    /// given relative files — mirroring `generate-registry.sh`'s layout.
    fn make_bundle(name: &str, files: &[(&str, &[u8])]) -> Result<Vec<u8>> {
        let mut tar = tar::Builder::new(Vec::new());
        for &(relpath, data) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, format!("{name}/{relpath}"), data)
                .with_context(|| format!("adding {relpath} to bundle"))?;
        }
        let tar_bytes = tar.into_inner()?;
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        use std::io::Write;
        enc.write_all(&tar_bytes)?;
        Ok(enc.finish()?)
    }

    #[test]
    fn verify_sha256_accepts_matching_rejects_other() {
        let data = b"payload";
        let good = sha256::digest(data);
        assert!(verify_sha256(data, &good, "atom").is_ok(), "matching digest passes");
        assert!(
            verify_sha256(data, "deadbeef", "atom").is_err(),
            "wrong digest fails"
        );
    }

    #[test]
    fn resolve_bundle_url_absolute_passthrough_and_relative_join() {
        // Absolute bundle URLs are used verbatim (back-compat with older
        // indices that publish absolute URLs), regardless of the base.
        assert_eq!(
            resolve_bundle_url(
                "http://localhost:8000/registry/",
                "https://x/registry/specs/rest.tar.gz"
            ),
            "https://x/registry/specs/rest.tar.gz"
        );
        assert_eq!(
            resolve_bundle_url(
                "http://localhost:8000/registry/",
                "http://cdn.example.com/b.tar.gz"
            ),
            "http://cdn.example.com/b.tar.gz"
        );

        // Relative bundle URLs join onto the registry base that served the
        // index, whether or not the base ends in a slash, and a stray leading
        // slash on the bundle path is trimmed.
        assert_eq!(
            resolve_bundle_url(
                "http://localhost:8000/registry/",
                "specs/infections/rest.tar.gz"
            ),
            "http://localhost:8000/registry/specs/infections/rest.tar.gz"
        );
        assert_eq!(
            resolve_bundle_url(
                "http://localhost:8000/registry",
                "specs/infections/rest.tar.gz"
            ),
            "http://localhost:8000/registry/specs/infections/rest.tar.gz"
        );
        assert_eq!(
            resolve_bundle_url(
                "http://localhost:8000/registry/",
                "/specs/rest.tar.gz"
            ),
            "http://localhost:8000/registry/specs/rest.tar.gz"
        );
    }

    #[test]
    fn extract_bundle_lands_under_root_with_expected_layout() {
        let bytes = make_bundle(
            "rest",
            &[
                ("infection.toml", b"[infection]\nname = \"rest\"\n"),
                ("files/rest-auth.toml", b"token = \"x\"\n"),
                ("rest.service", b"[Unit]\nName=rest\n"),
            ],
        )
        .unwrap();

        let tmp = tempfile::tempdir().unwrap();
        let dir = extract_bundle(&bytes, "rest", tmp.path()).unwrap();

        // Returned dir is the atom's top-level dir, strictly under the root.
        assert!(dir.starts_with(tmp.path()));
        assert!(tmp.path().join("rest/infection.toml").is_file());
        assert!(tmp.path().join("rest/files/rest-auth.toml").is_file());
        assert!(tmp.path().join("rest/rest.service").is_file());
    }

    #[test]
    fn extract_bundle_never_writes_outside_root() {
        let tmp = tempfile::tempdir().unwrap();
        let parent = tmp.path().parent().unwrap().to_path_buf();

        match make_bundle("../evil", &[( "payload", b"pwned")]) {
            Ok(bytes) => match extract_bundle(&bytes, "../evil", tmp.path()) {
                Ok(dir) => {
                    // If it succeeded at all, the dir must still be under the root.
                    assert!(dir.starts_with(tmp.path()), "extracted dir escaped root");
                }
                Err(_) => {} // the traversal guard rejected it — expected.
            },
            Err(_) => {} // the tar crate refused to build a `..` entry — also safe.
        }

        // In every case, nothing may have been written outside the root.
        assert!(
            !parent.join("evil").exists(),
            "path-traversal entry escaped the extraction root"
        );
        assert!(
            !parent.join("payload").exists(),
            "path-traversal entry escaped the extraction root"
        );
    }
}
