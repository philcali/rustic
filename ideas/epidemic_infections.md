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
| 3 | Multicast + targeting + canary | Level 2 | **done** |
| 4 | Reliability + hardening (retries, audit, rate-limit, per-node secrets, TLS, signing) | Level 3 + Phase 4 | **in progress** — 4a TLS done; sender-side audit done (shipped as 5a); retries/idempotency, rate-limit, payload signing, per-node secrets/mTLS remaining |
| 5 | Epidemic observability (console / UI to see + manage the spread) | Cross-cutting (all levels) | **in progress** — 5a audit/record foundation **done**, 5b read-only console **done**; 5c trigger + live progress remaining |

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

#### Increment 3 — Multicast + targeting + canary — **done**

Goal: subnet-wide spread with **criteria targeting** and a **canary** rollout,
on top of Increment 2 discovery.

Shipped:
- **Intent-only multicast.** `pandemic-common::multicast` (`send_intent`,
  `IntentListener`): a short JSON `SpreadIntent` over UDP multicast — group
  `239.255.77.11`, port `7712`, TTL 1, loop on, re-sent `INTENT_RESENDS=3`
  times. The intent carries **no payload and no secret** — only the plan's
  identity (`name`/`version`/`sha256`), the targeting criteria, the canary
  cohort, the coordinator's `origin:callback_port`, and a **token**.
- **The token = group membership.** `token = HMAC-SHA256(epidemic_secret,
  spread_id)` — the same HMAC primitive as the wire handshake (the `spread_id`
  plays the nonce role). A node verifies it against its *own* secret before any
  action; a wrong/absent secret ⇒ the intent is dropped. Pure logic lives in
  `pandemic-common::intent` (token, freshness, criteria, canary cohort) so both
  sides — and the tests — agree.
- **Node self-selection.** `pandemic-node` joins the intent group by default
  (on the interface matching `--listen`), held for the process lifetime;
  `--no-multicast` opts out (TCP surface unaffected). On each intent it checks,
  in order: protocol version → token → freshness (60s) → dedupe (applied set)
  → criteria → canary cohort. Only a selected node dials the coordinator back
  (`pandemic-node/src/spread.rs`), reusing the **stable** wire handshake
  (node = client) and a one-`BufReader`-per-connection line model. It verifies
  `sha256(wire_line) == intent.plan.sha256` before applying through the agent.
- **Criteria + canary.** `key=value` criteria (AND; empty = all): `name=X`, a
  node label (`--label`), or `cap:X=true` (fetched lazily). Canary cohort is a
  **percentage** (stateless hash bucket — first 2 hex bytes of
  `HMAC(secret, spread_id ‖ "|" ‖ node_name)` mod 100) or a **named subset**
  (one extra criterion). **Promote** = re-broadcast the same `spread_id` at
  Full; the node de-duplicates by `spread_id`, so canary→promote applies "first
  cohort, then everyone else" with no double-apply.
- **Coordinator + history.** `epidemic spread --broadcast` binds an ephemeral
  callback listener, sets its LAN IPv4 as `origin`, broadcasts, and collects
  per-node `Response`s. `epidemic spreads` lists recent broadcast history
  (newest first, `--limit`) from `~/.local/state/pandemic/spread-history.log` —
  the source of `--spread-id` for `--promote`. A canary/promote that reaches no
  node is a **failure** (non-zero), except a promote that finds everyone
  already applied (benign no-op).

- **Acceptance (met):** on the two-container bridge, a Full broadcast applies
  the plan **on B** (file + `deployment status` present on B, absent on A);
  canary `0%` is ignored, `100%` applies; a subset cohort applies for a matching
  `--label` and is ignored otherwise; a canary that excludes B is applied by the
  subsequent `--promote` of the same `spread_id`; re-sends de-duplicate; a node
  with a **different epidemic secret** rejects the intent
  (`token does not match`); `epidemic spreads` records every stage. See
  `e2e/README.md` → section **F.2** (the verified cross-host loop) and
  `docs/epidemic.md` → Broadcast.
