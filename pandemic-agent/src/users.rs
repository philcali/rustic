use std::{collections::HashSet, process::Command};

use pandemic_protocol::UserConfig;
use serde::Deserialize;
use tracing::warn;

#[derive(Debug, Deserialize)]
struct BlocklistConfig {
    blocklist: Blocklist,
}

#[derive(Debug, Deserialize)]
struct Blocklist {
    users: Vec<String>,
    groups: Vec<String>,
}

fn load_blocklist() -> (HashSet<String>, HashSet<String>) {
    let config_content =
        std::fs::read_to_string("/etc/pandemic/blocklist.toml").unwrap_or_else(|_| {
            warn!("No blocklist config found, using built-in defaults");
            get_default_blocklist_config()
        });

    match toml::from_str::<BlocklistConfig>(&config_content) {
        Ok(config) => (
            config.blocklist.users.into_iter().collect(),
            config.blocklist.groups.into_iter().collect(),
        ),
        Err(e) => {
            warn!(
                "Failed to parse blocklist config: {}, using built-in defaults",
                e
            );
            get_default_blocklist()
        }
    }
}

fn get_default_users() -> Vec<&'static str> {
    vec![
        "root",
        "daemon",
        "bin",
        "sys",
        "sync",
        "games",
        "man",
        "lp",
        "mail",
        "news",
        "uucp",
        "proxy",
        "www-data",
        "backup",
        "list",
        "irc",
        "gnats",
        "nobody",
        "systemd-network",
        "systemd-resolve",
        "systemd-timesync",
        "messagebus",
        "syslog",
        "uuidd",
        "tcpdump",
        "tss",
        "_apt",
        "lxd",
        "dnsmasq",
        "landscape",
        "pollinate",
        "sshd",
        "pandemic",
    ]
}

fn get_default_groups() -> Vec<&'static str> {
    vec![
        "root",
        "daemon",
        "bin",
        "sys",
        "adm",
        "tty",
        "disk",
        "lp",
        "mail",
        "news",
        "uucp",
        "man",
        "proxy",
        "kmem",
        "dialout",
        "fax",
        "voice",
        "cdrom",
        "floppy",
        "tape",
        "sudo",
        "audio",
        "dip",
        "www-data",
        "backup",
        "operator",
        "list",
        "irc",
        "src",
        "gnats",
        "shadow",
        "utmp",
        "video",
        "sasl",
        "plugdev",
        "staff",
        "games",
        "users",
        "nogroup",
        "systemd-journal",
        "systemd-network",
        "systemd-resolve",
        "systemd-timesync",
        "input",
        "kvm",
        "render",
        "crontab",
        "netdev",
        "messagebus",
        "systemd-coredump",
        "lxd",
        "mlocate",
        "ssh",
        "landscape",
        "admin",
        "wheel",
        "pandemic",
    ]
}

fn get_default_blocklist_config() -> String {
    let users = get_default_users();
    let groups = get_default_groups();

    format!(
        r#"[blocklist]
users = {:?}
groups = {:?}"#,
        users, groups
    )
}

fn get_default_blocklist() -> (HashSet<String>, HashSet<String>) {
    let users = get_default_users().into_iter().map(String::from).collect();
    let groups = get_default_groups().into_iter().map(String::from).collect();
    (users, groups)
}

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
    let (blocklist_users, blocklist_groups) = load_blocklist();
    if blocklist_users.contains(username) {
        return Err(anyhow::anyhow!("Cannot update blocked user: {}", username));
    }

    let mut cmd = Command::new("usermod");

    if let Some(shell) = &config.shell {
        cmd.arg("-s").arg(shell);
    }
    if let Some(home) = &config.home_dir {
        cmd.arg("-d").arg(home);
    }
    if let Some(groups) = &config.groups {
        for group in groups {
            if blocklist_groups.contains(group) {
                warn!("Cannot add user {} to blocked group {}", username, group);
                continue;
            }
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
    let (blocklist_users, blocklist_groups) = load_blocklist();
    if blocklist_users.contains(username) {
        return Err(anyhow::anyhow!(
            "Cannot add blocked user to group: {}",
            username
        ));
    }
    if blocklist_groups.contains(group) {
        return Err(anyhow::anyhow!(
            "Cannot add user to blocked group: {}",
            group
        ));
    }
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
    let (blocklist_users, blocklist_groups) = load_blocklist();
    if blocklist_users.contains(username) {
        return Err(anyhow::anyhow!(
            "Cannot add blocked user to group: {}",
            username
        ));
    }
    if blocklist_groups.contains(group) {
        return Err(anyhow::anyhow!(
            "Cannot add user to blocked group: {}",
            group
        ));
    }
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
    let (blocklist_users, _) = load_blocklist();
    if blocklist_users.contains(username) {
        return Err(anyhow::anyhow!("Cannot delete blocked user: {}", username));
    }
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

    let (blocklist_users, _) = load_blocklist();
    let users: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| line.split(':').next().unwrap_or("").to_string())
        .filter(|u| !u.is_empty())
        .filter(|u| !blocklist_users.contains(u))
        .collect();

    Ok(users)
}

pub async fn list_groups() -> anyhow::Result<Vec<String>> {
    let output = Command::new("getent").arg("group").output()?;
    if !output.status.success() {
        return Err(anyhow::anyhow!("getent group failed"));
    }

    let (_, blocklist_groups) = load_blocklist();
    let groups: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| line.split(':').next().unwrap_or("").to_string())
        .filter(|g| !g.is_empty())
        .filter(|g| !blocklist_groups.contains(g))
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
    let (_, blocklist_groups) = load_blocklist();
    if blocklist_groups.contains(groupname) {
        return Err(anyhow::anyhow!(
            "Cannot delete blocked group: {}",
            groupname
        ));
    }
    let output = Command::new("groupdel").arg(groupname).output()?;
    if !output.status.success() {
        return Err(anyhow::anyhow!(
            "groupdel failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}
