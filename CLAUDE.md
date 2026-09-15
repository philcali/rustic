# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**Pandemic** is a lightweight Rust daemon for managing "infection" plugins (external processes) via Unix domain sockets. The daemon acts as a central hub for plugin registration, IPC, health monitoring, and event distribution.

## Workspace Structure

All crates share version `0.4.0` and workspace dependencies defined in the root `Cargo.toml`.

| Crate | Purpose |
|-------|---------|
| `pandemic-daemon` | Core daemon — listens on a Unix socket, manages plugin registry, event bus, health metrics |
| `pandemic-protocol` | Shared types: `Request`, `Response`, `Event`, `PluginInfo`, `HealthMetrics`, `AgentRequest`, and the spec-driven install types (`InfectionSpec`/`DeploymentSpec`, `InfectionState`/`DeploymentState`) |
| `pandemic-common` | Shared client libraries: `DaemonClient` / `PersistentClient` (daemon IPC), `AgentClient` (admin socket IPC), `RegistryClient` (remote infection registry) |
| `pandemic-cli` | CLI tool: `daemon list/get/health/deregister`, `service install/start/stop/restart`, `service attach/detach`, `infection install/status/uninstall` (spec-driven lifecycle), `deployment install/list/status/remove` (deployment lifecycle), `bootstrap`, `agent` operations |
| `pandemic-agent` | Privileged root-only agent handling systemd service management, user/group management, infection attach/detach (sidecar units), deployment record/list/status/remove (ownership + reverse-order removal), and registry operations |
| `pandemic-rest` | HTTP REST API server (axum) — exposes daemon operations over HTTP with Bearer token auth |
| `pandemic-console` | Web dashboard (Vite + vanilla JS) — serves static SPA, registers as a plugin with the daemon |
| `pandemic-udp` | UDP proxy — exposes the daemon's Unix socket over UDP |
| `pandemic-iam` | IAM Anywhere integration (AWS Roles Anywhere) — certificate-based auth, credential rotation |
| `pandemic-proxy` | Service wrapper — either launches a config-specified process or attaches an existing systemd unit (`--attach`) and registers it with the daemon |
| `pandemic-mqtt` | One-way MQTT bridge — subscribes to the daemon's event bus and republishes every event to an MQTT broker (retained status/health/plugins snapshots on (re)connect, LWT offline marker) |
| `examples/hello-infection` | Example infection plugin |

## Architecture

The daemon (`pandemic`) is the central process. It:

1. Listens on a Unix domain socket (default: `/var/run/pandemic/pandemic.sock`)
2. Accepts JSON-over-line connections from plugins and clients
3. Maintains a plugin registry (`HashMap<String, PluginInfo>`)
4. Runs an `EventBus` for pub/sub event distribution (topic-based with wildcard support)
5. Collects system health metrics (CPU, memory, uptime, load average)

Plugins communicate with the daemon via `Request`/`Response`/`Event` messages over Unix sockets (line-delimited JSON). The `pandemic-agent` (root-only) handles privileged operations via a separate admin socket at `/var/run/pandemic/admin.sock`.

`service attach <unit>` wraps an existing systemd unit as an infection: the agent writes `/etc/pandemic/infections/<name>.toml` (with `attach = "<unit>"`) plus a sidecar unit `pandemic-<name>.service` that runs `pandemic-proxy --attach <unit>`; detach removes both. Agent auth is HMAC-SHA256 challenge/response; the shared secret resolves in order `--secret` → `--secret-path` → `/etc/pandemic/agent-secret` (0600, minted by `bootstrap install --with-agent` / `agent install`) → random (logged, last resort). Agent wire messages are bare (`AgentRequest`/`AuthResponse` serialized directly, no `AgentMessage` wrapper) to match the daemon pattern.

`infection install <spec> [--set k=v ...]` is the spec-driven lifecycle: the CLI resolves variables (`--set` > defaults), validates, and renders **all** templates before touching the host, then applies via sequenced agent primitives (packages → groups → users → files → unit/attach), health-checks, and records state at `/etc/pandemic/infections/<name>/state.toml` (dir 0700, file 0600: resolved spec, variable values, file list with sha256, unit name, install timestamp). `infection status [name]` and `infection uninstall <name>` cover both recorded infections and the legacy `service attach` bridge. On failure the CLI reports the failing step plus the steps already applied (re-running is safe — completed steps are idempotent).

