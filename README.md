# Pandemic

A lightweight daemon for managing "infection" plugins via Unix domain sockets.

## Architecture

- **pandemic-daemon**: Core hub managing plugin registry and IPC
- **pandemic-protocol**: Shared message definitions for IPC communication  
- **pandemic-cli**: Privileged tool for systemd service management
- **pandemic-udp**: Launches a UDP server proxy to the daemon
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

# Run example plugin
docker run -v /tmp/pandemic:/var/run/pandemic pandemic /usr/local/bin/hello-infection
```

## CLI Management

```bash
# List registered plugins
pandemic-cli daemon list

# Get specific plugin details
pandemic-cli daemon get hello-infection

# Deregister a plugin
pandemic-cli daemon deregister hello-infection

# Install plugin as systemd service
sudo pandemic-cli service install hello ./target/debug/hello-infection

# Control plugin services
sudo pandemic-cli service start hello
sudo pandemic-cli service stop hello
sudo pandemic-cli service restart hello
```