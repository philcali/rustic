//! Integration tests: the bridge against a real (in-process) daemon and a
//! minimal MQTT 3.1.1 stub broker that captures published messages.

use pandemic_daemon::tests::test_harness::{register_plugin, TestHarness};
use pandemic_mqtt::BridgeConfig;
use pandemic_protocol::{Request, Response};
use rumqttc::QoS;
use serde_json::json;
use socket2::{Domain, SockAddr, Socket, Type};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

// --- Minimal MQTT 3.1.1 stub broker ----------------------------------------

#[derive(Debug, Clone, PartialEq)]
struct Captured {
    topic: String,
    payload: Vec<u8>,
    qos: u8,
    retain: bool,
}

struct StubBroker {
    thread: Option<JoinHandle<()>>,
    running: Arc<AtomicBool>,
    captures: Arc<Mutex<Vec<Captured>>>,
    /// Live client connections, severed on `stop()` so clients notice.
    connections: Arc<Mutex<Vec<Arc<TcpStream>>>>,
}

impl StubBroker {
    fn start() -> (Self, SocketAddr) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub broker");
        let addr = listener.local_addr().unwrap();
        (Self::spawn(listener), addr)
    }

    /// Rebind the same address (with SO_REUSEADDR to dodge TIME_WAIT).
    fn restart_on(&mut self, addr: SocketAddr) {
        self.stop();
        let socket = Socket::new(Domain::IPV4, Type::STREAM, None).expect("v4 socket");
        socket.set_reuse_address(true).expect("SO_REUSEADDR");
        socket
            .bind(&SockAddr::from(addr))
            .expect("rebind stub broker address");
        socket.listen(16).expect("listen stub broker");
        let listener: TcpListener = socket.into();
        *self = Self::spawn(listener);
    }

    fn spawn(listener: TcpListener) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let captures = Arc::new(Mutex::new(Vec::new()));
        let connections = Arc::new(Mutex::new(Vec::new()));
        let thread = std::thread::Builder::new()
            .name("stub-broker".into())
            .spawn({
                let listener = Arc::new(listener);
                let captures = captures.clone();
                let running = running.clone();
                let connections = connections.clone();
                move || accept_loop(listener, captures, running, connections)
            })
            .expect("spawn stub broker thread");
        Self {
            thread: Some(thread),
            running,
            captures,
            connections,
        }
    }

    fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Ok(mut connections) = self.connections.lock() {
            for connection in connections.iter() {
                let _ = connection.shutdown(std::net::Shutdown::Both);
            }
            connections.clear();
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    /// Payload of the most recent capture on `topic`, if any.
    fn latest(&self, topic: &str) -> Option<String> {
        let captures = self.captures.lock().unwrap();
        captures
            .iter()
            .rev()
            .find(|c| c.topic == topic)
            .map(|c| String::from_utf8_lossy(&c.payload).into_owned())
    }

    /// Whether a retained publish has been seen on `topic` (latest match).
    fn latest_retained(&self, topic: &str) -> Option<bool> {
        let captures = self.captures.lock().unwrap();
        captures
            .iter()
            .rev()
            .find(|c| c.topic == topic)
            .map(|c| c.retain)
    }
}

impl Drop for StubBroker {
    fn drop(&mut self) {
        self.stop();
    }
}

fn accept_loop(
    listener: Arc<TcpListener>,
    captures: Arc<Mutex<Vec<Captured>>>,
    running: Arc<AtomicBool>,
    connections: Arc<Mutex<Vec<Arc<TcpStream>>>>,
) {
    let _ = listener.set_nonblocking(true);
    while running.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                let _ = stream.set_nonblocking(false);
                let stream = Arc::new(stream);
                connections.lock().unwrap().push(stream.clone());
                let captures = captures.clone();
                std::thread::Builder::new()
                    .name("stub-broker-conn".into())
                    .spawn(move || serve_connection(stream, captures))
                    .expect("spawn connection thread");
            }
            Err(ref err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(ref err) if err.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break, // listener closed
        }
    }
}

