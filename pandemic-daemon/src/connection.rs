use anyhow::Result;
use pandemic_protocol::{Event, Message, Request, Response};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, Mutex};
use tracing::{error, warn};

use crate::daemon::Daemon;

pub async fn handle_connection(
    stream: UnixStream,
    connection_id: String,
    daemon: Arc<Mutex<Daemon>>,
    mut event_rx: mpsc::UnboundedReceiver<Event>,
) -> Result<()> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();

    loop {
        tokio::select! {
            result = reader.read_line(&mut line) => {
                match result {
                    Ok(0) => break,
                    Ok(_) => {
                        let trimmed = line.trim();
                        if !trimmed.is_empty() {
                            let response = {
                                let mut daemon_guard = daemon.lock().await;
                                match serde_json::from_str::<Request>(trimmed) {
                                    Ok(request) => daemon_guard.handle_request(request, &connection_id),
                                    Err(e) => {
                                        warn!("Invalid request: {}", e);
                                        Response::error(format!("Invalid request: {}", e))
                                    }
                                }
                            };

                            let response_json = serde_json::to_string(&response)?;
                            reader.get_mut().write_all(response_json.as_bytes()).await?;
                            reader.get_mut().write_all(b"\n").await?;
                        }
                        line.clear();
                    }
                    Err(e) => {
                        error!("Read error: {}", e);
                        break;
                    }
                }
            }
            event = event_rx.recv() => {
                if let Some(event) = event {
                    let event_json = serde_json::to_string(&Message::Event(event))?;
                    if let Err(e) = reader.get_mut().write_all(event_json.as_bytes()).await {
                        warn!("Failed to send event: {}", e);
                        break;
                    }
                    if let Err(e) = reader.get_mut().write_all(b"\n").await {
                        warn!("Failed to send event newline: {}", e);
                        break;
                    }
                } else {
                    break;
                }
            }
        }
    }

    {
        let mut daemon_guard = daemon.lock().await;
        daemon_guard.remove_connection(&connection_id);
    }

    Ok(())
}
