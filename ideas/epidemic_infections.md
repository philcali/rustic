# Epidemic Infections: Network Configuration Spreading

## Overview

Epidemic infections enable configuration and updates to "spread" across pandemic nodes in the network, with different infection levels controlling the propagation mechanism and intentionality.

## Implementation plan (codified)

> **This is the single source of truth for building epidemic across sessions.**
> Read this top-to-bottom to pick up the work cold: the fixed decisions say what
> is *not* to be re-litigated, the increments say what is done vs. next, and the
> resume point names the exact next step and where each piece lives.

### Fixed decisions (do not re-litigate without the owner)

1. **The primitive is node / group / coordinator**, not "infection level
   metadata in the plugin registry" and not a bespoke protocol.
   - **Node** = a host running `pandemic-node` (identity `name` + `addr:port`).
   - **Group** = a named trust boundary: one shared *epidemic secret* + a node
     roster, declared in `~/.config/pandemic/groups.toml`.
   - **Coordinator** = `pandemic-cli epidemic`: holds a group's secret + roster,
     plans once, applies to every node, reports per-node results.
2. **Reuse the existing agent wire protocol** — line-delimited JSON, one
   HMAC-SHA256 challenge/response handshake + one request/response — carried
   over **TCP** instead of a Unix socket. Do **not** invent a new wire format;
   the node and the coordinator speak the same protocol as the agent.
3. **One wire source of truth:** `pandemic-common::{auth, wire, remote}`.
   The agent server, the node receiver, and both client paths all go through
   these. Any new network capability routes through them too.
4. **Two secrets, two hops — never mix them.**
   - *Epidemic* secret (`/etc/pandemic/epidemic-secret`) guards the **network**
     coordinator→node hop.
   - *Agent* secret (`/etc/pandemic/agent-secret`) guards the **local**
     node→agent hop.
5. **A node is a narrow deployment surface** (allowlist):
   `GetCapabilities`, `ApplyDeployment`, `GetDeploymentStatus`, `ListDeployments`,
   `PreviewDeployment`. No general remote shell; everything else is refused and
   never reaches the agent.
6. **The coordinator reuses the same pure `Plan` as `deployment install`**
   (`resolve_deployment_plan` + `print_dry_run` in `pandemic-cli/src/deployment.rs`).
   One plan is computed locally, once, and sent to every node.
7. **A partial spread is a failure** — the coordinator exits non-zero if any node
   fails. It must never look like success.
8. **Honest security posture:** transport is **authenticated, not yet
   encrypted** (HMAC proves the peer holds the secret; the payload travels in
   cleartext over TCP). **TLS + payload signing are the production gate**
   (Increment 4), shared with the registry signature work. Node default bind is
   loopback (`127.0.0.1:7711`); binding `0.0.0.0` is deliberate + firewalled.

### Increment status

| # | Increment | Maps to this doc's levels | Status |
|---|---|---|---|
| 1 | Node / group / coordinator + reliable TCP spread | Foundation | **done** (v0.5.0) |
| 2 | Discovery (mDNS/Bonjour) — roster discovery | Level 1 | **done** |
| 3 | Multicast + targeting + canary | Level 2 | planned — **next** |
| 4 | Reliability + hardening (retries, audit, rate-limit, per-node secrets, TLS, signing) | Level 3 + Phase 4 | planned |

#### Increment 1 — Node / group / coordinator + reliable TCP spread — **done**

Shipped: the `pandemic-node` receiver (handshake + narrow allowlist + forward to
local agent, 5 integration tests); `pandemic-cli epidemic spread/nodes`
(dry-run, per-node results, non-zero on partial); the groups file + roster merge;
the shared `pandemic-common::{auth,wire,remote}` (with the agent refactored onto
it); docs (`docs/epidemic.md`), operator loop (`e2e/README.md`), and this plan.
Gate green: `cargo build --workspace`, `clippy --workspace -- -D warnings`,
`cargo fmt --check`, `cargo test --workspace` (199 passing).

#### Increment 2 — Discovery (mDNS/Bonjour) — **done**

Goal: the coordinator **discovers** the roster instead of hand-typing
`--node host:port`. A node advertises itself; the coordinator lists the live
peers and folds them into the target list.

Shipped:
- Node side: `pandemic-node` advertises `_pandemic-node._tcp.local` by default
  (instance name = `--name` or hostname; SRV carries the port from `--listen`,
  A record the address, TXT `pandemic=<version>`), on the interface matching
  `--listen`. `--no-advertise` opts out; a failed advertisement is a warning and
  never blocks the TCP surface. IPv6-only listens are refused (mDNS is IPv4).
- Coordinator side: `epidemic nodes --discover` lists the live peers above the
  groups; `epidemic spread --discover` unions discovered peers with any
  `--group`/`--node` roster (dedup by address, roster name wins). `--interface`
  (IPv4) scopes the probe; `--timeout` (seconds, default 3).
- Shared code in `pandemic-common::discovery` (`advertise_node`,
  `discover_nodes`, `DiscoveredNode`) on `agnostic-mdns` (tokio).

