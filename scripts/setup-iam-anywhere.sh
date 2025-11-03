#!/bin/bash

# Setup IAM Roles Anywhere with custom CA
set -e

if [ $# -ne 2 ]; then
    echo "Usage: $0 <trust-anchor-name> <profile-name>"
    exit 1
fi

TRUST_ANCHOR_NAME=$1
PROFILE_NAME=$2
CLIENT_NAME="pandemic-client"
CA_DIR="./ca"

# Check if CA exists
if [ ! -f "$CA_DIR/ca-chain.crt" ]; then
    echo "CA not found. Run create-ca.sh first."
    exit 1
fi

# Create trust anchor
echo "Creating trust anchor..."
TRUST_ANCHOR_ARN=$(aws rolesanywhere create-trust-anchor \
    --name "$TRUST_ANCHOR_NAME" \
    --source sourceType=CERTIFICATE_BUNDLE,sourceData={x509CertificateData="$(base64 -w 0 $CA_DIR/ca-chain.crt)"} \
    --query 'trustAnchor.trustAnchorArn' \
    --output text)

echo "Trust anchor created: $TRUST_ANCHOR_ARN"

# Create IAM role for the profile
ROLE_NAME="${PROFILE_NAME}-role"
cat > trust-policy.json << EOF
{
    "Version": "2012-10-17",
    "Statement": [
        {
            "Effect": "Allow",
            "Principal": {
                "Service": "rolesanywhere.amazonaws.com"
            },
            "Action": [
                "sts:AssumeRole",
                "sts:TagSession",
                "sts:SetSourceIdentity"
            ],
            "Condition": {
                "StringEquals": {
                    "aws:PrincipalTag/x509Subject/CN": "${CLIENT_NAME}"
                },
                "ArnEquals": {
                    "aws:SourceArn": "${TRUST_ANCHOR_ARN}"
                }
            }
        }
    ]
}
EOF

aws iam create-role \
    --role-name "$ROLE_NAME" \
    --assume-role-policy-document file://trust-policy.json

# Attach basic policies
aws iam attach-role-policy \
    --role-name "$ROLE_NAME" \
    --policy-arn "arn:aws:iam::aws:policy/ReadOnlyAccess"

ROLE_ARN=$(aws iam get-role --role-name "$ROLE_NAME" --query 'Role.Arn' --output text)

# Create profile
PROFILE_ARN=$(aws rolesanywhere create-profile \
    --name "$PROFILE_NAME" \
    --role-arns "$ROLE_ARN" \
    --query 'profile.profileArn' \
    --output text)

echo "Profile created: $PROFILE_ARN"

# Output configuration
cat > iam-anywhere-config.toml << EOF
[aws]
trust_anchor_arn = "$TRUST_ANCHOR_ARN"
profile_arn = "$PROFILE_ARN"
role_arn = "$ROLE_ARN"
certificate_path = "$CA_DIR/certs/$CLIENT_NAME.crt"
private_key_path = "$CA_DIR/private/$CLIENT_NAME.key"

[server]
bind_address = "127.0.0.1"
port = 8169
EOF

echo "Configuration saved to iam-anywhere-config.toml"
rm trust-policy.json