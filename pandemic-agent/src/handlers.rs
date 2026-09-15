use pandemic_common::RegistryClient;
use pandemic_protocol::{AgentRequest, Response};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::infection::{attach_infection, detach_infection, AttachParams};
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
                "capabilities": ["systemd", "service_management", "user_management", "group_management", "service_config", "infection_registry", "package_management", "file_management", "infection_lifecycle", "deployment_lifecycle"],
                "package_managers": crate::packages::detect_package_managers()
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
                "daemon-reload" => crate::systemd::daemon_reload()
                    .await
                    .map(|()| String::new()),
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

        AgentRequest::GetInfectionManifest { name } => {
            info!("Getting infection manifest: {}", name);
            let client = RegistryClient::new();
            match client.get_infection_manifest(&name).await {
                Ok(manifest) => Response::success_with_data(serde_json::json!(manifest)),
                Err(e) => Response::error(format!("Failed to get manifest: {}", e)),
            }
        }

        AgentRequest::InstallInfection { name, target_path } => {
            info!("Installing infection: {}", name);
            let client = RegistryClient::new();

            let manifest = match client.get_infection_manifest(&name).await {
                Ok(m) => m,
                Err(e) => return Response::error(format!("Failed to get manifest: {}", e)),
            };

            let install_path = target_path.unwrap_or_else(|| format!("/usr/local/bin/{}", name));

            match client.download_infection(&manifest, &install_path).await {
                Ok(_) => Response::success_with_data(serde_json::json!({
                    "name": name,
                    "version": manifest.version,
                    "path": install_path
                })),
                Err(e) => Response::error(format!("Failed to install infection: {}", e)),
            }
        }

        AgentRequest::AttachInfection {
            unit,
            name,
            version,
            description,
            health_check,
            health_interval,
            proxy_path,
        } => {
            info!("Attaching infection from unit: {}", unit);
            match attach_infection(&AttachParams {
                unit,
                name,
                version,
                description,
                health_check,
                health_interval,
                proxy_path,
            })
            .await
            {
                Ok(result) => Response::success_with_data(result),
                Err(e) => Response::error(format!("Failed to attach infection: {}", e)),
            }
        }

        AgentRequest::DetachInfection { name } => {
            info!("Detaching infection: {}", name);
            match detach_infection(&name).await {
                Ok(result) => Response::success_with_data(result),
                Err(e) => Response::error(format!("Failed to detach infection: {}", e)),
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

        AgentRequest::PackageInstall { manager, packages } => {
            info!("Installing packages via {manager}: {}", packages.join(", "));
            match crate::packages::install_packages(&manager, &packages).await {
                Ok(()) => Response::success(),
                Err(e) => Response::error(format!("Package install failed: {e}")),
            }
        }

        AgentRequest::WriteFile {
            path,
            content,
            owner,
            mode,
        } => {
            info!(
                "Writing file {path} (owner {owner}, mode {mode}, {} bytes)",
                content.len()
            );
            match crate::files::write_file(&path, &content, &owner, &mode).await {
                Ok(()) => Response::success(),
                Err(e) => Response::error(format!("WriteFile failed: {e}")),
            }
        }

        AgentRequest::RecordInfection { name, state } => {
            info!("Recording infection: {name} (version {})", state.version);
            match crate::state::record_infection(&name, &state) {
                Ok(()) => Response::success_with_data(serde_json::json!({ "name": name })),
                Err(e) => Response::error(format!("Failed to record infection: {e}")),
            }
        }

        AgentRequest::ListInfections => {
            info!("Listing infections");
            match crate::state::list_infections() {
                Ok(infections) => Response::success_with_data(serde_json::json!({
                    "infections": infections
                })),
                Err(e) => Response::error(format!("Failed to list infections: {e}")),
            }
        }

        AgentRequest::GetInfectionStatus { name } => {
            info!("Infection status: {name}");
            if !crate::state::is_installed(&name) {
                return Response::not_found(format!("infection '{name}' is not installed"));
            }
            match crate::state::infection_status(&name).await {
                Ok(status) => Response::success_with_data(status),
                Err(e) => Response::error(format!("Failed to read infection status: {e}")),
            }
        }

        AgentRequest::UninstallInfection { name, purge } => {
            info!("Uninstalling infection: {name} (purge: {purge})");
            if !crate::state::is_installed(&name) {
                return Response::not_found(format!("infection '{name}' is not installed"));
            }
            match crate::state::uninstall_infection(&name, purge).await {
                Ok(result) => Response::success_with_data(result),
                Err(e) => Response::error(format!("Failed to uninstall infection: {e}")),
            }
        }

        AgentRequest::RecordDeployment { name, state } => {
            info!(
                "Recording deployment: {name} (version {}, {} infections)",
                state.version,
                state.infections.len()
            );
            match crate::deployments::record_deployment(&name, &state) {
                Ok(()) => Response::success_with_data(serde_json::json!({ "name": name })),
                Err(e) => Response::error(format!("Failed to record deployment: {e}")),
            }
        }

        AgentRequest::ListDeployments => {
            info!("Listing deployments");
            match crate::deployments::list_deployments() {
                Ok(deployments) => Response::success_with_data(serde_json::json!({
                    "deployments": deployments
                })),
                Err(e) => Response::error(format!("Failed to list deployments: {e}")),
            }
        }

        AgentRequest::GetDeploymentStatus { name } => {
            info!("Deployment status: {name}");
            if !crate::deployments::is_deployed(&name) {
                return Response::not_found(format!("deployment '{name}' is not installed"));
            }
            match crate::deployments::deployment_status(&name).await {
                Ok(status) => Response::success_with_data(status),
                Err(e) => Response::error(format!("Failed to read deployment status: {e}")),
            }
        }

        AgentRequest::RemoveDeployment { name, purge } => {
            info!("Removing deployment: {name} (purge: {purge})");
            if !crate::deployments::is_deployed(&name) {
                return Response::not_found(format!("deployment '{name}' is not installed"));
            }
            match crate::deployments::remove_deployment(&name, purge).await {
                Ok(result) => Response::success_with_data(result),
                Err(e) => Response::error(format!("Failed to remove deployment: {e}")),
            }
        }

        AgentRequest::ApplyInfection { plan, owner } => {
            info!("Applying infection: {} (owner {:?})", plan.name, owner);
            match crate::apply::apply_infection(&plan, owner.as_deref()).await {
                Ok(result) => Response::success_with_data(result),
                Err(e) => Response::error(format!("Failed to apply infection: {e}")),
            }
        }

        AgentRequest::ApplyDeployment {
            name,
            version,
            variables,
            infections,
        } => {
            info!(
                "Applying deployment: {name} (v{version}, {} infections)",
                infections.len()
            );
            match crate::apply::apply_deployment(&name, &version, &variables, &infections).await {
                Ok(result) => Response::success_with_data(result),
                Err(e) => Response::error(format!("Failed to apply deployment: {e}")),
            }
        }

        AgentRequest::PreviewInfection { plan } => {
            info!("Previewing infection plan: {}", plan.name);
            Response::success_with_data(crate::preview::preview_infection(&plan).await)
        }
        AgentRequest::PreviewDeployment { infections } => {
            info!("Previewing deployment ({} infections)", infections.len());
            Response::success_with_data(crate::preview::preview_deployment(&infections).await)
        }
    }
}
