//! Loopback client for reading usage data from `nullrouter-state`.
//!
//! The usage dashboard surface lives here, but the records live in the state
//! service, so these endpoints read across loopback rather than duplicating
//! storage.

use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

/// Default loopback address of `nullrouter-state`.
const DEFAULT_STATE_ADDR: &str = "127.0.0.1:20134";
/// Dashboard reads must not hang the UI.
const STATE_TIMEOUT: Duration = Duration::from_secs(5);

/// One recorded request, as the dashboard consumes it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageRecord {
    pub id: String,
    pub timestamp: u64,
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub connection_id: Option<String>,
    #[serde(default)]
    pub endpoint: Option<String>,
    pub status: String,
    #[serde(default)]
    pub status_code: Option<u16>,
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
    #[serde(default)]
    pub latency_ms: u64,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RecordsResponse {
    #[serde(default)]
    records: Vec<UsageRecord>,
}

/// Default loopback address of `nullrouter-runtime`.
const DEFAULT_RUNTIME_ADDR: &str = "127.0.0.1:20132";
/// A dashboard chat turn can legitimately take a while.
const RUNTIME_TIMEOUT: Duration = Duration::from_secs(300);

/// Forwards dashboard chat turns to `nullrouter-runtime`.
///
/// The runtime owns provider execution, so the dashboard's chat endpoint
/// proxies to it rather than duplicating the pipeline.
#[derive(Debug, Clone)]
pub struct RuntimeClient {
    client: reqwest::Client,
    base: String,
}

impl Default for RuntimeClient {
    fn default() -> Self {
        Self::from_env()
    }
}

/// An upstream runtime reply, ready to relay to the caller.
#[derive(Debug)]
pub(crate) struct ForwardedResponse {
    pub status: u16,
    pub content_type: String,
    pub body: String,
}

impl RuntimeClient {
    /// Client pointed at `NULLROUTER_RUNTIME_ADDR`, or the default loopback port.
    pub fn from_env() -> Self {
        let addr = std::env::var("NULLROUTER_RUNTIME_ADDR")
            .unwrap_or_else(|_| DEFAULT_RUNTIME_ADDR.to_owned());
        Self::new(&addr)
    }

    /// Client for an explicit `host:port`.
    pub fn new(addr: &str) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(RUNTIME_TIMEOUT)
                .build()
                .unwrap_or_default(),
            base: format!("http://{addr}"),
        }
    }

    /// Forward a chat request to the runtime's OpenAI-compatible endpoint.
    ///
    /// Returns `None` when the runtime is unreachable, so the caller can report
    /// that rather than surfacing a transport error.
    pub(crate) async fn forward_chat(&self, body: &[u8]) -> Option<ForwardedResponse> {
        let url = format!("{}/v1/chat/completions", self.base);
        let response = self
            .client
            .post(&url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body.to_vec())
            .send()
            .await
            .inspect_err(|error| {
                tracing::warn!(%error, "runtime unreachable for dashboard chat");
            })
            .ok()?;

        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("application/json")
            .to_owned();
        let body = response.text().await.ok()?;
        Some(ForwardedResponse {
            status,
            content_type,
            body,
        })
    }

    /// `GET /internal/pxpipe/{action}` on the runtime.
    ///
    /// The worker holding the transform lives there, so the routes that report or
    /// change its state are proxied rather than answered here — see
    /// `crate::pxpipe`. `/internal/*` is refused by the gateway from outside, so
    /// this is reachable only from a service on the loopback.
    pub(crate) async fn pxpipe_get(&self, action: &str) -> Option<ForwardedResponse> {
        self.relay(self.client.get(self.pxpipe_url(action))).await
    }

    /// `POST /internal/pxpipe/{action}` on the runtime.
    pub(crate) async fn pxpipe_post(&self, action: &str, body: &[u8]) -> Option<ForwardedResponse> {
        self.relay(
            self.client
                .post(self.pxpipe_url(action))
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.to_vec()),
        )
        .await
    }

    fn pxpipe_url(&self, action: &str) -> String {
        format!("{}/internal/pxpipe/{action}", self.base)
    }

    /// Send one request and capture the reply verbatim.
    ///
    /// The upstream status is preserved rather than normalised: the runtime answers
    /// 409 for "not installed" and 502 for "will not load", and those call for
    /// different actions from whoever is reading.
    async fn relay(&self, request: reqwest::RequestBuilder) -> Option<ForwardedResponse> {
        let response = request
            .send()
            .await
            .inspect_err(|error| {
                tracing::warn!(%error, "runtime unreachable for a pxpipe control call");
            })
            .ok()?;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("application/json")
            .to_owned();
        let body = response.text().await.ok()?;
        Some(ForwardedResponse {
            status,
            content_type,
            body,
        })
    }
}

