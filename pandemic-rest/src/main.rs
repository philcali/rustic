mod auth;
mod events;
mod handlers;
mod middleware;
mod websocket;

use anyhow::Result;
use axum::{
    middleware::from_fn_with_state,
    routing::{delete, get, post},
    Router,
};
use clap::Parser;
use pandemic_common::{AgentStatus, DaemonClient};
use pandemic_protocol::{PluginInfo, Request};
use std::collections::HashMap;
use std::path::PathBuf;
use tower::ServiceBuilder;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::{error, info};

use auth::AuthConfig;
use events::publish_event;
use handlers::{
    add_user_to_group, control_system_service, create_group, create_user, delete_group,
    delete_user, deregister_plugin, get_admin_capabilities, get_health, get_plugin,
    get_service_config, get_system_service, list_groups, list_plugins, list_system_services,
    list_users, modify_user, remove_user_from_group, reset_service_config, set_service_config,
    AppState,
};
use middleware::auth_middleware;
use std::sync::{Arc, Mutex};
use websocket::websocket_handler;

#[derive(Parser)]
#[command(name = "pandemic-rest")]
#[command(about = "REST API server infection for pandemic daemon")]
struct Args {
    #[arg(long, default_value = "/var/run/pandemic/pandemic.sock")]
    socket_path: PathBuf,

    #[arg(long, default_value = "127.0.0.1")]
    bind_address: String,

    #[arg(long, default_value = "8080")]
    port: u16,

    #[arg(long, default_value = "/etc/pandemic/rest-auth.toml")]
    auth_config: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    // Load authentication configuration
    let auth_config = match AuthConfig::load(&args.auth_config).await {
        Ok(config) => {
            info!("Loaded auth config from {:?}", args.auth_config);
            config
        }
        Err(e) => {
            error!("Failed to load auth config: {}", e);
            info!("Creating default auth config...");
            create_default_auth_config(&args.auth_config).await?;
            AuthConfig::load(&args.auth_config).await?
        }
    };

    // Register with pandemic daemon
    let plugin_info = PluginInfo {
        name: "pandemic-rest".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        description: Some("REST API server for pandemic daemon".to_string()),
        config: Some({
            let mut config = HashMap::new();
            config.insert("port".to_string(), args.port.to_string());
            config.insert("bind_address".to_string(), args.bind_address.clone());
            config
        }),
        registered_at: None,
    };

    let mut client = DaemonClient::connect(&args.socket_path).await?;
    client
        .send_request(&Request::Register {
            plugin: plugin_info,
        })
        .await?;

    info!("Registered with pandemic daemon");

    // Set up application state
    let state = AppState {
        socket_path: args.socket_path,
        auth_config,
        agent_status: Arc::new(Mutex::new(AgentStatus::new())),
    };

    // Build the router with auth-protected routes
    let protected_routes = Router::new()
        .route("/api/plugins", get(list_plugins))
        .route("/api/plugins/:name", get(get_plugin))
        .route("/api/plugins/:name", delete(deregister_plugin))
        .route("/api/health", get(get_health))
        .route("/api/events", post(publish_event))
        .route("/api/admin/services", get(list_system_services))
        .route("/api/admin/services/:name", get(get_system_service))
        .route(
            "/api/admin/services/:name/action",
            post(control_system_service),
        )
        .route("/api/admin/capabilities", get(get_admin_capabilities))
        // Admin user management routes
        .route("/api/admin/users", post(create_user).get(list_users))
        .route(
            "/api/admin/users/:username",
            delete(delete_user).put(modify_user),
        )
        // Admin group management routes
        .route("/api/admin/groups", get(list_groups))
        .route(
            "/api/admin/groups/:groupname",
            post(create_group).delete(delete_group),
        )
        .route(
            "/api/admin/groups/:groupname/users/:username",
            post(add_user_to_group).delete(remove_user_from_group),
        )
        // Admin service configuration routes
        .route(
            "/api/admin/services/:service/config",
            get(get_service_config)
                .put(set_service_config)
                .delete(reset_service_config),
        )
        .layer(from_fn_with_state(state.clone(), auth_middleware));

    // WebSocket route handles auth internally
    let websocket_routes = Router::new().route("/api/events/stream", get(websocket_handler));

    let app = Router::new()
        .merge(protected_routes)
        .merge(websocket_routes)
        .layer(
            ServiceBuilder::new()
                .layer(TraceLayer::new_for_http())
                .layer(CorsLayer::permissive()),
        )
        .with_state(state);

    // Start the server
    let bind_addr = format!("{}:{}", args.bind_address, args.port);
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    info!("REST API server listening on {}", bind_addr);

    axum::serve(listener, app).await?;

    Ok(())
}

async fn create_default_auth_config(path: &PathBuf) -> Result<()> {
    let default_config = r#"[identities.admin]
api_key = "pandemic-admin-key-change-me"
roles = ["admin"]

[identities.reader]
api_key = "pandemic-reader-key-change-me"
roles = ["reader"]

[roles.admin]
scopes = ["*"]

[roles.reader]
scopes = ["plugins:read", "health:read", "events:subscribe"]
"#;

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    tokio::fs::write(path, default_config).await?;
    info!("Created default auth config at {:?}", path);
    info!("WARNING: Please change the default API keys!");

    Ok(())
}