- **Acceptance (met):** in the e2e container, starting `pandemic-node` makes it
  discoverable and `epidemic nodes --discover` lists it with the right port;
  spreading to a discovered peer behaves exactly like an explicit `--node`.
  Verified two ways in `e2e/README.md`: section E (same host, over loopback) and
  section F (two containers on a shared Docker bridge — the "creative"
  cross-host loop), including a real `spread --discover` applying on the remote
  container.
- **Decisions (resolved):**
  - **Module, not a crate:** `pandemic-common::discovery` — discovery is also
    useful to the daemon/console, and a shared module keeps one wire source of
    truth. (Chosen over a `pandemic-discovery` crate.)
  - **Dedicated mDNS dep:** `agnostic-mdns` 0.4 (tokio feature) — the only
    maintained mDNS crate with both an advertise server and a discover client.
    `pandemic-udp` is a UDP→daemon proxy, *not* an mDNS stack, so it is not
    reused. A `rustix/time` feature is enabled (feature unification) to work
    around an upstream rustix 1.1.x feature-gating bug in the `net` module.
  - **Discover-and-list + union, no auto-join:** `--discover` lists and unions
    with the roster; auto-joining a named group is deferred to Increment 4
    (per-node secrets/TLS make a discovered identity trustworthy enough to
    adopt).

- **Known issue:** the `agnostic-mdns`/`dns-protocol-patch` stack occasionally
  panics on a worker thread while parsing a receive-loop packet (rare, ~1/9
  loopback probes; non-fatal — discovery still returns the correct nodes).
  Upstream library bug on the latest published versions; see
  `docs/epidemic.md` → Known issues.

#### Increment 3 — Multicast + targeting + canary

Goal: subnet-wide spread with **criteria targeting** and a **canary** rollout,
on top of Increment 2 discovery.

- Broadcast the *intent* to a multicast group; nodes self-select by matching
  criteria (labels/roles/capabilities) against their own identity.
- **Canary:** apply to a named subset or a percentage first, then the rest; the
  coordinator reports per-cohort results.
- **Acceptance:** a multicast spread reaches every matching node on the test
  subnet, non-matching nodes are untouched, and a canary spread applies to the
  first cohort only until promoted.
- **Open decisions:** the multicast group address + payload schema; the
  criteria-matching semantics (who evaluates — node vs. coordinator); canary as
  percentage vs. named subset.

#### Increment 4 — Reliability + hardening

Goal: make epidemic production-grade.

- **TLS** on the coordinator→node hop (rustls); per-node secrets or mTLS
  identity in place of the single shared group secret.
- **Retries + idempotency** on the apply; a **sender-side audit** entry per
  spread (which nodes, which plan hash, per-node outcome).
- **Rate limiting** to prevent spread storms.
- **Payload signing** (the production gate, shared with the registry): a node
  only applies a deployment it can verify.
- **Acceptance:** a spread over an untrusted network is end-to-end encrypted and
  signed; a forged/unsigned deployment is refused; a dropped node is retried and
  reported, not silently lost.

### Resume point

- **Next up: Increment 3 (multicast + targeting + canary).** Build on the
  Increment 2 discovery: broadcast the *intent* to a multicast group, nodes
  self-select by criteria (labels/roles/capabilities), and a canary cohort is
  applied first. Resolve the open decisions in the Increment 3 section first
  (multicast group address + payload schema; who evaluates criteria; canary as
  percentage vs. named subset).
- **The gate to pass before an increment counts as done** (mirrors CI):
  `cargo build --workspace` && `cargo clippy --workspace -- -D warnings` &&
  `cargo fmt --check` && `cargo test --workspace`.
- **Where each piece lives (code map):**
  - `pandemic-common/src/auth.rs` — handshake crypto (`sign`/`verify`/
    `generate_nonce`/`generate_secret`) + `EPIDEMIC_SECRET_PATH`.
  - `pandemic-common/src/wire.rs` — `authenticate_stream` +
    `send_request_stream` (shared framing for Unix + TCP).
  - `pandemic-common/src/remote.rs` — `RemoteClient` (coordinator→node TCP).
  - `pandemic-common/src/discovery.rs` — mDNS advertise + probe
    (`advertise_node`, `discover_nodes`, `DiscoveredNode`, `SERVICE_FQDN`).
  - `pandemic-common/src/groups.rs` — `NodeConfig`, `GroupConfig`,
    `load_groups[_or_default]`, `find_group`, `merge_roster`,
    `default_groups_path`.
  - `pandemic-common/src/agent.rs` — `AgentClient` (local Unix), `AGENT_SECRET_PATH`.
  - `pandemic-node/src/{main,allowlist}.rs` — the node receiver + allowlist +
    mDNS announce (`--name`/`--no-advertise`, `advertise_for_listen`).
  - `pandemic-cli/src/epidemic.rs` — coordinator (`spread`/`nodes`/
    `resolve_roster`/`merge_discovered`/`apply_to_node`/
    `resolve_epidemic_secret`/`print_results`).
  - `pandemic-cli/src/deployment.rs` — `resolve_deployment_plan` +
    `print_dry_run` (shared by `deployment install` **and** `epidemic spread`).
  - `pandemic-agent/src/main.rs` — agent server (shares `auth`).
  - `pandemic-protocol/` — `AgentRequest`, `Response`, `AuthChallenge`/`AuthResponse`.
  - How-to: `docs/epidemic.md`; operator loop: `e2e/README.md` (epidemic section).

