use anyhow::Result;
use clap::Parser;
use pandemic_protocol::{PluginInfo, Request, Response};
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tracing::{error, info, warn};

#[derive(Parser)]
#[command(name = "pandemic")]
#[command(about = "Lightweight daemon for managing infection plugins")]
struct Args {
    #[arg(long, default_value = "/var/run/pandemic/pandemic.sock")]
    socket_path: PathBuf,
}

struct Daemon {
    plugins: HashMap<String, PluginInfo>,
}

impl Daemon {
    fn new() -> Self {
        Self {
            plugins: HashMap::new(),
        }
    }

    fn handle_request(&mut self, request: Request) -> Response {
        match request {
            Request::Register { mut plugin } => {
                info!("Registering plugin: {}", plugin.name);
                plugin.registered_at = Some(SystemTime::now());
                self.plugins.insert(plugin.name.clone(), plugin);
                Response::success()
            }
            Request::ListPlugins => {
                let plugins: Vec<&PluginInfo> = self.plugins.values().collect();
                Response::success_with_data(json!(plugins))
            }
            Request::GetPlugin { name } => {
                match self.plugins.get(&name) {
                    Some(plugin) => Response::success_with_data(json!(plugin)),
                    None => Response::not_found(format!("Plugin '{}' not found", name)),
                }
            }
        }
    }

    async fn handle_connection(&mut self, stream: UnixStream) -> Result<()> {
        let mut reader = BufReader::new(stream);
        let mut line = String::new();

        while reader.read_line(&mut line).await? > 0 {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                line.clear();
                continue;
            }

            let response = match serde_json::from_str::<Request>(trimmed) {
                Ok(request) => self.handle_request(request),
                Err(e) => {
                    warn!("Invalid request: {}", e);
                    Response::error(format!("Invalid request: {}", e))
                }
            };

            let response_json = serde_json::to_string(&response)?;
            reader.get_mut().write_all(response_json.as_bytes()).await?;
            reader.get_mut().write_all(b"\n").await?;

            line.clear();
        }

        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    if let Some(parent) = args.socket_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let _ = tokio::fs::remove_file(&args.socket_path).await;
    let listener = UnixListener::bind(&args.socket_path)?;
    info!("Pandemic daemon listening on {:?}", args.socket_path);

    let mut daemon = Daemon::new();

    while let Ok((stream, _)) = listener.accept().await {
        if let Err(e) = daemon.handle_connection(stream).await {
            error!("Connection error: {}", e);
        }
    }

    Ok(())
}