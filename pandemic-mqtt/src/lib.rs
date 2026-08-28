//! `pandemic-mqtt` — a one-way bridge from the pandemic daemon to an MQTT broker.
//!
//! This infection registers with the daemon, subscribes to every event on the
//! daemon's event bus, and republishes each event as an MQTT message. Outside
//! subscribers (IoT agents, dashboards, alerting) can then react to daemon
//! activity without speaking the daemon's Unix-socket protocol.
//!
//! The bridge is intentionally one-way (daemon → MQTT): the daemon stays
//! authoritative. Application-specific ingress behavior belongs to an MQTT
//! client attached via `pandemic-proxy` or an external service.
//!
//! Behavior:
//! * Events are published to `{prefix}/{daemon-topic}` (prefix configurable,
//!   `""` passes daemon topics through unchanged).
//! * On (re)connect to the broker, retained snapshots are (re)published to
//!   `{prefix}/status`, `{prefix}/health` and `{prefix}/plugins` so late
//!   subscribers immediately see the current world state.
//! * A last-will (retained `{prefix}/status = offline`) is registered so a
//!   crash is visible to MQTT subscribers; a graceful shutdown clears the
//!   retained state instead.
//! * No unbounded buffering: the MQTT outbox is capped (256 messages). When
//!   it is full — broker down and events piling up — events are dropped with
//!   a warning.

use anyhow::{bail, Context, Result};
use pandemic_common::{DaemonClient, PersistentClient};
use pandemic_protocol::{Event as DaemonEvent, PluginInfo, Request, Response};
use rumqttc::{
    AsyncClient, ConnectReturnCode, ConnectionError, Event as MqttEvent, EventLoop, Incoming,
    LastWill, MqttOptions, Outgoing, QoS,
};
use serde_json::json;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::signal::unix::{signal, SignalKind};
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

/// Plugin name this infection registers under.
pub const BRIDGE_NAME: &str = "pandemic-mqtt";

/// Bound on MQTT messages in flight while the broker is unreachable.
/// Older events beyond this cap are dropped (with a warning) rather than
/// buffering an unbounded stale backlog.
pub const MQTT_OUTBOX: usize = 256;

/// Connection settings for the bridge.
#[derive(Debug, Clone)]
pub struct BridgeConfig {
    /// Path to the pandemic daemon's Unix socket.
    pub socket_path: PathBuf,
    pub broker_host: String,
    pub broker_port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    pub client_id: String,
    /// Topic prefix prepended to every published topic (`""` = passthrough).
    pub topic_prefix: String,
    pub qos: QoS,
}

/// Parse an MQTT broker URL (`mqtt://[user[:pass]@]host[:port]`, scheme and
/// port optional) into host/port/credentials. Explicit username/password win
/// over userinfo embedded in the URL.
pub fn parse_broker(
    broker_url: &str,
    username: Option<String>,
    password: Option<String>,
) -> Result<(String, u16, Option<String>, Option<String>)> {
    let with_scheme = if broker_url.contains("://") {
        broker_url.to_string()
    } else {
        format!("mqtt://{broker_url}")
    };
    let parsed = url::Url::parse(&with_scheme)
        .with_context(|| format!("invalid broker URL: {broker_url}"))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("broker URL must include a host: {broker_url}"))?
        .to_string();
    let port = parsed.port().unwrap_or(1883);
    let username = username.or_else(|| {
        if parsed.username().is_empty() {
            None
        } else {
            Some(parsed.username().to_string())
        }
    });
    let password = password.or_else(|| parsed.password().map(str::to_string));
    Ok((host, port, username, password))
}

/// Map a daemon event topic to its MQTT topic, sanitized so it is a valid
/// MQTT topic name (no whitespace, `+`, or `#`).
pub fn mqtt_topic(prefix: &str, event_topic: &str) -> String {
    let sanitize = |seg: &str| {
        seg.chars()
            .map(|c| {
                if c.is_whitespace() || c == '+' || c == '#' {
                    '_'
                } else {
                    c
                }
            })
            .collect::<String>()
    };
    let prefix = sanitize(prefix);
    let event = sanitize(event_topic);
    match (prefix.is_empty(), event.is_empty()) {
        (true, true) => String::new(),
        (true, false) => event,
        (false, true) => prefix,
        (false, false) => format!("{prefix}/{event}"),
    }
}

/// MQTT topic for the bridge's retained status marker.
pub fn status_topic(prefix: &str) -> String {
    mqtt_topic(prefix, "status")
}

