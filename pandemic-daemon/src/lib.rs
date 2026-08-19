//! Public API for the pandemic daemon.
//!
//! This library exposes the core daemon types for use by other crates
//! and for integration testing.

pub mod connection;
pub mod daemon;
pub mod event_bus;
pub mod handlers;

/// Test harness for integration tests.
pub mod tests {
    pub mod test_harness;
}
