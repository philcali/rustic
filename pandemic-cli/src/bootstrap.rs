use anyhow::Result;
use std::path::Path;

use crate::{agent, system, BootstrapAction};

pub fn handle_bootstrap_command(action: BootstrapAction) -> Result<()> {
    match action {
        BootstrapAction::Install {
            binary_path,
            with_agent,
        } => install_daemon(&binary_path, with_agent),
        BootstrapAction::Uninstall => system::uninstall_service("pandemic"),
        BootstrapAction::Start => system::start_service("pandemic"),
        BootstrapAction::Stop => system::stop_service("pandemic"),
        BootstrapAction::Restart => system::restart_service("pandemic"),
        BootstrapAction::Status => system::status_service("pandemic"),
    }
}

fn install_daemon(binary_path: &Path, with_agent: bool) -> Result<()> {
    let service_content = format!(
        r#"[Unit]
Description=Pandemic Daemon
After=network.target

[Service]
Type=simple
ExecStart={}
Restart=always
RestartSec=5
User=pandemic
Group=pandemic
RuntimeDirectory=pandemic
RuntimeDirectoryMode=0755

[Install]
WantedBy=multi-user.target
"#,
        binary_path.display()
    );

    system::install_service("pandemic", &service_content)?;

    if with_agent {
        agent::install_agent(Path::new("/usr/local/bin/pandemic-agent"))?;
    }

    Ok(())
}
