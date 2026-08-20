use anyhow::Result;
use clap::Parser;
use pandemic_common::DaemonClient;
use pandemic_protocol::{PluginInfo, Request, Response};
use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use tracing::info;

#[derive(Parser)]
#[command(name = "hello-infection")]
#[command(about = "A simple hello world infection plugin")]
struct Args {
    #[arg(long, default_value = "/var/run/pandemic/pandemic.sock")]
    socket_path: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    let mut config = HashMap::new();
    config.insert("greeting".to_string(), "Hello, World!".to_string());

    let plugin = PluginInfo {
        name: "hello-infection".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        description: Some("A simple hello world infection plugin".to_string()),
        config: Some(config),
        registered_at: None,
    };

    let mut client = DaemonClient::connect(&args.socket_path).await?;
    info!("Connected to daemon, registering...");

    // Register the plugin
    let response = client
        .send_request(&Request::Register {
            plugin: plugin.clone(),
        })
        .await?;
    if let Response::Success { .. } = &response {
        info!("Registered plugin");
    } else {
        anyhow::bail!("Registration failed: {:?}", response);
    }

    // Subscribe to events
    client.subscribe(vec!["*".to_string()]).await?;
    info!("Subscribed to all events");

    // Event loop — keeps the connection alive
    loop {
        if let Some(event) = client.read_event().await? {
            info!("Received event: {:?}", event);
        }
    }
}