---

## Infection Levels

### **Level 0: Isolated** 
- No network spreading
- Local infection only
- Default for most services

### **Level 1: Discoverable (mDNS)**
- Passive discovery via mDNS/Bonjour
- "Plug and play" - new nodes auto-discover
- Suitable for service discovery, local clusters

```bash
pandemic-cli epidemic set-level my-service 1
# Service becomes discoverable on local network
```

### **Level 2: Contagious (UDP Multicast)**
- Active spreading via UDP multicast
- Intentional configuration distribution
- Controlled by infection policies

```bash
pandemic-cli epidemic spread config-update --level 2 --target subnet:192.168.1.0/24
# Actively spreads to matching nodes
```

### **Level 3: Virulent (TCP Mesh)**
- Aggressive cross-network spreading
- Reliable delivery with retry logic
- For critical updates and security patches

```bash
pandemic-cli epidemic spread security-patch --level 3 --priority critical
# Spreads across network boundaries with guaranteed delivery
```

## Layered Architecture

### **Discovery Layer (Level 1)**
```
Node A ←→ mDNS ←→ Node B
  ↓                 ↓
"I exist"      "I see you"
```

### **Multicast Layer (Level 2)**  
```
Node A → UDP Multicast → [Node B, Node C, Node D]
         "Here's config X"
```

### **Mesh Layer (Level 3)**
```
Node A → TCP → Node B → TCP → Node C
  ↓              ↓              ↓
Relay         Relay         Apply
```

## Configuration Spreading

### **Epidemic Payload Structure**
```toml
[epidemic]
name = "edge-config-v2"
infection_level = 2
spread_policy = "multicast"
target_criteria = ["role:edge-device", "version:<2.0"]

[propagation]
max_hops = 3
ttl_seconds = 3600
verification = "signature_required"
rollback_on_failure = true

[payload]
type = "config_update"
data = { 
  api_endpoint = "https://api-v2.example.com",
  feature_flags = { new_ui = true, beta_api = false }
}
```

### **Spreading Mechanisms**

#### **Level 1: mDNS Discovery**
- Service announces: `_pandemic._tcp.local`
- Automatic peer discovery
- Service registry synchronization

#### **Level 2: UDP Multicast**
- Multicast group: `239.255.pandemic.1`
- Targeted spreading with criteria matching
- Efficient for subnet-wide updates

#### **Level 3: TCP Mesh**
- Persistent connections between nodes
- Guaranteed delivery with acknowledgments
- Cross-subnet and WAN propagation

## Use Cases by Level

### **Level 1 Examples**
```bash
# Service discovery
pandemic-cli epidemic discover --services
# → Finds: redis@192.168.1.10, mqtt@192.168.1.15

# Local cluster formation
pandemic-cli epidemic join-cluster edge-cluster
```

### **Level 2 Examples**
```bash
# Configuration rollout
pandemic-cli epidemic spread app-config --target role:web-server

# Feature flag updates
pandemic-cli epidemic spread feature-flags --canary 25%

# Service endpoint changes
pandemic-cli epidemic spread service-registry --immediate
```

### **Level 3 Examples**
```bash
# Security patches
pandemic-cli epidemic spread security-update --priority critical --verify-all

# System-wide policy changes
pandemic-cli epidemic spread compliance-policy --mandatory

# Emergency configuration
pandemic-cli epidemic spread emergency-config --override-all
```

## Implementation Strategy

### **Phase 1: Foundation**
- Infection level metadata in plugin registry
- Basic mDNS discovery (Level 1)
- CLI commands for level management

### **Phase 2: Multicast Spreading**
- UDP multicast implementation (Level 2)
- Target criteria matching
- Configuration payload distribution

### **Phase 3: Mesh Network**
- TCP mesh networking (Level 3)
- Reliable delivery guarantees
- Cross-network propagation

### **Phase 4: Advanced Features**
- Canary deployments
- Rollback mechanisms
- Conflict resolution
- Security and verification

## Security Considerations

- **Payload signing** - Cryptographic verification of epidemic payloads
- **Network isolation** - Respect network boundaries and firewall rules
- **Rate limiting** - Prevent epidemic storms and network flooding
- **Access control** - Only authorized nodes can initiate epidemics
- **Audit logging** - Track all epidemic activities for compliance

## Integration Points

- **pandemic-daemon** - Core epidemic coordination
- **pandemic-udp** - Level 2 multicast implementation
- **pandemic-cli** - Epidemic management interface
- **Event system** - Epidemic status and progress events
- **Configuration system** - Target for epidemic payloads