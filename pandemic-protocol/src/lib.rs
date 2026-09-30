use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

mod time_format {
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(time: &Option<DateTime<Utc>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match time {
            Some(t) => serializer.serialize_str(&t.to_rfc3339()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<DateTime<Utc>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt: Option<String> = Option::deserialize(deserializer)?;
        match opt {
            Some(s) => {
                let dt = DateTime::parse_from_rfc3339(&s).map_err(serde::de::Error::custom)?;
                Ok(Some(dt.with_timezone(&Utc)))
            }
            None => Ok(None),
        }
    }
}

/// Spec-driven install: infection and deployment specs (see
/// `ideas/deployments.md`). Pure logic — parsing, validation, variable
/// resolution, and `{{name}}` rendering.
pub mod spec;

use spec::{DeploymentState, InfectionState};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthMetrics {
    // Daemon metrics
    pub active_plugins: usize,
    pub total_connections: usize,
    pub event_bus_subscribers: usize,
    pub uptime_seconds: u64,

    // System metrics
    pub memory_used_mb: u64,
    pub memory_total_mb: u64,
    pub cpu_usage_percent: f32,
    pub load_average: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub config: Option<HashMap<String, String>>,
    #[serde(with = "time_format")]
    pub registered_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Request {
    Register {
        plugin: PluginInfo,
    },
    Deregister {
        name: String,
    },
    ListPlugins,
    GetPlugin {
        name: String,
    },
    Subscribe {
        topics: Vec<String>,
    },
    Unsubscribe {
        topics: Vec<String>,
    },
    Publish {
        topic: String,
        data: serde_json::Value,
    },
    GetHealth,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AgentRequest {
    GetHealth,
    GetCapabilities,
    ListServices,
    SystemdControl {
        action: String,
        service: String,
    },

    // User management
    UserCreate {
        username: String,
        config: UserConfig,
    },
    UserDelete {
        username: String,
    },
    UserModify {
        username: String,
        config: UserConfig,
    },
    ListUsers,

    // Group management
    GroupCreate {
        groupname: String,
    },
    GroupDelete {
        groupname: String,
    },
    GroupAddUser {
        groupname: String,
        username: String,
    },
    GroupRemoveUser {
        groupname: String,
        username: String,
    },
    ListGroups,

    // Service configuration
    ServiceConfigOverride {
        service: String,
        overrides: ServiceOverrides,
    },
    ServiceConfigReset {
        service: String,
    },
    GetServiceConfig {
        service: String,
    },

    // Registry operations
    GetInfectionManifest {
        name: String,
    },
    InstallInfection {
        name: String,
        target_path: Option<String>,
    },

    // Infection attach (register an existing systemd unit via pandemic-proxy)
    AttachInfection {
        /// systemd unit to attach, e.g. "mosquitto" or "mosquitto.service"
        unit: String,
        /// infection name; defaults to the unit's base name
        name: Option<String>,
        /// version recorded in the daemon; defaults to "0.0.0"
        version: Option<String>,
        description: Option<String>,
        /// health check command; defaults to `systemctl is-active <unit>`
        health_check: Option<Vec<String>>,
        /// health check interval in seconds; defaults to 30
        health_interval: Option<u64>,
        /// path to the pandemic-proxy binary; defaults to /usr/local/bin/pandemic-proxy
        proxy_path: Option<String>,
    },
    DetachInfection {
        /// infection name as created by AttachInfection
        name: String,
    },

    // Spec-driven install primitives (ideas/deployments.md, phase 2)
    PackageInstall {
        /// package manager: one of `apt`, `dnf`, `pacman`, `apk`, `zypper`
        manager: String,
        /// package names to install
        packages: Vec<String>,
    },
    WriteFile {
        /// absolute host path (allowlisted; pandemic internals protected)
        path: String,
        /// file content
        content: String,
        /// owning user
        owner: String,
        /// file mode, e.g. "0600"
        mode: String,
    },

    // Spec-driven infection lifecycle (ideas/deployments.md, phase 3)
    /// Record the state of an installed infection (0600 root-only)
    RecordInfection {
        /// infection name
        name: String,
        /// the recorded state (variables, files + hashes, unit/attach, health)
        state: InfectionState,
    },
    /// List installed infections (spec-driven state + legacy attach records)
    ListInfections,
    /// Status of one infection: recorded state + live unit/file checks
    GetInfectionStatus {
        /// infection name
        name: String,
    },
    /// Uninstall an infection (reverse of install, driven by recorded state)
    UninstallInfection {
        /// infection name
        name: String,
        /// also delete the users/groups the state record says this
        /// infection created (phase 8 `--purge`; default leaves them in
        /// place — they may be shared)
        #[serde(default)]
        purge: bool,
    },

    // Spec-driven deployment lifecycle (ideas/deployments.md, phase 4)
    /// Record the state of an installed deployment (0600 root-only)
    RecordDeployment {
        /// deployment name
        name: String,
        /// the recorded state (resolved shared variables, owned infections
        /// in install order)
        state: DeploymentState,
    },
    /// List installed deployments
    ListDeployments,
    /// Status of one deployment: recorded state + each owned infection's
    /// live state (missing infections are reported, not an error)
    GetDeploymentStatus {
        /// deployment name
        name: String,
    },
    /// Remove a deployment: uninstall its owned infections in reverse
    /// order, then drop the record. Infections not owned by it are
    /// left untouched.
    RemoveDeployment {
        /// deployment name
        name: String,
        /// also delete the users/groups the infection state records say
        /// the deployment's infections created (phase 8 `--purge`; default
        /// leaves them in place — they may be shared)
        #[serde(default)]
        purge: bool,
    },

    // Plan/Apply boundary (ideas/deployments.md, phase 5). The client does
    // the pure `Plan` step (resolve, render, validate) and sends the
    // concrete plan(s); the agent owns the privileged `Apply` step.
    /// Apply a single concrete infection plan and record it. `owner` is
    /// `None` for a standalone install, the deployment name when applied as
    /// part of one (what makes `RemoveDeployment` precise).
    ApplyInfection {
        /// fully rendered infection plan
        plan: Plan,
        /// owning deployment name, or `None` for standalone
        owner: Option<String>,
    },
    /// Apply a whole deployment: ownership pre-flight, then each infection
    /// (in `infections` order) applied with this deployment as owner, then
    /// the deployment record written. Mirrors [`AgentRequest::RemoveDeployment`].
    ApplyDeployment {
        /// deployment name
        name: String,
        /// deployment version (recorded)
        version: String,
        /// resolved shared variables (recorded)
        variables: BTreeMap<String, String>,
        /// per-infection record metadata + concrete plans, in install order
        infections: Vec<ApplyDeploymentInfection>,
    },

    // Host preview / diff (ideas/deployments.md, phase 8). Read-only:
    // reports what applying the plan(s) would change, without writing
    // anything. The client merges the result into the redacted dry-run.
    /// Preview one concrete plan against host state (zero writes): per-file
    /// absent/unchanged/modified (by sha256), unit file + active state,
    /// attach target active, groups/users present, the package manager this
    /// host would use, and whether a state record already exists.
    PreviewInfection {
        /// the concrete plan to preview
        plan: Plan,
    },
    /// Preview every infection of a deployment (same per-infection shape,
    /// in the given order).
    PreviewDeployment {
        /// per-infection metadata + concrete plans, in install order
        infections: Vec<ApplyDeploymentInfection>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserConfig {
    pub shell: Option<String>,
    pub home_dir: Option<String>,
    pub groups: Option<Vec<String>>,
    pub system_user: Option<bool>,
}

/// A rendered file ready to write (rendered content + host placement).
///
/// Carried inside [`Plan`] so an `Apply*` request is fully concrete — the
/// agent writes it as-is and never sees a template.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderedFile {
    pub target: String,
    pub content: String,
    pub owner: String,
    pub mode: String,
}

/// The unit an install owns, rendered to /etc/systemd/system/.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanUnit {
    pub name: String,
    pub target: String,
    pub content: String,
    pub enable: bool,
}

/// The fully rendered, pre-flight plan for one infection install
/// (ideas/deployments.md, phase 5 — the Plan/Apply boundary).
///
/// This is the *concrete* artifact of the pure `Plan` step: variables
/// resolved, every template rendered, the unit/attach chosen. It is sent
/// to the agent as an `ApplyInfection`/`ApplyDeployment` request, which
/// executes it with its privileged primitives. `declared_packages` is the
/// raw per-manager list — the agent picks the manager its own host supports.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub name: String,
    pub version: String,
    pub description: String,
    pub variables: BTreeMap<String, String>,
    pub files: Vec<RenderedFile>,
    pub unit: Option<PlanUnit>,
    pub attach: Option<String>,
    pub health_check: Vec<String>,
    pub health_interval: u64,
    /// The spec's non-empty `[packages]` entries. The agent selects the
    /// manager this host supports (see `spec::select_packages`).
    pub declared_packages: BTreeMap<String, Vec<String>>,
    pub groups: Vec<String>,
    pub users: Vec<(String, UserConfig)>,
}

/// One infection inside [`AgentRequest::ApplyDeployment`]: its recorded
/// metadata (name/version/order/source) plus the concrete [`Plan`] the agent
/// will apply.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyDeploymentInfection {
    pub name: String,
    pub version: String,
    pub order: u64,
    pub source: String,
    pub plan: Plan,
}

// ---------------------------------------------------------------------------
// Dry-run previews (ideas/deployments.md, phase 8 — wire masking).
//
// The concrete [`Plan`] carries rendered contents, variable values, and the
// health-check command — fine between CLI and agent (both trusted, host
// local), but never sent to a browser: values may be secrets and the state
// record is 0600 root-only. The `Preview*` types are the redacted wire form
// a reviewer needs: names, targets, owners, modes, content hashes.
// ---------------------------------------------------------------------------

/// One rendered file, redacted: placement + content hash, never the content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreviewFile {
    pub target: String,
    pub owner: String,
    pub mode: String,
    /// sha256 of the rendered content — verifiable without revealing it.
    pub sha256: String,
}

/// The owned unit, redacted: identity + enablement, never the content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreviewUnit {
    pub name: String,
    pub target: String,
    pub enable: bool,
    /// sha256 of the rendered unit content.
    pub sha256: String,
}

