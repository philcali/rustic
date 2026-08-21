use chrono::Utc;
use pandemic_protocol::{Event, Request, Response};
use serde_json::json;
use tracing::info;

use crate::daemon::Daemon;

impl Daemon {
    pub fn handle_request(&mut self, request: Request, connection_id: &str) -> Response {
        match request {
            Request::Register { mut plugin } => {
                info!("Registering plugin: {}", plugin.name);
                plugin.registered_at = Some(Utc::now());

                if let Some(context) = self.connections.get_mut(connection_id) {
                    context.plugin_name = Some(plugin.name.clone());
                }

                let event = Event {
                    topic: "plugin.registered".to_string(),
                    source: "pandemic".to_string(),
                    data: json!(plugin),
                    timestamp: Some(Utc::now()),
                };
                self.event_bus
                    .register_connection(&plugin.name, connection_id);
                let targets = self.event_bus.publish(&event);
                for target_id in targets {
                    if let Some(context) = self.connections.get(&target_id) {
                        let _ = context.event_sender.send(event.clone());
                    }
                }

                let plugin_name = plugin.name.clone();
                self.plugins.insert(plugin_name.clone(), plugin);
                self.registered_by
                    .insert(plugin_name, connection_id.to_string());
                Response::success()
            }
            Request::Deregister { name } => match self.plugins.remove(&name) {
                Some(plugin) => {
                    info!("Deregistered plugin: {}", plugin.name);

                    let event = Event {
                        topic: "plugin.deregistered".to_string(),
                        source: "pandemic".to_string(),
                        data: json!({"name": name}),
                        timestamp: Some(Utc::now()),
                    };
                    let targets = self.event_bus.publish(&event);
                    for target_id in targets {
                        if let Some(context) = self.connections.get(&target_id) {
                            let _ = context.event_sender.send(event.clone());
                        }
                    }
                    self.event_bus.remove_plugin(&name);
                    self.registered_by.remove(&name);

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
                    let plugin_name = context
                        .plugin_name
                        .clone()
                        .unwrap_or_else(|| format!("sub-{}", connection_id));
                    self.event_bus
                        .subscribe(&plugin_name, connection_id, topics);
                    // Also set the synthetic plugin name on the context so events can be routed
                    if context.plugin_name.is_none() {
                        if let Some(ctx) = self.connections.get_mut(connection_id) {
                            ctx.plugin_name = Some(plugin_name);
                        }
                    }
                    Response::success()
                } else {
                    Response::error("Connection not found")
                }
            }
            Request::Unsubscribe { topics } => {
                if let Some(context) = self.connections.get(connection_id) {
                    let plugin_name = context
                        .plugin_name
                        .clone()
                        .unwrap_or_else(|| format!("sub-{}", connection_id));
                    self.event_bus.unsubscribe(&plugin_name, &topics);
                    Response::success()
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
                    timestamp: Some(Utc::now()),
                };
                let targets = self.event_bus.publish(&event);
                for target_id in targets {
                    if let Some(context) = self.connections.get(&target_id) {
                        let _ = context.event_sender.send(event.clone());
                    }
                }
                Response::success()
            }
            Request::GetHealth => {
                let health = self.collect_health_metrics();
                Response::success_with_data(json!(health))
            }
        }
    }
}
