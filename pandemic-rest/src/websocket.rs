use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    response::{IntoResponse, Response},
};
use futures_util::{sink::SinkExt, stream::StreamExt};

use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::handlers::AppState;

#[derive(Deserialize)]
pub struct WebSocketQuery {
    token: Option<String>,
    topics: Option<String>, // Comma-separated topics like "plugin.*,health.*"
}

pub async fn websocket_handler(
    ws: WebSocketUpgrade,
    Query(params): Query<WebSocketQuery>,
    State(state): State<AppState>,
) -> Response {
    // Authenticate using token from query params
    let api_key = match params.token {
        Some(token) => token,
        None => {
            error!("WebSocket upgrade failed: missing token");
            return axum::http::Response::builder()
                .status(401)
                .body(axum::body::Body::from("Missing token"))
                .unwrap()
                .into_response();
        }
    };

    let scopes = match state.auth_config.authenticate(&api_key) {
        Some(scopes) => scopes,
        None => {
            error!("WebSocket upgrade failed: invalid token");
            return axum::http::Response::builder()
                .status(401)
                .body(axum::body::Body::from("Invalid token"))
                .unwrap()
                .into_response();
        }
    };

    // Check if user has events:subscribe scope
    if !state.auth_config.authorize(&scopes, "events:subscribe") {
        error!("WebSocket upgrade failed: insufficient permissions");
        return axum::http::Response::builder()
            .status(403)
            .body(axum::body::Body::from("Insufficient permissions"))
            .unwrap()
            .into_response();
    }

    // Parse topics filter
    let topics: Vec<String> = params
        .topics
        .unwrap_or_else(|| "*".to_string())
        .split(',')
        .map(|s| s.trim().to_string())
        .collect();

    info!("WebSocket connection established with topics: {:?}", topics);

    ws.on_upgrade(move |socket| handle_websocket(socket, state, topics))
}

async fn handle_websocket(socket: WebSocket, state: AppState, topics: Vec<String>) {
    let (mut sender, mut receiver) = socket.split();

    // Create a persistent connection to the daemon
    let mut daemon_client = match pandemic_common::DaemonClient::connect(&state.socket_path).await {
        Ok(client) => client,
        Err(e) => {
            error!("Failed to connect to daemon: {}", e);
            let _ = sender
                .send(Message::Text(
                    json!({
                        "type": "error",
                        "message": format!("Failed to connect to daemon: {}", e)
                    })
                    .to_string(),
                ))
                .await;
            return;
        }
    };

    // Subscribe to topics
    if let Err(e) = daemon_client.subscribe(topics.clone()).await {
        error!("Failed to subscribe to topics: {}", e);
        let _ = sender
            .send(Message::Text(
                json!({
                    "type": "error",
                    "message": format!("Failed to subscribe to topics: {}", e)
                })
                .to_string(),
            ))
            .await;
        return;
    }

    info!("Subscribed to topics: {:?}", topics);

    // Send connection success message
    let _ = sender
        .send(Message::Text(
            json!({
                "type": "connected",
                "topics": topics
            })
            .to_string(),
        ))
        .await;

    // Create channels for handling WebSocket messages and daemon events
    let (ws_tx, mut ws_rx) = mpsc::unbounded_channel::<Message>();

    // Task to handle incoming WebSocket messages (for future subscription management)
    let ws_sender = ws_tx.clone();
    tokio::spawn(async move {
        while let Some(msg) = receiver.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    // Handle subscription management messages
                    if let Ok(request) = serde_json::from_str::<serde_json::Value>(&text) {
                        info!("Received WebSocket message: {}", request);
                        // Future: handle subscribe/unsubscribe requests
                    }
                }
                Ok(Message::Close(_)) => {
                    info!("WebSocket connection closed by client");
                    break;
                }
                Ok(Message::Ping(data)) => {
                    let _ = ws_sender.send(Message::Pong(data));
                }
                Err(e) => {
                    warn!("WebSocket error: {}", e);
                    break;
                }
                _ => {}
            }
        }
    });

    // Task to read events from daemon and forward to WebSocket
    let ws_sender = ws_tx.clone();
    tokio::spawn(async move {
        loop {
            match daemon_client.read_event().await {
                Ok(Some(event)) => {
                    let message = json!({
                        "type": "event",
                        "data": event
                    });

                    if ws_sender.send(Message::Text(message.to_string())).is_err() {
                        info!("WebSocket channel closed, stopping event forwarding");
                        break;
                    }
                }
                Ok(None) => {
                    info!("Daemon connection closed");
                    let _ = ws_sender.send(Message::Text(
                        json!({
                            "type": "error",
                            "message": "Daemon connection closed"
                        })
                        .to_string(),
                    ));
                    break;
                }
                Err(e) => {
                    error!("Error reading event from daemon: {}", e);
                    let _ = ws_sender.send(Message::Text(
                        json!({
                            "type": "error",
                            "message": format!("Error reading events: {}", e)
                        })
                        .to_string(),
                    ));
                    break;
                }
            }
        }
    });

    // Main loop to send messages to WebSocket client
    while let Some(message) = ws_rx.recv().await {
        if sender.send(message).await.is_err() {
            info!("WebSocket connection closed");
            break;
        }
    }

    info!("WebSocket handler finished");
}
