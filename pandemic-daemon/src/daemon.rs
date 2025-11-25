use pandemic_protocol::{Event, HealthMetrics, PluginInfo};
use std::collections::HashMap;
use std::time::{Duration, SystemTime};
use sysinfo::System;
use tokio::sync::mpsc;
use tracing::info;

use crate::event_bus::EventBus;

pub struct ConnectionContext {
    pub plugin_name: Option<String>,
    pub event_sender: mpsc::UnboundedSender<Event>,
}

pub struct Daemon {
    pub plugins: HashMap<String, PluginInfo>,
    pub event_bus: EventBus,
    pub connections: HashMap<String, ConnectionContext>,
    start_time: SystemTime,
    system: System,
}

impl Daemon {
    pub fn new() -> Self {
        Self {
            plugins: HashMap::new(),
            event_bus: EventBus::new(),
            connections: HashMap::new(),
            start_time: SystemTime::now(),
            system: System::new_all(),
        }
    }

    pub fn collect_health_metrics(&mut self) -> HealthMetrics {
        self.system.refresh_all();

        let uptime = self
            .start_time
            .elapsed()
            .unwrap_or(Duration::ZERO)
            .as_secs();
        let memory = self.system.total_memory() / 1024 / 1024;
        let memory_used = self.system.used_memory() / 1024 / 1024;

        let cpu_usage = self.system.global_cpu_info().cpu_usage();
        let load_avg = System::load_average();

        HealthMetrics {
            active_plugins: self.plugins.len(),
            total_connections: self.connections.len(),
            event_bus_subscribers: self.event_bus.subscribers.len(),
            uptime_seconds: uptime,
            memory_used_mb: memory_used,
            memory_total_mb: memory,
            cpu_usage_percent: cpu_usage,
            load_average: if load_avg.one > 0.0 {
                Some(load_avg.one as f32)
            } else {
                None
            },
        }
    }

    pub fn add_connection(&mut self, connection_id: String) -> mpsc::UnboundedReceiver<Event> {
        let (tx, rx) = mpsc::unbounded_channel();
        let context = ConnectionContext {
            plugin_name: None,
            event_sender: tx,
        };
        self.connections.insert(connection_id, context);
        rx
    }

    pub fn remove_connection(&mut self, connection_id: &str) {
        if let Some(context) = self.connections.remove(connection_id) {
            if let Some(plugin_name) = &context.plugin_name {
                if self.event_bus.subscribers.contains_key(plugin_name) {
                    self.event_bus.remove_plugin(plugin_name);
                    self.plugins.remove(plugin_name);
                    info!(
                        "Removed plugin {} due to persistent connection close",
                        plugin_name
                    );
                } else {
                    info!("Transient connection for plugin {} closed", plugin_name);
                }
            }
        }
    }
}
