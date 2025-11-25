mod connection;
mod daemon;
mod event_bus;
mod handlers;

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::UnixListener;
use tokio::sync::Mutex;
use tracing::{error, info};

use connection::handle_connection;
use daemon::Daemon;

#[derive(Parser)]
#[command(name = "pandemic")]
#[command(about = "Lightweight daemon for managing infection plugins")]
struct Args {
    #[arg(long, default_value = "/var/run/pandemic/pandemic.sock")]
    socket_path: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    if let Some(parent) = args.socket_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let _ = tokio::fs::remove_file(&args.socket_path).await;
    let listener = UnixListener::bind(&args.socket_path)?;
    info!("Pandemic daemon listening on {:?}", args.socket_path);

    let daemon = Arc::new(Mutex::new(Daemon::new()));
    let mut connection_counter = 0u64;

    while let Ok((stream, _)) = listener.accept().await {
        connection_counter += 1;
        let connection_id = format!("conn_{}", connection_counter);

        let event_rx = {
            let mut daemon_guard = daemon.lock().await;
            daemon_guard.add_connection(connection_id.clone())
        };

        let daemon_clone = Arc::clone(&daemon);
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, connection_id, daemon_clone, event_rx).await {
                error!("Connection error: {}", e);
            }
        });
    }

    Ok(())
}
