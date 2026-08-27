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
