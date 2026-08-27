//! Usage recording and aggregation.
//!
//! Backs the `/api/usage/*` dashboard surface, which previously returned
//! hardcoded zeros. Ports the shape of `inspire/src/lib/usageDb.js` without its
//! `SQLite` storage: records live in the same JSON snapshot as the rest of state,
//! with a bounded ring of recent requests.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Cap on retained request records, so the state file cannot grow without
/// bound. Aggregate totals are kept separately and are never trimmed.
pub(crate) const MAX_REQUEST_LOG: usize = 1000;

/// One completed (or failed) request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageRecord {
    pub id: String,
    /// Epoch millis.
    pub timestamp: u64,
    pub provider: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// `success` or `error`.
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub cached_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
    /// Total wall-clock duration in milliseconds.
    #[serde(default)]
    pub latency_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What the runtime reports after a request finishes.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageInput {
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub connection_id: Option<String>,
    #[serde(default)]
    pub api_key_id: Option<String>,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub status_code: Option<u16>,
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub cached_tokens: u64,
    #[serde(default)]
    pub latency_ms: u64,
    #[serde(default)]
    pub error: Option<String>,
}

/// Running totals plus the recent-request ring.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageLog {
    #[serde(default)]
    pub records: Vec<UsageRecord>,
    /// Requests recorded since inception, including trimmed ones.
    #[serde(default)]
    pub total_requests: u64,
    #[serde(default)]
    pub total_prompt_tokens: u64,
    #[serde(default)]
    pub total_completion_tokens: u64,
    #[serde(default)]
    pub total_cached_tokens: u64,
    /// Monotonic counter backing record ids.
    #[serde(default)]
    pub sequence: u64,
}

impl UsageLog {
    /// Append a record, trimming the oldest once the cap is reached.
    pub(crate) fn record(&mut self, input: UsageInput, now_ms: u64) -> UsageRecord {
        self.sequence = self.sequence.saturating_add(1);
        let total_tokens = input.prompt_tokens.saturating_add(input.completion_tokens);

        let record = UsageRecord {
            id: format!("req_{}", self.sequence),
            timestamp: now_ms,
            provider: input.provider,
            model: input.model,
            connection_id: input.connection_id,
            api_key_id: input.api_key_id,
            endpoint: input.endpoint,
            status: input.status.unwrap_or_else(|| "success".to_owned()),
            status_code: input.status_code,
            prompt_tokens: input.prompt_tokens,
            completion_tokens: input.completion_tokens,
            cached_tokens: input.cached_tokens,
            total_tokens,
            latency_ms: input.latency_ms,
            error: input.error,
        };

        self.total_requests = self.total_requests.saturating_add(1);
        self.total_prompt_tokens = self
            .total_prompt_tokens
            .saturating_add(record.prompt_tokens);
        self.total_completion_tokens = self
            .total_completion_tokens
            .saturating_add(record.completion_tokens);
        self.total_cached_tokens = self
            .total_cached_tokens
            .saturating_add(record.cached_tokens);

        self.records.push(record.clone());
        if self.records.len() > MAX_REQUEST_LOG {
            let excess = self.records.len() - MAX_REQUEST_LOG;
            self.records.drain(..excess);
        }
        record
    }

    /// Records newer than `since_ms`, newest first.
    pub(crate) fn recent(&self, since_ms: u64, limit: usize) -> Vec<&UsageRecord> {
        let mut recent: Vec<&UsageRecord> = self
            .records
            .iter()
            .filter(|record| record.timestamp >= since_ms)
            .collect();
        // Newest first.
        recent.sort_by_key(|record| std::cmp::Reverse(record.timestamp));
        recent.truncate(limit);
        recent
    }

