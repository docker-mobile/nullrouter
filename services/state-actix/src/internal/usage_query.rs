//! Filtering, pagination, and aggregation over recorded usage.
//!
//! Backs the loopback usage endpoints that `nullrouter-api` reads for the
//! `/api/usage/*` dashboard surface. The record ring lives in
//! [`crate::usage::UsageLog`]; everything here is a read-only projection of it,
//! so no handler has to invent a number it does not have.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::usage::UsageRecord;

/// Which records a `request-details` read wants.
///
/// Every field maps to a query parameter the dashboard actually sends. A filter
/// that is `None` matches everything.
#[derive(Debug, Default)]
pub(super) struct DetailFilter {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub connection_id: Option<String>,
    pub status: Option<String>,
    /// Inclusive lower bound, epoch millis.
    pub start_ms: Option<u64>,
    /// Inclusive upper bound, epoch millis.
    pub end_ms: Option<u64>,
}

impl DetailFilter {
    /// `true` when `record` satisfies every set predicate.
    fn matches(&self, record: &UsageRecord) -> bool {
        let text_matches = |filter: Option<&String>, value: Option<&str>| {
            filter.is_none_or(|wanted| value.is_some_and(|actual| actual == wanted))
        };

        text_matches(self.provider.as_ref(), Some(record.provider.as_str()))
            && text_matches(self.model.as_ref(), Some(record.model.as_str()))
            && text_matches(self.connection_id.as_ref(), record.connection_id.as_deref())
            && text_matches(self.status.as_ref(), Some(record.status.as_str()))
            && self.start_ms.is_none_or(|start| record.timestamp >= start)
            && self.end_ms.is_none_or(|end| record.timestamp <= end)
    }
}

/// One page of filtered records, newest first, plus the totals the dashboard's
/// pager needs.
#[derive(Debug)]
pub(super) struct DetailPage {
    pub records: Vec<UsageRecord>,
    pub page: u32,
    pub page_size: u32,
    pub total_items: usize,
    pub total_pages: u32,
}

impl DetailPage {
    /// The loopback JSON body. `nullrouter-api` reshapes this into the
    /// dashboard's `details`/`pagination` envelope.
    pub(super) fn to_value(&self) -> Value {
        json!({
            "records": self.records,
            "page": self.page,
            "pageSize": self.page_size,
            "totalItems": self.total_items,
            "totalPages": self.total_pages,
        })
    }
}

/// Apply `filter`, sort newest-first, and return the requested page.
///
/// `total_items` counts every match, not just the returned page, so the pager
/// reports the real size of the result set.
pub(super) fn page_details(
    records: &[UsageRecord],
    filter: &DetailFilter,
    page: u32,
    page_size: u32,
) -> DetailPage {
    let mut matched: Vec<&UsageRecord> = records
        .iter()
        .filter(|record| filter.matches(record))
        .collect();
    // Newest first, matching upstream's `ORDER BY timestamp DESC`.
    matched.sort_by_key(|record| std::cmp::Reverse(record.timestamp));

    let total_items = matched.len();
    let page_size = page_size.max(1);
    let page = page.max(1);
    let span = usize::try_from(page_size).unwrap_or(usize::MAX);
    let total_pages = u32::try_from(total_items.div_ceil(span)).unwrap_or(u32::MAX);
    let offset = usize::try_from(page.saturating_sub(1))
        .unwrap_or(usize::MAX)
        .saturating_mul(span);

    let records = matched
        .into_iter()
        .skip(offset)
        .take(span)
        .cloned()
        .collect();

    DetailPage {
        records,
        page,
        page_size,
        total_items,
        total_pages,
    }
}

/// Per-provider counters for the `request-details` provider selector.
#[derive(Debug, Default, Clone, Copy)]
struct ProviderTotals {
    requests: u64,
    prompt_tokens: u64,
    completion_tokens: u64,
    cached_tokens: u64,
    errors: u64,
    last_used: u64,
}

impl ProviderTotals {
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
        self.last_used = self.last_used.max(record.timestamp);
    }
}

