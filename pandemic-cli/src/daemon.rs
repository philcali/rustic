use anyhow::Result;
use pandemic_common::DaemonClient;
use pandemic_protocol::{Request, Response};
use std::path::PathBuf;

use crate::DaemonAction;

pub async fn handle_daemon_command(socket_path: &PathBuf, action: DaemonAction) -> Result<()> {
    let request = match action {
        DaemonAction::List => Request::ListPlugins,
        DaemonAction::Get { name } => Request::GetPlugin { name },
        DaemonAction::Deregister { name } => Request::Deregister { name },
        DaemonAction::Status => {
            println!("Daemon is running at {:?}", socket_path);
            return Ok(());
        }
        DaemonAction::Health => Request::GetHealth,
    };

    let response = DaemonClient::send_request(socket_path, &request).await?;
    match response {
        Response::Success { data } => {
            if let Some(data) = data {
                println!("{}", serde_json::to_string_pretty(&data)?);
            } else {
                println!("Success");
            }
        }
        Response::Error { message } => {
            eprintln!("Error: {}", message);
        }
        Response::NotFound { message } => {
            eprintln!("Not Found: {}", message);
        }
    }

    Ok(())
}