    /// Aggregate stats in the dashboard's expected shape.
    pub(crate) fn stats(&self, now_ms: u64) -> Value {
        let mut by_provider: BTreeMap<&str, Buckets> = BTreeMap::new();
        let mut by_model: BTreeMap<&str, Buckets> = BTreeMap::new();
        let mut by_account: BTreeMap<&str, Buckets> = BTreeMap::new();
        let mut by_api_key: BTreeMap<&str, Buckets> = BTreeMap::new();
        let mut by_endpoint: BTreeMap<&str, Buckets> = BTreeMap::new();

        for record in &self.records {
            by_provider.entry(&record.provider).or_default().add(record);
            by_model.entry(&record.model).or_default().add(record);
            if let Some(connection) = record.connection_id.as_deref() {
                by_account.entry(connection).or_default().add(record);
            }
            if let Some(key) = record.api_key_id.as_deref() {
                by_api_key.entry(key).or_default().add(record);
            }
            if let Some(endpoint) = record.endpoint.as_deref() {
                by_endpoint.entry(endpoint).or_default().add(record);
            }
        }

        json!({
            "totalRequests": self.total_requests,
            "totalPromptTokens": self.total_prompt_tokens,
            "totalCompletionTokens": self.total_completion_tokens,
            "totalCachedTokens": self.total_cached_tokens,
            "totalCost": 0,
            "byProvider": buckets_to_value(&by_provider),
            "byModel": buckets_to_value(&by_model),
            "byAccount": buckets_to_value(&by_account),
            "byApiKey": buckets_to_value(&by_api_key),
            "byEndpoint": buckets_to_value(&by_endpoint),
            "last10Minutes": self.last_ten_minutes(now_ms),
        })
    }

    /// Per-minute request counts over the last 10 minutes, oldest first.
    fn last_ten_minutes(&self, now_ms: u64) -> Vec<Value> {
        const MINUTE_MS: u64 = 60 * 1000;
        let window_start = now_ms.saturating_sub(10 * MINUTE_MS);
        let mut buckets = vec![(0_u64, 0_u64); 10];

        for record in &self.records {
            if record.timestamp < window_start {
                continue;
            }
            let offset = (record.timestamp.saturating_sub(window_start)) / MINUTE_MS;
            let index = usize::try_from(offset).unwrap_or(9).min(9);
            if let Some(bucket) = buckets.get_mut(index) {
                bucket.0 = bucket.0.saturating_add(1);
                bucket.1 = bucket.1.saturating_add(record.total_tokens);
            }
        }

        buckets
            .into_iter()
            .enumerate()
            .map(|(index, (requests, tokens))| {
                let minute_offset = u64::try_from(index).unwrap_or(0);
                json!({
                    "timestamp": window_start.saturating_add(minute_offset * MINUTE_MS),
                    "requests": requests,
                    "tokens": tokens,
                })
            })
            .collect()
    }
}

/// Per-dimension counters.
#[derive(Debug, Default, Clone, Copy)]
struct Buckets {
    requests: u64,
    prompt_tokens: u64,
    completion_tokens: u64,
    cached_tokens: u64,
    errors: u64,
}

impl Buckets {
    fn add(&mut self, record: &UsageRecord) {
        self.requests = self.requests.saturating_add(1);
        self.prompt_tokens = self.prompt_tokens.saturating_add(record.prompt_tokens);
        self.completion_tokens = self
            .completion_tokens
            .saturating_add(record.completion_tokens);
        self.cached_tokens = self.cached_tokens.saturating_add(record.cached_tokens);
        if record.status != "success" {
            self.errors = self.errors.saturating_add(1);
        }
    }

    fn to_value(self) -> Value {
        json!({
            "requests": self.requests,
            "promptTokens": self.prompt_tokens,
            "completionTokens": self.completion_tokens,
            "cachedTokens": self.cached_tokens,
            "totalTokens": self.prompt_tokens.saturating_add(self.completion_tokens),
            "errors": self.errors,
            "cost": 0,
        })
    }
}

