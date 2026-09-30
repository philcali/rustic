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
        let client = AgentClient::new().with_secret(&state.agent_secret);
        let new_status = AgentStatus::refresh(&client).await;
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

#[derive(Deserialize)]
pub struct RemoveQuery {
    /// Also delete the users/groups the deployment's infections created
    /// (`?purge=true`; default leaves them in place — they may be shared).
    #[serde(default)]
    pub purge: bool,
}

pub async fn remove_deployment(
    Path(name): Path<String>,
    State(state): State<AppState>,
    Extension(scopes): Extension<Vec<String>>,
    Query(params): Query<RemoveQuery>,
) -> ApiResult {
    require_scope!(&state.auth_config, &scopes, "admin");

    let request = AgentRequest::RemoveDeployment {
        name,
        purge: params.purge,
    };
    let agent_client = AgentClient::new().with_secret(&state.agent_secret);
    let response = agent_client.send_agent_request(&request);
    format_pandemic_response(response.await)
}

#[derive(Deserialize)]
pub struct AuditQuery {
    /// How many most recent entries to return (oldest → newest).
    #[serde(default = "default_audit_limit")]
    pub limit: usize,
}

fn default_audit_limit() -> usize {
    50
}

/// `GET /api/admin/audit?limit=N` — the host audit log (phase 8): what the
/// agent applied / uninstalled / removed, step by step. The log lives on
/// disk (0600 root), so the agent writes it; REST only reads it back.
pub async fn get_audit(
    State(state): State<AppState>,
    Extension(scopes): Extension<Vec<String>>,
    Query(params): Query<AuditQuery>,
) -> ApiResult {
    require_scope!(&state.auth_config, &scopes, "admin");

    let limit = params.limit.clamp(1, 500);
    match pandemic_common::audit::read_last(limit) {
        Ok(entries) => Ok(Json(json!({
            "status": "success",
            "data": { "entries": entries, "count": entries.len() }
        }))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"status": "error", "message": e.to_string()})),
        )),
    }
}

// ── Epidemic read surface (increment 5b): "see the spread" ────────────────
//
// Read-only views of the coordinator's on-disk state: the spread history
// (`~/.local/state/pandemic/spread-history.log`, written by `epidemic
// spread`) and the roster groups (`~/.config/pandemic/groups.toml`). Both
// are shared `pandemic-common` readers, so the console shows exactly what
// `epidemic spreads` / `epidemic nodes` show.

#[derive(Deserialize)]
pub struct EpidemicSpreadsQuery {
    /// How many most recent spreads to return (newest first).
    #[serde(default = "default_epidemic_spread_limit")]
    pub limit: usize,
}

fn default_epidemic_spread_limit() -> usize {
    50
}

/// Upper bound for `?limit=` so a typo (or a hostile client) can't ask for
/// the whole history in one response.
const EPIDEMIC_SPREAD_LIMIT_CAP: usize = 1000;

/// `GET /api/epidemic/spreads?limit=N` — recent spread history, newest
/// first. Each record is the shared `SpreadRecord` (5a shape): plan
/// identity + hash, targeting, and the per-node ✓/✗ outcomes. A missing
/// history file is an empty list, not an error.
pub async fn get_epidemic_spreads(
    State(state): State<AppState>,
    Extension(scopes): Extension<Vec<String>>,
    Query(params): Query<EpidemicSpreadsQuery>,
) -> ApiResult {
    require_scope!(&state.auth_config, &scopes, "epidemic:read");

    let path = pandemic_common::default_history_path();
    let records = pandemic_common::load_spreads(&path, params.limit.min(EPIDEMIC_SPREAD_LIMIT_CAP));
    match serde_json::to_value(&records) {
        Ok(spreads) => Ok(Json(json!({
            "status": "success",
            "data": { "spreads": spreads, "count": records.len(), "path": path.to_string_lossy() }
        }))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"status": "error", "message": e.to_string()})),
        )),
    }
}

