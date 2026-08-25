pub mod agent;
pub mod client;
pub mod registry;
mod tests;

// Re-export public APIs for easy access
pub use agent::{AgentClient, AgentStatus, AGENT_SECRET_PATH, AGENT_SOCKET_PATH};
pub use client::{DaemonClient, PersistentClient};
pub use registry::{InfectionManifest, InfectionSummary, RegistryClient};
