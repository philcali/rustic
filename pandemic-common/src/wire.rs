//! Line-delimited JSON wire helpers for the agent protocol.
//!
//! The agent protocol is: one `AuthChallenge`/`AuthResponse` handshake, then a
//! single `AgentRequest` line followed by one `Response` line, over any
//! bidirectional byte stream — the Unix admin socket (`AgentClient`) or the
//! network node socket (`RemoteClient`). These free functions take a generic
//! stream so both transports share the exact same framing and handshake code.

use anyhow::Result;
use pandemic_protocol::{AgentRequest, AuthChallenge, AuthResponse, Response};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

use crate::auth;

/// Perform the handshake against a server on `stream`, returning the
/// authenticated stream. Reads the `AuthChallenge`, signs its nonce with
/// `secret`, and sends the `AuthResponse`.
pub async fn authenticate_stream<S>(mut stream: S, secret: &str) -> Result<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    // Read the challenge (scoped so the borrow ends before we write back).
    let mut line = String::new();
    {
        let mut reader = BufReader::new(&mut stream);
        reader.read_line(&mut line).await?;
    }
    let challenge: AuthChallenge = serde_json::from_str(line.trim())
        .map_err(|e| anyhow::anyhow!("expected AuthChallenge: {e}"))?;

    let signature = auth::sign(secret, &challenge.nonce);
    let response = AuthResponse {
        nonce: challenge.nonce,
        signature,
    };
    let mut payload = serde_json::to_string(&response)?;
    payload.push('\n');
    stream.write_all(payload.as_bytes()).await?;
    stream.flush().await?;
    Ok(stream)
}

/// Send one `AgentRequest` and read back the single `Response` over
/// `stream`.
pub async fn send_request_stream<S>(mut stream: S, request: &AgentRequest) -> Result<Response>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    // Write the request first, then read the response.
    let mut payload = serde_json::to_string(request)?;
    payload.push('\n');
    stream.write_all(payload.as_bytes()).await?;
    stream.flush().await?;

    let mut reader = BufReader::new(&mut stream);
    let mut response_line = String::new();
    reader.read_line(&mut response_line).await?;
    Ok(serde_json::from_str(response_line.trim())?)
}
