//! Epidemic broadcast (increment 3): the shared, pure decision logic for
//! spread intents.
//!
//! The coordinator signs an intent; the node verifies it and self-selects.
//! Everything here is pure (no I/O, no network) so both sides — and the
//! tests — run it identically:
//!
//!   * the **intent token** — `HMAC-SHA256(epidemic_secret, spread_id)`, the
//!     group-membership proof carried in cleartext;
//!   * **freshness** — the replay/clock-skew window on `issued_at`;
//!   * **criteria** — the `key=value` targeting grammar and node-side
//!     matching (name / labels / agent capabilities);
//!   * **canary cohort** — percentage (stateless hash bucket) or named
//!     subset.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Result};
use pandemic_protocol::{Canary, SpreadIntent};

use crate::auth;

/// The intent schema version both sides of this build understand.
pub const INTENT_VERSION: u32 = 1;

/// How far in either direction an intent's `issued_at` may be from the node
/// clock and still be acted on (clock-skew tolerance; also the replay guard
/// for captured intents).
pub const INTENT_FRESHNESS: Duration = Duration::from_secs(60);

/// A node's identity as seen by the targeting criteria.
#[derive(Debug, Clone, Default)]
pub struct NodeIdentity {
    /// The node's identity name (`--name`, else the hostname).
    pub name: String,
    /// Operator-declared labels (`--label key=value`).
    pub labels: BTreeMap<String, String>,
    /// Capabilities reported by the node's local agent.
    pub capabilities: BTreeSet<String>,
}

impl NodeIdentity {
    /// True when every criterion in `criteria` matches this identity
    /// (AND semantics; an empty list matches every node).
    pub fn matches_all(&self, criteria: &[Criterion]) -> bool {
        criteria.iter().all(|c| c.matches(self))
    }
}

/// One targeting criterion — the parsed form of a `key=value` criterion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Criterion {
    /// The node's identity name: `name=web-1`.
    Name(String),
    /// An operator-declared node label: `role=edge`.
    Label { key: String, value: String },
    /// The node's local agent exposes the capability: `cap:mqtt=true`.
    Capability(String),
}

impl Criterion {
    /// Parse a `key=value` criterion. `name=` and `cap:` are reserved keys;
    /// everything else is a label.
    pub fn parse(criterion: &str) -> Result<Self> {
        let (key, value) = criterion.split_once('=').ok_or_else(|| {
            anyhow!("criterion '{criterion}' is not key=value (try role=edge, name=web-1, cap:mqtt=true)")
        })?;
        if key.is_empty() {
            bail!("criterion '{criterion}' has an empty key");
        }
        if key == "name" {
            if value.is_empty() {
                bail!("criterion 'name=' has an empty value");
            }
            return Ok(Criterion::Name(value.to_string()));
        }
        if let Some(cap) = key.strip_prefix("cap:") {
            if cap.is_empty() {
                bail!("criterion '{criterion}' names no capability");
            }
            if value != "true" {
                bail!("capability criteria are boolean: use 'cap:{cap}=true'");
            }
            return Ok(Criterion::Capability(cap.to_string()));
        }
        Ok(Criterion::Label {
            key: key.to_string(),
            value: value.to_string(),
        })
    }

    /// Whether this identity satisfies the criterion.
    pub fn matches(&self, identity: &NodeIdentity) -> bool {
        match self {
            Criterion::Name(name) => identity.name == *name,
            Criterion::Label { key, value } => identity.labels.get(key) == Some(value),
            Criterion::Capability(cap) => identity.capabilities.contains(cap),
        }
    }
}

/// Parse a list of `key=value` criteria (the coordinator validates them up
/// front; the node parses the ones it received).
pub fn parse_criteria(criteria: &[String]) -> Result<Vec<Criterion>> {
    criteria
        .iter()
        .map(|c| Criterion::parse(c))
        .collect::<Result<Vec<_>>>()
}

/// True when any criterion needs the local agent's capabilities (the node
/// fetches them lazily, only in that case).
pub fn needs_capabilities(criteria: &[Criterion]) -> bool {
    criteria
        .iter()
        .any(|c| matches!(c, Criterion::Capability(_)))
}

/// The intent token: `HMAC-SHA256(epidemic_secret, spread_id)`, hex. The
/// same primitive as the wire handshake — the `spread_id` plays the role of
/// the nonce.
pub fn intent_token(secret: &str, spread_id: &str) -> String {
    auth::sign(secret, spread_id)
}

