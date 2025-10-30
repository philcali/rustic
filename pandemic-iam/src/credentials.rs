use anyhow::Result;
use chrono::{DateTime, Utc};
use rustls::pki_types::CertificateDer;
use rustls_pemfile::{certs, private_key};
use serde::{Deserialize, Serialize};
use std::io::BufReader;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwsCredentials {
    #[serde(rename = "AccessKeyId")]
    pub access_key_id: String,
    #[serde(rename = "SecretAccessKey")]
    pub secret_access_key: String,
    #[serde(rename = "Token")]
    pub token: String,
    #[serde(rename = "Expiration")]
    pub expiration: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionToken {
    pub token: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct CredentialManager {
    credentials: Arc<RwLock<Option<AwsCredentials>>>,
    session_tokens: Arc<RwLock<std::collections::HashMap<String, SessionToken>>>,
}

impl CredentialManager {
    pub fn new() -> Self {
        Self {
            credentials: Arc::new(RwLock::new(None)),
            session_tokens: Arc::new(RwLock::new(std::collections::HashMap::new())),
        }
    }

    pub async fn get_credentials(&self) -> Option<AwsCredentials> {
        let creds = self.credentials.read().await;
        creds.clone()
    }

    pub async fn update_credentials(&self, credentials: AwsCredentials) {
        let mut creds = self.credentials.write().await;
        info!(
            "Updated AWS credentials, expires at: {}",
            credentials.expiration
        );
        *creds = Some(credentials);
    }

    pub async fn create_session_token(&self) -> String {
        let token = uuid::Uuid::new_v4().to_string();
        let expires_at = Utc::now() + chrono::Duration::seconds(21600); // 6 hours

        let session_token = SessionToken {
            token: token.clone(),
            expires_at,
        };

        let mut tokens = self.session_tokens.write().await;
        tokens.insert(token.clone(), session_token);

        // Clean up expired tokens
        tokens.retain(|_, v| v.expires_at > Utc::now());

        token
    }

    pub async fn validate_session_token(&self, token: &str) -> bool {
        let tokens = self.session_tokens.read().await;
        if let Some(session_token) = tokens.get(token) {
            session_token.expires_at > Utc::now()
        } else {
            false
        }
    }

    pub async fn needs_refresh(&self) -> bool {
        let creds = self.credentials.read().await;
        match &*creds {
            Some(credentials) => {
                // Refresh if expiring within 5 minutes
                credentials.expiration < Utc::now() + chrono::Duration::minutes(5)
            }
            None => true,
        }
    }

    pub async fn refresh_credentials(&self, config: &crate::config::AwsConfig) -> Result<()> {
        info!("Refreshing credentials via IAM Anywhere");

        match self.get_iam_anywhere_credentials(config).await {
            Ok(credentials) => {
                self.update_credentials(credentials).await;
                Ok(())
            }
            Err(e) => {
                error!("Failed to refresh IAM Anywhere credentials: {}", e);
                Err(e)
            }
        }
    }

    async fn get_iam_anywhere_credentials(
        &self,
        config: &crate::config::AwsConfig,
    ) -> Result<AwsCredentials> {
        info!(
            "Calling IAM Anywhere CreateSession API for profile: {}",
            config.profile_arn
        );

        // Load certificate and private key
        let cert_pem = tokio::fs::read(&config.certificate_path).await?;
        let key_pem = tokio::fs::read(&config.private_key_path).await?;

        let certs = certs(&mut BufReader::new(&cert_pem[..]))
            .collect::<Result<Vec<CertificateDer>, _>>()?;
        let key = private_key(&mut BufReader::new(&key_pem[..]))?
            .ok_or_else(|| anyhow::anyhow!("No private key found"))?;

        if certs.is_empty() {
            return Err(anyhow::anyhow!("No certificates found"));
        }

        // Create TLS client config with client certificate
        let mut client_config = rustls::ClientConfig::builder()
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_client_auth_cert(certs, key)?;
        client_config
            .dangerous()
            .set_certificate_verifier(Arc::new(crate::iam_anywhere::NoVerifier));

        let client = reqwest::Client::builder()
            .use_preconfigured_tls(client_config)
            .build()?;

        // Extract region from trust anchor ARN
        let region = config
            .trust_anchor_arn
            .split(':')
            .nth(3)
            .ok_or_else(|| anyhow::anyhow!("Invalid trust anchor ARN format"))?;
        let url = format!("https://rolesanywhere.{}.amazonaws.com/sessions", region);

        let request_body = serde_json::json!({
            "profileArn": config.profile_arn,
            "roleArn": config.role_arn,
            "trustAnchorArn": config.trust_anchor_arn,
            "durationSeconds": 3600
        });

        let response = client
            .post(&url)
            .header("Content-Type", "application/x-amz-json-1.1")
            .header("X-Amz-Target", "RolesAnywhereService.CreateSession")
            .json(&request_body)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(anyhow::anyhow!("IAM Anywhere API error: {}", error_text));
        }

        let session_response: crate::iam_anywhere::CreateSessionResponse = response.json().await?;

        Ok(AwsCredentials {
            access_key_id: session_response.credential_set.credentials.access_key_id,
            secret_access_key: session_response
                .credential_set
                .credentials
                .secret_access_key,
            token: session_response.credential_set.credentials.session_token,
            expiration: DateTime::parse_from_rfc3339(
                &session_response.credential_set.credentials.expiration,
            )?
            .with_timezone(&Utc),
        })
    }
}
