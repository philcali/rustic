//! Test harness for daemon integration tests.
//!
//! This module provides a minimal harness that starts the real daemon in-process
//! and exposes only the Unix socket boundary. Tests interact with the daemon
//! exclusively through the protocol — no internal channel tapping.

use anyhow::Result;
use pandemic_protocol::{Event, Message, Request, Response};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, Mutex};
use tracing::info;

/// A test harness that runs the real daemon accept loop in-process.
///
/// Tests interact with the daemon only through the Unix socket —
/// no internal state inspection.
pub struct TestHarness {
    _temp_dir: TempDir,
    pub socket_path: PathBuf,
    shutdown_tx: Option<mpsc::Sender<()>>,
}

impl TestHarness {
    /// Create a new harness with a temporary socket path and start the daemon loop.
    pub async fn new() -> Result<Self> {
        Self::with_persistent(false).await
    }

    /// Create a new harness with persistent connections enabled.
    /// Persistent connections trigger plugin deregistration when dropped.
    pub async fn with_persistent(persistent: bool) -> Result<Self> {
        let temp_dir = TempDir::new()?;
        let socket_path = temp_dir.path().join("test_daemon.sock");

        let daemon = Arc::new(Mutex::new(crate::daemon::Daemon::new()));
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel(1);

        // Bind the listener
        let _ = std::fs::remove_file(&socket_path).ok();
        let listener = UnixListener::bind(&socket_path)?;

        let daemon_clone = Arc::clone(&daemon);
        let mut connection_counter: u64 = 0;

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    result = listener.accept() => {
                        match result {
                            Ok((stream, _)) => {
                                connection_counter += 1;
                                let connection_id = format!("conn_{}", connection_counter);
                                let event_rx = {
                                    let mut guard = daemon_clone.lock().await;
                                    guard.add_connection(connection_id.clone(), persistent)
                                };
                                let daemon_inner = Arc::clone(&daemon_clone);
                                tokio::spawn(async move {
                                    if let Err(e) =
                                        crate::connection::handle_connection(
                                            stream, connection_id, daemon_inner, event_rx,
                                        )
                                        .await
                                    {
                                        tracing::warn!("Connection error: {}", e);
                                    }
                                });
                            }
                            Err(e) => {
                                tracing::error!("Accept error: {}", e);
                                break;
                            }
                        }
                    }
                    _ = shutdown_rx.recv() => {
                        info!("Test harness shutting down");
                        break;
                    }
                }
            }
        });

        // Give the listener time to bind
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        Ok(Self {
            _temp_dir: temp_dir,
            socket_path,
            shutdown_tx: Some(shutdown_tx),
        })
    }

    /// Connect to the daemon and send a single request, returning the response.
    /// The connection is dropped immediately after the response is received.
    pub async fn send_request(&self, request: &Request) -> Result<Response> {
        let mut conn = self.connect().await?;
        conn.send_request(request).await
    }

    /// Connect to the daemon and return a handle for the connection.
    pub async fn connect(&self) -> Result<PluginConnection> {
        PluginConnection::connect(&self.socket_path).await
    }

    /// Shutdown the harness.
    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(()).await;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
}

/// A persistent plugin connection that can send requests and read events.
///
/// Events arrive on the same stream as responses (line-delimited JSON).
/// Use `read_event()` to pull events from the stream.
pub struct PluginConnection {
    reader: BufReader<UnixStream>,
    line: String,
}

impl PluginConnection {
    /// Connect to the daemon at the given socket path.
    pub async fn connect(socket_path: &Path) -> Result<Self> {
        let stream = UnixStream::connect(socket_path).await?;
        let reader = BufReader::new(stream);
        Ok(Self {
            reader,
            line: String::new(),
        })
    }

