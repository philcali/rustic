# Pandemic

A lightweight daemon for managing "infection" plugins via Unix domain sockets.

## Architecture

- **pandemic-daemon**: Core hub managing plugin registry, IPC, and health monitoring
- **pandemic-protocol**: Shared message definitions for IPC communication  
- **pandemic-cli**: Privileged tool for systemd service management
- **pandemic-udp**: Launches a UDP server proxy to the daemon
- **pandemic-rest**: HTTP REST API server for web-based access
- **pandemic-console**: Web dashboard for monitoring and managing the daemon
- **infections**: Plugin processes that register with the daemon

## Quick Start

```bash
# Build all components
cargo build

# Start the daemon
./target/debug/pandemic

# In another terminal, run the example plugin
./target/debug/hello-infection
```

## Protocol

Plugins communicate with the daemon over Unix domain sockets using JSON messages:

- **Register**: `{"type": "Register", "plugin": {"name": "...", "version": "...", "description": "...", "config": {...}}}`
- **Deregister**: `{"type": "Deregister", "name": "..."}`
- **ListPlugins**: `{"type": "ListPlugins"}`
- **GetPlugin**: `{"type": "GetPlugin", "name": "..."}`
- **GetHealth**: `{"type": "GetHealth"}`

Responses: `{"status": "Success", "data": ...}`, `{"status": "Error", "message": "..."}`, or `{"status": "NotFound", "message": "..."}`

## Docker Deployment

Build a single image containing all pandemic components:

```bash
# Build the image
docker build -t pandemic .

# Run daemon (default)
docker run -v /tmp/pandemic:/var/run/pandemic pandemic

# Run CLI
docker run -v /tmp/pandemic:/var/run/pandemic pandemic /usr/local/bin/pandemic-cli daemon list

# Run UDP proxy
docker run -p 8080:8080 -v /tmp/pandemic:/var/run/pandemic pandemic /usr/local/bin/pandemic-udp

# Run REST API server
docker run -p 8080:8080 -v /tmp/pandemic:/var/run/pandemic pandemic /usr/local/bin/pandemic-rest

# Run web console
docker run -p 3000:3000 -v /tmp/pandemic:/var/run/pandemic pandemic /usr/local/bin/pandemic-console

# Run example plugin
docker run -v /tmp/pandemic:/var/run/pandemic pandemic /usr/local/bin/hello-infection
```

## E2E Testing with Dockerized Systemd

For end-to-end tests that need a real init system (systemd units, `systemctl`,
journald) without host access or KVM, there is a systemd-in-docker template:
see [`e2e/README.md`](e2e/README.md) for launch instructions and a full
attach/detach walkthrough.

## Deployments (Spec-Driven Install)

Install software as declarative, templated specs — one infection, or a
deployment of several wired together — with dry-run preview, drift-aware
status, ownership-respecting removal, an audit log, and a web console:

```bash
pandemic-cli deployment install ./webapp/deployment.toml --dry-run
sudo pandemic-cli deployment install ./webapp/deployment.toml
sudo pandemic-cli deployment remove webapp --purge
sudo pandemic-cli audit
```

Full how-to — spec anatomy, CLI/REST/console, registry publishing, state
and ownership, rollback and `--purge` policy: see
[`docs/deployments.md`](docs/deployments.md).

## CLI Management

```bash
# List registered plugins
pandemic-cli daemon list

# Get specific plugin details
pandemic-cli daemon get hello-infection

# Get health metrics
pandemic-cli daemon health

# Deregister a plugin
pandemic-cli daemon deregister hello-infection

# Install plugin as systemd service
sudo pandemic-cli service install hello ./target/debug/hello-infection

# Control plugin services
sudo pandemic-cli service start hello
sudo pandemic-cli service stop hello
sudo pandemic-cli service restart hello
```

## REST API

The pandemic-rest infection provides HTTP access to the daemon:

```bash
# Start REST API server
./target/debug/pandemic-rest

# List plugins via HTTP
curl -H "Authorization: Bearer your-api-key" http://localhost:8080/api/plugins

# Get health metrics
curl -H "Authorization: Bearer your-api-key" http://localhost:8080/api/health
```

### Authentication

Configure API keys in `/etc/pandemic/rest-auth.toml`:

```toml
[identities.admin]
api_key = "your-admin-key"
roles = ["admin"]

[roles.admin]
scopes = ["*"]
```

## Web Console

The pandemic-console provides a web-based dashboard:

```bash
# Start web console
./target/debug/pandemic-console

# Access dashboard
open http://localhost:3000
```

### Features
- Real-time system health monitoring
- Plugin registry management
- Responsive web interface
- Configurable API endpoint and authentication