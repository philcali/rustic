pub mod client;
pub mod config;
mod tests;

// Re-export public APIs for easy access
pub use client::{DaemonClient, PersistentClient};
pub use config::{ConfigManager, FileConfigManager};