/// MQTT topic for the retained daemon health snapshot.
pub fn health_topic(prefix: &str) -> String {
    mqtt_topic(prefix, "health")
}

/// MQTT topic for the retained plugin-list snapshot.
pub fn plugins_topic(prefix: &str) -> String {
    mqtt_topic(prefix, "plugins")
}

/// Convert a numeric QoS (0–2) into a rumqttc [`QoS`].
pub fn qos_from_u8(value: u8) -> QoS {
    match value {
        0 => QoS::AtMostOnce,
        2 => QoS::ExactlyOnce,
        _ => QoS::AtLeastOnce,
    }
}

fn mqtt_options(cfg: &BridgeConfig) -> MqttOptions {
    let mut options = MqttOptions::new(&cfg.client_id, &cfg.broker_host, cfg.broker_port);
    options.set_keep_alive(Duration::from_secs(30));
    if let (Some(username), Some(password)) = (&cfg.username, &cfg.password) {
        options.set_credentials(username, password);
    }
    // Crash-safe presence: if we die without a clean DISCONNECT, the broker
    // publishes this retained marker. A graceful shutdown clears it.
    let offline = json!({ "status": "offline", "bridge": BRIDGE_NAME });
    options.set_last_will(LastWill::new(
        status_topic(&cfg.topic_prefix),
        offline.to_string(),
        QoS::AtLeastOnce,
        true,
    ));
    options
}

