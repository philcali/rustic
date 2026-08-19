use pandemic_protocol::Event;
use std::collections::HashMap;

pub struct EventBus {
    pub subscribers: HashMap<String, Vec<String>>, // plugin_name -> topics
    /// Maps plugin_name to the connection ID that should receive events.
    connection_map: HashMap<String, String>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    pub fn new() -> Self {
        Self {
            subscribers: HashMap::new(),
            connection_map: HashMap::new(),
        }
    }

    pub fn subscribe(&mut self, plugin_name: &str, connection_id: &str, topics: Vec<String>) {
        self.subscribers.insert(plugin_name.to_string(), topics);
        self.connection_map
            .insert(plugin_name.to_string(), connection_id.to_string());
    }

    pub fn register_connection(&mut self, plugin_name: &str, connection_id: &str) {
        self.connection_map
            .insert(plugin_name.to_string(), connection_id.to_string());
        // Auto-subscribe to its own registration event
        let topics = self.subscribers.entry(plugin_name.to_string()).or_default();
        if !topics.contains(&"plugin.registered".to_string()) {
            topics.push("plugin.registered".to_string());
        }
    }

    pub fn unsubscribe(&mut self, plugin_name: &str, topics: &[String]) {
        if let Some(current_topics) = self.subscribers.get_mut(plugin_name) {
            current_topics.retain(|t| !topics.contains(t));
            // Clean up empty subscriptions
            if current_topics.is_empty() {
                self.subscribers.remove(plugin_name);
                self.connection_map.remove(plugin_name);
            }
        }
    }

    /// Publish an event and return the list of connection IDs that should receive it.
    pub fn publish(&mut self, event: &Event) -> Vec<String> {
        let mut targets = Vec::new();

        for (plugin_name, topics) in &self.subscribers {
            let matches = topics.iter().any(|topic| {
                if topic.ends_with('*') {
                    event.topic.starts_with(topic.trim_end_matches('*'))
                } else {
                    event.topic == *topic
                }
            });

            if matches {
                if let Some(connection_id) = self.connection_map.get(plugin_name).cloned() {
                    targets.push(connection_id);
                }
            }
        }

        targets
    }

    pub fn remove_plugin(&mut self, plugin_name: &str) {
        self.subscribers.remove(plugin_name);
        self.connection_map.remove(plugin_name);
    }
}
