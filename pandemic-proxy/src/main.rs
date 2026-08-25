use anyhow::{bail, Result};
use clap::Parser;
use pandemic_common::{DaemonClient, PersistentClient};
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
#[command(
    about = "Universal infection wrapper for arbitrary executables and existing systemd services"
)]
struct Args {
    #[arg(long, default_value = "/var/run/pandemic/pandemic.sock")]
    socket_path: PathBuf,

    #[arg(long, default_value = "infection.toml")]
    config: PathBuf,

    /// Attach to an existing systemd unit instead of spawning a process
    #[arg(long)]
    attach: Option<String>,
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
    /// Command to spawn and supervise (required unless `attach` is set)
    pub command: Option<Vec<String>>,
    pub health_check: Option<Vec<String>>,
    pub health_interval: Option<u64>,
    /// Name of an existing systemd unit to register as an infection instead of spawning
    pub attach: Option<String>,
}

/// What the proxy does with the infection target.
enum Target {
    /// Spawn and supervise a child process
    Spawn(Vec<String>),
    /// Register an existing systemd unit (the CLI flag wins over the config file)
    Attach(String),
}

/// Resolve the infection target, requiring exactly one of `command` / `attach`.
fn resolve_target(cli_attach: Option<&str>, runtime: &RuntimeConfig) -> Result<Target> {
    let attach = cli_attach.map(str::to_string).or(runtime.attach.clone());
    match (&attach, &runtime.command) {
        (Some(_), Some(_)) => bail!(
            "infection target is ambiguous: set either `attach` or `command`, not both"
        ),
        (Some(unit), None) => Ok(Target::Attach(unit.clone())),
        (None, Some(command)) => Ok(Target::Spawn(command.clone())),
        (None, None) => bail!(
            "infection target missing: set `command` (spawn a process) or `attach` (an existing systemd unit) in the runtime config"
        ),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    let config = load_config(&args.config).await?;
    info!("Loaded config for infection: {}", config.infection.name);

    let target = resolve_target(args.attach.as_deref(), &config.runtime)?;
    let health_interval = Duration::from_secs(config.runtime.health_interval.unwrap_or(30));

    // Attached services default to polling the unit's state via systemctl
    let health_check = config
        .runtime
        .health_check
        .clone()
        .or_else(|| match &target {
            Target::Attach(unit) => Some(vec![
                "systemctl".to_string(),
                "is-active".to_string(),
                unit.clone(),
            ]),
            Target::Spawn(_) => None,
        });

    // Register with pandemic daemon
    let mut plugin_config = HashMap::new();
    plugin_config.insert("proxy".to_string(), "true".to_string());
    match &target {
        Target::Spawn(command) => {
            plugin_config.insert("command".to_string(), command.join(" "));
        }
        Target::Attach(unit) => {
            plugin_config.insert("attach".to_string(), unit.clone());
        }
    }

    let plugin_info = PluginInfo {
        name: config.infection.name.clone(),
        version: config.infection.version.clone(),
        description: config.infection.description.clone(),
        config: Some(plugin_config),
        registered_at: None,
    };

    let mut client = DaemonClient::connect(&args.socket_path).await?;
    client
        .send_request(&Request::Register {
            plugin: plugin_info,
        })
        .await?;
    info!("Registered {} with pandemic daemon", config.infection.name);

    match target {
        Target::Spawn(command) => {
            run_spawner(
                &mut client,
                &config.infection.name,
                &command,
                &health_check,
                health_interval,
            )
            .await?
        }
        Target::Attach(unit) => {
            run_attacher(
                &mut client,
                &config.infection.name,
                &unit,
                &health_check,
                health_interval,
            )
            .await
        }
    }

    info!("Proxy shutting down");
    Ok(())
}

/// Spawn mode: supervise a child process; exit (and let systemd restart us) when it dies.
async fn run_spawner(
    client: &mut PersistentClient,
    name: &str,
    command: &[String],
    health_check: &Option<Vec<String>>,
    health_interval: Duration,
) -> Result<()> {
    let mut child = Command::new(&command[0])
        .args(&command[1..])
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?;

    info!("Started process: {:?}", command);

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
                run_health_cycle(client, name, health_check, &mut last_health_status).await;
            }
        }
    }

    let _ = child.kill().await;
    Ok(())
}

