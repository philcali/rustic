//! Attach an existing systemd unit as an infection.
//!
//! The agent installs a "sidecar registrar" unit (`pandemic-<name>.service`)
//! that runs `pandemic-proxy --attach <unit>`: the proxy registers the named
//! unit with the daemon and mirrors its health, while systemd keeps owning
//! the actual service lifecycle.

use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const DEFAULT_PROXY_PATH: &str = "/usr/local/bin/pandemic-proxy";
pub const INFECTIONS_DIR: &str = "/etc/pandemic/infections";
pub const DEFAULT_VERSION: &str = "0.0.0";
pub const DEFAULT_HEALTH_INTERVAL: u64 = 30;

#[derive(Debug, Clone)]
pub struct AttachParams {
    pub unit: String,
    pub name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub health_check: Option<Vec<String>>,
    pub health_interval: Option<u64>,
    pub proxy_path: Option<String>,
}

/// `mosquitto` / `mosquitto.service` -> `mosquitto`
pub fn unit_base_name(unit: &str) -> String {
    unit.split('.').next().unwrap_or(unit).to_string()
}

/// Sidecar unit that hosts the attached infection.
pub fn attach_service_name(name: &str) -> String {
    if name.starts_with("pandemic") {
        name.to_string()
    } else {
        format!("pandemic-{}", name)
    }
}

pub fn validate_infection_name(name: &str) -> Result<()> {
    let valid = !name.is_empty()
        && name.len() <= 63
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if valid {
        Ok(())
    } else {
        bail!(
            "invalid infection name '{name}': use 1-63 lowercase letters, digits or hyphens (no leading/trailing hyphen)"
        )
    }
}

/// Parameters after defaults are applied, used to render files.
#[derive(Debug, Clone)]
pub struct ResolvedAttach {
    name: String,
    unit: String,
    version: String,
    description: Option<String>,
    health_check: Option<Vec<String>>,
    health_interval: u64,
}

fn resolve(params: &AttachParams) -> Result<ResolvedAttach> {
    let name = params
        .name
        .clone()
        .unwrap_or_else(|| unit_base_name(&params.unit));
    validate_infection_name(&name)?;
    Ok(ResolvedAttach {
        name,
        unit: params.unit.clone(),
        version: params
            .version
            .clone()
            .unwrap_or_else(|| DEFAULT_VERSION.to_string()),
        description: params.description.clone(),
        health_check: params.health_check.clone(),
        health_interval: params.health_interval.unwrap_or(DEFAULT_HEALTH_INTERVAL),
    })
}

#[derive(Serialize)]
struct InfectionToml {
    infection: InfectionMeta,
    runtime: RuntimeToml,
}

#[derive(Serialize)]
struct InfectionMeta {
    name: String,
    version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

#[derive(Serialize)]
struct RuntimeToml {
    attach: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    health_check: Option<Vec<String>>,
    health_interval: u64,
}

/// Render the pandemic-proxy config file for an attached infection.
pub fn build_infection_config(resolved: &ResolvedAttach) -> String {
    let toml = InfectionToml {
        infection: InfectionMeta {
            name: resolved.name.clone(),
            version: resolved.version.clone(),
            description: resolved.description.clone(),
        },
        runtime: RuntimeToml {
            attach: resolved.unit.clone(),
            health_check: resolved.health_check.clone(),
            health_interval: resolved.health_interval,
        },
    };
    // Static struct with trivial fields: cannot fail.
    toml::to_string(&toml).expect("infection config serializes to TOML")
}

/// Render the sidecar unit that supervises `pandemic-proxy --attach`.
pub fn build_attach_unit(resolved: &ResolvedAttach, config_path: &str, proxy_path: &str) -> String {
    let wants_unit = if resolved.unit.contains('.') {
        resolved.unit.clone()
    } else {
        format!("{}.service", resolved.unit)
    };
    format!(
        r#"[Unit]
Description=Pandemic Infection (attached): {}
Wants={}
After={} pandemic.service
Requires=pandemic.service

[Service]
Type=simple
ExecStart={} --attach {} --config {}
Restart=always
RestartSec=5
User=pandemic
Group=pandemic

[Install]
WantedBy=multi-user.target
"#,
        resolved.name, wants_unit, wants_unit, proxy_path, resolved.unit, config_path
    )
}

async fn ensure_unit_loaded(unit: &str) -> Result<()> {
    let output = Command::new("systemctl")
        .args(["show", unit, "--property=LoadState", "--value"])
        .output()
        .context("failed to run systemctl")?;
    let load_state = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if output.status.success() && load_state == "loaded" {
        Ok(())
    } else {
        bail!(
            "systemd unit '{unit}' is not loaded (LoadState={load_state}); install and enable it first"
        )
    }
}

async fn daemon_reload() -> Result<()> {
    let status = Command::new("systemctl").arg("daemon-reload").status()?;
    if !status.success() {
        bail!("systemctl daemon-reload failed");
    }
    Ok(())
}

/// Install the sidecar unit + proxy config and start everything.
pub async fn attach_infection(params: &AttachParams) -> Result<serde_json::Value> {
    let resolved = resolve(params)?;
    let name = resolved.name.clone();
    ensure_unit_loaded(&resolved.unit).await?;

    let proxy_path = params
        .proxy_path
        .clone()
        .unwrap_or_else(|| DEFAULT_PROXY_PATH.to_string());
    if !Path::new(&proxy_path).exists() {
        bail!("pandemic-proxy not found at {proxy_path}; install it or pass proxy_path");
    }

    let config_path = PathBuf::from(INFECTIONS_DIR).join(format!("{name}.toml"));
    let unit_path = PathBuf::from(format!(
        "/etc/systemd/system/{}.service",
        attach_service_name(&name)
    ));
    let service_name = attach_service_name(&name);

    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(&config_path, build_infection_config(&resolved))
        .with_context(|| format!("writing {}", config_path.display()))?;
    std::fs::write(
        &unit_path,
        build_attach_unit(&resolved, &config_path.to_string_lossy(), &proxy_path),
    )
    .with_context(|| format!("writing {}", unit_path.display()))?;

    daemon_reload().await?;

    let output = Command::new("systemctl")
        .args(["enable", "--now", &service_name])
        .output()?;
    if !output.status.success() {
        // Roll back the files we just wrote so a failed enable leaves no residue.
        let _ = std::fs::remove_file(&config_path);
        let _ = std::fs::remove_file(&unit_path);
        let _ = Command::new("systemctl").arg("daemon-reload").status();
        bail!(
            "systemctl enable --now {service_name} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(serde_json::json!({
        "name": name,
        "unit": resolved.unit,
        "service": service_name,
        "config_path": config_path.to_string_lossy(),
        "proxy_path": proxy_path,
    }))
}

