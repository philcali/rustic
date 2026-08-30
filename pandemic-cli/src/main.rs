mod agent;
mod bootstrap;
mod daemon;
mod registry;
mod secret;
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
    /// Manage pandemic agent service
    Agent {
        #[command(subcommand)]
        action: AgentAction,
    },
    /// Search and install infections from registry
    Registry {
        #[command(subcommand)]
        action: RegistryAction,
    },
}

#[derive(Subcommand)]
enum RegistryAction {
    /// Search for infections in the registry
    Search {
        /// Search query
        query: String,
        /// Registry URL to use
        #[arg(long)]
        registry_url: Option<String>,
    },
    /// Get infection manifest details
    Get {
        /// Infection name
        name: String,
        /// Registry URL to use
        #[arg(long)]
        registry_url: Option<String>,
    },
    /// Install an infection from the registry
    Install {
        /// Infection name
        name: String,
        /// Registry URL to use
        #[arg(long)]
        registry_url: Option<String>,
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
enum AgentAction {
    /// Install pandemic agent service
    Install {
        /// Path to pandemic agent binary
        #[arg(long, default_value = "/usr/local/bin/pandemic-agent")]
        binary_path: PathBuf,
    },
    /// Uninstall pandemic agent service
    Uninstall,
    /// Start pandemic agent service
    Start,
    /// Stop pandemic agent service
    Stop,
    /// Restart pandemic agent service
    Restart,
    /// Show pandemic agent service status
    Status,
    /// Send a raw AgentRequest as JSON (dev/e2e aid, e.g. `{"type":"GetCapabilities"}`)
    Request {
        /// AgentRequest JSON, tagged with "type"
        json: String,
        /// agent shared secret (overrides the default path)
        #[arg(long)]
        agent_secret: Option<String>,
        /// path to the agent shared secret
        #[arg(long)]
        agent_secret_path: Option<PathBuf>,
    },
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
    /// Attach an existing systemd unit as an infection (via pandemic-agent)
    Attach {
        /// systemd unit to attach (e.g. mosquitto)
        unit: String,
        /// infection name (defaults to the unit's base name)
        #[arg(long)]
        name: Option<String>,
        /// infection version recorded in the daemon
        #[arg(long)]
        version: Option<String>,
        /// description of the infection
        #[arg(long)]
        description: Option<String>,
        /// health check interval in seconds
        #[arg(long)]
        health_interval: Option<u64>,
        /// agent shared secret (overrides the default path)
        #[arg(long)]
        agent_secret: Option<String>,
        /// path to the agent shared secret
        #[arg(long)]
        agent_secret_path: Option<PathBuf>,
    },
    /// Detach a previously attached infection (via pandemic-agent)
    Detach {
        /// infection name as created by `service attach`
        name: String,
        /// agent shared secret (overrides the default path)
        #[arg(long)]
        agent_secret: Option<String>,
        /// path to the agent shared secret
        #[arg(long)]
        agent_secret_path: Option<PathBuf>,
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
        Commands::Service { action } => service::handle_service_command(action).await?,
        Commands::Bootstrap { action } => bootstrap::handle_bootstrap_command(action)?,
        Commands::Agent { action } => agent::handle_agent_command(action).await?,
        Commands::Registry { action } => {
            registry::handle_registry_command(&args.socket_path, action).await?
        }
    }

    Ok(())
}