/// Distinct providers seen in `records`, with their aggregates.
///
/// Mirrors upstream's `getDistinctProviders` plus its display-name resolution:
/// `names` maps a provider id to a configured provider-node name, and ids with
/// no node fall back to the id itself.
pub(super) fn providers(records: &[UsageRecord], names: &BTreeMap<String, String>) -> Vec<Value> {
    let mut totals: BTreeMap<&str, ProviderTotals> = BTreeMap::new();
    for record in records {
        totals
            .entry(record.provider.as_str())
            .or_default()
            .add(record);
    }

    totals
        .into_iter()
        .map(|(id, total)| {
            json!({
                "id": id,
                "name": names.get(id).map_or(id, String::as_str),
                "requests": total.requests,
                "promptTokens": total.prompt_tokens,
                "completionTokens": total.completion_tokens,
                "cachedTokens": total.cached_tokens,
                "totalTokens": total.prompt_tokens.saturating_add(total.completion_tokens),
                "errors": total.errors,
                "lastUsed": total.last_used,
            })
        })
        .collect()
}

/// How recent an error must be to still be reported as the failing provider.
/// Matches upstream's 10-second window in `getUsageStats`.
const ERROR_PROVIDER_WINDOW_MS: u64 = 10_000;
/// Recent requests carried in a live snapshot, matching upstream's `.slice(0, 20)`.
const RECENT_LIMIT: usize = 20;

/// The live half of the dashboard's stats payload.
///
/// `activeRequests` and `pending` are always empty: nothing reports in-flight
/// request starts to this service, so there is no honest count to give. The rest
/// is derived from completed records.
pub(super) fn live_snapshot(records: &[UsageRecord], now_ms: u64) -> Value {
    let mut newest: Vec<&UsageRecord> = records.iter().collect();
    newest.sort_by_key(|record| std::cmp::Reverse(record.timestamp));

    let recent: Vec<Value> = newest
        .iter()
        .take(RECENT_LIMIT)
        .map(|record| {
            json!({
                "timestamp": record.timestamp,
                "model": record.model,
                "provider": record.provider,
                "promptTokens": record.prompt_tokens,
                "completionTokens": record.completion_tokens,
                "cachedTokens": record.cached_tokens,
                "status": record.status,
            })
        })
        .collect();

    // The most recent failure, and only while it is still fresh.
    let error_provider = newest
        .iter()
        .find(|record| record.status != "success")
        .filter(|record| now_ms.saturating_sub(record.timestamp) < ERROR_PROVIDER_WINDOW_MS)
        .map_or("", |record| record.provider.as_str());

    json!({
        "activeRequests": Vec::<Value>::new(),
        "recentRequests": recent,
        "errorProvider": error_provider,
        "pending": { "byModel": {}, "byAccount": {} },
    })
}

/// Aggregate usage for a single connection.
///
/// Returns the connection's own totals over `records`; the caller supplies the
/// connection metadata it already holds.
pub(super) fn connection_totals(records: &[UsageRecord], connection_id: &str) -> Value {
    let mut totals = ProviderTotals::default();
    let mut by_model: BTreeMap<&str, ProviderTotals> = BTreeMap::new();
    for record in records
        .iter()
        .filter(|record| record.connection_id.as_deref() == Some(connection_id))
    {
        totals.add(record);
        by_model
            .entry(record.model.as_str())
            .or_default()
            .add(record);
    }

    let models = Value::Object(
        by_model
            .into_iter()
            .map(|(model, total)| {
                (
                    model.to_owned(),
                    json!({
                        "requests": total.requests,
                        "promptTokens": total.prompt_tokens,
                        "completionTokens": total.completion_tokens,
                        "cachedTokens": total.cached_tokens,
                        "totalTokens": total.prompt_tokens.saturating_add(total.completion_tokens),
                        "errors": total.errors,
                        "lastUsed": total.last_used,
                    }),
                )
            })
            .collect(),
    );

    json!({
        "requests": totals.requests,
        "promptTokens": totals.prompt_tokens,
        "completionTokens": totals.completion_tokens,
        "cachedTokens": totals.cached_tokens,
        "totalTokens": totals.prompt_tokens.saturating_add(totals.completion_tokens),
        "errors": totals.errors,
        "lastUsed": totals.last_used,
        "byModel": models,
    })
}

/// Display names the dashboard's aggregate tables need, resolved from state.
///
/// Ids with no entry fall back to the id, as upstream does.
#[derive(Debug, Default)]
pub(super) struct DisplayNames {
    /// Provider id → configured provider-node name.
    pub providers: BTreeMap<String, String>,
    /// Connection id → account name.
    pub connections: BTreeMap<String, String>,
    /// API key id → key name.
    pub api_keys: BTreeMap<String, String>,
}

