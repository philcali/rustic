//! Edge cases and error handling.

use pandemic_daemon::tests::test_harness::*;
use pandemic_protocol::{PluginInfo, Request, Response};
use tokio::io::AsyncWriteExt;

#[tokio::test]
async fn empty_line_ignored() {
    let harness = TestHarness::new().await.unwrap();

    // Send empty lines followed by a valid request
    let stream = tokio::net::UnixStream::connect(&harness.socket_path)
        .await
        .unwrap();
    let mut writer = tokio::io::BufWriter::new(stream);
    let _ = writer.write_all(b"\n\n\n").await;
    let _ = writer.write_all(b"{\"type\": \"ListPlugins\"}\n").await;
    drop(writer);

    // Daemon should still be functional
    let response = harness.send_request(&Request::GetHealth).await.unwrap();
    assert!(matches!(response, Response::Success { .. }));

    harness.shutdown().await;
}

#[tokio::test]
async fn large_payload_handled() {
    let harness = TestHarness::new().await.unwrap();

    // Register a plugin
    let plugin = pandemic_protocol::PluginInfo {
        name: "large-payload".to_string(),
        version: "1.0.0".to_string(),
        description: Some("A".repeat(10000)),
        config: None,
        registered_at: None,
    };
    let _ = harness
        .send_request(&Request::Register { plugin })
        .await
        .unwrap();

    // Publish a large event
    let large_data = serde_json::json!({
        "data": "A".repeat(50000),
    });
    let _ = harness
        .send_request(&Request::Publish {
            topic: "large.topic".to_string(),
            data: large_data,
        })
        .await
        .unwrap();

    // Should still work
    let response = harness.send_request(&Request::GetHealth).await.unwrap();
    assert!(matches!(response, Response::Success { .. }));

    harness.shutdown().await;
}

#[tokio::test]
async fn rapid_register_deregister() {
    let harness = TestHarness::new().await.unwrap();

    // Rapidly register and deregister the same plugin
    for i in 0..10 {
        let name = format!("rapid-{}", i % 3);

        let plugin = pandemic_protocol::PluginInfo {
            name: name.clone(),
            version: "1.0.0".to_string(),
            description: None,
            config: None,
            registered_at: None,
        };
        let _ = harness
            .send_request(&Request::Register { plugin })
            .await
            .unwrap();

        let dereg_name = name.clone();
        let _ = harness
            .send_request(&Request::Deregister { name: dereg_name })
            .await
            .unwrap();

        // Verify it's gone
        let response = harness
            .send_request(&Request::GetPlugin { name: name.clone() })
            .await
            .unwrap();
        assert!(
            matches!(response, Response::NotFound { .. }),
            "Plugin {} should be deregistered at iteration {}",
            name,
            i
        );
    }

    // No plugins should remain
    let response = harness.send_request(&Request::ListPlugins).await.unwrap();
    match response {
        Response::Success { data } => {
            let plugins: Vec<PluginInfo> = serde_json::from_value(data.unwrap()).unwrap();
            assert!(
                plugins.is_empty(),
                "No plugins should remain after rapid deregister"
            );
        }
        _ => panic!("Expected success"),
    }

    harness.shutdown().await;
}

#[tokio::test]
async fn health_metrics_fields_present() {
    let harness = TestHarness::new().await.unwrap();

    let response = harness.send_request(&Request::GetHealth).await.unwrap();
    match response {
        Response::Success { data } => {
            let health: serde_json::Value = data.unwrap();

            // All expected fields should be present
            assert!(
                health["active_plugins"].is_number(),
                "active_plugins should be a number"
            );
            assert!(
                health["total_connections"].is_number(),
                "total_connections should be a number"
            );
            assert!(
                health["event_bus_subscribers"].is_number(),
                "event_bus_subscribers should be a number"
            );
            assert!(
                health["uptime_seconds"].is_number(),
                "uptime_seconds should be a number"
            );
            assert!(
                health["memory_used_mb"].is_number(),
                "memory_used_mb should be a number"
            );
            assert!(
                health["memory_total_mb"].is_number(),
                "memory_total_mb should be a number"
            );
            assert!(
                health["cpu_usage_percent"].is_number(),
                "cpu_usage_percent should be a number"
            );
            assert!(
                health["load_average"].is_null() || health["load_average"].is_number(),
                "load_average should be null or a number"
            );
        }
        _ => panic!("Expected success"),
    }

    harness.shutdown().await;
}

#[tokio::test]
async fn connection_after_shutdown_fails() {
    let harness = TestHarness::new().await.unwrap();

    // Save socket path before shutdown consumes the harness
    let socket_path = harness.socket_path.clone();

    // Shutdown the harness
    harness.shutdown().await;

    // Give the accept loop time to fully stop
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // The daemon loop has stopped, so new connections should fail
    let result = tokio::net::UnixStream::connect(&socket_path).await;

    // Connection should fail (daemon stopped accepting)
    assert!(result.is_err(), "Connection to stopped daemon should fail");
}
