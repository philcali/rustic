# E2E Testing with Dockerized Systemd

A systemd-in-docker template for end-to-end tests that need a real init
system — `systemctl`, units, journald, cgroup management, root-only
behavior — **without touching the host**. No host sudo, no KVM; the
container's PID 1 is a genuine systemd.

Use this instead of faking systemd behavior in unit tests when the thing
under test is the integration itself: bootstrap → agent → systemd units →
daemon registration → attach/detach lifecycle.

## Requirements

- Docker (any reasonably recent version)
- A cgroup v2 host (default on modern Linux distros; macOS/Windows via
  Docker Desktop also works)
- Multi-arch: the base image is `ubuntu:24.04`, so it builds natively on
  both `aarch64` and `amd64` hosts

## Launch

```bash
docker build -t pandemic-systemd e2e/
docker run -d --name pandemic-e2e \
  --privileged \
  --tmpfs /run \
  --tmpfs /tmp \
  --memory=2g \
  pandemic-systemd

# Wait until systemd is up (should print "running" or "degraded")
docker exec pandemic-e2e systemctl is-system-running
```

- `--privileged`: systemd needs to manage cgroups and devices; this is a
  throwaway test container, not a security boundary.
- `--tmpfs /run --tmpfs /tmp`: socket dirs (`/var/run/pandemic/…`) and
  scratch space must not live in container layers.
- `--memory=2g`: optional guard against a runaway test.

## Loading the binaries under test

Build on the host, copy in (debug builds are fine for E2E):

```bash
cargo build
for b in pandemic pandemic-cli pandemic-proxy pandemic-agent; do
  docker cp target/debug/$b pandemic-e2e:/usr/local/bin/$b
done
```

## Bootstrapping the stack

`bootstrap install` creates the `pandemic` user, installs and **enables**
the units (but does not start them), and — with `--with-agent` — also
installs `pandemic-agent` and mints `/etc/pandemic/agent-secret`
(0600 root:root):

```bash
docker exec -u root pandemic-e2e pandemic-cli bootstrap install --with-agent
docker exec -u root pandemic-e2e systemctl start pandemic pandemic-agent
docker exec pandemic-e2e systemctl status pandemic pandemic-agent --no-pager
```

## Example: attach/detach E2E

Define a stand-in service (any unit works), then exercise the full
attach → health-mirroring → detach lifecycle:

```bash
docker exec -u root pandemic-e2e bash -c '
cat > /etc/systemd/system/mosquitto.service <<UNIT
[Unit]
Description=Fake MQTT broker (E2E)

[Service]
ExecStart=/bin/sleep infinity

[Install]
WantedBy=multi-user.target
UNIT
systemctl daemon-reload && systemctl enable --now mosquitto'

# Attach as an infection
docker exec -u root pandemic-e2e pandemic-cli \
  service attach mosquitto --version 2.0.18 --description "Fake MQTT broker (E2E)"

# Verify: sidecar active, infection registered
docker exec pandemic-e2e systemctl is-active pandemic-mosquitto
docker exec pandemic-e2e pandemic-cli daemon list | grep mosquitto

# Health mirroring: stop the target, wait one interval, check, restart
docker exec -u root pandemic-e2e systemctl stop mosquitto
docker exec pandemic-e2e sh -c 'sleep 35; journalctl -u pandemic-mosquitto --no-pager | grep -i health | tail -1'
docker exec -u root pandemic-e2e systemctl start mosquitto

# Detach: sidecar stopped, files removed, daemon auto-deregisters,
# target unit untouched
docker exec -u root pandemic-e2e pandemic-cli service detach mosquitto
docker exec pandemic-e2e sh -c '
  echo "sidecar:   $(systemctl is-active pandemic-mosquitto)"
  echo "target:    $(systemctl is-active mosquitto)"
  echo "daemon:    $(pandemic-cli daemon list | grep -c mosquitto) mentions"'
```

Expected: `active` / `active` / `0`, and after detach
`inactive` / `active` / `0`.

## Debugging

```bash
docker exec -it pandemic-e2e bash                 # interactive root shell
docker exec pandemic-e2e journalctl -u pandemic --no-pager -f
docker exec pandemic-e2e journalctl -u pandemic-mosquitto --since "2 min ago"
docker logs pandemic-e2e                           # only pre-init output
```

## Teardown

```bash
docker rm -f pandemic-e2e
docker rmi pandemic-systemd   # optional
```

## Limitations

- Network is the container's; no real NICs, no host network namespaces.
- No kernel modules, no KVM — fine for unit-level E2E, not for testing
  kernel features (e.g. BPF, cgroup limits that need real cpusets).
- Time and health intervals behave normally (real kernel clocks).
- Everything is root inside; there is no host privilege separation.