/// One row of an aggregate table.
#[derive(Debug, Default, Clone)]
struct Bucket {
    totals: ProviderTotals,
    /// Extra identity fields for this row, e.g. `rawModel` and `accountName`.
    labels: Vec<(&'static str, Value)>,
}

impl Bucket {
    fn to_value(&self) -> Value {
        let mut row = serde_json::Map::new();
        row.insert("requests".to_owned(), json!(self.totals.requests));
        row.insert("promptTokens".to_owned(), json!(self.totals.prompt_tokens));
        row.insert(
            "completionTokens".to_owned(),
            json!(self.totals.completion_tokens),
        );
        row.insert("cachedTokens".to_owned(), json!(self.totals.cached_tokens));
        row.insert(
            "totalTokens".to_owned(),
            json!(
                self.totals
                    .prompt_tokens
                    .saturating_add(self.totals.completion_tokens)
            ),
        );
        row.insert("errors".to_owned(), json!(self.totals.errors));
        // Pricing is not wired into usage recording, so there is no cost to
        // report. Upstream's field is kept so the dashboard's cost columns
        // render as zero rather than blank.
        row.insert("cost".to_owned(), json!(0));
        row.insert("lastUsed".to_owned(), json!(self.totals.last_used));
        for (name, value) in &self.labels {
            row.insert((*name).to_owned(), value.clone());
        }
        Value::Object(row)
    }
}

/// Collect buckets into a JSON object, preserving insertion-independent order.
fn buckets_to_value(buckets: &BTreeMap<String, Bucket>) -> Value {
    Value::Object(
        buckets
            .iter()
            .map(|(key, bucket)| (key.clone(), bucket.to_value()))
            .collect(),
    )
}

/// Aggregate `records` into the dashboard's `by*` tables.
///
/// Group keys and identity fields match upstream's `getUsageStats`, so the
/// dashboard's grouping and "Last Used" columns render from real data. Totals
/// are summed over exactly the records handed in, which is what makes the
/// caller's period window real rather than decorative.
pub(super) fn aggregate(records: &[UsageRecord], names: &DisplayNames, now_ms: u64) -> Value {
    let mut totals = ProviderTotals::default();
    for record in records {
        totals.add(record);
    }

    let mut stats = json!({
        "totalRequests": totals.requests,
        "totalPromptTokens": totals.prompt_tokens,
        "totalCompletionTokens": totals.completion_tokens,
        "totalCachedTokens": totals.cached_tokens,
        // No pricing source is wired into usage recording.
        "totalCost": 0,
        "byProvider": buckets_to_value(&group_by_provider(records)),
        "byModel": buckets_to_value(&group_by_model(records, names)),
        "byAccount": buckets_to_value(&group_by_account(records, names)),
        "byApiKey": buckets_to_value(&group_by_api_key(records, names)),
        "byEndpoint": buckets_to_value(&group_by_endpoint(records, names)),
        "last10Minutes": last_ten_minutes(records, now_ms),
    });
    // The live half: recent requests, the failing provider, and the empty
    // in-flight structures.
    if let (Some(target), Some(live)) = (
        stats.as_object_mut(),
        live_snapshot(records, now_ms).as_object().cloned(),
    ) {
        target.extend(live);
    }
    stats
}

/// A provider id's configured display name, falling back to the id.
fn provider_label(names: &DisplayNames, id: &str) -> String {
    names
        .providers
        .get(id)
        .cloned()
        .unwrap_or_else(|| id.to_owned())
}

/// `byProvider`, keyed by provider id.
fn group_by_provider(records: &[UsageRecord]) -> BTreeMap<String, Bucket> {
    let mut grouped: BTreeMap<String, Bucket> = BTreeMap::new();
    for record in records {
        grouped
            .entry(record.provider.clone())
            .or_default()
            .totals
            .add(record);
    }
    grouped
}

/// `byModel`, keyed `model (provider)` as upstream does.
fn group_by_model(records: &[UsageRecord], names: &DisplayNames) -> BTreeMap<String, Bucket> {
    let mut grouped: BTreeMap<String, Bucket> = BTreeMap::new();
    for record in records {
        let key = format!("{} ({})", record.model, record.provider);
        grouped
            .entry(key)
            .or_insert_with(|| Bucket {
                labels: vec![
                    ("rawModel", json!(record.model)),
                    ("provider", json!(provider_label(names, &record.provider))),
                ],
                ..Bucket::default()
            })
            .totals
            .add(record);
    }
    grouped
}

/// `byAccount`, keyed `model (provider - accountName)`. Records with no
/// connection are skipped, as upstream skips them.
fn group_by_account(records: &[UsageRecord], names: &DisplayNames) -> BTreeMap<String, Bucket> {
    let mut grouped: BTreeMap<String, Bucket> = BTreeMap::new();
    for record in records {
        let Some(connection_id) = record.connection_id.as_deref() else {
            continue;
        };
        let account_name = names
            .connections
            .get(connection_id)
            .cloned()
            .unwrap_or_else(|| format!("Account {}", short_id(connection_id)));
        let key = format!("{} ({} - {account_name})", record.model, record.provider);
        grouped
            .entry(key)
            .or_insert_with(|| Bucket {
                labels: vec![
                    ("rawModel", json!(record.model)),
                    ("provider", json!(provider_label(names, &record.provider))),
                    ("connectionId", json!(connection_id)),
                    ("accountName", json!(account_name)),
                ],
                ..Bucket::default()
            })
            .totals
            .add(record);
    }
    grouped
}

/// `byApiKey`, keyed `keyId|model|provider`, or `local-no-key` when the request
/// carried no managed key.
///
/// Records store the key's id, never its secret, so there is nothing to mask:
/// `apiKeyMasked` is null and the id carries the grouping.
fn group_by_api_key(records: &[UsageRecord], names: &DisplayNames) -> BTreeMap<String, Bucket> {
    let mut grouped: BTreeMap<String, Bucket> = BTreeMap::new();
    for record in records {
        let (key, key_name) = record.api_key_id.as_deref().map_or_else(
            || ("local-no-key".to_owned(), "Local (No API Key)".to_owned()),
            |id| {
                let name = names
                    .api_keys
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| short_id(id).to_owned());
                (format!("{id}|{}|{}", record.model, record.provider), name)
            },
        );
        grouped
            .entry(key.clone())
            .or_insert_with(|| Bucket {
                labels: vec![
                    ("rawModel", json!(record.model)),
                    ("provider", json!(provider_label(names, &record.provider))),
                    ("apiKeyMasked", Value::Null),
                    ("keyName", json!(key_name)),
                    ("apiKeyKey", json!(key)),
                ],
                ..Bucket::default()
            })
            .totals
            .add(record);
    }
    grouped
}