impl ForwardedResponse {
    /// Relay this reply to the caller unchanged.
    pub(crate) fn into_response(self) -> actix_web::HttpResponse {
        let status = actix_web::http::StatusCode::from_u16(self.status)
            .unwrap_or(actix_web::http::StatusCode::BAD_GATEWAY);
        crate::responses::passthrough(status, &self.content_type, self.body)
    }

    /// This reply's body as JSON, when it is JSON.
    pub(crate) fn json(&self) -> Option<serde_json::Value> {
        serde_json::from_str(&self.body).ok()
    }
}

/// The outcome of reading one connection record.
///
/// A missing connection and an unreachable state service are separate cases: the
/// first is the caller's mistake and stays a 404, the second is this deployment's
/// problem and must not be reported as "no such connection".
#[derive(Debug)]
pub(crate) enum ConnectionLookup {
    /// The connection's public record.
    Found(Value),
    /// State answered, and has no such connection.
    Missing,
    /// State could not be read.
    Unavailable,
}

/// Reader for state-owned usage data.
#[derive(Debug, Clone)]
pub struct StateClient {
    client: reqwest::Client,
    base: String,
}

impl Default for StateClient {
    fn default() -> Self {
        Self::from_env()
    }
}

impl StateClient {
    /// Client pointed at `NULLROUTER_STATE_ADDR`, or the default loopback port.
    pub fn from_env() -> Self {
        let addr = std::env::var("NULLROUTER_STATE_ADDR")
            .unwrap_or_else(|_| DEFAULT_STATE_ADDR.to_owned());
        Self::new(&addr)
    }

