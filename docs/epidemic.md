# Epidemic: Spreading a Deployment to a Group of Nodes

**Epidemic** spreads a resolved deployment to many hosts at once. It is the
networking half of the deployment feature: the *coordinator* runs the same
pure **Plan** step you already trust from `deployment install` — resolve
variables, render every template, validate — exactly once, locally. It then
sends that single concrete plan to every **node** in a **group**; each node
runs the privileged **Apply** locally through its own agent.

One plan, many hosts, per-node results. The agent stays a dumb executor —
it never sees the group, the roster, or the other nodes. Only the deployment
payload crosses the wire.

## The primitive: node / group / coordinator

The feature is built on three small ideas, not on a bespoke protocol:

- **Node** — a host running the `pandemic-node` receiver. An identity
  (`name`) plus a network endpoint (`addr:port`). It exposes a *narrow*
  deployment surface and forwards approved work to the local agent.
- **Group** — a named trust boundary: one shared **epidemic secret** plus a
  roster of nodes that joined it. Groups are declared in a local TOML file.
- **Coordinator** — `pandemic-cli epidemic`. Holds a group's secret + roster,
  plans the deployment, applies it to every node, and reports per-node
  results. A partial spread is a *failure*, not a success.

The wire protocol is the existing agent protocol (line-delimited JSON, one
handshake + request/response), carried over TCP instead of a Unix socket. The
handshake crypto, framing, and secret handling all live in `pandemic-common`
so a node, an agent, and a coordinator agree byte-for-byte.

## Two secrets, two hops

There are two distinct secrets because there are two distinct trust
boundaries. Mixing them up is the classic footgun, so name them by the hop
they guard:

| Secret | Path (default) | Guards |
|---|---|---|
| **Epidemic** secret | `/etc/pandemic/epidemic-secret` | The **network** hop: coordinator → node (TCP) |
| **Agent** secret | `/etc/pandemic/agent-secret` | The **local** hop: node → agent (Unix socket) |

Both handshakes are the same HMAC-SHA256 challenge/response:

```
server → client:  { "type": "AuthChallenge", "nonce": "<32 hex>" }
client → server:  { "type": "AuthResponse", "nonce": "<same>",
                    "signature": "<HMAC-SHA256(secret, nonce), 64 hex>" }
```

The server verifies in constant time and drops the connection on mismatch.
The signature proves the peer holds the shared secret; it does **not**
encrypt the payload that follows (see Security notes).

## Declaring a group

Groups live in `~/.config/pandemic/groups.toml` (honors `$XDG_CONFIG_HOME`):

```toml
# A group is a named trust boundary: a shared secret + a roster of nodes.
[[group]]
name = "edge"
secret_path = "/etc/pandemic/secrets/edge"   # optional

[[group.node]]
name = "edge-1"
addr = "192.168.1.10:7711"

[[group.node]]
name = "edge-2"
addr = "192.168.1.11:7711"

# A group without a secret_path falls back to the default path.
[[group]]
name = "lab"

[[group.node]]
name = "lab-a"
addr = "127.0.0.1:7711"
```

`secret_path` is the file holding the group's epidemic secret. When it is
omitted the coordinator falls back to the default path.

## Running a node receiver

Each target host runs `pandemic-node` (root — it forwards to the root-only
agent). It listens on TCP, authenticates the coordinator with the epidemic
secret, and forwards each **allowed** request to the local agent over the
Unix admin socket:

```bash
# Listen on loopback (default), pick up both secrets from their default paths:
sudo pandemic-node

# Explicit, non-loopback bind for real networking:
sudo pandemic-node \
  --listen 0.0.0.0:7711 \
  --secret /etc/pandemic/epidemic-secret \
  --agent-secret /etc/pandemic/agent-secret
```

Flags:

| Flag | Default | Meaning |
|---|---|---|
| `--listen` | `127.0.0.1:7711` | TCP address to bind (the coordinator → node hop) |
| `--secret` / `--secret-path` | — | Epidemic (network) shared secret, inline or file |
| `--agent-socket` | `/var/run/pandemic/admin.sock` | Local agent admin socket to forward to |
| `--agent-secret` / `--agent-secret-path` | — | Agent (local) shared secret, inline or file |
| `--name` | hostname | mDNS instance name the node advertises under (see Discovery) |
| `--no-advertise` | off | Disable mDNS advertisement entirely |

