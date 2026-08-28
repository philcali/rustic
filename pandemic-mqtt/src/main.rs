use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "pandemic-mqtt")]
#[command(about = "Bridge pandemic daemon events out to an MQTT broker (one-way: daemon → MQTT)")]
struct Args {
    /// Path to the pandemic daemon's Unix socket
    #[arg(long, default_value = "/var/run/pandemic/pandemic.sock")]
    socket_path: PathBuf,

    /// MQTT broker URL, e.g. `mqtt://127.0.0.1:1883` (scheme and port optional)
    #[arg(long, default_value = "mqtt://127.0.0.1:1883")]
    broker_url: String,

    /// MQTT username (wins over userinfo in `--broker-url`)
    #[arg(long)]
    username: Option<String>,

    /// MQTT password (wins over userinfo in `--broker-url`)
    #[arg(long)]
    password: Option<String>,

    /// MQTT client identifier
    #[arg(long, default_value_t = default_client_id())]
    client_id: String,

    /// Topic prefix for all published topics (`""` passes daemon topics through)
    #[arg(long, default_value = "pandemic")]
    topic_prefix: String,

    /// MQTT QoS for bridged events (0 = at most once, 1 = at least once, 2 = exactly once)
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u8).range(0..=2))]
    qos: u8,
}

fn default_client_id() -> String {
    format!("pandemic-mqtt-{}", std::process::id())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    let (host, port, username, password) =
        pandemic_mqtt::parse_broker(&args.broker_url, args.username, args.password)?;

    let config = pandemic_mqtt::BridgeConfig {
        socket_path: args.socket_path,
        broker_host: host,
        broker_port: port,
        username,
        password,
        client_id: args.client_id,
        topic_prefix: args.topic_prefix,
        qos: pandemic_mqtt::qos_from_u8(args.qos),
    };

    pandemic_mqtt::run(config).await
}
