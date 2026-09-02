//! Stable per-connection session ids, for providers that key a prompt cache by one.
//!
//! Codex and Antigravity both cache prompts against a session id. An id that changes per request is not
//! a failure — the request succeeds — but it discards the cache on every turn, so a long conversation is
//! re-billed in full each time. That makes it exactly the kind of bug that never surfaces as an error.
//!
//! Ports the observable behaviour of `open-sse/utils/sessionManager.js`. Upstream generates an id once
//! per connection and holds it for the process lifetime; a restart changes it, which mirrors the vendor
//! binary being restarted. Reproduced here, with the cache bounded and aged.
//!
//! Upstream also has a step this does not: hashing the conversation's assistant text to recover a
//! session across a *client* restart. Deliberately not ported yet — getting it wrong silently merges two
//! different conversations into one cache entry, and a per-connection id is already correct, just less
//! sticky.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long an unused session id is kept.
const SESSION_MAX_AGE: Duration = Duration::from_secs(3600);

/// How many connections are remembered before the least recently used is dropped.
const MAX_SESSIONS: usize = 1000;

struct Session {
    id: String,
    last_used: Instant,
}

fn store() -> &'static Mutex<HashMap<String, Session>> {
    static STORE: OnceLock<Mutex<HashMap<String, Session>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The session id for one connection, generating and remembering one on first use.
///
/// `scope` separates providers, so one connection used for two providers does not share an id.
pub(crate) fn for_connection(scope: &str, connection_id: &str) -> String {
    if connection_id.is_empty() {
        // Nothing to key on. A fresh id is right here: one request's worth of cache beats every
        // anonymous request sharing a single entry.
        return generate();
    }
    let key = format!("{scope}:{connection_id}");
    let Ok(mut store) = store().lock() else {
        return generate();
    };

    if let Some(session) = store.get_mut(&key) {
        if session.last_used.elapsed() <= SESSION_MAX_AGE {
            session.last_used = Instant::now();
            return session.id.clone();
        }
        store.remove(&key);
    }

    let id = generate();
    if store.len() >= MAX_SESSIONS
        && let Some(oldest) = store
            .iter()
            .min_by_key(|(_key, session)| session.last_used)
            .map(|(key, _session)| key.clone())
    {
        store.remove(&oldest);
    }
    store.insert(
        key,
        Session {
            id: id.clone(),
            last_used: Instant::now(),
        },
    );
    id
}

/// A client-supplied session id, if the request carries one.
///
/// Preferred over a generated one: a client managing its own conversation ids knows better than this
/// router which requests belong together.
pub(crate) fn from_body(body: &serde_json::Value) -> Option<String> {
    for key in ["session_id", "sessionId", "prompt_cache_key"] {
        if let Some(value) = body
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(value.to_owned());
        }
    }
    None
}

/// Resolve a session id in upstream's order: the client's, then the workspace, then per-connection.
pub(crate) fn resolve(
    scope: &str,
    body: &serde_json::Value,
    workspace_id: Option<&str>,
    connection_id: &str,
) -> String {
    if let Some(client) = from_body(body) {
        return client;
    }
    if let Some(workspace) = workspace_id
        .map(str::trim)
        .filter(|workspace| !workspace.is_empty())
    {
        return workspace.to_owned();
    }
    for_connection(scope, connection_id)
}

/// A fresh id in the shape the vendor binaries use: random hex plus a millisecond timestamp.
fn generate() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis());
    format!("{}{millis}", super::session_id().replace('-', ""))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{for_connection, resolve};

    #[test]
    fn one_connection_keeps_one_id_across_requests() {
        // The whole point: Codex keys its prompt cache by this, so an id that changes per request
        // discards the cache every turn. A cost, not an error, and so easy to miss.
        let first = for_connection("codex", "conn-stable-1");
        let second = for_connection("codex", "conn-stable-1");
        assert_eq!(first, second);
    }

    #[test]
    fn different_connections_and_scopes_do_not_share_an_id() {
        assert_ne!(
            for_connection("codex", "conn-a"),
            for_connection("codex", "conn-b"),
            "two accounts must not share a prompt cache"
        );
        // One connection used for two providers is two caches.
        assert_ne!(
            for_connection("codex", "conn-shared"),
            for_connection("antigravity", "conn-shared")
        );
    }

    #[test]
    fn an_anonymous_request_gets_a_fresh_id_rather_than_a_shared_one() {
        // Keying nothing on an empty connection id would put every anonymous request in one cache
        // entry, which is worse than no caching at all.
        assert_ne!(for_connection("codex", ""), for_connection("codex", ""));
    }

    #[test]
    fn a_client_supplied_session_wins_over_a_generated_one() {
        assert_eq!(
            resolve(
                "codex",
                &json!({ "session_id": "client-owned-7" }),
                Some("ws-1"),
                "conn-1"
            ),
            "client-owned-7"
        );
    }

    #[test]
    fn a_workspace_outranks_a_per_connection_id() {
        assert_eq!(
            resolve("codex", &json!({}), Some("ws-42"), "conn-1"),
            "ws-42"
        );
    }

    #[test]
    fn a_blank_workspace_falls_through_to_the_connection() {
        assert_eq!(
            resolve("codex", &json!({}), Some("   "), "conn-fallthrough"),
            for_connection("codex", "conn-fallthrough")
        );
    }

    #[test]
    fn an_id_is_shaped_like_the_vendor_binaries() {
        // Hex plus a millisecond timestamp, as upstream's `rs() + Date.now()` produces.
        let id = for_connection("codex", "conn-shape");
        assert!(id.len() > 32, "{id}");
        assert!(
            id.chars()
                .all(|character| character.is_ascii_alphanumeric()),
            "{id}"
        );
        assert!(
            id.chars()
                .rev()
                .take(10)
                .all(|character| character.is_ascii_digit()),
            "the tail is a timestamp: {id}"
        );
    }
}
