//! Asking a provider which models it actually serves.
//!
//! Ports the remote `/models` probe that upstream performs for user-added
//! compatible providers. It exists because the registry cannot know: an
//! `openai-compatible-*` or `anthropic-compatible-*` connection points at a host
//! chosen by its owner, so the only authority on its model list is the host itself.
//!
//! Without this, `/v1/models` reports **nothing** for such a connection —
//! `models_for_key` finds no registry row for the node id, and an owner who has not
//! typed a model list by hand gets an empty picker in every client that reads the
//! route.
//!
//! Both dialects answer the same shape. OpenAI's `GET /v1/models` and Anthropic's
//! `GET /v1/models` both return `{"data":[{"id":…}]}`, so one parser serves both;
//! `object` and `type` fields differ but nothing here reads them.
//!
//! **A failed probe is not an empty model list.** Emptying the list on a timeout
//! would take a working picker away from the user because their provider was briefly
//! slow, so every failure path leaves the caller's configured list alone and says
//! why. That asymmetry is the whole design.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;

/// How long a probe result is reused.
///
/// `/v1/models` is called by editors on startup and sometimes per completion, so an
/// uncached probe would put a provider round trip on a route that is expected to be
/// cheap. Five minutes is long enough to absorb that and short enough that a model
/// added upstream shows up without a restart.
pub const DEFAULT_TTL: Duration = Duration::from_secs(300);

/// How long to wait for a provider's model list.
///
/// Matches upstream's 5s abort. Deliberately shorter than a completion timeout: this
/// is a metadata call on a route a client is usually blocking its own startup on, and
/// a stale list beats a spinner.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// Marks a request as a router's own model probe (upstream
/// `INTERNAL_MODELS_FETCH_HEADER`).
///
/// **This is a loop guard, not telemetry.** A compatible provider's base URL is typed
/// by the owner and can perfectly well point at another router — or at this one. Both
/// would then probe each other on every `/v1/models`, forever. A router seeing this
/// header on its own `/v1/models` answers from configuration without probing, which
/// terminates the chain at one hop.
///
/// The name carries upstream's `9r` prefix on purpose: the two implementations have to
/// recognise *each other's* probes for the guard to work in a mixed deployment, and a
/// renamed header would silently reintroduce the loop.
pub const INTERNAL_PROBE_HEADER: &str = "x-9r-internal-models-fetch";

/// Why a probe produced no list.
///
/// Carried rather than collapsed into an `Option` so the dashboard can tell an owner
/// that their key is wrong, which is the common case and is not visible from an
/// empty list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeError {
    /// The provider rejected the credentials (401/403).
    Unauthorised { status: u16, message: String },
    /// The provider answered, but not with a model list.
    Unreadable { detail: String },
    /// Any other status.
    Status { status: u16, message: String },
    /// No answer within the timeout.
    Timeout { after_ms: u64 },
    /// The request never reached the provider.
    Unreachable { detail: String },
}

impl ProbeError {
    /// A one-line description for the dashboard and logs.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Unauthorised { status, message } => {
                format!("provider rejected the credentials ({status}): {message}")
            }
            Self::Unreadable { detail } => format!("model list was unreadable: {detail}"),
            Self::Status { status, message } => format!("provider returned {status}: {message}"),
            Self::Timeout { after_ms } => format!("no model list within {after_ms}ms"),
            Self::Unreachable { detail } => format!("could not reach the provider: {detail}"),
        }
    }

    /// Whether retrying sooner than the TTL could plausibly succeed.
    ///
    /// A rejected key will be rejected again until the owner changes it, so caching
    /// that verdict is right. A timeout is worth another attempt.
    #[must_use]
    pub const fn is_transient(&self) -> bool {
        matches!(self, Self::Timeout { .. } | Self::Unreachable { .. })
    }
}

/// One model the provider named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbedModel {
    pub id: String,
}

#[derive(Debug, Deserialize)]
struct ModelsBody {
    #[serde(default)]
    data: Vec<ModelEntry>,
    /// Some compatible servers return a bare array under `models` instead.
    #[serde(default)]
    models: Vec<ModelEntry>,
}

#[derive(Debug, Deserialize)]
struct ModelEntry {
    #[serde(default)]
    id: String,
    /// Ollama and a few others name it `name`.
    #[serde(default)]
    name: String,
    /// A third spelling upstream accepts (`model?.id || model?.name || model?.model`).
    #[serde(default)]
    model: String,
}

