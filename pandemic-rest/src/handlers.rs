use anyhow::Error;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
    Extension,
};
use pandemic_common::{AgentClient, AgentStatus, DaemonClient, RegistryClient};
use pandemic_protocol::{
    AgentRequest, Request, Response as PandemicResponse, ServiceOverrides, UserConfig,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
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
    pub agent_secret: String,
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
    let agent_client = AgentClient::new().with_secret(&state.agent_secret);
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

    let agent_client = AgentClient::new().with_secret(&state.agent_secret);
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

    let agent_client = AgentClient::new().with_secret(&state.agent_secret);
    let response = agent_client.send_agent_request(&request);
    format_pandemic_response(response.await)
}

// User management handlers
pub async fn list_users(
    State(state): State<AppState>,
    Extension(scopes): Extension<Vec<String>>,
) -> ApiResult {
    require_scope!(&state.auth_config, &scopes, "admin");

    let request = AgentRequest::ListUsers;
    let agent_client = AgentClient::new().with_secret(&state.agent_secret);
    let response = agent_client.send_agent_request(&request);
    format_pandemic_response(response.await)
}

pub async fn create_user(
    State(state): State<AppState>,
    Extension(scopes): Extension<Vec<String>>,
    Json(payload): Json<CreateUserPayload>,
) -> ApiResult {
    require_scope!(&state.auth_config, &scopes, "admin");

    let request = AgentRequest::UserCreate {
        username: payload.username,
        config: payload.config,
    };
    let agent_client = AgentClient::new().with_secret(&state.agent_secret);
    let response = agent_client.send_agent_request(&request);
    format_pandemic_response(response.await)
}

#[derive(serde::Deserialize)]
pub struct CreateUserPayload {
    username: String,
    config: UserConfig,
}

pub async fn delete_user(
    State(state): State<AppState>,
    Path(username): Path<String>,
    Extension(scopes): Extension<Vec<String>>,
) -> ApiResult {
    require_scope!(&state.auth_config, &scopes, "admin");

    let request = AgentRequest::UserDelete { username };
    let agent_client = AgentClient::new().with_secret(&state.agent_secret);
    let response = agent_client.send_agent_request(&request);
    format_pandemic_response(response.await)
}

pub async fn modify_user(
    State(state): State<AppState>,
    Path(username): Path<String>,
    Extension(scopes): Extension<Vec<String>>,
    Json(config): Json<UserConfig>,
) -> ApiResult {
    require_scope!(&state.auth_config, &scopes, "admin");

    let request = AgentRequest::UserModify { username, config };
    let agent_client = AgentClient::new().with_secret(&state.agent_secret);
    let response = agent_client.send_agent_request(&request);
    format_pandemic_response(response.await)
}

// Group management handlers
pub async fn list_groups(
    State(state): State<AppState>,
    Extension(scopes): Extension<Vec<String>>,
) -> ApiResult {
    require_scope!(&state.auth_config, &scopes, "admin");

    let request = AgentRequest::ListGroups;
    let agent_client = AgentClient::new().with_secret(&state.agent_secret);
    let response = agent_client.send_agent_request(&request);
    format_pandemic_response(response.await)
}

pub async fn create_group(
    State(state): State<AppState>,
    Path(groupname): Path<String>,
    Extension(scopes): Extension<Vec<String>>,
) -> ApiResult {
    require_scope!(&state.auth_config, &scopes, "admin");

    let request = AgentRequest::GroupCreate { groupname };
    let agent_client = AgentClient::new().with_secret(&state.agent_secret);
    let response = agent_client.send_agent_request(&request);
    format_pandemic_response(response.await)
}

pub async fn delete_group(
    State(state): State<AppState>,
    Path(groupname): Path<String>,
    Extension(scopes): Extension<Vec<String>>,
) -> ApiResult {
    require_scope!(&state.auth_config, &scopes, "admin");

    let request = AgentRequest::GroupDelete { groupname };
    let agent_client = AgentClient::new().with_secret(&state.agent_secret);
    let response = agent_client.send_agent_request(&request);
    format_pandemic_response(response.await)
}

pub async fn add_user_to_group(
    State(state): State<AppState>,
    Path((groupname, username)): Path<(String, String)>,
    Extension(scopes): Extension<Vec<String>>,
) -> ApiResult {
    require_scope!(&state.auth_config, &scopes, "admin");

    let request = AgentRequest::GroupAddUser {
        groupname,
        username,
    };
    let agent_client = AgentClient::new().with_secret(&state.agent_secret);
    let response = agent_client.send_agent_request(&request);
    format_pandemic_response(response.await)
}