- **Decisions (resolved):**
  - **Node-side evaluation, not coordinator-side.** The coordinator broadcasts
    an intent and never learns who applied; the node does all matching locally
    against its own identity. Keeps the roster-free model and the narrow-surface
    guarantee (a node is never told *who else* is in the group).
  - **Canary = percentage OR named subset** (both supported), not one or the
    other. Percentage is a stateless hash bucket (no fleet view); subset is an
    extra criterion. Promote re-broadcasts the same `spread_id` at Full.
  - **Group + port + defaults:** site-local, non-assignable multicast group
    `239.255.77.11`, port `7712`, TTL 1, loop on, 3 re-sends. Overridable per
    side (`--multicast-group` / `--multicast-port`); documented.
  - **Intent, not payload, on the wire.** The full plan still crosses the
    authenticated node→coordinator TCP callback and its `sha256` must match the
    intent's — so multicast adds no new cleartext-payload exposure beyond the
    already-documented "authenticated, not encrypted" posture.
- **Gate green:** `cargo build --workspace`, `clippy --workspace -- -D
  warnings`, `cargo fmt --check`, `cargo test --workspace` (239 passing, 26
  suites). New pure-logic tests: intent token/freshness/criteria/canary
  (`pandemic-common::intent`), node `decide_intent` + callback
  (`pandemic-node::spread`), multicast loopback round-trip
  (`pandemic-common::multicast`), and the coordinator flag/record/timestamp
  tests (`pandemic-cli::epidemic`).

#### Increment 4 — Reliability + hardening

Goal: make epidemic production-grade.

- **TLS** on the coordinator→node hop (rustls) — **done (4a).** The node
  serves `--tls --tls-cert --tls-key`; the coordinator spreads with
  `--tls --tls-ca [--tls-server-name]`. Opt-in, so existing cleartext
  loopback stays as-is. Config lives in `pandemic-common/src/tls.rs`
  (`TlsServer`/`TlsClient`); the wire handshake is unchanged (TLS wraps the
  stream). Tests: `pandemic-node` `tls_round_trip`, `tls_client_refuses_untrusted_node`,
  `cleartext_client_cannot_talk_to_tls_node`.
- per-node secrets or mTLS identity in place of the single shared group secret
  — **remaining.**
- **Retries + idempotency** on the apply — **remaining.** (The sibling
  **sender-side audit** entry per spread — which nodes, which plan hash,
  per-node outcome — **shipped as 5a**, the audit/record foundation.)
- **Rate limiting** to prevent spread storms — **remaining.**
- **Payload signing** (the production gate, shared with the registry): a node
  only applies a deployment it can verify — **remaining.**
- **Acceptance:** a spread over an untrusted network is end-to-end encrypted and
  signed; a forged/unsigned deployment is refused; a dropped node is retried and
  reported, not silently lost.

#### Increment 5 — Epidemic observability (console / UI) — **in progress** (5a done)

Goal: a human can **see the spread** — which roster groups exist, what has been
spread, to which nodes, and each node's outcome — and, later, **manage** it
(trigger a spread and watch it progress) from the console, not only the CLI.

Context / why this is new: the console (`pandemic-console`) serves the static SPA
and registers as a daemon plugin, but has **no epidemic surface today**; the REST
API (`pandemic-rest`) exposes no epidemic endpoints. Two traps to keep straight:
(1) the console's existing "Groups" tab is **IAM user-groups**, *not* epidemic
**node rosters** — this increment adds the latter; (2) there is no persistent
"infected nodes" registry — a node is a roster config entry plus the spread
events it produced — so **"see the spread" is the honest first slice**, ahead of
"manage."