fn serve_connection(stream: Arc<TcpStream>, captures: Arc<Mutex<Vec<Captured>>>) {
    // std implements Read/Write for &TcpStream, so a shared reference suffices.
    let mut stream = &*stream;
    let mut header = [0u8; 1];
    let mut body = Vec::new();

    loop {
        body.clear();

        if stream.read_exact(&mut header).is_err() {
            break;
        }
        let byte = header[0];
        let command = byte & 0xF0;

        // Remaining-length varint.
        let mut length: usize = 0;
        let mut multiplier: u32 = 1;
        loop {
            let digit = [0u8; 1];
            let mut digit = digit;
            if stream.read_exact(&mut digit).is_err() {
                return;
            }
            length += (digit[0] as usize & 0x7F) * (multiplier as usize);
            if digit[0] & 0x80 == 0 {
                break;
            }
            multiplier *= 128;
            if multiplier > 128 * 128 * 128 * 128 {
                return;
            }
        }
        if length > 0 {
            body.resize(length, 0);
            if stream.read_exact(&mut body).is_err() {
                break;
            }
        }

        match command {
            0x10 => {
                // CONNECT → CONNACK (session not present, success).
                let _ = stream.write_all(&[0x20, 0x02, 0x00, 0x00]);
            }
            0x30 => {
                // PUBLISH
                let qos = (byte >> 1) & 0x03;
                let retain = byte & 0x01 == 1;
                if body.len() < 2 {
                    break;
                }
                let topic_len = usize::from(u16::from_be_bytes([body[0], body[1]]));
                if body.len() < 2 + topic_len {
                    break;
                }
                let topic = String::from_utf8_lossy(&body[2..2 + topic_len]).into_owned();
                let mut rest = &body[2 + topic_len..];
                let mut packet_id: [u8; 2] = [0; 2];
                if qos > 0 {
                    if rest.len() < 2 {
                        break;
                    }
                    packet_id = [rest[0], rest[1]];
                    rest = &rest[2..];
                }
                captures.lock().unwrap().push(Captured {
                    topic,
                    payload: rest.to_vec(),
                    qos,
                    retain,
                });
                if qos == 1 {
                    let _ = stream.write_all(&[0x40, 0x02, packet_id[0], packet_id[1]]);
                } else if qos == 2 {
                    let _ = stream.write_all(&[0x50, 0x02, packet_id[0], packet_id[1]]);
                    // PUBREC
                }
            }
            0x52 => {
                // PUBREL → PUBCOMP.
                let packet_id = [body[0], body[1]];
                let _ = stream.write_all(&[0x70, 0x02, packet_id[0], packet_id[1]]);
            }
            0x82 => {
                // SUBSCRIBE → SUBACK (grant QoS 0).
                let packet_id = [body[0], body[1]];
                let _ = stream.write_all(&[0x90, 0x03, packet_id[0], packet_id[1], 0x00]);
            }
            0xC0 => {
                // PINGREQ → PINGRESP.
                let _ = stream.write_all(&[0xD0, 0x00]);
            }
            0xE0 => break, // DISCONNECT
            _ => {}
        }
    }
}

// --- Helpers ----------------------------------------------------------------

async fn eventually(mut check: impl FnMut() -> bool, timeout_ms: u64) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        if check() {
            return true;
        }
        if std::time::Instant::now() > deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn test_config(harness: &TestHarness, addr: SocketAddr, client_id: &str) -> BridgeConfig {
    BridgeConfig {
        socket_path: harness.socket_path.clone(),
        broker_host: addr.ip().to_string(),
        broker_port: addr.port(),
        username: None,
        password: None,
        client_id: client_id.to_string(),
        topic_prefix: "pandemic".to_string(),
        qos: QoS::AtLeastOnce,
    }
}

