use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
}
