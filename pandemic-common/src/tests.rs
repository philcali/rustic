#[cfg(test)]
mod client_tests {
    use crate::client::DaemonClient;
    use pandemic_protocol::{PluginInfo, Request, Response};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU32, Ordering};
    use tempfile::TempDir;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    async fn mock_daemon_server(socket_path: String) {
        let _ = std::fs::remove_file(&socket_path);
        let listener = UnixListener::bind(&socket_path).unwrap();

        if let Ok((stream, _)) = listener.accept().await {
            let mut reader = BufReader::new(stream);
            let mut line = String::new();

            if reader.read_line(&mut line).await.unwrap() > 0 {
                let request: Request = serde_json::from_str(line.trim()).unwrap();

                let response = match request {
                    Request::ListPlugins => Response::success_with_data(serde_json::json!([])),
                    Request::GetPlugin { name } => {
                        if name == "test-plugin" {
                            let plugin = PluginInfo {
                                version: "1.0.0".to_string(),
                                name: "test-plugin".to_string(),
                                description: Some("Test plugin".to_string()),
                                config: None,
                                registered_at: None,
                            };
                            Response::success_with_data(serde_json::json!(plugin))
                        } else {
                            Response::not_found("Plugin not found")
                        }
                    }
                    Request::Register { .. } => Response::success(),
                    Request::Deregister { name } => {
                        if name == "test-plugin" {
                            Response::success()
                        } else {
                            Response::not_found("Plugin not found")
                        }
                    }
                    Request::Publish { .. } => Response::success(),
                    Request::Unsubscribe { .. } => Response::success(),
                    Request::Subscribe { .. } => Response::success(),
                };

                let response_json = serde_json::to_string(&response).unwrap();
                reader
                    .get_mut()
                    .write_all(response_json.as_bytes())
                    .await
                    .unwrap();
                reader.get_mut().write_all(b"\n").await.unwrap();
            }
        }
    }

    #[tokio::test]
    async fn test_list_plugins() {
        let temp_dir = TempDir::new().unwrap();
        let socket_path = temp_dir.path().join(format!(
            "test_{}.sock",
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let socket_path_str = socket_path.to_str().unwrap();

        tokio::spawn(mock_daemon_server(socket_path_str.to_string()));
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let request = Request::ListPlugins;
        let response = DaemonClient::send_request(&socket_path, &request)
            .await
            .unwrap();

        match response {
            Response::Success { data } => assert!(data.is_some()),
            _ => panic!("Expected success response"),
        }
    }

    #[tokio::test]
    async fn test_get_existing_plugin() {
        let temp_dir = TempDir::new().unwrap();
        let socket_path = temp_dir.path().join(format!(
            "test_{}.sock",
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let socket_path_str = socket_path.to_str().unwrap();

        tokio::spawn(mock_daemon_server(socket_path_str.to_string()));
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let request = Request::GetPlugin {
            name: "test-plugin".to_string(),
        };
        let response = DaemonClient::send_request(&socket_path, &request)
            .await
            .unwrap();

        match response {
            Response::Success { data } => assert!(data.is_some()),
            _ => panic!("Expected success response"),
        }
    }

    #[tokio::test]
    async fn test_get_nonexistent_plugin() {
        let temp_dir = TempDir::new().unwrap();
        let socket_path = temp_dir.path().join(format!(
            "test_{}.sock",
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let socket_path_str = socket_path.to_str().unwrap();

        tokio::spawn(mock_daemon_server(socket_path_str.to_string()));
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let request = Request::GetPlugin {
            name: "nonexistent".to_string(),
        };
        let response = DaemonClient::send_request(&socket_path, &request)
            .await
            .unwrap();

        match response {
            Response::NotFound { .. } => {}
            _ => panic!("Expected not found response"),
        }
    }

    #[tokio::test]
    async fn test_register_plugin() {
        let temp_dir = TempDir::new().unwrap();
        let socket_path = temp_dir.path().join(format!(
            "test_{}.sock",
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let socket_path_str = socket_path.to_str().unwrap();

        tokio::spawn(mock_daemon_server(socket_path_str.to_string()));
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let plugin = PluginInfo {
            name: "test-plugin".to_string(),
            version: "1.0.0".to_string(),
            description: Some("Test plugin".to_string()),
            config: Some(HashMap::new()),
            registered_at: None,
        };

        let request = Request::Register { plugin };
        let response = DaemonClient::send_request(&socket_path, &request)
            .await
            .unwrap();

        match response {
            Response::Success { .. } => {}
            _ => panic!("Expected success response"),
        }
    }
}

#[cfg(test)]
mod config_tests {
    use crate::config::{ConfigManager, FileConfigManager};
    use tempfile::TempDir;
    use tokio::fs;

    #[tokio::test]
    async fn test_get_config_with_defaults_only() {
        let temp_dir = TempDir::new().unwrap();
        let default_dir = temp_dir.path().join("defaults");
        let override_dir = temp_dir.path().join("overrides");

        fs::create_dir_all(&default_dir).await.unwrap();
        fs::write(default_dir.join("test.toml"), "key1 = \"default_value\"")
            .await
            .unwrap();

        let config_manager = FileConfigManager::new(&default_dir, &override_dir);
        let config = config_manager.get_config("test").await.unwrap();

        assert_eq!(config["key1"], "default_value");
    }

    #[tokio::test]
    async fn test_get_config_with_overrides() {
        let temp_dir = TempDir::new().unwrap();
        let default_dir = temp_dir.path().join("defaults");
        let override_dir = temp_dir.path().join("overrides");

        fs::create_dir_all(&default_dir).await.unwrap();
        fs::create_dir_all(&override_dir).await.unwrap();

        fs::write(
            default_dir.join("test.toml"),
            "key1 = \"default_value\"\nkey2 = \"default2\"",
        )
        .await
        .unwrap();

        fs::write(override_dir.join("test.toml"), "key1 = \"override_value\"")
            .await
            .unwrap();

        let config_manager = FileConfigManager::new(&default_dir, &override_dir);
        let config = config_manager.get_config("test").await.unwrap();

        assert_eq!(config["key1"], "override_value");
        assert_eq!(config["key2"], "default2");
    }
}
