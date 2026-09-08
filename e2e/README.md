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

## Example: spec primitives (phase 2)

`pandemic-cli agent request <json>` sends a raw `AgentRequest` over the
authenticated socket — a dev/e2e aid for exercising agent primitives
before the phase 3/4 CLI surface lands. Prereq: the bootstrap section
above (secret minted, `pandemic-agent` started).

```bash
# Capabilities now report which package managers the host supports
docker exec -u root pandemic-e2e pandemic-cli agent request '{"type":"GetCapabilities"}'
#   → "package_managers": ["apt"] on the ubuntu base image

# PackageInstall: root installs a package the way a spec would
docker exec -u root pandemic-e2e pandemic-cli agent request \
  '{"type":"PackageInstall","manager":"apt","packages":["mosquitto"]}'
docker exec pandemic-e2e dpkg -s mosquitto | head -2
docker exec pandemic-e2e command -v mosquitto

# WriteFile: rendered config with owner + mode, as infection specs demand
docker exec -u root pandemic-e2e pandemic-cli agent request \
  '{"type":"WriteFile","path":"/etc/pandemic/rest-auth.toml","content":"token = \"demo\"\n","owner":"root","mode":"0600"}'
docker exec pandemic-e2e stat -c '%U %a %n' /etc/pandemic/rest-auth.toml
#   → root 600 /etc/pandemic/rest-auth.toml

# Rejection paths — each must exit non-zero with a clear message:
#   outside the allowlist, protected pandemic internals, unknown manager
for req in \
  '{"type":"WriteFile","path":"/tmp/evil","content":"x","owner":"root","mode":"0644"}' \
  '{"type":"WriteFile","path":"/etc/pandemic/agent-secret","content":"x","owner":"root","mode":"0600"}' \
  '{"type":"WriteFile","path":"/usr/local/bin/pandemic-agent","content":"x","owner":"root","mode":"0755"}' \
  '{"type":"WriteFile","path":"/etc/pandemic/blocklist.toml","content":"x","owner":"root","mode":"0600"}' \
  '{"type":"PackageInstall","manager":"yum","packages":["x"]}' ; do
  docker exec -u root pandemic-e2e pandemic-cli agent request "$req" || echo "  ^ rejected as expected"
done
```

## Example: infection spec lifecycle (phase 3)

The `infection` subcommand drives a spec end-to-end — resolve → validate →
render **all** templates → apply via agent primitives → health-check →
record state. On failure it reports the failing step and the steps already
applied (re-running is safe: completed steps are idempotent). Prereq: the
bootstrap section above (secret minted, `pandemic-agent` started).

### A. Config-only infection with variables

No `[systemd]` block — files only. `--set` overrides the declared default;
the rendered file and the recorded state are root-only.

```bash
docker exec -u root pandemic-e2e bash -c '
mkdir -p /opt/specs/configonly/files
cat > /opt/specs/configonly/infection.toml <<SPEC
[infection]
name = "configonly"
version = "1.0.0"
description = "Config-only infection (E2E)"

[variables]
token = { default = "changeme" }

[files]
"auth.toml" = { target = "/etc/pandemic/configonly-auth.toml", owner = "root", mode = "0600" }

[health]
check = ["true"]
SPEC
cat > /opt/specs/configonly/files/auth.toml <<TMPL
token = "{{ token }}"
TMPL'

# Install with an override; the CLI renders every file before touching the host
docker exec -u root pandemic-e2e pandemic-cli \
  infection install /opt/specs/configonly/infection.toml --set token=secret123

# Rendered: variable substituted, owner/mode from the spec
docker exec pandemic-e2e cat /etc/pandemic/configonly-auth.toml
docker exec pandemic-e2e stat -c '%U %a %n' /etc/pandemic/configonly-auth.toml
#   → token = "secret123"
#   → root 600 /etc/pandemic/configonly-auth.toml

# State recorded: dir 0700, state.toml 0600, resolved value + sha256
docker exec pandemic-e2e stat -c '%a %n' /etc/pandemic/infections/configonly
docker exec pandemic-e2e stat -c '%a %n' /etc/pandemic/infections/configonly/state.toml
docker exec pandemic-e2e cat /etc/pandemic/infections/configonly/state.toml

# Status: list, then detail (re-hashes each recorded file to report drift)
docker exec pandemic-e2e pandemic-cli infection status
docker exec pandemic-e2e pandemic-cli infection status configonly

# Uninstall: rendered file removed, state dir removed
docker exec -u root pandemic-e2e pandemic-cli infection uninstall configonly
docker exec pandemic-e2e sh -c '
  echo "file:  $(test -f /etc/pandemic/configonly-auth.toml && echo present || echo removed)"
  echo "state: $(test -d /etc/pandemic/infections/configonly && echo present || echo removed)"'
```

