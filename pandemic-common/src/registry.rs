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
    pub description: String,
    pub manifest_url: String,
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
                    let manifest = self
                        .client
                        .get(&summary.manifest_url)
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
