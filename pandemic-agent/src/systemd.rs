use anyhow::Result;
use pandemic_protocol::ServiceOverrides;
use std::process::Command;

use crate::handlers::PandemicServiceSummary;

pub async fn execute_systemctl(action: &str, service: &str) -> Result<String> {
    let output = Command::new("systemctl")
        .arg(action)
        .arg(service)
        .output()?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(anyhow::anyhow!(
            "systemctl failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

pub async fn list_pandemic_services() -> Result<Vec<serde_json::Value>> {
    let output = Command::new("systemctl")
        .arg("--legend=false")
        .arg("--plain")
        .arg("list-units")
        .arg("pandemic*")
        .output()?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let services: Vec<serde_json::Value> = stdout
            .lines()
            .filter_map(|line| {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 4 {
                    let summary = PandemicServiceSummary {
                        name: parts[0].to_string(),
                        description: parts[3..].join(" "),
                        status: parts[2].to_string(),
                    };
                    Some(serde_json::json!(summary))
                } else {
                    None
                }
            })
            .collect();
        Ok(services)
    } else {
        Err(anyhow::anyhow!(
            "systemctl list-units failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

pub async fn delete_service_override(service: &str) -> anyhow::Result<()> {
    let override_dir = format!("/etc/systemd/system/{}.service.d", service);
    let override_file = format!("{}/override.conf", override_dir);

    if std::path::Path::new(&override_file).exists() {
        std::fs::remove_file(override_file)?;
        std::fs::remove_dir_all(override_dir)?;
    }

    // Reload systemd
    let status = Command::new("systemctl").arg("daemon-reload").status()?;
    if !status.success() {
        return Err(anyhow::anyhow!("systemctl daemon-reload failed"));
    }

    Ok(())
}

pub async fn get_service_override(service: &str) -> anyhow::Result<Option<ServiceOverrides>> {
    let override_file = format!("/etc/systemd/system/{}.service.d/override.conf", service);
    if !std::path::Path::new(&override_file).exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(override_file)?;
    let mut overrides = ServiceOverrides {
        environment: None,
        exec_start: None,
        restart: None,
        user: None,
        group: None,
    };

    for line in content.lines() {
        if let Some((key, value)) = line.split_once('=') {
            match key {
                "User" => overrides.user = Some(value.to_string()),
                "Group" => overrides.group = Some(value.to_string()),
                "Restart" => overrides.restart = Some(value.to_string()),
                "ExecStart" => overrides.exec_start = Some(value.to_string()),
                "Environment" => {
                    if let Some((env_key, env_value)) = value.split_once('=') {
                        overrides
                            .environment
                            .get_or_insert_with(Default::default)
                            .insert(env_key.to_string(), env_value.to_string());
                    }
                }
                _ => {}
            }
        }
    }

    Ok(Some(overrides))
}

pub async fn set_service_override(
    service: &str,
    overrides: &ServiceOverrides,
) -> anyhow::Result<()> {
    let override_dir = format!("/etc/systemd/system/{}.service.d", service);
    std::fs::create_dir_all(&override_dir)?;

    let override_file = format!("{}/override.conf", override_dir);
    let mut content = String::from("[Service]\n");

    if let Some(user) = &overrides.user {
        content.push_str(&format!("User={}\n", user));
    }
    if let Some(group) = &overrides.group {
        content.push_str(&format!("Group={}\n", group));
    }
    if let Some(restart) = &overrides.restart {
        content.push_str(&format!("Restart={}\n", restart));
    }
    if let Some(exec_start) = &overrides.exec_start {
        content.push_str("ExecStart=\n");
        content.push_str(&format!("ExecStart={}\n", exec_start));
    }
    if let Some(env) = &overrides.environment {
        for (key, value) in env {
            content.push_str(&format!("Environment={}={}\n", key, value));
        }
    }

    std::fs::write(&override_file, content)?;

    // Reload systemd
    let status = Command::new("systemctl").arg("daemon-reload").status()?;
    if !status.success() {
        return Err(anyhow::anyhow!("systemctl daemon-reload failed"));
    }

    Ok(())
}
