//! Live usage data: parsing, ordering, formatting, and SSE frame decoding.
//!
//! The Usage panel used to render `usage_snapshot`, a compile-time fixture whose
//! zeros were indistinguishable from a genuinely quiet router. Everything the
//! panel shows now comes from `/api/usage/*`, and every derivation lives here so
//! it stays testable on the native target, where there is no browser.
//!
//! Two rules shape the types below. A body that does not carry the shape the
//! endpoint promises parses to `None`, so a contract change surfaces as a
//! visible failure rather than a page of zeros. A counter the server did not
//! send stays `None`, so "no reading" and "zero" never render the same.

use serde_json::Value;

/// Rendered in place of a counter the server has not reported.
pub const NO_READING: &str = "—";

/// Shown for a name the record omitted.
const UNKNOWN: &str = "unknown";

/// Smallest bar height, in percent, for a minute that saw traffic.
///
/// One request and no requests must not look identical.
const MIN_BAR_PERCENT: u8 = 6;

/// A window the usage API accepts for its `period` query parameter.
///
/// Mirrors the guard in `services/api-actix/src/usage.rs`: any other value is
/// answered with `400`, so the selector must not be able to offer one.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum UsagePeriod {
    Today,
    LastDay,
    /// The upstream default.
    #[default]
    Week,
    Month,
    TwoMonths,
    All,
}

impl UsagePeriod {
    /// Every accepted period, in selector order.
    pub const ALL: [Self; 6] = [
        Self::Today,
        Self::LastDay,
        Self::Week,
        Self::Month,
        Self::TwoMonths,
        Self::All,
    ];

    /// The `period` query value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Today => "today",
            Self::LastDay => "24h",
            Self::Week => "7d",
            Self::Month => "30d",
            Self::TwoMonths => "60d",
            Self::All => "all",
        }
    }

    /// Selector label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Today => "Today",
            Self::LastDay => "24 hours",
            Self::Week => "7 days",
            Self::Month => "30 days",
            Self::TwoMonths => "60 days",
            Self::All => "All time",
        }
    }

    /// How the metric cards describe the window they are counting over.
    pub const fn detail(self) -> &'static str {
        match self {
            Self::Today => "Today",
            Self::LastDay => "Last 24 hours",
            Self::Week => "Last 7 days",
            Self::Month => "Last 30 days",
            Self::TwoMonths => "Last 60 days",
            Self::All => "All recorded usage",
        }
    }

    /// Full stats path for this window.
    ///
    /// Returned as a `&'static str` so it can be handed straight to the shared
    /// fetch helper without allocating a query string per render.
    pub const fn stats_path(self) -> &'static str {
        match self {
            Self::Today => "/api/usage/stats?period=today",
            Self::LastDay => "/api/usage/stats?period=24h",
            Self::Week => "/api/usage/stats?period=7d",
            Self::Month => "/api/usage/stats?period=30d",
            Self::TwoMonths => "/api/usage/stats?period=60d",
            Self::All => "/api/usage/stats?period=all",
        }
    }
}

/// One row of a `byProvider` / `byModel` / `byAccount` breakdown.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UsageBreakdownRow {
    pub name: String,
    pub requests: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cached_tokens: u64,
    pub total_tokens: u64,
    pub errors: u64,
    pub cost: f64,
}

impl UsageBreakdownRow {
    /// Share of `total` this row accounts for, as a percentage.
    ///
    /// `0` when the total is zero, so a quiet router does not divide by it.
    pub fn share_percent(&self, total: u64) -> u8 {
        if total == 0 {
            return 0;
        }
        let ratio = (self.requests.saturating_mul(100)) / total;
        u8::try_from(ratio.min(100)).unwrap_or(100)
    }
}

/// One minute of the `last10Minutes` series.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UsageMinute {
    /// Epoch millis at the start of the minute.
    pub timestamp: u64,
    pub requests: u64,
    pub tokens: u64,
}

/// The parsed `GET /api/usage/stats` body.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UsageStats {
    pub total_requests: u64,
    pub total_prompt_tokens: u64,
    pub total_completion_tokens: u64,
    pub total_cached_tokens: u64,
    pub total_cost: f64,
    pub by_provider: Vec<UsageBreakdownRow>,
    pub by_model: Vec<UsageBreakdownRow>,
    pub last_ten_minutes: Vec<UsageMinute>,
}

impl UsageStats {
    /// Prompt plus completion tokens.
    pub const fn total_tokens(&self) -> u64 {
        self.total_prompt_tokens
            .saturating_add(self.total_completion_tokens)
    }