/// `GET /api/epidemic/groups` — the roster groups and their nodes, from
/// `groups.toml`. A missing file means "no named groups" (an empty list).
pub async fn get_epidemic_groups(
    State(state): State<AppState>,
    Extension(scopes): Extension<Vec<String>>,
) -> ApiResult {
    require_scope!(&state.auth_config, &scopes, "epidemic:read");

    let path = pandemic_common::default_groups_path();
    let groups = match pandemic_common::load_groups(&path) {
        Ok(groups) => groups,
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"status": "error", "message": e.to_string()})),
            ))
        }
    };
    Ok(Json(json!({
        "status": "success",
        "data": {
            "groups": groups.iter().map(|g| {
                json!({
                    "name": g.name,
                    "secret_path": g.secret_path,
                    "nodes": g.node.iter().map(|n| json!({ "name": n.name, "addr": n.addr })).collect::<Vec<_>>()
                })
            }).collect::<Vec<_>>(),
            "count": groups.len(),
            "path": path.to_string_lossy()
        }
    })))
}

// ── Epidemic trigger (increment 5c): "start the spread" ─────────────────
//
// `POST /api/epidemic/spread` runs the exact same shared coordinator that
// `epidemic spread` does (`pandemic_common::run_roster_spread` /
// `run_broadcast_spread`), so CLI and console cannot diverge. Live progress
// is published to the daemon event bus on topic `epidemic.spread`, which the
// websocket (`/api/events/stream`) forwards to any subscribed console.

use std::net::Ipv4Addr;

#[derive(Deserialize)]
pub struct EpidemicSpreadPayload {
    /// Registry deployment name (exactly one of `name` or `path` required).
    #[serde(default)]
    name: Option<String>,
    /// Path to a local deployment spec (`deployment.toml`). Exactly one of
    /// `name` or `path` required.
    #[serde(default)]
    path: Option<String>,
    /// Variable overrides (like the CLI `--set`); defaults to the spec's.
    #[serde(default)]
    vars: BTreeMap<String, String>,
    /// Registry URL to use for a by-name spread (overrides the default).
    #[serde(default)]
    registry_url: Option<String>,
    /// Target group name from `groups.toml` (roster path; merged with `nodes`).
    #[serde(default)]
    group: Option<String>,
    /// Ad-hoc node endpoints `host:port` (roster path; merged with `group`).
    #[serde(default)]
    nodes: Vec<String>,
    /// Inline epidemic secret. Falls back to the group's secret file, then
    /// the default secret path, then a generated secret.
    #[serde(default)]
    secret: Option<String>,
    /// Path to a file containing the epidemic secret.
    #[serde(default)]
    secret_path: Option<String>,
    /// Encrypt the coordinator→node hop (roster path). Requires `tls_ca`.
    #[serde(default)]
    tls: bool,
    /// Root CA (PEM file path) for the nodes' TLS certificates.
    #[serde(default)]
    tls_ca: Option<String>,
    /// Server name the nodes' certs must present (default: the node's host).
    #[serde(default)]
    tls_server_name: Option<String>,
    /// Broadcast (multicast) instead of targeting a roster. Mutually
    /// exclusive with `group`/`nodes`.
    #[serde(default)]
    broadcast: bool,
    /// Broadcast targeting criteria, `KEY=VALUE` (AND semantics).
    #[serde(default)]
    criteria: Vec<String>,
    /// Canary cohort: a percentage (`"25"`) or a `KEY=VALUE` criterion.
    #[serde(default)]
    canary: Option<String>,
    /// Promote a previous canary (requires `spread_id`).
    #[serde(default)]
    promote: bool,
    /// A fixed spread id (broadcast only; required for `promote`). Roster
    /// spreads generate their id server-side (same rule as the CLI).
    #[serde(default)]
    spread_id: Option<String>,
    /// Multicast group (default `239.255.77.11`).
    #[serde(default)]
    multicast_group: Option<Ipv4Addr>,
    /// Multicast UDP port (default `7712`).
    #[serde(default)]
    multicast_port: Option<u16>,
    /// Seconds to wait for node callbacks (default `15`).
    #[serde(default)]
    wait: Option<u64>,
    /// Retries per node after a transient failure (roster path; default
    /// [`DEFAULT_APPLY_RETRIES`]). A node verdict is never retried.
    #[serde(default)]
    retries: Option<u32>,
    /// Per-attempt timeout in seconds (roster path; default 30). A node that
    /// never answers is retried after this long.
    #[serde(default)]
    timeout_secs: Option<u64>,
}