### Discovery (mDNS / Bonjour)

By default a node **advertises itself over mDNS** so a coordinator on the same
LAN can find it without hand-typing the roster. The node publishes the
service type `_pandemic-node._tcp` (DNS-SD, RFC 6335) on the interface that
matches `--listen`:

- **instance name** = `--name` (or the hostname) — the node's identity;
- **port** from `--listen` (carried in the SRV record);
- **address** from `--listen` (the A record); for a `0.0.0.0` bind the node
  picks its primary LAN IPv4;
- a `pandemic=<version>` TXT record for tooling.

This advertisement is **purely a discovery aid**: it adds no network surface
and does not change the TCP/allowlist model. If mDNS cannot start (no usable
IPv4 interface, multicast blocked, …) the node logs a warning and keeps serving
its TCP surface — it simply won't be discoverable.

A loopback bind (`--listen 127.0.0.1:…`, the default) advertises on loopback,
so it is only discoverable from the same host — the safe default for local
testing. Bind `0.0.0.0` to make the node discoverable (and reachable) on the
LAN.

### The narrow surface

A node is a deployment endpoint, not a general remote shell. Only these
requests are forwarded; everything else is answered with a refusal and never
reaches the agent:

- `GetCapabilities`
- `ApplyDeployment`
- `GetDeploymentStatus`
- `ListDeployments`
- `PreviewDeployment`

So a compromised coordinator can only *deploy* on a node — it cannot ask the
node to delete users, install packages ad hoc, or read the agent's state.

## Spreading from the CLI

```bash
# Ad-hoc: spread a local deployment spec to one or more endpoints.
# --node is repeatable; no groups file needed.
pandemic-cli epidemic spread ./webapp/deployment.toml \
  --node 127.0.0.1:7711 --node 192.168.1.10:7711

# By named group: the group's roster is the target list (plus any extra --node).
pandemic-cli epidemic spread ./webapp/deployment.toml --group edge

# By registry deployment name (checksummed bundle).
pandemic-cli epidemic spread rest-mqtt --group edge \
  --registry-url https://example.com/registry

# Shared variables, same precedence as `deployment install`.
pandemic-cli epidemic spread ./webapp/deployment.toml --group edge \
  --set api_port=9090

# Preview: print the node list + the resolved plan. Touches nothing.
pandemic-cli epidemic spread ./webapp/deployment.toml --group edge --dry-run

# See your groups and rosters.
pandemic-cli epidemic nodes            # all groups
pandemic-cli epidemic nodes --group edge

# Discovery (mDNS): list the nodes advertising on the LAN, and/or spread to
# whatever is found (unioned with any --group / --node, deduped by address).
pandemic-cli epidemic nodes --discover --interface 192.168.1.0
pandemic-cli epidemic spread ./webapp/deployment.toml --discover
pandemic-cli epidemic spread ./webapp/deployment.toml --discover --dry-run
```

### Discovery flags

`--discover` (on `nodes` and `spread`) probes the LAN over mDNS for
`_pandemic-node._tcp` and uses what it finds:

| Flag | Default | Meaning |
|---|---|---|
| `--discover` | off | Also target / list nodes found via mDNS |
| `--interface` | all interfaces | The IPv4 address to probe on (e.g. `192.168.1.5`; for loopback, `127.0.0.1`) |
| `--timeout` | `3` | mDNS probe timeout in seconds |

