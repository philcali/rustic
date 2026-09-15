# Deployments: Spec-Driven Install

Deployments install software the way it should be installed: **declared,
rendered, applied, recorded** — with a precise inverse for removal.

Two spec layers (see the full design in [`ideas/deployments.md`](../ideas/deployments.md)):

- **Infection spec** — one piece of software: packages, rendered files,
  users/groups, a systemd unit (or an attach of an existing unit), and a
  health check. Templated with variables. The atomic, registry-publishable
  unit.
- **Deployment** — a named set of infection specs with shared variables,
  explicit ordering, and cross-infection wiring. The unit of install,
  status, and removal.

Everything here works the same from the CLI, the REST API, and the web
console — all three are thin passthroughs over the same
`Plan` (pure) → `Apply` (privileged, agent) boundary.

## Writing an infection spec

An `infection.toml` plus its templates (inline paths or under `files/`):

```toml
[infection]
name = "rest"
version = "0.4.0"
description = "Pandemic REST API"

[variables]
port = { default = "8080" }
token = {}                      # no default → required at install time

[packages]
apt = ["curl"]
dnf = ["curl"]

[groups]
# rest = {}

[users]
# rest = { group = "rest", system = true }

[files]
# key = template (relative to the spec, or in files/); value = placement
"rest-auth.toml" = { target = "/etc/pandemic/rest-auth.toml", owner = "root", mode = "0600" }

[systemd]
unit_file = "rest.service"      # rendered to /etc/systemd/system/, OR
# attach = "mosquitto.service"  # wrap an existing unit instead
enable = true

[health]
check = ["systemctl", "is-active", "rest"]
interval = 15
```

Rules:

- Exactly one of `unit_file` / `attach`; neither for a config-only infection.
- `{{name}}` substitution; a required variable with no `--set` value is a
  validation error before anything is applied.
- `[packages]` is keyed by manager; the agent picks the one its host
  supports.
- Health checks are **not** run through a shell — use direct-exec commands.

## Writing a deployment

A `deployment.toml` composing infections under one name:

```toml
[deployment]
name = "webapp"
version = "1.0.0"

[variables]
host = "127.0.0.1"
api_port = "8080"

[[infections]]
name = "api"
source = "./api/infection.toml"   # local path or a registry infection name
order = 1
vars = { port = "{{api_port}}" }

[[infections]]
name = "web"
source = "./web/infection.toml"
order = 2
vars = { host = "{{host}}", api_port = "{{api_port}}" }
```

Rules:

- Variable resolution: CLI `--set` → deployment `[variables]` → per-infection
  `vars` bindings → infection defaults.
- `order` is explicit; ties are an error.
- A deployment **owns** the infections it installs and can only remove
  its own. It cannot adopt an infection that is standalone or owned by
  another deployment (v1: refuse, not merge) — and a standalone install
  over a deployment-owned infection is refused too.
- Re-running `deployment install` under an existing name is an idempotent
  re-apply/upgrade: changed files are re-rendered, units are restarted,
  not duplicated.

## Installing from the CLI

```bash
# Preview first: resolves, renders, and prints the full plan — touches nothing
pandemic-cli deployment install ./webapp/deployment.toml --dry-run

# Apply (installs in `order`; records the deployment as owner of each infection)
sudo pandemic-cli deployment install ./webapp/deployment.toml --set api_port=9090

# A single infection, standalone
pandemic-cli infection install ./mosquitto/infection.toml --set port=1883

# By registry name (checksummed bundle; --registry-url / PANDEMIC_REGISTRY_URL
# steer index + bundles)
pandemic-cli deployment install rest-mqtt --registry-url https://example.com/registry
```

## Status, removal, and the audit log

```bash
pandemic-cli deployment list
pandemic-cli deployment status webapp    # unit active? recorded files drift?
pandemic-cli infection status            # every infection, with OWNER=

# Remove: uninstalls the deployment's infections in reverse order;
# standalone and foreign infections are left alone.
sudo pandemic-cli deployment remove webapp

# Same, but also delete the users/groups the state records say this
# deployment's infections created (see --purge policy below):
sudo pandemic-cli deployment remove webapp --purge

sudo pandemic-cli infection uninstall mosquitto --purge

# What the host applied / removed, step by step (0600 root-only log):
sudo pandemic-cli audit                 # last 20 entries
sudo pandemic-cli audit --limit 500 --json
```

### Rollback (failed deployment installs)

If a deployment run fails — an infection's apply fails, or the deployment
record cannot be written — the infections **that run freshly installed are
best-effort uninstalled in reverse order** (files, unit, state record).
Infections that already existed (re-applies) are left in place and reported.
Users/groups are never touched by rollback — they may be shared. The error
and the `apply_deployment` audit entry carry `rolled_back`, `left_applied`,
and any `rollback_errors`; re-run the install after fixing the failure
(completed steps are idempotent).

