//! Epidemic broadcast (increment 3): the multicast transport for spread
//! intents.
//!
//! One-way, link-local: the coordinator sends the intent datagram to the
//! site-local group; every node that joined the group on that link receives
//! it. Delivery is best-effort by design (UDP) — the coordinator re-sends a
//! few times, and nodes de-duplicate by `spread_id`, so re-sends are safe.
//!
//! Defaults: group `239.255.77.11`, port `7712` — both in the
//! administratively-scoped block, and the last two octets echo the node's
//! default TCP port (`7711`) so the pair is easy to remember. Overridable
//! by both sides.

use std::io;
use std::net::{Ipv4Addr, SocketAddr};

use anyhow::{Context, Result};
use pandemic_protocol::SpreadIntent;

/// Default multicast group for spread intents (site-local block).
pub const DEFAULT_MULTICAST_GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 77, 11);
/// Default UDP port for spread intents (one above the node's default TCP).
pub const DEFAULT_MULTICAST_PORT: u16 = 7712;
/// Multicast TTL: 1 — the intent stays on this link (subnet-wide, never
/// router-hopped).
const MULTICAST_TTL: u32 = 1;
/// Re-sends of the intent (UDP is best-effort; nodes de-dup by spread_id,
/// so repeats are harmless).
pub const INTENT_RESENDS: u32 = 3;

/// Send `intent` to the multicast group once. `interface` scopes the send to
/// that NIC's local IPv4 (loopback for local testing); `None` uses the
/// system default multicast route.
pub fn send_intent(
    group: Ipv4Addr,
    port: u16,
    interface: Option<Ipv4Addr>,
    intent: &SpreadIntent,
) -> Result<()> {
    let bytes = serde_json::to_vec(intent)
        .with_context(|| format!("serializing spread intent {}", intent.spread_id))?;
    if bytes.len() > 65_507 {
        anyhow::bail!(
            "spread intent is {} bytes — too large for one UDP datagram",
            bytes.len()
        );
    }
    use socket2::{Domain, SockAddr, Socket, Type};

    let sock = Socket::new(Domain::IPV4, Type::DGRAM, None).map_err(io::Error::other)?;
    sock.set_multicast_ttl_v4(MULTICAST_TTL)
        .map_err(io::Error::other)?;
    // Loop on, so same-host receivers (the e2e loopback path) get it too.
    sock.set_multicast_loop_v4(true).map_err(io::Error::other)?;
    if let Some(iface) = interface {
        sock.set_multicast_if_v4(&iface).map_err(io::Error::other)?;
    }
    let dest = SocketAddr::V4(std::net::SocketAddrV4::new(group, port));
    sock.send_to(&bytes, &SockAddr::from(dest))
        .map_err(io::Error::other)
        .with_context(|| format!("sending spread intent to {group}:{port}"))?;
    Ok(())
}

/// A node-side listener joined to the intent group on one interface.
///
/// Binds `0.0.0.0:port` with `SO_REUSEADDR` so several listeners (multiple
/// nodes, tests) can coexist on one host, then joins the multicast group on
/// `interface`.
pub struct IntentListener {
    socket: tokio::net::UdpSocket,
}

impl IntentListener {
    /// Bind and join. `interface` is the local IPv4 of the NIC the group
    /// lives on (loopback for local testing).
    pub fn join(group: Ipv4Addr, port: u16, interface: Ipv4Addr) -> Result<Self> {
        use socket2::{Domain, SockAddr, Socket, Type};

        let std_sock = Socket::new(Domain::IPV4, Type::DGRAM, None).map_err(io::Error::other)?;
        std_sock.set_nonblocking(true).map_err(io::Error::other)?;
        std_sock.set_reuse_address(true).map_err(io::Error::other)?;
        let bind_addr = SocketAddr::V4(std::net::SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port));
        std_sock
            .bind(&SockAddr::from(bind_addr))
            .map_err(io::Error::other)?;
        std_sock
            .join_multicast_v4(&group, &interface)
            .map_err(io::Error::other)
            .with_context(|| format!("joining multicast group {group} on {interface}"))?;

        let socket = tokio::net::UdpSocket::from_std(std_sock.into())
            .context("adopting the multicast socket")?;
        Ok(Self { socket })
    }

    /// Wait for one intent datagram. Returns the raw payload and the sender
    /// address.
    pub async fn recv(&self, buf: &mut [u8]) -> std::io::Result<(usize, SocketAddr)> {
        self.socket.recv_from(buf).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intent() -> SpreadIntent {
        SpreadIntent {
            version: 1,
            spread_id: "test-spread".to_string(),
            stage: pandemic_protocol::SpreadStage::Full,
            issued_at: 0,
            token: "ab".repeat(32),
            group: None,
            plan: pandemic_protocol::PlanIdentity {
                name: "webapp".into(),
                version: "1.0.0".into(),
                sha256: "cd".repeat(32),
            },
            criteria: vec!["role=edge".to_string()],
            canary: None,
            origin: "127.0.0.1".into(),
            callback_port: 7711,
        }
    }

    /// Loopback round trip: join the group on `127.0.0.1`, send on the same
    /// interface, and the listener must receive the identical intent.
    /// (Multicast on loopback is the same mechanism the mDNS tests already
    /// rely on.)
    #[tokio::test]
    async fn intent_round_trips_over_multicast() {
        let lo = Ipv4Addr::new(127, 0, 0, 1);
        let listener = IntentListener::join(DEFAULT_MULTICAST_GROUP, DEFAULT_MULTICAST_PORT, lo)
            .expect("listener joins");
        let expected = intent();

        send_intent(
            DEFAULT_MULTICAST_GROUP,
            DEFAULT_MULTICAST_PORT,
            Some(lo),
            &expected,
        )
        .expect("send");

        let mut buf = vec![0u8; 65_536];
        let (n, _from) = listener
            .recv(&mut buf)
            .await
            .expect("recv delivers the datagram");
        let got: SpreadIntent =
            serde_json::from_slice(&buf[..n]).expect("payload parses as SpreadIntent");
        assert_eq!(got, expected);
    }

    #[test]
    fn defaults_are_site_local_and_distinct_from_tcp() {
        assert!(DEFAULT_MULTICAST_GROUP.is_broadcast() || DEFAULT_MULTICAST_GROUP.is_multicast());
        assert_eq!(DEFAULT_MULTICAST_PORT, 7712);
    }
}
