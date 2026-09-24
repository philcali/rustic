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
use tokio_rustls::TlsConnector;

use crate::{agent, wire, TlsClient};

/// A node endpoint (`host:port`) plus the epidemic (network) shared secret,
/// and (optionally) the TLS trust to encrypt the coordinator -> node hop.
pub struct RemoteClient {
    addr: String,
    secret: String,
    tls: Option<TlsClient>,
}

impl RemoteClient {
    pub fn new(addr: impl Into<String>, secret: impl Into<String>) -> Self {
        Self {
            addr: addr.into(),
            secret: secret.into(),
            tls: None,
        }
    }

    /// Enable TLS for the coordinator -> node hop. `tls` carries the root CA
    /// the node must chain to and the server name its cert must present. The
    /// shared auth handshake then runs *inside* the TLS tunnel. Without this
    /// the connection is cleartext TCP (the default).
    pub fn with_tls(mut self, tls: TlsClient) -> Self {
        self.tls = Some(tls);
        self
    }

    /// Connect over cleartext TCP and complete the handshake. The node
    /// receiver holds the peer end. (The TLS transport is selected
    /// automatically by [`send_agent_request`] when [`with_tls`] is set.)
    pub async fn connect(&self) -> Result<TcpStream> {
        let stream = TcpStream::connect(&self.addr).await?;
        wire::authenticate_stream(stream, &self.secret).await
    }

    /// Send one request to the node and read back the response, over whichever
    /// transport is configured (cleartext TCP by default, or TLS when
    /// [`with_tls`] is set). Both paths share the identical handshake +
    /// framing — only the stream type differs.
    pub async fn send_agent_request(&self, request: &AgentRequest) -> Result<Response> {
        match &self.tls {
            Some(tls) => {
                let tcp = TcpStream::connect(&self.addr).await?;
                let connector = TlsConnector::from(tls.config.clone());
                let stream = connector.connect(tls.server_name.clone(), tcp).await?;
                let stream = wire::authenticate_stream(stream, &self.secret).await?;
                wire::send_request_stream(stream, request).await
            }
            None => {
                let stream = self.connect().await?;
                wire::send_request_stream(stream, request).await
            }
        }
    }

    /// Liveness + capability probe: a `GetCapabilities` round-trip.
    pub async fn ping(&self) -> Result<Vec<String>> {
        let response = self
            .send_agent_request(&AgentRequest::GetCapabilities)
            .await?;
        agent::capabilities_from(&response)
    }
}