/// `byEndpoint`, keyed `endpoint|model|provider`.
fn group_by_endpoint(records: &[UsageRecord], names: &DisplayNames) -> BTreeMap<String, Bucket> {
    let mut grouped: BTreeMap<String, Bucket> = BTreeMap::new();
    for record in records {
        let endpoint = record.endpoint.as_deref().unwrap_or("Unknown");
        let key = format!("{endpoint}|{}|{}", record.model, record.provider);
        grouped
            .entry(key)
            .or_insert_with(|| Bucket {
                labels: vec![
                    ("endpoint", json!(endpoint)),
                    ("rawModel", json!(record.model)),
                    ("provider", json!(provider_label(names, &record.provider))),
                ],
                ..Bucket::default()
            })
            .totals
            .add(record);
    }
    grouped
}

/// First 8 characters of an id, as upstream's `slice(0, 8)` labels do.
fn short_id(id: &str) -> &str {
    id.char_indices()
        .nth(8)
        .map_or(id, |(index, _)| id.get(..index).unwrap_or(id))
}

/// Per-minute request counts over the last 10 minutes, oldest first.
fn last_ten_minutes(records: &[UsageRecord], now_ms: u64) -> Vec<Value> {
    const MINUTE_MS: u64 = 60 * 1000;
    // Align to minute boundaries, as upstream's bucket map does.
    let current_minute = (now_ms / MINUTE_MS).saturating_mul(MINUTE_MS);
    let window_start = current_minute.saturating_sub(9 * MINUTE_MS);
    let mut buckets = vec![(0_u64, 0_u64, 0_u64); 10];

    for record in records {
        if record.timestamp < window_start || record.timestamp > now_ms {
            continue;
        }
        let offset = record.timestamp.saturating_sub(window_start) / MINUTE_MS;
        let index = usize::try_from(offset).unwrap_or(9).min(9);
        if let Some(bucket) = buckets.get_mut(index) {
            bucket.0 = bucket.0.saturating_add(1);
            bucket.1 = bucket.1.saturating_add(record.prompt_tokens);
            bucket.2 = bucket.2.saturating_add(record.completion_tokens);
        }
    }

    buckets
        .into_iter()
        .enumerate()
        .map(|(index, (requests, prompt, completion))| {
            let minute = u64::try_from(index).unwrap_or(0);
            json!({
                "timestamp": window_start.saturating_add(minute * MINUTE_MS),
                "requests": requests,
                "promptTokens": prompt,
                "completionTokens": completion,
                "tokens": prompt.saturating_add(completion),
                "cost": 0,
            })
        })
        .collect()
}

