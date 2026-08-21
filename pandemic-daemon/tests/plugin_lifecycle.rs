//! Tests for the plugin registration lifecycle.

use pandemic_daemon::tests::test_harness::*;
use pandemic_protocol::{PluginInfo, Request, Response};
use std::collections::HashMap;

#[tokio::test]
async fn full_register_list_get_deregister() {
    let harness = TestHarness::new().await.unwrap();

    // Register
    let plugin = PluginInfo {
        name: "test-plugin".to_string(),
        version: "1.0.0".to_string(),
        description: Some("A test plugin".to_string()),
        config: Some({
            let mut cfg = HashMap::new();
            cfg.insert("key".to_string(), "value".to_string());
            cfg
        }),
        registered_at: None,
    };
    let response = harness
        .send_request(&Request::Register {
            plugin: plugin.clone(),
        })
        .await
        .unwrap();
    assert!(matches!(response, Response::Success { .. }));

    // List
    let response = harness.send_request(&Request::ListPlugins).await.unwrap();
    match response {
        Response::Success { data } => {
            let plugins: Vec<PluginInfo> = serde_json::from_value(data.unwrap()).unwrap();
            assert_eq!(plugins.len(), 1);
            assert_eq!(plugins[0].name, "test-plugin");
        }
        _ => panic!("Expected success for ListPlugins"),
    }

    // Get
    let response = harness
        .send_request(&Request::GetPlugin {
            name: "test-plugin".to_string(),
        })
        .await
        .unwrap();
    match response {
        Response::Success { data } => {
            let got: PluginInfo = serde_json::from_value(data.unwrap()).unwrap();
            assert_eq!(got.name, "test-plugin");
            assert_eq!(got.version, "1.0.0");
            assert_eq!(got.description, Some("A test plugin".to_string()));
        }
        _ => panic!("Expected success for GetPlugin"),
    }

    // Deregister
    let response = harness
        .send_request(&Request::Deregister {
            name: "test-plugin".to_string(),
        })
        .await
        .unwrap();
    assert!(matches!(response, Response::Success { .. }));

    // Verify gone
    let response = harness
        .send_request(&Request::GetPlugin {
            name: "test-plugin".to_string(),
        })
        .await
        .unwrap();
    assert!(matches!(response, Response::NotFound { .. }));

    let response = harness.send_request(&Request::ListPlugins).await.unwrap();
    match response {
        Response::Success { data } => {
            let plugins: Vec<PluginInfo> = serde_json::from_value(data.unwrap()).unwrap();
            assert!(plugins.is_empty());
        }
        _ => panic!("Expected success for ListPlugins"),
    }

    harness.shutdown().await;
}

#[tokio::test]
async fn duplicate_registration_overwrites() {
    let harness = TestHarness::new().await.unwrap();

    let plugin1 = PluginInfo {
        name: "dup-plugin".to_string(),
        version: "1.0.0".to_string(),
        description: Some("First version".to_string()),
        config: None,
        registered_at: None,
    };
    let _ = harness
        .send_request(&Request::Register { plugin: plugin1 })
        .await
        .unwrap();

    let plugin2 = PluginInfo {
        name: "dup-plugin".to_string(),
        version: "2.0.0".to_string(),
        description: Some("Second version".to_string()),
        config: None,
        registered_at: None,
    };
    let _ = harness
        .send_request(&Request::Register { plugin: plugin2 })
        .await
        .unwrap();

    // Should have exactly one entry
    let response = harness.send_request(&Request::ListPlugins).await.unwrap();
    match response {
        Response::Success { data } => {
            let plugins: Vec<PluginInfo> = serde_json::from_value(data.unwrap()).unwrap();
            assert_eq!(plugins.len(), 1);
            assert_eq!(plugins[0].version, "2.0.0");
            assert_eq!(plugins[0].description, Some("Second version".to_string()));
        }
        _ => panic!("Expected success"),
    }

    harness.shutdown().await;
}

#[tokio::test]
async fn get_nonexistent_plugin_returns_not_found() {
    let harness = TestHarness::new().await.unwrap();

    let response = harness
        .send_request(&Request::GetPlugin {
            name: "nonexistent".to_string(),
        })
        .await
        .unwrap();
    assert!(matches!(response, Response::NotFound { .. }));

    harness.shutdown().await;
}

#[tokio::test]
async fn deregister_nonexistent_returns_not_found() {
    let harness = TestHarness::new().await.unwrap();

    let response = harness
        .send_request(&Request::Deregister {
            name: "ghost".to_string(),
        })
        .await
        .unwrap();
    assert!(matches!(response, Response::NotFound { .. }));

    harness.shutdown().await;
}

#[tokio::test]
async fn multiple_plugins_listed_correctly() {
    let harness = TestHarness::new().await.unwrap();

    for i in 0..5 {
        let plugin = PluginInfo {
            name: format!("plugin-{}", i),
            version: "1.0.0".to_string(),
            description: None,
            config: None,
            registered_at: None,
        };
        let _ = harness
            .send_request(&Request::Register { plugin })
            .await
            .unwrap();
    }

    let response = harness.send_request(&Request::ListPlugins).await.unwrap();
    match response {
        Response::Success { data } => {
            let plugins: Vec<PluginInfo> = serde_json::from_value(data.unwrap()).unwrap();
            assert_eq!(plugins.len(), 5);
        }
        _ => panic!("Expected success"),
    }

    // Deregister some, verify count drops
    let _ = harness
        .send_request(&Request::Deregister {
            name: "plugin-0".to_string(),
        })
        .await
        .unwrap();
    let _ = harness
        .send_request(&Request::Deregister {
            name: "plugin-4".to_string(),
        })
        .await
        .unwrap();

    let response = harness.send_request(&Request::ListPlugins).await.unwrap();
    match response {
        Response::Success { data } => {
            let plugins: Vec<PluginInfo> = serde_json::from_value(data.unwrap()).unwrap();
            assert_eq!(plugins.len(), 3);
        }
        _ => panic!("Expected success"),
    }

    harness.shutdown().await;
}

#[tokio::test]
async fn plugin_registered_at_is_set() {
    let harness = TestHarness::new().await.unwrap();

    let plugin = PluginInfo {
        name: "timing-plugin".to_string(),
        version: "1.0.0".to_string(),
        description: None,
        config: None,
        registered_at: None,
    };
    let _ = harness
        .send_request(&Request::Register { plugin })
        .await
        .unwrap();

    let response = harness
        .send_request(&Request::GetPlugin {
            name: "timing-plugin".to_string(),
        })
        .await
        .unwrap();
    match response {
        Response::Success { data } => {
            let got: serde_json::Value = data.unwrap();
            assert!(
                got["registered_at"].is_string(),
                "registered_at should be an RFC3339 timestamp (string), got: {}",
                got["registered_at"]
            );
        }
        _ => panic!("Expected success"),
    }

    harness.shutdown().await;
}
