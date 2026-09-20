//! Epidemic discovery (increment 2): mDNS/Bonjour advertise + probe.
//!
//! A node advertises itself on the local link as
//! `<name>._pandemic-node._tcp.local` alongside its TCP listener; the
//! coordinator probes the link for those records and gets live
//! `name -> host:port` pairs — no hand-typed roster required.
//!
//! Both sides speak the same mDNS protocol (RFC 6762 + DNS-SD, RFC 6335), so
//! a node and a coordinator on the same LAN — or the same loopback in the
//! e2e loop — find each other without any other rendezvous.

use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use agnostic_mdns::service::{Service, ServiceBuilder};
use agnostic_mdns::worksteal::net::tokio::Net;
use agnostic_mdns::worksteal::{channel, query, Server};
use agnostic_mdns::{Label, QueryParam, ServerOptions, SmolStr};
use anyhow::{Context, Result};
use tracing::info;

/// The mDNS service type every epidemic node advertises (DNS-SD records are
/// named `_service._proto`; the full name is `_pandemic-node._tcp.local`).
pub const SERVICE_TYPE: &str = "_pandemic-node._tcp";
/// The mDNS domain — mDNS lives in the `.local` zone, always.
pub const DOMAIN: &str = "local";
/// The fully-qualified service name a coordinator probes for.
pub const SERVICE_FQDN: &str = "_pandemic-node._tcp.local";

/// A node discovered on the link: its stable identity (the mDNS instance
/// name) and the TCP endpoint to reach it on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredNode {
    /// The instance name the node advertised (its identity).
    pub name: String,
    /// The node's `host:port` endpoint, as advertised in its SRV record.
    pub addr: String,
}

/// The mDNS advertiser for one node. Hold it for as long as the node should
/// stay discoverable: it answers queries and re-announces while alive, and
/// stops advertising when dropped (or after [`Server::shutdown`]).
pub type Advertiser = Server<Net, Service>;

/// Validate an mDNS instance name: one DNS label of letters, digits, and
/// hyphens (1–63 chars). Spaces, dots, and other characters would produce a
/// malformed record, so reject them up front with a readable error.
fn valid_instance_name(name: &str) -> bool {
    (1..=63).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// Advertise `name` as a `_pandemic-node._tcp.local` service at `ip:port`,
/// on the interface `interface` (the local IP of the NIC to multicast on).
///
/// The returned [`Advertiser`] keeps the advertisement alive — see its docs.
pub async fn advertise_node(
    name: &str,
    ip: IpAddr,
    port: u16,
    interface: Ipv4Addr,
) -> Result<Advertiser> {
    if !valid_instance_name(name) {
        anyhow::bail!(
            "invalid node name '{name}': use 1-63 letters, digits, or hyphens (the mDNS instance name)"
        );
    }
    let service = ServiceBuilder::new(Label::from(name), Label::from(SERVICE_TYPE))
        .with_domain(Label::from(DOMAIN))
        .with_port(port)
        .with_ip(ip)
        .with_txt_record(SmolStr::from(format!(
            "pandemic={}",
            env!("CARGO_PKG_VERSION")
        )))
        .finalize()
        .context("building the mDNS service record")?;
    let options = ServerOptions::new().with_ipv4_interface(interface);
    let server = Server::<Net, Service>::new(service, options)
        .await
        .context("starting the mDNS advertiser")?;
    info!(
        instance = %name,
        endpoint = %format!("{ip}:{port}"),
        interface = %interface,
        "advertising pandemic-node over mDNS"
    );
    Ok(server)
}

/// Probe the link for `_pandemic-node._tcp.local` records, collecting the
/// results for up to `timeout`. `interface` scopes the probe to one NIC
/// (its local IP); `None` uses the system default.
///
/// Results are de-duplicated (a re-announcing node appears once) and sorted
/// by name for stable output.
pub async fn discover_nodes(
    interface: Option<Ipv4Addr>,
    timeout: Duration,
) -> Result<Vec<DiscoveredNode>> {
    let mut params = QueryParam::new(Label::from(SERVICE_TYPE))
        .with_timeout(timeout)
        .with_disable_ipv6(true);
    if let Some(iface) = interface {
        params = params.with_ipv4_interface(iface);
    }
    let (tx, rx) = channel::unbounded();
    query::<Net>(params, tx)
        .await
        .context("starting the mDNS probe")?;

    let mut nodes: Vec<DiscoveredNode> = Vec::new();
    while let Ok(entry) = rx.recv().await {
        let ip = match entry.ipv4_addr() {
            Some(ip) => *ip,
            None => continue, // the rendezvous is IPv4; skip v6-only records
        };
        let port = entry.port();
        if port == 0 {
            continue;
        }
        let name = instance_name(entry.name());
        let addr = format!("{ip}:{port}");
        if nodes.iter().any(|n| n.name == name && n.addr == addr) {
            continue; // re-announcement of the same node
        }
        nodes.push(DiscoveredNode { name, addr });
    }
    nodes.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(nodes)
}

/// Extract the instance name from a full mDNS name:
/// `edge-1._pandemic-node._tcp.local` -> `edge-1`.
fn instance_name(fully_qualified: &str) -> String {
    let suffix = format!(".{SERVICE_FQDN}");
    fully_qualified
        .strip_suffix(suffix.as_str())
        .unwrap_or(fully_qualified)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_name_is_extracted() {
        assert_eq!(instance_name("edge-1._pandemic-node._tcp.local"), "edge-1");
        // A foreign/unknown record passes through unchanged.
        assert_eq!(
            instance_name("something._http._tcp.local"),
            "something._http._tcp.local"
        );
    }

    #[test]
    fn instance_names_are_validated() {
        assert!(valid_instance_name("edge-1"));
        assert!(valid_instance_name("a"));
        assert!(valid_instance_name("Edge-Node-2"));
        assert!(!valid_instance_name(""));
        assert!(!valid_instance_name("edge 1")); // space: malformed label
        assert!(!valid_instance_name("edge.1")); // dot: two labels
        assert!(!valid_instance_name(&"a".repeat(64))); // > 63 chars
    }

    /// Advertise a node on loopback, then probe the same interface: the
    /// coordinator-side probe must list it with the right name + port. This is
    /// the same round trip the e2e loop exercises (there, over the container's
    /// network).
    #[tokio::test]
    async fn advertised_node_is_discovered() {
        let lo = Ipv4Addr::new(127, 0, 0, 1);
        let server = advertise_node("epi-disc-test", IpAddr::V4(lo), 7711, lo)
            .await
            .expect("advertiser starts");

        let found = discover_nodes(Some(lo), Duration::from_secs(3))
            .await
            .expect("probe completes");

        assert!(
            found
                .iter()
                .any(|n| n.name == "epi-disc-test" && n.addr == "127.0.0.1:7711"),
            "expected epi-disc-test@127.0.0.1:7711 in {found:?}"
        );

        server.shutdown().await;
    }
}