fn buckets_to_value(buckets: &BTreeMap<&str, Buckets>) -> Value {
    Value::Object(
        buckets
            .iter()
            .map(|(key, bucket)| ((*key).to_owned(), bucket.to_value()))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::{MAX_REQUEST_LOG, UsageInput, UsageLog};
    use serde_json::json;

    fn input(provider: &str, model: &str, prompt: u64, completion: u64) -> UsageInput {
        UsageInput {
            provider: provider.to_owned(),
            model: model.to_owned(),
            prompt_tokens: prompt,
            completion_tokens: completion,
            ..UsageInput::default()
        }
    }

    #[test]
    fn recording_accumulates_totals() {
        let mut log = UsageLog::default();
        log.record(input("openai", "gpt-5", 10, 5), 1000);
        log.record(input("openai", "gpt-5", 20, 7), 2000);

        assert_eq!(log.total_requests, 2);
        assert_eq!(log.total_prompt_tokens, 30);
        assert_eq!(log.total_completion_tokens, 12);
        assert_eq!(log.records.len(), 2);
        // Ids are stable and sequential.
        assert_eq!(log.records.first().map(|r| r.id.as_str()), Some("req_1"));
        assert_eq!(log.records.get(1).map(|r| r.id.as_str()), Some("req_2"));
        assert_eq!(log.records.first().map(|r| r.total_tokens), Some(15));
    }

    #[test]
    fn stats_group_by_every_dimension() {
        let mut log = UsageLog::default();
        log.record(
            UsageInput {
                connection_id: Some("conn_1".to_owned()),
                api_key_id: Some("key_1".to_owned()),
                endpoint: Some("/v1/chat/completions".to_owned()),
                ..input("openai", "gpt-5", 10, 5)
            },
            1000,
        );
        log.record(
            UsageInput {
                connection_id: Some("conn_2".to_owned()),
                status: Some("error".to_owned()),
                ..input("anthropic", "claude-sonnet-4.5", 3, 0)
            },
            2000,
        );

        let stats = log.stats(3000);
        assert_eq!(stats.get("totalRequests"), Some(&json!(2)));
        assert_eq!(
            stats.pointer("/byProvider/openai/requests"),
            Some(&json!(1))
        );
        assert_eq!(
            stats.pointer("/byProvider/anthropic/errors"),
            Some(&json!(1))
        );
        assert_eq!(
            stats.pointer("/byModel/gpt-5/totalTokens"),
            Some(&json!(15))
        );
        assert_eq!(stats.pointer("/byAccount/conn_1/requests"), Some(&json!(1)));
        assert_eq!(stats.pointer("/byApiKey/key_1/requests"), Some(&json!(1)));
        assert_eq!(
            stats.pointer("/byEndpoint/~1v1~1chat~1completions/requests"),
            Some(&json!(1))
        );
    }

    #[test]
    fn records_are_trimmed_but_totals_are_not() {
        let mut log = UsageLog::default();
        for index in 0..(MAX_REQUEST_LOG + 50) {
            log.record(input("openai", "gpt-5", 1, 1), 1000 + index as u64);
        }
        assert_eq!(log.records.len(), MAX_REQUEST_LOG);
        // Lifetime totals survive trimming.
        assert_eq!(log.total_requests, (MAX_REQUEST_LOG + 50) as u64);
        // The oldest records were dropped, not the newest.
        assert_eq!(log.records.first().map(|r| r.id.as_str()), Some("req_51"));
    }

    #[test]
    fn recent_returns_newest_first_within_the_window() {
        let mut log = UsageLog::default();
        log.record(input("openai", "old", 1, 1), 1000);
        log.record(input("openai", "new", 1, 1), 5000);

        let recent = log.recent(2000, 10);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent.first().map(|r| r.model.as_str()), Some("new"));

        let all = log.recent(0, 10);
        assert_eq!(all.len(), 2);
        // Newest first.
        assert_eq!(all.first().map(|r| r.model.as_str()), Some("new"));
    }

    #[test]
    fn ten_minute_window_buckets_by_minute() {
        // Well past the 10-minute mark, so the window start is not clamped to 0
        // by saturating_sub and "outside the window" is unambiguous.
        let now = 60 * 60 * 1000;
        let mut log = UsageLog::default();
        // Inside the window.
        log.record(input("openai", "gpt-5", 1, 1), now - 30_000);
        // Well outside the window.
        log.record(input("openai", "gpt-5", 1, 1), now - 40 * 60 * 1000);

        let stats = log.stats(now);
        let series = stats
            .get("last10Minutes")
            .and_then(|value| value.as_array())
            .expect("series");
        assert_eq!(series.len(), 10);
        let counted: u64 = series
            .iter()
            .filter_map(|entry| entry.get("requests").and_then(serde_json::Value::as_u64))
            .sum();
        assert_eq!(counted, 1, "only the in-window record is bucketed");
    }
}
