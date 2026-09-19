//! The narrow deployment surface a node exposes to a coordinator.
//!
//! A node is a *target* of the epidemic, not a general-purpose remote shell:
//! it only accepts the deployment lifecycle requests and nothing that mutates
//! users/groups/services or runs arbitrary packages/files. This "narrow
//! posture" is what lets a coordinator spread a deployment to a whole group
//! without turning every node into a full remote admin socket.

use pandemic_protocol::AgentRequest;

/// Whether `request` is on the node's exposed deployment surface.
pub fn request_allowed(request: &AgentRequest) -> bool {
    matches!(
        request,
        AgentRequest::GetCapabilities
            | AgentRequest::ApplyDeployment { .. }
            | AgentRequest::GetDeploymentStatus { .. }
            | AgentRequest::ListDeployments
            | AgentRequest::PreviewDeployment { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pandemic_protocol::{ApplyDeploymentInfection, Plan, UserConfig};

    fn plan() -> Plan {
        Plan {
            name: "i".into(),
            version: "0.1.0".into(),
            description: String::new(),
            variables: Default::default(),
            files: vec![],
            unit: None,
            attach: None,
            health_check: vec![],
            health_interval: 30,
            declared_packages: Default::default(),
            groups: vec![],
            users: vec![],
        }
    }

    fn infection() -> ApplyDeploymentInfection {
        ApplyDeploymentInfection {
            name: "i".into(),
            version: "0.1.0".into(),
            order: 0,
            source: "inline".into(),
            plan: plan(),
        }
    }

    fn empty_user() -> UserConfig {
        UserConfig {
            shell: None,
            home_dir: None,
            groups: None,
            system_user: None,
        }
    }

    #[test]
    fn deployment_surface_is_allowed() {
        assert!(request_allowed(&AgentRequest::GetCapabilities));
        assert!(request_allowed(&AgentRequest::ListDeployments));
        assert!(request_allowed(&AgentRequest::GetDeploymentStatus {
            name: "d".into()
        }));
        assert!(request_allowed(&AgentRequest::ApplyDeployment {
            name: "d".into(),
            version: "1".into(),
            variables: Default::default(),
            infections: vec![infection()],
        }));
        assert!(request_allowed(&AgentRequest::PreviewDeployment {
            infections: vec![infection()]
        }));
    }

    #[test]
    fn everything_else_is_denied() {
        assert!(!request_allowed(&AgentRequest::GetHealth));
        assert!(!request_allowed(&AgentRequest::ListServices));
        assert!(!request_allowed(&AgentRequest::SystemdControl {
            action: "start".into(),
            service: "x".into()
        }));
        assert!(!request_allowed(&AgentRequest::UserCreate {
            username: "u".into(),
            config: empty_user()
        }));
        assert!(!request_allowed(&AgentRequest::PackageInstall {
            manager: "apt".into(),
            packages: vec!["x".into()]
        }));
        assert!(!request_allowed(&AgentRequest::WriteFile {
            path: "/x".into(),
            content: "c".into(),
            owner: "root".into(),
            mode: "0600".into()
        }));
        assert!(!request_allowed(&AgentRequest::ApplyInfection {
            plan: plan(),
            owner: None
        }));
        assert!(!request_allowed(&AgentRequest::RemoveDeployment {
            name: "d".into(),
            purge: false
        }));
        assert!(!request_allowed(&AgentRequest::GetInfectionStatus {
            name: "i".into()
        }));
    }
}
