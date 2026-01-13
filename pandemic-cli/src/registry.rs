use crate::RegistryAction;
use anyhow::Result;
use pandemic_common::RegistryClient;
use std::path::PathBuf;
use tracing::{error, info};

pub async fn handle_registry_command(_socket_path: &PathBuf, action: RegistryAction) -> Result<()> {
    match action {
        RegistryAction::Search {
            query,
            registry_url,
        } => search_infections(&query, registry_url).await,
        RegistryAction::Get { name, registry_url } => {
            get_infection_manifest(&name, registry_url).await
        }
        RegistryAction::Install { name, registry_url } => {
            install_infection(&name, registry_url).await
        }
    }
}

async fn search_infections(query: &str, registry_url: Option<String>) -> Result<()> {
    let registry = match registry_url {
        Some(url) => RegistryClient::with_registry_url(url),
        None => RegistryClient::new(),
    };

    info!("Searching for infections matching '{}'...", query);

    match registry.search_infections(query).await {
        Ok(infections) => {
            if infections.is_empty() {
                println!("No infections found matching '{}'", query);
                return Ok(());
            }

            println!("Found {} infection(s):", infections.len());
            println!();

            for infection in infections {
                println!("📦 {}", infection.name);
                println!("   Version: {}", infection.latest_version);
                println!("   Description: {}", infection.description);
                println!();
            }
        }
        Err(e) => {
            error!("Failed to search infections: {}", e);
            return Err(e);
        }
    }

    Ok(())
}

async fn get_infection_manifest(name: &str, registry_url: Option<String>) -> Result<()> {
    let registry = match registry_url {
        Some(url) => RegistryClient::with_registry_url(url),
        None => RegistryClient::new(),
    };

    info!("Getting manifest for infection '{}'...", name);

    match registry.get_infection_manifest(name).await {
        Ok(manifest) => {
            println!("📋 Infection Manifest: {}", manifest.name);
            println!("   Version: {}", manifest.version);
            println!("   Description: {}", manifest.description);
            println!("   Author: {}", manifest.author);

            if let Some(homepage) = &manifest.homepage {
                println!("   Homepage: {}", homepage);
            }

            if let Some(license) = &manifest.license {
                println!("   License: {}", license);
            }

            if !manifest.dependencies.is_empty() {
                println!("   Dependencies: {}", manifest.dependencies.join(", "));
            }

            if !manifest.platforms.is_empty() {
                println!("   Platforms:");
                for platform in &manifest.platforms {
                    println!("     - {}-{}", platform.os, platform.arch);
                }
            }
        }
        Err(e) => {
            error!("Failed to get infection manifest: {}", e);
            return Err(e);
        }
    }

    Ok(())
}

async fn install_infection(name: &str, registry_url: Option<String>) -> Result<()> {
    let registry = match registry_url {
        Some(url) => RegistryClient::with_registry_url(url),
        None => RegistryClient::new(),
    };

    info!("Installing infection '{}'...", name);

    // Get the manifest first
    let manifest = registry.get_infection_manifest(name).await?;

    // Download to a default location
    let target_path = format!("/tmp/{}", name);

    match registry.download_infection(&manifest, &target_path).await {
        Ok(()) => {
            println!(
                "✅ Successfully downloaded infection '{}' to {}",
                name, target_path
            );
            println!(
                "   To install as a service, use: pandemic-cli service install {} {}",
                name, target_path
            );
        }
        Err(e) => {
            error!("Failed to download infection: {}", e);
            return Err(e);
        }
    }

    Ok(())
}
