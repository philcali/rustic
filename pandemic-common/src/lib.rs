pub mod agent;
pub mod apply;
pub mod audit;
pub mod auth;
pub mod client;
pub mod coordinator;
pub mod discovery;
pub mod groups;
pub mod history;
pub mod intent;
pub mod multicast;
pub mod ratelimit;
pub mod registry;
pub mod remote;
pub mod resolve;
mod tests;
pub mod tls;
pub mod wire;

// Re-export public APIs for easy access
pub use agent::{AgentClient, AgentStatus, AGENT_SECRET_PATH, AGENT_SOCKET_PATH};
pub use apply::{
    build_deployment_plan, build_deployment_plan_from, build_plan, build_plan_from_spec,
    build_plan_from_spec_with, deployment_apply_infections, deployment_preview_data, find_template,
    parse_set_args, parse_set_values, plan_preview, sha256_hex, validate_set, DeploymentPlan,
    ResolvedInfection,
};
pub use auth::{generate_nonce, generate_secret, sign, verify, EPIDEMIC_SECRET_PATH};
pub use client::{DaemonClient, PersistentClient};
pub use coordinator::{
    apply_with_retries, broadcast_node_results, build_intent, host_of, merge_discovered,
    now_unix_secs, parse_canary_arg, primary_lan_ip, resolve_epidemic_secret, resolve_roster,
    roster_node_results, roster_record, run_broadcast_spread, run_roster_spread,
    serve_one_callback, validate_broadcast, ApplyOutcome, BroadcastSpread, ResolvedSecret,
    RosterSpread, SecretSource, SpreadResult, TlsOptions, DEFAULT_APPLY_RETRIES,
    DEFAULT_APPLY_TIMEOUT,
};
pub use discovery::{advertise_node, discover_nodes, Advertiser, DiscoveredNode, SERVICE_FQDN};
pub use groups::{
    default_groups_path, find_group, load_groups, load_groups_or_default, GroupConfig, NodeConfig,
};
pub use history::{append_record, default_history_path, load_spreads};
pub use intent::{
    cohort_bucket, generate_spread_id, in_canary_cohort, intent_is_fresh, intent_token,
    needs_capabilities, parse_criteria, verify_intent_token, Criterion, NodeIdentity,
    INTENT_FRESHNESS, INTENT_VERSION,
};
pub use multicast::{
    send_intent, IntentListener, DEFAULT_MULTICAST_GROUP, DEFAULT_MULTICAST_PORT, INTENT_RESENDS,
};
pub use ratelimit::{
    default_rate_limit_path, RateLimited, SpreadRateLimit, DEFAULT_RATE_LIMIT_MAX,
    DEFAULT_RATE_LIMIT_WINDOW_SECS,
};
pub use registry::{
    extract_bundle, verify_sha256, InfectionManifest, InfectionSummary, RegistryClient,
};
pub use remote::RemoteClient;
pub use resolve::{resolve_deployment_target, resolve_infection_target};
pub use tls::{TlsClient, TlsServer};
