use anyhow::Result;
use pandemic_protocol::{AgentMessage, AgentRequest, Response};

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

const AGENT_SOCKET_PATH: &str = "/var/run/pandemic/admin.sock";
const CACHE_DURATION: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct AgentStatus {
    pub available: bool,
    pub capabilities: Vec<String>,
    last_check: Instant,
}

impl AgentStatus {
    pub fn new() -> Self {
        Self {
            available: false,
            capabilities: Vec::new(),
            last_check: Instant::now() - CACHE_DURATION,
        }
    }

    pub fn is_stale(&self) -> bool {
        self.last_check.elapsed() > CACHE_DURATION
    }

    pub async fn refresh() -> Self {
        match AgentClient::new().ping().await {
            Ok(capabilities) => Self {
                available: true,
                capabilities,
                last_check: Instant::now(),
            },
            Err(_) => Self {
                available: false,
                capabilities: Vec::new(),
                last_check: Instant::now(),
            },
        }
    }
}

impl Default for AgentStatus {
    fn default() -> Self {
        Self::new()
    }
}

pub struct AgentClient {
    socket_path: PathBuf,
}

impl AgentClient {
    pub fn new() -> Self {
        Self {
            socket_path: PathBuf::from(AGENT_SOCKET_PATH),
        }
    }

    pub fn with_socket_path<P: AsRef<Path>>(path: P) -> Self {
        Self {
            socket_path: path.as_ref().to_path_buf(),
        }
    }

    pub async fn connect(&self) -> Result<UnixStream> {
        let stream = UnixStream::connect(&self.socket_path).await?;
        Ok(stream)
    }

    pub async fn send_agent_request(&self, request: &AgentRequest) -> Result<Response> {
        let stream = self.connect().await?;
        let mut buf_reader = BufReader::new(stream);

        let message = AgentMessage::Request(request.clone());
        let request_json = serde_json::to_string(&message)?;
        buf_reader
            .get_mut()
            .write_all(request_json.as_bytes())
            .await?;
        buf_reader.get_mut().write_all(b"\n").await?;

        let mut response_line = String::new();
        buf_reader.read_line(&mut response_line).await?;

        let response: Response = serde_json::from_str(response_line.trim())?;
        Ok(response)
    }

    pub async fn ping(&self) -> Result<Vec<String>> {
        let request = AgentRequest::GetCapabilities;
        let response = self.send_agent_request(&request).await?;

        match response {
            Response::Success { data: Some(data) } => {
                if let Some(capabilities) = data.get("capabilities") {
                    if let Some(caps_array) = capabilities.as_array() {
                        let caps: Vec<String> = caps_array
                            .iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect();
                        return Ok(caps);
                    }
                }
                Ok(vec!["systemd".to_string()])
            }
            _ => Err(anyhow::anyhow!("Agent ping failed")),
        }
    }
}

impl Default for AgentClient {
    fn default() -> Self {
        Self::new()
    }
}
