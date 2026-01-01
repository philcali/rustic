use pandemic_protocol::{AgentRequest, Response};
use serde::{Deserialize, Serialize};
use tracing::info;

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
                "capabilities": ["systemd", "service_management", "user_management", "group_management", "service_config", "infection_registry"]
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
