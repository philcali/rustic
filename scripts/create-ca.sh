#!/bin/bash

# Create CA infrastructure for IAM Roles Anywhere
set -e

CA_DIR="./ca"
mkdir -p $CA_DIR/{root,intermediate,certs,private,csr}
chmod 700 $CA_DIR/private

# Generate root CA private key
openssl genrsa -out $CA_DIR/private/root-ca.key 4096
chmod 400 $CA_DIR/private/root-ca.key

# Create root CA certificate
cat > $CA_DIR/root-ca.conf << EOF
[req]
distinguished_name = req_distinguished_name
x509_extensions = v3_ca
prompt = no

[req_distinguished_name]
C = US
ST = VA
L = Herndon
O = Pandemic Systems
OU = Security
CN = Pandemic Root CA

[v3_ca]
basicConstraints = critical,CA:TRUE
keyUsage = critical,keyCertSign,cRLSign
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid:always,issuer
EOF

openssl req -new -x509 -key $CA_DIR/private/root-ca.key \
    -out $CA_DIR/root/root-ca.crt \
    -days 3650 \
    -config $CA_DIR/root-ca.conf

# Generate intermediate CA private key
openssl genrsa -out $CA_DIR/private/intermediate-ca.key 4096
chmod 400 $CA_DIR/private/intermediate-ca.key

# Create intermediate CA CSR
cat > $CA_DIR/intermediate-ca.conf << EOF
[req]
distinguished_name = req_distinguished_name
prompt = no

[req_distinguished_name]
C = US
ST = VA
L = Herndon
O = Pandemic Systems
OU = Security
CN = Pandemic Intermediate CA
EOF

openssl req -new -key $CA_DIR/private/intermediate-ca.key \
    -out $CA_DIR/csr/intermediate-ca.csr \
    -config $CA_DIR/intermediate-ca.conf

# Sign intermediate CA with root CA
cat > $CA_DIR/intermediate-signing.conf << EOF
[v3_intermediate_ca]
basicConstraints = critical,CA:TRUE,pathlen:0
keyUsage = critical,keyCertSign,cRLSign
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid:always,issuer
EOF

openssl x509 -req -in $CA_DIR/csr/intermediate-ca.csr \
    -CA $CA_DIR/root/root-ca.crt \
    -CAkey $CA_DIR/private/root-ca.key \
    -out $CA_DIR/intermediate/intermediate-ca.crt \
    -days 1825 \
    -extensions v3_intermediate_ca \
    -extfile $CA_DIR/intermediate-signing.conf \
    -CAcreateserial

# Create certificate chain
cat $CA_DIR/intermediate/intermediate-ca.crt $CA_DIR/root/root-ca.crt > $CA_DIR/ca-chain.crt

echo "CA created successfully!"
echo "Root CA: $CA_DIR/root/root-ca.crt"
echo "Intermediate CA: $CA_DIR/intermediate/intermediate-ca.crt"
echo "CA Chain: $CA_DIR/ca-chain.crt"