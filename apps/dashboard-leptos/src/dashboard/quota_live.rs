//! Live quota state: per-account usage the router recorded, and the absence of
//! provider-reported limits.
//!
//! The Quota panel used to render `quota_tracker_state()` — a fixture with an
//! empty row list and a "Live limits: Offline" card. It was not a lie, but it
//! also showed nothing, while `/api/usage/stats` was already reporting real
//! per-account request and token counts.
//!
//! This module draws the line the panel has to hold. Two different facts live
//! here and are never blended:
//!
//! * **Recorded usage** is real. `byAccount` is summed from records the router
//!   wrote, so requests, tokens, and errors per account are readings.
//! * **A provider-reported limit** is not available. `/api/usage/{id}` answers
//!   with upstream's "Usage API not implemented for {provider}" envelope and an
//!   empty `quotas` array, because none of upstream's per-provider quota APIs
//!   are ported. So [`QuotaRow::limit`] stays `None` and renders as
//!   [`LIMIT_NOT_REPORTED`] — never as `0%`, never as a full bar.
//!
//! Kept free of `leptos` and of `fetch` so every derivation below is unit
//! testable on the native target.

use serde_json::Value;

use crate::api::ApiError;
use crate::dashboard::providers_live::{Connection, ConnectionList};

/// `GET /api/providers`, the account-name source for the join.
pub const CONNECTIONS_PATH: &str = "/api/providers";

/// Rendered wherever a provider-reported ceiling would go.
///
/// A distinct string rather than an empty cell: a blank column reads as "zero
/// used of zero" at a glance, which is the exact misreading this panel exists to
/// prevent.
pub const LIMIT_NOT_REPORTED: &str = "limit not reported";

/// Why no ceiling is shown, in one sentence.
pub const LIMIT_NOT_REPORTED_DETAIL: &str =
    "This build does not call provider quota APIs, so no account ceiling is known.";

/// Shown for an identity field the record did not carry.
const UNKNOWN: &str = "unknown";

/// A window `/api/usage/stats` accepts for `period`.
///
/// Mirrors the guard in `services/api-actix/src/usage.rs`; any other value is
/// answered with `400`, so the selector must not be able to offer one.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum QuotaWindow {
    Today,
    LastDay,
    #[default]
    Week,
    Month,
    TwoMonths,
    All,
}

impl QuotaWindow {
    /// Every accepted window, in selector order.
    pub const ALL: [Self; 6] = [
        Self::Today,
        Self::LastDay,
        Self::Week,
        Self::Month,
        Self::TwoMonths,
        Self::All,
    ];

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

    /// How the panel describes the window it is counting over.
    pub const fn detail(self) -> &'static str {
        match self {
            Self::Today => "Recorded today",
            Self::LastDay => "Recorded in the last 24 hours",
            Self::Week => "Recorded in the last 7 days",
            Self::Month => "Recorded in the last 30 days",
            Self::TwoMonths => "Recorded in the last 60 days",
            Self::All => "All recorded usage",
        }
    }

    /// Full stats path for this window.
    ///
    /// `&'static str` so it can be handed to the shared fetch helper without
    /// allocating a query string per render.
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

/// `GET /api/usage/{connectionId}`, the provider-quota probe for one account.
///
/// Ids are minted by the state service, but they still travel through a URL, so
/// anything outside RFC 3986 `unreserved` is percent-encoded rather than trusted
/// to be path-safe.
pub fn connection_usage_path(connection_id: &str) -> String {
    format!("/api/usage/{}", encode_path_segment(connection_id))
}

/// Percent-encode everything outside RFC 3986 `unreserved`.
fn encode_path_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(hex_digit(byte >> 4));
            encoded.push(hex_digit(byte & 0x0f));
        }
    }
    encoded
}

/// One uppercase hex digit for a nibble.
///
/// A match rather than a lookup table so no index can be out of range and the
/// function is total without a fallback that would mis-encode a byte.
const fn hex_digit(nibble: u8) -> char {
    match nibble {
        0 => '0',
        1 => '1',
        2 => '2',
        3 => '3',
        4 => '4',
        5 => '5',
        6 => '6',
        7 => '7',
        8 => '8',
        9 => '9',
        10 => 'A',
        11 => 'B',
        12 => 'C',
        13 => 'D',
        14 => 'E',
        _ => 'F',
    }
}

