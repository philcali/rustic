use pandemic_protocol::{Event, Request, Response};
use serde_json::json;
use std::time::SystemTime;
use tracing::info;

use crate::daemon::Daemon;

impl Daemon {
    pub fn handle_request(&mut self, request: Request, connection_id: &str) -> Response {
        match request {
            Request::Register { mut plugin } => {
                info!("Registering plugin: {}", plugin.name);
                plugin.registered_at = Some(SystemTime::now());

                if let Some(context) = self.connections.get_mut(connection_id) {
                    context.plugin_name = Some(plugin.name.clone());
                }

                let event = Event {
                    topic: "plugin.registered".to_string(),
                    source: "pandemic".to_string(),
                    data: json!(plugin),
                    timestamp: Some(SystemTime::now()),
                };
                self.event_bus.publish(event, &self.connections);

                self.plugins.insert(plugin.name.clone(), plugin);
                Response::success()
            }
            Request::Deregister { name } => match self.plugins.remove(&name) {
                Some(plugin) => {
                    info!("Deregistered plugin: {}", plugin.name);

                    let event = Event {
                        topic: "plugin.deregistered".to_string(),
                        source: "pandemic".to_string(),
                        data: json!({"name": name}),
                        timestamp: Some(SystemTime::now()),
                    };
                    self.event_bus.publish(event, &self.connections);
                    self.event_bus.remove_plugin(&name);

                    Response::success()
                }
                None => Response::not_found(format!("Plugin '{}' not found", name)),
            },
            Request::ListPlugins => {
                let plugins: Vec<&_> = self.plugins.values().collect();
                Response::success_with_data(json!(plugins))
            }
            Request::GetPlugin { name } => match self.plugins.get(&name) {
                Some(plugin) => Response::success_with_data(json!(plugin)),
                None => Response::not_found(format!("Plugin '{}' not found", name)),
            },
            Request::Subscribe { topics } => {
                if let Some(context) = self.connections.get(connection_id) {
                    if let Some(plugin_name) = &context.plugin_name {
                        self.event_bus.subscribe(plugin_name, topics);
                        Response::success()
                    } else {
                        Response::error("Must register plugin before subscribing to events")
                    }
                } else {
                    Response::error("Connection not found")
                }
            }
            Request::Unsubscribe { topics } => {
                if let Some(context) = self.connections.get(connection_id) {
                    if let Some(plugin_name) = &context.plugin_name {
                        self.event_bus.unsubscribe(plugin_name, &topics);
                        Response::success()
                    } else {
                        Response::error("Must register plugin before unsubscribing from events")
                    }
                } else {
                    Response::error("Connection not found")
                }
            }
            Request::Publish { topic, data } => {
                let source = if let Some(context) = self.connections.get(connection_id) {
                    context
                        .plugin_name
                        .clone()
                        .unwrap_or_else(|| "unknown".to_string())
                } else {
                    "unknown".to_string()
                };

                let event = Event {
                    topic,
                    source,
                    data,
                    timestamp: Some(SystemTime::now()),
                };
                self.event_bus.publish(event, &self.connections);
                Response::success()
            }
            Request::GetHealth => {
                let health = self.collect_health_metrics();
                Response::success_with_data(json!(health))
            }
        }
    }
}