/// The health check, redacted: whether one is configured and how often it
/// runs — never the command (it may carry credentials).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreviewHealth {
    pub configured: bool,
    pub interval: u64,
}

/// The redacted wire form of a [`Plan`]: everything a reviewer needs to
/// approve an install (names, target paths, owners, modes, hashes, packages,
/// groups, user names, variable names, health interval) without the secret
/// parts (variable values, file/unit contents, the health command).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanPreview {
    pub name: String,
    pub version: String,
    pub description: String,
    pub files: Vec<PreviewFile>,
    pub unit: Option<PreviewUnit>,
    pub attach: Option<String>,
    pub health: PreviewHealth,
    pub declared_packages: BTreeMap<String, Vec<String>>,
    pub groups: Vec<String>,
    /// User names only (no config).
    pub users: Vec<String>,
    /// Variable names only (never values).
    pub variable_names: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceOverrides {
    pub environment: Option<HashMap<String, String>>,
    pub exec_start: Option<String>,
    pub restart: Option<String>,
    pub user: Option<String>,
    pub group: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Message {
    Request(Request),
    Response(Response),
    Event(Event),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthChallenge {
    pub nonce: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthResponse {
    pub nonce: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub topic: String,
    pub source: String,
    pub data: serde_json::Value,
    pub timestamp: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum Response {
    Success { data: Option<serde_json::Value> },
    Error { message: String },
    NotFound { message: String },
}

impl Response {
    pub fn success() -> Self {
        Self::Success { data: None }
    }

    pub fn success_with_data(data: serde_json::Value) -> Self {
        Self::Success { data: Some(data) }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self::Error {
            message: message.into(),
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound {
            message: message.into(),
        }
    }
}

// ── Epidemic broadcast (increment 3): multicast spread intent ─────────────
//
// A broadcast spread carries only the *intent* over the multicast group — a
// small cleartext datagram with no payload and no secret. It carries a
// `token = HMAC-SHA256(epidemic_secret, spread_id)`, so only nodes holding
// the group's secret act on it. The plan payload still travels the usual
// authenticated wire on the callback connection, with the roles flipped:
// the node dials the coordinator, signs its challenge, and receives the
// `ApplyDeployment` request it applies through its local agent.

/// Which stage of a broadcast a datagram is in. A canary spread is broadcast
/// twice: first at [`SpreadStage::Canary`] (only the canary cohort acts),
/// then — once promoted — at [`SpreadStage::Full`] (every matching node that
/// has not applied yet).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpreadStage {
    /// Only the canary cohort should apply.
    Canary,
    /// Every matching node applies (a promoted canary, or a non-canary
    /// spread).
    Full,
}

/// The canary cohort definition (carried in both stages of a canary spread;
/// evaluated only at [`SpreadStage::Canary`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Canary {
    /// Roughly the first `pct` percent of the matching nodes. Membership is
    /// stateless: a node is in the cohort when its bucket
    /// (`HMAC(secret, spread_id ‖ node_name)`, see the shared intent module)
    /// is below `pct` — each node decides for itself, no fleet view needed.
    Percentage { pct: u8 },
    /// The matching nodes whose identity additionally matches `criterion`
    /// (a `key=value` targeting criterion, e.g. `role=canary`).
    Subset { criterion: String },
}

/// The plan's identity as carried in the intent — *not* the payload.
/// `sha256` is the digest of the canonical JSON of the `ApplyDeployment`
/// request that follows on the callback connection, so a node detects a
/// mismatch instead of applying an unexpected plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanIdentity {
    pub name: String,
    pub version: String,
    pub sha256: String,
}

/// A broadcast spread intent: one datagram, sent to the site-local
/// multicast group (default `239.255.77.11:7712`, TTL 1 — it stays on the
/// link).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub struct SpreadIntent {
    /// Schema version. Currently `1`; nodes ignore higher versions.
    pub version: u32,
    /// Stable across the canary broadcast and its promotion. A node's
    /// de-duplication key: each node applies a given `spread_id` at most
    /// once.
    pub spread_id: String,
    /// Who should act on this datagram.
    pub stage: SpreadStage,
    /// Unix seconds when the coordinator issued the intent. Nodes reject
    /// intents outside the freshness window (replay guard, clock-skew
    /// tolerant).
    pub issued_at: i64,
    /// `HMAC-SHA256(epidemic_secret, spread_id)`, hex. Proves the issuer
    /// holds the group's secret; the secret itself never appears.
    pub token: String,
    /// The coordinator's group name — for node logging only (nodes do not
    /// know their own group name; the token is the membership proof).
    pub group: Option<String>,
    /// The plan's identity (name / version / sha256).
    pub plan: PlanIdentity,
    /// Targeting criteria, `key=value`, AND semantics, evaluated by the
    /// node against its own identity. Empty = every node in the group.
    pub criteria: Vec<String>,
    /// The canary cohort (present when `stage == Canary`).
    pub canary: Option<Canary>,
    /// The IP address the node dials back to.
    pub origin: String,
    /// The coordinator's callback TCP port.
    pub callback_port: u16,
}

// ── Epidemic spread history (increment 5a): the coordinator's audit record ──
//
// The coordinator records every spread — roster *and* broadcast — as one JSON
// line in the spread history (default
// `~/.local/state/pandemic/spread-history.log`). The record carries the plan
// identity (name / version / sha256), the targeting, and the per-node
// outcomes, so a human (and the console, increment 5b) can see what was
// spread where and what each node did with it.

/// How a spread reached its nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpreadMode {
    /// The coordinator applied the plan to an explicit roster
    /// (`epidemic spread --group/--node`).
    Roster,
    /// The coordinator broadcast the intent and nodes self-selected and dialed
    /// back (`epidemic spread --broadcast`).
    Broadcast,
}