/// One account's recorded usage, and what is known about its ceiling.
///
/// `limit` is `Option` rather than a number with a sentinel because the two
/// cases are different claims: `Some(n)` means a provider reported a ceiling of
/// `n`, and `None` means nothing was reported. There is no third state in which
/// a plausible number stands in for the second.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QuotaRow {
    /// The `byAccount` map key, kept verbatim as the row's identity.
    pub key: String,
    /// Account name, from the bucket or from the joined connection.
    pub account: String,
    /// Provider display name, when the record or the join supplied one.
    pub provider: String,
    /// Model this bucket covers, when the shape carries one per account.
    pub model: Option<String>,
    /// The connection this bucket belongs to, when the shape carries it.
    pub connection_id: Option<String>,
    /// `true` once a `GET /api/providers` row was matched to this account.
    ///
    /// Drives the "not in the connection list" note, so an orphaned bucket is
    /// visible as such instead of quietly showing its raw key as a name.
    pub matched_connection: bool,
    pub requests: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cached_tokens: u64,
    pub total_tokens: u64,
    pub errors: u64,
    /// Provider-reported ceiling on requests. `None` until a provider quota API
    /// reports one, which this build never does.
    pub limit: Option<u64>,
}

impl QuotaRow {
    /// The ceiling as display text, naming its absence when there is none.
    pub fn limit_label(&self) -> String {
        self.limit
            .map_or_else(|| LIMIT_NOT_REPORTED.to_owned(), format_count)
    }

    /// Share of the reported ceiling, as a percentage.
    ///
    /// `None` whenever there is no ceiling — which is what stops the bar from
    /// rendering `0%` (reads as "unused") or `100%` (reads as "exhausted") for an
    /// account whose limit nobody reported.
    pub fn limit_percent(&self) -> Option<u8> {
        let limit = self.limit.filter(|limit| *limit > 0)?;
        let ratio = self.requests.saturating_mul(100) / limit;
        Some(u8::try_from(ratio.min(100)).unwrap_or(100))
    }

    /// Share of the window's total recorded requests, as a percentage.
    ///
    /// This is a share of what the router recorded, not of any allowance, and
    /// the panel labels it that way. `0` when the window recorded nothing, so a
    /// quiet router does not divide by it.
    pub fn share_percent(&self, window_requests: u64) -> u8 {
        if window_requests == 0 {
            return 0;
        }
        let ratio = self.requests.saturating_mul(100) / window_requests;
        u8::try_from(ratio.min(100)).unwrap_or(100)
    }

    /// Text alternative for this row's bar.
    ///
    /// The bar carries share by width and the limit by nothing at all, so this
    /// sentence has to state both — including that the ceiling is unknown.
    pub fn bar_summary(&self, window_requests: u64) -> String {
        let share = self.share_percent(window_requests);
        match self.limit {
            Some(limit) => format!(
                "{} of {} requests against the reported limit; {share}% of recorded requests in this window.",
                format_count(self.requests),
                format_count(limit)
            ),
            None => format!(
                "{} requests recorded, {share}% of this window. No provider limit reported, so no remaining allowance is known.",
                format_count(self.requests)
            ),
        }
    }

    /// Accessible label for the per-row limit probe.
    pub fn probe_label(&self) -> String {
        format!("Check provider limit for {}", self.account)
    }

    /// Id of this row's live status region.
    pub fn status_id(&self) -> String {
        format!("nr-quota-status-{}", dom_suffix(&self.key))
    }
}

/// Reduce a key to characters that are safe in a DOM id.
fn dom_suffix(key: &str) -> String {
    key.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

/// The per-account projection of one `/api/usage/stats` body.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QuotaSnapshot {
    /// `totalRequests` for the window, used as the share denominator.
    pub window_requests: u64,
    pub rows: Vec<QuotaRow>,
}

impl QuotaSnapshot {
    /// `true` when the window recorded no per-account usage.
    ///
    /// The panel renders this as its empty state. It is deliberately not the
    /// same as a failed parse: "no account has sent a request in this window" is
    /// a real answer, and the old fixture could not express it.
    pub const fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Requests summed over the rows present.
    pub fn recorded_requests(&self) -> u64 {
        self.rows
            .iter()
            .map(|row| row.requests)
            .fold(0, u64::saturating_add)
    }

    /// Tokens summed over the rows present.
    pub fn recorded_tokens(&self) -> u64 {
        self.rows
            .iter()
            .map(|row| row.total_tokens)
            .fold(0, u64::saturating_add)
    }

    /// How many rows carry a provider-reported ceiling.
    ///
    /// The panel shows this as a count so "0 of 4 accounts report a limit" is
    /// stated outright rather than inferred from four identical dashes.
    pub fn rows_with_limit(&self) -> usize {
        self.rows.iter().filter(|row| row.limit.is_some()).count()
    }

