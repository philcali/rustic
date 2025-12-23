use pandemic_protocol::{AgentRequest, Response};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::systemd::{execute_systemctl, list_pandemic_services, set_service_override};
use crate::users::{create_group, create_user, list_users};

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

        _ => Response::error("Operation not implemented yet"),
    }
}
