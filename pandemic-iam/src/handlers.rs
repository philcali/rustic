use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::json;
use tracing::{info, warn};

use crate::{config::IamConfig, credentials::CredentialManager};

#[derive(Clone)]
pub struct AppState {
    pub config: IamConfig,
    pub credential_manager: CredentialManager,
}

// IMDSv2 Token endpoint
pub async fn get_token(State(state): State<AppState>) -> Response {
    let token = state.credential_manager.create_session_token().await;

    (StatusCode::OK, [("Content-Type", "text/plain")], token).into_response()
}

// List available roles
pub async fn list_roles(headers: HeaderMap, State(state): State<AppState>) -> Response {
    if !validate_token(&headers, &state).await {
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }

    let parts = state.config.aws.role_arn.split("/");
    let role_name = parts.last().unwrap_or("");

    (
        StatusCode::OK,
        [("Content-Type", "text/plain")],
        role_name.to_string(),
    )
        .into_response()
}

// Get credentials for a specific role
pub async fn get_role_credentials(
    Path(role_name): Path<String>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Response {
    if !validate_token(&headers, &state).await {
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }

    // Check if role exists
    let configured_role = state.config.aws.role_arn.split("/").last().unwrap_or("");
    if role_name != configured_role {
        return (StatusCode::NOT_FOUND, "Role not found").into_response();
    }

    // Get current credentials
    match state.credential_manager.get_credentials().await {
        Some(credentials) => {
            info!("Serving credentials for role: {}", role_name);

            let response = json!({
                "Code": "Success",
                "LastUpdated": chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
                "Type": "AWS-HMAC",
                "AccessKeyId": credentials.access_key_id,
                "SecretAccessKey": credentials.secret_access_key,
                "Token": credentials.token,
                "Expiration": credentials.expiration.format("%Y-%m-%dT%H:%M:%SZ").to_string()
            });

            (
                StatusCode::OK,
                [("Content-Type", "application/json")],
                response.to_string(),
            )
                .into_response()
        }
        None => {
            warn!("No credentials available for role: {}", role_name);
            (StatusCode::SERVICE_UNAVAILABLE, "Credentials not available").into_response()
        }
    }
}

// Health check endpoint
pub async fn health_check() -> Response {
    (
        StatusCode::OK,
        [("Content-Type", "application/json")],
        json!({"status": "healthy"}).to_string(),
    )
        .into_response()
}

async fn validate_token(headers: &HeaderMap, state: &AppState) -> bool {
    if let Some(token_header) = headers.get("X-aws-ec2-metadata-token") {
        if let Ok(token) = token_header.to_str() {
            return state.credential_manager.validate_session_token(token).await;
        }
    }
    false
}