/// Connect to the daemon, register as the `pandemic-mqtt` plugin, and
/// subscribe to every topic. Retries with backoff so the bridge can start
/// before the daemon.
async fn establish_daemon(cfg: &BridgeConfig) -> Result<PersistentClient> {
    let mut backoff = Duration::from_millis(250);
    loop {
        match connect_and_register(cfg).await {
            Ok(client) => return Ok(client),
            Err(err) => {
                warn!(
                    error = %err,
                    socket = %cfg.socket_path.display(),
                    "daemon unavailable, retrying"
                );
                sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
        }
    }
}

async fn connect_and_register(cfg: &BridgeConfig) -> Result<PersistentClient> {
    let mut client = DaemonClient::connect(&cfg.socket_path)
        .await
        .with_context(|| format!("connecting to daemon at {}", cfg.socket_path.display()))?;

    let plugin = PluginInfo {
        name: BRIDGE_NAME.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        description: Some("Bridges daemon events to an MQTT broker".to_string()),
        config: None,
        registered_at: None,
    };
    match client.send_request(&Request::Register { plugin }).await? {
        Response::Success { .. } => {}
        response => bail!("daemon rejected registration: {response:?}"),
    }
    client
        .subscribe(vec!["*".to_string()])
        .await
        .context("subscribing to all daemon topics")?;
    Ok(client)
}

/// Forward one daemon event to the MQTT broker.
fn forward_event(cfg: &BridgeConfig, client: &AsyncClient, event: &DaemonEvent) -> Result<()> {
    let topic = mqtt_topic(&cfg.topic_prefix, &event.topic);
    let payload = serde_json::to_string(event)?;
    client
        .try_publish(topic, cfg.qos, false, payload)
        .map_err(|err| anyhow::anyhow!("MQTT outbox full or client closed: {err}"))
}

/// Publish a retained snapshot to `topic` (best effort).
fn publish_retained(
    cfg: &BridgeConfig,
    client: &AsyncClient,
    topic: &str,
    value: serde_json::Value,
) {
    let payload = value.to_string();
    match client.try_publish(topic, cfg.qos, true, payload) {
        Ok(()) => debug!(topic, "published retained snapshot"),
        Err(err) => warn!(topic, error = %err, "failed to publish retained snapshot"),
    }
}

/// (Re)publish the retained world-state snapshots: status, daemon health, and
/// the current plugin list.
async fn publish_snapshots(
    cfg: &BridgeConfig,
    client: &AsyncClient,
    daemon: &mut PersistentClient,
) {
    publish_retained(
        cfg,
        client,
        &status_topic(&cfg.topic_prefix),
        json!({ "status": "online", "bridge": BRIDGE_NAME }),
    );
    if let Ok(Response::Success { data: Some(health) }) =
        daemon.send_request(&Request::GetHealth).await
    {
        publish_retained(cfg, client, &health_topic(&cfg.topic_prefix), health);
    } else {
        warn!("could not refresh retained health snapshot");
    }
    if let Ok(Response::Success {
        data: Some(plugins),
    }) = daemon.send_request(&Request::ListPlugins).await
    {
        publish_retained(cfg, client, &plugins_topic(&cfg.topic_prefix), plugins);
    } else {
        warn!("could not refresh retained plugins snapshot");
    }
}

/// Delete the retained state on a clean exit (retained + empty payload).
/// Returns the number of clear publishes successfully enqueued.
fn clear_retained(cfg: &BridgeConfig, client: &AsyncClient) -> usize {
    let mut enqueued = 0;
    for topic in [
        status_topic(&cfg.topic_prefix),
        health_topic(&cfg.topic_prefix),
        plugins_topic(&cfg.topic_prefix),
    ] {
        match client.try_publish(topic.as_str(), cfg.qos, true, "") {
            Ok(()) => enqueued += 1,
            Err(err) => debug!(topic, error = %err, "failed to clear retained state"),
        }
    }
    enqueued
}

/// Poll the event loop until `count` outbox deliveries have been confirmed
/// for `qos` (QoS 0: sent, QoS 1: PUBACK, QoS 2: PUBCOMP) or `deadline`
/// passes. Best effort — if the broker is unreachable the outbox cannot
/// drain and we proceed with the shutdown.
async fn flush_publishes(eventloop: &mut EventLoop, qos: QoS, count: usize, deadline: Duration) {
    let confirmed = |outgoing: &Outgoing| match qos {
        QoS::AtMostOnce => matches!(outgoing, Outgoing::Publish(0)),
        QoS::AtLeastOnce => matches!(outgoing, Outgoing::PubAck(_)),
        QoS::ExactlyOnce => matches!(outgoing, Outgoing::PubComp(_)),
    };
    let mut acked = 0;
    let end = Instant::now() + deadline;
    while acked < count && Instant::now() < end {
        match tokio::time::timeout(Duration::from_millis(100), eventloop.poll()).await {
            Ok(Ok(MqttEvent::Outgoing(outgoing))) if confirmed(&outgoing) => acked += 1,
            Ok(Ok(_)) => {}
            Ok(Err(err)) => {
                warn!(error = %err, "MQTT error while flushing outbox");
                break;
            }
            Err(_) => {}
        }
    }
    if acked < count {
        warn!(
            acked,
            wanted = count,
            "MQTT outbox not fully flushed before shutdown"
        );
    }
}

/// Wait until a clean DISCONNECT has been sent (so the broker suppresses the
/// last-will), bounded by `deadline`.
async fn flush_disconnect(eventloop: &mut EventLoop, deadline: Duration) {
    let end = Instant::now() + deadline;
    while Instant::now() < end {
        match tokio::time::timeout(Duration::from_millis(100), eventloop.poll()).await {
            Ok(Ok(MqttEvent::Outgoing(Outgoing::Disconnect))) => return,
            Ok(Ok(_)) => {}
            Ok(Err(err)) => {
                warn!(error = %err, "MQTT error while sending DISCONNECT");
                return;
            }
            Err(_) => {}
        }
    }
    warn!("DISCONNECT not confirmed before shutdown");
}

async fn shutdown_signal() {
    let mut terminate = signal(SignalKind::terminate()).expect("installing SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
}

/// Run the bridge until SIGINT/SIGTERM.
pub async fn run(cfg: BridgeConfig) -> Result<()> {
    let (client, mut eventloop) = AsyncClient::new(mqtt_options(&cfg), MQTT_OUTBOX);
    info!(
        broker = %format!("{}:{}", cfg.broker_host, cfg.broker_port),
        prefix = %cfg.topic_prefix,
        client_id = %cfg.client_id,
        "starting pandemic-mqtt bridge"
    );

    let mut daemon = establish_daemon(&cfg).await?;
    info!("connected to daemon, subscribing to all topics");

    let mut broker_up = false;

    loop {
        tokio::select! {
            event = daemon.read_event() => match event {
                Ok(Some(event)) => {
                    if let Err(err) = forward_event(&cfg, &client, &event) {
                        warn!(
                            topic = %event.topic,
                            error = %err,
                            "dropping daemon event (MQTT outbox full or broker unreachable)"
                        );
                        broker_up = false;
                    }
                    // Keep the retained plugin list fresh.
                    if broker_up
                        && matches!(event.topic.as_str(), "plugin.registered" | "plugin.deregistered")
                    {
                        publish_snapshots(&cfg, &client, &mut daemon).await;
                    }
                }
                Ok(None) => {
                    warn!("daemon connection closed, re-establishing");
                    broker_up = false;
                    daemon = establish_daemon(&cfg).await?;
                    info!("reconnected to daemon");
                }
                Err(err) => {
                    error!(error = %err, "daemon read error, re-establishing");
                    broker_up = false;
                    daemon = establish_daemon(&cfg).await?;
                    info!("reconnected to daemon");
                }
            },
            mqtt_event = eventloop.poll() => match mqtt_event {
                Ok(MqttEvent::Incoming(Incoming::ConnAck(_))) => {
                    if !broker_up {
                        info!("connected to MQTT broker");
                    }
                    broker_up = true;
                    publish_snapshots(&cfg, &client, &mut daemon).await;
                }
                Ok(MqttEvent::Incoming(incoming)) => {
                    debug!(?incoming, "mqtt packet in");
                }
                Ok(MqttEvent::Outgoing(outgoing)) => {
                    debug!(?outgoing, "mqtt packet out");
                }
                Err(err) => {
                    if let ConnectionError::ConnectionRefused(code) = &err {
                        if matches!(
                            code,
                            ConnectReturnCode::RefusedProtocolVersion
                                | ConnectReturnCode::BadClientId
                                | ConnectReturnCode::BadUserNamePassword
                                | ConnectReturnCode::NotAuthorized
                        ) {
                            // Reconnecting cannot fix an auth/protocol rejection.
                            bail!("broker rejected the connection: {code:?}");
                        }
                    }
                    warn!(error = %err, "MQTT connection lost, reconnecting");
                    broker_up = false;
                    // Avoid a tight reconnect spin while the broker is down.
                    sleep(Duration::from_millis(200)).await;
                }
            },
            _ = shutdown_signal() => {
                info!("shutting down");
                // Flush the outbox so the clears actually reach the broker
                // before we drop the event loop, then send a clean
                // DISCONNECT so the last-will is not published.
                let cleared = clear_retained(&cfg, &client);
                if cleared > 0 {
                    flush_publishes(&mut eventloop, cfg.qos, cleared, Duration::from_secs(2))
                        .await;
                }
                if client.try_disconnect().is_ok() {
                    flush_disconnect(&mut eventloop, Duration::from_secs(1)).await;
                }
                break;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_is_prefixed() {
        assert_eq!(
            mqtt_topic("pandemic", "plugin.registered"),
            "pandemic/plugin.registered"
        );
    }

    #[test]
    fn empty_prefix_is_passthrough() {
        assert_eq!(mqtt_topic("", "plugin.registered"), "plugin.registered");
    }

    #[test]
    fn topic_characters_are_sanitized() {
        assert_eq!(
            mqtt_topic("pandemic", "my topic with + and #"),
            "pandemic/my_topic_with___and__"
        );
    }

    #[test]
    fn snapshot_topics_use_the_prefix() {
        assert_eq!(status_topic("pandemic"), "pandemic/status");
        assert_eq!(health_topic("pandemic"), "pandemic/health");
        assert_eq!(plugins_topic("pandemic"), "pandemic/plugins");
        assert_eq!(status_topic(""), "status");
    }

    #[test]
    fn qos_conversion() {
        assert_eq!(qos_from_u8(0), QoS::AtMostOnce);
        assert_eq!(qos_from_u8(1), QoS::AtLeastOnce);
        assert_eq!(qos_from_u8(2), QoS::ExactlyOnce);
    }

    #[test]
    fn broker_url_with_defaults() {
        let (host, port, user, pass) = parse_broker("127.0.0.1", None, None).unwrap();
        assert_eq!((host.as_str(), port), ("127.0.0.1", 1883));
        assert_eq!((user, pass), (None, None));
    }

    #[test]
    fn broker_url_with_scheme_port_and_credentials() {
        let (host, port, user, pass) =
            parse_broker("mqtt://broker.example:1884", None, None).unwrap();
        assert_eq!((host.as_str(), port), ("broker.example", 1884));
        assert_eq!((user, pass), (None, None));

        let (host, port, user, pass) =
            parse_broker("mqtt://alice:secret@broker.example", None, None).unwrap();
        assert_eq!(host, "broker.example");
        assert_eq!(
            (port, user.as_deref(), pass.as_deref()),
            (1883, Some("alice"), Some("secret"))
        );

        // Explicit credentials win over URL userinfo.
        let (_, _, user, pass) = parse_broker(
            "mqtt://alice:secret@broker.example",
            Some("bob".into()),
            Some("pw".into()),
        )
        .unwrap();
        assert_eq!(
            (user.as_deref(), pass.as_deref()),
            (Some("bob"), Some("pw"))
        );
    }

    #[test]
    fn broker_url_rejects_garbage() {
        assert!(parse_broker("://", None, None).is_err());
        assert!(parse_broker("http://", None, None).is_err());
    }
}
