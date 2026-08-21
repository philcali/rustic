# TODO

Planned improvements for the Pandemic codebase.

## Security

- [x] **Default auth keys** — Fixed: generates random 32-char keys on first run and logs them with `error!` (red) so they're hard to miss. Keys are written to the config file and won't be displayed again.
- [x] **Agent authorization** — `pandemic-agent` now requires a challenge-response handshake: agent sends a random nonce on connect, client must respond with HMAC-SHA256(nonce, shared_secret). Agent requires `--secret`/`--secret-path`; REST API requires `--agent-secret`/`--agent-secret-path`. Socket access alone no longer grants root. Scope-based logical authorization is a future phase.
- [x] **IAM example config** — `pandemic-iam/example-config.toml` has hardcoded placeholder ARNs (`123456789012`) that could be copy-pasted into production. Make it a template requiring explicit input.

## Correctness

- [x] **Timestamp deserialization is a no-op** — Fixed: deserializer now properly parses `%Y-%m-%d %H:%M:%S UTC` strings back into `SystemTime`. Added round-trip test.
- [x] **`PersistentClient::register_and_keep_alive` makes the connection unusable** — Fixed: removed the method entirely. Updated `hello-infection` example to show the correct pattern (connect → register → subscribe → event loop via `read_event()`).

## Architecture / Performance

- [ ] **Daemon serializes all operations** on a single `Arc<Mutex<Daemon>>`. For many concurrent plugins this is a bottleneck. Consider per-plugin state or fine-grained locking.
- [x] **EventBus publish is O(n)** — iterates all connections to find matching ones. Maintain a reverse index from plugin_name to connection_ids for O(1) lookup.
- [ ] **No config file for the daemon** — only a CLI flag for socket path. No way to configure health check intervals, max plugins, or logging.
- [x] **`pandemic-protocol` depends on `chrono`** for timestamp formatting. Switched all crates to `chrono::DateTime<Utc>` with RFC3339 serialization for consistency across the codebase.
- [x] **`pandemic-proxy` lists `chrono` in Cargo.toml** but doesn't appear to use it — dead dependency. Fixed: switched the entire codebase to use `chrono::DateTime<Utc>` with RFC3339 serialization for timestamp consistency.
- [ ] **No graceful shutdown** in the daemon — `while let Ok((stream, _)) = listener.accept().await` with no signal handling. No plugin deregistration events or socket cleanup on exit.

## Testing

- [ ] **Integration tests missing** — no tests for daemon plugin lifecycle, event bus routing, connection handling, or client behavior.
- [x] **`pandemic-common/src/tests.rs`** — declared as `mod tests` but unclear if it's compiled/run. Verify it exists and is exercised.

## Testing Procedures

### Local LAN testing (daemon + rest + console)

All three services must be running with the same socket path. The console's frontend defaults its API URL to `http://<current-host>:8080`, so the REST server must be on port 8080.

```bash
# 1. Start daemon on a local socket
mkdir -p /tmp/pandemic
cargo run --release -p pandemic-daemon -- --socket-path /tmp/pandemic/pandemic.sock

# 2. Start REST API (LAN-accessible)
cargo run --release -p pandemic-rest -- \
  --socket-path /tmp/pandemic/pandemic.sock \
  --port 8080 \
  --bind-address 0.0.0.0 \
  --auth-config /tmp/pandemic/rest-auth.toml

# 3. Start console (LAN-accessible)
cargo run --release -p pandemic-console -- \
  --socket-path /tmp/pandemic/pandemic.sock \
  --port 3000 \
  --bind-address 0.0.0.0

# 4. Access from LAN: http://<server-ip>:3000
#    Use API key: pandemic-admin-key-change-me
```

**Note:** If you change the console's frontend (web/src/), rebuild first:
```bash
cd pandemic-console/web && npm run build
```

**Common issues:**
- **WebSocket errors** — ensure the `pandemic-api-url` isn't stale in browser localStorage. Clear it or re-enter the API key via the "Save" button.
- **`window.location.host` vs `hostname`** — the API URL must use `hostname` (no port), not `host` (includes port), otherwise you get double-port URLs like `10.0.0.16:3000:8080`.

## Code Organization

- [x] **Web console is ~600 lines of vanilla JS** in a single file. Fixed: split into 9 focused ES modules (api, websocket, health, plugins, services, users, groups, registry, tabs) with main.js as a thin orchestrator (~300 lines). No new dependencies — Vite handles module resolution natively.