`deployment install <spec> [--set k=v ...] [--dry-run]` composes a deployment from multiple infections: it resolves **shared** variables (`--set` > `vars` bindings > declared defaults, iteratively so values may reference each other) plus per-infection `vars` overrides, renders every infection, and applies them in the deployment's declared `order`, recording an `owner` (the deployment name) on each infection. State lands at `/etc/pandemic/deployments/<name>/state.toml` (dir 0700, file 0600: shared variables, ordered owned infections). `deployment list` and `deployment status [name]` re-check reality (unit active/inactive, rendered-file hashes) against the record. `deployment remove <name>` uninstalls only the infections it owns, in **reverse** order. Re-running `deployment install` under an existing name is an idempotent re-apply/upgrade. Ownership is precise: a deployment **refuses** to adopt an infection that is standalone or already owned by another deployment (v1 refuses rather than merges), and `infection install` refuses to install an infection that a deployment owns. The agent stays a dumb executor — only individual primitives + record/list/status/remove requests cross the wire, never the deployment spec.

The event bus supports wildcard topics (`plugin.deregistered*` matches `plugin.deregistered`).

## Building & Running

```bash
# Build everything
cargo build

# Build web assets for pandemic-console (required before building the console)
cd pandemic-console/web && npm install && npm run build && cd ../..

# Run the daemon
cargo run -p pandemic-daemon

# Run the REST API server
cargo run -p pandemic-rest

# Run the web console
cargo run -p pandemic-console

# Run the UDP proxy
cargo run -p pandemic-udp

# Run the MQTT bridge (needs a broker, e.g. mosquitto)
cargo run -p pandemic-mqtt -- --broker-url mqtt://127.0.0.1:1883 --topic-prefix pandemic

# Run the example infection
cargo run -p hello-infection

# Run the CLI
cargo run -p pandemic-cli -- daemon list
```

## Testing & Linting

```bash
# Run all tests
cargo test --workspace

# Run clippy (CI enforces -D warnings)
cargo clippy --workspace -- -D warnings

# Check formatting
cargo fmt --check
```

For E2E tests that need a real init system (systemd units, `systemctl`, journald) without host access, use the dockerized-systemd template in [`e2e/`](e2e/README.md).

## CI/CD

Three GitHub Actions workflows:
- **ci.yml**: Runs on push/PR to `main` — builds, tests, clippy, fmt
- **build.yml**: Multi-arch build (x86_64, armv7, aarch64 musl) — called by release workflow
- **release.yml**: On `v*` tag push — builds, creates GitHub release, deploys docs to GitHub Pages, generates registry index

## Key Scripts

| Script | Purpose |
|--------|---------|
| `scripts/bump-version.sh <ver>` | Bumps version across all Cargo.toml files |
| `scripts/generate-registry.sh` | Generates registry index.json and per-binary manifests for GitHub Pages hosting |
| `scripts/setup-iam-anywhere.sh` | Sets up IAM Anywhere certificates |
| `scripts/create-ca.sh` | Creates CA for certificate generation |
| `scripts/create-client-cert.sh` | Creates client certificates |
| `scripts/setup-complete.sh` | Full setup script |
| `install.sh` | One-line installer (downloads from GitHub releases) |

## Protocol Details

Messages are line-delimited JSON. The daemon uses `serde_json` with `#[serde(tag = "type")]` for request types and `#[serde(tag = "status")]` for responses.

**Request types**: `Register`, `Deregister`, `ListPlugins`, `GetPlugin`, `Subscribe`, `Unsubscribe`, `Publish`, `GetHealth`

**AgentRequest types**: `GetHealth`, `GetCapabilities`, `ListServices`, `SystemdControl` (start/stop/restart/enable/disable/status/daemon-reload), `UserCreate/Delete/Modify`, `ListUsers`, `GroupCreate/Delete/AddUser/RemoveUser`, `ListGroups`, `ServiceConfigOverride/Reset`, `GetServiceConfig`, `GetInfectionManifest`, `InstallInfection`, `AttachInfection`/`DetachInfection`, `PackageInstall`, `WriteFile`, `RecordInfection`, `ListInfections`, `GetInfectionStatus`, `UninstallInfection`, `RecordDeployment`, `ListDeployments`, `GetDeploymentStatus`, `RemoveDeployment`

**Response types**: `Success { data }`, `Error { message }`, `NotFound { message }`

## Version Bumping

```bash
./scripts/bump-version.sh 0.5.0
# Then: git add . && git commit -m "Bump version to v0.5.0" && git tag v0.5.0 && git push origin main --tags
```

## Planned Improvements

See `TODO.md` for the current list of planned improvements and their status.

## Docker

```bash
docker build -t pandemic .
# Default runs the daemon
docker run -v /tmp/pandemic:/var/run/pandemic pandemic
# Override entrypoint to run other components
docker run -v /tmp/pandemic:/var/run/pandemic pandemic /usr/local/bin/pandemic-cli daemon list
```
