pub mod agent;
pub mod apply;
pub mod client;
pub mod registry;
mod tests;

// Re-export public APIs for easy access
pub use agent::{AgentClient, AgentStatus, AGENT_SECRET_PATH, AGENT_SOCKET_PATH};
pub use apply::{
    build_deployment_plan, build_deployment_plan_from, build_plan, build_plan_from_spec,
    build_plan_from_spec_with, deployment_apply_infections, find_template, parse_set_args,
    sha256_hex, DeploymentPlan, ResolvedInfection,
};
pub use client::{DaemonClient, PersistentClient};
pub use registry::{InfectionManifest, InfectionSummary, RegistryClient};
