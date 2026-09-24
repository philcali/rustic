# TLS test fixtures

Throwaway self-signed certificates used only by the node's TLS tests
(`src/main.rs` → `mod tests`). They have **no real-world validity** — the CA
key was discarded at generation time, and the private key (`server-key.pem`)
is a random test key with zero value. They exist so the tests can exercise a
real rustls handshake without pulling in a cert-generation dev-dependency.

| file             | role                                             |
| ---------------- | ------------------------------------------------ |
| `ca.pem`         | test root CA — signs `server.pem`; the trusted root the coordinator-side client trusts |
| `server.pem`     | node (server) certificate, SAN `DNS:localhost, IP:127.0.0.1` |
| `server-key.pem` | node (server) private key for `server.pem`       |
| `other-ca.pem`   | an *unrelated* CA — trusting it must fail to validate `server.pem` (refusal test) |

Regenerate (requires `openssl`) if the fixtures are ever lost:

```sh
openssl genpkey -algorithm RSA -out /tmp/ca-key.pem -pkeyopt rsa_keygen_bits:2048
openssl req -x509 -new -key /tmp/ca-key.pem -sha256 -days 3650 -subj "/CN=Pandemic Test CA" -out ca.pem
openssl genpkey -algorithm RSA -out server-key.pem -pkeyopt rsa_keygen_bits:2048
openssl req -new -key server-key.pem -subj "/CN=localhost" -out /tmp/srv.csr
printf 'subjectAltName=DNS:localhost,IP:127.0.0.1\n' > /tmp/ext.cnf
openssl x509 -req -in /tmp/srv.csr -CA ca.pem -CAkey /tmp/ca-key.pem -CAcreateserial \
  -days 3650 -sha256 -extfile /tmp/ext.cnf -out server.pem
```