impl ModelEntry {
    /// The three spellings upstream accepts, in its order of preference.
    fn identifier(&self) -> Option<&str> {
        [self.id.trim(), self.name.trim(), self.model.trim()]
            .into_iter()
            .find(|candidate| !candidate.is_empty())
    }
}

/// Parse a provider's `/models` body into model ids.
///
/// Separate from the request so the shape handling is unit-testable without a
/// socket, and because the shapes in the wild vary more than the specs suggest.
pub fn parse_models(body: &str) -> Result<Vec<ProbedModel>, ProbeError> {
    let parsed: ModelsBody =
        serde_json::from_str(body).map_err(|error| ProbeError::Unreadable {
            detail: format!("not JSON: {error}"),
        })?;

    let mut models = Vec::new();
    for entry in parsed.data.iter().chain(parsed.models.iter()) {
        if let Some(id) = entry.identifier()
            && !models.iter().any(|seen: &ProbedModel| seen.id == id)
        {
            models.push(ProbedModel { id: id.to_owned() });
        }
    }

    if models.is_empty() {
        // A provider that answers 200 with no models is reported as unreadable rather
        // than as an empty success: an empty list from a working provider and a body
        // this parser did not understand are indistinguishable here, and the safe
        // reading is that the configured list should stand.
        return Err(ProbeError::Unreadable {
            detail: "no model ids in the response".to_owned(),
        });
    }
    Ok(models)
}

/// The URL a provider's model list lives at, given a connection's base URL.
///
/// Base URLs are stored with and without a trailing `/v1`, and sometimes with a full
/// endpoint path, because they are typed by hand. A `/chat/completions` or
/// `/messages` suffix is dropped rather than having `/models` appended to it.
#[must_use]
pub fn models_url(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    let stripped = trimmed
        .strip_suffix("/chat/completions")
        .or_else(|| trimmed.strip_suffix("/messages"))
        .or_else(|| trimmed.strip_suffix("/responses"))
        .unwrap_or(trimmed);
    format!("{}/models", stripped.trim_end_matches('/'))
}

/// A cached set of probe outcomes, keyed by connection.
///
/// Keyed by connection rather than by provider because a compatible node can have
/// several connections (a key pool), and one of them having a bad key says nothing
/// about the others.
#[derive(Debug, Clone, Default)]
pub struct ProbeCache {
    entries: Arc<Mutex<BTreeMap<String, CacheEntry>>>,
    ttl: Option<Duration>,
}

type CacheEntry = (Instant, Result<Vec<ProbedModel>, ProbeError>);

impl ProbeCache {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A cache with a non-default TTL, for tests that would otherwise have to wait.
    #[must_use]
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            entries: Arc::new(Mutex::new(BTreeMap::new())),
            ttl: Some(ttl),
        }
    }

    /// A result for this connection, if one landed within the TTL.
    ///
    /// A transient failure is not served from cache: a timeout should not suppress
    /// probing for the next five minutes, or one slow moment costs the owner their
    /// model list for far longer than the outage lasted.
    #[must_use]
    pub fn get(&self, connection_id: &str) -> Option<Result<Vec<ProbedModel>, ProbeError>> {
        let ttl = self.ttl.unwrap_or(DEFAULT_TTL);
        let mut entries = self.entries.lock().ok()?;
        let fresh = match entries.get(connection_id) {
            Some((at, result)) if at.elapsed() <= ttl => match result {
                Err(error) if error.is_transient() => None,
                other => Some(other.clone()),
            },
            Some(_) => {
                entries.remove(connection_id);
                None
            }
            None => None,
        };
        drop(entries);
        fresh
    }

    /// Record an outcome for reuse.
    pub fn put(&self, connection_id: &str, result: &Result<Vec<ProbedModel>, ProbeError>) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.insert(connection_id.to_owned(), (Instant::now(), result.clone()));
        }
    }

    /// Drop a connection's entry, so an owner who fixes a key sees the effect at once
    /// rather than after the TTL.
    pub fn invalidate(&self, connection_id: &str) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.remove(connection_id);
        }
    }

    /// How many entries are held. For tests and diagnostics.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries
            .lock()
            .map(|entries| entries.len())
            .unwrap_or_default()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Ask a provider for its model list.
