use anyhow::Result;
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