/// Stop, disable and remove a previously attached infection.
pub async fn detach_infection(name: &str) -> Result<serde_json::Value> {
    validate_infection_name(name)?;
    let service_name = attach_service_name(name);
    let unit_path = format!("/etc/systemd/system/{service_name}.service");
    let config_path = format!("{INFECTIONS_DIR}/{name}.toml");

    // Best effort: stop + disable (unit may not be enabled or running).
    let _ = Command::new("systemctl")
        .args(["disable", "--now", &service_name])
        .status();
    let _ = Command::new("systemctl")
        .arg("stop")
        .arg(&service_name)
        .status();

    let removed_unit = Path::new(&unit_path).exists() && std::fs::remove_file(&unit_path).is_ok();
    let removed_config =
        Path::new(&config_path).exists() && std::fs::remove_file(&config_path).is_ok();

    // Drop the infections dir if it is now empty (best effort).
    let _ = std::fs::remove_dir(INFECTIONS_DIR);

    daemon_reload().await?;

    Ok(serde_json::json!({
        "name": name,
        "service": service_name,
        "removed_unit": removed_unit,
        "removed_config": removed_config,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_base_name_strips_suffix() {
        assert_eq!(unit_base_name("mosquitto"), "mosquitto");
        assert_eq!(unit_base_name("mosquitto.service"), "mosquitto");
        assert_eq!(unit_base_name("foo-bar.service"), "foo-bar");
    }

    #[test]
    fn attach_service_name_prefixed_once() {
        assert_eq!(attach_service_name("mosquitto"), "pandemic-mosquitto");
        assert_eq!(attach_service_name("pandemic-foo"), "pandemic-foo");
    }

    #[test]
    fn name_validation() {
        assert!(validate_infection_name("mosquitto").is_ok());
        assert!(validate_infection_name("a-b-2").is_ok());
        assert!(validate_infection_name("").is_err());
        assert!(validate_infection_name("Has-Upper").is_err());
        assert!(validate_infection_name("-lead").is_err());
        assert!(validate_infection_name("trail-").is_err());
        assert!(validate_infection_name("with space").is_err());
        assert!(validate_infection_name(&"a".repeat(64)).is_err());
    }

    fn params(name: Option<&str>, unit: &str) -> AttachParams {
        AttachParams {
            unit: unit.to_string(),
            name: name.map(str::to_string),
            version: None,
            description: Some("test infection".to_string()),
            health_check: None,
            health_interval: None,
            proxy_path: None,
        }
    }

    #[test]
    fn config_defaults_and_rendering() {
        let resolved = resolve(&params(None, "mosquitto")).unwrap();
        assert_eq!(resolved.name, "mosquitto");
        assert_eq!(resolved.version, DEFAULT_VERSION);
        assert_eq!(resolved.health_interval, DEFAULT_HEALTH_INTERVAL);

        let toml = build_infection_config(&resolved);
        assert!(toml.contains(r#"name = "mosquitto""#));
        assert!(toml.contains(r#"attach = "mosquitto""#));
        assert!(toml.contains("health_interval = 30"));
        assert!(toml.contains(r#"description = "test infection""#));
    }

    #[test]
    fn service_unit_wiring() {
        let resolved = resolve(&params(Some("mosq"), "mosquitto")).unwrap();
        let unit = build_attach_unit(
            &resolved,
            "/etc/pandemic/infections/mosq.toml",
            "/usr/local/bin/pandemic-proxy",
        );
        assert!(unit.contains("Wants=mosquitto.service"));
        assert!(unit.contains("After=mosquitto.service pandemic.service"));
        assert!(unit.contains("Requires=pandemic.service"));
        assert!(unit.contains(
            "ExecStart=/usr/local/bin/pandemic-proxy --attach mosquitto --config /etc/pandemic/infections/mosq.toml"
        ));
        assert!(unit.contains("User=pandemic"));
        assert!(unit.contains("WantedBy=multi-user.target"));
    }
}