/// One node's outcome in a recorded spread (coordinator's view).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpreadNodeResult {
    /// The node's name — its roster name, or its callback peer address for a
    /// broadcast (the coordinator never learns a broadcast node's name).
    pub name: String,
    /// The node's endpoint `host:port`.
    pub addr: String,
    /// Did the node apply the deployment?
    pub ok: bool,
    /// The failure message (when `ok` is false).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// How many apply attempts this took (increment 4b): 1 for a single
    /// attempt or the broadcast path, up to `retries + 1` when the coordinator
    /// retried transient failures.
    #[serde(default = "default_one")]
    pub attempts: u32,
}

/// `1` — the serde default for [`SpreadNodeResult::attempts`], so history
/// lines written before retries existed read back as a single attempt.
fn default_one() -> u32 {
    1
}

/// One recorded spread: one JSON line of the spread history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpreadRecord {
    /// Unix seconds when the spread was issued.
    pub timestamp: i64,
    /// The spread's identity — the broadcast's de-duplication key; a fresh
    /// id for roster spreads.
    pub spread_id: String,
    /// How the spread reached its nodes.
    pub mode: SpreadMode,
    /// `full` / `canary` / `promote` (broadcast stages; roster is `full`).
    pub stage: String,
    /// The deployment's name.
    pub name: String,
    /// The deployment's version.
    pub version: String,
    /// sha256 of the concrete `ApplyDeployment` plan (the same digest the
    /// broadcast intent carries).
    pub sha256: String,
    /// The named target group (roster path); absent for ad-hoc / broadcast.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// The coordinator's callback origin (broadcast).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// Targeting criteria (broadcast; empty = every node in the group).
    #[serde(default)]
    pub criteria: Vec<String>,
    /// The canary cohort (broadcast canary stage).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canary: Option<Canary>,
    /// Per-node outcomes.
    #[serde(default)]
    pub nodes: Vec<SpreadNodeResult>,
    /// How many nodes applied.
    pub applied: u32,
    /// How many nodes failed.
    pub failed: u32,
    /// Every node the coordinator reached applied, with no failures.
    pub ok: bool,
}

