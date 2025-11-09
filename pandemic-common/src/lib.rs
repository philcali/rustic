pub mod agent;
pub mod client;
mod tests;

// Re-export public APIs for easy access
pub use agent::{AgentClient, AgentStatus};
pub use client::{DaemonClient, PersistentClient};
