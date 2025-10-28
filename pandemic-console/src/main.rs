use anyhow::Result;
use axum::{
    http::{header, StatusCode, Uri},
    response::{Html, IntoResponse, Response},
    routing::get,
    Router,
};
use clap::Parser;
use include_dir::{include_dir, Dir};
use pandemic_common::DaemonClient;
use pandemic_protocol::{PluginInfo, Request};
use std::collections::HashMap;
use std::path::PathBuf;
use tower::ServiceBuilder;
use tower_http::trace::TraceLayer;
use tracing::info;

static ASSETS_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/web/dist");

#[derive(Parser)]
#[command(name = "pandemic-console")]
#[command(about = "Web console infection for pandemic daemon")]
struct Args {
    #[arg(long, default_value = "/var/run/pandemic/pandemic.sock")]
    socket_path: PathBuf,

    #[arg(long, default_value = "127.0.0.1")]
    bind_address: String,

    #[arg(long, default_value = "3000")]
    port: u16,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    // Register with pandemic daemon
    let plugin_info = PluginInfo {
        name: "pandemic-console".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        description: Some("Web console for pandemic daemon".to_string()),
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

    // Build the router
    let app = Router::new()
        .route("/", get(serve_index))
        .route("/*file", get(serve_static))
        .layer(ServiceBuilder::new().layer(TraceLayer::new_for_http()));

    // Start the server
    let bind_addr = format!("{}:{}", args.bind_address, args.port);
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    info!("Console server listening on {}", bind_addr);

    axum::serve(listener, app).await?;

    Ok(())
}

async fn serve_index() -> impl IntoResponse {
    serve_static_file("index.html").await
}

async fn serve_static(uri: Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');
    serve_static_file(path).await
}

async fn serve_static_file(path: &str) -> Response {
    match ASSETS_DIR.get_file(path) {
        Some(file) => {
            let mime_type = mime_guess::from_path(path).first_or_octet_stream();
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, mime_type.as_ref())],
                file.contents(),
            )
                .into_response()
        }
        None => {
            // For SPA routing, serve index.html for unknown routes
            if let Some(index) = ASSETS_DIR.get_file("index.html") {
                Html(std::str::from_utf8(index.contents()).unwrap_or("")).into_response()
            } else {
                (StatusCode::NOT_FOUND, "File not found").into_response()
            }
        }
    }
}