- **5a — the audit/record foundation — done.** Both spread paths now record one
  `SpreadRecord` per spread (the roster path previously printed results and
  exited *without* recording; no path recorded per-node detail). `SpreadRecord`
  now carries per-node results (`name` / `addr` / `ok` / `error`), the target
  group (roster `--group`), the plan hash (`sha256`), `mode` (roster/broadcast),
  `stage`, `origin`/`criteria`/`canary` (broadcast). `spread-history.log` is
  **JSONL** (one JSON line per spread) with **legacy TSV lines still readable**
  (`legacy_tsv_record`), so an operator's existing history survives. The
  `SpreadRecord`/`SpreadMode`/`SpreadNodeResult` types live in
  **`pandemic-protocol`** (a deliberate deviation from the original code map's
  "CLI + per-node type in protocol" so that 5b's `pandemic-rest` reads the same
  wire types — one source of truth). *This item **was** the still-remaining
  Increment 4 "sender-side audit entry per spread" — folded in here because it
  is the substrate for observability, not a duplicate.*
  - **Acceptance (met):** in the e2e container, a roster spread-to-self records
    a JSON line (`mode=roster`, `group` present only for `--group`, per-node
    `✓/✗` + error), a partial spread (good node + dead port) records the mixed
    outcome **and** exits non-zero, a broadcast records `mode=broadcast` +
    `origin` + the dialing node's `addr`, and `epidemic spreads` renders all of
    it newest-first (including a pre-existing TSV line in the same file).
  - **Gate green:** build + clippy + fmt + test (247 passing, 26 suites). New
    tests: `spread_record_round_trips_as_one_json_line`,
    `spread_record_missing_optionals_default`, `legacy_tsv_lines_still_load`,
    `legacy_and_json_lines_mix_in_one_file`, `roster_record_shapes_the_audit_entry`,
    `broadcast_node_results_maps_each_callback`.
  - **Bug fixed along the way:** `default_groups_path` fell back to
    `$HOME/pandemic/groups.toml`, contradicting the docs (`~/.config/pandemic/
    groups.toml`, honoring `$XDG_CONFIG_HOME`). Now matches the XDG default;
    verified in the e2e container (a group written to the documented path is
    found by `epidemic nodes` / `spread --group`).
- **5b — read-only "see the spread." — done.** REST: `GET /api/epidemic/spreads?limit=N`
  (default 50, capped at 1000; history newest-first with per-node detail,
  including legacy TSV lines) and `GET /api/epidemic/groups` (roster groups +
  nodes from `groups.toml`), both behind a new `epidemic:read` scope (added to
  the default reader role; admin `*` already covers it). Console: a new top-level
  **Epidemic** section (deliberately distinct from the IAM "Groups" tab) —
  roster-group cards (name, node count, secret path, node list) and
  spread-history cards with per-node ✓/✗ + error, group/origin/criteria/canary,
  and the plan hash. The shared history logic (path resolution + JSONL/legacy-TSV
  parsing + append) moved from the CLI into **`pandemic-common::history`** so
  the CLI and REST can never diverge — `epidemic spreads` now reads through the
  same module (verified: identical output on the same file).
  - **Acceptance (met):** with seeded fixtures (1 legacy TSV line + 2 JSON
    records; groups `edge` + `lab`), the admin key got 200 on both endpoints:
    spreads newest-first (broadcast with `origin`/`criteria`, roster with
    `group` + per-node ✓/✗ + "connection refused", legacy TSV mapped to
    `mode=broadcast` with `canary pct=25` and no per-node detail), groups with
    rosters + secret path; a key *without* `epidemic:read` got 403; no key got
    401. CLI `epidemic spreads` rendered the identical three records.
  - **Gate green:** build + clippy (`-D warnings`) + fmt + test all pass across
    the workspace. New tests: `history_path_honors_xdg_state_home` (common),
    `spreads_query_defaults_limit_and_caps_it` (rest).
- **5c — manage the spread (later).** Trigger a spread from the console
  (`POST /api/epidemic/spread`) and stream live progress over the existing
  `/api/events/stream` websocket.

- **Acceptance:** the operator opens the console and sees the roster groups plus a
  history of spreads with each node's outcome and error (roster *and* broadcast
  spreads); triggering a spread from the console works end-to-end and reports
  progress live.

### Resume point

- **Next up: 5c — manage the spread (trigger + live progress).** 5b is done
  and committed: the REST read surface (`GET /api/epidemic/spreads`,
  `GET /api/epidemic/groups`, scope `epidemic:read`) and the console's
  **Epidemic** section show roster groups + the full spread history; the shared
  history code now lives in `pandemic-common::history` (used by CLI and REST
  alike, write-side included, so 5c can append from a future trigger path).
  5c adds `POST /api/epidemic/spread` (trigger a spread from the console) and
  streams live progress over the existing `/api/events/stream` websocket. The
  rest of Increment 4 (per-node secrets / mTLS identity, retries +
  idempotency, rate limiting, and **payload signing** — the production gate,
  shared with the registry) stays open and is independent of 5c. See the
  Increment 5 section for scope and acceptance.
- **The gate to pass before an increment counts as done** (mirrors CI):
  `cargo build --workspace` && `cargo clippy --workspace -- -D warnings` &&
  `cargo fmt --check` && `cargo test --workspace`.