/// `POST /api/epidemic/spread` — run a spread (roster or broadcast) as the
/// coordinator and report the full `SpreadRecord` when it finishes.
///
/// Scope: `epidemic:spread` (admin `*` covers it; the default reader role
/// only gets `epidemic:read`). Pre-execution problems are 400; a spread that
/// ran — even with per-node failures — is a *result* (200, `ok: false`);
/// server-side failures (secret file, history append) are 500.
pub async fn trigger_spread(
    State(state): State<AppState>,
    Extension(scopes): Extension<Vec<String>>,
    Json(payload): Json<EpidemicSpreadPayload>,
) -> ApiResult {
    require_scope!(&state.auth_config, &scopes, "epidemic:spread");

    let bad_request = |message: String| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"status": "error", "message": message})),
        )
    };

    // 1. Pre-validation — the same shared rules the CLI enforces.
    let canary = match payload
        .canary
        .as_deref()
        .map(pandemic_common::parse_canary_arg)
        .transpose()
    {
        Ok(c) => c,
        Err(e) => return Err(bad_request(e.to_string())),
    };

    if let Err(e) = pandemic_common::validate_broadcast(
        payload.broadcast,
        payload.group.as_deref(),
        &payload.nodes,
        false, // `--discover` is not part of the REST surface
        &payload.criteria,
        canary.as_ref(),
        payload.promote,
        payload.spread_id.as_deref(),
    ) {
        return Err(bad_request(e.to_string()));
    }

    if payload.tls && payload.tls_ca.is_none() {
        return Err(bad_request(
            "tls is enabled but tls_ca (root CA, PEM file) is required".to_string(),
        ));
    }

    // 2. Resolve the deployment plan (registry name or local spec path).
    let build: anyhow::Result<pandemic_common::DeploymentPlan> = match (payload.name, payload.path)
    {
        (Some(name), _) => {
            let client = match payload.registry_url {
                Some(url) => RegistryClient::with_registry_url(url),
                None => RegistryClient::new(),
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
        Err(e) => return Err(bad_request(e.to_string())),
    };

    // 3. Forward live progress to the daemon event bus (topic epidemic.spread).
    let progress = progress_forwarder(state.socket_path.clone());

    let spread = if payload.broadcast {
        pandemic_common::run_broadcast_spread(&pandemic_common::BroadcastSpread {
            plan: dp,
            secret: pandemic_common::SecretSource::new(
                payload.secret,
                payload.secret_path.map(PathBuf::from),
            ),
            criteria: payload.criteria,
            canary,
            promote: payload.promote,
            spread_id: payload.spread_id,
            multicast_group: payload
                .multicast_group
                .unwrap_or(pandemic_common::DEFAULT_MULTICAST_GROUP),
            multicast_port: payload
                .multicast_port
                .unwrap_or(pandemic_common::DEFAULT_MULTICAST_PORT),
            wait_secs: payload.wait.unwrap_or(15),
            interface: None,
            history_path: None,
            progress: Some(progress),
        })
        .await
    } else {
        let (roster, group_secret_path) =
            match pandemic_common::resolve_roster(payload.group.as_deref(), &payload.nodes) {
                Ok(r) => r,
                Err(e) => return Err(bad_request(e.to_string())),
            };
        if roster.is_empty() {
            return Err(bad_request(
                "no nodes to spread to — pass 'nodes' (host:port) or a 'group'".to_string(),
            ));
        }
        pandemic_common::run_roster_spread(&pandemic_common::RosterSpread {
            roster,
            group: payload.group,
            group_secret_path,
            plan: dp,
            secret: pandemic_common::SecretSource::new(
                payload.secret,
                payload.secret_path.map(PathBuf::from),
            ),
            tls: pandemic_common::TlsOptions {
                enabled: payload.tls,
                ca: payload.tls_ca.map(PathBuf::from),
                server_name: payload.tls_server_name,
            },
            // Retries + per-attempt timeout (4b); defaults mirror the CLI.
            retries: payload
                .retries
                .unwrap_or(pandemic_common::DEFAULT_APPLY_RETRIES),
            timeout: std::time::Duration::from_secs(
                payload
                    .timeout_secs
                    .unwrap_or(pandemic_common::DEFAULT_APPLY_TIMEOUT.as_secs()),
            ),
            // The roster id is generated server-side (shared rule:
            // `--spread-id` is a broadcast flag); the console follows the
            // live `started` event instead.
            spread_id: None,
            history_path: None,
            progress: Some(progress),
        })
        .await
    };

    let result = match spread {
        Ok(r) => r,
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"status": "error", "message": e.to_string()})),
            ))
        }
    };

    Ok(Json(json!({
        "status": "success",
        "data": {
            "spread": result.record,
            "secret_generated": result.generated_secret.is_some(),
            "generated_secret": result.generated_secret
        }
    })))
}

