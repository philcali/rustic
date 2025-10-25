use anyhow::Result;
use pandemic_protocol::{Event, Message, Request, Response};
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tracing::info;

pub struct DaemonClient;

pub struct PersistentClient {
    stream: BufReader<UnixStream>,
    event_rx: Option<mpsc::UnboundedReceiver<Event>>,
}

impl DaemonClient {
    /// Send a single request and close connection (for CLI/transient use)
    pub async fn send_request<P: AsRef<Path>>(
        socket_path: P,
        request: &Request,
    ) -> Result<Response> {
        let stream = UnixStream::connect(socket_path).await?;
        let mut reader = BufReader::new(stream);

        let request_json = serde_json::to_string(request)?;
        reader.get_mut().write_all(request_json.as_bytes()).await?;
        reader.get_mut().write_all(b"\n").await?;

        let mut response_line = String::new();
        reader.read_line(&mut response_line).await?;

        let response: Response = serde_json::from_str(&response_line)?;
        Ok(response)
    }

    /// Create a persistent connection (for long-running plugins)
    pub async fn connect<P: AsRef<Path>>(socket_path: P) -> Result<PersistentClient> {
        let stream = UnixStream::connect(socket_path).await?;
        let reader = BufReader::new(stream);

        Ok(PersistentClient {
            stream: reader,
            event_rx: None,
        })
    }
}

impl PersistentClient {
    pub async fn send_request(&mut self, request: &Request) -> Result<Response> {
        let request_json = serde_json::to_string(request)?;
        self.stream
            .get_mut()
            .write_all(request_json.as_bytes())
            .await?;
        self.stream.get_mut().write_all(b"\n").await?;

        let mut response_line = String::new();
        self.stream.read_line(&mut response_line).await?;

        let response: Response = serde_json::from_str(&response_line)?;
        Ok(response)
    }

    /// Subscribe to event topics
    pub async fn subscribe(&mut self, topics: Vec<String>) -> Result<()> {
        let request = Request::Subscribe { topics };
        let _response = self.send_request(&request).await?;
        Ok(())
    }

    /// Read the next event from the stream (blocking)
    pub async fn read_event(&mut self) -> Result<Option<Event>> {
        loop {
            let mut line = String::new();

            match self.stream.read_line(&mut line).await? {
                0 => return Ok(None), // Connection closed
                _ => {
                    if let Ok(Message::Event(event)) = serde_json::from_str::<Message>(line.trim())
                    {
                        return Ok(Some(event));
                    }
                    // Invalid JSON or not an event, continue loop to read next line
                }
            }
        }
    }

    /// Try to receive an event without blocking
    pub async fn try_recv_event(&mut self) -> Option<Event> {
        if let Some(ref mut rx) = self.event_rx {
            rx.try_recv().ok()
        } else {
            None
        }
    }

    /// Wait for the next event
    pub async fn recv_event(&mut self) -> Option<Event> {
        if let Some(ref mut rx) = self.event_rx {
            rx.recv().await
        } else {
            None
        }
    }

    pub async fn register_and_keep_alive(
        &mut self,
        plugin_info: pandemic_protocol::PluginInfo,
    ) -> Result<()> {
        let request = Request::Register {
            plugin: plugin_info,
        };
        let _response = self.send_request(&request).await?;

        // Keep connection alive by reading events
        let mut line = String::new();
        while self.stream.read_line(&mut line).await? > 0 {
            if let Ok(Message::Event(event)) = serde_json::from_str::<Message>(line.trim()) {
                // Handle incoming events (plugins can override this behavior)
                info!("Received event: {:?}", event);
            }
            line.clear();
        }

        Ok(())
    }
}
