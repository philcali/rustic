# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**Pandemic** is a lightweight Rust daemon for managing "infection" plugins (external processes) via Unix domain sockets. The daemon acts as a central hub for plugin registration, IPC, health monitoring, and event distribution.

## Workspace Structure

All crates share version `0.4.0` and workspace dependencies defined in the root `Cargo.toml`.

| Crate | Purpose |
|-------|---------|
| `pandemic-daemon` | Core daemon — listens on a Unix socket, manages plugin registry, event bus, health metrics |
| `pandemic-protocol` | Shared types: `Request`, `Response`, `Event`, `PluginInfo`, `HealthMetrics`, `AgentRequest` |
| `pandemic-common` | Shared client libraries: `DaemonClient` / `PersistentClient` (daemon IPC), `AgentClient` (admin socket IPC), `RegistryClient` (remote infection registry) |
| `pandemic-cli` | CLI tool: `daemon list/get/health/deregister`, `service install/start/stop/restart`, `bootstrap`, `agent` operations |
| `pandemic-agent` | Privileged root-only agent handling systemd service management, user/group management, and registry operations |
| `pandemic-rest` | HTTP REST API server (axum) — exposes daemon operations over HTTP with Bearer token auth |
| `pandemic-console` | Web dashboard (Vite + vanilla JS) — serves static SPA, registers as a plugin with the daemon |
| `pandemic-udp` | UDP proxy — exposes the daemon's Unix socket over UDP |
| `pandemic-iam` | IAM Anywhere integration (AWS Roles Anywhere) — certificate-based auth, credential rotation |
| `pandemic-proxy` | Service wrapper — launches a config-specified process and registers it with the daemon |
| `examples/hello-infection` | Example infection plugin |

## Architecture

The daemon (`pandemic`) is the central process. It:

1. Listens on a Unix domain socket (default: `/var/run/pandemic/pandemic.sock`)
2. Accepts JSON-over-line connections from plugins and clients
3. Maintains a plugin registry (`HashMap<String, PluginInfo>`)
4. Runs an `EventBus` for pub/sub event distribution (topic-based with wildcard support)
5. Collects system health metrics (CPU, memory, uptime, load average)

Plugins communicate with the daemon via `Request`/`Response`/`Event` messages over Unix sockets (line-delimited JSON). The `pandemic-agent` (root-only) handles privileged operations via a separate admin socket at `/var/run/pandemic/admin.sock`.

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

**AgentRequest types**: `GetHealth`, `GetCapabilities`, `ListServices`, `SystemdControl`, `UserCreate/Delete/Modify`, `GroupCreate/Delete/AddUser/RemoveUser`, `ServiceConfigOverride/Reset`, `SearchInfections`, `GetInfectionManifest`, `InstallInfection`

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