/// Attach mode: no child process — report the state of an existing systemd unit.
async fn run_attacher(
    client: &mut PersistentClient,
    name: &str,
    unit: &str,
    health_check: &Option<Vec<String>>,
    health_interval: Duration,
) {
    info!("Attached to existing systemd unit: {}", unit);

    let mut last_health_status: Option<bool> = None;

    // Report the unit's current state immediately, then poll on the interval
    run_health_cycle(client, name, health_check, &mut last_health_status).await;

    loop {
        sleep(health_interval).await;
        run_health_cycle(client, name, health_check, &mut last_health_status).await;
    }
}

/// Run one health check and publish an event on the `health.<name>` topic when the state changes.
async fn run_health_cycle(
    client: &mut PersistentClient,
    name: &str,
    health_check: &Option<Vec<String>>,
    last_health_status: &mut Option<bool>,
) {
    let command = health_check.as_deref().unwrap_or_default();
    match run_health_check(command).await {
        Ok(is_healthy) => {
            if *last_health_status != Some(is_healthy) {
                let status = if is_healthy { "healthy" } else { "unhealthy" };
                info!("Health status changed to: {}", status);
                publish_health_event(client, name, status, is_healthy, None).await;
                *last_health_status = Some(is_healthy);
            } else if is_healthy {
                info!("Health check passed");
            } else {
                warn!("Health check failed");
            }
        }
        Err(e) => {
            warn!("Health check error: {}", e);
            // Treat errors as unhealthy
            if *last_health_status != Some(false) {
                publish_health_event(client, name, "error", false, Some(e.to_string())).await;
                *last_health_status = Some(false);
            }
        }
    }
}

async fn publish_health_event(
    client: &mut PersistentClient,
    service: &str,
    status: &str,
    healthy: bool,
    error: Option<String>,
) {
    let topic = format!("health.{}", service);
    let mut data = serde_json::json!({
        "service": service,
        "status": status,
        "healthy": healthy,
        "timestamp": chrono::Utc::now().to_rfc3339()
    });
    if let Some(err) = error {
        data["error"] = serde_json::json!(err);
    }

    if let Err(e) = client.send_request(&Request::Publish { topic, data }).await {
        warn!("Failed to publish health event: {}", e);
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime(command: Option<Vec<String>>, attach: Option<String>) -> RuntimeConfig {
        RuntimeConfig {
            command,
            health_check: None,
            health_interval: None,
            attach,
        }
    }

    #[test]
    fn spawn_mode_from_command() {
        let rt = runtime(Some(vec!["mosquitto".to_string()]), None);
        assert!(matches!(resolve_target(None, &rt), Ok(Target::Spawn(_))));
    }

    #[test]
    fn attach_mode_from_config() {
        let rt = runtime(None, Some("mosquitto".to_string()));
        assert!(matches!(
            resolve_target(None, &rt),
            Ok(Target::Attach(unit)) if unit == "mosquitto"
        ));
    }

    #[test]
    fn cli_flag_wins_over_config_attach() {
        let rt = runtime(None, Some("redis".to_string()));
        assert!(matches!(
            resolve_target(Some("mosquitto"), &rt),
            Ok(Target::Attach(unit)) if unit == "mosquitto"
        ));
    }

    #[test]
    fn rejects_command_and_attach_together() {
        let rt = runtime(
            Some(vec!["mosquitto".to_string()]),
            Some("mosquitto".to_string()),
        );
        assert!(resolve_target(None, &rt).is_err());
    }

    #[test]
    fn rejects_missing_target() {
        let rt = runtime(None, None);
        assert!(resolve_target(None, &rt).is_err());
    }
}
