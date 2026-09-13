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
pandemic-cli deployment install ./pandemic-full.toml --set host=10.0.1.5

# Inspect / tear down
pandemic-cli deployment list
pandemic-cli deployment status pandemic-full
pandemic-cli deployment remove pandemic-full

# A single infection, standalone
pandemic-cli infection install ./mosquitto.toml --set port=1883
```

## Principles

1. **Infections don't know about each other.** An infection declares the variables it needs; it never references another service's port, URL, or name.
2. **Wiring lives in the deployment.** Cross-infection references (`api_url = "http://{{host}}:{{rest_port}}"`) exist only at the deployment level. This is what keeps infections reusable across deployments.
3. **The agent owns apply; the boundary is a verb, not a process.** Each install splits into `Plan*` (pure — resolve, render, validate, return a concrete plan; no privileged ops) and `Apply*` (privileged — execute the concrete plan and record state). Removal, state, and status already live in the agent; install is consolidated there too. CLI, REST, and the console are thin passthroughs — they express intent, they don't re-implement the loop. Nothing privileged happens until `Apply*`.
4. **A deployment owns what it installs.** Ownership is recorded so `deployment remove` is precise and standalone infections are left alone.
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

The boundary is a **protocol split**, not "the client does the work":

- **The client** (CLI / REST / console) resolves each `source` (local path or registry fetch) and binds CLI `--set` / deployment `[variables]`. Registry resolution stays client-side for now (see Security), so the agent never does a network fetch.
- **`Plan*`** — pure, non-privileged, no network. Renders every template with resolved variables, validates (names, variable coverage, unit/attach exclusivity, target-path collisions across infections), and returns a **concrete plan**: packages, files with rendered content + placement, user/group/unit actions, in `order`. Nothing is written. This is the `--dry-run` artifact.
- **`Apply*`** — privileged (agent, root). For each infection, in `order`, via the existing primitives: `PackageInstall { manager, packages }`, `GroupCreate` / `UserCreate`, `WriteFile { path, content, owner, mode }`, unit install / `AttachInfection`, `SystemdControl { enable, start }`. Health-checks per infection; on failure, reports the failing infection and the steps already taken. Idempotent — safe to re-run (re-apply/upgrade).
- **Removal / state / status** — already agent-owned: `RemoveDeployment` (reverse-order uninstall of owned infections), `RecordInfection` / `RecordDeployment`, and the `*Status` handlers.

CLI, REST, and the console are **thin passthroughs**: build the resolved spec, call `Plan*` (optionally show it), call `Apply*`. No client re-implements the apply loop, so there is nothing to drift. Epidemic is the same shape on N nodes — `Plan*`+`Apply*` (or just `Apply*`) per node.

New protocol surface: `InfectionSpec` / `DeploymentSpec` / plan types in `pandemic-protocol` (shared by CLI, agent, REST, console, and later epidemic), plus `Plan*` / `Apply*` `AgentRequest` variants. `Apply*` receives a **resolved, concrete** plan — never a raw templated spec.

## State & Ownership

- Infection: `/etc/pandemic/infections/<name>/` — resolved spec, variable values, file list with sha256, unit name, install timestamp. (Generalizes the current `/etc/pandemic/infections/<name>.toml` written by attach.)
- Deployment: `/etc/pandemic/deployments/<name>.toml` — spec, resolved variables, ordered owned infections.
- `deployment status` compares recorded state to host reality: unit active/inactive, rendered file hashes.
- `deployment remove` applies infections in reverse order; infections not owned by the deployment are untouched.
- Re-running `deployment install` under an existing name = idempotent re-apply/upgrade: diff rendered files, replace changed ones, restart affected units.

## CLI Surface

```
pandemic-cli deployment install <path|name> [--set k=v ...] [--registry-url URL] [--dry-run]
pandemic-cli deployment list
pandemic-cli deployment status [name]
pandemic-cli deployment remove <name>

pandemic-cli infection install <path|name> [--set k=v ...] [--registry-url URL]
pandemic-cli infection status [name]
pandemic-cli infection uninstall <name>

