use anyhow::Result;
use clap::Parser;
use pandemic_common::{DaemonClient, PersistentClient};
use pandemic_protocol::{PluginInfo, Request, Response};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, Mutex};
use tracing::{error, info, warn};

#[derive(Parser)]
#[command(name = "pandemic-udp")]
#[command(about = "UDP proxy for pandemic daemon")]
struct Args {
    #[arg(long, default_value = "/var/run/pandemic/pandemic.sock")]
    socket_path: PathBuf,

    #[arg(long, default_value = "0.0.0.0:8080")]
    bind_addr: SocketAddr,
}

async fn create_persistent_client(
    socket_path: &PathBuf,
    bind_addr: &SocketAddr,
) -> Result<PersistentClient> {
    let mut config = HashMap::new();
    config.insert("bind_address".to_string(), bind_addr.to_string());
    config.insert("protocol".to_string(), "UDP".to_string());

    let plugin = PluginInfo {
        name: "pandemic-udp".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        description: Some("UDP proxy for pandemic daemon".to_string()),
        config: Some(config),
        registered_at: None,
    };

    let mut client = DaemonClient::connect(socket_path).await?;
    let request = Request::Register { plugin };
    let response = client.send_request(&request).await?;
    info!("Registration response: {:?}", response);

    // Subscribe to plugin deregister events
    client
        .subscribe(vec!["plugin.deregistered".to_string()])
        .await?;

    Ok(client)
}

async fn proxy_request(
    client: &Arc<Mutex<PersistentClient>>,
    request_data: &[u8],
) -> Result<Vec<u8>> {
    let request: Request = serde_json::from_slice(request_data)?;
    let response = {
        let mut client_guard = client.lock().await;
        client_guard.send_request(&request).await?
    };
    let response_json = serde_json::to_string(&response)?;
    Ok(response_json.into_bytes())
}

async fn run_udp_server(
    client: Arc<Mutex<PersistentClient>>,
    bind_addr: SocketAddr,
    mut shutdown_rx: mpsc::Receiver<()>,
) -> Result<()> {
    let udp_socket = UdpSocket::bind(bind_addr).await?;
    info!("UDP proxy listening on {}", bind_addr);

    let mut buf = vec![0u8; 4096];

    loop {
        tokio::select! {
            // Handle UDP requests
            result = udp_socket.recv_from(&mut buf) => {
                match result {
                    Ok((len, addr)) => {
                        let request_data = &buf[..len];

                        match proxy_request(&client, request_data).await {
                            Ok(response) => {
                                if let Err(e) = udp_socket.send_to(&response, addr).await {
                                    error!("Failed to send UDP response to {}: {}", addr, e);
                                }
                            }
                            Err(e) => {
                                warn!("Proxy request failed: {}", e);
                                let error_response = serde_json::to_string(&Response::error(format!("Proxy error: {}", e)))?;
                                if let Err(e) = udp_socket.send_to(error_response.as_bytes(), addr).await {
                                    error!("Failed to send error response to {}: {}", addr, e);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        error!("UDP receive error: {}", e);
                    }
                }
            }
            // Handle shutdown signal
            _ = shutdown_rx.recv() => {
                info!("Received shutdown signal, stopping UDP server");
                break;
            }
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    // Create persistent connection and register
    let client = create_persistent_client(&args.socket_path, &args.bind_addr).await?;
    let client = Arc::new(Mutex::new(client));

    info!("UDP proxy registered and maintaining connection to daemon");

    // Create shutdown channel
    let (shutdown_tx, shutdown_rx) = mpsc::channel(1);

    // Spawn task to monitor for deregister events
    let client_clone = Arc::clone(&client);
    tokio::spawn(async move {
        info!("Monitoring for deregister events");
        loop {
            let event_result = {
                let mut client_guard = client_clone.lock().await;
                client_guard.read_event().await
            };

            match event_result {
                Ok(Some(event)) => {
                    info!("Received event: {}", event.topic);
                    if event.topic == "plugin.deregistered" {
                        if let Some(data) = event.data.as_object() {
                            if let Some(name) = data.get("name").and_then(|v| v.as_str()) {
                                if name == "pandemic-udp" {
                                    info!("Received deregister event for pandemic-udp, initiating shutdown");
                                    let _ = shutdown_tx.send(()).await;
                                    break;
                                }
                            }
                        }
                    }
                }
                Ok(None) => {
                    info!("Connection closed, shutting down");
                    let _ = shutdown_tx.send(()).await;
                    break;
                }
                Err(e) => {
                    error!("Error reading event: {:?}", e);
                    break;
                }
            }
        }
    });

    // Run UDP server with persistent daemon connection
    run_udp_server(client, args.bind_addr, shutdown_rx).await?;

    info!("UDP proxy shutdown complete");
    Ok(())
}
