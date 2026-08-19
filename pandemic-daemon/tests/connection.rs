//! Tests for connection handling and lifecycle.

use pandemic_daemon::tests::test_harness::*;
use pandemic_protocol::{Request, Response};
use tokio::io::AsyncWriteExt;

#[tokio::test]
async fn plugin_removed_on_connection_drop() {
    let harness = TestHarness::with_persistent(true).await.unwrap();

    // Register a plugin via a persistent connection
    let mut conn = register_plugin(&harness, "drop-test", "1.0.0")
        .await
        .unwrap();

    // Verify it's registered
    let response = harness
        .send_request(&Request::GetPlugin {
            name: "drop-test".to_string(),
        })
        .await
        .unwrap();
    assert!(matches!(response, Response::Success { .. }));

    // Close the connection to trigger deregistration
    conn.close().await.unwrap();

    // Poll until plugin is deregistered (with timeout)
    let mut found = false;
    for _ in 0..20 {
        let response = harness
            .send_request(&Request::GetPlugin {
                name: "drop-test".to_string(),
            })
            .await
            .unwrap();
        if matches!(response, Response::NotFound { .. }) {
            found = true;
            break;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    }
    assert!(found, "Plugin should be removed after connection drops");

    harness.shutdown().await;
}

#[tokio::test]
async fn transient_client_does_not_register_plugin() {
    let harness = TestHarness::new().await.unwrap();

    // Send a simple request (transient connection)
    let _ = harness.send_request(&Request::ListPlugins).await.unwrap();

    // Send another transient request
    let _ = harness.send_request(&Request::GetHealth).await.unwrap();

    // No plugins should be registered
    let response = harness.send_request(&Request::ListPlugins).await.unwrap();
    match response {
        Response::Success { data } => {
            let plugins: Vec<pandemic_protocol::PluginInfo> =
                serde_json::from_value(data.unwrap()).unwrap();
            assert!(
                plugins.is_empty(),
                "Transient connections should not register plugins"
            );
        }
        _ => panic!("Expected success"),
    }

    harness.shutdown().await;
}

#[tokio::test]
async fn concurrent_requests_no_panic() {
    let harness = TestHarness::new().await.unwrap();

    // Spawn many concurrent persistent connections
    let mut conns = vec![];
    for i in 0..20 {
        let plugin = pandemic_protocol::PluginInfo {
            name: format!("concurrent-{}", i),
            version: "1.0.0".to_string(),
            description: None,
            config: None,
            registered_at: None,
        };
        let mut conn = harness.connect().await.unwrap();
        let response = conn
            .send_request(&Request::Register { plugin })
            .await
            .unwrap();
        assert!(matches!(response, Response::Success { .. }));
        conns.push(conn);
    }

    // Verify all are registered
    let response = harness.send_request(&Request::ListPlugins).await.unwrap();
    match response {
        Response::Success { data } => {
            let plugins: Vec<pandemic_protocol::PluginInfo> =
                serde_json::from_value(data.unwrap()).unwrap();
            assert_eq!(plugins.len(), 20);
        }
        _ => panic!("Expected success"),
    }

    // Drop all connections to deregister
    drop(conns);

    harness.shutdown().await;
}

#[tokio::test]
async fn connection_close_sends_deregister_event() {
    let harness = TestHarness::with_persistent(true).await.unwrap();

    // Subscribe to deregister events first (via a persistent connection)
    let mut subscriber = harness.connect().await.unwrap();
    let _ = subscriber
        .send_request(&Request::Subscribe {
            topics: vec!["plugin.deregistered".to_string()],
        })
        .await
        .unwrap();

    // Register a plugin
    let mut conn = register_plugin(&harness, "dereg-event", "1.0.0")
        .await
        .unwrap();

    // Drop the plugin connection — should trigger deregister event
    conn.close().await.unwrap();

    // Poll for the deregister event on the subscriber connection
    let mut got_event = false;
    for _ in 0..20 {
        if let Some(event) = subscriber.read_event(50).await.unwrap() {
            if event.topic == "plugin.deregistered" {
                got_event = true;
                break;
            }
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    }
    assert!(got_event, "Expected deregister event");

    harness.shutdown().await;
}

#[tokio::test]
async fn malformed_json_returns_error_not_panic() {
    let harness = TestHarness::new().await.unwrap();

    // Connect and send malformed JSON
    let stream = tokio::net::UnixStream::connect(&harness.socket_path)
        .await
        .unwrap();
    let mut writer = stream;
    let _ = writer.write_all(b"this is not json\n").await;
    drop(writer);

    // The daemon should handle this gracefully (return an error response)
    // We can't easily read the response in this test since the connection is dropped,
    // but we verify the daemon didn't panic by checking it still works
    let response = harness.send_request(&Request::ListPlugins).await.unwrap();
    assert!(matches!(response, Response::Success { .. }));

    harness.shutdown().await;
}

#[tokio::test]
async fn health_reflects_active_plugins() {
    let harness = TestHarness::new().await.unwrap();

    // Initially zero plugins
    let response = harness.send_request(&Request::GetHealth).await.unwrap();
    match response {
        Response::Success { data } => {
            let health: serde_json::Value = data.unwrap();
            assert_eq!(health["active_plugins"].as_u64().unwrap(), 0);
        }
        _ => panic!("Expected success"),
    }

    // Register 3 plugins, keeping connections alive
    let mut conns = vec![];
    for i in 0..3 {
        let plugin = pandemic_protocol::PluginInfo {
            name: format!("health-{}", i),
            version: "1.0.0".to_string(),
            description: None,
            config: None,
            registered_at: None,
        };
        let mut conn = harness.connect().await.unwrap();
        let resp = conn
            .send_request(&Request::Register { plugin })
            .await
            .unwrap();
        assert!(matches!(resp, Response::Success { .. }));
        conns.push(conn);
    }

    // Health should show 3
    let response = harness.send_request(&Request::GetHealth).await.unwrap();
    match response {
        Response::Success { data } => {
            let health: serde_json::Value = data.unwrap();
            assert_eq!(
                health["active_plugins"].as_u64().unwrap(),
                3,
                "Health should reflect registered plugin count"
            );
        }
        _ => panic!("Expected success"),
    }

    // Deregister one (via a transient request)
    let response = harness
        .send_request(&Request::Deregister {
            name: "health-1".to_string(),
        })
        .await
        .unwrap();
    assert!(matches!(response, Response::Success { .. }));

    // Health should show 2
    let response = harness.send_request(&Request::GetHealth).await.unwrap();
    match response {
        Response::Success { data } => {
            let health: serde_json::Value = data.unwrap();
            assert_eq!(
                health["active_plugins"].as_u64().unwrap(),
                2,
                "Health should reflect deregistered plugin count"
            );
        }
        _ => panic!("Expected success"),
    }

    // Drop remaining connections
    drop(conns);

    harness.shutdown().await;
}
