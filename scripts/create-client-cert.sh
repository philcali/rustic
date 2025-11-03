#!/bin/bash

# Create client certificate for IAM Roles Anywhere
set -e

if [ $# -ne 1 ]; then
    echo "Usage: $0 <client-name>"
    exit 1
fi

CLIENT_NAME=$1
CA_DIR="./ca"

# Generate client private key
openssl genrsa -out $CA_DIR/private/${CLIENT_NAME}.key 2048
chmod 400 $CA_DIR/private/${CLIENT_NAME}.key

# Create client certificate config
cat > $CA_DIR/${CLIENT_NAME}.conf << EOF
[req]
distinguished_name = req_distinguished_name
req_extensions = v3_req
prompt = no

[req_distinguished_name]
C = US
ST = VA
L = Herndon
O = Pandemic Systems
OU = Clients
CN = ${CLIENT_NAME}

[v3_req]
keyUsage = keyEncipherment, dataEncipherment, digitalSignature
extendedKeyUsage = clientAuth
EOF

# Generate CSR
openssl req -new -key $CA_DIR/private/${CLIENT_NAME}.key \
    -out $CA_DIR/csr/${CLIENT_NAME}.csr \
    -config $CA_DIR/${CLIENT_NAME}.conf

# Sign with intermediate CA
openssl x509 -req -in $CA_DIR/csr/${CLIENT_NAME}.csr \
    -CA $CA_DIR/intermediate/intermediate-ca.crt \
    -CAkey $CA_DIR/private/intermediate-ca.key \
    -out $CA_DIR/certs/${CLIENT_NAME}.crt \
    -days 365 \
    -extensions v3_req \
    -extfile $CA_DIR/${CLIENT_NAME}.conf \
    -CAcreateserial

echo "Client certificate created: $CA_DIR/certs/${CLIENT_NAME}.crt"
echo "Client private key: $CA_DIR/private/${CLIENT_NAME}.key"