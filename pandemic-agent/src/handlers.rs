use pandemic_protocol::{AgentRequest, Response};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use tracing::{info, warn};

use crate::systemd::{
    delete_service_override, execute_systemctl, get_service_override, list_pandemic_services,
    set_service_override,
};
use crate::users::{
    add_user_to_group, create_group, create_user, delete_group, delete_user, list_groups,
    list_users, remove_user_from_group, update_user,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PandemicServiceSummary {
    pub name: String,
    pub description: String,
    pub status: String,
}

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

pub async fn handle_agent_request(request: AgentRequest) -> Response {
    match request {
        AgentRequest::GetHealth => {
            info!("Health check requested");
            Response::success_with_data(serde_json::json!({
                "status": "healthy",
                "capabilities": ["systemd"]
            }))
        }

        AgentRequest::ListServices => {
            info!("Service list requested");
            match list_pandemic_services().await {
                Ok(services) => Response::success_with_data(serde_json::json!({
                    "services": services
                })),
                Err(e) => Response::error(format!("Failed to list services: {}", e)),
            }
        }

        AgentRequest::GetCapabilities => {
            info!("Capabilities requested");
            Response::success_with_data(serde_json::json!({
                "capabilities": ["systemd", "service_management", "user_management", "group_management", "service_config"]
            }))
        }

        AgentRequest::UserCreate { username, config } => {
            info!("Creating user: {}", username);
            match create_user(&username, &config).await {
                Ok(_) => Response::success(),
                Err(e) => Response::error(format!("Failed to create user: {}", e)),
            }
        }

        AgentRequest::ListUsers => {
            info!("Listing users");
            match list_users().await {
                Ok(users) => Response::success_with_data(serde_json::json!({ "users": users })),
                Err(e) => Response::error(format!("Failed to list users: {}", e)),
            }
        }

        AgentRequest::ListGroups => {
            info!("Listing groups");
            match list_groups().await {
                Ok(groups) => Response::success_with_data(serde_json::json!({ "groups": groups })),
                Err(e) => Response::error(format!("Failed to list groups: {}", e)),
            }
        }

        AgentRequest::GroupCreate { groupname } => {
            info!("Creating group: {}", groupname);
            match create_group(&groupname).await {
                Ok(_) => Response::success(),
                Err(e) => Response::error(format!("Failed to create group: {}", e)),
            }
        }

        AgentRequest::ServiceConfigOverride { service, overrides } => {
            info!("Setting service config override for: {}", service);
            match set_service_override(&service, &overrides).await {
                Ok(_) => Response::success(),
                Err(e) => Response::error(format!("Failed to set service override: {}", e)),
            }
        }

        AgentRequest::GetServiceConfig { service } => {
            info!("Getting service config for: {}", service);
            match get_service_override(&service).await {
                Ok(config) => Response::success_with_data(serde_json::json!({
                    "service": service,
                    "config": config
                })),
                Err(e) => Response::error(format!("Failed to get service config: {}", e)),
            }
        }

        AgentRequest::ServiceConfigReset { service } => {
            info!("Resetting service config for: {}", service);
            match delete_service_override(&service).await {
                Ok(_) => Response::success(),
                Err(e) => Response::error(format!("Failed to reset service config: {}", e)),
            }
        }

        AgentRequest::SystemdControl { action, service } => {
            info!("Systemd control: {} {}", action, service);

            let result = match action.as_str() {
                "start" | "stop" | "restart" | "enable" | "disable" | "status" => {
                    execute_systemctl(&action, &service).await
                }
                _ => {
                    return Response::error("Invalid systemd action");
                }
            };

            match result {
                Ok(output) => Response::success_with_data(serde_json::json!({
                    "action": action,
                    "service": service,
                    "output": output
                })),
                Err(e) => Response::error(format!("Systemd operation failed: {}", e)),
            }
        }

        AgentRequest::UserDelete { username } => {
            info!("Deleting user: {}", username);
            let (blocked_users, _) = load_blocklist();
            if blocked_users.contains(&username) {
                return Response::error(format!(
                    "User '{}' is protected and cannot be deleted",
                    username
                ));
            }
            match delete_user(&username).await {
                Ok(_) => Response::success(),
                Err(e) => Response::error(format!("Failed to delete user: {}", e)),
            }
        }

        AgentRequest::UserModify { username, config } => {
            info!("Modifying user: {}", username);
            match update_user(&username, &config).await {
                Ok(_) => Response::success(),
                Err(e) => Response::error(format!("Failed to modify user: {}", e)),
            }
        }

        AgentRequest::GroupDelete { groupname } => {
            info!("Deleting group: {}", groupname);
            let (_, blocked_groups) = load_blocklist();
            if blocked_groups.contains(&groupname) {
                return Response::error(format!(
                    "Group '{}' is protected and cannot be deleted",
                    groupname
                ));
            }
            match delete_group(&groupname).await {
                Ok(_) => Response::success(),
                Err(e) => Response::error(format!("Failed to delete group: {}", e)),
            }
        }

        AgentRequest::GroupAddUser {
            groupname,
            username,
        } => {
            info!("Adding user to group: {} {}", username, groupname);
            match add_user_to_group(&username, &groupname).await {
                Ok(_) => Response::success(),
                Err(e) => Response::error(format!("Failed to add user to group: {}", e)),
            }
        }

        AgentRequest::GroupRemoveUser {
            groupname,
            username,
        } => {
            info!("Removing user from group: {} {}", username, groupname);
            match remove_user_from_group(&username, &groupname).await {
                Ok(_) => Response::success(),
                Err(e) => Response::error(format!("Failed to remove user from group: {}", e)),
            }
        }
    }
}
