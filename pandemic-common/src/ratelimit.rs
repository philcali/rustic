//! Epidemic spread rate limiting (increment 4c): prevent spread storms.
//!
//! A "spread storm" is too many spreads being *started* in a short window — an
//! operator misfire, a retry loop that lost its guard, or abuse of the
//! console/REST trigger. Instead of letting every one of them fan out to every
//! node, the coordinator consults a shared sliding-window limiter *before* it
//! touches any node: at most `max_in_window` spread starts are allowed within
//! any rolling `window`. A spread that would exceed the limit is refused up
//! front — reported clearly (CLI: non-zero exit; REST: HTTP 429 with a
//! retry-after) — rather than silently queued, dropped, or fanning out.
//!
//! The limiter is file-backed (like the spread history in [`crate::history`])
//! so the CLI and the REST server — different processes — share one limit and
//! one source of truth, and the window survives a restart. An advisory
//! `flock` on the state file makes the check-and-record step atomic across
//! those processes.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};

/// Generous default: only a real storm (a runaway loop or abuse), not a human
/// operator, trips it. Override per spread with CLI `--rate-limit MAX/WINDOW`
/// or the REST `rate_limit_max` / `rate_limit_window_secs` fields; a max of `0`
/// disables the limit entirely.
pub const DEFAULT_RATE_LIMIT_MAX: u32 = 100;
/// Rolling window length (seconds) for the default limit.
pub const DEFAULT_RATE_LIMIT_WINDOW_SECS: u64 = 60;

/// Where the rolling window's start timestamps live, shared across processes:
/// `$XDG_STATE_HOME/pandemic/…`, else `~/.local/state/pandemic/…`, else
/// `/etc/pandemic/` (same resolution as the spread history).
pub fn default_rate_limit_path() -> PathBuf {
    if let Some(state) = std::env::var_os("XDG_STATE_HOME") {
        return PathBuf::from(state).join("pandemic/spread-ratelimit.json");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".local/state/pandemic/spread-ratelimit.json");
    }
    PathBuf::from("/etc/pandemic/spread-ratelimit.json")
}

/// A spreading rate limit: at most `max_in_window` spread starts within any
/// rolling `window` of `window_secs`. `max_in_window == 0` disables the limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpreadRateLimit {
    /// Maximum spread starts allowed inside the rolling window (`0` = unlimited).
    pub max_in_window: u32,
    /// Length of the rolling window, in seconds.
    pub window_secs: u64,
    /// Where the window's start timestamps live (shared across processes).
    pub path: PathBuf,
}

impl Default for SpreadRateLimit {
    fn default() -> Self {
        Self {
            max_in_window: DEFAULT_RATE_LIMIT_MAX,
            window_secs: DEFAULT_RATE_LIMIT_WINDOW_SECS,
            path: default_rate_limit_path(),
        }
    }
}

impl SpreadRateLimit {
    /// A limit with an explicit policy and state-file path.
    pub fn new(max_in_window: u32, window_secs: u64, path: impl Into<PathBuf>) -> Self {
        Self {
            max_in_window,
            window_secs,
            path: path.into(),
        }
    }

    /// The default policy at the default state path.
    pub fn default_policy() -> Self {
        Self {
            max_in_window: DEFAULT_RATE_LIMIT_MAX,
            window_secs: DEFAULT_RATE_LIMIT_WINDOW_SECS,
            path: default_rate_limit_path(),
        }
    }

    /// A limit with the default policy at a specific state path (tests).
    pub fn with_path(max_in_window: u32, window_secs: u64, path: impl Into<PathBuf>) -> Self {
        Self::new(max_in_window, window_secs, path)
    }

    /// No limit (used by tests and by an explicit `--rate-limit 0/…`).
    pub fn disabled(path: impl Into<PathBuf>) -> Self {
        Self::new(0, 0, path)
    }

    /// Is the limit active? A max of `0` means unlimited.
    pub fn is_disabled(&self) -> bool {
        self.max_in_window == 0
    }

    /// Record a spread start, refusing (`Err(RateLimited)`) if it would exceed
    /// the limit within the rolling window. A no-op when the limit is disabled.
    ///
    /// Safe to call from any number of processes: an advisory `flock` makes
    /// the check-and-record step atomic on the shared state file.
    pub fn record(&self) -> Result<()> {
        if self.is_disabled() {
            return Ok(());
        }
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let mut file = File::options()
            .create(true)
            .read(true)
            .write(true)
            // Read the existing stamps before truncating them below (a fresh
            // file is empty), so keep the old content through the read step.
            .truncate(false)
            .open(&self.path)
            .with_context(|| format!("opening rate-limit state {}", self.path.display()))?;
        rustix::fs::flock(&file, rustix::fs::FlockOperation::LockExclusive)
            .with_context(|| format!("locking rate-limit state {}", self.path.display()))?;
        let result = self.record_locked(&mut file);
        // Always release, even on the error path.
        let _ = rustix::fs::flock(&file, rustix::fs::FlockOperation::Unlock);
        result
    }

