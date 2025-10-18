use anyhow::Result;
use clap::{Parser, Subcommand};
use pandemic_common::DaemonClient;
use pandemic_protocol::{Request, Response};
use std::path::PathBuf;
use std::process::Command;

#[derive(Parser)]
#[command(name = "pandemic-cli")]
#[command(about = "Management tool for pandemic daemon and infection services")]
struct Args {
    #[arg(long, default_value = "/var/run/pandemic/pandemic.sock")]
    socket_path: PathBuf,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Communicate with the daemon
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },
    /// Manage systemd services
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
}

#[derive(Subcommand)]
enum DaemonAction {
    /// List registered plugins
    List,
    /// Get a specific plugin
    Get {
        /// Plugin name
        name: String,
    },
    /// Deregister a plugin
    Deregister {
        /// Plugin name
        name: String,
    },
    /// Check daemon status
    Status,
}

#[derive(Subcommand)]
enum ServiceAction {
    /// Install a new infection service
    Install {
        /// Service name
        name: String,
        /// Path to infection binary
        binary_path: PathBuf,
    },
    /// Uninstall an infection service
    Uninstall {
        /// Service name
        name: String,
    },
    /// Start an infection service
    Start {
        /// Service name
        name: String,
    },
    /// Stop an infection service
    Stop {
        /// Service name
        name: String,
    },
    /// Restart an infection service
    Restart {
        /// Service name
        name: String,
    },
}

async fn daemon_command(socket_path: &PathBuf, action: DaemonAction) -> Result<()> {
    let request = match action {
        DaemonAction::List => Request::ListPlugins,
        DaemonAction::Get { name } => Request::GetPlugin { name },
        DaemonAction::Deregister { name } => Request::Deregister { name },
        DaemonAction::Status => {
            println!("Daemon is running at {:?}", socket_path);
            return Ok(());
        }
    };

    let response = DaemonClient::send_request(socket_path, &request).await?;
    match response {
        Response::Success { data } => {
            if let Some(data) = data {
                println!("{}", serde_json::to_string_pretty(&data)?);
            } else {
                println!("Success");
            }
        }
        Response::Error { message } => {
            eprintln!("Error: {}", message);
        }
        Response::NotFound { message } => {
            eprintln!("Not Found: {}", message);
        }
    }

    Ok(())
}

fn service_command(action: ServiceAction) -> Result<()> {
    match action {
        ServiceAction::Install { name, binary_path } => {
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

            let service_path = format!("/etc/systemd/system/pandemic-{}.service", name);
            std::fs::write(&service_path, service_content)?;

            Command::new("systemctl").args(["daemon-reload"]).status()?;

            Command::new("systemctl")
                .args(["enable", &format!("pandemic-{}", name)])
                .status()?;

            println!("Installed service: pandemic-{}", name);
        }
        ServiceAction::Uninstall { name } => {
            let service_name = format!("pandemic-{}", name);

            Command::new("systemctl")
                .args(["stop", &service_name])
                .status()?;

            Command::new("systemctl")
                .args(["disable", &service_name])
                .status()?;

            let service_path = format!("/etc/systemd/system/{}.service", service_name);
            std::fs::remove_file(&service_path)?;

            Command::new("systemctl").args(["daemon-reload"]).status()?;

            println!("Uninstalled service: {}", service_name);
        }
        ServiceAction::Start { name } => {
            Command::new("systemctl")
                .args(["start", &format!("pandemic-{}", name)])
                .status()?;
            println!("Started service: pandemic-{}", name);
        }
        ServiceAction::Stop { name } => {
            Command::new("systemctl")
                .args(["stop", &format!("pandemic-{}", name)])
                .status()?;
            println!("Stopped service: pandemic-{}", name);
        }
        ServiceAction::Restart { name } => {
            Command::new("systemctl")
                .args(["restart", &format!("pandemic-{}", name)])
                .status()?;
            println!("Restarted service: pandemic-{}", name);
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    match args.command {
        Commands::Daemon { action } => daemon_command(&args.socket_path, action).await?,
        Commands::Service { action } => service_command(action)?,
    }

    Ok(())
}