///
/// Auth comes from [`crate::credentials::build_headers`], so a probe presents exactly
/// what a completion would: a provider that accepts one and rejects the other would
/// otherwise report a model list the router cannot then use.
///
/// `Content-Type` is dropped because this is a GET, and `Accept` is set to JSON so a
/// server that would otherwise negotiate SSE does not.
pub async fn probe_models(
    client: &reqwest::Client,
    provider: &str,
    credentials: &crate::Credentials,
    timeout: Duration,
) -> Result<Vec<ProbedModel>, ProbeError> {
    // No key means no probe, as upstream does. An unauthenticated GET to a provider that
    // requires auth returns a 401 that would then be cached as a verdict about the
    // owner's credentials — when the real problem is that they have not entered any.
    if credentials
        .api_key
        .as_deref()
        .unwrap_or_default()
        .is_empty()
    {
        return Err(ProbeError::Unauthorised {
            status: 0,
            message: "connection has no API key".to_owned(),
        });
    }

    let base = credentials
        .base_url()
        .ok_or_else(|| ProbeError::Unreachable {
            detail: "connection has no base URL to probe".to_owned(),
        })?;
    let url = models_url(base);

    let mut request = client.get(&url).timeout(timeout);
    for (name, value) in crate::credentials::build_headers(provider, credentials, false) {
        if name.eq_ignore_ascii_case("content-type") {
            continue;
        }
        request = request.header(name, value);
    }
    request = request
        .header("Accept", "application/json")
        // The loop guard. See INTERNAL_PROBE_HEADER: without it, a compatible node
        // pointed at another router makes the two probe each other indefinitely.
        .header(INTERNAL_PROBE_HEADER, "1");

    let response = match request.send().await {
        Ok(response) => response,
        Err(error) if error.is_timeout() => {
            return Err(ProbeError::Timeout {
                after_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
            });
        }
        Err(error) => {
            return Err(ProbeError::Unreachable {
                detail: error.to_string(),
            });
        }
    };

    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();

    if status == 401 || status == 403 {
        return Err(ProbeError::Unauthorised {
            status,
            message: first_line(&body),
        });
    }
    if !(200..300).contains(&status) {
        return Err(ProbeError::Status {
            status,
            message: first_line(&body),
        });
    }
    parse_models(&body)
}