/// Constant-time check that `intent.token` is the valid HMAC for the node's
/// own epidemic secret. True ⇔ the issuer holds the node's group secret.
pub fn verify_intent_token(secret: &str, intent: &SpreadIntent) -> bool {
    auth::verify(secret, &intent.spread_id, &intent.token)
}

/// Freshness: `issued_at` within [`INTENT_FRESHNESS`] of `now`, in either
/// direction (clock skew).
pub fn intent_is_fresh(intent: &SpreadIntent, now: SystemTime) -> bool {
    let now_secs = now
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let delta = (now_secs - intent.issued_at).unsigned_abs();
    delta <= INTENT_FRESHNESS.as_secs()
}

/// A stable 0–99 cohort bucket for one node in one spread: the first two
/// bytes of `HMAC-SHA256(secret, spread_id ‖ "|" ‖ node_name)`, mod 100.
/// Deterministic per (secret, spread, node) and uniform across nodes — the
/// basis of percentage canaries without a fleet view.
pub fn cohort_bucket(secret: &str, spread_id: &str, node_name: &str) -> u32 {
    let mac = auth::sign(secret, &format!("{spread_id}|{node_name}"));
    u16::from_str_radix(&mac[..4], 16).expect("HMAC hex") as u32 % 100
}

/// Whether `identity` is in the canary cohort of `canary`.
pub fn in_canary_cohort(
    canary: &Canary,
    secret: &str,
    spread_id: &str,
    identity: &NodeIdentity,
) -> Result<bool> {
    match canary {
        Canary::Percentage { pct } => {
            if *pct == 0 {
                return Ok(false);
            }
            if *pct >= 100 {
                return Ok(true);
            }
            Ok(cohort_bucket(secret, spread_id, &identity.name) < u32::from(*pct))
        }
        Canary::Subset { criterion } => {
            let c = Criterion::parse(criterion)?;
            Ok(c.matches(identity))
        }
    }
}

