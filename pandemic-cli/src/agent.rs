use crate::{secret, system, AgentAction};
use anyhow::Result;
use pandemic_protocol::AgentRequest;
use std::path::Path;

pub async fn handle_agent_command(action: AgentAction) -> Result<()> {
    match action {
        AgentAction::Install { binary_path } => install_agent(&binary_path),
        AgentAction::Uninstall => system::uninstall_service("agent"),
        AgentAction::Start => system::start_service("agent"),
        AgentAction::Stop => system::stop_service("agent"),
        AgentAction::Restart => system::restart_service("agent"),
        AgentAction::Status => system::status_service("agent"),
        AgentAction::Request {
            json,
            agent_secret,
            agent_secret_path,
        } => {
            let request: AgentRequest = serde_json::from_str(&json)
                .map_err(|e| anyhow::anyhow!("invalid AgentRequest JSON: {e}"))?;
            let data =
                crate::service::agent_action(&request, agent_secret, agent_secret_path).await?;
            println!("{}", serde_json::to_string_pretty(&data)?);
            Ok(())
        }
    }
}

pub fn install_agent(binary_path: &Path) -> Result<()> {
    let service_content = format!(
        r#"[Unit]
Description=Pandemic Agent - Privileged Operations Service
After=network.target

[Service]
Type=simple
ExecStart={}
Restart=always
RestartSec=5
User=root
Group=root

[Install]
WantedBy=multi-user.target
"#,
        binary_path.display()
    );

    // Mint the shared secret at the default path so the agent and the CLI
    // agree without extra ceremony. The agent falls back to this path when
    // --secret / --secret-path are not set.
    let secret_path = secret::ensure_agent_secret()?;

    system::install_service("agent", &service_content)?;
    println!(
        "Agent secret installed at {} (0600, root-only)",
        secret_path.display()
    );
    Ok(())
}
