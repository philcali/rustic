//! Tests for the event bus pub/sub system.

use pandemic_daemon::tests::test_harness::*;
use pandemic_protocol::{Request, Response};

#[tokio::test]
async fn subscribe_and_receive_event() {
    let harness = TestHarness::new().await.unwrap();

    // Register a plugin
    let mut conn = register_plugin(&harness, "subscriber", "1.0.0")
        .await
        .unwrap();

    // Subscribe to a topic
    let _ = conn
        .send_request(&Request::Subscribe {
            topics: vec!["test.topic".to_string()],
        })
        .await
        .unwrap();

    // Publish an event
    let event_data = serde_json::json!({"message": "hello"});
    let _ = harness
        .send_request(&Request::Publish {
            topic: "test.topic".to_string(),
            data: event_data.clone(),
        })
        .await
        .unwrap();

    // Receive the event from the connection's stream
    let event = tokio::time::timeout(
        tokio::time::Duration::from_millis(500),
        conn.read_event(500),
    )
    .await
    .expect("Timed out waiting for event")
    .unwrap()
    .expect("Expected an event");

    assert_eq!(event.topic, "test.topic");
    // Source is "unknown" because the publisher is a transient connection
    assert_eq!(event.source, "unknown");
    assert_eq!(event.data, event_data);

    harness.shutdown().await;
}

#[tokio::test]
async fn wildcard_topic_matching() {
    let harness = TestHarness::new().await.unwrap();

    // Register a plugin
    let mut conn = register_plugin(&harness, "wildcard-sub", "1.0.0")
        .await
        .unwrap();

    // Subscribe with wildcard
    let _ = conn
        .send_request(&Request::Subscribe {
            topics: vec!["plugin.*".to_string()],
        })
        .await
        .unwrap();

    // Publish to a matching topic
    let _ = harness
        .send_request(&Request::Publish {
            topic: "plugin.registered".to_string(),
            data: serde_json::json!({"name": "test"}),
        })
        .await
        .unwrap();

    // Should receive
    let event = tokio::time::timeout(
        tokio::time::Duration::from_millis(500),
        conn.read_event(500),
    )
    .await
    .expect("Timed out waiting for wildcard-matched event")
    .unwrap()
    .expect("Expected an event");

    assert_eq!(event.topic, "plugin.registered");

    harness.shutdown().await;
}

#[tokio::test]
async fn multiple_subscribers_receive_same_event() {
    let harness = TestHarness::new().await.unwrap();

    let mut conn_a = register_plugin(&harness, "multi-a", "1.0.0").await.unwrap();
    let mut conn_b = register_plugin(&harness, "multi-b", "1.0.0").await.unwrap();

    // Both subscribe to the same topic
    let _ = conn_a
        .send_request(&Request::Subscribe {
            topics: vec!["shared.topic".to_string()],
        })
        .await
        .unwrap();
    let _ = conn_b
        .send_request(&Request::Subscribe {
            topics: vec!["shared.topic".to_string()],
        })
        .await
        .unwrap();

    // Publish once
    let _ = harness
        .send_request(&Request::Publish {
            topic: "shared.topic".to_string(),
            data: serde_json::json!({"broadcast": true}),
        })
        .await
        .unwrap();

    // Both should receive
    let event_a = tokio::time::timeout(
        tokio::time::Duration::from_millis(500),
        conn_a.read_event(500),
    )
    .await
    .expect("Plugin A timed out")
    .unwrap()
    .expect("Plugin A got no event");

    let event_b = tokio::time::timeout(
        tokio::time::Duration::from_millis(500),
        conn_b.read_event(500),
    )
    .await
    .expect("Plugin B timed out")
    .unwrap()
    .expect("Plugin B got no event");

    assert_eq!(event_a.topic, "shared.topic");
    assert_eq!(event_b.topic, "shared.topic");
    // Source is "unknown" because the publisher is a transient connection
    assert_eq!(event_a.source, "unknown");
    assert_eq!(event_b.source, "unknown");

    harness.shutdown().await;
}

