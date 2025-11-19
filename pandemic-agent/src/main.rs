mod handlers;
mod socket;
mod systemd;

use anyhow::Result;
use clap::Parser;
use pandemic_protocol::{AgentMessage, Response};
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tracing::{error, info, warn};

use handlers::handle_agent_request;
use socket::setup_socket_permissions;

#[derive(Parser)]
#[command(name = "pandemic-agent")]
#[command(about = "Privileged agent for pandemic system management")]
pub struct Args {
    #[arg(long, default_value = "/var/run/pandemic/admin.sock")]
    pub socket_path: PathBuf,

    #[arg(long, default_value = "pandemic")]
    pub user: String,

    #[arg(long, default_value = "pandemic")]
    pub group: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    // Ensure we're running as root
    if unsafe { libc::getuid() } != 0 {
        return Err(anyhow::anyhow!("pandemic-agent must run as root"));
    }

    info!("Starting pandemic-agent as root");

    // Remove existing socket if it exists
    if args.socket_path.exists() {
        std::fs::remove_file(&args.socket_path)?;
    }

    // Create socket directory if it doesn't exist
    if let Some(parent) = args.socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Bind to Unix socket
    let listener = UnixListener::bind(&args.socket_path)?;

    // Set socket permissions and ownership
    setup_socket_permissions(&args)?;

    info!("Agent listening on {:?}", args.socket_path);

    // Accept connections
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tokio::spawn(handle_connection(stream));
            }
            Err(e) => {
                error!("Failed to accept connection: {}", e);
            }
        }
    }
}

async fn handle_connection(mut stream: UnixStream) -> Result<()> {
    let (reader, mut writer) = stream.split();
    let mut buf_reader = BufReader::new(reader);
    let mut line = String::new();

    while buf_reader.read_line(&mut line).await? > 0 {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            line.clear();
            continue;
        }

        let response = match serde_json::from_str::<AgentMessage>(trimmed) {
            Ok(AgentMessage::Request(request)) => handle_agent_request(request).await,
            Ok(_) => Response::error("Expected request message"),
            Err(e) => {
                warn!("Failed to parse message: {}", e);
                Response::error("Invalid message format")
            }
        };

        let response_json = serde_json::to_string(&response)?;
        writer.write_all(response_json.as_bytes()).await?;
        writer.write_all(b"\n").await?;

        line.clear();
    }

    Ok(())
}