/// Parse a dashboard date filter into epoch millis.
///
/// Accepts what upstream's `new Date(value)` accepts in practice: this store's
/// own `unix-ms:` stamps, bare epoch millis, and ISO-8601 with or without a time
/// or zone. `None` when the value is not a date, so a malformed filter is
/// rejected rather than silently ignored.
pub(super) fn parse_date_ms(value: &str) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    // `unix-ms:1699…` is this store's own timestamp encoding.
    if let Some(millis) = value.strip_prefix("unix-ms:") {
        return millis.trim().parse::<u64>().ok();
    }
    // Bare epoch millis, as sent by anything reading our own record timestamps.
    if value.bytes().all(|byte| byte.is_ascii_digit()) {
        return value.parse::<u64>().ok();
    }
    parse_iso_ms(value)
}

/// Parse `YYYY-MM-DD[THH:MM[:SS[.fff]]][Z|±HH:MM]`.
fn parse_iso_ms(value: &str) -> Option<u64> {
    let (date_part, rest) = value
        .split_once(['T', 't'])
        .map_or((value, ""), |(date, rest)| (date, rest));

    let mut date_fields = date_part.split('-');
    let year: i64 = date_fields.next()?.parse().ok()?;
    let month: u32 = date_fields.next().map_or(Ok(1), str::parse).ok()?;
    let day: u32 = date_fields.next().map_or(Ok(1), str::parse).ok()?;
    if date_fields.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let (time_part, offset_ms) = split_zone(rest)?;
    let (hour, minute, second, milli) = parse_time(time_part)?;

    let days = days_from_civil(year, month, day);
    let millis = days
        .checked_mul(86_400_000)?
        .checked_add(i64::from(hour) * 3_600_000)?
        .checked_add(i64::from(minute) * 60_000)?
        .checked_add(i64::from(second) * 1000)?
        .checked_add(i64::from(milli))?
        .checked_sub(offset_ms)?;
    u64::try_from(millis).ok()
}

/// Split a trailing zone designator off a time, returning its offset in millis.
///
/// A bare local time carries no offset, matching how the dashboard's
/// `datetime-local` inputs are read.
fn split_zone(time: &str) -> Option<(&str, i64)> {
    if let Some(stripped) = time.strip_suffix(['Z', 'z']) {
        return Some((stripped, 0));
    }
    // Scan from the end so the sign of a zone offset is not confused with a
    // date separator earlier in the string.
    for (index, byte) in time.bytes().enumerate().rev() {
        if byte == b'+' || byte == b'-' {
            let (head, zone) = (time.get(..index)?, time.get(index.saturating_add(1)..)?);
            let mut fields = zone.split(':');
            let hours: i64 = fields.next()?.parse().ok()?;
            let minutes: i64 = fields.next().map_or(Ok(0), str::parse).ok()?;
            if fields.next().is_some() {
                return None;
            }
            let magnitude = hours
                .checked_mul(3_600_000)?
                .checked_add(minutes.checked_mul(60_000)?)?;
            return Some((head, if byte == b'-' { -magnitude } else { magnitude }));
        }
    }
    Some((time, 0))
}

