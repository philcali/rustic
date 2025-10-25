use anyhow::Result;
use std::path::{Path, PathBuf};

/// Configuration management trait for handling plugin configurations
#[allow(async_fn_in_trait)]
pub trait ConfigManager {
    /// Get merged configuration for a plugin (defaults + overrides)
    async fn get_config(&self, plugin_name: &str) -> Result<serde_json::Value>;

    /// Set override configuration (typically from cloud agent)
    async fn set_override(&self, plugin_name: &str, config: serde_json::Value) -> Result<()>;

    /// Remove override configuration (revert to defaults)
    async fn clear_override(&self, plugin_name: &str) -> Result<()>;
}

/// File-based configuration manager
pub struct FileConfigManager {
    default_dir: PathBuf,  // /etc/pandemic/plugins/
    override_dir: PathBuf, // /var/lib/pandemic/overrides/
}

impl FileConfigManager {
    pub fn new<P: AsRef<Path>>(default_dir: P, override_dir: P) -> Self {
        Self {
            default_dir: default_dir.as_ref().to_path_buf(),
            override_dir: override_dir.as_ref().to_path_buf(),
        }
    }

    pub fn new_default() -> Self {
        Self::new("/etc/pandemic/plugins", "/var/lib/pandemic/overrides")
    }

    async fn load_toml_file(&self, path: &Path) -> Result<serde_json::Value> {
        let content = tokio::fs::read_to_string(path).await?;
        let toml_value: toml::Value = toml::from_str(&content)?;
        let json_value = serde_json::to_value(toml_value)?;
        Ok(json_value)
    }
}

fn merge_json(base: &mut serde_json::Value, override_val: serde_json::Value) {
    if let serde_json::Value::Object(override_map) = &override_val {
        if let serde_json::Value::Object(base_map) = base {
            for (key, value) in override_map {
                if let Some(base_value) = base_map.get_mut(key) {
                    merge_json(base_value, value.clone());
                } else {
                    base_map.insert(key.clone(), value.clone());
                }
            }
            return;
        }
    }
    *base = override_val;
}

impl ConfigManager for FileConfigManager {
    async fn get_config(&self, plugin_name: &str) -> Result<serde_json::Value> {
        let default_path = self.default_dir.join(format!("{}.toml", plugin_name));
        let override_path = self.override_dir.join(format!("{}.toml", plugin_name));

        // Start with defaults (empty object if file doesn't exist)
        let mut config = self
            .load_toml_file(&default_path)
            .await
            .unwrap_or_else(|_| serde_json::json!({}));

        // Apply overrides if they exist
        if let Ok(overrides) = self.load_toml_file(&override_path).await {
            merge_json(&mut config, overrides);
        }

        Ok(config)
    }

    async fn set_override(&self, plugin_name: &str, config: serde_json::Value) -> Result<()> {
        // Ensure override directory exists
        tokio::fs::create_dir_all(&self.override_dir).await?;

        let override_path = self.override_dir.join(format!("{}.toml", plugin_name));

        // Convert JSON to TOML and write
        let toml_value: toml::Value = serde_json::from_value(config)?;
        let toml_string = toml::to_string_pretty(&toml_value)?;

        tokio::fs::write(override_path, toml_string).await?;
        Ok(())
    }

    async fn clear_override(&self, plugin_name: &str) -> Result<()> {
        let override_path = self.override_dir.join(format!("{}.toml", plugin_name));

        if override_path.exists() {
            tokio::fs::remove_file(override_path).await?;
        }

        Ok(())
    }
}