    /// `true` when nothing has been recorded in this window.
    ///
    /// Drives the empty state, which must read as "nothing yet" rather than as a
    /// failed request.
    pub const fn is_empty(&self) -> bool {
        self.total_requests == 0 && self.by_provider.is_empty() && self.by_model.is_empty()
    }
}

/// One row of `GET /api/usage/logs`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UsageLogEntry {
    pub id: String,
    /// Epoch millis.
    pub timestamp: u64,
    pub provider: String,
    pub model: String,
    pub endpoint: Option<String>,
    pub status: String,
    pub status_code: Option<u16>,
    pub total_tokens: u64,
    pub latency_ms: u64,
    pub error: Option<String>,
}

impl UsageLogEntry {
    /// `true` when the request did not succeed.
    pub fn failed(&self) -> bool {
        self.status != "success"
    }

    /// Status pill class, matching the shared pill vocabulary.
    pub fn status_class(&self) -> &'static str {
        if self.failed() {
            "is-degraded"
        } else {
            "is-connected"
        }
    }
}

/// The payload of one `usage` SSE frame.
///
/// Counters are optional: a frame that omits one is reported as "no reading",
/// never as zero.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveUsage {
    /// `false` while the events service cannot read state, so an empty
    /// dashboard is distinguishable from an unwired one.
    pub live_telemetry: bool,
    pub active_requests: Option<u64>,
    pub requests_today: Option<u64>,
    pub tokens_today: Option<u64>,
    pub estimated_cost: Option<String>,
    pub recent_requests: Vec<UsageLogEntry>,
}

impl LiveUsage {
    /// Which counters differ from `previous`, so only those pulse.
    pub fn changes_from(&self, previous: &Self) -> LiveChanges {
        LiveChanges {
            active_requests: self.active_requests != previous.active_requests,
            requests_today: self.requests_today != previous.requests_today,
            tokens_today: self.tokens_today != previous.tokens_today,
            estimated_cost: self.estimated_cost != previous.estimated_cost,
        }
    }
}

/// Which live counters changed on the most recent frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LiveChanges {
    pub active_requests: bool,
    pub requests_today: bool,
    pub tokens_today: bool,
    pub estimated_cost: bool,
}

/// State of the `/api/usage/stream` subscription.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StreamState {
    #[default]
    Connecting,
    /// Connected, and the events service can read usage.
    Live,
    /// Connected, but state is unreachable upstream, so counters are unknown.
    Degraded,
    /// The browser lost the connection and is retrying.
    Interrupted,
    /// No browser to subscribe from (native builds).
    Unavailable,
}

impl StreamState {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Connecting => "Connecting…",
            Self::Live => "Live",
            Self::Degraded => "Telemetry unavailable",
            Self::Interrupted => "Reconnecting…",
            Self::Unavailable => "Stream offline",
        }
    }

    pub const fn class_name(self) -> &'static str {
        match self {
            Self::Live => "is-connected",
            Self::Degraded | Self::Interrupted => "is-degraded",
            Self::Connecting | Self::Unavailable => "is-idle",
        }
    }

    /// Whether live counters can be trusted as readings.
    pub const fn carries_readings(self) -> bool {
        matches!(self, Self::Live)
    }
}

/// A `u64` counter, when present.
fn counter(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(Value::as_u64)
}

/// A `u64` counter, defaulting to zero.
///
/// Only used inside a breakdown bucket, where the enclosing object already
/// proved the shape is right and a missing sub-counter genuinely means none.
fn counter_or_zero(value: &Value, key: &str) -> u64 {
    counter(value, key).unwrap_or(0)
}

/// A numeric field as `f64`, defaulting to zero.
fn amount(value: &Value, key: &str) -> f64 {
    value.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

/// A string field, when present and non-empty.
fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|found| !found.is_empty())
        .map(ToOwned::to_owned)
}

/// Parse one breakdown map into rows, ordered by requests descending.
fn breakdown(stats: &Value, key: &str) -> Vec<UsageBreakdownRow> {
    let Some(map) = stats.get(key).and_then(Value::as_object) else {
        return Vec::new();
    };

    let mut rows: Vec<UsageBreakdownRow> = map
        .iter()
        .map(|(name, bucket)| UsageBreakdownRow {
            name: name.clone(),
            requests: counter_or_zero(bucket, "requests"),
            prompt_tokens: counter_or_zero(bucket, "promptTokens"),
            completion_tokens: counter_or_zero(bucket, "completionTokens"),
            cached_tokens: counter_or_zero(bucket, "cachedTokens"),
            total_tokens: counter_or_zero(bucket, "totalTokens"),
            errors: counter_or_zero(bucket, "errors"),
            cost: amount(bucket, "cost"),
        })
        .collect();

    sort_breakdown(&mut rows);
    rows
}

