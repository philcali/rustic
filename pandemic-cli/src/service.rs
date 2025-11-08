use anyhow::Result;
use std::path::Path;
use std::process::Command;

use crate::{system, ServiceAction};

pub fn handle_service_command(action: ServiceAction) -> Result<()> {
    match action {
        ServiceAction::Install { name, binary_path } => install_service(&name, &binary_path),
        ServiceAction::Uninstall { name } => system::uninstall_service(&name),
        ServiceAction::Start { name } => system::start_service(&name),
        ServiceAction::Stop { name } => system::stop_service(&name),
        ServiceAction::Restart { name } => system::restart_service(&name),
        ServiceAction::Status { name } => system::status_service(&name),
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
