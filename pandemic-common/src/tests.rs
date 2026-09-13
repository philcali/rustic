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
                    Request::GetHealth => {
                        let health = serde_json::json!({
                            "active_plugins": 1,
                            "total_connections": 1,
                            "event_bus_subscribers": 0,
                            "uptime_seconds": 60,
                            "memory_used_mb": 512,
                            "memory_total_mb": 2048,
                            "cpu_usage_percent": 25.5,
                            "load_average": 1.2
                        });
                        Response::success_with_data(health)
                    }
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

    #[tokio::test]
    async fn test_get_health() {
        let temp_dir = TempDir::new().unwrap();
        let socket_path = temp_dir.path().join(format!(
            "test_{}.sock",
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let socket_path_str = socket_path.to_str().unwrap();

        tokio::spawn(mock_daemon_server(socket_path_str.to_string()));
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let request = Request::GetHealth;
        let response = DaemonClient::send_request(&socket_path, &request)
            .await
            .unwrap();

        match response {
            Response::Success { data } => {
                assert!(data.is_some());
                let health_data = data.unwrap();
                assert!(health_data["active_plugins"].is_number());
                assert!(health_data["memory_used_mb"].is_number());
                assert!(health_data["cpu_usage_percent"].is_number());
            }
            _ => panic!("Expected success response"),
        }
    }
}

#[cfg(test)]
mod apply_preview_tests {
    //! Phase 8 wire masking: the dry-run payload must carry names, targets,
    //! owners, modes and content hashes — never values, contents, or the
    //! health-check command.

    use crate::apply::{
        deployment_preview_data, plan_preview, sha256_hex, DeploymentPlan, ResolvedInfection,
    };
    use pandemic_protocol::spec::{parse_deployment_spec, parse_infection_spec};
    use pandemic_protocol::{Plan, PlanUnit, RenderedFile, UserConfig};
    use std::collections::BTreeMap;

    const SECRET_VALUE: &str = "s3cr3t-api-key";
    const FILE_CONTENT: &str = "api_key: file-secret-VALUE\nlisten: 0.0.0.0";
    const UNIT_CONTENT: &str = "Environment=UNIT-secret-VALUE";
    const HEALTH_WORD: &str = "-H Authorization: Bearer HEALTH-secret-VALUE";

    const SECRETS: &[&str] = &[
        SECRET_VALUE,
        "file-secret-VALUE",
        "UNIT-secret-VALUE",
        "HEALTH-secret-VALUE",
    ];

    fn secret_plan() -> Plan {
        Plan {
            name: "rest".into(),
            version: "1.0.0".into(),
            description: "REST API".into(),
            variables: {
                let mut m = BTreeMap::new();
                m.insert("api_key".into(), SECRET_VALUE.into());
                m.insert("listen_host".into(), "127.0.0.1".into());
                m
            },
            files: vec![RenderedFile {
                target: "/etc/pandemic/rest/rest-auth.toml".into(),
                content: FILE_CONTENT.into(),
                owner: "pandemic".into(),
                mode: "0600".into(),
            }],
            unit: Some(PlanUnit {
                name: "pandemic-rest".into(),
                target: "/etc/systemd/system/pandemic-rest.service".into(),
                content: UNIT_CONTENT.into(),
                enable: true,
            }),
            attach: None,
            health_check: vec!["curl".into(), "-sf".into(), HEALTH_WORD.into()],
            health_interval: 30,
            declared_packages: {
                let mut m = BTreeMap::new();
                m.insert("apt".into(), vec!["curl".into()]);
                m
            },
            groups: vec!["pandemic".into()],
            users: vec![(
                "pandemic".into(),
                UserConfig {
                    shell: None,
                    home_dir: None,
                    groups: None,
                    system_user: Some(true),
                },
            )],
        }
    }

    #[test]
    fn plan_preview_redacts_secrets() {
        let preview = plan_preview(&secret_plan());
        let json = serde_json::to_string(&preview).unwrap();

        for secret in SECRETS {
            assert!(!json.contains(secret), "preview leaks '{secret}': {json}");
        }

        // The review-relevant facts remain.
        assert!(json.contains("/etc/pandemic/rest/rest-auth.toml"));
        assert!(json.contains("0600"));
        assert!(json.contains("pandemic-rest"));
        assert_eq!(preview.files[0].sha256, sha256_hex(FILE_CONTENT));
        assert_eq!(
            preview.unit.as_ref().unwrap().sha256,
            sha256_hex(UNIT_CONTENT)
        );
        assert_eq!(preview.users, vec!["pandemic".to_string()]);
        assert_eq!(
            preview.variable_names,
            vec!["api_key".to_string(), "listen_host".to_string()]
        );
        assert!(preview.health.configured);
        assert_eq!(preview.health.interval, 30);
    }

    #[test]
    fn deployment_preview_data_redacts_shared_values() {
        let inf_spec = parse_infection_spec(
            r#"
[infection]
name = "rest"
version = "1.0.0"
description = "REST API"

[health]
check = ["true"]
"#,
        )
        .unwrap();
        let dep_spec = parse_deployment_spec(
            r#"
[deployment]
name = "rest-mqtt"
version = "1.0.0"

[variables]
api_key = "default-secret"
host = "localhost"

[[infections]]
name = "rest"
source = "infections/rest/infection.toml"
order = 1
"#,
        )
        .unwrap();

        let dp = DeploymentPlan {
            spec: dep_spec,
            shared: {
                let mut m = BTreeMap::new();
                m.insert("api_key".into(), SECRET_VALUE.into());
                m.insert("host".into(), "localhost".into());
                m
            },
            infections: vec![ResolvedInfection {
                name: "rest".into(),
                order: 1,
                source: "infections/rest/infection.toml".into(),
                spec: inf_spec,
                plan: secret_plan(),
            }],
        };

        let data = deployment_preview_data(&dp);
        let json = serde_json::to_string(&data).unwrap();

        for secret in SECRETS {
            assert!(
                !json.contains(secret),
                "dry-run payload leaks '{secret}': {json}"
            );
        }
        assert!(
            !json.contains("default-secret"),
            "spec default leaked: {json}"
        );

        // Names and structure remain.
        assert_eq!(data["name"], "rest-mqtt");
        assert_eq!(
            data["shared_variable_names"],
            serde_json::json!(["api_key", "host"])
        );
        assert_eq!(data["infections"][0]["name"], "rest");
        assert_eq!(
            data["infections"][0]["preview"]["variable_names"],
            serde_json::json!(["api_key", "listen_host"])
        );
        assert_eq!(
            data["infections"][0]["preview"]["files"][0]["sha256"],
            sha256_hex(FILE_CONTENT)
        );
    }
}
