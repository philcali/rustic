#!/bin/bash
set -e

# Pandemic installation script
REPO="philcali/rustic"
INSTALL_DIR="/usr/local/bin"
TEMP_DIR=$(mktemp -d)

# Detect architecture
detect_arch() {
    local arch=$(uname -m)
    case $arch in
        x86_64) echo "x86_64-unknown-linux-gnu" ;;
        armv7l) echo "armv7-unknown-linux-gnueabihf" ;;
        aarch64) echo "aarch64-unknown-linux-gnu" ;;
        *) echo "Unsupported architecture: $arch" >&2; exit 1 ;;
    esac
}

# Get latest release
get_latest_release() {
    curl -s "https://api.github.com/repos/$REPO/releases/latest" | \
        grep '"tag_name":' | \
        sed -E 's/.*"([^"]+)".*/\1/'
}

main() {
    echo "🦠 Installing Pandemic..."
    
    local target=$(detect_arch)
    local version=$(get_latest_release)
    local url="https://github.com/$REPO/releases/download/$version/pandemic-$target.tar.gz"
    
    echo "Architecture: $target"
    echo "Version: $version"
    
    # Download and extract
    cd "$TEMP_DIR"
    curl -L "$url" | tar -xz
    
    # Install binaries
    sudo cp pandemic pandemic-cli pandemic-udp pandemic-rest pandemic-console pandemic-iam "$INSTALL_DIR/"
    sudo chmod +x "$INSTALL_DIR"/pandemic*
    
    # Create user and group
    if ! id pandemic >/dev/null 2>&1; then
        sudo useradd --system --shell /bin/false --home-dir /var/lib/pandemic pandemic
    fi
    
    # Create directories
    sudo mkdir -p /var/lib/pandemic /etc/pandemic
    sudo chown pandemic:pandemic /var/lib/pandemic
    
    echo "✅ Pandemic installed successfully!"
    echo ""
    echo "Next steps:"
    echo "1. Install daemon service: sudo pandemic-cli bootstrap install"
    echo "2. Start the daemon: sudo pandemic-cli bootstrap start"
    echo "3. Check status: pandemic-cli daemon status"
    
    # Cleanup
    rm -rf "$TEMP_DIR"
}

main "$@"