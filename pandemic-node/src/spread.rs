//! Broadcast (increment 3): the node side of multicast spread intents.
//!
//! The node listens for intents on the group and self-selects — protocol
//! version, group membership (token), replay (freshness), dedupe (applied
//! set), targeting (criteria), and the canary cohort. Only a selected node
//! dials the coordinator's callback listener, receives the `ApplyDeployment`
//! (checking the plan's integrity), and applies it through the local agent.
//!
//! The callback connection reuses the stable wire helpers for the handshake
//! (the node is the client there), then one `BufReader` per connection for
//! the line traffic — the same pattern as [`serve_node`](super::serve_node).

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::{Context, Result};
use pandemic_common::{
    in_canary_cohort, intent_is_fresh, needs_capabilities, parse_criteria, verify_intent_token,
    AgentClient, IntentListener, NodeIdentity, INTENT_VERSION,
};
use pandemic_protocol::{AgentRequest, Response, SpreadIntent, SpreadStage};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

/// Whether an intent is for this node, and why.
#[derive(Debug, PartialEq, Eq)]
pub enum IntentDecision {
    /// The node should apply the spread now.
    Apply,
    /// With a human-readable reason (for logs and tests).
    Ignore(String),
}

/// Pure selection policy, checked in this order: protocol version, group
/// membership (token), replay (freshness), dedupe (applied set), targeting
/// (criteria), canary cohort. Every check is local — the node never asks the
/// coordinator whether it is in scope.
pub fn decide_intent(
    intent: &SpreadIntent,
    secret: &str,
    identity: &NodeIdentity,
    now: SystemTime,
    applied: &[String],
) -> Result<IntentDecision> {
    if intent.version != INTENT_VERSION {
        return Ok(IntentDecision::Ignore(format!(
            "unsupported intent version {}",
            intent.version
        )));
    }
    if !verify_intent_token(secret, intent) {
        return Ok(IntentDecision::Ignore(
            "token does not match this node's group secret".into(),
        ));
    }
    if !intent_is_fresh(intent, now) {
        return Ok(IntentDecision::Ignore(
            "stale (outside the freshness window)".into(),
        ));
    }
    if applied.contains(&intent.spread_id) {
        return Ok(IntentDecision::Ignore("spread already applied".into()));
    }
    let criteria = parse_criteria(&intent.criteria)?;
    if !identity.matches_all(&criteria) {
        return Ok(IntentDecision::Ignore(
            "criteria do not match this node".into(),
        ));
    }
    if intent.stage == SpreadStage::Canary {
        if let Some(canary) = &intent.canary {
            if !in_canary_cohort(canary, secret, &intent.spread_id, identity)? {
                return Ok(IntentDecision::Ignore("not in the canary cohort".into()));
            }
        }
    }
    Ok(IntentDecision::Apply)
}

/// Parse repeatable `--label key=value` arguments into the node's label map.
pub fn parse_labels(raw: &[String]) -> Result<std::collections::BTreeMap<String, String>> {
    let mut out = std::collections::BTreeMap::new();
    for entry in raw {
        let (key, value) = entry
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("--label expects key=value, got '{entry}'"))?;
        if key.is_empty() || value.is_empty() {
            anyhow::bail!("--label expects non-empty key=value, got '{entry}'");
        }
        out.insert(key.to_string(), value.to_string());
    }
    Ok(out)
}

