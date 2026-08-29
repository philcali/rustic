# Deployments: Templated, Spec-Driven Install

## Overview

Pandemic today can install software two limited ways: `registry install` fetches a prebuilt binary, and `service install/attach` registers a single unit. Neither can express a *full infection* — packages, rendered configuration, users/groups, a unit, and a health check — and nothing expresses a *set* of infections deployed together with shared configuration (e.g. rest + console + mqtt + a proxy-attached mosquitto).

This idea introduces two spec layers:

- **Infection spec** — a declarative description of one piece of software installed and configured on a host. Templated with variables. The atomic, registry-publishable unit.
- **Deployment** — a composition of infection specs with shared variables, explicit ordering, and cross-infection wiring. The unit of install, status, and removal.

This is the prerequisite for epidemic: epidemic spreads a *deployment* (spec + variable values) to N nodes and runs install on each, instead of inventing a separate payload format.

## Core Concept

```bash
# One command, four infections, wired together
pandemic-cli deploy install ./pandemic-full.toml --set host=10.0.1.5

# Inspect / tear down
pandemic-cli deploy list
pandemic-cli deploy status pandemic-full
pandemic-cli deploy remove pandemic-full

# A single infection, standalone
pandemic-cli infection install ./mosquitto.toml --set port=1883
```

## Principles

1. **Infections don't know about each other.** An infection declares the variables it needs; it never references another service's port, URL, or name.
2. **Wiring lives in the deployment.** Cross-infection references (`api_url = "http://{{host}}:{{rest_port}}"`) exist only at the deployment level. This is what keeps infections reusable across deployments.
3. **The agent is a dumb executor.** The CLI resolves, renders, and validates; the agent executes individual privileged steps via `AgentRequest`s. A deployment never crosses the wire to the agent.
4. **A deployment owns what it installs.** Ownership is recorded so `deploy remove` is precise and standalone infections are left alone.
5. **Not Ansible.** A small fixed schema: packages, files, users/groups, unit-or-attach, health. No arbitrary command execution in v1.

## Infection Schema

A strict superset of today's attach/service fields (name, version, description, unit, health_check, health_interval):

```toml
[infection]
name = "rest"
version = "0.4.0"
description = "Pandemic REST API"

[variables]
socket_path  = { default = "/var/run/pandemic/pandemic.sock" }
port         = { default = "8080" }
bind_address = { default = "127.0.0.1" }

[packages]
apt = []
dnf = []
pacman = []

[groups]
# rest = {}

[users]
# rest = { group = "rest", system = true }

[files]
# key = template path (relative to the spec or in files/); value = host placement
"rest-auth.toml" = { target = "/etc/pandemic/rest-auth.toml", owner = "root", mode = "0600" }

[systemd]
unit_file = "rest.service"      # rendered to /etc/systemd/system/, OR
# attach = "mosquitto.service"  # wrap an existing unit (existing attach flow)
enable = true

[health]
check = ["curl", "-sf", "http://127.0.0.1:{{port}}/health"]
interval = 15
```

Config-only infections are first-class — the mosquitto half of the motivating example has `[files]` + `attach` and no packages or unit of its own:

```toml
[infection]
name = "mosquitto"
version = "2.0"
description = "MQTT broker (attached)"

[variables]
port = { default = "1883" }

[files]
"mosquitto.conf" = { target = "/etc/mosquitto/mosquitto.conf", owner = "mosquitto", mode = "0640" }

[systemd]
attach = "mosquitto.service"

[health]
check = ["systemctl", "is-active", "mosquitto"]
interval = 15
```

Rules:

- Exactly one of `systemd.unit_file` / `systemd.attach`; neither is allowed for a config-only infection.
- `{{name}}` substitution only in v1 (minijinja later, if conditionals are needed). A missing required variable is a validation error before anything is applied.
- `[packages]` is keyed by manager; the agent picks the key its host supports and errors if none match.
- Names reuse the existing `validate_infection_name` charset rules.

## Deployment Schema

```toml
[deployment]
name = "pandemic-full"
version = "0.4.0"

[variables]
host = "127.0.0.1"
socket_path = "/var/run/pandemic/pandemic.sock"
rest_port = "8080"
console_port = "3000"
mqtt_port = "1883"
topic_prefix = "pandemic"

[[infections]]
name = "mosquitto"
source = "./infections/mosquitto.toml"
order = 1
vars = { port = "{{mqtt_port}}" }

[[infections]]
name = "rest"
source = "./infections/rest.toml"
order = 2
vars = { socket_path = "{{socket_path}}", port = "{{rest_port}}" }

[[infections]]
name = "console"
source = "./infections/console.toml"
order = 3
vars = { socket_path = "{{socket_path}}", port = "{{console_port}}", api_url = "http://{{host}}:{{rest_port}}" }

[[infections]]
name = "mqtt"
source = "./infections/mqtt.toml"
order = 4
vars = { socket_path = "{{socket_path}}", broker_url = "mqtt://{{host}}:{{mqtt_port}}", topic_prefix = "{{topic_prefix}}" }
```

