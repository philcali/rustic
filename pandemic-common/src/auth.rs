//! Shared agent-protocol authentication primitives.
//!
//! The agent, the node receiver (`pandemic-node`), and the coordinator
//! (`pandemic-cli epidemic`) all speak the same HMAC challenge/response
//! handshake. Keeping [`sign`], [`verify`], and the nonce/secret generators in
//! one place means there is a single source of truth for the crypto: the agent
//! server, the node server, and both client paths agree byte-for-byte.
//!
//! The handshake (per connection, before any request):
//!   1. server sends [`pandemic_protocol::AuthChallenge`] `{ nonce }`
//!   2. client replies [`pandemic_protocol::AuthResponse`]
//!      `{ nonce, signature }` where `signature = HMAC-SHA256(secret, nonce)`
//!   3. server [`verify`]s the signature in constant time.
//!
//! The `secret` is the shared key for one trust boundary — the agent secret
//! for the local admin socket, the **epidemic secret** for the network
//! coordinator→node hop.

use subtle::ConstantTimeEq;

/// Default location of the network (coordinator→node) shared secret. The node
/// receiver and the `epidemic` coordinator both fall back to this path when no
/// `--secret` / `--secret-path` / per-group secret is given. Distinct from the
/// agent secret ([`AGENT_SECRET_PATH`]) — the epidemic secret authorizes the
/// network hop; the agent secret authorizes the local privileged hop.
pub const EPIDEMIC_SECRET_PATH: &str = "/etc/pandemic/epidemic-secret";

/// `HMAC-SHA256(secret, nonce)`, hex-encoded. Deterministic — the same secret
/// and nonce always yield the same 64-char string.
pub fn sign(secret: &str, nonce: &str) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    let mut mac: Hmac<Sha256> =
        Hmac::new_from_slice(secret.as_bytes()).expect("HMAC accepts keys of any length");
    mac.update(nonce.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Constant-time check that `signature` is the valid HMAC for `(secret, nonce)`.
///
/// Returns `false` (rather than panicking) when the lengths differ. Compares the
/// hex encodings in constant time, which is equivalent to a constant-time
/// comparison of the decoded MAC bytes.
pub fn verify(secret: &str, nonce: &str, signature: &str) -> bool {
    let expected = sign(secret, nonce);
    let (e, p) = (expected.as_bytes(), signature.as_bytes());
    if e.len() != p.len() {
        return false;
    }
    ConstantTimeEq::ct_eq(e, p).into()
}

/// A fresh random nonce: 16 random bytes, hex-encoded (32 chars).
pub fn generate_nonce() -> String {
    hex::encode(rand::random::<[u8; 16]>())
}

/// A fresh random secret: 32 random bytes, hex-encoded (64 chars).
pub fn generate_secret() -> String {
    hex::encode(rand::random::<[u8; 32]>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_is_deterministic() {
        let a = sign("s3cret", "nonce-1");
        let b = sign("s3cret", "nonce-1");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64, "HMAC-SHA256 hex is 64 chars");
    }

    #[test]
    fn sign_depends_on_secret_and_nonce() {
        let base = sign("s3cret", "nonce-1");
        assert_ne!(base, sign("other", "nonce-1"));
        assert_ne!(base, sign("s3cret", "nonce-2"));
    }

    #[test]
    fn verify_round_trips() {
        let secret = "s3cret";
        let nonce = generate_nonce();
        let sig = sign(secret, &nonce);
        assert!(verify(secret, &nonce, &sig));
    }

    #[test]
    fn verify_rejects_wrong_secret() {
        let nonce = "nonce-1";
        let sig = sign("correct", nonce);
        assert!(!verify("wrong", nonce, &sig));
    }

    #[test]
    fn verify_rejects_wrong_nonce() {
        let sig = sign("s3cret", "nonce-1");
        assert!(!verify("s3cret", "nonce-2", &sig));
    }

    #[test]
    fn verify_rejects_garbage_without_panic() {
        // Odd length / non-hex must be a clean `false`, not a panic.
        assert!(!verify("s3cret", "nonce-1", "zz"));
        assert!(!verify("s3cret", "nonce-1", ""));
        assert!(!verify("s3cret", "nonce-1", &"a".repeat(63)));
    }

    #[test]
    fn generated_values_are_hex_and_sized() {
        let n = generate_nonce();
        assert_eq!(n.len(), 32);
        assert!(n.chars().all(|c| c.is_ascii_hexdigit()));

        let s = generate_secret();
        assert_eq!(s.len(), 64);
        assert!(s.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