#[tokio::test]
async fn cross_topic_isolation() {
    let harness = TestHarness::new().await.unwrap();

    let mut conn_a = register_plugin(&harness, "isolation-a", "1.0.0")
        .await
        .unwrap();
    let mut conn_b = register_plugin(&harness, "isolation-b", "1.0.0")
        .await
        .unwrap();

    // Subscribe to different topics
    let _ = conn_a
        .send_request(&Request::Subscribe {
            topics: vec!["topic.a".to_string()],
        })
        .await
        .unwrap();
    let _ = conn_b
        .send_request(&Request::Subscribe {
            topics: vec!["topic.b".to_string()],
        })
        .await
        .unwrap();

    // Publish to topic.b — only plugin B should get it
    let _ = harness
        .send_request(&Request::Publish {
            topic: "topic.b".to_string(),
            data: serde_json::json!({"target": "b"}),
        })
        .await
        .unwrap();

    // Plugin A should NOT receive
    let result = tokio::time::timeout(
        tokio::time::Duration::from_millis(200),
        conn_a.read_event(200),
    )
    .await;
    // Accept timeout (Err), or None (response skipped), but reject receiving an event
    assert!(
        matches!(result, Ok(Ok(None)) | Err(_)),
        "Plugin A should not have received an event for topic.b, got: {:?}",
        result
    );

    // Plugin B should receive
    let event_b = tokio::time::timeout(
        tokio::time::Duration::from_millis(500),
        conn_b.read_event(500),
    )
    .await
    .expect("Plugin B timed out")
    .unwrap()
    .expect("Plugin B got no event");
    assert_eq!(event_b.topic, "topic.b");

    harness.shutdown().await;
}

#[tokio::test]
async fn transient_connection_can_subscribe_with_synthetic_name() {
    let harness = TestHarness::new().await.unwrap();

    // Connect a transient client (not registered as a plugin)
    let response = harness
        .send_request(&Request::Subscribe {
            topics: vec!["any.topic".to_string()],
        })
        .await
        .unwrap();

    // Transient connections get a synthetic plugin name like "sub-conn_1"
    assert!(
        matches!(response, Response::Success { .. }),
        "Transient connection should be able to subscribe, got: {:?}",
        response
    );

    harness.shutdown().await;
}

#[tokio::test]
async fn unsubscribe_removes_subscription() {
    let harness = TestHarness::new().await.unwrap();

    let mut conn = register_plugin(&harness, "unsubscribe-test", "1.0.0")
        .await
        .unwrap();

    // Subscribe
    let _ = conn
        .send_request(&Request::Subscribe {
            topics: vec!["unsub.topic".to_string()],
        })
        .await
        .unwrap();

    // Unsubscribe
    let _ = conn
        .send_request(&Request::Unsubscribe {
            topics: vec!["unsub.topic".to_string()],
        })
        .await
        .unwrap();

    // Publish — should NOT receive
    let after_unsub =
        serde_json::Map::from_iter([("after-unsub".into(), serde_json::Value::Bool(true))]);
    let _ = harness
        .send_request(&Request::Publish {
            topic: "unsub.topic".to_string(),
            data: after_unsub.into(),
        })
        .await
        .unwrap();

    let result = tokio::time::timeout(
        tokio::time::Duration::from_millis(200),
        conn.read_event(200),
    )
    .await;
    assert!(
        matches!(result, Ok(Ok(None)) | Err(_)),
        "Plugin should not receive events after unsubscribing, got: {:?}",
        result
    );

    harness.shutdown().await;
}

#[tokio::test]
async fn event_data_preserved_correctly() {
    let harness = TestHarness::new().await.unwrap();

    let mut conn = register_plugin(&harness, "data-test", "1.0.0")
        .await
        .unwrap();

    let _ = conn
        .send_request(&Request::Subscribe {
            topics: vec!["data.topic".to_string()],
        })
        .await
        .unwrap();

    let complex_data = serde_json::json!({
        "nested": {
            "array": [1, 2, 3],
            "flag": true
        },
        "string": "hello world",
        "null_val": null
    });

    let _ = harness
        .send_request(&Request::Publish {
            topic: "data.topic".to_string(),
            data: complex_data.clone(),
        })
        .await
        .unwrap();

    let event = tokio::time::timeout(
        tokio::time::Duration::from_millis(500),
        conn.read_event(500),
    )
    .await
    .expect("Timed out")
    .unwrap()
    .expect("No event");

    assert_eq!(event.data, complex_data);

    harness.shutdown().await;
}

#[tokio::test]
async fn wildcard_prefix_matching() {
    let harness = TestHarness::new().await.unwrap();

    let mut conn = register_plugin(&harness, "prefix-wildcard", "1.0.0")
        .await
        .unwrap();

    // Subscribe with prefix wildcard: "health.*"
    let _ = conn
        .send_request(&Request::Subscribe {
            topics: vec!["health.*".to_string()],
        })
        .await
        .unwrap();

    // Publish to health.cpu
    let usage_val = serde_json::Map::from_iter([("usage".into(), serde_json::json!(42.0))]);
    let _ = harness
        .send_request(&Request::Publish {
            topic: "health.cpu".to_string(),
            data: usage_val.into(),
        })
        .await
        .unwrap();

    // Should receive
    let event = tokio::time::timeout(
        tokio::time::Duration::from_millis(500),
        conn.read_event(500),
    )
    .await
    .expect("Timed out")
    .unwrap()
    .expect("No event");
    assert_eq!(event.topic, "health.cpu");

    harness.shutdown().await;
}
