#!/bin/bash

# Complete setup for IAM Roles Anywhere with Pandemic
set -e

echo "Setting up IAM Roles Anywhere integration..."

# Step 1: Create CA infrastructure
echo "1. Creating CA infrastructure..."
chmod +x ./scripts/create-ca.sh
./scripts/create-ca.sh

# Step 2: Create client certificate
echo "2. Creating client certificate..."
chmod +x ./scripts/create-client-cert.sh
./scripts/create-client-cert.sh pandemic-client

# Step 3: Setup IAM Roles Anywhere resources
echo "3. Setting up IAM Roles Anywhere resources..."
chmod +x ./scripts/setup-iam-anywhere.sh
./scripts/setup-iam-anywhere.sh pandemic-trust-anchor pandemic-profile

# Step 4: Copy config to expected location
echo "4. Setting up configuration..."
sudo mkdir -p /etc/pandemic
sudo cp iam-anywhere-config.toml /etc/pandemic/iam-config.toml

echo "Setup complete!"
echo ""
echo "Next steps:"
echo "1. Build the pandemic-iam service: cargo build --bin pandemic-iam"
echo "2. Start the pandemic daemon: ./target/debug/pandemic"
echo "3. Start the IAM service: ./target/debug/pandemic-iam"
echo "4. Test with: curl -X PUT http://localhost:8169/latest/api/token -H 'X-aws-ec2-metadata-token-ttl-seconds: 21600'"