/// Spawn a task that relays each [`SpreadProgress`](pandemic_protocol::SpreadProgress)
/// event to the daemon event bus on topic `epidemic.spread`; the console
/// picks the events up over `/api/events/stream`. Returns the sending half.
///
/// A dropped receiver (spread finished first) or an unreachable daemon never
/// fails the spread — progress is best-effight observation.
fn progress_forwarder(
    socket_path: PathBuf,
) -> tokio::sync::mpsc::UnboundedSender<pandemic_protocol::SpreadProgress> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Some(progress) = rx.recv().await {
            let event = match serde_json::to_value(&progress) {
                Ok(event) => event,
                Err(e) => {
                    tracing::warn!("epidemic: dropping progress event (encoding: {e})");
                    continue;
                }
            };
            let request = Request::Publish {
                topic: "epidemic.spread".to_string(),
                data: json!({"spread_progress": event}),
            };
            if let Err(e) = DaemonClient::send_request(&socket_path, &request).await {
                tracing::warn!("epidemic: failed to publish progress event: {e}");
            }
        }
    });
    tx
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
    /// When true, return the resolved plan without applying — redacted:
    /// names, targets, owners, modes, content hashes; never values, file
    /// contents, or the health command.
    #[serde(default)]
    dry_run: bool,
}

