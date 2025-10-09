# Pandemic

A lightweight daemon for managing "infection" plugins via Unix domain sockets.

## Architecture

- **pandemic-daemon**: Core hub managing plugin registry and IPC
- **pandemic-protocol**: Shared message definitions for IPC communication  
- **pandemic-cli**: Privileged tool for systemd service management
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

- **Register**: `{"type": "Register", "plugin": {"name": "...", "description": "...", "config": {...}}}`
- **ListPlugins**: `{"type": "ListPlugins"}`

Responses: `{"status": "Success", "data": ...}` or `{"status": "Error", "message": "..."}`