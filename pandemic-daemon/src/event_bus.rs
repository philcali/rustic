use pandemic_protocol::Event;
use std::collections::HashMap;

pub struct EventBus {
    pub subscribers: HashMap<String, Vec<String>>, // plugin_name -> topics
    /// Maps plugin_name to the connection ID that should receive events.
    connection_map: HashMap<String, String>,
    /// Reverse index: topic -> list of connection IDs subscribed to that exact topic.
    topic_index: HashMap<String, Vec<String>>,
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
            topic_index: HashMap::new(),
        }
    }

    pub fn subscribe(&mut self, plugin_name: &str, connection_id: &str, topics: Vec<String>) {
        self.subscribers
            .insert(plugin_name.to_string(), topics.clone());
        self.connection_map
            .insert(plugin_name.to_string(), connection_id.to_string());
        // Update the reverse index for exact topic matches
        for topic in &topics {
            self.topic_index
                .entry(topic.clone())
                .or_default()
                .push(connection_id.to_string());
        }
    }

    pub fn register_connection(&mut self, plugin_name: &str, connection_id: &str) {
        self.connection_map
            .insert(plugin_name.to_string(), connection_id.to_string());
        // Auto-subscribe to its own registration event
        let topics = self.subscribers.entry(plugin_name.to_string()).or_default();
        if !topics.contains(&"plugin.registered".to_string()) {
            topics.push("plugin.registered".to_string());
            self.topic_index
                .entry("plugin.registered".to_string())
                .or_default()
                .push(connection_id.to_string());
        }
    }

    pub fn unsubscribe(&mut self, plugin_name: &str, topics: &[String]) {
        if let Some(current_topics) = self.subscribers.get_mut(plugin_name) {
            current_topics.retain(|t| !topics.contains(t));
            // Clean up empty subscriptions
            if current_topics.is_empty() {
                let conn_id = self.connection_map.remove(plugin_name);
                self.subscribers.remove(plugin_name);
                // Remove from reverse index
                if let Some(id) = conn_id {
                    for topic in topics {
                        if let Some(connections) = self.topic_index.get_mut(topic) {
                            connections.retain(|c| c != &id);
                            if connections.is_empty() {
                                self.topic_index.remove(topic);
                            }
                        }
                    }
                }
            }
        }
    }

    /// Publish an event and return the list of connection IDs that should receive it.
    pub fn publish(&mut self, event: &Event) -> Vec<String> {
        let mut targets = Vec::new();

        // Check the reverse index for exact topic matches (O(1) per subscriber)
        if let Some(connection_ids) = self.topic_index.get(&event.topic) {
            targets.extend(connection_ids.clone());
        }

        // Handle wildcard subscriptions (cannot be pre-indexed)
        for (plugin_name, topics) in &self.subscribers {
            let has_wildcard = topics.iter().any(|t| t.ends_with('*'));
            if !has_wildcard {
                continue;
            }
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
        // Collect topics and connection ID before removing the plugin entry
        let (topics, conn_id) = self
            .subscribers
            .remove(plugin_name)
            .map(|topics| (topics, self.connection_map.remove(plugin_name)))
            .unwrap_or_default();
        // Remove from reverse index
        if let Some(id) = conn_id {
            for topic in &topics {
                if let Some(connections) = self.topic_index.get_mut(topic) {
                    connections.retain(|c| c != &id);
                    if connections.is_empty() {
                        self.topic_index.remove(topic);
                    }
                }
            }
        }
    }
}
