//! Append-only audit log (ideas/deployments.md, phase 8).
//!
//! Every privileged lifecycle operation the agent performs — infection
//! apply / uninstall, deployment apply / remove — appends one JSON line to
//! [`AUDIT_FILE`] (0600, root-only): what was done, step by step, how each
//! step went, and what was left behind. Entries never contain variable
//! values or rendered file contents — names, steps, and outcomes only —
//! but the log is still secrets-adjacent, so it stays root-only like the
//! state records.
//!
//! Writers (agent) and readers (CLI, REST) share this module so the path
//! and the line format have exactly one definition.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use anyhow::{Context, Result};
use serde_json::Value;

/// Directory for the audit log (created 0755 on first write).
pub const AUDIT_DIR: &str = "/var/log/pandemic";
/// The append-only JSONL audit log (0600).
pub const AUDIT_FILE: &str = "/var/log/pandemic/audit.jsonl";

/// Current time, RFC3339 — same convention as the state records'
/// `installed_at` stamp.
pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Append `entry` to the JSONL log at `path`, enforcing 0600.
///
/// Best-effort by convention: callers log a warning on failure and let the
/// operation itself stand — the audit log must never break an install.
pub fn record_in(path: &str, entry: &Value) -> Result<()> {
    let parent = std::path::Path::new(path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty());
    if let Some(dir) = parent {
        fs::create_dir_all(dir)
            .with_context(|| format!("cannot create audit dir {}", dir.display()))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("cannot open audit log {path} for append"))?;
    // Enforce 0600 even against a restrictive umask or a pre-existing file.
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("cannot set 0600 on audit log {path}"))?;
    let mut line = serde_json::to_string(entry).context("serializing audit entry")?;
    line.push('\n');
    file.write_all(line.as_bytes())
        .with_context(|| format!("cannot write audit log {path}"))?;
    file.sync_all()
        .with_context(|| format!("cannot fsync audit log {path}"))?;
    Ok(())
}

/// Append `entry` to the default log.
pub fn record(entry: &Value) -> Result<()> {
    record_in(AUDIT_FILE, entry)
}

/// Record `entry` in the log at `path`, downgrading any failure to a
/// warning.
///
/// The audit log must never change the outcome of the operation being
/// audited — a full disk or missing `/var/log` cannot break an install.
pub fn record_best_effort_in(path: &str, entry: &Value) {
    if let Err(e) = record_in(path, entry) {
        tracing::warn!("audit log write failed (operation result unchanged): {e}");
    }
}

/// Record `entry` in the default log, best-effort (see
/// [`record_best_effort_in`]).
pub fn record_best_effort(entry: &Value) {
    record_best_effort_in(AUDIT_FILE, entry)
}

/// The last `limit` entries of the log at `path`, oldest → newest.
/// Malformed lines are skipped (a torn final line must not poison reads).
pub fn read_last_in(path: &str, limit: usize) -> Result<Vec<Value>> {
    let content =
        fs::read_to_string(path).with_context(|| format!("cannot read audit log {path}"))?;
    let entries: Vec<Value> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let start = entries.len().saturating_sub(limit);
    Ok(entries[start..].to_vec())
}

/// The last `limit` entries of the default log, oldest → newest.
pub fn read_last(limit: usize) -> Result<Vec<Value>> {
    read_last_in(AUDIT_FILE, limit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_log(label: &str) -> String {
        let dir =
            std::env::temp_dir().join(format!("pandemic-audit-{}-{label}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.join("audit.jsonl").to_string_lossy().into_owned()
    }

    fn cleanup(path: &str) {
        let _ = fs::remove_dir_all(std::path::Path::new(path).parent().unwrap());
    }

    #[test]
    fn record_appends_and_enforces_0600() {
        let path = temp_log("write");
        record_in(&path, &json!({"event": "e1", "n": 1})).unwrap();
        record_in(&path, &json!({"event": "e2", "n": 2})).unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "audit log must be 0600");

        let raw = fs::read_to_string(&path).unwrap();
        assert_eq!(raw.lines().count(), 2, "one JSON object per line");
        cleanup(&path);
    }

    #[test]
    fn read_last_limits_and_preserves_order() {
        let path = temp_log("limit");
        for i in 0..5 {
            record_in(&path, &json!({"event": "apply", "n": i})).unwrap();
        }
        let last2 = read_last_in(&path, 2).unwrap();
        assert_eq!(last2.len(), 2);
        assert_eq!(last2[0]["n"], 3);
        assert_eq!(last2[1]["n"], 4);
        // limit larger than the log -> the whole log
        let all = read_last_in(&path, 99).unwrap();
        assert_eq!(all.len(), 5);
        cleanup(&path);
    }

    #[test]
    fn read_skips_malformed_lines() {
        let path = temp_log("malformed");
        record_in(&path, &json!({"event": "ok"})).unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"not-json\n")
            .unwrap();
        record_in(&path, &json!({"event": "also-ok"})).unwrap();

        let entries = read_last_in(&path, 10).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1]["event"], "also-ok");
        cleanup(&path);
    }

    #[test]
    fn read_missing_log_is_an_error() {
        let missing = temp_log("missing"); // never created
        assert!(read_last_in(&missing, 10).is_err());
        cleanup(&missing);
    }
}