/// A short, log-safe excerpt of a provider's error body.
///
/// Truncated because some providers return an HTML error page, and a whole one in a
/// dashboard field or a log line is unreadable.
fn first_line(body: &str) -> String {
    let line = body.trim().lines().next().unwrap_or_default().trim();
    if line.len() <= 200 {
        return line.to_owned();
    }
    let mut end = 200;
    while end > 0 && !line.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", line.get(..end).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_TTL, ProbeCache, ProbeError, ProbedModel, models_url, parse_models};
    use std::time::Duration;

    #[test]
    fn an_openai_model_list_parses() {
        let models = parse_models(
            r#"{"object":"list","data":[{"id":"gpt-4o","object":"model"},{"id":"gpt-4o-mini"}]}"#,
        )
        .expect("parses");
        assert_eq!(
            models,
            vec![
                ProbedModel {
                    id: "gpt-4o".to_owned()
                },
                ProbedModel {
                    id: "gpt-4o-mini".to_owned()
                },
            ]
        );
    }

    #[test]
    fn an_anthropic_model_list_parses_with_the_same_reader() {
        // Anthropic's /v1/models uses `data` and a `type` field rather than `object`.
        // Nothing here reads either, which is why one parser serves both dialects.
        let models = parse_models(
            r#"{"data":[{"type":"model","id":"claude-sonnet-4-5","display_name":"Sonnet"}]}"#,
        )
        .expect("parses");
        assert_eq!(
            models,
            vec![ProbedModel {
                id: "claude-sonnet-4-5".to_owned()
            }]
        );
    }

    #[test]
    fn a_models_key_and_a_name_field_are_both_accepted() {
        // Ollama-shaped: `models` rather than `data`, `name` rather than `id`.
        let models = parse_models(r#"{"models":[{"name":"llama3.2:latest"}]}"#).expect("parses");
        assert_eq!(
            models,
            vec![ProbedModel {
                id: "llama3.2:latest".to_owned()
            }]
        );
    }

    #[test]
    fn duplicate_ids_collapse() {
        let models =
            parse_models(r#"{"data":[{"id":"m"},{"id":"m"}],"models":[{"id":"m"}]}"#).expect("ok");
        assert_eq!(models.len(), 1);
    }

    #[test]
    fn a_body_that_is_not_json_is_unreadable_not_empty() {
        // The distinction matters: unreadable leaves the configured list standing,
        // whereas an empty success would replace it with nothing.
        let error = parse_models("<html>502 Bad Gateway</html>").expect_err("not JSON");
        assert!(matches!(error, ProbeError::Unreadable { .. }), "{error:?}");
    }

    #[test]
    fn a_two_hundred_with_no_models_is_unreadable_rather_than_an_empty_success() {
        let error = parse_models(r#"{"object":"list","data":[]}"#).expect_err("no models");
        assert!(matches!(error, ProbeError::Unreadable { .. }), "{error:?}");
    }

    #[test]
    fn blank_ids_are_skipped() {
        let error = parse_models(r#"{"data":[{"id":"  "},{"id":""}]}"#).expect_err("all blank");
        assert!(matches!(error, ProbeError::Unreadable { .. }), "{error:?}");
    }

    #[test]
    fn a_models_url_is_built_from_any_shape_of_base() {
        // All four are shapes users actually paste.
        assert_eq!(models_url("https://h/v1"), "https://h/v1/models");
        assert_eq!(models_url("https://h/v1/"), "https://h/v1/models");
        assert_eq!(
            models_url("https://h/v1/chat/completions"),
            "https://h/v1/models"
        );
        assert_eq!(models_url("https://h/v1/messages"), "https://h/v1/models");
        assert_eq!(models_url("https://h/v1/responses"), "https://h/v1/models");
    }

    #[test]
    fn a_cached_success_is_reused() {
        let cache = ProbeCache::new();
        let models = vec![ProbedModel { id: "m".to_owned() }];
        cache.put("conn_1", &Ok(models.clone()));
        assert_eq!(cache.get("conn_1"), Some(Ok(models)));
        assert_eq!(cache.get("conn_2"), None);
    }

    #[test]
    fn a_rejected_key_is_cached_but_a_timeout_is_not() {
        let cache = ProbeCache::new();
        cache.put(
            "bad_key",
            &Err(ProbeError::Unauthorised {
                status: 401,
                message: "nope".to_owned(),
            }),
        );
        cache.put("slow", &Err(ProbeError::Timeout { after_ms: 10_000 }));

        // A key stays wrong until the owner changes it, so caching that verdict avoids
        // probing a provider that will keep saying no.
        assert!(cache.get("bad_key").is_some());
        // A timeout says nothing about the next attempt. Suppressing probes for the whole
        // TTL would cost the owner their model list for far longer than the outage.
        assert!(cache.get("slow").is_none());
    }

    #[test]
    fn an_expired_entry_is_dropped() {
        let cache = ProbeCache::with_ttl(Duration::from_millis(1));
        cache.put("conn", &Ok(vec![ProbedModel { id: "m".to_owned() }]));
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(cache.get("conn"), None);
        assert!(cache.is_empty(), "the expired entry should not be retained");
    }

    #[test]
    fn invalidate_takes_effect_before_the_ttl() {
        let cache = ProbeCache::new();
        cache.put("conn", &Ok(vec![ProbedModel { id: "m".to_owned() }]));
        cache.invalidate("conn");
        assert_eq!(cache.get("conn"), None);
    }

    #[test]
    fn the_default_ttl_is_long_enough_to_absorb_editor_startup_polling() {
        // Guards the reason, not the number: an editor that lists models on startup and
        // again per completion must not produce a probe each time.
        assert!(DEFAULT_TTL >= Duration::from_secs(60));
    }

    #[test]
    fn every_failure_describes_itself_without_panicking() {
        for error in [
            ProbeError::Unauthorised {
                status: 401,
                message: "bad key".to_owned(),
            },
            ProbeError::Unreadable {
                detail: "not JSON".to_owned(),
            },
            ProbeError::Status {
                status: 500,
                message: "boom".to_owned(),
            },
            ProbeError::Timeout { after_ms: 1 },
            ProbeError::Unreachable {
                detail: "refused".to_owned(),
            },
        ] {
            assert!(!error.describe().is_empty());
        }
    }

    #[test]
    fn a_long_error_body_is_truncated_on_a_char_boundary() {
        let body = "é".repeat(400);
        let excerpt = super::first_line(&body);
        assert!(excerpt.len() <= 205, "len {}", excerpt.len());
        assert!(excerpt.ends_with('…'));
    }
}