// ── Epidemic spread progress (increment 5c): live coordinator events ──────
//
// When a spread is triggered from the console (`POST /api/epidemic/spread`),
// the coordinator emits one of these as it progresses. The REST layer
// publishes each one to the daemon event bus on topic `epidemic.spread`, and
// the console's live panel renders them off the existing `/api/events/stream`
// websocket (filtered by `spread_id`). The CLI emits the same events to a
// `None` sink, so the wire shape and the audit record never diverge.

/// A node the coordinator is spreading to (roster path).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpreadTarget {
    /// The node's roster name (ad-hoc nodes name themselves by address).
    pub name: String,
    /// The node's endpoint `host:port`.
    pub addr: String,
}

/// One progress event during a spread, published on topic `epidemic.spread`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum SpreadProgress {
    /// The spread has started: what is being applied where.
    Started {
        /// The spread's identity (the console tracks progress by this).
        spread_id: String,
        /// How the spread reaches its nodes.
        mode: SpreadMode,
        /// `full` / `canary` / `promote` (roster is `full`).
        stage: String,
        /// The deployment's name.
        name: String,
        /// The deployment's version.
        version: String,
        /// sha256 of the concrete plan.
        sha256: String,
        /// The named target group (roster path).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group: Option<String>,
        /// The roster's nodes (roster path; broadcast has none — nodes
        /// self-select).
        #[serde(default)]
        nodes: Vec<SpreadTarget>,
        /// Targeting criteria (broadcast).
        #[serde(default)]
        criteria: Vec<String>,
        /// The canary cohort (broadcast canary stage).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        canary: Option<Canary>,
    },
    /// One node's outcome (as it arrives).
    Node {
        /// The spread's identity.
        spread_id: String,
        /// The node's name (roster name, or callback peer address).
        name: String,
        /// The node's endpoint `host:port`.
        addr: String,
        /// Did the node apply the deployment?
        ok: bool,
        /// The failure message (when `ok` is false).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        /// How many apply attempts this took (4b roster retries; 1 otherwise).
        #[serde(default = "default_one")]
        attempts: u32,
    },
    /// The spread has finished (the full audit record is in the history).
    Finished {
        /// The spread's identity.
        spread_id: String,
        /// How many nodes applied.
        applied: u32,
        /// How many nodes failed.
        failed: u32,
        /// Every node applied, with no failures (a benign promote no-op is
        /// still `ok`).
        ok: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;

    #[test]
    fn test_plugin_info_serialization() {
        let mut config = HashMap::new();
        config.insert("key1".to_string(), "value1".to_string());

        let plugin = PluginInfo {
            name: "test-plugin".to_string(),
            version: "1.0.0".to_string(),
            description: Some("Test description".to_string()),
            config: Some(config),
            registered_at: None,
        };

        let json = serde_json::to_string(&plugin).unwrap();
        let deserialized: PluginInfo = serde_json::from_str(&json).unwrap();

        assert_eq!(plugin.name, deserialized.name);
        assert_eq!(plugin.description, deserialized.description);
        assert_eq!(plugin.config, deserialized.config);
    }

    #[test]
    fn test_register_request_serialization() {
        let plugin = PluginInfo {
            name: "test-plugin".to_string(),
            version: "1.0.0".to_string(),
            description: None,
            config: None,
            registered_at: None,
        };

        let request = Request::Register { plugin };
        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains(r#""type":"Register""#));
        assert!(json.contains(r#""name":"test-plugin""#));
        assert!(json.contains(r#""version":"1.0.0""#));

        let deserialized: Request = serde_json::from_str(&json).unwrap();
        match deserialized {
            Request::Register { plugin } => assert_eq!(plugin.name, "test-plugin"),
            _ => panic!("Expected Register request"),
        }
    }

    #[test]
    fn test_deregister_request_serialization() {
        let request = Request::Deregister {
            name: "test-plugin".to_string(),
        };
        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains(r#""type":"Deregister""#));
        assert!(json.contains(r#""name":"test-plugin""#));

        let deserialized: Request = serde_json::from_str(&json).unwrap();
        match deserialized {
            Request::Deregister { name } => assert_eq!(name, "test-plugin"),
            _ => panic!("Expected Deregister request"),
        }
    }

    #[test]
    fn test_list_plugins_request_serialization() {
        let request = Request::ListPlugins;
        let json = serde_json::to_string(&request).unwrap();

        assert_eq!(json, r#"{"type":"ListPlugins"}"#);

        let deserialized: Request = serde_json::from_str(&json).unwrap();
        match deserialized {
            Request::ListPlugins => {}
            _ => panic!("Expected ListPlugins request"),
        }
    }

    #[test]
    fn test_get_plugin_request_serialization() {
        let request = Request::GetPlugin {
            name: "test-plugin".to_string(),
        };
        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains(r#""type":"GetPlugin""#));
        assert!(json.contains(r#""name":"test-plugin""#));

        let deserialized: Request = serde_json::from_str(&json).unwrap();
        match deserialized {
            Request::GetPlugin { name } => assert_eq!(name, "test-plugin"),
            _ => panic!("Expected GetPlugin request"),
        }
    }

    #[test]
    fn test_success_response_serialization() {
        let response = Response::success();
        let json = serde_json::to_string(&response).unwrap();

        assert!(json.contains(r#""status":"Success""#));

        let deserialized: Response = serde_json::from_str(&json).unwrap();
        match deserialized {
            Response::Success { data } => assert!(data.is_none()),
            _ => panic!("Expected Success response"),
        }
    }

    #[test]
    fn test_success_with_data_response_serialization() {
        let data = serde_json::json!({"test": "value"});
        let response = Response::success_with_data(data.clone());
        let json = serde_json::to_string(&response).unwrap();

        assert!(json.contains(r#""status":"Success""#));

        let deserialized: Response = serde_json::from_str(&json).unwrap();
        match deserialized {
            Response::Success { data: Some(d) } => assert_eq!(d, data),
            _ => panic!("Expected Success response with data"),
        }
    }

    #[test]
    fn test_error_response_serialization() {
        let response = Response::error("Test error");
        let json = serde_json::to_string(&response).unwrap();

        assert!(json.contains(r#""status":"Error""#));
        assert!(json.contains(r#""message":"Test error""#));

        let deserialized: Response = serde_json::from_str(&json).unwrap();
        match deserialized {
            Response::Error { message } => assert_eq!(message, "Test error"),
            _ => panic!("Expected Error response"),
        }
    }

    #[test]
    fn test_not_found_response_serialization() {
        let response = Response::not_found("Plugin not found");
        let json = serde_json::to_string(&response).unwrap();

        assert!(json.contains(r#""status":"NotFound""#));
        assert!(json.contains(r#""message":"Plugin not found""#));

        let deserialized: Response = serde_json::from_str(&json).unwrap();
        match deserialized {
            Response::NotFound { message } => assert_eq!(message, "Plugin not found"),
            _ => panic!("Expected NotFound response"),
        }
    }

    #[test]
    fn test_attach_infection_request_roundtrip() {
        let request = AgentRequest::AttachInfection {
            unit: "mosquitto".to_string(),
            name: Some("mosq".to_string()),
            version: Some("2.0.18".to_string()),
            description: Some("MQTT broker".to_string()),
            health_check: Some(vec!["systemctl".to_string(), "is-active".to_string()]),
            health_interval: Some(15),
            proxy_path: None,
        };
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains(r#""type":"AttachInfection""#));
        assert!(json.contains(r#""unit":"mosquitto""#));

        let deserialized: AgentRequest = serde_json::from_str(&json).unwrap();
        match deserialized {
            AgentRequest::AttachInfection {
                unit,
                name,
                version,
                health_interval,
                ..
            } => {
                assert_eq!(unit, "mosquitto");
                assert_eq!(name.as_deref(), Some("mosq"));
                assert_eq!(version.as_deref(), Some("2.0.18"));
                assert_eq!(health_interval, Some(15));
            }
            _ => panic!("Expected AttachInfection request"),
        }
    }

    #[test]
    fn test_agent_auth_messages_wire_format() {
        // The agent protocol sends bare messages (like the daemon protocol).
        // Auth messages are plain structs exchanged at fixed points of the
        // handshake; requests are `type`-tagged.
        let challenge = AuthChallenge {
            nonce: "abc123".to_string(),
        };
        let json = serde_json::to_string(&challenge).unwrap();
        assert_eq!(json, r#"{"nonce":"abc123"}"#);
        let back: AuthChallenge = serde_json::from_str(&json).unwrap();
        assert_eq!(back.nonce, "abc123");

        let response = AuthResponse {
            nonce: "abc123".to_string(),
            signature: "deadbeef".to_string(),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert_eq!(json, r#"{"nonce":"abc123","signature":"deadbeef"}"#);
        let back: AuthResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.signature, "deadbeef");
    }

    #[test]
    fn test_detach_infection_request_roundtrip() {
        let request = AgentRequest::DetachInfection {
            name: "mosquitto".to_string(),
        };
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains(r#""type":"DetachInfection""#));

        let deserialized: AgentRequest = serde_json::from_str(&json).unwrap();
        match deserialized {
            AgentRequest::DetachInfection { name } => assert_eq!(name, "mosquitto"),
            _ => panic!("Expected DetachInfection request"),
        }
    }

    #[test]
    fn test_infection_lifecycle_request_roundtrips() {
        use spec::{InfectionRecordedFile, InfectionState};

        let state = InfectionState {
            name: "rest".to_string(),
            version: "0.4.0".to_string(),
            description: "Pandemic REST API".to_string(),
            variables: {
                let mut m = std::collections::BTreeMap::new();
                m.insert("port".to_string(), "8080".to_string());
                m
            },
            groups: vec!["rest".to_string()],
            users: vec!["rest".to_string()],
            files: vec![InfectionRecordedFile {
                target: "/etc/pandemic/rest-auth.toml".to_string(),
                sha256: "abc123".to_string(),
                owner: "root".to_string(),
                mode: "0600".to_string(),
            }],
            unit: Some("rest".to_string()),
            attach: None,
            health_check: vec!["curl".to_string(), "-sf".to_string()],
            health_interval: 15,
            installed_at: None,
            owner: None,
        };

        let request = AgentRequest::RecordInfection {
            name: "rest".to_string(),
            state: state.clone(),
        };
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains(r#""type":"RecordInfection""#));
        let back: AgentRequest = serde_json::from_str(&json).unwrap();
        match back {
            AgentRequest::RecordInfection { name, state } => {
                assert_eq!(name, "rest");
                assert_eq!(state, state);
            }
            other => panic!("Expected RecordInfection, got {other:?}"),
        }

        for (req, tag) in [
            (AgentRequest::ListInfections, r#""type":"ListInfections""#),
            (
                AgentRequest::GetInfectionStatus {
                    name: "rest".to_string(),
                },
                r#""type":"GetInfectionStatus""#,
            ),
            (
                AgentRequest::UninstallInfection {
                    name: "rest".to_string(),
                    purge: false,
                },
                r#""type":"UninstallInfection""#,
            ),
            (
                AgentRequest::UninstallInfection {
                    name: "rest".to_string(),
                    purge: true,
                },
                r#""type":"UninstallInfection""#,
            ),
        ] {
            let json = serde_json::to_string(&req).unwrap();
            assert!(json.contains(tag), "missing {tag} in {json}");
            let back: AgentRequest = serde_json::from_str(&json).unwrap();
            assert_eq!(serde_json::to_string(&back).unwrap(), json);
        }
    }

    #[test]
    fn test_deployment_lifecycle_request_roundtrips() {
        use spec::{DeploymentRecordedInfection, DeploymentState};

        let state = DeploymentState {
            name: "rest-stack".to_string(),
            version: "1.2.0".to_string(),
            variables: {
                let mut m = std::collections::BTreeMap::new();
                m.insert("port".to_string(), "8080".to_string());
                m
            },
            infections: vec![DeploymentRecordedInfection {
                name: "rest".to_string(),
                version: "0.4.0".to_string(),
                order: 1,
                source: "rest/infection.toml".to_string(),
            }],
            installed_at: None,
        };

        let request = AgentRequest::RecordDeployment {
            name: "rest-stack".to_string(),
            state: state.clone(),
        };
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains(r#""type":"RecordDeployment""#));
        let back: AgentRequest = serde_json::from_str(&json).unwrap();
        match back {
            AgentRequest::RecordDeployment { name, state } => {
                assert_eq!(name, "rest-stack");
                assert_eq!(state, state);
            }
            other => panic!("Expected RecordDeployment, got {other:?}"),
        }

        for (req, tag) in [
            (AgentRequest::ListDeployments, r#""type":"ListDeployments""#),
            (
                AgentRequest::GetDeploymentStatus {
                    name: "rest-stack".to_string(),
                },
                r#""type":"GetDeploymentStatus""#,
            ),
            (
                AgentRequest::RemoveDeployment {
                    name: "rest-stack".to_string(),
                    purge: false,
                },
                r#""type":"RemoveDeployment""#,
            ),
            (
                AgentRequest::RemoveDeployment {
                    name: "rest-stack".to_string(),
                    purge: true,
                },
                r#""type":"RemoveDeployment""#,
            ),
        ] {
            let json = serde_json::to_string(&req).unwrap();
            assert!(json.contains(tag), "missing {tag} in {json}");
            let back: AgentRequest = serde_json::from_str(&json).unwrap();
            assert_eq!(serde_json::to_string(&back).unwrap(), json);
        }
    }

    #[test]
    fn test_timestamp_serialization_roundtrip() {
        let original_time = Utc::now();
        let plugin = PluginInfo {
            name: "test".to_string(),
            version: "1.0.0".to_string(),
            description: None,
            config: None,
            registered_at: Some(original_time),
        };

        let json = serde_json::to_string(&plugin).unwrap();
        // Timestamp is now serialized as RFC3339 string
        assert!(json.contains("registered_at"));

        let deserialized: PluginInfo = serde_json::from_str(&json).unwrap();

        let deserialized_time = deserialized.registered_at.unwrap();
        let diff = deserialized_time
            .signed_duration_since(original_time)
            .to_std()
            .unwrap();
        assert!(
            diff.as_secs() <= 1,
            "Timestamp mismatch: original={original_time}, deserialized={deserialized_time}"
        );
    }

    #[test]
    fn test_plan_preview_serialization_roundtrip() {
        let preview = PlanPreview {
            name: "rest".to_string(),
            version: "1.2.3".to_string(),
            description: "REST API".to_string(),
            files: vec![PreviewFile {
                target: "/etc/pandemic/rest/rest-auth.toml".to_string(),
                owner: "pandemic".to_string(),
                mode: "0600".to_string(),
                sha256: "a".repeat(64),
            }],
            unit: Some(PreviewUnit {
                name: "pandemic-rest".to_string(),
                target: "/etc/systemd/system/pandemic-rest.service".to_string(),
                enable: true,
                sha256: "b".repeat(64),
            }),
            attach: None,
            health: PreviewHealth {
                configured: true,
                interval: 30,
            },
            declared_packages: BTreeMap::from([("apt".to_string(), vec!["curl".to_string()])]),
            groups: vec!["pandemic".to_string()],
            users: vec!["pandemic".to_string()],
            variable_names: vec!["api_key".to_string(), "listen_port".to_string()],
        };

        let json = serde_json::to_string(&preview).unwrap();
        let back: PlanPreview = serde_json::from_str(&json).unwrap();
        assert_eq!(json, serde_json::to_string(&back).unwrap());
        assert_eq!(back.users, vec!["pandemic".to_string()]);
        assert_eq!(
            back.variable_names,
            vec!["api_key".to_string(), "listen_port".to_string()]
        );
        assert_eq!(back.files[0].sha256, "a".repeat(64));
    }

    fn sample_intent(canary: Option<Canary>, stage: SpreadStage) -> SpreadIntent {
        SpreadIntent {
            version: 1,
            spread_id: "8f3a1c02d4e5b6a7".to_string(),
            stage,
            issued_at: 1_750_000_000,
            token: "ab".repeat(32),
            group: Some("edge".to_string()),
            plan: PlanIdentity {
                name: "webapp".to_string(),
                version: "1.2.3".to_string(),
                sha256: "cd".repeat(32),
            },
            criteria: vec!["role=edge".to_string(), "cap:mqtt=true".to_string()],
            canary,
            origin: "192.168.1.5".to_string(),
            callback_port: 41731,
        }
    }

    #[test]
    fn spread_intent_round_trips() {
        for (canary, stage) in [
            (None, SpreadStage::Full),
            (Some(Canary::Percentage { pct: 25 }), SpreadStage::Canary),
            (
                Some(Canary::Subset {
                    criterion: "role=canary".to_string(),
                }),
                SpreadStage::Canary,
            ),
        ] {
            let intent = sample_intent(canary, stage);
            let json = serde_json::to_string(&intent).unwrap();
            let back: SpreadIntent = serde_json::from_str(&json).unwrap();
            assert_eq!(json, serde_json::to_string(&back).unwrap());
            assert_eq!(back, intent);
        }
    }

    #[test]
    fn spread_intent_carries_its_type_tag() {
        let json = serde_json::to_string(&sample_intent(None, SpreadStage::Full)).unwrap();
        assert!(json.contains(r#""type":"SpreadIntent""#), "got: {json}");
    }

    #[test]
    fn canary_variants_are_distinguishable_on_the_wire() {
        let pct: Canary = serde_json::from_str(r#"{"kind":"percentage","pct":10}"#).unwrap();
        let sub: Canary =
            serde_json::from_str(r#"{"kind":"subset","criterion":"env=staging"}"#).unwrap();
        assert_eq!(pct, Canary::Percentage { pct: 10 });
        assert_eq!(
            sub,
            Canary::Subset {
                criterion: "env=staging".to_string()
            }
        );
    }

    #[test]
    fn spread_stage_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&SpreadStage::Canary).unwrap(),
            r#""canary""#
        );
        assert_eq!(
            serde_json::to_string(&SpreadStage::Full).unwrap(),
            r#""full""#
        );
    }

    #[test]
    fn spread_record_round_trips_as_one_json_line() {
        let record = SpreadRecord {
            timestamp: 1_750_000_123,
            spread_id: "8f3a1c02d4e5b6a7".to_string(),
            mode: SpreadMode::Roster,
            stage: "full".to_string(),
            name: "webapp".to_string(),
            version: "1.2.3".to_string(),
            sha256: "cd".repeat(32),
            group: Some("edge".to_string()),
            origin: None,
            criteria: Vec::new(),
            canary: None,
            nodes: vec![
                SpreadNodeResult {
                    name: "edge-1".to_string(),
                    addr: "10.0.0.1:7711".to_string(),
                    ok: true,
                    error: None,
                    attempts: 1,
                },
                SpreadNodeResult {
                    name: "edge-2".to_string(),
                    addr: "10.0.0.2:7711".to_string(),
                    ok: false,
                    error: Some("node 10.0.0.2:7711 failed to apply".to_string()),
                    attempts: 3,
                },
            ],
            applied: 1,
            failed: 1,
            ok: false,
        };

        let line = serde_json::to_string(&record).unwrap();
        assert!(!line.contains('\n'), "a record is one JSON line: {line}");
        let back: SpreadRecord = serde_json::from_str(&line).unwrap();
        assert_eq!(back, record);
        // The mode is snake_case on the wire (the console keys on it).
        assert!(line.contains(r#""mode":"roster""#), "got: {line}");
    }

    #[test]
    fn spread_record_missing_optionals_default() {
        // A minimal line (e.g. a record written before a field existed)
        // still parses — the console must not break on history evolution.
        let json = r#"{
            "timestamp": 0,
            "spread_id": "x",
            "mode": "broadcast",
            "stage": "full",
            "name": "n",
            "version": "1",
            "sha256": "ab",
            "applied": 0,
            "failed": 0,
            "ok": true
        }"#;
        let rec: SpreadRecord = serde_json::from_str(json).unwrap();
        assert_eq!(rec.mode, SpreadMode::Broadcast);
        assert!(rec.group.is_none());
        assert!(rec.origin.is_none());
        assert!(rec.criteria.is_empty());
        assert!(rec.canary.is_none());
        assert!(rec.nodes.is_empty());
    }
}
