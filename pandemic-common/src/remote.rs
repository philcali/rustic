//! A network client for the agent protocol over TCP.
//!
//! [`AgentClient`] speaks to the local Unix admin socket; [`RemoteClient`]
//! speaks the identical handshake + request/response framing to a
//! [`pandemic-node`] receiver across the network. The only difference is the
//! transport — the protocol, the auth, and the allowlist of valid requests all
//! come from the shared modules in this crate.

use anyhow::Result;
use pandemic_protocol::{AgentRequest, Response};
use tokio::net::TcpStream;

use crate::{agent, wire};

/// A node endpoint (`host:port`) plus the epidemic (network) shared secret.
pub struct RemoteClient {
    addr: String,
    secret: String,
}

impl RemoteClient {
    pub fn new(addr: impl Into<String>, secret: impl Into<String>) -> Self {
        Self {
            addr: addr.into(),
            secret: secret.into(),
        }
    }

    /// Connect and complete the handshake. The node receiver holds the peer end.
    pub async fn connect(&self) -> Result<TcpStream> {
        let stream = TcpStream::connect(&self.addr).await?;
        wire::authenticate_stream(stream, &self.secret).await
    }

    /// Send one request to the node and read back the response.
    pub async fn send_agent_request(&self, request: &AgentRequest) -> Result<Response> {
        let stream = self.connect().await?;
        wire::send_request_stream(stream, request).await
    }

    /// Liveness + capability probe: a `GetCapabilities` round-trip.
    pub async fn ping(&self) -> Result<Vec<String>> {
        let response = self
            .send_agent_request(&AgentRequest::GetCapabilities)
            .await?;
        agent::capabilities_from(&response)
    }
}
