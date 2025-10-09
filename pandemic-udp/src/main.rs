use anyhow::Result;
use clap::Parser;
use pandemic_protocol::{PluginInfo, Request, Response};
use pandemic_common::DaemonClient;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use tokio::net::UdpSocket;
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

async fn register_with_daemon(socket_path: &PathBuf, bind_addr: &SocketAddr) -> Result<()> {
    let mut config = HashMap::new();
    config.insert("bind_address".to_string(), bind_addr.to_string());
    config.insert("protocol".to_string(), "UDP".to_string());
    
    let plugin = PluginInfo {
        name: "pandemic-udp".to_string(),
        description: Some("UDP proxy for pandemic daemon".to_string()),
        config: Some(config),
        registered_at: None,
    };
    
    let request = Request::Register { plugin };
    let response = DaemonClient::send_request(socket_path, &request).await?;
    info!("Registration response: {:?}", response);
    
    Ok(())
}

async fn proxy_request(socket_path: &PathBuf, request_data: &[u8]) -> Result<Vec<u8>> {
    let request: Request = serde_json::from_slice(request_data)?;
    let response = DaemonClient::send_request(socket_path, &request).await?;
    let response_json = serde_json::to_string(&response)?;
    Ok(response_json.into_bytes())
}

async fn run_udp_server(socket_path: PathBuf, bind_addr: SocketAddr) -> Result<()> {
    let udp_socket = UdpSocket::bind(bind_addr).await?;
    info!("UDP proxy listening on {}", bind_addr);
    
    let mut buf = vec![0u8; 4096];
    
    loop {
        match udp_socket.recv_from(&mut buf).await {
            Ok((len, addr)) => {
                let request_data = &buf[..len];
                
                match proxy_request(&socket_path, request_data).await {
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
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();
    
    register_with_daemon(&args.socket_path, &args.bind_addr).await?;
    
    run_udp_server(args.socket_path, args.bind_addr).await?;
    
    Ok(())
}