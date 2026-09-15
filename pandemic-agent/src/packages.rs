//! Package manager detection and installation (spec-driven install,
//! phase 2).
//!
//! A spec lists packages per manager; the agent picks the manager its
//! host supports. v1: plain package names, installed idempotently
//! (package managers no-op on already-installed packages).

use std::process::Command;

use anyhow::{bail, Result};
use tracing::info;

use pandemic_protocol::spec::KNOWN_PACKAGE_MANAGERS;

/// The binary + install verb for each known manager.
pub fn command_for(manager: &str) -> Option<(&'static str, &'static str)> {
    match manager {
        "apt" => Some(("apt-get", "install")),
        "dnf" => Some(("dnf", "install")),
        "pacman" => Some(("pacman", "install")),
        "apk" => Some(("apk", "add")),
        "zypper" => Some(("zypper", "install")),
        _ => None,
    }
}

/// Whether a binary is present on the agent's `PATH`.
pub fn binary_present(binary: &str) -> bool {
    match std::env::var_os("PATH") {
        Some(paths) => std::env::split_paths(&paths).any(|dir| dir.join(binary).is_file()),
        None => false,
    }
}

/// The package managers this host supports, in canonical order.
pub fn detect_package_managers() -> Vec<&'static str> {
    KNOWN_PACKAGE_MANAGERS
        .iter()
        .copied()
        .filter(|manager| command_for(manager).is_some_and(|(binary, _)| binary_present(binary)))
        .collect()
}

/// Run a command, surfacing stderr on failure.
fn run_program(binary: &str, args: &[&str]) -> Result<()> {
    info!("running: {binary} {}", args.join(" "));
    let output = Command::new(binary)
        .args(args)
        .output()
        .map_err(|e| anyhow::anyhow!("failed to run {binary}: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        bail!(
            "{} failed: {}",
            binary,
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// Install `packages` with `manager`.
///
/// `manager` must be one of `KNOWN_PACKAGE_MANAGERS`; an empty package
/// list is a no-op. `apt` refreshes its index first (fresh containers
/// ship without one).
pub async fn install_packages(manager: &str, packages: &[String]) -> Result<()> {
    if !KNOWN_PACKAGE_MANAGERS.contains(&manager) {
        bail!(
            "unknown package manager '{manager}': expected one of {}",
            KNOWN_PACKAGE_MANAGERS.join(", ")
        );
    }
    if packages.is_empty() {
        info!("no packages to install via {manager}");
        return Ok(());
    }
    let names: Vec<&str> = packages.iter().map(|p| p.as_str()).collect();
    if names.iter().any(|p| p.is_empty()) {
        bail!("empty package name in {manager} install");
    }

    match manager {
        "apt" => {
            run_program("apt-get", &["update"])?;
            let mut args: Vec<&str> = vec!["install", "-y"];
            args.extend(names.iter());
            run_program("apt-get", &args)
        }
        "dnf" => {
            let mut args: Vec<&str> = vec!["install", "-y"];
            args.extend(names.iter());
            run_program("dnf", &args)
        }
        "pacman" => {
            let mut args: Vec<&str> = vec!["-Sy", "--noconfirm"];
            args.extend(names.iter());
            run_program("pacman", &args)
        }
        "apk" => {
            let mut args: Vec<&str> = vec!["add", "--no-cache"];
            args.extend(names.iter());
            run_program("apk", &args)
        }
        "zypper" => {
            let mut args: Vec<&str> = vec!["--non-interactive", "install"];
            args.extend(names.iter());
            run_program("zypper", &args)
        }
        _ => unreachable!("manager already validated above"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_for_known_managers() {
        assert_eq!(command_for("apt"), Some(("apt-get", "install")));
        assert_eq!(command_for("dnf"), Some(("dnf", "install")));
        assert_eq!(command_for("pacman"), Some(("pacman", "install")));
        assert_eq!(command_for("apk"), Some(("apk", "add")));
        assert_eq!(command_for("zypper"), Some(("zypper", "install")));
        assert_eq!(command_for("yum"), None);
    }

    #[test]
    fn binary_present_finds_coreutils() {
        assert!(binary_present("sh"));
        assert!(!binary_present("definitely-not-a-real-binary-xyz"));
    }

    #[test]
    fn detects_apt_on_debian_like_hosts() {
        // The CI/dev host is debian-like (ubuntu-latest); where apt is
        // absent the detection simply reports nothing for it.
        let managers = detect_package_managers();
        if binary_present("apt-get") {
            assert!(managers.contains(&"apt"));
        } else {
            assert!(!managers.contains(&"apt"));
        }
    }
}