pub async fn remove_user_from_group(
    State(state): State<AppState>,
    Path((groupname, username)): Path<(String, String)>,
    Extension(scopes): Extension<Vec<String>>,
) -> ApiResult {
    require_scope!(&state.auth_config, &scopes, "admin");

    let request = AgentRequest::GroupRemoveUser {
        groupname,
        username,
    };
    let agent_client = AgentClient::new().with_secret(&state.agent_secret);
    let response = agent_client.send_agent_request(&request);
    format_pandemic_response(response.await)
}

// Service configuration handlers
pub async fn get_service_config(
    State(state): State<AppState>,
    Path(service): Path<String>,
    Extension(scopes): Extension<Vec<String>>,
) -> ApiResult {
    require_scope!(&state.auth_config, &scopes, "admin");

    let request = AgentRequest::GetServiceConfig { service };
    let agent_client = AgentClient::new().with_secret(&state.agent_secret);
    let response = agent_client.send_agent_request(&request);
    format_pandemic_response(response.await)
}

pub async fn set_service_config(
    State(state): State<AppState>,
    Path(service): Path<String>,
    Extension(scopes): Extension<Vec<String>>,
    Json(overrides): Json<ServiceOverrides>,
) -> ApiResult {
    require_scope!(&state.auth_config, &scopes, "admin");

    let request = AgentRequest::ServiceConfigOverride { service, overrides };
    let agent_client = AgentClient::new().with_secret(&state.agent_secret);
    let response = agent_client.send_agent_request(&request);
    format_pandemic_response(response.await)
}

pub async fn reset_service_config(
    State(state): State<AppState>,
    Path(service): Path<String>,
    Extension(scopes): Extension<Vec<String>>,
) -> ApiResult {
    require_scope!(&state.auth_config, &scopes, "admin");

    let request = AgentRequest::ServiceConfigReset { service };
    let agent_client = AgentClient::new().with_secret(&state.agent_secret);
    let response = agent_client.send_agent_request(&request);
    format_pandemic_response(response.await)
}
// Registry handlers
//
// `find` is a pure, read-only search and is consistent with the CLI
// (`pandemic-cli registry find`): it resolves client-side against the registry
// directly rather than round-tripping through the agent, which adds no value
// for a lookup. `?registry_url=` mirrors the CLI `--registry-url` flag.
pub async fn find_infections(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    Extension(scopes): Extension<Vec<String>>,
) -> ApiResult {
    require_scope!(&state.auth_config, &scopes, "admin");

    let query = params.get("q").unwrap_or(&String::new()).clone();
    let client = match params.get("registry_url") {
        Some(url) => RegistryClient::with_registry_url(url.clone()),
        None => RegistryClient::new(),
    };
    // `search_infections` swallows fetch errors and returns an empty list when
    // the registry is unreachable, so this stays a 200 (possibly-empty) result.
    let infections = client.search_infections(&query).await.unwrap_or_default();
    Ok(Json(json!({
        "status": "success",
        "data": {
            "infections": infections
        }
    })))
}

pub async fn get_infection_manifest(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Extension(scopes): Extension<Vec<String>>,
) -> ApiResult {
    require_scope!(&state.auth_config, &scopes, "admin");

    let request = AgentRequest::GetInfectionManifest { name };
    let agent_client = AgentClient::new().with_secret(&state.agent_secret);
    let response = agent_client.send_agent_request(&request);
    format_pandemic_response(response.await)
}

#[derive(serde::Deserialize)]
pub struct InstallPayload {
    target_path: Option<String>,
}

pub async fn install_infection(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Extension(scopes): Extension<Vec<String>>,
    Json(payload): Json<InstallPayload>,
) -> ApiResult {
    require_scope!(&state.auth_config, &scopes, "admin");

    let request = AgentRequest::InstallInfection {
        name,
        target_path: payload.target_path,
    };
    let agent_client = AgentClient::new().with_secret(&state.agent_secret);
    let response = agent_client.send_agent_request(&request);
    format_pandemic_response(response.await)
}

// Deployment lifecycle handlers (ideas/deployments.md, phase 4).
//
// The pure `Plan` step (resolve + render + validate) lives in the shared
// `pandemic_common::apply` builder, so the REST API drives the identical
// plan the CLI does; the agent runs the privileged `Apply` step.

