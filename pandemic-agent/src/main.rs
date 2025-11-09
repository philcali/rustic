use anyhow::Result;
use clap::Parser;
use pandemic_protocol::{AgentMessage, AgentRequest, Response};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tracing::{error, info, warn};

#[derive(Parser)]
#[command(name = "pandemic-agent")]
#[command(about = "Privileged agent for pandemic system management")]
struct Args {
    #[arg(long, default_value = "/var/run/pandemic/admin.sock")]
    socket_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PandemicServiceSummary {
    pub name: String,
    pub description: String,
    pub status: String,
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

    // Set socket permissions (root:root 600)
    std::fs::set_permissions(&args.socket_path, std::fs::Permissions::from_mode(0o600))?;

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

async fn handle_agent_request(request: AgentRequest) -> Response {
    match request {
        AgentRequest::GetHealth => {
            info!("Health check requested");
            Response::success_with_data(serde_json::json!({
                "status": "healthy",
                "capabilities": ["systemd"]
            }))
        }

        AgentRequest::ListServices => {
            info!("Service list requested");
            match list_pandemic_services().await {
                Ok(services) => Response::success_with_data(serde_json::json!({
                    "services": services
                })),
                Err(e) => Response::error(format!("Failed to list services: {}", e)),
            }
        }

        AgentRequest::GetCapabilities => {
            info!("Capabilities requested");
            Response::success_with_data(serde_json::json!({
                "capabilities": ["systemd", "service_management"]
            }))
        }

        AgentRequest::SystemdControl { action, service } => {
            info!("Systemd control: {} {}", action, service);

            let result = match action.as_str() {
                "start" | "stop" | "restart" | "enable" | "disable" | "status" => {
                    execute_systemctl(&action, &service).await
                }
                _ => {
                    return Response::error("Invalid systemd action");
                }
            };

            match result {
                Ok(output) => Response::success_with_data(serde_json::json!({
                    "action": action,
                    "service": service,
                    "output": output
                })),
                Err(e) => Response::error(format!("Systemd operation failed: {}", e)),
            }
        }
    }
}

async fn execute_systemctl(action: &str, service: &str) -> Result<String> {
    let output = Command::new("systemctl")
        .arg(action)
        .arg(service)
        .output()?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(anyhow::anyhow!(
            "systemctl failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

async fn list_pandemic_services() -> Result<Vec<serde_json::Value>> {
    let output = Command::new("systemctl")
        .arg("--legend=false")
        .arg("--plain")
        .arg("list-units")
        .arg("pandemic*")
        .output()?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let services: Vec<serde_json::Value> = stdout
            .lines()
            .filter_map(|line| {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 4 {
                    let summary = PandemicServiceSummary {
                        name: parts[0].to_string(),
                        description: parts[3..].join(" "),
                        status: parts[2].to_string(),
                    };
                    Some(serde_json::json!(summary))
                } else {
                    None
                }
            })
            .collect();
        Ok(services)
    } else {
        Err(anyhow::anyhow!(
            "systemctl list-units failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

#[cfg(target_os = "linux")]
use std::os::unix::fs::PermissionsExt;

#[cfg(not(target_os = "linux"))]
trait PermissionsExt {
    fn from_mode(_mode: u32) -> std::fs::Permissions {
        std::fs::Permissions::from(
            std::fs::File::open("/dev/null")
                .unwrap()
                .metadata()
                .unwrap()
                .permissions(),
        )
    }
}
