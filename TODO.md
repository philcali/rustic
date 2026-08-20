# TODO

Planned improvements for the Pandemic codebase.

## Security

- [ ] **Default auth keys** — REST API generates known default keys (`pandemic-admin-key-change-me`). Should either refuse to start until keys are configured, or generate random keys on first run and log them.
- [ ] **Agent authorization** — `pandemic-agent` has broad root privileges (user/group management, systemd control) with no per-operation authorization. Consider scope-based permissions on the admin socket.
- [ ] **IAM example config** — `pandemic-iam/example-config.toml` has hardcoded placeholder ARNs (`123456789012`) that could be copy-pasted into production. Make it a template requiring explicit input.

## Correctness

- [x] **Timestamp deserialization is a no-op** — Fixed: deserializer now properly parses `%Y-%m-%d %H:%M:%S UTC` strings back into `SystemTime`. Added round-trip test.
- [x] **`PersistentClient::register_and_keep_alive` makes the connection unusable** — Fixed: removed the method entirely. Updated `hello-infection` example to show the correct pattern (connect → register → subscribe → event loop via `read_event()`).

## Architecture / Performance

- [ ] **Daemon serializes all operations** on a single `Arc<Mutex<Daemon>>`. For many concurrent plugins this is a bottleneck. Consider per-plugin state or fine-grained locking.
- [ ] **EventBus publish is O(n)** — iterates all connections to find matching ones. Maintain a reverse index from plugin_name to connection_ids for O(1) lookup.
- [ ] **No config file for the daemon** — only a CLI flag for socket path. No way to configure health check intervals, max plugins, or logging.
- [ ] **`pandemic-protocol` depends on `chrono`** for timestamp formatting. Consider serializing as unix timestamp (integer) to drop the dependency.
- [ ] **`pandemic-proxy` lists `chrono` in Cargo.toml** but doesn't appear to use it — dead dependency.
- [ ] **No graceful shutdown** in the daemon — `while let Ok((stream, _)) = listener.accept().await` with no signal handling. No plugin deregistration events or socket cleanup on exit.

## Testing

- [ ] **Integration tests missing** — no tests for daemon plugin lifecycle, event bus routing, connection handling, or client behavior.
- [ ] **`pandemic-common/src/tests.rs`** — declared as `mod tests` but unclear if it's compiled/run. Verify it exists and is exercised.

## Code Organization

- [ ] **Web console is ~600 lines of vanilla JS** in a single file. Consider HTMX, Alpine.js, or splitting into modules for maintainability.
