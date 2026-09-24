//! TLS for the coordinator -> node hop (epidemic, increment 4).
//!
//! The wire protocol — one HMAC challenge/response handshake, then a
//! request/response — is transport-agnostic: it runs over any
//! `AsyncRead + AsyncWrite + Unpin` stream (see [`crate::wire`]). This module
//! builds the two rustls configs that turn a raw TCP stream into an encrypted
//! one:
//!
//!   * [`TlsServer`] — the node's identity: the leaf cert chain + private key
//!     it presents (built by `pandemic-node`);
//!   * [`TlsClient`] — the coordinator's trust: the root CA(s) the node's
//!     cert must chain to, plus the server name the cert must present
//!     (built by `pandemic-cli`).
//!
//! TLS is strictly opt-in. Without it the node and coordinator still speak the
//! cleartext handshake over TCP exactly as before, so existing loopback
//! deployments and tests are unaffected.
//!
//! Both sides still run the shared [`crate::auth`] handshake *inside* the TLS
//! tunnel — the HMAC secret is the request-level gate and TLS is the
//! transport-level encryption; together they are the production transport.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use rustls::crypto::ring::default_provider;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use rustls_pemfile::{certs, private_key};

/// The ring-based crypto provider, wrapped for [`rustls`]. We select it
/// explicitly (via `builder_with_provider`) rather than relying on rustls to
/// auto-detect it, so the choice stays deterministic even when other workspace
/// members enable a different provider via feature unification — which would
/// otherwise make rustls refuse to pick one and panic at runtime.
fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(default_provider())
}

/// Node-side TLS identity: the leaf certificate chain + private key the node
/// presents to a connecting coordinator.
#[derive(Clone)]
pub struct TlsServer {
    /// The ready-to-use rustls server config (cloned into a `TlsAcceptor`).
    pub config: Arc<ServerConfig>,
}

impl TlsServer {
    /// Build the server config from PEM-encoded material. `cert_pem` is the
    /// leaf certificate (optionally followed by its issuer chain); `key_pem`
    /// is the matching private key (PKCS#8, PKCS#1/RSA, or SEC1/EC).
    pub fn from_pem(cert_pem: &[u8], key_pem: &[u8]) -> Result<Self> {
        let chain =
            certs_from(cert_pem).with_context(|| "parsing the node's TLS certificate PEM")?;
        if chain.is_empty() {
            anyhow::bail!("no certificate found in the node TLS material");
        }
        let key = key_from(key_pem)
            .with_context(|| "parsing the node's TLS private key PEM")?
            .ok_or_else(|| anyhow::anyhow!("no private key found in the node TLS material"))?;
        let config = ServerConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()?
            .with_no_client_auth()
            .with_single_cert(chain, key)
            .context("building the node's TLS server config (cert/key mismatch?)")?;
        Ok(Self {
            config: Arc::new(config),
        })
    }

    /// Build the server config from PEM files on disk (the usual operator path:
    /// `--tls-cert` + `--tls-key`).
    pub fn from_paths(cert: &Path, key: &Path) -> Result<Self> {
        let cert_pem = std::fs::read(cert)
            .with_context(|| format!("reading the node TLS cert {}", cert.display()))?;
        let key_pem = std::fs::read(key)
            .with_context(|| format!("reading the node TLS key {}", key.display()))?;
        Self::from_pem(&cert_pem, &key_pem)
    }
}

/// Coordinator-side TLS trust: the root CA(s) used to verify the node's
/// certificate, and the server name the certificate must present.
#[derive(Clone)]
pub struct TlsClient {
    /// The ready-to-use rustls client config (cloned into a `TlsConnector`).
    pub config: Arc<ClientConfig>,
    /// The server name the node's certificate must present (its CN/SAN).
    pub server_name: ServerName<'static>,
}

impl TlsClient {
    /// Build the shared trust config from PEM-encoded root CA material. The
    /// result is reusable across nodes (the server name is attached per
    /// connection, see [`with_server_name`]).
    pub fn trust_from_pem(root_ca_pem: &[u8]) -> Result<Arc<ClientConfig>> {
        let ca_certs = certs_from(root_ca_pem).with_context(|| "parsing the root CA PEM")?;
        if ca_certs.is_empty() {
            anyhow::bail!("no root CA certificate found in the TLS trust material");
        }
        let mut roots = RootCertStore::empty();
        for ca in ca_certs {
            roots.add(ca).context("adding a root CA certificate")?;
        }
        let config = ClientConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()?
            .with_root_certificates(roots)
            .with_no_client_auth();
        Ok(Arc::new(config))
    }

    /// Build the shared trust config from a root CA PEM file on disk (the usual
    /// operator path: `--tls-ca`).
    pub fn trust_from_paths(root_ca: &Path) -> Result<Arc<ClientConfig>> {
        let pem = std::fs::read(root_ca)
            .with_context(|| format!("reading the root CA {}", root_ca.display()))?;
        Self::trust_from_pem(&pem)
    }

    /// Attach a per-connection `server_name` (the DNS name the node cert is
    /// issued for) to a shared trust [`TlsClient::trust_from_pem`] config.
    pub fn with_server_name(
        config: Arc<ClientConfig>,
        server_name: impl Into<String>,
    ) -> Result<Self> {
        let name = ServerName::try_from(server_name.into())
            .with_context(|| "parsing the TLS server name")?;
        Ok(Self {
            config,
            server_name: name,
        })
    }

    /// Convenience: build a ready client from root CA PEM + a single
    /// `server_name` (when every node presents the same name).
    pub fn from_pem(root_ca_pem: &[u8], server_name: impl Into<String>) -> Result<Self> {
        let config = Self::trust_from_pem(root_ca_pem)?;
        Self::with_server_name(config, server_name)
    }

    /// Convenience: build a ready client from a root CA PEM file + a single
    /// `server_name`.
    pub fn from_paths(root_ca: &Path, server_name: impl Into<String>) -> Result<Self> {
        let config = Self::trust_from_paths(root_ca)?;
        Self::with_server_name(config, server_name)
    }
}

/// Collect the certificates out of a `&[u8]` PEM buffer. (rustls-pemfile's
/// `certs` needs a `&mut dyn BufRead`; `&[u8]` qualifies, but the binding must
/// be mutable, so the shadowing lives in one place.)
fn certs_from(pem: &[u8]) -> std::io::Result<Vec<CertificateDer<'static>>> {
    let mut pem = pem;
    certs(&mut pem).collect()
}

/// Read the first private key out of a `&[u8]` PEM buffer.
fn key_from(pem: &[u8]) -> std::io::Result<Option<PrivateKeyDer<'static>>> {
    let mut pem = pem;
    private_key(&mut pem)
}
