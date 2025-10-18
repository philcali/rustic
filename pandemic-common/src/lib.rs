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
    pub async fn send_request<P: AsRef<Path>>(socket_path: P, request: &Request) -> Result<Response> {
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
        self.stream.get_mut().write_all(request_json.as_bytes()).await?;
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
                    if let Ok(message) = serde_json::from_str::<Message>(&line.trim()) {
                        if let Message::Event(event) = message {
                            return Ok(Some(event));
                        }
                        // Not an event, continue loop to read next line
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

    pub async fn register_and_keep_alive(&mut self, plugin_info: pandemic_protocol::PluginInfo) -> Result<()> {
        let request = Request::Register { plugin: plugin_info };
        let _response = self.send_request(&request).await?;

        // Keep connection alive by reading events
        let mut line = String::new();
        while self.stream.read_line(&mut line).await? > 0 {
            if let Ok(message) = serde_json::from_str::<Message>(&line.trim()) {
                match message {
                    Message::Event(event) => {
                        // Handle incoming events (plugins can override this behavior)
                        info!("Received event: {:?}", event);
                    }
                    _ => {}
                }
            }
            line.clear();
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pandemic_protocol::{PluginInfo, Request, Response};
    use std::collections::HashMap;
    use tempfile::TempDir;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    async fn mock_daemon_server(socket_path: String) {
        let _ = std::fs::remove_file(&socket_path);
        let listener = UnixListener::bind(&socket_path).unwrap();
        
        if let Ok((stream, _)) = listener.accept().await {
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            
            if reader.read_line(&mut line).await.unwrap() > 0 {
                let request: Request = serde_json::from_str(line.trim()).unwrap();
                
                let response = match request {
                    Request::ListPlugins => Response::success_with_data(serde_json::json!([])),
                    Request::GetPlugin { name } => {
                        if name == "test-plugin" {
                            let plugin = PluginInfo {
                                name: "test-plugin".to_string(),
                                version: "1.0.0".to_string(),
                                description: Some("Test plugin".to_string()),
                                config: None,
                                registered_at: None,
                            };
                            Response::success_with_data(serde_json::json!(plugin))
                        } else {
                            Response::not_found("Plugin not found")
                        }
                    }
                    Request::Register { .. } => Response::success(),
                    Request::Deregister { name } => {
                        if name == "test-plugin" {
                            Response::success()
                        } else {
                            Response::not_found("Plugin not found")
                        }
                    }
                    Request::Publish { .. } => Response::success(),
                    Request::Unsubscribe { .. } => Response::success(),
                    Request::Subscribe { .. } => Response::success(),
                };
                
                let response_json = serde_json::to_string(&response).unwrap();
                reader.get_mut().write_all(response_json.as_bytes()).await.unwrap();
                reader.get_mut().write_all(b"\n").await.unwrap();
            }
        }
    }

    #[tokio::test]
    async fn test_list_plugins() {
        let temp_dir = TempDir::new().unwrap();
        let socket_path = temp_dir.path().join(format!("test_{}.sock", COUNTER.fetch_add(1, Ordering::SeqCst)));
        let socket_path_str = socket_path.to_str().unwrap();
        
        tokio::spawn(mock_daemon_server(socket_path_str.to_string()));
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        
        let request = Request::ListPlugins;
        let response = DaemonClient::send_request(&socket_path, &request).await.unwrap();
        
        match response {
            Response::Success { data } => assert!(data.is_some()),
            _ => panic!("Expected success response"),
        }
    }

    #[tokio::test]
    async fn test_get_existing_plugin() {
        let temp_dir = TempDir::new().unwrap();
        let socket_path = temp_dir.path().join(format!("test_{}.sock", COUNTER.fetch_add(1, Ordering::SeqCst)));
        let socket_path_str = socket_path.to_str().unwrap();
        
        tokio::spawn(mock_daemon_server(socket_path_str.to_string()));
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        
        let request = Request::GetPlugin { name: "test-plugin".to_string() };
        let response = DaemonClient::send_request(&socket_path, &request).await.unwrap();
        
        match response {
            Response::Success { data } => assert!(data.is_some()),
            _ => panic!("Expected success response"),
        }
    }

    #[tokio::test]
    async fn test_get_nonexistent_plugin() {
        let temp_dir = TempDir::new().unwrap();
        let socket_path = temp_dir.path().join(format!("test_{}.sock", COUNTER.fetch_add(1, Ordering::SeqCst)));
        let socket_path_str = socket_path.to_str().unwrap();
        
        tokio::spawn(mock_daemon_server(socket_path_str.to_string()));
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        
        let request = Request::GetPlugin { name: "nonexistent".to_string() };
        let response = DaemonClient::send_request(&socket_path, &request).await.unwrap();
        
        match response {
            Response::NotFound { .. } => {},
            _ => panic!("Expected not found response"),
        }
    }

    #[tokio::test]
    async fn test_register_plugin() {
        let temp_dir = TempDir::new().unwrap();
        let socket_path = temp_dir.path().join(format!("test_{}.sock", COUNTER.fetch_add(1, Ordering::SeqCst)));
        let socket_path_str = socket_path.to_str().unwrap();
        
        tokio::spawn(mock_daemon_server(socket_path_str.to_string()));
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        
        let plugin = PluginInfo {
            name: "test-plugin".to_string(),
            version: "1.0.0".to_string(),
            description: Some("Test plugin".to_string()),
            config: Some(HashMap::new()),
            registered_at: None,
        };
        
        let request = Request::Register { plugin };
        let response = DaemonClient::send_request(&socket_path, &request).await.unwrap();
        
        match response {
            Response::Success { .. } => {},
            _ => panic!("Expected success response"),
        }
    }
}