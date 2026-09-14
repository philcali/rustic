//! `pandemic-cli audit` — view the host's audit log
//! (ideas/deployments.md, phase 8).
//!
//! The log is 0600 root-only, so this is effectively a root command (or
//! `sudo pandemic-cli audit`): it shows exactly what the agent applied,
//! uninstalled, or removed, step by step, including the users/groups a
//! failed install left behind.

use anyhow::Result;
use pandemic_common::audit;

pub fn handle_audit_command(limit: usize, as_json: bool) -> Result<()> {
    let entries = audit::read_last(limit.max(1)).map_err(|e| {
        anyhow::anyhow!("{e}\n  hint: the audit log is 0600 root — try `sudo pandemic-cli audit`")
    })?;

    if entries.is_empty() {
        println!("(audit log is empty)");
        return Ok(());
    }

    if as_json {
        for entry in &entries {
            println!("{}", serde_json::to_string_pretty(entry)?);
        }
        return Ok(());
    }

    for entry in &entries {
        let ts = entry.get("ts").and_then(|v| v.as_str()).unwrap_or("?");
        let event = entry.get("event").and_then(|v| v.as_str()).unwrap_or("?");
        let name = entry.get("name").and_then(|v| v.as_str()).unwrap_or("-");
        let outcome = entry.get("outcome").and_then(|v| v.as_str()).unwrap_or("?");
        let detail = entry
            .get("error")
            .and_then(|v| v.as_str())
            .map(|e| format!("  {e}"))
            .unwrap_or_default();
        println!("{ts}  {event:<20} {name:<24} {outcome}{detail}");
    }
    Ok(())
}