Standalone `infection install` keeps the simpler model: on failure it reports
the failing step and the steps already applied, and you re-run.

### `--purge` policy

Default removal **leaves users/groups in place** — they may be shared — and
reports them (`left_users` / `left_groups` + a note). `--purge` deletes only
the identity the **state record says this infection created**:

- users first (a group cannot be deleted while it is someone's primary group);
- a group is deleted only when member counting proves **zero** members —
  indeterminate or nonzero is reported, never force-removed;
- the user/group blocklists remain a final guard either way.

## REST API

All admin routes require auth (see `pandemic-rest` README) and the `admin`
scope; they pass through to the agent, which does the privileged work.

| Method & path | Purpose |
|---|---|
| `GET /api/admin/deployments` | list deployments |
| `POST /api/admin/deployments` | install — body: `{ "name" \| "path", "vars", "registry_url", "dry_run" }` |
| `GET /api/admin/deployments/:name` | status |
| `DELETE /api/admin/deployments/:name?purge=true` | remove (optional purge) |
| `GET /api/admin/audit` | last N audit entries (`?limit=`) |
| `GET /api/admin/capabilities` | agent capabilities (gates the console tab) |
| `GET /api/admin/registry/find?query=` | search the registry index |
| `GET /api/admin/registry/infections/:name` | manifest (bundle URL + sha256) |
| `POST /api/admin/registry/infections/:name/install` | install by registry name (`{ "target_path"? }`) |

`dry_run: true` returns the resolved plan **redacted**: names, targets,
owners, modes, per-file sha256, packages, groups, user *names*, health
interval — never variable values, rendered file contents, or the health
command — plus a per-infection `diff` against what is already on the host
(file absent/unchanged/modified, unit exists/active, identity
present/missing). If the agent is unreachable the `diff` is simply absent.

## Web console

The **Deployments** tab (capability-gated on the agent's
`deployment_lifecycle` capability) lists deployments, shows status, and
installs with a **two-step flow**: step 1 previews the redacted plan (and
diff), step 2 applies the identical payload. Removal is a confirm dialog.
The browser never sees variable values or rendered contents.

## Publishing to the registry

Spec atoms live in `registry-src/` in the repo:

```
registry-src/infections/<name>/infection.toml (+ files/)
registry-src/deployments/<name>/deployment.toml (+ nested infection specs)
```

`scripts/generate-registry.sh` (run by the release workflow) tars each atom
into a single `registry/specs/{infections,deployments}/<name>.tar.gz` bundle
with one sha256 in the index entry (`type`: `infection-spec` | `deployment`,
`bundle_url`, `checksum`). Clients verify the checksum before applying —
an unverified bundle is never installed. Signatures are the remaining
production gate (see Security below).

## State & ownership model

| Path | Mode | Contents |
|---|---|---|
| `/etc/pandemic/infections/<name>/` | 0700 dir, 0600 state | resolved spec, variable **values**, recorded files (target + sha256 + owner + mode), unit, health, `owner` |
| `/etc/pandemic/deployments/<name>/` | 0700 dir, 0600 state | deployment spec, resolved variables, ordered infections |
| `/var/log/pandemic/audit.jsonl` | 0600 file | append-only JSONL: every apply/uninstall/remove with steps + outcomes |

- `deployment status` re-hashes recorded files and re-checks unit state to
  report drift.
- `owner` on an infection state record is what makes `deployment remove`
  precise and makes standalone installs over owned names (and adoption of
  foreign names) refuse cleanly.
- The state records are the source of truth for `--purge`: only the
  users/groups *they* list are candidates for deletion.

## Security notes

- **Values are secret-shaped.** Variable values and rendered file contents
  are never sent to the browser and never appear in the REST dry-run
  response, the audit log, or the state-record listings — only names,
  target paths, and modes. State records and the audit log are `0600`
  root-only.
- **`WriteFile` is path-restricted**: allowlisted prefixes, never the agent
  secret, socket dir, or agent/daemon binaries; user/group creation and
  deletion are blocklist-guarded.
- **Checksums are enforced** on every registry bundle fetch (client-side).
- **Registry resolution stays client-side.** The agent's `Plan`/`Apply`
  never does a network fetch; the CLI/REST resolves `source` names to
  concrete specs first. Agent-side resolution (option b) is **deferred**:
  it would be unguarded root network access until signatures and a guarded
  fetch path exist — signatures are the production gate, shared with
  epidemic.
- Secret *management* (vaults, rotation) is out of scope — `pandemic-iam`
  is the planned home.
