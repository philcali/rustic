//! WriteFile primitive: path-restricted file writes (spec-driven install,
//! phase 2).
//!
//! v1 restrictions (ideas/deployments.md, "Security Considerations"):
//! allowed prefixes are `/etc`, `/opt`, `/usr/local`, `/var`, and the
//! agent secret, socket dir, and pandemic binaries are never writable
//! through this path.

use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Result};
use tracing::info;

/// Prefixes `WriteFile` may write under (v1 allowlist).
pub const ALLOWED_PREFIXES: &[&str] = &["/etc", "/opt", "/usr/local", "/var"];

/// Protected paths: pandemic internals that must never be overwritten
/// through `WriteFile`, even though they sit under allowed prefixes.
pub const PROTECTED_PATHS: &[&str] = &[
    "/etc/pandemic/agent-secret",
    "/etc/pandemic/blocklist.toml",
    "/etc/pandemic/infections",
    "/var/run/pandemic",
    "/run/pandemic",
    "/usr/local/bin/pandemic",
    "/usr/local/bin/pandemic-agent",
    "/usr/local/bin/pandemic-proxy",
    "/usr/local/bin/pandemic-cli",
    "/usr/bin/pandemic",
    "/usr/bin/pandemic-agent",
    "/usr/bin/pandemic-proxy",
    "/usr/bin/pandemic-cli",
];

/// Validate a `WriteFile` target path: absolute, no `..` traversal,
/// under an allowed prefix, and not a protected pandemic path.
pub fn validate_write_path(path: &str) -> Result<()> {
    if !path.starts_with('/') {
        bail!("path '{path}' must be absolute");
    }
    if path.split('/').any(|component| component == "..") {
        bail!("path '{path}' must not contain '..' components");
    }

    let normalized = path.trim_end_matches('/');
    if !normalized.is_empty() && normalized != "/" {
        let allowed = ALLOWED_PREFIXES
            .iter()
            .any(|prefix| normalized == *prefix || normalized.starts_with(&format!("{prefix}/")));
        if !allowed {
            bail!(
                "path '{path}' is outside the allowed prefixes ({})",
                ALLOWED_PREFIXES.join(", ")
            );
        }
    }

    for protected in PROTECTED_PATHS {
        if normalized == *protected || normalized.starts_with(&format!("{protected}/")) {
            bail!("path '{path}' is protected (pandemic internals) and cannot be written");
        }
    }

    Ok(())
}

fn owner_part_ok(part: &str) -> bool {
    !part.is_empty()
        && part.len() <= 32
        && part.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_' || c == '.'
        })
}

/// Validate an owning user (or `user:group`) name.
pub fn validate_owner(owner: &str) -> Result<()> {
    let ok = match owner.split_once(':') {
        Some((user, group)) => owner_part_ok(user) && owner_part_ok(group),
        None => owner_part_ok(owner),
    };
    if ok {
        Ok(())
    } else {
        bail!("invalid owner '{owner}': expected a user name or 'user:group'")
    }
}

/// Validate a file mode: 3-4 digit octal string, e.g. "0600".
pub fn validate_mode(mode: &str) -> Result<()> {
    let digits = mode.trim_start_matches("0o");
    if !digits.is_empty() && digits.len() <= 4 && digits.chars().all(|c| ('0'..='7').contains(&c)) {
        Ok(())
    } else {
        bail!("invalid mode '{mode}': expected 3-4 octal digits, e.g. \"0600\"")
    }
}

/// Run a small helper program, surfacing stderr on failure.
fn run_program(binary: &str, args: &[&str]) -> Result<()> {
    let output = Command::new(binary)
        .args(args)
        .output()
        .map_err(|e| anyhow::anyhow!("failed to run {binary}: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        bail!(
            "{binary} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    }
}

/// Write `content` to `path`, then set ownership and mode.
///
/// The target directory is created if needed. `chown` runs before
/// `chmod` so the final mode wins.
pub async fn write_file(path: &str, content: &str, owner: &str, mode: &str) -> Result<()> {
    validate_write_path(path)?;
    validate_owner(owner)?;
    validate_mode(mode)?;

    let target = Path::new(path);
    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    fs::write(target, content)?;
    run_program("chown", &[owner, path])?;
    run_program("chmod", &[mode, path])?;

    info!(
        "wrote {} bytes to {path} (owner {owner}, mode {mode})",
        content.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_paths_under_allowed_prefixes() {
        assert!(validate_write_path("/etc/pandemic/rest-auth.toml").is_ok());
        assert!(validate_write_path("/opt/app/config.yaml").is_ok());
        assert!(validate_write_path("/usr/local/share/data").is_ok());
        assert!(validate_write_path("/var/lib/app/state").is_ok());
        // exact prefix is also fine
        assert!(validate_write_path("/etc").is_ok());
    }

    #[test]
    fn rejects_paths_outside_allowlist() {
        assert!(validate_write_path("/tmp/evil").is_err());
        assert!(validate_write_path("/home/user/.bashrc").is_err());
        assert!(validate_write_path("/bin/ls").is_err());
        // prefix must be a whole component: /etcfoo is not /etc
        assert!(validate_write_path("/etcfoo/evil").is_err());
    }

    #[test]
    fn rejects_relative_and_traversal() {
        assert!(validate_write_path("etc/passwd").is_err());
        assert!(validate_write_path("/etc/../etc/passwd").is_err());
        assert!(validate_write_path("/etc/pandemic/../../tmp/x").is_err());
    }

    #[test]
    fn rejects_protected_paths() {
        assert!(validate_write_path("/etc/pandemic/agent-secret").is_err());
        assert!(validate_write_path("/var/run/pandemic/admin.sock").is_err());
        assert!(validate_write_path("/run/pandemic/admin.sock").is_err());
        assert!(validate_write_path("/usr/local/bin/pandemic-agent").is_err());
        assert!(validate_write_path("/usr/local/bin/pandemic").is_err());
        // pandemic config that would weaken its own controls
        assert!(validate_write_path("/etc/pandemic/blocklist.toml").is_err());
        assert!(validate_write_path("/etc/pandemic/infections/mosq.toml").is_err());
        assert!(validate_write_path("/etc/pandemic/infections").is_err());
        // but sibling pandemic state files are fine
        assert!(validate_write_path("/etc/pandemic/rest-auth.toml").is_ok());
        assert!(validate_write_path("/usr/local/bin/pandemic-other").is_ok());
    }

    #[test]
    fn validates_owner() {
        assert!(validate_owner("root").is_ok());
        assert!(validate_owner("rest").is_ok());
        assert!(validate_owner("mosquitto").is_ok());
        assert!(validate_owner("svc:wheel").is_ok());
        assert!(validate_owner("").is_err());
        assert!(validate_owner("bad name").is_err());
        assert!(validate_owner("with/slash").is_err());
        assert!(validate_owner("UPPER").is_err());
        assert!(validate_owner(":group").is_err());
        assert!(validate_owner("user:").is_err());
    }

    #[test]
    fn validates_mode() {
        assert!(validate_mode("0600").is_ok());
        assert!(validate_mode("0644").is_ok());
        assert!(validate_mode("600").is_ok());
        assert!(validate_mode("0o640").is_ok());
        assert!(validate_mode("").is_err());
        assert!(validate_mode("0999").is_err());
        assert!(validate_mode("abcd").is_err());
        assert!(validate_mode("01234").is_err());
    }
}
