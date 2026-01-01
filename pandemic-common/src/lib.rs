pub mod agent;
pub mod client;
pub mod registry;
mod tests;

// Re-export public APIs for easy access
pub use agent::{AgentClient, AgentStatus};
pub use client::{DaemonClient, PersistentClient};
pub use registry::{InfectionManifest, InfectionSummary, RegistryClient};
