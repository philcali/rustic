use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IamConfig {
    pub server: ServerConfig,
    pub aws: AwsConfig,
    pub roles: HashMap<String, String>, // role_name -> arn
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub bind_address: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwsConfig {
    pub certificate_path: String,
    pub private_key_path: String,
    pub trust_anchor_arn: String,
    pub profile_arn: String,
    pub role_arn: String,
    pub session_duration_seconds: Option<u32>,
}

impl IamConfig {
    pub async fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = tokio::fs::read_to_string(path).await?;
        let config: IamConfig = toml::from_str(&content)?;
        Ok(config)
    }
}
