use anyhow::Result;
use clap::Parser;
use pandemic_protocol::{Event, HealthMetrics, Message, PluginInfo, Request, Response};
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use sysinfo::System;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, Mutex};
use tracing::{error, info, warn};

#[derive(Parser)]
#[command(name = "pandemic")]
#[command(about = "Lightweight daemon for managing infection plugins")]
struct Args {
    #[arg(long, default_value = "/var/run/pandemic/pandemic.sock")]
    socket_path: PathBuf,
}

struct EventBus {
    subscribers: HashMap<String, Vec<String>>, // plugin_name -> topics
}

impl EventBus {
    fn new() -> Self {
        Self {
            subscribers: HashMap::new(),
        }
    }

    fn subscribe(&mut self, plugin_name: &str, topics: Vec<String>) {
        self.subscribers.insert(plugin_name.to_string(), topics);
    }

    fn unsubscribe(&mut self, plugin_name: &str, topics: &[String]) {
        if let Some(current_topics) = self.subscribers.get_mut(plugin_name) {
            current_topics.retain(|t| !topics.contains(t));
        }
    }

    fn publish(&mut self, event: Event, connections: &HashMap<String, ConnectionContext>) {
        for (plugin_name, topics) in &self.subscribers {
            let matches = topics.iter().any(|topic| {
                if topic.ends_with('*') {
                    // Wildcard match: "plugin.*" matches "plugin.registered"
                    event.topic.starts_with(topic.trim_end_matches('*'))
                } else {
                    // Exact match: "plugin.registered" matches "plugin.registered"
                    event.topic == *topic
                }
            });

            if matches {
                info!(
                    "Matched event source {}, topic {} for plugin {}",
                    event.source, event.topic, plugin_name
                );

                // Find the connection for this plugin and send event
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

    fn remove_plugin(&mut self, plugin_name: &str) {
        self.subscribers.remove(plugin_name);
    }
}

struct ConnectionContext {
    plugin_name: Option<String>,
    event_sender: mpsc::UnboundedSender<Event>,
}

struct Daemon {
    plugins: HashMap<String, PluginInfo>,
    event_bus: EventBus,
    connections: HashMap<String, ConnectionContext>, // connection_id -> context
    start_time: SystemTime,
    system: System,
}

impl Daemon {
    fn new() -> Self {
        Self {
            plugins: HashMap::new(),
            event_bus: EventBus::new(),
            connections: HashMap::new(),
            start_time: SystemTime::now(),
            system: System::new_all(),
        }
    }

    fn collect_health_metrics(&mut self) -> HealthMetrics {
        self.system.refresh_all();

        let uptime = self
            .start_time
            .elapsed()
            .unwrap_or(Duration::ZERO)
            .as_secs();
        let memory = self.system.total_memory() / 1024 / 1024; // Convert to MB
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

    fn handle_request(&mut self, request: Request, connection_id: &str) -> Response {
        match request {
            Request::Register { mut plugin } => {
                info!("Registering plugin: {}", plugin.name);
                plugin.registered_at = Some(SystemTime::now());

                // Associate connection with plugin
                if let Some(context) = self.connections.get_mut(connection_id) {
                    context.plugin_name = Some(plugin.name.clone());
                }

                // Publish plugin registered event
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
            Request::Deregister { name } => {
                match self.plugins.remove(&name) {
                    Some(plugin) => {
                        info!("Deregistered plugin: {}", plugin.name);

                        // Publish plugin deregistered event
                        let event = Event {
                            topic: "plugin.deregistered".to_string(),
                            source: "pandemic".to_string(),
                            data: json!({"name": name}),
                            timestamp: Some(SystemTime::now()),
                        };
                        self.event_bus.publish(event, &self.connections);

                        // Remove from event bus
                        self.event_bus.remove_plugin(&name);

                        Response::success()
                    }
                    None => Response::not_found(format!("Plugin '{}' not found", name)),
                }
            }
            Request::ListPlugins => {
                let plugins: Vec<&PluginInfo> = self.plugins.values().collect();
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

    fn add_connection(&mut self, connection_id: String) -> mpsc::UnboundedReceiver<Event> {
        let (tx, rx) = mpsc::unbounded_channel();
        let context = ConnectionContext {
            plugin_name: None,
            event_sender: tx,
        };
        self.connections.insert(connection_id, context);
        rx
    }

    fn remove_connection(&mut self, connection_id: &str) {
        if let Some(context) = self.connections.remove(connection_id) {
            // Only remove plugin if this was a persistent connection that had subscribed to events
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

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    if let Some(parent) = args.socket_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let _ = tokio::fs::remove_file(&args.socket_path).await;
    let listener = UnixListener::bind(&args.socket_path)?;
    info!("Pandemic daemon listening on {:?}", args.socket_path);

    let _daemon = Daemon::new();

    let daemon = Arc::new(Mutex::new(_daemon));
    let mut connection_counter = 0u64;

    while let Ok((stream, _)) = listener.accept().await {
        connection_counter += 1;
        let connection_id = format!("conn_{}", connection_counter);

        let event_rx = {
            let mut daemon_guard = daemon.lock().await;
            daemon_guard.add_connection(connection_id.clone())
        };

        let daemon_clone = Arc::clone(&daemon);
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, connection_id, daemon_clone, event_rx).await {
                error!("Connection error: {}", e);
            }
        });
    }

    Ok(())
}

async fn handle_connection(
    stream: UnixStream,
    connection_id: String,
    daemon: Arc<Mutex<Daemon>>,
    mut event_rx: mpsc::UnboundedReceiver<Event>,
) -> Result<()> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();

    loop {
        tokio::select! {
            // Handle incoming requests
            result = reader.read_line(&mut line) => {
                match result {
                    Ok(0) => break, // Connection closed
                    Ok(_) => {
                        let trimmed = line.trim();
                        if !trimmed.is_empty() {
                            let response = {
                                let mut daemon_guard = daemon.lock().await;
                                match serde_json::from_str::<Request>(trimmed) {
                                    Ok(request) => daemon_guard.handle_request(request, &connection_id),
                                    Err(e) => {
                                        warn!("Invalid request: {}", e);
                                        Response::error(format!("Invalid request: {}", e))
                                    }
                                }
                            };

                            let response_json = serde_json::to_string(&response)?;
                            reader.get_mut().write_all(response_json.as_bytes()).await?;
                            reader.get_mut().write_all(b"\n").await?;
                        }
                        line.clear();
                    }
                    Err(e) => {
                        error!("Read error: {}", e);
                        break;
                    }
                }
            }
            // Handle outgoing events
            event = event_rx.recv() => {
                if let Some(event) = event {
                    let event_json = serde_json::to_string(&Message::Event(event))?;
                    if let Err(e) = reader.get_mut().write_all(event_json.as_bytes()).await {
                        warn!("Failed to send event: {}", e);
                        break;
                    }
                    if let Err(e) = reader.get_mut().write_all(b"\n").await {
                        warn!("Failed to send event newline: {}", e);
                        break;
                    }
                } else {
                    break; // Channel closed
                }
            }
        }
    }

    // Clean up connection
    {
        let mut daemon_guard = daemon.lock().await;
        daemon_guard.remove_connection(&connection_id);
    }

    Ok(())
}
