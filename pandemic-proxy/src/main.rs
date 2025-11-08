use anyhow::Result;
use clap::Parser;
use pandemic_common::DaemonClient;
use pandemic_protocol::{PluginInfo, Request};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;
use tokio::time::{sleep, Duration};
use tracing::{error, info, warn};

#[derive(Parser)]
#[command(name = "pandemic-proxy")]
#[command(about = "Universal infection wrapper for arbitrary executables")]
struct Args {
    #[arg(long, default_value = "/var/run/pandemic/pandemic.sock")]
    socket_path: PathBuf,

    #[arg(long, default_value = "infection.toml")]
    config: PathBuf,
}

#[derive(Debug, Deserialize, Serialize)]
struct ProxyConfig {
    pub infection: InfectionConfig,
    pub runtime: RuntimeConfig,
}

#[derive(Debug, Deserialize, Serialize)]
struct InfectionConfig {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct RuntimeConfig {
    pub command: Vec<String>,
    pub health_check: Option<Vec<String>>,
    pub health_interval: Option<u64>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    let config = load_config(&args.config).await?;
    info!("Loaded config for infection: {}", config.infection.name);

    // Register with pandemic daemon
    let plugin_info = PluginInfo {
        name: config.infection.name.clone(),
        version: config.infection.version.clone(),
        description: config.infection.description.clone(),
        config: Some({
            let mut plugin_config = HashMap::new();
            plugin_config.insert("proxy".to_string(), "true".to_string());
            plugin_config.insert("command".to_string(), config.runtime.command.join(" "));
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
    info!("Registered {} with pandemic daemon", config.infection.name);

    // Start the wrapped process
    let mut child = Command::new(&config.runtime.command[0])
        .args(&config.runtime.command[1..])
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?;

    info!("Started process: {:?}", config.runtime.command);

    // Health check loop
    let health_interval = Duration::from_secs(config.runtime.health_interval.unwrap_or(30));
    let mut last_health_status: Option<bool> = None;

    loop {
        tokio::select! {
            // Check if child process is still running
            status = child.wait() => {
                match status {
                    Ok(exit_status) => {
                        if exit_status.success() {
                            info!("Process exited successfully");
                        } else {
                            error!("Process exited with status: {}", exit_status);
                        }
                        break;
                    }
                    Err(e) => {
                        error!("Error waiting for process: {}", e);
                        break;
                    }
                }
            }

            // Periodic health check
            _ = sleep(health_interval) => {
                if let Some(health_cmd) = &config.runtime.health_check {
                    match run_health_check(health_cmd).await {
                        Ok(is_healthy) => {
                            // Check if health status changed
                            if last_health_status != Some(is_healthy) {
                                let status = if is_healthy { "healthy" } else { "unhealthy" };
                                info!("Health status changed to: {}", status);

                                // Publish health status change event
                                let topic = format!("health.{}", config.infection.name);
                                let data = serde_json::json!({
                                    "service": config.infection.name,
                                    "status": status,
                                    "healthy": is_healthy,
                                    "timestamp": chrono::Utc::now().to_rfc3339()
                                });

                                if let Err(e) = client.send_request(&Request::Publish { topic, data }).await {
                                    warn!("Failed to publish health event: {}", e);
                                }

                                last_health_status = Some(is_healthy);
                            } else if is_healthy {
                                info!("Health check passed");
                            } else {
                                warn!("Health check failed");
                            }
                        }
                        Err(e) => {
                            warn!("Health check error: {}", e);
                            // Treat errors as unhealthy
                            if last_health_status != Some(false) {
                                let topic = format!("health.{}", config.infection.name);
                                let data = serde_json::json!({
                                    "service": config.infection.name,
                                    "status": "error",
                                    "healthy": false,
                                    "error": e.to_string(),
                                    "timestamp": chrono::Utc::now().to_rfc3339()
                                });

                                if let Err(e) = client.send_request(&Request::Publish { topic, data }).await {
                                    warn!("Failed to publish health error event: {}", e);
                                }

                                last_health_status = Some(false);
                            }
                        }
                    }
                }
            }
        }
    }

    // Cleanup
    let _ = child.kill().await;
    info!("Proxy shutting down");
    Ok(())
}

async fn load_config(path: &PathBuf) -> Result<ProxyConfig> {
    let content = tokio::fs::read_to_string(path).await?;
    let config: ProxyConfig = toml::from_str(&content)?;
    Ok(config)
}

async fn run_health_check(command: &[String]) -> Result<bool> {
    if command.is_empty() {
        return Ok(true);
    }

    let output = Command::new(&command[0])
        .args(&command[1..])
        .output()
        .await?;

    Ok(output.status.success())
}