/// A fresh spread id: 8 random bytes, hex (16 chars). Unique enough for a
/// LAN spread; stable across a canary broadcast and its promotion.
pub fn generate_spread_id() -> String {
    hex::encode(rand::random::<[u8; 8]>())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intent(issued_at: i64, spread_id: &str) -> SpreadIntent {
        SpreadIntent {
            version: INTENT_VERSION,
            spread_id: spread_id.to_string(),
            stage: pandemic_protocol::SpreadStage::Full,
            issued_at,
            token: "ab".repeat(32),
            group: None,
            plan: pandemic_protocol::PlanIdentity {
                name: "webapp".into(),
                version: "1.0.0".into(),
                sha256: "cd".repeat(32),
            },
            criteria: vec![],
            canary: None,
            origin: "127.0.0.1".into(),
            callback_port: 1,
        }
    }

    #[test]
    fn token_verifies_with_the_right_secret_only() {
        let secret = "epi-secret";
        let mut i = intent(0, "spread-1");
        i.token = intent_token(secret, "spread-1");
        assert!(verify_intent_token(secret, &i));
        assert!(!verify_intent_token("other-secret", &i));
        // A token for a different spread id is no good either.
        i.token = intent_token(secret, "spread-2");
        assert!(!verify_intent_token(secret, &i));
    }

    #[test]
    fn freshness_window() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000);
        let fresh = intent(1_000_000, "s");
        assert!(intent_is_fresh(&fresh, now));
        // Skew in either direction is tolerated within the window…
        assert!(intent_is_fresh(&intent(1_000_000 + 59, "s"), now));
        assert!(intent_is_fresh(&intent(1_000_000 - 59, "s"), now));
        // …but a replay from a minute ago is not.
        assert!(!intent_is_fresh(&intent(1_000_000 - 61, "s"), now));
        assert!(!intent_is_fresh(&intent(1_000_000 + 61, "s"), now));
    }

    #[test]
    fn criterion_parsing() {
        assert_eq!(
            Criterion::parse("name=web-1").unwrap(),
            Criterion::Name("web-1".into())
        );
        assert_eq!(
            Criterion::parse("role=edge").unwrap(),
            Criterion::Label {
                key: "role".into(),
                value: "edge".into()
            }
        );
        assert_eq!(
            Criterion::parse("cap:mqtt=true").unwrap(),
            Criterion::Capability("mqtt".into())
        );
        // The `cap` label and `cap:` capability keys must not collide.
        assert_eq!(
            Criterion::parse("cap=true").unwrap(),
            Criterion::Label {
                key: "cap".into(),
                value: "true".into()
            }
        );

        assert!(Criterion::parse("no-equals").is_err());
        assert!(Criterion::parse("=v").is_err());
        assert!(Criterion::parse("name=").is_err());
        assert!(Criterion::parse("cap:=true").is_err());
        assert!(Criterion::parse("cap:mqtt=1").is_err());
    }

    fn identity() -> NodeIdentity {
        NodeIdentity {
            name: "edge-1".into(),
            labels: BTreeMap::from([
                ("role".to_string(), "edge".to_string()),
                ("env".to_string(), "prod".to_string()),
            ]),
            capabilities: BTreeSet::from(["mqtt".to_string()]),
        }
    }

    #[test]
    fn criteria_match_name_label_capability() {
        let id = identity();
        assert!(Criterion::parse("name=edge-1").unwrap().matches(&id));
        assert!(!Criterion::parse("name=edge-2").unwrap().matches(&id));
        assert!(Criterion::parse("role=edge").unwrap().matches(&id));
        assert!(!Criterion::parse("role=core").unwrap().matches(&id));
        assert!(!Criterion::parse("zone=dmz").unwrap().matches(&id));
        assert!(Criterion::parse("cap:mqtt=true").unwrap().matches(&id));
        assert!(!Criterion::parse("cap:rest=true").unwrap().matches(&id));
    }

    #[test]
    fn matches_all_is_and_semantics() {
        let id = identity();
        let ok = parse_criteria(&["role=edge".into(), "cap:mqtt=true".into()]).unwrap();
        assert!(id.matches_all(&ok));
        let bad = parse_criteria(&["role=edge".into(), "env=staging".into()]).unwrap();
        assert!(!id.matches_all(&bad));
        // Empty matches everyone.
        assert!(id.matches_all(&[]));
    }

    #[test]
    fn percentage_cohort_is_deterministic_and_bounded() {
        let secret = "epi-secret";
        let names = ["edge-1", "edge-2", "core-1", "a", "b", "c", "node-9"];
        for name in &names {
            let b = cohort_bucket(secret, "spread-1", name);
            assert!((0..100).contains(&b), "bucket {b} out of range for {name}");
            // Same node, same spread → same bucket.
            assert_eq!(b, cohort_bucket(secret, "spread-1", name));
            // Different spread → (generally) a different draw.
        }
        assert!(in_canary_cohort(
            &Canary::Percentage { pct: 100 },
            secret,
            "spread-1",
            &identity()
        )
        .unwrap());
        assert!(!in_canary_cohort(
            &Canary::Percentage { pct: 0 },
            secret,
            "spread-1",
            &identity()
        )
        .unwrap());
    }

    #[test]
    fn percentage_cohort_scales_with_pct() {
        let secret = "epi-secret";
        // 200 synthetic nodes: a 50% cohort should land in a sane band.
        let mut in_half = 0;
        for i in 0..200u32 {
            let id = NodeIdentity {
                name: format!("node-{i:03}"),
                ..Default::default()
            };
            if in_canary_cohort(&Canary::Percentage { pct: 50 }, secret, "spread-1", &id).unwrap() {
                in_half += 1;
            }
        }
        assert!(
            (60..140).contains(&in_half),
            "50% cohort of 200 nodes came out {in_half}; expected ~100"
        );
    }

    #[test]
    fn subset_cohort_matches_the_extra_criterion() {
        let id = identity();
        let canary = Canary::Subset {
            criterion: "env=prod".into(),
        };
        assert!(in_canary_cohort(&canary, "s", "spread-1", &id).unwrap());
        let canary = Canary::Subset {
            criterion: "env=staging".into(),
        };
        assert!(!in_canary_cohort(&canary, "s", "spread-1", &id).unwrap());
    }

    #[test]
    fn spread_ids_are_hex_and_sized() {
        let a = generate_spread_id();
        let b = generate_spread_id();
        assert_eq!(a.len(), 16);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b, "fresh spread ids should differ");
    }
}
