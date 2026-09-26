//! Epidemic spread history: the coordinator's on-disk audit log (5a).
//!
//! The coordinator records every spread — roster *and* broadcast — as one
//! JSON line (a `pandemic_protocol::SpreadRecord`) in the history file. The
//! same file is what `epidemic spreads` prints and what the console's REST
//! surface (`GET /api/epidemic/spreads`, 5b) serves; both read through this
//! module, so the CLI and the REST server agree on the file's location and
//! shape by construction.
//!
//! Lines that predate the JSONL format (pre-5a broadcast history, one TSV
//! line per broadcast) are still readable: they map onto the same record
//! shape with no per-node detail, so an operator's old history survives
//! instead of vanishing.

use anyhow::Context;
use pandemic_protocol::{Canary, SpreadMode, SpreadRecord};
use std::path::{Path, PathBuf};

/// Where spread history is stored: `$XDG_STATE_HOME/pandemic/…`, falling back
/// to `~/.local/state/pandemic/…`, then `/etc/pandemic/`.
pub fn default_history_path() -> PathBuf {
    if let Some(state) = std::env::var_os("XDG_STATE_HOME") {
        return PathBuf::from(state).join("pandemic/spread-history.log");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".local/state/pandemic/spread-history.log");
    }
    PathBuf::from("/etc/pandemic/spread-history.log")
}

/// Append one record as a JSON line, creating the file (and parent dir).
pub fn append_record(path: &Path, record: &SpreadRecord) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let line = serde_json::to_string(record).with_context(|| "encoding the spread record")?;
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    writeln!(f, "{line}")?;
    Ok(())
}

/// Read the most recent `limit` records (newest first) from the history file.
/// JSON lines (5a and later) parse straight into [`SpreadRecord`]; legacy TSV
/// lines (pre-5a broadcasts) map onto the same shape with no per-node detail.
/// A missing file is an empty history, not an error; malformed lines are
/// skipped, not fatal.
pub fn load_spreads(path: &Path, limit: usize) -> Vec<SpreadRecord> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|line| {
            let line = line.trim();
            if line.starts_with('{') {
                serde_json::from_str(line).ok()
            } else {
                legacy_tsv_record(line)
            }
        })
        .rev()
        .take(limit)
        .collect()
}

/// Map one pre-5a TSV history line onto the current record shape (13 fields:
/// timestamp, spread_id, stage, name, version, sha256, origin, criteria,
/// canary, callbacks, applied, failed, ok). Per-node detail did not exist
/// then, so `nodes` is empty.
fn legacy_tsv_record(line: &str) -> Option<SpreadRecord> {
    let f: Vec<&str> = line.split('\t').collect();
    if f.len() < 13 {
        return None;
    }
    Some(SpreadRecord {
        timestamp: f[0].parse().ok()?,
        spread_id: f[1].to_string(),
        mode: SpreadMode::Broadcast,
        stage: f[2].to_string(),
        name: f[3].to_string(),
        version: f[4].to_string(),
        sha256: f[5].to_string(),
        group: None,
        origin: if f[6].is_empty() {
            None
        } else {
            Some(f[6].to_string())
        },
        criteria: f[7]
            .split(';')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
        canary: legacy_canary(f[8]),
        nodes: Vec::new(),
        applied: f[10].parse().ok()?,
        failed: f[11].parse().ok()?,
        ok: f[12] == "true",
    })
}