/// The intent recv loop: one intent at a time (re-sends are safe — the
/// applied set de-dupes, and an unselected spread id is never consumed).
pub async fn run_intent_listener(
    listener: IntentListener,
    name: String,
    labels: std::collections::BTreeMap<String, String>,
    epidemic_secret: String,
    agent_client: AgentClient,
) -> Result<()> {
    // Capabilities are fetched lazily (one agent round trip) only when an
    // intent's criteria actually test a `cap:` entry, then cached.
    let capabilities: Arc<Mutex<Option<BTreeSet<String>>>> = Arc::new(Mutex::new(None));
    let applied: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let mut buf = vec![0u8; 65_536];

    loop {
        let (n, _from) = listener.recv(&mut buf).await?;
        let intent: SpreadIntent = match serde_json::from_slice(&buf[..n]) {
            Ok(intent) => intent,
            Err(e) => {
                warn!("dropping undecodable spread intent: {e}");
                continue;
            }
        };
        if intent.spread_id.is_empty() {
            warn!("dropping spread intent with an empty spread_id");
            continue;
        }

        let criteria = match parse_criteria(&intent.criteria) {
            Ok(criteria) => criteria,
            Err(e) => {
                warn!("intent {}: unparseable criteria: {e}", intent.spread_id);
                continue;
            }
        };

        let capabilities = {
            let cached = capabilities.lock().await.clone();
            if let Some(cached) = cached {
                cached
            } else if needs_capabilities(&criteria) {
                let list = agent_client
                    .ping()
                    .await
                    .with_context(|| "capability probe for cap: criteria")?;
                let set: BTreeSet<String> = list.into_iter().collect();
                *capabilities.lock().await = Some(set.clone());
                set
            } else {
                BTreeSet::new()
            }
        };
        let identity = NodeIdentity {
            name: name.clone(),
            labels: labels.clone(),
            capabilities,
        };

        let applied_snapshot = applied.lock().await.clone();
        let decision = match decide_intent(
            &intent,
            &epidemic_secret,
            &identity,
            SystemTime::now(),
            &applied_snapshot,
        ) {
            Ok(decision) => decision,
            Err(e) => {
                warn!("intent {}: {e}", intent.spread_id);
                continue;
            }
        };

        match decision {
            IntentDecision::Apply => {
                info!(
                    "intent {}: selected — dialing callback at {}:{}",
                    intent.spread_id, intent.origin, intent.callback_port
                );
                match act_on_intent(&intent, &epidemic_secret, &agent_client).await {
                    Ok(response) => {
                        if matches!(response, Response::Success { .. }) {
                            info!("intent {}: applied", intent.spread_id);
                            applied.lock().await.push(intent.spread_id.clone());
                        } else {
                            warn!(
                                "intent {}: apply reported an error: {response:?}",
                                intent.spread_id
                            );
                        }
                    }
                    Err(e) => warn!("intent {}: apply failed: {e}", intent.spread_id),
                }
            }
            IntentDecision::Ignore(reason) => {
                debug!("intent {}: ignored ({reason})", intent.spread_id);
            }
        }
    }
}

