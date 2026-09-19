//! Group / Node configuration for epidemic spreads.
//!
//! The **node / group / coordinator** primitive:
//!   - a **Node** is a host running a `pandemic-node` receiver: an identity
//!     name plus an `addr:port` endpoint.
//!   - a **Group** is a named trust boundary: one shared epidemic secret plus a
//!     roster of nodes.
//!   - the **Coordinator** (`pandemic-cli epidemic`) holds a group's secret +
//!     roster, plans a deployment, and applies it to every node in the roster.
//!
//! Groups are declared in a TOML file (default: `~/.config/pandemic/groups.toml`,
//! honoring `$XDG_CONFIG_HOME`):
//!
//! ```toml
//! [[group]]
//! name = "edge"
//! secret_path = "/etc/pandemic/secrets/edge"   # optional
//!
//! [[group.node]]
//! name = "edge-1"
//! addr = "192.168.1.10:7711"
//!
//! [[group.node]]
//! name = "edge-2"
//! addr = "192.168.1.11:7711"
//! ```

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One host in a group's roster.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeConfig {
    /// Human-readable identity for this node (shown in the results table).
    pub name: String,
    /// The node receiver's `host:port` endpoint.
    pub addr: String,
}

/// A named trust boundary: a shared secret + the nodes that joined it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupConfig {
    /// Unique group name (e.g. `edge`), addressed by `epidemic spread --group`.
    pub name: String,
    /// Optional path to this group's epidemic secret. When absent, the
    /// coordinator falls back to `--secret` / `--secret-path` / the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_path: Option<String>,
    /// The roster of nodes in this group. In the file this is `[[group.node]]`
    /// (the TOML table-array name is `node`; the field holds the array).
    #[serde(default)]
    pub node: Vec<NodeConfig>,
}

/// The on-disk shape of the groups file: a `[[group]]` array.
#[derive(Debug, Clone, Default, Deserialize)]
struct GroupsFile {
    #[serde(default)]
    group: Vec<GroupConfig>,
}

/// The default groups file: `$XDG_CONFIG_HOME/pandemic/groups.toml`, else
/// `~/.config/pandemic/groups.toml`.
pub fn default_groups_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(home_dir)
        .unwrap_or_else(|| PathBuf::from("/etc/pandemic"));
    base.join("pandemic").join("groups.toml")
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Load the group roster from `path`. A *missing* file is not an error — it
/// means "no named groups" (ad-hoc `--node` endpoints still work). A present
/// but unreadable or malformed file is an error.
pub fn load_groups(path: &std::path::Path) -> anyhow::Result<Vec<GroupConfig>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(anyhow::anyhow!("reading {}: {e}", path.display())),
    };
    let file: GroupsFile =
        toml::from_str(&text).map_err(|e| anyhow::anyhow!("parsing {}: {e}", path.display()))?;
    Ok(file.group)
}

/// Load from an explicit path, or the default when `None`.
pub fn load_groups_or_default(path: Option<&std::path::Path>) -> anyhow::Result<Vec<GroupConfig>> {
    match path {
        Some(p) => load_groups(p),
        None => load_groups(&default_groups_path()),
    }
}

/// Find a group by exact name.
pub fn find_group<'a>(groups: &'a [GroupConfig], name: &str) -> Option<&'a GroupConfig> {
    groups.iter().find(|g| g.name == name)
}

/// Merge a group's roster with ad-hoc `--node` endpoints, de-duplicated by
/// `addr` (first occurrence wins). The result is the coordinator's target list.
pub fn merge_roster(group: Option<&GroupConfig>, adhoc: &[String]) -> Vec<NodeConfig> {
    let mut out: Vec<NodeConfig> = Vec::new();
    if let Some(group) = group {
        for node in &group.node {
            if !out.iter().any(|n| n.addr == node.addr) {
                out.push(node.clone());
            }
        }
    }
    for addr in adhoc {
        let name = addr.clone();
        if !out.iter().any(|n| n.addr == *addr) {
            out.push(NodeConfig {
                name,
                addr: addr.clone(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[[group]]
name = "edge"
secret_path = "/etc/pandemic/secrets/edge"

[[group.node]]
name = "edge-1"
addr = "192.168.1.10:7711"

[[group.node]]
name = "edge-2"
addr = "192.168.1.11:7711"

[[group]]
name = "lab"

[[group.node]]
name = "lab-a"
addr = "127.0.0.1:7711"
"#;

    #[test]
    fn parses_groups_and_nodes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("groups.toml");
        std::fs::write(&path, SAMPLE).unwrap();
        let groups = load_groups(&path).expect("parse");
        assert_eq!(groups.len(), 2);

        let edge = find_group(&groups, "edge").expect("edge group");
        assert_eq!(
            edge.secret_path.as_deref(),
            Some("/etc/pandemic/secrets/edge")
        );
        assert_eq!(edge.node.len(), 2);
        assert_eq!(edge.node[0].name, "edge-1");
        assert_eq!(edge.node[1].addr, "192.168.1.11:7711");

        let lab = find_group(&groups, "lab").expect("lab group");
        assert_eq!(lab.secret_path, None);
        assert_eq!(lab.node.len(), 1);
    }

    #[test]
    fn secret_path_is_optional() {
        let g: GroupConfig =
            toml::from_str("name = \"x\"\n[[node]]\nname=\"n\"\naddr=\"1.2.3.4:1\"")
                .expect("parse");
        assert_eq!(g.secret_path, None);
        assert_eq!(g.node.len(), 1);
    }

    #[test]
    fn load_missing_file_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.toml");
        let groups = load_groups(&path).expect("missing file -> Ok(empty)");
        assert!(groups.is_empty());
    }

    #[test]
    fn load_malformed_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "this is [[[ not valid toml").unwrap();
        assert!(load_groups(&path).is_err());
    }

    #[test]
    fn merge_dedupes_by_addr_and_keeps_group_names() {
        let g = GroupConfig {
            name: "edge".into(),
            secret_path: None,
            node: vec![NodeConfig {
                name: "edge-1".into(),
                addr: "1.1.1.1:7711".into(),
            }],
        };
        // ad-hoc re-listing 1.1.1.1 is dropped; a new one is appended.
        let merged = merge_roster(
            Some(&g),
            &["1.1.1.1:7711".to_string(), "2.2.2.2:7711".to_string()],
        );
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].name, "edge-1", "group node keeps its name");
        assert_eq!(merged[1].name, "2.2.2.2:7711", "ad-hoc node names itself");
    }

    #[test]
    fn merge_with_no_group_is_just_adhoc() {
        let merged = merge_roster(None, &["9.9.9.9:7711".to_string()]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].addr, "9.9.9.9:7711");
    }
}
