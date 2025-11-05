# Pandemic

A lightweight daemon for managing "infection" plugins via Unix domain sockets.

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

## Edge Device Optimization

Pandemic is designed for resource-constrained environments:

- **Minimal footprint**: ~10MB total binary size
- **Low memory**: <50MB RAM usage
- **No Docker required**: Native binaries for ARM devices
- **Systemd integration**: Proper service management
- **Unix sockets**: Efficient local IPC

Perfect for Raspberry Pi, embedded Linux, and IoT deployments!