Rules:

- `source` is a local path or a registry infection name (resolved like `registry install` today).
- `order` is explicit; ties are an error. No dependency graph — a handful of infections don't need topological sort.
- Variable resolution: CLI `--set` → deployment `[variables]` → per-infection `vars` bindings → infection defaults. Bindings may reference deployment variables.
- Duplicate infection names within a deployment are an error. A deployment cannot adopt an infection name already owned by another deployment (v1: refuse, not merge).

## Execution Model

`deploy install` = resolve → validate → render → N × apply:

1. Resolve each `source` (local path or registry).
2. Validate: names, variable coverage, unit/attach exclusivity, target-path collisions across infections.
3. Render every template with resolved variables. Nothing is written yet.
4. For each infection, in `order`, via the agent (root):
   - `PackageInstall { manager, packages }` — **new** variant
   - `GroupCreate` / `UserCreate` — existing
   - `WriteFile { path, content, owner, mode }` — **new** variant (generalizes what the agent already does when writing unit files)
   - unit install / `AttachInfection` — existing flows
   - `SystemdControl { enable, start }` — existing
5. Health-check per infection. On failure, report the failing infection and the steps already taken.

New protocol surface: `InfectionSpec` / `DeploymentSpec` types in `pandemic-protocol` (shared by CLI, agent, and later epidemic), plus the two new `AgentRequest` variants. The agent never sees a deployment.

## State & Ownership

- Infection: `/etc/pandemic/infections/<name>/` — resolved spec, variable values, file list with sha256, unit name, install timestamp. (Generalizes the current `/etc/pandemic/infections/<name>.toml` written by attach.)
- Deployment: `/etc/pandemic/deployments/<name>.toml` — spec, resolved variables, ordered owned infections.
- `deploy status` compares recorded state to host reality: unit active/inactive, rendered file hashes.
- `deploy remove` applies infections in reverse order; infections not owned by the deployment are untouched.
- Re-running `deploy install` under an existing name = idempotent re-apply/upgrade: diff rendered files, replace changed ones, restart affected units.

## CLI Surface

```
pandemic-cli deploy install <path|name> [--set k=v ...] [--registry-url URL] [--dry-run]
pandemic-cli deploy list
pandemic-cli deploy status [name]
pandemic-cli deploy remove <name>

pandemic-cli infection install <path> [--set k=v ...]
pandemic-cli infection status [name]
pandemic-cli infection uninstall <name>
```

`--dry-run` resolves, renders, and prints the full plan — packages, files with diffs against what's on disk, user and unit actions — without applying. This is the "plan" view: the review moment before root does anything.

## Registry Distribution

Both layers are distributable. Infection specs are the publishable atom; a deployment is a thin manifest referencing infection names. `source = "mosquitto"` + `--registry-url` resolves exactly like `registry install` today. Publishing extends the existing release workflow: a spec index alongside the binary index.

## Relationship to Existing & Future Features

- **Attach**: the mosquitto half of the motivating example is `[files]` + `attach` with no packages or own unit — the schema covers mixed shapes without special cases.
- **Bootstrap**: conceptually the first deployment (daemon + agent). Stays a special case until the schema proves itself, then can be expressed as one.
- **Epidemic**: the payload becomes `{ deployment, vars }`; each node runs the same `deploy install`. Transport and levels in `epidemic_infections.md` are unchanged — this doc is what they spread.

## Security Considerations

- A registry-fetched spec applied as root is a privilege-escalation channel. Checksums (then signatures, shared with epidemic) are required before `source = "<registry-name>"` is considered production-ready.
- `WriteFile` is path-restricted in v1: allowlisted prefixes (`/etc`, `/opt`, `/usr/local`, `/var`), and never the agent secret, socket dir, or agent/daemon binaries.
- Variables may carry secret values; rendered state files are `0600` root-only. Secret *management* itself is out of scope — `pandemic-iam` is the planned home.

## Implementation Plan

1. **Schema & render** — `InfectionSpec` / `DeploymentSpec` in `pandemic-protocol`; TOML parsing, variable resolution, validation. Pure logic, unit-tested.
2. **Agent primitives** — `PackageInstall`, `WriteFile` handlers; package-manager detection (extend `GetCapabilities`). E2E via the dockerized-systemd template in `e2e/`.
3. **Infection apply** — `infection install/status/uninstall` + state dir. The standalone path works end-to-end here.
4. **Deployment apply** — `deploy install/list/status/remove`, ownership, reverse-order removal, re-apply/upgrade.
5. **Registry** — spec and deployment manifests in the registry index; `source` name resolution.
6. **Hardening** — dry-run diffs, checksums, audit log of applied steps, best-effort rollback.

## Open Questions

- Template engine: `{{var}}` substitution only in v1? (leaning yes; minijinja when conditionals are actually needed)
- Package installs: `install` with version-pin strings in v1, or plain names? (leaning pins — "full blown infection" should be reproducible)
- Naming: CLI verb blessed as `deploy`; "plan" is the `--dry-run` view. Confirm before phase 3.
