use std::sync::Arc;
use tokio::sync::RwLock;

use crate::iam_anywhere::{CreateSessionRequest, CreateSessionResponse};
use crate::signer::FileSigner;
use crate::signing::{sign_request, SigningParams};
use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};
use tracing::{error, info};
use uuid::Uuid;

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
        // Load signer
        let signer = FileSigner::new(&config.certificate_path, &config.private_key_path)?;

        // Extract region from trust anchor ARN if not provided
        let region = config
            .region
            .clone()
            .or(extract_region_from_arn(&config.trust_anchor_arn))
            .unwrap_or_else(|| "us-east-1".to_string());

        // Build endpoint URL
        let endpoint = config
            .endpoint
            .clone()
            .unwrap_or(format!("https://rolesanywhere.{}.amazonaws.com", region));

        // Build URL with query parameters
        let mut url = format!("{}/sessions", endpoint);
        let params = [
            ("profileArn", &config.profile_arn),
            ("roleArn", &config.role_arn),
            ("trustAnchorArn", &config.trust_anchor_arn),
        ];

        let query_string = params
            .iter()
            .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&");

        url.push('?');
        url.push_str(&query_string);

        // Create request payload (only cert and duration)
        let request = CreateSessionRequest {
            duration_seconds: config.session_duration_seconds.unwrap_or(3600),
            role_session_name: config.session_name.clone(),
        };

        // Create signed request
        let client = reqwest::Client::new();
        let body = serde_json::to_string(&request)?;

        // Set up signing parameters
        let signing_params = SigningParams::new(region.clone());
        let mut headers = HeaderMap::new();
        headers.insert(
            "amz-sdk-invocation-id",
            Uuid::new_v4().to_string().parse().unwrap(),
        );
        headers.insert("amz-sdk-request", "attempt=1; max=3".parse().unwrap());
        headers.insert("content-type", "application/json".parse().unwrap());

        // Sign the request
        let serial_number = signer.get_serial_number()?;
        sign_request(
            "POST",
            &url,
            &mut headers,
            &body,
            &signing_params,
            &signer.certificate_base64(),
            &serial_number,
            &signer,
        )?;

        let response = client.post(&url).headers(headers).body(body).send().await?;

        if !response.status().is_success() {
            return Err(anyhow!("Request failed with status: {}", response.status()));
        }

        let session_response: CreateSessionResponse = response.json().await?;

        if session_response.credential_set.is_empty() {
            return Err(anyhow!("No credentials returned from CreateSession"));
        }

        let credentials = &session_response.credential_set[0].credentials;

        Ok(AwsCredentials {
            access_key_id: credentials.access_key_id.clone(),
            secret_access_key: credentials.secret_access_key.clone(),
            token: credentials.session_token.clone(),
            expiration: DateTime::parse_from_rfc3339(&credentials.expiration)?.with_timezone(&Utc),
        })
    }
}

fn extract_region_from_arn(arn: &str) -> Option<String> {
    // ARN format: arn:aws:rolesanywhere:region:account:trust-anchor/id
    let parts: Vec<&str> = arn.split(':').collect();
    if parts.len() >= 4 {
        Some(parts[3].to_string())
    } else {
        None
    }
}
