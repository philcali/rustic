use anyhow::Result;
use pandemic_protocol::{Event, Message, Request, Response};
use std::collections::VecDeque;
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;

pub struct DaemonClient;

pub struct PersistentClient {
    stream: BufReader<UnixStream>,
    event_rx: Option<mpsc::UnboundedReceiver<Event>>,
    /// Events the daemon interleaved on the stream while we awaited a response
    pending_events: VecDeque<Event>,
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
            pending_events: VecDeque::new(),
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

        // The daemon may push event lines onto this connection at any time
        // (e.g. the auto-emitted `plugin.registered` event), interleaved
        // with our response. Buffer any events and keep reading until the
        // line that is actually our response arrives.
        loop {
            let mut line = String::new();
            self.stream.read_line(&mut line).await?;

            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(response) = serde_json::from_str::<Response>(trimmed) {
                return Ok(response);
            }
            if let Ok(Message::Event(event)) = serde_json::from_str::<Message>(trimmed) {
                self.pending_events.push_back(event);
            }
        }
    }

    /// Subscribe to event topics
    pub async fn subscribe(&mut self, topics: Vec<String>) -> Result<()> {
        let request = Request::Subscribe { topics };
        let _response = self.send_request(&request).await?;
        Ok(())
    }

    /// Read the next event from the stream (blocking)
    pub async fn read_event(&mut self) -> Result<Option<Event>> {
        // Events captured while awaiting responses are delivered first
        if let Some(event) = self.pending_events.pop_front() {
            return Ok(Some(event));
        }

        loop {
            let mut line = String::new();

            match self.stream.read_line(&mut line).await? {
                0 => return Ok(None), // Connection closed
                _ => {
                    if let Ok(Message::Event(event)) = serde_json::from_str::<Message>(line.trim())
                    {
                        return Ok(Some(event));
                    }
                    // Not an event (e.g. a stray response line), keep reading
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
}

impl Drop for PersistentClient {
    fn drop(&mut self) {
        // The UnixStream will be automatically closed when dropped,
        // which will signal the daemon to clean up this connection
        tracing::info!("PersistentClient connection dropped");
    }
}
