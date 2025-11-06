#!/bin/bash
set -e

# Amazon Linux QEMU image downloader
AL2023_VERSION=2023.9.20251105.0
BASE_URL="https://cdn.amazonlinux.com/al2023/os-images/$AL2023_VERSION/kvm/"
ARCH=${1:-x86_64}

main() {
    local filename="al2023-kvm-${AL2023_VERSION}-kernel-6.1-${ARCH}.xfs.gpt.qcow2"
    local url="$BASE_URL$filename"
    
    echo "Downloading Amazon Linux 2023 QEMU image..."
    echo "Architecture: $ARCH"
    echo "URL: $url"
    
    curl -L -o "$filename" "$url"
    echo "Downloaded: $filename"
}

main "$@"