use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub api_key: String,
    pub roles: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Role {
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    pub identities: HashMap<String, Identity>,
    pub roles: HashMap<String, Role>,
}

impl AuthConfig {
    pub async fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = tokio::fs::read_to_string(path).await?;
        let config: AuthConfig = toml::from_str(&content)?;
        Ok(config)
    }

    pub fn authenticate(&self, api_key: &str) -> Option<Vec<String>> {
        // Find identity by API key
        let identity = self.identities.values().find(|id| id.api_key == api_key)?;

        // Collect all scopes from user's roles
        let mut scopes = Vec::new();
        for role_name in &identity.roles {
            if let Some(role) = self.roles.get(role_name) {
                scopes.extend(role.scopes.clone());
            }
        }

        Some(scopes)
    }

    pub fn authorize(&self, scopes: &[String], required_scope: &str) -> bool {
        // Check for wildcard admin access
        if scopes.contains(&"*".to_string()) {
            return true;
        }

        // Check for exact scope match
        scopes.contains(&required_scope.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_auth_config_load() {
        let config_content = r#"
[identities.admin]
api_key = "admin-key"
roles = ["admin"]

[identities.reader]
api_key = "reader-key"
roles = ["reader"]

[roles.admin]
scopes = ["*"]

[roles.reader]
scopes = ["plugins:read", "health:read"]
"#;

        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(config_content.as_bytes()).unwrap();
        temp_file.flush().unwrap();

        let config = AuthConfig::load(temp_file.path()).await.unwrap();

        // Test authentication
        let admin_scopes = config.authenticate("admin-key").unwrap();
        assert!(config.authorize(&admin_scopes, "plugins:write"));

        let reader_scopes = config.authenticate("reader-key").unwrap();
        assert!(config.authorize(&reader_scopes, "plugins:read"));
        assert!(!config.authorize(&reader_scopes, "plugins:write"));

        // Test invalid key
        assert!(config.authenticate("invalid-key").is_none());
    }
}