    /// Send a request and read the response.
    /// Skips any events that arrive before the response.
    pub async fn send_request(&mut self, request: &Request) -> Result<Response> {
        let request_json = serde_json::to_string(request)?;
        self.reader
            .get_mut()
            .write_all(request_json.as_bytes())
            .await?;
        self.reader.get_mut().write_all(b"\n").await?;
        self.reader.get_mut().flush().await?;

        // Read response, skipping any events that may have arrived
        loop {
            self.line.clear();
            self.reader.read_line(&mut self.line).await?;
            let trimmed = self.line.trim();
            // Try to parse as a Message first (events, etc.)
            if let Ok(message) = serde_json::from_str::<Message>(trimmed) {
                if let Message::Response(response) = message {
                    return Ok(response);
                }
                // It's an event or request message, skip it
                continue;
            }
            // Not a Message, try as a plain Response
            if let Ok(response) = serde_json::from_str::<Response>(trimmed) {
                return Ok(response);
            }
            return Err(anyhow::anyhow!("Failed to parse response: {}", trimmed));
        }
    }

    /// Read the next event from the stream, waiting up to `timeout_ms` milliseconds.
    ///
    /// Events are line-delimited JSON `Message::Event` messages sent by the daemon.
    /// Returns `Ok(Some(event))` if an event was received, `Ok(None)` on timeout.
    pub async fn read_event(&mut self, timeout_ms: u64) -> Result<Option<Event>> {
        let result = tokio::time::timeout(
            tokio::time::Duration::from_millis(timeout_ms),
            self._read_line(),
        )
        .await;

        match result {
            Ok(Ok(true)) => {
                // Parse as Message
                let message: Message = serde_json::from_str(self.line.trim())?;
                if let Message::Event(event) = message {
                    Ok(Some(event))
                } else {
                    // Could be a Response message — return None to let the caller retry
                    Ok(None)
                }
            }
            Ok(Ok(false)) => Ok(None), // Timeout
            Ok(Err(_)) => Ok(None),    // EOF or error
            Err(_) => Ok(None),
        }
    }

    /// Read a line from the stream. Returns (success, is_event).
    async fn _read_line(&mut self) -> Result<bool> {
        self.line.clear();
        match self.reader.read_line(&mut self.line).await {
            Ok(0) => Ok(false), // EOF
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }

    /// Close the connection, triggering deregistration.
    pub async fn close(&mut self) -> Result<()> {
        self.reader.get_mut().shutdown().await?;
        Ok(())
    }
}

/// Helper to register a plugin and return a persistent connection.
pub async fn register_plugin(
    harness: &TestHarness,
    name: &str,
    version: &str,
) -> Result<PluginConnection> {
    let mut conn = harness.connect().await?;
    let plugin = pandemic_protocol::PluginInfo {
        name: name.to_string(),
        version: version.to_string(),
        description: None,
        config: None,
        registered_at: None,
    };
    let response = conn.send_request(&Request::Register { plugin }).await?;
    assert!(
        matches!(response, Response::Success { .. }),
        "Plugin registration failed for {}: {:?}",
        name,
        response
    );
    Ok(conn)
}

/// Helper to assert that an event was received within a timeout.
pub async fn assert_event_received(
    event_rx: &mut mpsc::UnboundedReceiver<Event>,
    topic: &str,
    timeout_ms: u64,
) -> Option<Event> {
    tokio::time::timeout(
        tokio::time::Duration::from_millis(timeout_ms),
        event_rx.recv(),
    )
    .await
    .ok()
    .flatten()
    .filter(|e| e.topic == topic)
}

/// Helper to assert no event was received within a timeout.
pub async fn assert_no_event(event_rx: &mut mpsc::UnboundedReceiver<Event>, timeout_ms: u64) {
    let result = tokio::time::timeout(
        tokio::time::Duration::from_millis(timeout_ms),
        event_rx.recv(),
    )
    .await;
    assert!(
        result.is_err(),
        "Expected no event within {}ms, but got {:?}",
        timeout_ms,
        result.ok().flatten()
    );
}
