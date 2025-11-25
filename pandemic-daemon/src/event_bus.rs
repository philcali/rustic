use pandemic_protocol::Event;
use std::collections::HashMap;
use tracing::{info, warn};

use crate::daemon::ConnectionContext;

pub struct EventBus {
    pub subscribers: HashMap<String, Vec<String>>, // plugin_name -> topics
}

impl EventBus {
    pub fn new() -> Self {
        Self {
            subscribers: HashMap::new(),
        }
    }

    pub fn subscribe(&mut self, plugin_name: &str, topics: Vec<String>) {
        self.subscribers.insert(plugin_name.to_string(), topics);
    }

    pub fn unsubscribe(&mut self, plugin_name: &str, topics: &[String]) {
        if let Some(current_topics) = self.subscribers.get_mut(plugin_name) {
            current_topics.retain(|t| !topics.contains(t));
        }
    }

    pub fn publish(&mut self, event: Event, connections: &HashMap<String, ConnectionContext>) {
        for (plugin_name, topics) in &self.subscribers {
            let matches = topics.iter().any(|topic| {
                if topic.ends_with('*') {
                    event.topic.starts_with(topic.trim_end_matches('*'))
                } else {
                    event.topic == *topic
                }
            });

            if matches {
                info!(
                    "Matched event source {}, topic {} for plugin {}",
                    event.source, event.topic, plugin_name
                );

                for context in connections.values() {
                    if let Some(ref conn_plugin_name) = context.plugin_name {
                        if conn_plugin_name == plugin_name {
                            if context.event_sender.send(event.clone()).is_err() {
                                warn!(
                                    "Failed to send event to plugin {}, channel closed",
                                    plugin_name
                                );
                            }
                            break;
                        }
                    }
                }
            }
        }
    }

    pub fn remove_plugin(&mut self, plugin_name: &str) {
        self.subscribers.remove(plugin_name);
    }
}