    /// Record a probe result against one row.
    pub fn set_limit(&mut self, key: &str, limit: Option<u64>) {
        if let Some(row) = self.rows.iter_mut().find(|row| row.key == key) {
            row.limit = limit;
        }
    }

    /// Attach account names from `GET /api/providers`.
    ///
    /// Matching is by `connectionId` when the bucket carries one, and by
    /// account-name equality otherwise, because the lifetime shape keys buckets
    /// by raw connection id while the windowed shape keys them by a composed
    /// label. A bucket that matches nothing keeps the name the record carried and
    /// is marked unmatched — it is a real bucket for a connection that has since
    /// been deleted, and hiding it would under-report usage.
    pub fn join_connections(&mut self, connections: &ConnectionList) {
        for row in &mut self.rows {
            let matched = row
                .connection_id
                .as_deref()
                .and_then(|id| find_by_id(connections, id))
                .or_else(|| find_by_id(connections, &row.key));
            let Some(connection) = matched else {
                continue;
            };

            row.matched_connection = true;
            row.connection_id = Some(connection.id.clone());
            if !connection.name.trim().is_empty() {
                row.account = connection.name.trim().to_owned();
            }
            row.provider = connection.provider_label();
        }
    }
}

/// One connection by id.
fn find_by_id<'list>(connections: &'list ConnectionList, id: &str) -> Option<&'list Connection> {
    connections
        .connections()
        .iter()
        .find(|connection| connection.id == id)
}

/// A `u64` counter, when present.
fn counter(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(Value::as_u64)
}

/// A `u64` counter, defaulting to zero.
///
/// Only used inside a bucket, where the enclosing object already proved the
/// shape is right and a missing sub-counter genuinely means none.
fn counter_or_zero(value: &Value, key: &str) -> u64 {
    counter(value, key).unwrap_or(0)
}

/// A string field, when present and non-empty.
fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|found| !found.is_empty())
        .map(ToOwned::to_owned)
}

/// Parse one `byAccount` bucket.
///
/// Two shapes reach this. The windowed aggregate keys buckets by
/// `model (provider - accountName)` and labels them with `connectionId`,
/// `accountName`, `provider`, and `rawModel`. The lifetime shape keys them by
/// raw connection id and carries no labels at all. Both are read here, and
/// whatever a shape omits stays absent rather than being invented — an
/// unlabelled bucket shows its key until the provider join supplies a name.
fn quota_row(key: &str, bucket: &Value) -> QuotaRow {
    let prompt_tokens = counter_or_zero(bucket, "promptTokens");
    let completion_tokens = counter_or_zero(bucket, "completionTokens");

    QuotaRow {
        key: key.to_owned(),
        account: text(bucket, "accountName").unwrap_or_else(|| key.to_owned()),
        provider: text(bucket, "provider").unwrap_or_else(|| UNKNOWN.to_owned()),
        model: text(bucket, "rawModel"),
        connection_id: text(bucket, "connectionId"),
        matched_connection: false,
        requests: counter_or_zero(bucket, "requests"),
        prompt_tokens,
        completion_tokens,
        cached_tokens: counter_or_zero(bucket, "cachedTokens"),
        // Prefer the reported total; fall back to the parts so the column is
        // never blank for a shape that omits the sum.
        total_tokens: counter(bucket, "totalTokens")
            .unwrap_or_else(|| prompt_tokens.saturating_add(completion_tokens)),
        errors: counter_or_zero(bucket, "errors"),
        // No provider quota API is called anywhere in this build, so there is
        // nothing that could fill this in at parse time.
        limit: None,
    }
}

/// Order rows: busiest first, then by tokens, then by key so ties are stable.
///
/// Stable ordering matters more than it looks: without the key tiebreak, two
/// accounts with equal request counts would swap places on every poll.
pub fn sort_rows(rows: &mut [QuotaRow]) {
    rows.sort_by(|left, right| {
        right
            .requests
            .cmp(&left.requests)
            .then_with(|| right.total_tokens.cmp(&left.total_tokens))
            .then_with(|| left.key.cmp(&right.key))
    });
}

/// Parse the `byAccount` half of a `GET /api/usage/stats` body.
///
/// `None` when the body is not JSON, is not an object, or omits `totalRequests`
/// — the one field the endpoint always sends. Treating a shape change as a
/// failure is deliberate: the alternative is a table of confident zeros.
///
/// A body whose `byAccount` is `{}` parses to an empty snapshot, not to `None`.
/// That is a real answer and the panel must render it as one.
pub fn parse_quota(body: &str) -> Option<QuotaSnapshot> {
    let value: Value = serde_json::from_str(body).ok()?;
    let window_requests = counter(&value, "totalRequests")?;

    let mut rows: Vec<QuotaRow> = value
        .get("byAccount")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(key, bucket)| quota_row(key, bucket))
                .collect()
        })
        .unwrap_or_default();
    sort_rows(&mut rows);

    Some(QuotaSnapshot {
        window_requests,
        rows,
    })
}

