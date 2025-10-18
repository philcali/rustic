use anyhow::Result;
use pandemic_protocol::PluginInfo;
use pandemic_common::DaemonClient;
use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use tracing::info;
use clap::Parser;

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
    info!("Connected to daemon, registering and keeping connection alive...");

    // This will register and keep the connection alive
    client.register_and_keep_alive(plugin).await?;
    
    Ok(())
}