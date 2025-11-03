use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IamConfig {
    pub server: ServerConfig,
    pub aws: AwsConfig,
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
    pub session_duration_seconds: Option<i32>,
    pub session_name: Option<String>,
    pub region: Option<String>,
    pub endpoint: Option<String>,
}

impl IamConfig {
    pub async fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = tokio::fs::read_to_string(path).await?;
        let config: IamConfig = toml::from_str(&content)?;

        // Validate required paths exist
        if !Path::new(&config.aws.certificate_path).exists() {
            return Err(anyhow::anyhow!(
                "Certificate file not found: {}",
                config.aws.certificate_path
            ));
        }
        if !Path::new(&config.aws.private_key_path).exists() {
            return Err(anyhow::anyhow!(
                "Private key file not found: {}",
                config.aws.private_key_path
            ));
        }

        Ok(config)
    }
}
