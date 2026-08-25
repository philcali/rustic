//! Agent shared-secret installation.
//!
//! Bootstrap/agent install runs as root, which makes it the natural place to
//! mint the agent secret. It lands at the default path
//! (`pandemic_common::AGENT_SECRET_PATH`, `/etc/pandemic/agent-secret`) so the
//! agent and this CLI agree without extra ceremony.

use anyhow::{Context, Result};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Generate the agent secret in `dir` if it does not exist yet.
///
/// Returns the secret file path. The file is written `0600 root` (the agent
/// runs as root; the secret is root-equivalent and must not be shared with
/// the `pandemic` group, which is only trusted to run infections).
pub fn ensure_agent_secret_in(dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join("agent-secret");

    if !path.exists() {
        let secret = hex::encode(rand::random::<[u8; 32]>());
        std::fs::write(&path, format!("{secret}\n"))
            .with_context(|| format!("writing {}", path.display()))?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("setting 0600 on {}", path.display()))?;
    }

    Ok(path)
}

/// Generate the agent secret at the default location, if missing.
pub fn ensure_agent_secret() -> Result<PathBuf> {
    ensure_agent_secret_in(Path::new("/etc/pandemic"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pandemic-cli-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn generates_secret_with_0600() {
        let dir = temp_dir("secret-gen");
        let path = ensure_agent_secret_in(&dir).unwrap();
        assert_eq!(path, dir.join("agent-secret"));

        let meta = std::fs::metadata(&path).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);

        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            content.trim().len(),
            64,
            "expected 32 random bytes hex-encoded"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn preserves_existing_secret() {
        let dir = temp_dir("secret-keep");
        let path = dir.join("agent-secret");
        std::fs::write(&path, "pre-existing-secret\n").unwrap();

        let result = ensure_agent_secret_in(&dir).unwrap();
        assert_eq!(result, path);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "pre-existing-secret\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
