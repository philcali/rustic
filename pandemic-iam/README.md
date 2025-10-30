# Pandemic IAM Infection

AWS IAM Anywhere integration for the Pandemic daemon, providing IMDSv2-compatible credential distribution.

## Overview

The pandemic-iam infection integrates with AWS IAM Anywhere to provide temporary AWS credentials to other pandemic components and external applications. It exposes IMDSv2-compatible HTTP endpoints that existing AWS SDKs can consume without modification.

## Features

- **IMDSv2 Compatibility**: Provides AWS credential endpoints compatible with EC2 Instance Metadata Service v2
- **X.509 Certificate Authentication**: Uses client certificates for secure credential exchange
- **Credential Caching**: Automatically refreshes credentials before expiration
- **Role Mapping**: Maps different services to specific IAM roles
- **Session Token Management**: Implements IMDSv2 session token requirements

## Configuration

Create `/etc/pandemic/iam-config.toml`:

```toml
[server]
host = "127.0.0.1"
port = 8080

[aws]
certificate_path = "/etc/pandemic/certs/client.crt"
private_key_path = "/etc/pandemic/certs/client.key"
trust_anchor_arn = "arn:aws:rolesanywhere:us-east-1:123456789012:trust-anchor/12345678-1234-1234-1234-123456789012"
profile_arn = "arn:aws:rolesanywhere:us-east-1:123456789012:profile/pandemic-profile"
role_arn = "arn:aws:iam::123456789012:role/PandemicRole"

[role_mappings]
"pandemic-daemon" = "arn:aws:iam::123456789012:role/PandemicDaemonRole"
"pandemic-rest" = "arn:aws:iam::123456789012:role/PandemicRestRole"
"default" = "arn:aws:iam::123456789012:role/PandemicDefaultRole"
```

## Usage

### Starting the Service

```bash
# Start pandemic-iam infection
./target/debug/pandemic-iam

# The service will register with the pandemic daemon and start the HTTP server
```

### Using with AWS SDKs

Configure your AWS SDK to use the pandemic-iam endpoint:

```bash
# Set environment variables
export AWS_EC2_METADATA_SERVICE_ENDPOINT=http://127.0.0.1:8080
export AWS_EC2_METADATA_SERVICE_ENDPOINT_MODE=IPv4

# Your application will now use pandemic-iam for credentials
python your_aws_app.py
```

### IMDSv2 Endpoints

The service provides these IMDSv2-compatible endpoints:

```bash
# Get session token (required for IMDSv2)
curl -X PUT "http://127.0.0.1:8080/latest/api/token" \
     -H "X-aws-ec2-metadata-token-ttl-seconds: 21600"

# List available roles
curl -H "X-aws-ec2-metadata-token: $TOKEN" \
     "http://127.0.0.1:8080/latest/meta-data/iam/security-credentials/"

# Get credentials for a role
curl -H "X-aws-ec2-metadata-token: $TOKEN" \
     "http://127.0.0.1:8080/latest/meta-data/iam/security-credentials/pandemic-role"
```

## IAM Anywhere Setup

### 1. Create Trust Anchor

```bash
aws rolesanywhere create-trust-anchor \
    --name "pandemic-trust-anchor" \
    --source sourceType=CERTIFICATE_BUNDLE,sourceData=file://ca-cert.pem
```

### 2. Create Profile

```bash
aws rolesanywhere create-profile \
    --name "pandemic-profile" \
    --role-arns "arn:aws:iam::123456789012:role/PandemicRole"
```

### 3. Generate Client Certificate

```bash
# Generate private key
openssl genrsa -out client.key 2048

# Create certificate signing request
openssl req -new -key client.key -out client.csr \
    -subj "/CN=pandemic-client"

# Sign with your CA (or use self-signed for testing)
openssl x509 -req -in client.csr -CA ca-cert.pem -CAkey ca-key.pem \
    -CAcreateserial -out client.crt -days 365
```

## Implementation Status

- ✅ HTTP server with IMDSv2 endpoints
- ✅ Session token management
- ✅ Configuration management
- ✅ Daemon registration
- ✅ Credential caching structure
- 🚧 **IAM Anywhere API integration** (currently using mock credentials)
- 🚧 X.509 certificate validation
- 🚧 Automatic credential refresh

## Development

The current implementation provides a working HTTP server with mock credentials. To complete the IAM Anywhere integration:

1. Implement X.509 certificate loading and validation
2. Add AWS IAM Anywhere API calls using the certificate for authentication
3. Parse and cache the returned temporary credentials
4. Add proper error handling and retry logic

## Security Considerations

- Store certificates and private keys securely with appropriate file permissions
- Use strong X.509 certificates with proper validation
- Implement proper session token validation
- Consider network security for the HTTP endpoints
- Regularly rotate certificates and monitor for compromise