pandemic-cli registry find <query> [--registry-url URL]
pandemic-cli registry get <name> [--registry-url URL]
pandemic-cli registry install <name> [--registry-url URL]
```

`--dry-run` resolves, renders, and prints the full plan — packages, files with diffs against what's on disk, user and unit actions — without applying. This is the "plan" view: the review moment before root does anything.

## Console & API Surface

The deployment lifecycle is a first-class surface in the REST API and the web console, not just the CLI. Both are thin passthroughs over `Plan*` / `Apply*` (see Execution Model).

- **REST** (`pandemic-rest`): `GET /api/admin/deployments` (list), `GET /api/admin/deployments/:name` (status), `POST /api/admin/deployments` (install — body: spec path/name + `vars`), `DELETE /api/admin/deployments/:name` (remove). Registry `source` resolution happens here (REST is already the registry proxy), so the root agent never fetches.
- **Console** (`pandemic-console`): a **Deployments** tab (list, status, install, remove) alongside Services / Users / Groups / Registry.
- **Capability gate**: the agent advertises a `deployment` capability in `GetCapabilities`; REST surfaces it at `/api/admin/capabilities`; the console shows the Deployments tab only when present. This degrades gracefully against older agents and reuses the mechanism the existing tabs already rely on.

## Registry Distribution

Both layers are distributable. Infection specs are the publishable atom; a deployment is a thin manifest referencing infection names. `source = "mosquitto"` + `--registry-url` resolves exactly like `registry install` today. Publishing extends the existing release workflow: a spec index alongside the binary index.

## Relationship to Existing & Future Features

- **Attach**: the mosquitto half of the motivating example is `[files]` + `attach` with no packages or own unit — the schema covers mixed shapes without special cases.
- **Bootstrap**: conceptually the first deployment (daemon + agent). Stays a special case until the schema proves itself, then can be expressed as one.
- **Epidemic**: the payload becomes `{ deployment, vars }`; each node runs the same `deployment install`. Transport and levels in `epidemic_infections.md` are unchanged — this doc is what they spread.

## Security Considerations

- A registry-fetched spec applied as root is a privilege-escalation channel. Checksums (then signatures, shared with epidemic) are required before `source = "<registry-name>"` is considered production-ready.
- **Registry resolution stays client-side for now** (option a): the agent's `Plan*` / `Apply*` never do a network fetch — the client (CLI/REST) resolves `source = "<name>"` and sends a concrete spec. Agent-side resolution (option b) is deferred to **Hardening**, gated on the checksums/signatures above.
- `WriteFile` is path-restricted in v1: allowlisted prefixes (`/etc`, `/opt`, `/usr/local`, `/var`), and never the agent secret, socket dir, or agent/daemon binaries.
- Variables may carry secret values; rendered state files are `0600` root-only. Secret *management* itself is out of scope — `pandemic-iam` is the planned home.

## Implementation Plan

Phases 1–7 are done: the pure `Plan` step (resolve + render + validate) is consolidated in `pandemic_common`, shared by the CLI and REST, while the privileged `Apply` step runs in the agent — one code path for every surface. The console Deployments tab (6) is in, with the two-step preview → apply install flow. Only Hardening (8) remains.

1. **Schema & render** — (done) `InfectionSpec` / `DeploymentSpec` in `pandemic-protocol`; TOML parsing, variable resolution, validation (shared `pandemic-protocol::spec`).
2. **Agent primitives** — (done) `PackageInstall`, `WriteFile` handlers; package-manager detection via `GetCapabilities`.
3. **Infection apply** — (done) `infection install/status/uninstall` + state dir. Standalone path works end-to-end (agent-owned state; install apply still CLI-driven).
4. **Deployment apply** — (done) `deployment install/list/status/remove`, ownership, reverse-order removal, re-apply/upgrade. Removal + state agent-owned; install apply still CLI-driven.
5. **Plan/Apply consolidation** — (done) the pure `Plan` step (resolve + render + validate) lives in `pandemic_common`, shared by CLI + REST; the privileged `Apply` step runs in the agent; one code path for every surface.
6. **Deployment UX** — (done) `/api/admin/deployments*` + `POST` install (name/path + `vars` + `dry_run`); console Deployments tab: two-step install (preview the rendered plan via `dry_run: true`, then apply the identical payload with `dry_run: false`), list, status, remove; capability-gated on the agent's `deployment_lifecycle` capability. The browser never shows variable *values* or rendered file contents — only names, target paths, and modes.
7. **Registry** — (done) spec + deployment atoms in the registry index (checksummed bundles, relative `bundle_url`); client-side `source` name resolution (`pandemic_common::resolve`); `registry find` (client-side, mirrored at `GET /api/admin/registry/find`); `--registry-url` / `PANDEMIC_REGISTRY_URL` steer both index and bundles (see Security).
8. **Hardening** — dry-run diffs, checksums, audit log of applied steps, best-effort rollback; **revisit agent-side registry resolution (option b) now that checksums exist.** Also: strip variable *values* and rendered file contents from the dry-run wire response (the console masks them today; the API still returns them), and decide removal policy for users/groups an infection created (currently left in place — possibly shared — but only tracked in state when created by the *final* successful apply).

## Open Questions

- Template engine: `{{var}}` substitution only in v1? (leaning yes; minijinja when conditionals are actually needed)
- Package installs: `install` with version-pin strings in v1, or plain names? (leaning pins — "full blown infection" should be reproducible)
- Naming: CLI resource noun is `deployment` (renamed from `deploy` to match `infection`); "plan" is the `--dry-run` view.