`epidemic nodes --discover` prints the discovered nodes (instance name →
`addr:port`) above your configured groups. `epidemic spread --discover` unions
the discovered nodes with any `--group` / `--node` roster, deduplicating by
address (a roster entry's configured name wins over the discovered one). On
`spread`, a discovery probe failure is a **warning** — the explicit roster is
still spread to; on `nodes` it is an error, since discovery is the point.

Because mDNS is per-interface, use `--interface` to target a specific LAN (or
loopback for local testing) rather than probing every interface.

`spread <target>` resolves `target` (local spec path **or** registry name)
with the *same* `resolve_deployment_plan` that `deployment install` uses — so
what you dry-run locally is exactly what every node applies.

The epidemic secret resolves in this order (first that is set wins):

1. `--secret` (inline)
2. `--secret-path` (file)
3. the group's `secret_path` (from the groups file)
4. `/etc/pandemic/epidemic-secret`
5. freshly generated (a bootstrap aid — it is printed and a warning is shown;
   every node must be given the same value or the handshakes fail)

`--secret` / `--secret-path` are top-level flags on the `epidemic` verb, e.g.
`pandemic-cli epidemic --secret <value> spread …`.

### Results

The coordinator applies the identical plan to each node in roster order and
prints one line per node:

```
  ✓  edge-1/192.168.1.10:7711  applied
  ✗  edge-2/192.168.1.11:7711  FAILED

⚠️  spread of 'webapp' failed on 1 of 2 node(s):
  ✗  192.168.1.11:7711: node 192.168.1.11:7711 failed to apply: …
```

If **any** node fails, the command exits non-zero. A partial spread is
reported as a failure — it is never silently treated as success.

## How it fits together

```
                 ┌──────────────────────── coordinator ────────────────────────┐
                 │  pandemic-cli epidemic spread <target> --group edge          │
                 │  1. resolve roster (groups file) + secret                   │
                 │  2. Plan locally (resolve, render, validate) — once         │
                 │  3. for each node: TCP + epidemic handshake → ApplyDeploy   │
                 └───────┬───────────────────────────────┬──────────────────────┘
        epidemic secret  │                               │  epidemic secret
                 ┌───────▼──────┐                 ┌───────▼──────┐
                 │   node-1     │                 │   node-2     │
                 │pandemic-node │                 │pandemic-node │
                 │ (allowlist)  │                 │ (allowlist)  │
                 └───────┬──────┘                 └───────┬──────┘
          agent secret    │ Unix socket                   │  agent secret
                 ┌───────▼──────┐                 ┌───────▼──────┐
                 │   agent-1    │                 │   agent-2    │
                 │  (Apply)     │                 │  (Apply)     │
                 └──────────────┘                 └──────────────┘
```

Each node is an independent endpoint: the coordinator never relays through a
node, and a node never learns about its peers.

## Security notes

- **Two secrets, two boundaries.** The epidemic secret only authorizes the
  network hop; the agent secret only authorizes the local hop. Keep them
  distinct per group.
- **Narrow allowlist.** A node can only be told to deploy/inspect deployments.
  There is no remote user/package/file primitive on the wire.
- **No values on the wire beyond the plan.** The coordinator sends the
  resolved deployment (variables + rendered infections) — the same bytes
  `deployment install` would apply locally. State records and the audit log on
  each node stay `0600` root-only.
- **Transport is authenticated, not encrypted (yet).** The HMAC handshake
  proves the peer holds the shared secret, but the deployment payload travels
  in cleartext over TCP. For anything beyond a trusted LAN, put the node
  behind a firewall / VPN, and treat **TLS + payload signing** as required
  before exposing it on untrusted networks. Signatures are the shared
  production gate with the registry (see `ideas/epidemic_infections.md`).
- **Default bind is loopback.** `pandemic-node` listens on `127.0.0.1` by
  default. Bind `0.0.0.0` deliberately and firewall port `7711`.

## Roadmap

The larger epidemic vision (discovery, multicast, reliability) is tracked in
[`ideas/epidemic_infections.md`](../ideas/epidemic_infections.md):

1. **Node / group / coordinator + TCP spread** — *done*.
2. **Discovery** — mDNS/Bonjour so the coordinator doesn't hand-type the roster
   — *done* (node advertises; `epidemic nodes/spread --discover`).
3. **Multicast + targeting** — subnet-wide spread, target criteria, canary.
4. **Reliability + hardening** — retries, sender-side audit, rate limiting,
   per-node secrets, TLS, payload signing.

## Known issues

- **mDNS probe can print a spurious panic.** The `agnostic-mdns` /
  `dns-protocol-patch` stack occasionally panics on a tokio worker thread
  while parsing a packet in its receive loop (observed ~1 in 9 loopback
  probes: `range start index … out of range for slice …` in `dns-protocol-patch`).
  It is **non-fatal** — discovery still returns the correct nodes and the
  command exits normally — but the panic text is printed to stderr. It is an
  upstream library bug (we are on the latest published versions), not a fault
  in how we drive the client (the same-process advertise→discover round-trip
  passes cleanly). If it becomes a problem, the workaround is to probe again.
