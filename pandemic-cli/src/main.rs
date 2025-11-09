mod bootstrap;
mod daemon;
mod service;
mod system;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

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
        /// Also install pandemic-agent for admin operations
        #[arg(long)]
        with_agent: bool,
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

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    match args.command {
        Commands::Daemon { action } => {
            daemon::handle_daemon_command(&args.socket_path, action).await?
        }
        Commands::Service { action } => service::handle_service_command(action)?,
        Commands::Bootstrap { action } => bootstrap::handle_bootstrap_command(action)?,
    }

    Ok(())
}