// --- Tests ------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bridge_forwards_events_and_publishes_snapshots() {
    let (mut broker, addr) = StubBroker::start();
    let harness = TestHarness::with_persistent(true).await.unwrap();

    let config = test_config(&harness, addr, "test-bridge-1");
    let bridge = tokio::spawn(async move { pandemic_mqtt::run(config).await });

    // Retained world-state snapshots appear on (first) connect.
    assert!(
        eventually(
            || broker.latest_retained("pandemic/status") == Some(true)
                && broker
                    .latest("pandemic/status")
                    .is_some_and(|p| p.contains("\"online\"")),
            10_000,
        )
        .await,
        "status snapshot not published"
    );
    assert!(
        eventually(
            || broker.latest_retained("pandemic/plugins") == Some(true)
                && broker
                    .latest("pandemic/plugins")
                    .is_some_and(|p| p.contains("pandemic-mqtt")),
            10_000,
        )
        .await,
        "plugins snapshot not published"
    );
    assert!(
        broker.latest_retained("pandemic/health").is_some(),
        "health snapshot not published"
    );

    // A second plugin registers: the event is bridged, the snapshot refreshes.
    let mut other = register_plugin(&harness, "other-plugin", "0.1.0")
        .await
        .unwrap();
    assert!(
        eventually(
            || broker
                .latest("pandemic/plugin.registered")
                .is_some_and(|p| p.contains("\"other-plugin\"")),
            10_000,
        )
        .await,
        "plugin.registered not bridged"
    );
    assert!(
        eventually(
            || broker
                .latest("pandemic/plugins")
                .is_some_and(|p| p.contains("other-plugin")),
            10_000,
        )
        .await,
        "plugins snapshot not refreshed"
    );

    // A user-published topic is bridged under the prefix.
    let response = harness
        .send_request(&Request::Publish {
            topic: "alerts".to_string(),
            data: json!({ "level": "critical" }),
        })
        .await
        .unwrap();
    assert!(matches!(response, Response::Success { .. }));
    assert!(
        eventually(
            || broker
                .latest("pandemic/alerts")
                .is_some_and(|p| p.contains("\"critical\"")),
            10_000,
        )
        .await,
        "user-published event not bridged"
    );

    // Deregistration is bridged too.
    other.close().await.unwrap();
    assert!(
        eventually(
            || broker
                .latest("pandemic/plugin.deregistered")
                .is_some_and(|p| p.contains("\"other-plugin\"")),
            10_000,
        )
        .await,
        "plugin.deregistered not bridged"
    );

    bridge.abort();
    harness.shutdown().await;
    broker.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bridge_reconnects_when_broker_restarts() {
    let (mut broker, addr) = StubBroker::start();
    let harness = TestHarness::with_persistent(true).await.unwrap();

    let config = test_config(&harness, addr, "test-bridge-2");
    let bridge = tokio::spawn(async move { pandemic_mqtt::run(config).await });

    // Established on the first broker.
    assert!(
        eventually(
            || broker
                .latest("pandemic/status")
                .is_some_and(|p| p.contains("\"online\"")),
            10_000,
        )
        .await,
        "status snapshot not published to first broker"
    );

    // Broker goes away and comes back on the same address.
    broker.restart_on(addr);

    // The bridge reconnects and re-publishes its retained state.
    assert!(
        eventually(
            || broker.latest_retained("pandemic/status") == Some(true)
                && broker
                    .latest("pandemic/status")
                    .is_some_and(|p| p.contains("\"online\"")),
            15_000,
        )
        .await,
        "status snapshot not re-published after broker restart"
    );

    // ...and keeps forwarding live events to the new broker.
    let mut late = register_plugin(&harness, "late-plugin", "0.1.0")
        .await
        .unwrap();
    assert!(
        eventually(
            || broker
                .latest("pandemic/plugin.registered")
                .is_some_and(|p| p.contains("\"late-plugin\"")),
            10_000,
        )
        .await,
        "event not bridged after broker restart"
    );

    late.close().await.unwrap();
    bridge.abort();
    harness.shutdown().await;
    broker.stop();
}
