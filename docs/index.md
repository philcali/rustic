---
layout: default
---

## Quick Install

```bash
curl -sSL https://raw.githubusercontent.com/philcali/rustic/main/install.sh | sudo bash
```

## Manual Installation

### Download for your architecture:

- **x86_64 (Intel/AMD)**: [pandemic-x86_64-unknown-linux-gnu.tar.gz](https://github.com/philcali/rustic/releases/latest/download/pandemic-x86_64-unknown-linux-gnu.tar.gz)
- **ARMv7 (Raspberry Pi 3/4)**: [pandemic-armv7-unknown-linux-gnueabihf.tar.gz](https://github.com/philcali/rustic/releases/latest/download/pandemic-armv7-unknown-linux-gnueabihf.tar.gz)  
- **ARM64**: [pandemic-aarch64-unknown-linux-gnu.tar.gz](https://github.com/philcali/rustic/releases/latest/download/pandemic-aarch64-unknown-linux-gnu.tar.gz)

### Extract and install:

```bash
tar -xzf pandemic-*.tar.gz
sudo cp pandemic* /usr/local/bin/
sudo useradd --system pandemic
sudo mkdir -p /var/lib/pandemic /etc/pandemic
```

## Getting Started

```bash
# Install daemon service
sudo pandemic-cli bootstrap install

# Start the daemon
sudo pandemic-cli bootstrap start

# Check status
pandemic-cli daemon status

# Run example plugin
hello-infection
```

## Static Registry

Pandemic comes with a software registry that is created for every
release. This registry contains curated infections and core components.

You can explore the registry online at the following URL:

- [Pandemic Registry](https://philcali.github.io/rustic/registry/index.json)

```
pandemic-cli registry search "pandemic"
Found 8 infection(s):

📦 pandemic-cli
   Version: 0.3.0
   Description: Pandemic CLI - command line interface for pandemic daemon

📦 pandemic-iam
   Version: 0.3.0
   Description: Pandemic infection: pandemic-iam

📦 pandemic-udp
   Version: 0.3.0
   Description: Pandemic infection: pandemic-udp

📦 pandemic
   Version: 0.3.0
   Description: Pandemic daemon - core hub for managing infections

📦 pandemic-rest
   Version: 0.3.0
   Description: Pandemic infection: pandemic-rest

📦 pandemic-console
   Version: 0.3.0
   Description: Pandemic infection: pandemic-console

📦 pandemic-agent
   Version: 0.3.0
   Description: Pandemic agent - privileged operations handler

📦 pandemic-proxy
   Version: 0.3.0
   Description: Pandemic infection: pandemic-proxy
```

## Edge Device Optimization

Pandemic is designed for resource-constrained environments:

- **Minimal footprint**: ~10MB total binary size
- **Low memory**: <50MB RAM usage
- **No Docker required**: Native binaries for ARM devices
- **Systemd integration**: Proper service management
- **Unix sockets**: Efficient local IPC

Perfect for Raspberry Pi, embedded Linux, and IoT deployments!