/// Parse `HH:MM[:SS[.fff]]`, defaulting an absent time to midnight.
fn parse_time(time: &str) -> Option<(u32, u32, u32, u32)> {
    if time.is_empty() {
        return Some((0, 0, 0, 0));
    }
    let mut fields = time.split(':');
    let hour: u32 = fields.next()?.parse().ok()?;
    let minute: u32 = fields.next().map_or(Ok(0), str::parse).ok()?;
    let seconds_field = fields.next().unwrap_or("0");
    if fields.next().is_some() {
        return None;
    }
    let (second_text, fraction) = seconds_field
        .split_once('.')
        .map_or((seconds_field, "0"), |(second, fraction)| {
            (second, fraction)
        });
    let second: u32 = second_text.parse().ok()?;
    // Fractional seconds are truncated to millisecond precision.
    let milli: u32 = format!("{fraction:0<3}").get(..3)?.parse().ok()?;

    (hour <= 23 && minute <= 59 && second <= 60).then_some((hour, minute, second, milli))
}

/// Days between 1970-01-01 and `year-month-day`, proleptic Gregorian.
///
/// Howard Hinnant's `days_from_civil`, which is exact for every year in range
/// and needs no date library.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let month = i64::from(month);
    let day = i64::from(day);
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::{
        DetailFilter, DisplayNames, aggregate, connection_totals, days_from_civil, live_snapshot,
        page_details, parse_date_ms, providers,
    };
    use crate::usage::{UsageInput, UsageLog, UsageRecord};
    use serde_json::json;
    use std::collections::BTreeMap;

    fn log() -> UsageLog {
        let mut log = UsageLog::default();
        log.record(
            UsageInput {
                provider: "openai".to_owned(),
                model: "gpt-5".to_owned(),
                connection_id: Some("conn_1".to_owned()),
                prompt_tokens: 10,
                completion_tokens: 5,
                ..UsageInput::default()
            },
            1000,
        );
        log.record(
            UsageInput {
                provider: "anthropic".to_owned(),
                model: "claude-sonnet-4.5".to_owned(),
                connection_id: Some("conn_2".to_owned()),
                status: Some("error".to_owned()),
                prompt_tokens: 3,
                ..UsageInput::default()
            },
            2000,
        );
        log.record(
            UsageInput {
                provider: "openai".to_owned(),
                model: "gpt-5-mini".to_owned(),
                connection_id: Some("conn_1".to_owned()),
                prompt_tokens: 7,
                completion_tokens: 2,
                ..UsageInput::default()
            },
            3000,
        );
        log
    }

    fn records(log: &UsageLog) -> Vec<UsageRecord> {
        log.records.clone()
    }

    #[test]
    fn each_filter_narrows_the_result_set() {
        let log = log();
        let all = records(&log);

        let unfiltered = page_details(&all, &DetailFilter::default(), 1, 50);
        assert_eq!(unfiltered.total_items, 3);
        // Newest first.
        assert_eq!(
            unfiltered.records.first().map(|r| r.model.as_str()),
            Some("gpt-5-mini")
        );

        let by_provider = page_details(
            &all,
            &DetailFilter {
                provider: Some("openai".to_owned()),
                ..DetailFilter::default()
            },
            1,
            50,
        );
        assert_eq!(by_provider.total_items, 2);

        let by_model = page_details(
            &all,
            &DetailFilter {
                model: Some("gpt-5".to_owned()),
                ..DetailFilter::default()
            },
            1,
            50,
        );
        assert_eq!(by_model.total_items, 1);

        let by_connection = page_details(
            &all,
            &DetailFilter {
                connection_id: Some("conn_2".to_owned()),
                ..DetailFilter::default()
            },
            1,
            50,
        );
        assert_eq!(by_connection.total_items, 1);

        let by_status = page_details(
            &all,
            &DetailFilter {
                status: Some("error".to_owned()),
                ..DetailFilter::default()
            },
            1,
            50,
        );
        assert_eq!(by_status.total_items, 1);

        let by_window = page_details(
            &all,
            &DetailFilter {
                start_ms: Some(2000),
                end_ms: Some(2000),
                ..DetailFilter::default()
            },
            1,
            50,
        );
        assert_eq!(by_window.total_items, 1);
        assert_eq!(
            by_window.records.first().map(|r| r.provider.as_str()),
            Some("anthropic")
        );
    }

    #[test]
    fn pagination_splits_matches_without_losing_the_total() {
        let log = log();
        let all = records(&log);

        let first = page_details(&all, &DetailFilter::default(), 1, 2);
        assert_eq!(first.records.len(), 2);
        assert_eq!(first.total_items, 3);
        assert_eq!(first.total_pages, 2);

        let second = page_details(&all, &DetailFilter::default(), 2, 2);
        assert_eq!(second.records.len(), 1);
        // The total describes the whole result set, not this page.
        assert_eq!(second.total_items, 3);
        // No record appears on both pages.
        assert!(
            second
                .records
                .iter()
                .all(|late| first.records.iter().all(|early| early.id != late.id))
        );

        // Past the end is empty, not an error.
        let past_end = page_details(&all, &DetailFilter::default(), 99, 2);
        assert!(past_end.records.is_empty());
        assert_eq!(past_end.total_items, 3);
    }

    #[test]
    fn providers_are_distinct_with_resolved_names() {
        let log = log();
        let mut names = BTreeMap::new();
        names.insert("openai".to_owned(), "My OpenAI Node".to_owned());

        let listed = providers(&records(&log), &names);
        assert_eq!(listed.len(), 2);
        let anthropic = listed.first().expect("first provider");
        assert_eq!(anthropic.get("id"), Some(&json!("anthropic")));
        // With no configured node, the name falls back to the id.
        assert_eq!(anthropic.get("name"), Some(&json!("anthropic")));
        assert_eq!(anthropic.get("errors"), Some(&json!(1)));

        let openai = listed.get(1).expect("second provider");
        assert_eq!(openai.get("name"), Some(&json!("My OpenAI Node")));
        assert_eq!(openai.get("requests"), Some(&json!(2)));
        assert_eq!(openai.get("totalTokens"), Some(&json!(24)));
    }

    #[test]
    fn live_snapshot_reports_recent_records_and_no_invented_pending() {
        let log = log();
        // 2500 is within 10s of the error at 2000.
        let live = live_snapshot(&records(&log), 2500);
        assert_eq!(
            live.pointer("/recentRequests/0/model"),
            Some(&json!("gpt-5-mini"))
        );
        assert_eq!(live.get("errorProvider"), Some(&json!("anthropic")));
        // Nothing tracks in-flight requests, so these stay empty.
        assert_eq!(live.get("activeRequests"), Some(&json!([])));
        assert_eq!(live.pointer("/pending/byModel"), Some(&json!({})));

        // Well past the window, the error provider is no longer reported.
        let stale = live_snapshot(&records(&log), 2000 + 60_000);
        assert_eq!(stale.get("errorProvider"), Some(&json!("")));
    }

    #[test]
    fn connection_totals_cover_only_that_connection() {
        let log = log();
        let totals = connection_totals(&records(&log), "conn_1");
        assert_eq!(totals.get("requests"), Some(&json!(2)));
        assert_eq!(totals.get("promptTokens"), Some(&json!(17)));
        assert_eq!(totals.pointer("/byModel/gpt-5/requests"), Some(&json!(1)));
        // The other connection's record is excluded.
        assert!(totals.pointer("/byModel/claude-sonnet-4.5").is_none());

        let unknown = connection_totals(&records(&log), "conn_missing");
        assert_eq!(unknown.get("requests"), Some(&json!(0)));
    }

    #[test]
    fn aggregate_uses_upstream_group_keys_and_display_labels() {
        let mut log = UsageLog::default();
        log.record(
            UsageInput {
                provider: "openai".to_owned(),
                model: "gpt-5".to_owned(),
                connection_id: Some("conn_abcdefghij".to_owned()),
                api_key_id: Some("key_1".to_owned()),
                endpoint: Some("/v1/chat/completions".to_owned()),
                prompt_tokens: 10,
                completion_tokens: 5,
                cached_tokens: 2,
                ..UsageInput::default()
            },
            1000,
        );

        let mut names = DisplayNames::default();
        names
            .providers
            .insert("openai".to_owned(), "My OpenAI".to_owned());
        names
            .api_keys
            .insert("key_1".to_owned(), "CI key".to_owned());

        let stats = aggregate(&records(&log), &names, 60_000);
        assert_eq!(stats.get("totalRequests"), Some(&json!(1)));
        assert_eq!(stats.get("totalCachedTokens"), Some(&json!(2)));
        // Upstream group keys.
        assert_eq!(
            stats.pointer("/byModel/gpt-5 (openai)/rawModel"),
            Some(&json!("gpt-5"))
        );
        // Provider node names resolve into the display field.
        assert_eq!(
            stats.pointer("/byModel/gpt-5 (openai)/provider"),
            Some(&json!("My OpenAI"))
        );
        // With no configured connection name, the account label is derived from
        // the id, as upstream's `Account xxxxxxxx` fallback does.
        assert_eq!(
            stats.pointer("/byAccount/gpt-5 (openai - Account conn_abc)/connectionId"),
            Some(&json!("conn_abcdefghij"))
        );
        assert_eq!(
            stats.pointer("/byApiKey/key_1|gpt-5|openai/keyName"),
            Some(&json!("CI key"))
        );
        // Secrets are never in a record, so there is nothing to mask.
        assert_eq!(
            stats.pointer("/byApiKey/key_1|gpt-5|openai/apiKeyMasked"),
            Some(&json!(null))
        );
        assert_eq!(
            stats.pointer("/byEndpoint/~1v1~1chat~1completions|gpt-5|openai/endpoint"),
            Some(&json!("/v1/chat/completions"))
        );
        // The live half travels with the aggregate.
        assert_eq!(stats.get("activeRequests"), Some(&json!([])));
        assert_eq!(stats.pointer("/pending/byAccount"), Some(&json!({})));
        assert_eq!(
            stats
                .get("last10Minutes")
                .and_then(|series| series.as_array())
                .map(Vec::len),
            Some(10)
        );
    }

    #[test]
    fn aggregate_totals_cover_only_the_records_given() {
        let log = log();
        let all = records(&log);
        let names = DisplayNames::default();

        // Every record.
        let full = aggregate(&all, &names, 4000);
        assert_eq!(full.get("totalRequests"), Some(&json!(3)));

        // A window: only records at or after 2000, as the caller filtered them.
        let windowed: Vec<UsageRecord> = all
            .iter()
            .filter(|record| record.timestamp >= 2000)
            .cloned()
            .collect();
        let partial = aggregate(&windowed, &names, 4000);
        assert_eq!(partial.get("totalRequests"), Some(&json!(2)));
        // The excluded record's model is absent from the tables too.
        assert!(partial.pointer("/byModel/gpt-5 (openai)").is_none());
    }

    #[test]
    fn dates_parse_from_every_shape_the_dashboard_sends() {
        // Epoch millis, bare and prefixed.
        assert_eq!(parse_date_ms("1700000000000"), Some(1_700_000_000_000));
        assert_eq!(
            parse_date_ms("unix-ms:1700000000000"),
            Some(1_700_000_000_000)
        );
        // The epoch itself.
        assert_eq!(parse_date_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_date_ms("1970-01-01"), Some(0));
        // A known instant: 2024-01-01T00:00:00Z.
        assert_eq!(
            parse_date_ms("2024-01-01T00:00:00Z"),
            Some(1_704_067_200_000)
        );
        // `datetime-local` sends no zone.
        assert_eq!(parse_date_ms("2024-01-01T00:00"), Some(1_704_067_200_000));
        // Zone offsets shift the instant.
        assert_eq!(
            parse_date_ms("2024-01-01T01:00+01:00"),
            Some(1_704_067_200_000)
        );
        assert_eq!(
            parse_date_ms("2023-12-31T23:00-01:00"),
            Some(1_704_067_200_000)
        );
        // Fractional seconds are kept to millis.
        assert_eq!(
            parse_date_ms("2024-01-01T00:00:00.250Z"),
            Some(1_704_067_200_250)
        );

        // Non-dates are rejected rather than silently treated as 0.
        assert_eq!(parse_date_ms("yesterday"), None);
        assert_eq!(parse_date_ms(""), None);
        assert_eq!(parse_date_ms("2024-13-01"), None);
        assert_eq!(parse_date_ms("2024-01-32"), None);
        assert_eq!(parse_date_ms("2024-01-01T25:00Z"), None);
    }

    #[test]
    fn civil_days_match_known_epochs() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(1970, 1, 2), 1);
        assert_eq!(days_from_civil(1969, 12, 31), -1);
        // A leap day lands where the calendar says it does.
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(days_from_civil(2024, 1, 1), 19_723);
    }
}
