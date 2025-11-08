use anyhow::Result;
use std::process::Command;

fn system_name(service: &str) -> String {
    if service.starts_with("pandemic") {
        service.to_string()
    } else {
        format!("pandemic-{}", service)
    }
}

pub fn install_service(service: &str, service_content: &str) -> Result<()> {
    let service_name = system_name(service);
    let service_path = format!("/etc/systemd/system/{}.service", service_name);
    std::fs::write(&service_path, service_content)?;
    Command::new("systemctl").args(["daemon-reload"]).status()?;
    Command::new("systemctl")
        .args(["enable", &service_name])
        .status()?;
    println!("Installed service: {}", service_name);
    Ok(())
}

pub fn uninstall_service(service: &str) -> Result<()> {
    let service_name = system_name(service);
    Command::new("systemctl")
        .args(["disable", &service_name])
        .status()?;
    Command::new("systemctl")
        .args(["stop", &service_name])
        .status()?;

    let service_path = format!("/etc/systemd/system/{}.service", service_name);
    std::fs::remove_file(&service_path)?;

    Command::new("systemctl").args(["daemon-reload"]).status()?;
    println!("Uninstalled service: {}", service_name);
    Ok(())
}

pub fn start_service(service: &str) -> Result<()> {
    let service_name = system_name(service);
    Command::new("systemctl")
        .args(["start", &service_name])
        .status()?;
    println!("Started service: {}", service_name);
    Ok(())
}

pub fn stop_service(service: &str) -> Result<()> {
    let service_name = system_name(service);
    Command::new("systemctl")
        .args(["stop", &service_name])
        .status()?;
    println!("Stopped service: {}", service_name);
    Ok(())
}

pub fn restart_service(service: &str) -> Result<()> {
    let service_name = system_name(service);
    Command::new("systemctl")
        .args(["restart", &service_name])
        .status()?;
    println!("Restarted service: {}", service_name);
    Ok(())
}

pub fn status_service(service: &str) -> Result<()> {
    let service_name = system_name(service);
    Command::new("systemctl")
        .args(["status", &service_name])
        .status()?;
    Ok(())
}