pub async fn list_deployments(
    State(state): State<AppState>,
    Extension(scopes): Extension<Vec<String>>,
) -> ApiResult {
    require_scope!(&state.auth_config, &scopes, "admin");

    let request = AgentRequest::ListDeployments;
    let agent_client = AgentClient::new().with_secret(&state.agent_secret);
    let response = agent_client.send_agent_request(&request);
    format_pandemic_response(response.await)
}

pub async fn get_deployment(
    Path(name): Path<String>,
    State(state): State<AppState>,
    Extension(scopes): Extension<Vec<String>>,
) -> ApiResult {
    require_scope!(&state.auth_config, &scopes, "admin");

    let request = AgentRequest::GetDeploymentStatus { name };
    let agent_client = AgentClient::new().with_secret(&state.agent_secret);
    let response = agent_client.send_agent_request(&request);
    format_pandemic_response(response.await)
}

pub async fn remove_deployment(
    Path(name): Path<String>,
    State(state): State<AppState>,
    Extension(scopes): Extension<Vec<String>>,
) -> ApiResult {
    require_scope!(&state.auth_config, &scopes, "admin");

    let request = AgentRequest::RemoveDeployment { name };
    let agent_client = AgentClient::new().with_secret(&state.agent_secret);
    let response = agent_client.send_agent_request(&request);
    format_pandemic_response(response.await)
}

#[derive(Deserialize)]
pub struct DeploymentInstallPayload {
    /// Registry deployment name (by-name install; its infection-spec atoms are
    /// pulled from the same registry). Mutually exclusive with `path`.
    #[serde(default)]
    name: Option<String>,
    /// Path to a local deployment spec (`deployment.toml`). Mutually exclusive
    /// with `name`.
    #[serde(default)]
    path: Option<String>,
    /// Variable overrides (like the CLI `--set`); defaults to the spec's.
    #[serde(default)]
    vars: BTreeMap<String, String>,
    /// Registry URL to use for a by-name install (overrides the default).
    #[serde(default)]
    registry_url: Option<String>,
    /// When true, return the resolved + rendered plan without applying.
    #[serde(default)]
    dry_run: bool,
}

/// `POST /api/admin/deployments` — the Plan/Apply consolidation.
///
/// Builds the concrete deployment plan (resolve + render + validate) from
/// either a registry `name` (fetch + sha256-verify + extract) or a local spec
/// `path` (offline), then either returns it (dry-run) or hands the concrete
/// plans to the agent to apply: ownership pre-flight, each infection in
/// `order`, then the record.
pub async fn install_deployment(
    State(state): State<AppState>,
    Extension(scopes): Extension<Vec<String>>,
    Json(payload): Json<DeploymentInstallPayload>,
) -> ApiResult {
    require_scope!(&state.auth_config, &scopes, "admin");

    let build: anyhow::Result<pandemic_common::DeploymentPlan> = match (
        payload.name,
        payload.path,
    ) {
        (Some(name), _) => {
            let client = match payload.registry_url {
                Some(url) => pandemic_common::RegistryClient::with_registry_url(url),
                None => pandemic_common::RegistryClient::new(),
            };
            pandemic_common::resolve_deployment_target(&client, &name, &payload.vars).await
        }
        (None, Some(path)) => {
            pandemic_common::build_deployment_plan(std::path::Path::new(&path), &payload.vars)
        }
        (None, None) => Err(anyhow::anyhow!(
            "provide either a registry 'name' or a local spec 'path'"
        )),
    };

    let dp = match build {
        Ok(dp) => dp,
        Err(e) => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"status": "error", "message": e.to_string()})),
            ))
        }
    };

    if payload.dry_run {
        let infections = dp
            .infections
            .iter()
            .map(|r| {
                json!({
                    "name": r.name,
                    "order": r.order,
                    "source": r.source,
                    "version": r.plan.version,
                    "plan": r.plan,
                })
            })
            .collect::<Vec<_>>();
        return Ok(Json(json!({
            "status": "success",
            "data": {
                "name": dp.spec.meta.name,
                "version": dp.spec.meta.version,
                "shared_variables": dp.shared,
                "infections": infections,
            }
        })));
    }

    let request = AgentRequest::ApplyDeployment {
        name: dp.spec.meta.name.clone(),
        version: dp.spec.meta.version.clone(),
        variables: dp.shared.clone(),
        infections: pandemic_common::deployment_apply_infections(&dp),
    };
    let agent_client = AgentClient::new().with_secret(&state.agent_secret);
    let response = agent_client.send_agent_request(&request);
    format_pandemic_response(response.await)
}