/// A legacy `canary` TSV field: `pct=25`, `subset=KEY=VALUE`, or empty.
fn legacy_canary(s: &str) -> Option<Canary> {
    if let Some(pct) = s.strip_prefix("pct=") {
        pct.parse().ok().map(|pct| Canary::Percentage { pct })
    } else if let Some(criterion) = s.strip_prefix("subset=") {
        (!criterion.is_empty()).then(|| Canary::Subset {
            criterion: criterion.to_string(),
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pandemic_protocol::SpreadNodeResult;
    use tempfile::tempdir;

    fn sample_node(ok: bool, name: &str, addr: &str) -> SpreadNodeResult {
        SpreadNodeResult {
            name: name.to_string(),
            addr: addr.to_string(),
            ok,
            error: (!ok).then(|| format!("node {addr} failed to apply")),
        }
    }

    fn sample_record(ts: i64, sid: &str, ok: bool) -> SpreadRecord {
        SpreadRecord {
            timestamp: ts,
            spread_id: sid.to_string(),
            mode: SpreadMode::Broadcast,
            stage: if ok {
                "full".to_string()
            } else {
                "canary".to_string()
            },
            name: "webapp".to_string(),
            version: "1.2.3".to_string(),
            sha256: "cd".repeat(32),
            group: None,
            origin: Some("192.168.1.5".to_string()),
            criteria: vec!["role=edge".to_string(), "env=prod".to_string()],
            canary: Some(Canary::Percentage { pct: 25 }),
            nodes: vec![
                sample_node(true, "edge-1", "10.0.0.1:7711"),
                sample_node(true, "edge-3", "10.0.0.3:7711"),
                sample_node(false, "edge-2", "10.0.0.2:7711"),
            ],
            applied: 2,
            failed: 1,
            ok,
        }
    }

    #[test]
    fn spread_history_round_trips_newest_first() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("sub").join("history.log");

        append_record(&path, &sample_record(1000, "aaaa", true)).unwrap();
        append_record(&path, &sample_record(2000, "bbbb", false)).unwrap();
        append_record(&path, &sample_record(3000, "cccc", true)).unwrap();

        let all = load_spreads(&path, 10);
        assert_eq!(all.len(), 3);
        // Newest first: the reverse of append order.
        assert_eq!(all[0].spread_id, "cccc");
        assert_eq!(all[1].spread_id, "bbbb");
        assert_eq!(all[2].spread_id, "aaaa");

        // Fields survive the round-trip — including the structured per-node
        // detail that is the point of 5a.
        assert_eq!(all[0].timestamp, 3000);
        assert_eq!(all[0].name, "webapp");
        assert_eq!(all[0].version, "1.2.3");
        assert_eq!(
            all[0].criteria,
            vec!["role=edge".to_string(), "env=prod".to_string()]
        );
        assert_eq!(all[0].canary, Some(Canary::Percentage { pct: 25 }));
        assert_eq!(all[0].nodes.len(), 3);
        assert_eq!(all[0].nodes[2].addr, "10.0.0.2:7711");
        assert!(!all[0].nodes[2].ok);
        assert!(all[0].nodes[2].error.is_some());
        assert!(all[0].nodes[0].error.is_none());
        assert_eq!(all[0].applied, 2);
        assert_eq!(all[0].failed, 1);
        assert!(all[0].ok);
        assert!(!all[1].ok);

        // Limit caps the count, newest first.
        let one = load_spreads(&path, 1);
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].spread_id, "cccc");

        // A missing file yields an empty history, not an error.
        assert!(load_spreads(&dir.path().join("nope.log"), 10).is_empty());
    }

    #[test]
    fn legacy_tsv_lines_still_load() {
        // A pre-5a history file (TSV, no per-node detail) stays readable.
        let dir = tempdir().unwrap();
        let path = dir.path().join("history.log");
        std::fs::write(
            &path,
            "1750000000\t8f3a\tfull\twebapp\t1.2.3\tcdcd\t192.168.1.5\trole=edge;env=prod\tpct=25\t3\t2\t1\ttrue\n",
        )
        .unwrap();

        let rec = load_spreads(&path, 10);
        assert_eq!(rec.len(), 1);
        let r = &rec[0];
        assert_eq!(r.spread_id, "8f3a");
        assert_eq!(r.mode, SpreadMode::Broadcast);
        assert_eq!(r.origin.as_deref(), Some("192.168.1.5"));
        assert_eq!(
            r.criteria,
            vec!["role=edge".to_string(), "env=prod".to_string()]
        );
        assert_eq!(r.canary, Some(Canary::Percentage { pct: 25 }));
        assert!(r.nodes.is_empty(), "legacy lines carry no per-node detail");
        assert_eq!(r.applied, 2);
        assert_eq!(r.failed, 1);
        assert!(r.ok);

        // Malformed lines are skipped, not fatal.
        let dir2 = tempdir().unwrap();
        let path2 = dir2.path().join("history.log");
        std::fs::write(&path2, "not\tenough\tfields\n").unwrap();
        assert!(load_spreads(&path2, 10).is_empty());
    }

    #[test]
    fn legacy_and_json_lines_mix_in_one_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("history.log");
        // A legacy line first, then a JSON record — both load, newest first.
        std::fs::write(
            &path,
            "1750000000\tlegacy-1\tfull\twebapp\t1.0.0\tab\t10.0.0.9\t\t\t1\t1\t0\ttrue\n",
        )
        .unwrap();
        append_record(&path, &sample_record(1750000001, "json-1", true)).unwrap();

        let rec = load_spreads(&path, 10);
        assert_eq!(rec.len(), 2);
        assert_eq!(rec[0].spread_id, "json-1");
        assert_eq!(rec[1].spread_id, "legacy-1");
        assert_eq!(rec[1].canary, None);
    }

    #[test]
    fn history_path_honors_xdg_state_home() {
        let dir = tempdir().unwrap();
        std::env::set_var("XDG_STATE_HOME", dir.path());
        assert_eq!(
            default_history_path(),
            dir.path().join("pandemic/spread-history.log")
        );
    }
}
