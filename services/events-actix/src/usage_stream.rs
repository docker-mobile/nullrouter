//! Live usage SSE.
//!
//! Replaces the single static frame that `/api/usage/stream` previously
//! returned: this polls `nullrouter-state` and streams a fresh `usage` event on
//! each tick, so the dashboard's telemetry updates without a page reload.
//! Mirrors the event names upstream's `/api/usage/stream` emits.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Default loopback address of `nullrouter-state`.
const DEFAULT_STATE_ADDR: &str = "127.0.0.1:20134";
/// How often to re-read usage. Matches upstream's dashboard cadence closely
/// enough to feel live without hammering the state service.
const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// A single read must not stall the stream.
const READ_TIMEOUT: Duration = Duration::from_secs(3);
/// Recent requests carried in each frame.
const RECENT_LIMIT: usize = 20;

/// The `usage` event payload.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageFrame {
    /// `true` once real telemetry is flowing, so the UI can distinguish an
    /// empty dashboard from an unwired one.
    pub live_telemetry: bool,
    pub active_requests: u64,
    pub requests_today: u64,
    pub tokens_today: u64,
    pub estimated_cost: String,
    pub recent_requests: Vec<Value>,
}

impl UsageFrame {
    /// The frame sent when state cannot be read.
    pub(crate) fn offline() -> Self {
        Self {
            live_telemetry: false,
            active_requests: 0,
            requests_today: 0,
            tokens_today: 0,
            estimated_cost: "$0.00".to_owned(),
            recent_requests: Vec::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct RecordsResponse {
    #[serde(default)]
    records: Vec<Value>,
}

/// Reads usage snapshots from the state service.
#[derive(Debug, Clone)]
pub struct UsageReader {
    client: reqwest::Client,
    base: String,
}

impl Default for UsageReader {
    fn default() -> Self {
        Self::from_env()
    }
}

impl UsageReader {
    /// Reader pointed at `NULLROUTER_STATE_ADDR`, or the default loopback port.
    pub fn from_env() -> Self {
        let addr = std::env::var("NULLROUTER_STATE_ADDR")
            .unwrap_or_else(|_| DEFAULT_STATE_ADDR.to_owned());
        Self::new(&addr)
    }

    /// Reader for an explicit `host:port`.
    pub fn new(addr: &str) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(READ_TIMEOUT)
                .build()
                .unwrap_or_default(),
            base: format!("http://{addr}"),
        }
    }

    /// Current usage snapshot, or the offline frame when state is unreachable.
    pub(crate) async fn snapshot(&self) -> UsageFrame {
        let stats = self.read_json("/internal/v1/usage/stats").await;
        let Some(stats) = stats else {
            return UsageFrame::offline();
        };

        let records = self
            .read_json(&format!(
                "/internal/v1/usage/records?sinceMs=0&limit={RECENT_LIMIT}"
            ))
            .await
            .and_then(|value| serde_json::from_value::<RecordsResponse>(value).ok())
            .map(|parsed| parsed.records)
            .unwrap_or_default();

        let read = |key: &str| stats.get(key).and_then(Value::as_u64).unwrap_or(0);
        let requests = read("totalRequests");
        let tokens = read("totalPromptTokens") + read("totalCompletionTokens");

        UsageFrame {
            // State answered, so telemetry is wired even at zero volume.
            live_telemetry: true,
            active_requests: 0,
            requests_today: requests,
            tokens_today: tokens,
            estimated_cost: "$0.00".to_owned(),
            recent_requests: records,
        }
    }

    async fn read_json(&self, path: &str) -> Option<Value> {
        let url = format!("{}{path}", self.base);
        match self.client.get(&url).send().await {
            Ok(response) => response.json::<Value>().await.ok(),
            Err(error) => {
                tracing::debug!(%error, path, "usage read failed");
                None
            }
        }
    }
}

/// Serialize one SSE event.
pub(crate) fn sse_event<T: Serialize>(event: &str, data: &T) -> String {
    let payload = serde_json::to_string(data).unwrap_or_else(|_| "{}".to_owned());
    format!("event: {event}\ndata: {payload}\n\n")
}

/// The opening `connected` frame.
pub(crate) fn connected_frame(service: &str) -> String {
    sse_event(
        "connected",
        &json!({ "service": service, "stream": "usage", "connected": true }),
    )
}

/// Poll interval for the live stream.
pub(crate) const fn poll_interval() -> Duration {
    POLL_INTERVAL
}

#[cfg(test)]
mod tests {
    use super::{UsageFrame, connected_frame, sse_event};
    use serde_json::json;

    #[test]
    fn offline_frame_marks_telemetry_unavailable() {
        let frame = UsageFrame::offline();
        assert!(!frame.live_telemetry);
        assert_eq!(frame.requests_today, 0);
        assert_eq!(frame.estimated_cost, "$0.00");
        assert!(frame.recent_requests.is_empty());
    }

    #[test]
    fn frames_are_sse_encoded_with_event_names() {
        let frame = sse_event("usage", &json!({ "a": 1 }));
        assert_eq!(frame, "event: usage\ndata: {\"a\":1}\n\n");

        let connected = connected_frame("nullrouter-events");
        assert!(connected.starts_with("event: connected\n"), "{connected}");
        assert!(connected.contains("\"stream\":\"usage\""), "{connected}");
        assert!(connected.ends_with("\n\n"));
    }

    #[test]
    fn usage_frame_serializes_camel_case() {
        let frame = UsageFrame::offline();
        let value = serde_json::to_value(&frame).expect("serializes");
        assert!(value.get("liveTelemetry").is_some());
        assert!(value.get("requestsToday").is_some());
        assert!(value.get("recentRequests").is_some());
        // snake_case keys must not leak into the wire shape.
        assert!(value.get("live_telemetry").is_none());
    }
}