    /// The locked check-and-record step (caller holds the exclusive `flock`).
    fn record_locked(&self, file: &mut File) -> Result<()> {
        use crate::coordinator::now_unix_secs;
        let now = now_unix_secs();
        let window = self.window_secs as i64;
        let cutoff = now.saturating_sub(window);

        file.seek(SeekFrom::Start(0))?;
        let mut buf = String::new();
        file.read_to_string(&mut buf)?;
        let mut stamps: Vec<i64> = if buf.trim().is_empty() {
            Vec::new()
        } else {
            serde_json::from_str(&buf).unwrap_or_default()
        };
        // A start made at `t` counts until `t + window`: keep only the stamps
        // still inside the window (exclusive lower bound), so a slot frees
        // exactly `window` seconds after it was used — not `window + 1`.
        stamps.retain(|&t| t > cutoff);

        if (stamps.len() as u32) >= self.max_in_window {
            // Refused: the window is full. The oldest stamp frees a slot after
            // the window elapses — that is the retry-after we report.
            let oldest = *stamps.first().unwrap_or(&now);
            let wait_secs = (oldest + window - now).max(0);
            return Err(RateLimited {
                retry_after: Duration::from_secs(wait_secs.max(1) as u64),
                max_in_window: self.max_in_window,
                window_secs: self.window_secs,
            }
            .into());
        }

        stamps.push(now);
        file.seek(SeekFrom::Start(0))?;
        file.set_len(0)?;
        file.write_all(serde_json::to_string(&stamps)?.as_bytes())?;
        file.sync_all().ok();
        Ok(())
    }
}

/// A spread start refused because the rate limit is reached. `retry_after` is
/// how long to wait before the rolling window frees a slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimited {
    /// How long to wait before another spread is allowed.
    pub retry_after: Duration,
    /// The limit that was hit (for the message).
    pub max_in_window: u32,
    /// The window that was hit (for the message).
    pub window_secs: u64,
}

impl std::fmt::Display for RateLimited {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "spread rate limit reached: {} spread(s) already started in the last {}s; \
             try again in ~{}s",
            self.max_in_window,
            self.window_secs,
            self.retry_after.as_secs().max(1)
        )
    }
}

impl std::error::Error for RateLimited {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    fn tmp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("pandemic-ratelimit-{name}-{}", std::process::id()))
    }

    #[test]
    fn allows_under_limit() {
        let p = tmp("allow");
        let rl = SpreadRateLimit::new(3, 60, &p);
        for _ in 0..3 {
            assert!(rl.record().is_ok(), "within the limit must be allowed");
        }
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn refuses_over_limit_with_retry_after() {
        let p = tmp("refuse");
        let rl = SpreadRateLimit::new(2, 60, &p);
        assert!(rl.record().is_ok());
        assert!(rl.record().is_ok());
        let err = rl.record().err().expect("third start must be refused");
        let rate_limited = err
            .downcast_ref::<RateLimited>()
            .expect("the refusal is a RateLimited error");
        assert!(rate_limited.retry_after >= Duration::from_secs(1));
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn window_frees_slots() {
        let p = tmp("window");
        let rl = SpreadRateLimit::new(1, 1, &p);
        assert!(rl.record().is_ok());
        assert!(
            rl.record().is_err(),
            "within the 1s window the second is refused"
        );
        // Sleep comfortably past the 1s window so the whole-second timestamp
        // has advanced (robust under a loaded CI runner).
        std::thread::sleep(Duration::from_millis(1500));
        assert!(
            rl.record().is_ok(),
            "after the window elapses a slot is free again"
        );
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn disabled_is_unlimited() {
        let p = tmp("disabled");
        let rl = SpreadRateLimit::disabled(&p);
        for _ in 0..100 {
            assert!(rl.record().is_ok());
        }
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn shared_across_instances() {
        // Two limiter instances on the same state file share one window — the
        // CLI and the REST server never each get their own budget.
        let p = tmp("shared");
        let a = SpreadRateLimit::new(2, 60, p.clone());
        let b = SpreadRateLimit::new(2, 60, p.clone());
        assert!(a.record().is_ok());
        assert!(b.record().is_ok());
        assert!(a.record().is_err(), "the shared window is exhausted");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn concurrent_starts_respect_limit() {
        // Many threads race the same file-backed window: exactly `max` may win.
        let max = 5u32;
        let p = tmp("concurrent");
        let rl = SpreadRateLimit::new(max, 60, &p);
        let n_threads = 20usize;
        let barrier = Arc::new(Barrier::new(n_threads));
        let mut handles = Vec::new();
        for _ in 0..n_threads {
            let rl = rl.clone();
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                rl.record().is_ok()
            }));
        }
        let allowed = handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .filter(|ok| *ok)
            .count();
        assert_eq!(
            allowed as u32, max,
            "exactly `max` concurrent starts must be allowed (got {allowed})"
        );
        std::fs::remove_file(&p).ok();
    }
}
