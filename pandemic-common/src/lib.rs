pub mod agent;
pub mod apply;
pub mod audit;
pub mod client;
pub mod registry;
pub mod resolve;
mod tests;

// Re-export public APIs for easy access
pub use agent::{AgentClient, AgentStatus, AGENT_SECRET_PATH, AGENT_SOCKET_PATH};
pub use apply::{
    build_deployment_plan, build_deployment_plan_from, build_plan, build_plan_from_spec,
    build_plan_from_spec_with, deployment_apply_infections, deployment_preview_data, find_template,
    parse_set_args, parse_set_values, plan_preview, sha256_hex, validate_set, DeploymentPlan,
    ResolvedInfection,
};
pub use client::{DaemonClient, PersistentClient};
pub use registry::{
    extract_bundle, verify_sha256, InfectionManifest, InfectionSummary, RegistryClient,
};
pub use resolve::{resolve_deployment_target, resolve_infection_target};