Expected: `token = "secret123"`, `root 600`, dir `700` / file `600`, then
`removed` / `removed`.

### B. Attach infection (mosquitto)

Wraps an existing unit — the legacy `service attach` flow, now spec-driven.
Create a stand-in target first, then install the attach spec:

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
systemctl daemon-reload && systemctl enable --now mosquitto
mkdir -p /opt/specs/mosquitto
cat > /opt/specs/mosquitto/infection.toml <<SPEC
[infection]
name = "mosquitto"
version = "2.0"
description = "MQTT broker (attached)"

[systemd]
attach = "mosquitto.service"

[health]
check = ["systemctl", "is-active", "mosquitto"]
SPEC'

docker exec -u root pandemic-e2e pandemic-cli infection install /opt/specs/mosquitto/infection.toml

# Sidecar active, target active, registered in the daemon
docker exec pandemic-e2e systemctl is-active pandemic-mosquitto
docker exec pandemic-e2e systemctl is-active mosquitto
docker exec pandemic-e2e pandemic-cli daemon list | grep mosquitto
docker exec pandemic-e2e pandemic-cli infection status mosquitto

# Uninstall detaches: sidecar stopped + removed, target unit untouched
docker exec -u root pandemic-e2e pandemic-cli infection uninstall mosquitto
docker exec pandemic-e2e sh -c '
  echo "sidecar: $(systemctl is-active pandemic-mosquitto)"
  echo "target:  $(systemctl is-active mosquitto)"
  echo "daemon:  $(pandemic-cli daemon list | grep -c mosquitto) mentions"'
```

Expected: `active` / `active`, and after uninstall `inactive` / `active` / `0`.

### C. Unit-file infection

The spec owns a brand-new unit, rendered to `/etc/systemd/system/` and
enabled. The unit template lives next to the spec (or in `files/`); the
`[files]` template is in `files/`:

```bash
docker exec -u root pandemic-e2e bash -c '
mkdir -p /opt/specs/rest/files
cat > /opt/specs/rest/infection.toml <<SPEC
[infection]
name = "rest"
version = "0.1.0"
description = "Demo service (E2E)"

[variables]
port = { default = "9999" }

[files]
"rest.toml" = { target = "/etc/rest/config.toml", owner = "root", mode = "0644" }

[systemd]
unit_file = "rest.service"
enable = true

[health]
check = ["systemctl", "is-active", "rest"]
SPEC
cat > /opt/specs/rest/rest.service <<UNIT
[Unit]
Description=Demo service (E2E)
After=network.target

[Service]
ExecStart=/bin/sleep infinity

[Install]
WantedBy=multi-user.target
UNIT
cat > /opt/specs/rest/files/rest.toml <<TMPL
port = {{ port }}
TMPL'

docker exec -u root pandemic-e2e pandemic-cli infection install /opt/specs/rest/infection.toml

# Unit rendered + enabled + started; config rendered with the resolved port
docker exec pandemic-e2e systemctl is-active rest
docker exec pandemic-e2e systemctl is-enabled rest
docker exec pandemic-e2e cat /etc/rest/config.toml
docker exec pandemic-e2e cat /etc/systemd/system/rest.service
docker exec pandemic-e2e pandemic-cli infection status rest

# Re-apply: install again — detected as already installed, restarts instead of start
docker exec -u root pandemic-e2e pandemic-cli infection install /opt/specs/rest/infection.toml
docker exec pandemic-e2e systemctl is-active rest

# Uninstall: unit stopped + disabled, unit file and config removed
docker exec -u root pandemic-e2e pandemic-cli infection uninstall rest
docker exec pandemic-e2e sh -c '
  echo "unit file: $(test -f /etc/systemd/system/rest.service && echo present || echo removed)"
  echo "config:    $(test -f /etc/rest/config.toml && echo present || echo removed)"
  echo "state:     $(test -d /etc/pandemic/infections/rest && echo present || echo removed)"'
```

Expected: `active` / `enabled`, `port = 9999`, `active` after re-apply, then
`removed` × 3.

### D. Legacy `service attach` bridge

An infection attached with the old `service attach` CLI is visible to — and
removable by — the `infection` commands (they share the same state dir):

```bash
docker exec -u root pandemic-e2e bash -c '
cat > /etc/systemd/system/mosquitto.service <<UNIT
[Unit]
Description=Legacy bridge (E2E)
[Service]
ExecStart=/bin/sleep infinity
[Install]
WantedBy=multi-user.target
UNIT
systemctl daemon-reload && systemctl enable --now mosquitto'

