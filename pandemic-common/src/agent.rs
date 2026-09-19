use anyhow::Result;
use pandemic_protocol::{AgentRequest, Response};

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::net::UnixStream;

use crate::wire;

pub const AGENT_SOCKET_PATH: &str = "/var/run/pandemic/admin.sock";
/// Default location of the agent shared secret, installed by
/// `pandemic-cli bootstrap install --with-agent` / `pandemic-cli agent install`.
pub const AGENT_SECRET_PATH: &str = "/etc/pandemic/agent-secret";
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

    /// Re-probe the agent using an already-configured (secret-bearing) client.
    pub async fn refresh(client: &AgentClient) -> Self {
        match client.ping().await {
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
    secret: String,
}

impl AgentClient {
    pub fn new() -> Self {
        Self {
            socket_path: PathBuf::from(AGENT_SOCKET_PATH),
            secret: String::new(),
        }
    }

    pub fn with_socket_path<P: AsRef<Path>>(path: P) -> Self {
        Self {
            socket_path: path.as_ref().to_path_buf(),
            secret: String::new(),
        }
    }

    pub fn with_secret<S: Into<String>>(mut self, secret: S) -> Self {
        self.secret = secret.into();
        self
    }

    pub fn with_secret_path<P: AsRef<Path>>(mut self, path: P) -> Result<Self> {
        self.secret = std::fs::read_to_string(path)?.trim().to_string();
        Ok(self)
    }

    async fn authenticate(&self, stream: UnixStream) -> Result<UnixStream> {
        wire::authenticate_stream(stream, &self.secret).await
    }

    pub async fn connect(&self) -> Result<UnixStream> {
        let stream = UnixStream::connect(&self.socket_path).await?;
        let authenticated = self.authenticate(stream).await?;
        Ok(authenticated)
    }

    /// Send one `AgentRequest` to the agent and read back the `Response`.
    pub async fn send_agent_request(&self, request: &AgentRequest) -> Result<Response> {
        let stream = self.connect().await?;
        wire::send_request_stream(stream, request).await
    }

    /// Liveness + capability probe: a `GetCapabilities` round-trip.
    pub async fn ping(&self) -> Result<Vec<String>> {
        let response = self
            .send_agent_request(&AgentRequest::GetCapabilities)
            .await?;
        capabilities_from(&response)
    }
}

/// Pull the `capabilities` list out of a `GetCapabilities` response.
///
/// Shared by [`AgentClient`] (local) and [`crate::remote::RemoteClient`] (network)
/// so both agree on the shape.
pub fn capabilities_from(response: &Response) -> Result<Vec<String>> {
    match response {
        Response::Success { data: Some(data) } => {
            if let Some(capabilities) = data.get("capabilities").and_then(|v| v.as_array()) {
                return Ok(capabilities
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect());
            }
            Ok(vec!["systemd".to_string()])
        }
        _ => Err(anyhow::anyhow!("Agent ping failed")),
    }
}

impl Default for AgentClient {
    fn default() -> Self {
        Self::new()
    }
}
