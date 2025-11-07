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
    /// Bootstrap pandemic daemon service
    Bootstrap {
        #[command(subcommand)]
        action: BootstrapAction,
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
    /// Get health metrics
    Health,
}

#[derive(Subcommand)]
enum BootstrapAction {
    /// Install pandemic daemon service
    Install {
        /// Path to pandemic daemon binary
        #[arg(long, default_value = "/usr/local/bin/pandemic")]
        binary_path: PathBuf,
    },
    /// Uninstall pandemic daemon service
    Uninstall,
    /// Start pandemic daemon service
    Start,
    /// Stop pandemic daemon service
    Stop,
    /// Restart pandemic daemon service
    Restart,
    /// Show pandemic daemon service status
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
    /// Displays the service status
    Status {
        /// Service name
        name: String,
    },
    /// View service logs
    Logs {
        /// Service name
        name: String,
        /// Follow log output
        #[arg(short, long)]
        follow: bool,
        /// Number of lines to show
        #[arg(short, long, default_value = "50")]
        lines: u32,
    },
    /// Configure service arguments
    Config {
        /// Service name
        name: String,
        /// Show current configuration
        #[arg(long)]
        show: bool,
        /// Reset to default configuration
        #[arg(long)]
        reset: bool,
        /// Custom arguments to pass to the service
        #[arg(last = true)]
        args: Vec<String>,
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
        DaemonAction::Health => Request::GetHealth,
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
        ServiceAction::Status { name } => {
            Command::new("systemctl")
                .args(["status", &format!("pandemic-{}", name)])
                .status()?;
        }
        ServiceAction::Logs {
            name,
            follow,
            lines,
        } => logs_command(&name, follow, lines)?,
        ServiceAction::Config {
            name,
            show,
            reset,
            args,
        } => service_config_command(&name, show, reset, args)?,
    }
    Ok(())
}

fn bootstrap_command(action: BootstrapAction) -> Result<()> {
    match action {
        BootstrapAction::Install { binary_path } => {
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

            std::fs::write("/etc/systemd/system/pandemic.service", service_content)?;
            Command::new("systemctl").args(["daemon-reload"]).status()?;
            Command::new("systemctl")
                .args(["enable", "pandemic"])
                .status()?;
            println!("Installed pandemic daemon service");
        }
        BootstrapAction::Uninstall => {
            Command::new("systemctl")
                .args(["stop", "pandemic"])
                .status()?;
            Command::new("systemctl")
                .args(["disable", "pandemic"])
                .status()?;
            std::fs::remove_file("/etc/systemd/system/pandemic.service")?;
            Command::new("systemctl").args(["daemon-reload"]).status()?;
            println!("Uninstalled pandemic daemon service");
        }
        BootstrapAction::Start => {
            Command::new("systemctl")
                .args(["start", "pandemic"])
                .status()?;
            println!("Started pandemic daemon service");
        }
        BootstrapAction::Stop => {
            Command::new("systemctl")
                .args(["stop", "pandemic"])
                .status()?;
            println!("Stopped pandemic daemon service");
        }
        BootstrapAction::Restart => {
            Command::new("systemctl")
                .args(["restart", "pandemic"])
                .status()?;
            println!("Restarted pandemic daemon service");
        }
        BootstrapAction::Status => {
            Command::new("systemctl")
                .args(["status", "pandemic"])
                .status()?;
        }
    }
    Ok(())
}

fn logs_command(service: &str, follow: bool, lines: u32) -> Result<()> {
    let service_name = if service.starts_with("pandemic") {
        service.to_string()
    } else {
        format!("pandemic-{}", service)
    };

    let mut cmd = Command::new("journalctl");
    cmd.args(["-u", &service_name, "-n", &lines.to_string()]);

    if follow {
        cmd.arg("-f");
    }

    cmd.status()?;
    Ok(())
}

fn service_config_command(name: &str, show: bool, reset: bool, args: Vec<String>) -> Result<()> {
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

    // Create override configuration
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

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    match args.command {
        Commands::Daemon { action } => daemon_command(&args.socket_path, action).await?,
        Commands::Service { action } => service_command(action)?,
        Commands::Bootstrap { action } => bootstrap_command(action)?,
    }

    Ok(())
}