/// The selected node's callback: dial the coordinator, handshake with the
/// epidemic secret (node = client, the stable wire helper), read the
/// `ApplyDeployment`, verify the plan integrity, apply it through the local
/// agent, and answer. The answer is always written — the coordinator gets a
/// per-node result either way.
pub async fn act_on_intent(
    intent: &SpreadIntent,
    epidemic_secret: &str,
    agent_client: &AgentClient,
) -> Result<Response> {
    let addr: SocketAddr = format!("{}:{}", intent.origin, intent.callback_port)
        .parse()
        .with_context(|| {
            format!(
                "parsing callback address {}:{:?}",
                intent.origin, intent.callback_port
            )
        })?;
    let stream = TcpStream::connect(addr)
        .await
        .with_context(|| format!("dialing the callback listener at {addr}"))?;
    let stream = pandemic_common::wire::authenticate_stream(stream, epidemic_secret)
        .await
        .with_context(|| "callback handshake (epidemic secret)")?;

    let (reader, mut writer) = tokio::io::split(stream);
    let mut buf_reader = BufReader::new(reader);
    let mut line = String::new();
    buf_reader.read_line(&mut line).await?;
    let wire_line = line.trim().to_string();
    let request: AgentRequest = serde_json::from_str(&wire_line)
        .map_err(|e| anyhow::anyhow!("expected an ApplyDeployment from the coordinator: {e}"))?;

    let response = if pandemic_common::sha256_hex(&wire_line) != intent.plan.sha256 {
        Response::error(format!(
            "plan integrity check failed: wire sha256 {} != intent plan sha256 {}",
            pandemic_common::sha256_hex(&wire_line),
            intent.plan.sha256
        ))
    } else {
        match agent_client.send_agent_request(&request).await {
            Ok(response) => response,
            Err(e) => Response::error(format!("agent error: {e}")),
        }
    };

    let mut payload = serde_json::to_string(&response)?;
    payload.push('\n');
    writer.write_all(payload.as_bytes()).await?;
    writer.flush().await?;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, UNIX_EPOCH};

    use pandemic_common::{
        cohort_bucket, intent_token, send_intent, sha256_hex, DEFAULT_MULTICAST_GROUP,
        DEFAULT_MULTICAST_PORT, INTENT_RESENDS,
    };
    use pandemic_protocol::{Canary, PlanIdentity};
    use serde_json::json;
    use tempfile::tempdir;
    use tokio::net::{TcpListener, UnixListener};

    fn now_secs() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    fn secret() -> &'static str {
        "epi-secret"
    }

    fn spread_id() -> &'static str {
        "8f3a1c02d4e5b6a7"
    }

    /// A valid Full intent for `secret()`/`spread_id()` that `identity` passes;
    /// `override_fn` tweaks individual fields.
    fn intent(override_fn: impl FnOnce(&mut SpreadIntent)) -> SpreadIntent {
        let mut i = SpreadIntent {
            version: INTENT_VERSION,
            spread_id: spread_id().to_string(),
            stage: SpreadStage::Full,
            issued_at: now_secs(),
            token: intent_token(secret(), spread_id()),
            group: Some("edge".to_string()),
            plan: PlanIdentity {
                name: "webapp".into(),
                version: "1.0.0".into(),
                sha256: "cd".repeat(32),
            },
            criteria: vec![],
            canary: None,
            origin: "127.0.0.1".into(),
            callback_port: 1,
        };
        override_fn(&mut i);
        i
    }

    fn identity(name: &str) -> NodeIdentity {
        NodeIdentity {
            name: name.to_string(),
            labels: BTreeMap::new(),
            capabilities: BTreeSet::new(),
        }
    }

    fn decide(i: &SpreadIntent, id: &NodeIdentity, applied: &[String]) -> IntentDecision {
        decide_intent(i, secret(), id, SystemTime::now(), applied).unwrap()
    }

    // ── decide_intent (pure selection policy) ────────────────────────────

    #[test]
    fn applies_when_everything_matches() {
        let i = intent(|_| {});
        assert!(matches!(
            decide(&i, &identity("edge-1"), &[]),
            IntentDecision::Apply
        ));
    }

    #[test]
    fn rejects_a_token_from_the_wrong_secret() {
        let mut i = intent(|_| {});
        i.token = intent_token("some-other-group", spread_id());
        assert!(matches!(
            decide(&i, &identity("edge-1"), &[]),
            IntentDecision::Ignore(r) if r.contains("token")
        ));
    }

    #[test]
    fn rejects_a_stale_intent() {
        let mut i = intent(|_| {});
        i.issued_at = now_secs() - 120;
        assert!(matches!(
            decide(&i, &identity("edge-1"), &[]),
            IntentDecision::Ignore(r) if r.contains("stale")
        ));
    }

    #[test]
    fn rejects_an_unsupported_version() {
        let mut i = intent(|_| {});
        i.version = INTENT_VERSION + 1;
        assert!(matches!(
            decide(&i, &identity("edge-1"), &[]),
            IntentDecision::Ignore(r) if r.contains("version")
        ));
    }

    #[test]
    fn rejects_a_spread_that_was_already_applied() {
        let i = intent(|_| {});
        assert!(matches!(
            decide(&i, &identity("edge-1"), &[spread_id().to_string()]),
            IntentDecision::Ignore(r) if r.contains("already applied")
        ));
    }

    #[test]
    fn criteria_select_and_exclude() {
        let id = NodeIdentity {
            labels: BTreeMap::from([("role".to_string(), "edge".to_string())]),
            ..identity("edge-1")
        };
        let hit = intent(|i| i.criteria = vec!["role=edge".into()]);
        assert!(matches!(decide(&hit, &id, &[]), IntentDecision::Apply));

        let miss = intent(|i| i.criteria = vec!["role=core".into()]);
        assert!(matches!(decide(&miss, &id, &[]), IntentDecision::Ignore(_)));

        // AND semantics: one criterion the node lacks vetoes the whole set.
        let and = intent(|i| i.criteria = vec!["role=edge".into(), "zone=dmz".into()]);
        assert!(matches!(decide(&and, &id, &[]), IntentDecision::Ignore(_)));
    }

    #[test]
    fn canary_percentage_includes_and_excludes_by_bucket() {
        let b = cohort_bucket(secret(), spread_id(), "edge-1");
        // pct == bucket  → `bucket < bucket` is false → out.
        let out = intent(|i| {
            i.stage = SpreadStage::Canary;
            i.canary = Some(Canary::Percentage { pct: b as u8 });
        });
        assert!(
            matches!(decide(&out, &identity("edge-1"), &[]), IntentDecision::Ignore(r) if r.contains("canary"))
        );
        // pct == bucket + 1 → `bucket < bucket + 1` is true → in.
        let in_cohort = intent(|i| {
            i.stage = SpreadStage::Canary;
            i.canary = Some(Canary::Percentage { pct: (b + 1) as u8 });
        });
        assert!(matches!(
            decide(&in_cohort, &identity("edge-1"), &[]),
            IntentDecision::Apply
        ));
    }

    #[test]
    fn canary_subset_includes_and_excludes() {
        let id = NodeIdentity {
            labels: BTreeMap::from([("role".to_string(), "canary".to_string())]),
            ..identity("edge-1")
        };
        let in_cohort = intent(|i| {
            i.stage = SpreadStage::Canary;
            i.canary = Some(Canary::Subset {
                criterion: "role=canary".into(),
            });
        });
        assert!(matches!(
            decide(&in_cohort, &id, &[]),
            IntentDecision::Apply
        ));

        let out = intent(|i| {
            i.stage = SpreadStage::Canary;
            i.canary = Some(Canary::Subset {
                criterion: "role=edge".into(),
            });
        });
        assert!(matches!(decide(&out, &id, &[]), IntentDecision::Ignore(_)));
    }

    #[test]
    fn parse_labels_round_trips_and_rejects_garbage() {
        let labels = parse_labels(&["role=edge".into(), "env=prod".into()]).unwrap();
        assert_eq!(labels.get("role"), Some(&"edge".to_string()));
        assert_eq!(labels.get("env"), Some(&"prod".to_string()));
        assert!(parse_labels(&["no-equals".into()]).is_err());
        assert!(parse_labels(&["=value".into()]).is_err());
    }

    // ── the callback path (node dials the coordinator) ───────────────────

    /// The coordinator's side of the callback connection: handshake (it is
    /// the server here), then send the exact `request_line` and read the
    /// node's `Response`. One persistent `BufReader` for the whole exchange —
    /// the same shape as `serve_node`.
    async fn serve_callback(
        stream: tokio::net::TcpStream,
        secret: &str,
        request_line: &str,
    ) -> Response {
        let (reader, mut writer) = tokio::io::split(stream);
        let mut br = BufReader::new(reader);
        let mut line = String::new();

        let challenge = pandemic_protocol::AuthChallenge {
            nonce: pandemic_common::auth::generate_nonce(),
        };
        let mut payload = serde_json::to_string(&challenge).unwrap();
        payload.push('\n');
        writer.write_all(payload.as_bytes()).await.unwrap();
        writer.flush().await.unwrap();

        br.read_line(&mut line).await.unwrap();
        let resp: pandemic_protocol::AuthResponse = serde_json::from_str(line.trim()).unwrap();
        if !pandemic_common::auth::verify(secret, &resp.nonce, &resp.signature) {
            return Response::error("callback handshake: bad signature");
        }

        // The wire line is newline-terminated; the intent's `plan.sha256` is
        // over this exact line *without* the terminator (the node trims it).
        let mut payload = request_line.to_string();
        payload.push('\n');
        writer.write_all(payload.as_bytes()).await.unwrap();
        writer.flush().await.unwrap();

        line.clear();
        br.read_line(&mut line).await.unwrap();
        serde_json::from_str(line.trim()).unwrap()
    }

    /// A stand-in local agent: handshake, then one request per connection,
    /// answering `ApplyDeployment` with `Success`. Counts `ApplyDeployment`s.
    async fn spawn_fake_agent(socket_path: &Path, agent_secret: &str, count: Arc<AtomicUsize>) {
        let listener = UnixListener::bind(socket_path).unwrap();
        let secret = agent_secret.to_string();
        tokio::spawn(async move {
            loop {
                let (stream, _) = match listener.accept().await {
                    Ok(x) => x,
                    Err(_) => break,
                };
                let secret = secret.clone();
                let count = count.clone();
                tokio::spawn(async move {
                    let (reader, mut writer) = tokio::io::split(stream);
                    let mut br = BufReader::new(reader);
                    let mut line = String::new();
                    let challenge = pandemic_protocol::AuthChallenge {
                        nonce: pandemic_common::auth::generate_nonce(),
                    };
                    let mut payload = serde_json::to_string(&challenge).unwrap();
                    payload.push('\n');
                    writer.write_all(payload.as_bytes()).await.unwrap();
                    writer.flush().await.unwrap();
                    br.read_line(&mut line).await.unwrap();
                    let resp: pandemic_protocol::AuthResponse =
                        serde_json::from_str(line.trim()).unwrap();
                    line.clear();
                    if !pandemic_common::auth::verify(&secret, &resp.nonce, &resp.signature) {
                        return;
                    }
                    br.read_line(&mut line).await.unwrap();
                    let request: AgentRequest = serde_json::from_str(line.trim()).unwrap();
                    let response = match &request {
                        AgentRequest::GetCapabilities => {
                            Response::success_with_data(json!({"capabilities": ["fake-cap"]}))
                        }
                        AgentRequest::ApplyDeployment { .. } => {
                            count.fetch_add(1, Ordering::SeqCst);
                            Response::success()
                        }
                        _ => Response::success_with_data(json!({"reached": true})),
                    };
                    let mut payload = serde_json::to_string(&response).unwrap();
                    payload.push('\n');
                    writer.write_all(payload.as_bytes()).await.unwrap();
                    writer.flush().await.unwrap();
                });
            }
        });
    }

    #[tokio::test]
    async fn integrity_mismatch_is_rejected_without_dialing_the_agent() {
        let dir = tempdir().unwrap();
        let agent_sock = dir.path().join("agent.sock");
        // Point the node at an agent socket that is never created: if the
        // integrity check wrongly passes, the dial below will fail.
        let agent_client = AgentClient::with_socket_path(&agent_sock).with_secret("agent-secret");

        let request = AgentRequest::ApplyDeployment {
            name: "webapp".into(),
            version: "1.0.0".into(),
            variables: BTreeMap::new(),
            infections: vec![],
        };
        let request_line = serde_json::to_string(&request).unwrap();
        // Deliberately the wrong digest for the wire line that follows.
        let mut intent = intent(|_| {});
        intent.plan.sha256 = "00".repeat(32);
        intent.origin = "127.0.0.1".into();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        intent.callback_port = port;
        let req = request_line.clone();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            serve_callback(stream, secret(), &req).await
        });

        let response = act_on_intent(&intent, secret(), &agent_client)
            .await
            .unwrap();
        assert!(
            matches!(&response, Response::Error { message } if message.contains("integrity")),
            "expected an integrity error, got {response:?}"
        );
        drop(server);
    }

    #[tokio::test]
    async fn loopback_broadcast_applies_exactly_once_across_resends() {
        let dir = tempdir().unwrap();
        let agent_sock = dir.path().join("agent.sock");
        let agent_count = Arc::new(AtomicUsize::new(0));
        spawn_fake_agent(&agent_sock, "agent-secret", agent_count.clone()).await;

        let request = AgentRequest::ApplyDeployment {
            name: "webapp".into(),
            version: "1.0.0".into(),
            variables: BTreeMap::new(),
            infections: vec![],
        };
        let request_line = serde_json::to_string(&request).unwrap();
        let plan_sha = sha256_hex(&request_line);
        let mut intent = intent(|i| i.plan.sha256 = plan_sha.clone());
        intent.origin = "127.0.0.1".into();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        intent.callback_port = port;
        let responses: Arc<std::sync::Mutex<Vec<Response>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        {
            let responses = responses.clone();
            let req = request_line.clone();
            tokio::spawn(async move {
                loop {
                    let (stream, _) = match listener.accept().await {
                        Ok(x) => x,
                        Err(_) => break,
                    };
                    let resp = serve_callback(stream, secret(), &req).await;
                    responses.lock().unwrap().push(resp);
                }
            });
        }

        let lo = std::net::Ipv4Addr::new(127, 0, 0, 1);
        let mcast =
            IntentListener::join(DEFAULT_MULTICAST_GROUP, DEFAULT_MULTICAST_PORT, lo).unwrap();
        let agent_client = AgentClient::with_socket_path(agent_sock).with_secret("agent-secret");
        tokio::spawn(async move {
            let _ = run_intent_listener(
                mcast,
                "edge-1".into(),
                BTreeMap::new(),
                secret().to_string(),
                agent_client,
            )
            .await;
        });

        for _ in 0..INTENT_RESENDS {
            send_intent(
                DEFAULT_MULTICAST_GROUP,
                DEFAULT_MULTICAST_PORT,
                Some(lo),
                &intent,
            )
            .unwrap();
        }

        let deadline = std::time::Instant::now() + Duration::from_secs(6);
        while responses.lock().unwrap().is_empty() && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        // Give the trailing re-sends a beat to be de-duplicated (ignored).
        tokio::time::sleep(Duration::from_millis(300)).await;

        let got = responses.lock().unwrap();
        assert_eq!(
            got.len(),
            1,
            "node must apply the spread exactly once, got {got:?}"
        );
        assert!(
            matches!(&got[0], Response::Success { .. }),
            "expected Success, got {got:?}"
        );
        drop(got);
        assert_eq!(
            agent_count.load(Ordering::SeqCst),
            1,
            "agent must be dialed exactly once"
        );
    }
}