# Attach the legacy way
docker exec -u root pandemic-e2e pandemic-cli \
  service attach mosquitto --version 2.0 --description "Legacy bridge (E2E)"

# The infection commands see it
docker exec pandemic-e2e pandemic-cli infection status mosquitto
docker exec pandemic-e2e pandemic-cli infection status | grep mosquitto

# ...and uninstall it
docker exec -u root pandemic-e2e pandemic-cli infection uninstall mosquitto
docker exec pandemic-e2e sh -c '
  echo "sidecar: $(systemctl is-active pandemic-mosquitto)"
  echo "target:  $(systemctl is-active mosquitto)"'
```

Expected: after uninstall `inactive` / `active`.

### E. Package install

The spec declares packages per manager; the agent picks the one its host
supports (here `apt`). `sl` is a tiny package; the health check is a real
direct-exec binary (health checks are **not** run through a shell, so avoid
shell builtins like `command -v`):

```bash
docker exec -u root pandemic-e2e bash -c '
mkdir -p /opt/specs/slin/files
cat > /opt/specs/slin/infection.toml <<"SPEC"
[infection]
name = "slin"
version = "0.1.0"
description = "Package install (E2E)"

[packages]
apt = ["sl"]

[files]
"note.conf" = { target = "/etc/pandemic/slin-note.conf", owner = "root", mode = "0644" }

[health]
check = ["true"]
SPEC
cat > /opt/specs/slin/files/note.conf <<"TMPL"
note = "installed via spec"
TMPL'

docker exec -u root pandemic-e2e pandemic-cli infection install /opt/specs/slin/infection.toml

# Package installed, config written, state recorded
docker exec pandemic-e2e dpkg -s sl | grep -E "Package|Status"
docker exec pandemic-e2e cat /etc/pandemic/slin-note.conf
docker exec pandemic-e2e pandemic-cli infection status slin

# Uninstall removes the file and state, but leaves the package in place
# (packages are shared system resources — not owned by the infection)
docker exec -u root pandemic-e2e pandemic-cli infection uninstall slin
docker exec -u root pandemic-e2e apt-get remove -y sl   # manual, if you want it gone
```

Expected: `install ok installed`, then `removed` for the file/state, `sl`
still present until you remove it manually.

### Failure path & required variables

Two behaviors worth seeing live. A failing health check exits non-zero, names
the failing step, lists the steps already applied, and does **not** record
state (re-running is safe):

```bash
docker exec -u root pandemic-e2e bash -c '
mkdir -p /opt/specs/failing/files
cat > /opt/specs/failing/infection.toml <<"SPEC"
[infection]
name = "failing"
version = "0.1.0"
[files]
"app.conf" = { target = "/etc/pandemic/failing-app.conf", owner = "root", mode = "0644" }
[health]
check = ["false"]
SPEC
echo "x" > /opt/specs/failing/files/app.conf'

docker exec -u root pandemic-e2e pandemic-cli infection install /opt/specs/failing/infection.toml \
  || echo "  ^ failed as expected"
#   → failed at step 'health check false'
#   → steps already applied: write /etc/pandemic/failing-app.conf
docker exec -u root pandemic-e2e sh -c '
  echo "state: $(test -d /etc/pandemic/infections/failing && echo present || echo absent)"'
docker exec -u root pandemic-e2e rm -f /etc/pandemic/failing-app.conf
```

Expected: non-zero exit, `state: absent`.

A variable with no `default` is required — install fails without `--set`:

```bash
docker exec -u root pandemic-e2e bash -c '
mkdir -p /opt/specs/req/files
cat > /opt/specs/req/infection.toml <<"SPEC"
[infection]
name = "reqvar"
version = "0.1.0"
[variables]
api_key = {}
[files]
"creds.toml" = { target = "/etc/pandemic/req-creds.toml", owner = "root", mode = "0600" }
[health]
check = ["true"]
SPEC
echo "api_key = \"{{ api_key }}\"" > /opt/specs/req/files/creds.toml'

docker exec -u root pandemic-e2e pandemic-cli infection install /opt/specs/req/infection.toml \
  || echo "  ^ missing required variable, as expected"
docker exec -u root pandemic-e2e pandemic-cli \
  infection install /opt/specs/req/infection.toml --set api_key=sk-live-abc
docker exec -u root pandemic-e2e cat /etc/pandemic/req-creds.toml
docker exec -u root pandemic-e2e pandemic-cli infection uninstall reqvar
```

Expected: error naming `api_key`, then `api_key = "sk-live-abc"`.

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
