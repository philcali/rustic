use anyhow::Result;
use pandemic_common::{AgentClient, AGENT_SECRET_PATH};
use pandemic_protocol::{AgentRequest, Response};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{system, ServiceAction};

pub async fn handle_service_command(action: ServiceAction) -> Result<()> {
    match action {
        ServiceAction::Install { name, binary_path } => install_service(&name, &binary_path),
        ServiceAction::Uninstall { name } => system::uninstall_service(&name),
        ServiceAction::Start { name } => system::start_service(&name),
        ServiceAction::Stop { name } => system::stop_service(&name),
        ServiceAction::Restart { name } => system::restart_service(&name),
        ServiceAction::Status { name } => system::status_service(&name),
        ServiceAction::Attach {
            unit,
            name,
            version,
            description,
            health_interval,
            agent_secret,
            agent_secret_path,
        } => {
            attach_infection(
                &unit,
                name,
                version,
                description,
                health_interval,
                agent_secret,
                agent_secret_path,
            )
            .await
        }
        ServiceAction::Detach {
            name,
            agent_secret,
            agent_secret_path,
        } => detach_infection(&name, agent_secret, agent_secret_path).await,
        ServiceAction::Logs {
            name,
            follow,
            lines,
        } => logs_service(&name, follow, lines),
        ServiceAction::Config {
            name,
            show,
            reset,
            args,
        } => config_service(&name, show, reset, args),
    }
}

fn install_service(name: &str, binary_path: &Path) -> Result<()> {
    let service_content = format!(
        r#"[Unit]
Description=Pandemic Infection: {}
After=pandemic.service
Requires=pandemic.service

[Service]
Type=simple
ExecStart={}
Restart=always
RestartSec=5
User=pandemic
Group=pandemic

[Install]
WantedBy=multi-user.target
"#,
        name,
        binary_path.display()
    );
    system::install_service(name, &service_content)
}

/// Build an authenticated agent client: `--agent-secret` > `--agent-secret-path`
/// > the default path installed by `bootstrap install --with-agent`.
fn agent_client(secret: Option<String>, secret_path: Option<PathBuf>) -> Result<AgentClient> {
    if let Some(secret) = secret {
        return Ok(AgentClient::new().with_secret(secret));
    }
    if let Some(path) = secret_path {
        return AgentClient::new().with_secret_path(path);
    }
    if Path::new(AGENT_SECRET_PATH).exists() {
        return AgentClient::new().with_secret_path(AGENT_SECRET_PATH);
    }
    Err(anyhow::anyhow!(
        "no agent secret found at {AGENT_SECRET_PATH}; run `pandemic-cli bootstrap install --with-agent` (or `pandemic-cli agent install`) first, or pass --agent-secret / --agent-secret-path"
    ))
}

async fn agent_action(
    request: &AgentRequest,
    secret: Option<String>,
    secret_path: Option<PathBuf>,
) -> Result<serde_json::Value> {
    let client = agent_client(secret, secret_path)?;
    let response = client.send_agent_request(request).await?;
    match response {
        Response::Success { data } => Ok(data.unwrap_or_else(|| serde_json::json!({}))),
        Response::Error { message } => Err(anyhow::anyhow!("{message}")),
        Response::NotFound { message } => Err(anyhow::anyhow!("{message}")),
    }
}

async fn attach_infection(
    unit: &str,
    name: Option<String>,
    version: Option<String>,
    description: Option<String>,
    health_interval: Option<u64>,
    agent_secret: Option<String>,
    agent_secret_path: Option<PathBuf>,
) -> Result<()> {
    let request = AgentRequest::AttachInfection {
        unit: unit.to_string(),
        name,
        version,
        description,
        health_check: None,
        health_interval,
        proxy_path: None,
    };
    let data = agent_action(&request, agent_secret, agent_secret_path).await?;

    if let Some(name) = data.get("name").and_then(|v| v.as_str()) {
        println!("✅ Attached {unit} as infection '{name}'");
    }
    if let Some(service) = data.get("service").and_then(|v| v.as_str()) {
        println!("   Service:  {service}");
    }
    if let Some(config) = data.get("config_path").and_then(|v| v.as_str()) {
        println!("   Config:   {config}");
    }
    if let Some(target) = data.get("unit").and_then(|v| v.as_str()) {
        println!("   Attaches: {target}.service");
    }
    Ok(())
}

async fn detach_infection(
    name: &str,
    agent_secret: Option<String>,
    agent_secret_path: Option<PathBuf>,
) -> Result<()> {
    let request = AgentRequest::DetachInfection {
        name: name.to_string(),
    };
    let data = agent_action(&request, agent_secret, agent_secret_path).await?;
    println!("✅ Detached infection '{name}'");
    if let Some(removed) = data.get("removed_unit").and_then(|v| v.as_bool()) {
        if !removed {
            println!("   Note: service unit was not present (already removed?)");
        }
    }
    Ok(())
}

fn logs_service(name: &str, follow: bool, lines: u32) -> Result<()> {
    let service_name = if name.starts_with("pandemic") {
        name.to_string()
    } else {
        format!("pandemic-{}", name)
    };

    let mut cmd = Command::new("journalctl");
    cmd.args(["-u", &service_name, "-n", &lines.to_string()]);

    if follow {
        cmd.arg("-f");
    }

    cmd.status()?;
    Ok(())
}

fn config_service(name: &str, show: bool, reset: bool, args: Vec<String>) -> Result<()> {
    let service_name = format!("pandemic-{}", name);
    let override_dir = format!("/etc/systemd/system/{}.service.d", service_name);
    let override_file = format!("{}/override.conf", override_dir);

    if show {
        if std::path::Path::new(&override_file).exists() {
            let content = std::fs::read_to_string(&override_file)?;
            println!("Current configuration for {}:", service_name);
            println!("{}", content);
        } else {
            println!("No custom configuration for {}", service_name);
        }
        return Ok(());
    }

    if reset {
        if std::path::Path::new(&override_dir).exists() {
            std::fs::remove_dir_all(&override_dir)?;
            Command::new("systemctl").args(["daemon-reload"]).status()?;
            println!("Reset {} to default configuration", service_name);
        } else {
            println!("{} already using default configuration", service_name);
        }
        return Ok(());
    }

    if args.is_empty() {
        eprintln!("No arguments provided. Use --show to view current config or --reset to restore defaults.");
        return Ok(());
    }

    let binary_path = format!("/usr/local/bin/pandemic-{}", name);
    let exec_start = format!("{} {}", binary_path, args.join(" "));
    let override_content = format!("[Service]\nExecStart=\nExecStart={}\n", exec_start);

    std::fs::create_dir_all(&override_dir)?;
    std::fs::write(&override_file, override_content)?;

    Command::new("systemctl").args(["daemon-reload"]).status()?;

    println!("Updated {} configuration:", service_name);
    println!("ExecStart={}", exec_start);
    println!("Run 'systemctl restart {}' to apply changes", service_name);

    Ok(())
}