- **Where each piece lives (code map):**
  - `pandemic-common/src/auth.rs` — handshake crypto (`sign`/`verify`/
    `generate_nonce`/`generate_secret`) + `EPIDEMIC_SECRET_PATH`.
  - `pandemic-common/src/wire.rs` — `authenticate_stream` +
    `send_request_stream` (shared framing for Unix + TCP).
  - `pandemic-common/src/remote.rs` — `RemoteClient` (coordinator→node TCP);
    `with_tls(TlsClient)` opts a connection into the TLS path.
  - `pandemic-common/src/tls.rs` — `TlsServer`/`TlsClient` (rustls config,
    ring provider, PEM in/out); the coordinator→node TLS hop (increment 4a).
    Test fixtures: `pandemic-node/tests/fixtures/` (throwaway self-signed CA +
    node cert/key + an unrelated CA).
  - `pandemic-common/src/discovery.rs` — mDNS advertise + probe
    (`advertise_node`, `discover_nodes`, `DiscoveredNode`, `SERVICE_FQDN`).
  - `pandemic-common/src/multicast.rs` — `send_intent` + `IntentListener`
    (the UDP multicast transport; `DEFAULT_MULTICAST_GROUP`/`PORT`, TTL, re-sends).
  - `pandemic-common/src/intent.rs` — the **pure** broadcast decision logic:
    intent token, freshness, `Criterion` parse/match, canary cohort
    (percentage bucket + subset), `NodeIdentity`. Shared by both sides + tests.
  - `pandemic-common/src/groups.rs` — `NodeConfig`, `GroupConfig`,
    `load_groups[_or_default]`, `find_group`, `merge_roster`,
    `default_groups_path`.
  - `pandemic-common/src/agent.rs` — `AgentClient` (local Unix), `AGENT_SECRET_PATH`.
  - `pandemic-node/src/{main,allowlist}.rs` — the node receiver + allowlist +
    mDNS announce (`--name`/`--no-advertise`, `advertise_for_listen`).
  - `pandemic-node/src/spread.rs` — the node's intent recv loop: `decide_intent`
    (version→token→freshness→dedupe→criteria→canary), `run_intent_listener`,
    `act_on_intent` (dial callback, verify `plan.sha256`, apply).
  - `pandemic-cli/src/epidemic.rs` — coordinator (`spread`/`nodes`/
    `resolve_roster`/`merge_discovered`/`apply_to_node`/
    `resolve_epidemic_secret`/`print_results`) **and** the broadcast path
    (`run_broadcast`/`build_intent`/`serve_one_callback`/`report_and_record`,
    `epidemic spreads` history).
  - `pandemic-cli/src/deployment.rs` — `resolve_deployment_plan` +
    `print_dry_run` (shared by `deployment install` **and** `epidemic spread`).
  - `pandemic-agent/src/main.rs` — agent server (shares `auth`).
  - `pandemic-protocol/` — `AgentRequest`, `Response`, `AuthChallenge`/
    `AuthResponse`, the broadcast types `SpreadStage`, `Canary`, `PlanIdentity`,
    `SpreadIntent`, **and** (5a) the shared spread-history types `SpreadRecord`,
    `SpreadMode`, `SpreadNodeResult` — in protocol (not the CLI) so 5b's REST
    reads the exact same wire shape.
  - `pandemic-common/src/history.rs` — shared spread-history file logic:
    `default_history_path` (XDG_STATE_HOME → `~/.local/state` → `/etc/pandemic`),
    `append_record` (JSONL write), `load_spreads` (JSONL + legacy TSV,
    newest-first, limit). Used by **both** the CLI and REST — one source of
    truth, so `epidemic spreads` and the console can never disagree.
  - **Increment 5 — 5a + 5b done, 5c not yet built.** 5a's data model: the
    types above; `pandemic-cli/src/epidemic.rs` (roster + broadcast both build
    a `SpreadRecord` via `roster_record`/`broadcast_node_results`); history I/O
    in `pandemic-common::history`. 5b's read surface:
    `pandemic-rest/src/handlers.rs` (`get_epidemic_spreads` /
    `get_epidemic_groups`, scope `epidemic:read`; routes registered in
    `pandemic-rest/src/main.rs`) and `pandemic-console/web/src/epidemic.js`
    (the **Epidemic** section, distinct from the IAM "Groups" tab).
    5c still to build: `POST /api/epidemic/spread` (trigger) + live progress
    over the existing `/api/events/stream` websocket.
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