use anyhow::Result;
use pandemic_protocol::{Request, Response};
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

pub struct DaemonClient;

impl DaemonClient {
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