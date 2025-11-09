use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
    Extension,
};
use pandemic_common::{AgentStatus, DaemonClient};
use pandemic_protocol::{Request, Response as PandemicResponse};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::auth::AuthConfig;

#[derive(Clone)]
pub struct AppState {
    pub socket_path: PathBuf,
    pub auth_config: AuthConfig,
    pub agent_status: Arc<Mutex<AgentStatus>>,
}

pub type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;

pub async fn list_plugins(
    State(state): State<AppState>,
    Extension(scopes): Extension<Vec<String>>,
) -> ApiResult {
    if !state.auth_config.authorize(&scopes, "plugins:read") {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({"status": "error", "message": "Insufficient permissions"})),
        ));
    }

    let request = Request::ListPlugins;
    match DaemonClient::send_request(&state.socket_path, &request).await {
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
                json!({"status": "error", "message": format!("Daemon communication error: {}", e)}),
            ),
        )),
    }
}

pub async fn get_plugin(
    Path(name): Path<String>,
    State(state): State<AppState>,
    Extension(scopes): Extension<Vec<String>>,
) -> ApiResult {
    if !state.auth_config.authorize(&scopes, "plugins:read") {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({"status": "error", "message": "Insufficient permissions"})),
        ));
    }

    let request = Request::GetPlugin { name };
    match DaemonClient::send_request(&state.socket_path, &request).await {
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
                json!({"status": "error", "message": format!("Daemon communication error: {}", e)}),
            ),
        )),
    }
}

pub async fn deregister_plugin(
    Path(name): Path<String>,
    State(state): State<AppState>,
    Extension(scopes): Extension<Vec<String>>,
) -> ApiResult {
    if !state.auth_config.authorize(&scopes, "plugins:write") {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({"status": "error", "message": "Insufficient permissions"})),
        ));
    }

    let request = Request::Deregister { name };
    match DaemonClient::send_request(&state.socket_path, &request).await {
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
                json!({"status": "error", "message": format!("Daemon communication error: {}", e)}),
            ),
        )),
    }
}

pub async fn get_health(
    State(state): State<AppState>,
    Extension(scopes): Extension<Vec<String>>,
) -> ApiResult {
    if !state.auth_config.authorize(&scopes, "health:read") {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({"status": "error", "message": "Insufficient permissions"})),
        ));
    }

    let request = Request::GetHealth;
    match DaemonClient::send_request(&state.socket_path, &request).await {
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
                json!({"status": "error", "message": format!("Daemon communication error: {}", e)}),
            ),
        )),
    }
}

pub async fn get_admin_capabilities(
    State(state): State<AppState>,
    Extension(scopes): Extension<Vec<String>>,
) -> ApiResult {
    if !state.auth_config.authorize(&scopes, "admin") {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({"status": "error", "message": "Insufficient permissions"})),
        ));
    }

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