/// What `GET /api/usage/{connectionId}` reported about one account's quota.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LimitProbe {
    /// A provider reported a request ceiling.
    Reported(u64),
    /// The endpoint answered, and stated it has no quota for this provider.
    ///
    /// Carries the provider's own message so the row can quote the router rather
    /// than paraphrase it.
    NotReported(String),
    /// The probe itself did not complete.
    Rejected(ApiError),
}

impl LimitProbe {
    /// Status text for the row.
    pub fn message(&self) -> String {
        match self {
            Self::Reported(limit) => {
                format!("Provider reports a limit of {}.", format_count(*limit))
            }
            Self::NotReported(detail) => detail.clone(),
            Self::Rejected(error) => error.message().to_owned(),
        }
    }

    /// The ceiling to record, or `None` when none was reported.
    pub const fn limit(&self) -> Option<u64> {
        match self {
            Self::Reported(limit) => Some(*limit),
            Self::NotReported(_) | Self::Rejected(_) => None,
        }
    }
}

/// Interpret a `GET /api/usage/{connectionId}` response.
///
/// This build's handler answers `200` with `{"message": "Usage API not
/// implemented for {provider}", "quotas": []}`, so the expected outcome is
/// [`LimitProbe::NotReported`]. The `Reported` arm is not dead code kept for
/// symmetry: the endpoint's contract is upstream's, and a future build that
/// wires a provider quota API will start filling `quotas` without this parser
/// changing. Reading it now is what makes the panel able to show a real ceiling
/// the moment one exists, and to keep saying "not reported" until then.
pub fn settle_probe(response: Result<&str, ApiError>) -> LimitProbe {
    let body = match response {
        Ok(body) => body,
        Err(error) => return LimitProbe::Rejected(error),
    };
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return LimitProbe::Rejected(ApiError::Body);
    };

    let quotas = value
        .get("quotas")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let reported = quotas.iter().filter_map(|quota| {
        counter(quota, "limit")
            .or_else(|| counter(quota, "total"))
            .or_else(|| counter(quota, "max"))
    });
    // The largest reported ceiling: a provider that reports several windows
    // (5-hour, weekly) states the widest one last, and under-reporting a limit
    // would overstate how close an account is to it.
    if let Some(limit) = reported.max() {
        return LimitProbe::Reported(limit);
    }

    LimitProbe::NotReported(
        text(&value, "message").unwrap_or_else(|| LIMIT_NOT_REPORTED_DETAIL.to_owned()),
    )
}

/// Format a count with thousands separators.
///
/// Grouped rather than abbreviated: `1.2M` hides whether a figure is 1,150,000
/// or 1,249,999, and this panel is what a user checks before questioning a
/// charge or a rate limit.
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

// ── requests ────────────────────────────────────────────────────────────────
//
// Thin wrappers over `crate::api`, kept here so the panel holds signals and
// views only. `api::request` is itself split on `target_arch`: the wasm arm
// performs the `fetch`, the native arm returns `ApiError::Environment` rather
// than pretending to have contacted a router. That keeps every function below
// callable — and every branch above testable — on the native target.

/// `GET /api/usage/stats` for one window, joined with `GET /api/providers`.
///
/// The join is best-effort by design: when the connection list cannot be read,
/// the usage rows still render with the names their own records carried, marked
/// unmatched. Recorded usage is the reading this panel exists to show, and
/// dropping it because a second request failed would hide real data.
pub async fn load_quota(window: QuotaWindow) -> Result<QuotaSnapshot, ApiError> {
    let body = crate::api::get(window.stats_path()).await?;
    let mut snapshot = parse_quota(&body).ok_or(ApiError::Body)?;

    if let Some(connections) = crate::api::get(CONNECTIONS_PATH)
        .await
        .ok()
        .and_then(|body| crate::dashboard::providers_live::parse_connections(&body))
    {
        snapshot.join_connections(&connections);
    }
    Ok(snapshot)
}

/// `GET /api/usage/{connectionId}`, asking whether a provider limit is known.
pub async fn probe_limit(connection_id: &str) -> LimitProbe {
    let response = crate::api::get(&connection_usage_path(connection_id)).await;
    settle_probe(response.as_deref().map_err(|error| *error))
}