    /// Client for an explicit `host:port`.
    pub fn new(addr: &str) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(STATE_TIMEOUT)
                .build()
                .unwrap_or_default(),
            base: format!("http://{addr}"),
        }
    }

    /// Trigger a 9Router import in the state service.
    ///
    /// Returns the raw report so the dashboard can show exactly what landed.
    pub(crate) async fn migrate_from_9router(
        &self,
        data_dir: Option<&str>,
        dry_run: bool,
    ) -> Option<(u16, Value)> {
        let url = format!("{}/internal/v1/migrate/9router", self.base);
        let payload = serde_json::json!({ "dataDir": data_dir, "dryRun": dry_run });
        // An import walks a whole database, so it gets a longer budget than a
        // dashboard read.
        let response = self
            .client
            .post(&url)
            .timeout(Duration::from_secs(120))
            .json(&payload)
            .send()
            .await
            .inspect_err(|error| tracing::warn!(%error, "9router import unreachable"))
            .ok()?;
        let status = response.status().as_u16();
        let body = response.json::<Value>().await.ok()?;
        Some((status, body))
    }

    /// Aggregate usage stats, or `None` when state is unreachable.
    pub(crate) async fn usage_stats(&self) -> Option<Value> {
        let url = format!("{}/internal/v1/usage/stats", self.base);
        match self.client.get(&url).send().await {
            Ok(response) => response.json::<Value>().await.ok(),
            Err(error) => {
                tracing::warn!(%error, "usage stats unavailable");
                None
            }
        }
    }

    /// Recent request records, newest first. Empty when state is unreachable.
    pub(crate) async fn usage_records(&self, since_ms: u64, limit: usize) -> Vec<UsageRecord> {
        let url = format!(
            "{}/internal/v1/usage/records?sinceMs={since_ms}&limit={limit}",
            self.base
        );
        match self.client.get(&url).send().await {
            Ok(response) => response
                .json::<RecordsResponse>()
                .await
                .map(|parsed| parsed.records)
                .unwrap_or_default(),
            Err(error) => {
                tracing::warn!(%error, "usage records unavailable");
                Vec::new()
            }
        }
    }

    /// Filtered, paginated request records.
    ///
    /// `query` is the already-encoded parameter string. Returns the status and
    /// body so a rejected filter (a malformed date) can be relayed rather than
    /// answered with a wider result set than was asked for. `None` when state is
    /// unreachable.
    pub(crate) async fn usage_details(&self, query: &str) -> Option<(u16, Value)> {
        let url = format!("{}/internal/v1/usage/details?{query}", self.base);
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .inspect_err(|error| tracing::warn!(%error, "usage details unavailable"))
            .ok()?;
        let status = response.status().as_u16();
        Some((status, response.json::<Value>().await.ok()?))
    }

    /// Distinct providers seen in recorded usage, with their aggregates.
    ///
    /// `None` when state is unreachable.
    pub(crate) async fn usage_providers(&self) -> Option<Value> {
        self.get_json("/internal/v1/usage/providers", "usage providers")
            .await
    }

    /// Live request telemetry: recent requests and the failing provider.
    ///
    /// `None` when state is unreachable.
    pub(crate) async fn usage_live(&self) -> Option<Value> {
        self.get_json("/internal/v1/usage/live", "usage live snapshot")
            .await
    }

    /// Aggregate stats over the window starting at `since_ms`.
    ///
    /// Distinct from [`Self::usage_stats`], which reports lifetime counters:
    /// this sums only the records inside the window. `None` when state is
    /// unreachable.
    pub(crate) async fn usage_aggregate(&self, since_ms: u64) -> Option<Value> {
        self.get_json(
            &format!("/internal/v1/usage/aggregate?sinceMs={since_ms}"),
            "usage aggregate",
        )
        .await
    }

    /// Read one provider connection's public record.
    ///
    /// Used by the connection test to learn which provider and model to probe. The
    /// public projection is deliberate: this path needs the provider id and default
    /// model, never the credential, which the runtime fetches for itself over
    /// `/internal/*`.
    pub(crate) async fn connection(&self, connection_id: &str) -> ConnectionLookup {
        let url = format!("{}/api/providers/{}", self.base, urlencode(connection_id));
        let Ok(response) =
            self.client.get(&url).send().await.inspect_err(
                |error| tracing::warn!(%error, "state unreachable reading connection"),
            )
        else {
            return ConnectionLookup::Unavailable;
        };
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return ConnectionLookup::Missing;
        }
        if !response.status().is_success() {
            return ConnectionLookup::Unavailable;
        }
        let Ok(body) = response.json::<Value>().await else {
            return ConnectionLookup::Unavailable;
        };
        // State answers either the record itself or `{"connection": …}`.
        ConnectionLookup::Found(
            body.get("connection")
                .filter(|found| found.is_object())
                .cloned()
                .unwrap_or(body),
        )
    }

    /// Every configured provider connection, as public records.
    ///
    /// `None` when state is unreachable, which is deliberately not the same as an
    /// empty list: a batch test that answered `{total: 0, failed: 0}` because the
    /// state service was down would read as "everything passed".
    pub(crate) async fn connections(&self) -> Option<Vec<Value>> {
        let url = format!("{}/api/providers", self.base);
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .inspect_err(|error| tracing::warn!(%error, "provider connections unavailable"))
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        let body = response.json::<Value>().await.ok()?;
        // State answers `{"connections": [...]}`; a bare array is accepted too.
        Some(
            body.get("connections")
                .unwrap_or(&body)
                .as_array()
                .cloned()
                .unwrap_or_default(),
        )
    }

    /// Recorded usage and metadata for one connection.
    ///
    /// Returns the status alongside the body so a missing connection stays a
    /// 404 instead of becoming an empty success. `None` when state is
    /// unreachable.
    pub(crate) async fn usage_connection(&self, connection_id: &str) -> Option<(u16, Value)> {
        let url = format!(
            "{}/internal/v1/usage/connection/{}",
            self.base,
            urlencode(connection_id)
        );
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .inspect_err(|error| tracing::warn!(%error, "connection usage unavailable"))
            .ok()?;
        let status = response.status().as_u16();
        Some((status, response.json::<Value>().await.ok()?))
    }

    /// GET a loopback path as JSON, logging `label` when state cannot be read.
    async fn get_json(&self, path: &str, label: &str) -> Option<Value> {
        let url = format!("{}{path}", self.base);
        match self.client.get(&url).send().await {
            Ok(response) => response.json::<Value>().await.ok(),
            Err(error) => {
                tracing::warn!(%error, %label, "state read failed");
                None
            }
        }
    }
}

/// Percent-encode a path segment.
///
/// Connection ids come from the request path, so they are escaped before being
/// spliced into a loopback URL.
pub(crate) fn urlencode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}
