mod config;
mod credentials;
mod handlers;
mod iam_anywhere;
mod signer;
mod signing;

use anyhow::Result;
use axum::{
    routing::{get, put},
    Router,
};
use clap::Parser;
use pandemic_common::DaemonClient;
use pandemic_protocol::{PluginInfo, Request};
use std::collections::HashMap;
use std::path::PathBuf;
use tower::ServiceBuilder;
use tower_http::trace::TraceLayer;
use tracing::{error, info};

use config::IamConfig;
use credentials::CredentialManager;
use handlers::{get_role_credentials, get_token, health_check, list_roles, AppState};

#[derive(Parser)]
#[command(name = "pandemic-iam")]
#[command(about = "AWS IAM Anywhere infection with IMDSv2-compatible endpoint")]
struct Args {
    #[arg(long, default_value = "/var/run/pandemic/pandemic.sock")]
    socket_path: PathBuf,

    #[arg(long, default_value = "/etc/pandemic/iam-config.toml")]
    config_path: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    // Load IAM configuration - fail if missing or invalid
    let config = IamConfig::load(&args.config_path)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load config file {:?}: {}", args.config_path, e))?;
    info!("Loaded IAM config from {:?}", args.config_path);

    // Initialize credential manager
    let credential_manager = CredentialManager::new();

    // Register with pandemic daemon
    let plugin_info = PluginInfo {
        name: "pandemic-iam".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        description: Some("AWS IAM Anywhere infection with IMDSv2-compatible endpoint".to_string()),
        config: Some({
            let mut plugin_config = HashMap::new();
            plugin_config.insert("port".to_string(), config.server.port.to_string());
            plugin_config.insert(
                "bind_address".to_string(),
                config.server.bind_address.clone(),
            );
            plugin_config
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
        credential_manager: credential_manager.clone(),
        config: config.clone(),
    };

    // Start credential refresh task
    let refresh_config = config.aws.clone();
    let refresh_manager = credential_manager.clone();
    tokio::spawn(async move {
        credential_refresh_loop(refresh_manager, refresh_config).await;
    });

    // Build the router with IMDSv2-compatible endpoints
    let app = Router::new()
        // IMDSv2 token endpoint
        .route("/latest/api/token", put(get_token))
        // Security credentials endpoints
        .route(
            "/latest/meta-data/iam/security-credentials/",
            get(list_roles),
        )
        .route(
            "/latest/meta-data/iam/security-credentials/:role",
            get(get_role_credentials),
        )
        // Health check
        .route("/health", get(health_check))
        .layer(ServiceBuilder::new().layer(TraceLayer::new_for_http()))
        .with_state(state);

    // Start the server
    let bind_addr = format!("{}:{}", config.server.bind_address, config.server.port);
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    info!("IAM Anywhere server listening on {}", bind_addr);
    info!(
        "IMDSv2-compatible endpoint available at http://{}",
        bind_addr
    );

    axum::serve(listener, app).await?;

    Ok(())
}

async fn credential_refresh_loop(manager: CredentialManager, config: config::AwsConfig) {
    let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(300)); // Check every 5 minutes

    loop {
        interval.tick().await;

        if manager.needs_refresh().await {
            info!("Refreshing AWS credentials...");
            if let Err(e) = manager.refresh_credentials(&config).await {
                error!("Failed to refresh credentials: {}", e);
            }
        }
    }
}
