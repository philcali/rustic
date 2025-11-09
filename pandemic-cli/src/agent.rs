use crate::{system, AgentAction};
use anyhow::Result;
use std::path::Path;

pub fn handle_agent_command(action: AgentAction) -> Result<()> {
    match action {
        AgentAction::Install { binary_path } => install_agent(&binary_path),
        AgentAction::Uninstall => system::uninstall_service("agent"),
        AgentAction::Start => system::start_service("agent"),
        AgentAction::Stop => system::stop_service("agent"),
        AgentAction::Restart => system::restart_service("agent"),
        AgentAction::Status => system::status_service("agent"),
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

    system::install_service("agent", &service_content)
}
