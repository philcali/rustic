use pandemic_protocol::{AgentRequest, Response};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::systemd::{execute_systemctl, list_pandemic_services};

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
                "capabilities": ["systemd", "service_management"]
            }))
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
    }
}