/// Attach the agent's per-infection diff to the dry-run payload (phase 8):
/// each `data.infections[i]` gets a `diff` object, matched by name.
fn merge_diff_into_preview(data: &mut Value, diff: &Value) {
    let Some(diff_infections) = diff.get("infections").and_then(Value::as_array) else {
        return;
    };
    let Some(infections) = data.get_mut("infections").and_then(Value::as_array_mut) else {
        return;
    };
    for infection in infections {
        let Some(name) = infection.get("name").and_then(Value::as_str) else {
            continue;
        };
        if let Some(matched) = diff_infections
            .iter()
            .find(|d| d.get("name").and_then(Value::as_str) == Some(name))
        {
            infection["diff"] = matched.clone();
        }
    }
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

    let build: anyhow::Result<pandemic_common::DeploymentPlan> = match (payload.name, payload.path)
    {
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

    // Dry-run: return the *redacted* plan (phase 8). Variable values,
    // rendered file/unit contents, and the health-check command never leave
    // the host — only names, targets, owners, modes, and content hashes.
    if payload.dry_run {
        let mut data = pandemic_common::deployment_preview_data(&dp);
        // Best-effort host diff (phase 8): what applying would change.
        // Omitted when the agent is unreachable, so an offline dry-run
        // (reviewing a spec with no host) still succeeds.
        let agent_client = AgentClient::new().with_secret(&state.agent_secret);
        let request = AgentRequest::PreviewDeployment {
            infections: pandemic_common::deployment_apply_infections(&dp),
        };
        if let Ok(PandemicResponse::Success { data: Some(diff) }) =
            agent_client.send_agent_request(&request).await
        {
            merge_diff_into_preview(&mut data, &diff);
        }
        return Ok(Json(json!({ "status": "success", "data": data })));
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

#[cfg(test)]
mod merge_diff_tests {
    use super::merge_diff_into_preview;
    use serde_json::json;

    #[test]
    fn merges_matching_infections_by_name() {
        let mut data = json!({
            "name": "web-tier",
            "infections": [
                { "name": "alpha", "order": 1 },
                { "name": "beta", "order": 2 }
            ]
        });
        let diff = json!({
            "infections": [
                { "name": "beta", "files": [] },
                { "name": "alpha", "files": [] }
            ]
        });
        merge_diff_into_preview(&mut data, &diff);
        let infections = data["infections"].as_array().unwrap();
        assert!(infections[0]["diff"]["files"].is_array());
        assert_eq!(infections[0]["diff"]["files"].as_array().unwrap().len(), 0);
        assert!(infections[1]["diff"]["files"].is_array());
    }

    #[test]
    fn unmatched_infections_and_malformed_shapes_are_noops() {
        let mut data = json!({ "infections": [ { "name": "alpha" } ] });
        // diff names don't match -> no diff attached
        merge_diff_into_preview(&mut data, &json!({ "infections": [ { "name": "other" } ] }));
        assert!(data["infections"][0].get("diff").is_none());
        // malformed diff -> untouched
        let mut data = json!({ "infections": [ { "name": "alpha" } ] });
        merge_diff_into_preview(&mut data, &json!({ "not_infections": [] }));
        assert!(data["infections"][0].get("diff").is_none());
        // malformed data -> no panic
        let mut data = json!({ "infections": "scalar" });
        merge_diff_into_preview(&mut data, &json!({ "infections": [] }));
        assert_eq!(data["infections"], "scalar");
    }
}

#[cfg(test)]
mod epidemic_tests {
    use super::*;

    #[test]
    fn spreads_query_defaults_limit_and_caps_it() {
        let q: EpidemicSpreadsQuery = serde_json::from_str("{}").unwrap();
        assert_eq!(q.limit, 50);

        let q: EpidemicSpreadsQuery = serde_json::from_str(r#"{"limit": 7}"#).unwrap();
        assert_eq!(q.limit, 7);
        assert_eq!(q.limit.min(EPIDEMIC_SPREAD_LIMIT_CAP), 7);

        // A huge limit is capped, not honored verbatim.
        let q: EpidemicSpreadsQuery = serde_json::from_str(r#"{"limit": 999999}"#).unwrap();
        assert_eq!(
            q.limit.min(EPIDEMIC_SPREAD_LIMIT_CAP),
            EPIDEMIC_SPREAD_LIMIT_CAP
        );
    }

    #[test]
    fn spread_payload_defaults_for_a_minimal_roster_trigger() {
        let p: EpidemicSpreadPayload = serde_json::from_str(r#"{"name": "web"}"#).unwrap();
        assert_eq!(p.name.as_deref(), Some("web"));
        assert!(p.path.is_none());
        assert!(p.vars.is_empty());
        assert!(p.group.is_none() && p.nodes.is_empty());
        assert!(p.secret.is_none() && p.secret_path.is_none());
        assert!(!p.tls && p.tls_ca.is_none());
        assert!(!p.broadcast && p.criteria.is_empty());
        assert!(p.canary.is_none() && !p.promote && p.spread_id.is_none());
        assert!(p.multicast_group.is_none() && p.multicast_port.is_none());
        // `wait: None` means "use the handler default (15s)" — the handler
        // applies `payload.wait.unwrap_or(15)`.
        assert_eq!(p.wait, None);
        // `retries`/`timeout_secs: None` means "use the coordinator defaults"
        // — the handler applies `.unwrap_or(DEFAULT_APPLY_RETRIES)` /
        // `.unwrap_or(DEFAULT_APPLY_TIMEOUT)`.
        assert_eq!(p.retries, None);
        assert_eq!(p.timeout_secs, None);
    }

    #[test]
    fn spread_payload_parses_full_broadcast_options() {
        let p: EpidemicSpreadPayload = serde_json::from_str(
            r#"{
                "name": "web",
                "vars": {"REPLICAS": "3"},
                "registry_url": "http://registry:8080",
                "broadcast": true,
                "criteria": ["tier=web", "zone=us"],
                "canary": "25",
                "multicast_group": "239.255.77.99",
                "multicast_port": 7799,
                "wait": 30
            }"#,
        )
        .unwrap();
        assert!(p.broadcast);
        assert_eq!(p.criteria, vec!["tier=web", "zone=us"]);
        assert_eq!(p.canary.as_deref(), Some("25"));
        assert_eq!(
            p.multicast_group,
            Some(std::net::Ipv4Addr::new(239, 255, 77, 99))
        );
        assert_eq!(p.multicast_port, Some(7799));
        assert_eq!(p.wait, Some(30));

        // And a roster-style payload with secrets + TLS:
        let p: EpidemicSpreadPayload = serde_json::from_str(
            r#"{
                "path": "deploy.toml",
                "group": "prod",
                "nodes": ["10.0.0.5:7711", "10.0.0.6:7711"],
                "secret_path": "/etc/pandemic/epidemic.key",
                "tls": true,
                "tls_ca": "/etc/pandemic/ca.pem",
                "tls_server_name": "node-a",
                "retries": 5,
                "timeout_secs": 10
            }"#,
        )
        .unwrap();
        assert_eq!(p.group.as_deref(), Some("prod"));
        assert_eq!(p.nodes.len(), 2);
        assert!(p.tls);
        assert_eq!(p.tls_server_name.as_deref(), Some("node-a"));
        assert_eq!(p.retries, Some(5));
        assert_eq!(p.timeout_secs, Some(10));
    }
}