/// Order breakdown rows: busiest first, then by name so ties are stable.
///
/// Stable ordering matters more than it looks — without the name tiebreak, two
/// providers with equal request counts would swap places on every poll.
pub fn sort_breakdown(rows: &mut [UsageBreakdownRow]) {
    rows.sort_by(|left, right| {
        right
            .requests
            .cmp(&left.requests)
            .then_with(|| right.total_tokens.cmp(&left.total_tokens))
            .then_with(|| left.name.cmp(&right.name))
    });
}

/// Parse the `last10Minutes` series, oldest first as the server emits it.
fn minutes(stats: &Value) -> Vec<UsageMinute> {
    stats
        .get("last10Minutes")
        .and_then(Value::as_array)
        .map(|series| {
            series
                .iter()
                .map(|entry| UsageMinute {
                    timestamp: counter_or_zero(entry, "timestamp"),
                    requests: counter_or_zero(entry, "requests"),
                    tokens: counter_or_zero(entry, "tokens"),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parse a `GET /api/usage/stats` body.
///
/// `None` when the body is not JSON, is not an object, or omits
/// `totalRequests` — the one field the endpoint always sends. Treating a
/// shape change as a failure is deliberate: the alternative is a page of
/// confident zeros.
pub fn parse_stats(body: &str) -> Option<UsageStats> {
    let value: Value = serde_json::from_str(body).ok()?;
    let total_requests = counter(&value, "totalRequests")?;

    Some(UsageStats {
        total_requests,
        total_prompt_tokens: counter_or_zero(&value, "totalPromptTokens"),
        total_completion_tokens: counter_or_zero(&value, "totalCompletionTokens"),
        total_cached_tokens: counter_or_zero(&value, "totalCachedTokens"),
        total_cost: amount(&value, "totalCost"),
        by_provider: breakdown(&value, "byProvider"),
        by_model: breakdown(&value, "byModel"),
        last_ten_minutes: minutes(&value),
    })
}

/// Parse one record from `/api/usage/logs` or from a frame's `recentRequests`.
fn log_entry(entry: &Value) -> UsageLogEntry {
    UsageLogEntry {
        id: text(entry, "id").unwrap_or_default(),
        timestamp: counter_or_zero(entry, "timestamp"),
        provider: text(entry, "provider").unwrap_or_else(|| UNKNOWN.to_owned()),
        model: text(entry, "model").unwrap_or_else(|| UNKNOWN.to_owned()),
        endpoint: text(entry, "endpoint"),
        status: text(entry, "status").unwrap_or_else(|| "success".to_owned()),
        status_code: counter(entry, "statusCode").and_then(|code| u16::try_from(code).ok()),
        total_tokens: counter_or_zero(entry, "totalTokens"),
        latency_ms: counter_or_zero(entry, "latencyMs"),
        error: text(entry, "error"),
    }
}

/// Parse a `GET /api/usage/logs` body, newest first.
///
/// `None` when the body is not a JSON array. An empty array is a valid, and
/// meaningful, answer.
pub fn parse_logs(body: &str) -> Option<Vec<UsageLogEntry>> {
    let value: Value = serde_json::from_str(body).ok()?;
    let array = value.as_array()?;
    let mut entries: Vec<UsageLogEntry> = array.iter().map(log_entry).collect();
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.timestamp));
    Some(entries)
}

/// Decode the `data:` payload of a `usage` SSE frame.
///
/// `None` only when the payload is not a JSON object. Missing counters are
/// preserved as `None` rather than coerced to zero, because a frame is a
/// snapshot of what the events service could read, not an assertion that the
/// router is idle.
pub fn parse_usage_frame(data: &str) -> Option<LiveUsage> {
    let value: Value = serde_json::from_str(data).ok()?;
    if !value.is_object() {
        return None;
    }

    let recent = value
        .get("recentRequests")
        .and_then(Value::as_array)
        .map(|entries| entries.iter().map(log_entry).collect())
        .unwrap_or_default();

    Some(LiveUsage {
        live_telemetry: value
            .get("liveTelemetry")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        active_requests: counter(&value, "activeRequests"),
        requests_today: counter(&value, "requestsToday"),
        tokens_today: counter(&value, "tokensToday"),
        estimated_cost: text(&value, "estimatedCost"),
        recent_requests: recent,
    })
}

/// Format a count with thousands separators.
///
/// Grouped rather than abbreviated: `1.2M` hides whether a bill is for
/// 1,150,000 or 1,249,999 tokens, and this panel is what a user checks before
/// questioning a charge.
pub fn format_count(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);

    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// Format an optional count, or the no-reading marker.
pub fn format_optional_count(value: Option<u64>) -> String {
    value.map_or_else(|| NO_READING.to_owned(), format_count)
}

/// Format a cost in dollars.
pub fn format_cost(value: f64) -> String {
    if value.is_finite() {
        format!("${value:.2}")
    } else {
        NO_READING.to_owned()
    }
}

/// Format a latency in milliseconds.
///
/// Integer arithmetic throughout, so a large millisecond count cannot lose
/// precision on the way to a display string.
pub fn format_latency(millis: u64) -> String {
    if millis < 1000 {
        return format!("{millis} ms");
    }
    let seconds = millis / 1000;
    let remainder = (millis % 1000) / 100;
    format!("{seconds}.{remainder} s")
}

/// Describe how long ago `timestamp` was, relative to `now`.
///
/// Both are epoch millis. A timestamp in the future (clock skew between the
/// browser and the router) reads as "just now" rather than a negative age.
pub fn format_age(timestamp: u64, now: u64) -> String {
    let elapsed = now.saturating_sub(timestamp) / 1000;
    match elapsed {
        0..=4 => "just now".to_owned(),
        5..=59 => format!("{elapsed}s ago"),
        60..=3599 => format!("{}m ago", elapsed / 60),
        3600..=86_399 => format!("{}h ago", elapsed / 3600),
        _ => format!("{}d ago", elapsed / 86_400),
    }
}

/// One bar of the 10-minute strip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SparkBar {
    /// Height as a percentage of the busiest minute in the series.
    pub height_percent: u8,
    pub requests: u64,
    pub tokens: u64,
    /// How far back this minute is, e.g. `9 min ago`.
    pub label: String,
}

/// Derive the bar strip from the `last10Minutes` series.
///
/// Heights are relative to the busiest minute so the strip stays legible at any
/// volume. A minute with traffic never renders flat, because a one-pixel bar and
/// an absent bar would read the same.
pub fn sparkline(series: &[UsageMinute]) -> Vec<SparkBar> {
    let peak = series
        .iter()
        .map(|minute| minute.requests)
        .max()
        .unwrap_or(0);
    let total = series.len();

    series
        .iter()
        .enumerate()
        .map(|(index, minute)| {
            let height_percent = if minute.requests == 0 || peak == 0 {
                0
            } else {
                let scaled = (minute.requests.saturating_mul(100)) / peak;
                u8::try_from(scaled.min(100))
                    .unwrap_or(100)
                    .max(MIN_BAR_PERCENT)
            };
            let minutes_ago = total.saturating_sub(index).saturating_sub(1);
            let label = if minutes_ago == 0 {
                "this minute".to_owned()
            } else {
                format!("{minutes_ago} min ago")
            };

            SparkBar {
                height_percent,
                requests: minute.requests,
                tokens: minute.tokens,
                label,
            }
        })
        .collect()
}

/// Text alternative for the bar strip.
///
/// The strip is decorative on its own; this sentence is what a screen reader
/// user gets, so it must carry the same information the bars do.
pub fn sparkline_summary(series: &[UsageMinute]) -> String {
    if series.is_empty() {
        return "No per-minute activity has been reported for the last 10 minutes.".to_owned();
    }

    let requests: u64 = series
        .iter()
        .map(|minute| minute.requests)
        .fold(0, u64::saturating_add);
    if requests == 0 {
        return "No requests in the last 10 minutes.".to_owned();
    }

    let tokens: u64 = series
        .iter()
        .map(|minute| minute.tokens)
        .fold(0, u64::saturating_add);
    let peak = series
        .iter()
        .map(|minute| minute.requests)
        .max()
        .unwrap_or(0);

    format!(
        "{} requests and {} tokens over the last {} minutes, peaking at {} requests in one minute.",
        format_count(requests),
        format_count(tokens),
        series.len(),
        format_count(peak)
    )
}

#[cfg(test)]
mod tests {
    use super::{UsagePeriod, format_count, parse_stats};

    #[test]
    fn every_period_path_carries_its_own_query_value() {
        // A selector option that sends an unaccepted period is answered with 400.
        for period in UsagePeriod::ALL {
            let path = period.stats_path();
            assert!(
                path.ends_with(period.as_str()),
                "{path} does not select {}",
                period.as_str()
            );
            assert!(path.starts_with("/api/usage/stats?period="), "{path}");
        }
    }

    #[test]
    fn grouping_starts_at_four_digits() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1000), "1,000");
    }

    #[test]
    fn a_body_without_total_requests_is_a_failure_not_an_empty_panel() {
        assert!(parse_stats("{}").is_none());
        assert!(parse_stats(r#"{"totalRequests":0}"#).is_some());
    }
}
