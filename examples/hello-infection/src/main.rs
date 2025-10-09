use anyhow::Result;
use pandemic_protocol::{PluginInfo, Request};
use pandemic_common::DaemonClient;
use std::collections::HashMap;
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
        description: Some("A simple hello world infection plugin".to_string()),
        config: Some(config),
        registered_at: None,
    };
    
    let request = Request::Register { plugin };
    let response = DaemonClient::send_request(&args.socket_path, &request).await?;
    info!("Registration response: {:?}", response);
    
    Ok(())
}