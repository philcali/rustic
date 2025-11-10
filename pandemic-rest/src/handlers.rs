use anyhow::Error;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
    Extension,
};
use pandemic_common::{AgentClient, AgentStatus, DaemonClient};
use pandemic_protocol::{AgentRequest, Request, Response as PandemicResponse};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::auth::AuthConfig;

macro_rules! require_scope {
    ($auth_config:expr, $scopes:expr, $required:expr) => {
        if !$auth_config.authorize($scopes, $required) {
            return Err((
                StatusCode::FORBIDDEN,
                Json(json!({"status": "error", "message": "Insufficient permissions"})),
            ));
        }
    };
}

#[derive(Clone)]
pub struct AppState {
    pub socket_path: PathBuf,
    pub auth_config: AuthConfig,
    pub agent_status: Arc<Mutex<AgentStatus>>,
}

pub type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;

fn format_pandemic_response(result: Result<PandemicResponse, Error>) -> ApiResult {
    match result {
        Ok(PandemicResponse::Success { data }) => {
            Ok(Json(json!({"status": "success", "data": data})))
        }
        Ok(PandemicResponse::Error { message }) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"status": "error", "message": message})),
        )),
        Ok(PandemicResponse::NotFound { message }) => Err((
            StatusCode::NOT_FOUND,
            Json(json!({"status": "not_found", "message": message})),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(
                json!({"status": "error", "message": format!("Socket communication error: {}", e)}),
            ),
        )),
    }
}

pub async fn list_plugins(
    State(state): State<AppState>,
    Extension(scopes): Extension<Vec<String>>,
) -> ApiResult {
    require_scope!(&state.auth_config, &scopes, "plugins:read");

    let request = Request::ListPlugins;
    let response = DaemonClient::send_request(&state.socket_path, &request);
    format_pandemic_response(response.await)
}

pub async fn get_plugin(
    Path(name): Path<String>,
    State(state): State<AppState>,
    Extension(scopes): Extension<Vec<String>>,
) -> ApiResult {
    require_scope!(&state.auth_config, &scopes, "plugins:read");

    let request = Request::GetPlugin { name };
    let response = DaemonClient::send_request(&state.socket_path, &request);
    format_pandemic_response(response.await)
}

pub async fn deregister_plugin(
    Path(name): Path<String>,
    State(state): State<AppState>,
    Extension(scopes): Extension<Vec<String>>,
) -> ApiResult {
    require_scope!(&state.auth_config, &scopes, "plugins:write");

    let request = Request::Deregister { name };
    let response = DaemonClient::send_request(&state.socket_path, &request);
    format_pandemic_response(response.await)
}

pub async fn get_health(
    State(state): State<AppState>,
    Extension(scopes): Extension<Vec<String>>,
) -> ApiResult {
    require_scope!(&state.auth_config, &scopes, "health:read");

    let request = Request::GetHealth;
    let response = DaemonClient::send_request(&state.socket_path, &request);
    format_pandemic_response(response.await)
}

pub async fn get_admin_capabilities(
    State(state): State<AppState>,
    Extension(scopes): Extension<Vec<String>>,
) -> ApiResult {
    require_scope!(&state.auth_config, &scopes, "admin");

    let needs_refresh = {
        let agent_status = state.agent_status.lock().unwrap();
        agent_status.is_stale()
    };

    if needs_refresh {
        let new_status = AgentStatus::refresh().await;
        let mut agent_status = state.agent_status.lock().unwrap();
        *agent_status = new_status;
    }

    let (available, capabilities) = {
        let agent_status = state.agent_status.lock().unwrap();
        (agent_status.available, agent_status.capabilities.clone())
    };

    Ok(Json(json!({
        "status": "success",
        "data": {
            "agent_available": available,
            "capabilities": capabilities
        }
    })))
}

pub async fn list_system_services(
    State(state): State<AppState>,
    Extension(scopes): Extension<Vec<String>>,
) -> ApiResult {
    require_scope!(&state.auth_config, &scopes, "admin");

    let request = AgentRequest::ListServices;
    let agent_client = AgentClient::default();
    let response = agent_client.send_agent_request(&request);
    format_pandemic_response(response.await)
}

pub async fn get_system_service(
    Path(name): Path<String>,
    State(state): State<AppState>,
    Extension(scopes): Extension<Vec<String>>,
) -> ApiResult {
    require_scope!(&state.auth_config, &scopes, "admin");

    let request = AgentRequest::SystemdControl {
        action: "status".to_string(),
        service: name,
    };

    let agent_client = AgentClient::default();
    let response = agent_client.send_agent_request(&request);
    format_pandemic_response(response.await)
}

#[derive(Deserialize)]
pub struct ServiceAction {
    action: String,
}

pub async fn control_system_service(
    Path(name): Path<String>,
    State(state): State<AppState>,
    Extension(scopes): Extension<Vec<String>>,
    Json(payload): Json<ServiceAction>,
) -> ApiResult {
    require_scope!(&state.auth_config, &scopes, "admin");

    let request = AgentRequest::SystemdControl {
        action: payload.action,
        service: name,
    };

    let agent_client = AgentClient::default();
    let response = agent_client.send_agent_request(&request);
    format_pandemic_response(response.await)
}
