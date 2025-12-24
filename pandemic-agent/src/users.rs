use std::process::Command;

use pandemic_protocol::UserConfig;
use tracing::warn;

pub async fn create_user(username: &str, config: &UserConfig) -> anyhow::Result<()> {
    let mut cmd = Command::new("useradd");

    if let Some(shell) = &config.shell {
        cmd.arg("-s").arg(shell);
    }
    if let Some(home) = &config.home_dir {
        cmd.arg("-d").arg(home);
    }
    if config.system_user == Some(true) {
        cmd.arg("-r");
    }

    cmd.arg(username);
    let output = cmd.output()?;

    if !output.status.success() {
        return Err(anyhow::anyhow!(
            "useradd failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    if let Some(groups) = &config.groups {
        for group in groups {
            let status = Command::new("usermod")
                .arg("-a")
                .arg("-G")
                .arg(group)
                .arg(username)
                .status()?;
            if !status.success() {
                warn!("Failed to add user {} to group {}", username, group);
            }
        }
    }

    Ok(())
}

pub async fn update_user(username: &str, config: &UserConfig) -> anyhow::Result<()> {
    let mut cmd = Command::new("usermod");

    if let Some(shell) = &config.shell {
        cmd.arg("-s").arg(shell);
    }
    if let Some(home) = &config.home_dir {
        cmd.arg("-d").arg(home);
    }
    if let Some(groups) = &config.groups {
        for group in groups {
            let status = Command::new("usermod")
                .arg("-a")
                .arg("-G")
                .arg(group)
                .arg(username)
                .status()?;
            if !status.success() {
                warn!("Failed to add user {} to group {}", username, group);
            }
        }
    }

    cmd.arg(username);
    let output = cmd.output()?;

    if !output.status.success() {
        return Err(anyhow::anyhow!(
            "usermod failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(())
}

pub async fn add_user_to_group(username: &str, group: &str) -> anyhow::Result<()> {
    let output = Command::new("usermod")
        .arg("-a")
        .arg("-G")
        .arg(group)
        .arg(username)
        .output()?;
    if !output.status.success() {
        return Err(anyhow::anyhow!(
            "usermod failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

pub async fn remove_user_from_group(username: &str, group: &str) -> anyhow::Result<()> {
    let output = Command::new("gpasswd")
        .arg("-d")
        .arg(username)
        .arg(group)
        .output()?;
    if !output.status.success() {
        return Err(anyhow::anyhow!(
            "gpasswd failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

pub async fn delete_user(username: &str) -> anyhow::Result<()> {
    let output = Command::new("userdel").arg("-r").arg(username).output()?;
    if !output.status.success() {
        return Err(anyhow::anyhow!(
            "userdel failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

pub async fn list_users() -> anyhow::Result<Vec<String>> {
    let output = Command::new("getent").arg("passwd").output()?;
    if !output.status.success() {
        return Err(anyhow::anyhow!("getent passwd failed"));
    }

    let users: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| line.split(':').next().unwrap_or("").to_string())
        .filter(|u| !u.is_empty())
        .collect();

    Ok(users)
}

pub async fn list_groups() -> anyhow::Result<Vec<String>> {
    let output = Command::new("getent").arg("group").output()?;
    if !output.status.success() {
        return Err(anyhow::anyhow!("getent group failed"));
    }

    let groups: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| line.split(':').next().unwrap_or("").to_string())
        .filter(|g| !g.is_empty())
        .collect();

    Ok(groups)
}

pub async fn create_group(groupname: &str) -> anyhow::Result<()> {
    let output = Command::new("groupadd").arg(groupname).output()?;
    if !output.status.success() {
        return Err(anyhow::anyhow!(
            "groupadd failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

pub async fn delete_group(groupname: &str) -> anyhow::Result<()> {
    let output = Command::new("groupdel").arg(groupname).output()?;
    if !output.status.success() {
        return Err(anyhow::anyhow!(
            "groupdel failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}
