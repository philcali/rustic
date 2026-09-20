pub mod agent;
pub mod apply;
pub mod audit;
pub mod auth;
pub mod client;
pub mod discovery;
pub mod groups;
pub mod registry;
pub mod remote;
pub mod resolve;
mod tests;
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
pub use discovery::{advertise_node, discover_nodes, Advertiser, DiscoveredNode, SERVICE_FQDN};
pub use groups::{
    default_groups_path, find_group, load_groups, load_groups_or_default, GroupConfig, NodeConfig,
};
pub use registry::{
    extract_bundle, verify_sha256, InfectionManifest, InfectionSummary, RegistryClient,
};
pub use remote::RemoteClient;
pub use resolve::{resolve_deployment_target, resolve_infection_target};
