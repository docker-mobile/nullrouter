//! Live usage parsing, ordering, formatting, and SSE decoding.
//!
//! The panel these back used to render a fixture of zeros, so the property under
//! test throughout is that the UI can never present a number the server did not
//! send: a shape change parses to `None` (a visible failure), a genuinely quiet
//! router parses to an empty state, and a counter absent from an SSE frame stays
//! absent.

use nullrouter_dashboard_wasm::dashboard::usage_live::{
    LiveUsage, NO_READING, UsageBreakdownRow, UsageMinute, UsagePeriod, format_age, format_cost,
    format_count, format_latency, format_optional_count, parse_logs, parse_stats,
    parse_usage_frame, sort_breakdown, sparkline, sparkline_summary,
};

/// A stats body in the shape `UsageLog::stats` produces, with traffic on two
/// providers and three models.
const FULL_STATS: &str = r#"{
  "totalRequests": 1234567,
  "totalPromptTokens": 89012,
  "totalCompletionTokens": 34567,
  "totalCachedTokens": 4096,
  "totalCost": 12.5,
  "byProvider": {
    "anthropic": {"requests": 40, "promptTokens": 400, "completionTokens": 200, "cachedTokens": 10, "totalTokens": 600, "errors": 2, "cost": 1.25},
    "openai": {"requests": 100, "promptTokens": 1000, "completionTokens": 500, "cachedTokens": 20, "totalTokens": 1500, "errors": 0, "cost": 3.5}
  },
  "byModel": {
    "claude-sonnet-4.5": {"requests": 40, "promptTokens": 400, "completionTokens": 200, "cachedTokens": 10, "totalTokens": 600, "errors": 2, "cost": 1.25},
    "gpt-5": {"requests": 90, "promptTokens": 900, "completionTokens": 450, "cachedTokens": 20, "totalTokens": 1350, "errors": 0, "cost": 3.0},
    "gpt-5-mini": {"requests": 10, "promptTokens": 100, "completionTokens": 50, "cachedTokens": 0, "totalTokens": 150, "errors": 1, "cost": 0.5}
  },
  "byAccount": {},
  "byApiKey": {},
  "byEndpoint": {"/v1/chat/completions": {"requests": 140, "promptTokens": 1400, "completionTokens": 700, "cachedTokens": 30, "totalTokens": 2100, "errors": 2, "cost": 4.75}},
  "last10Minutes": [
    {"timestamp": 1000, "requests": 0, "tokens": 0},
    {"timestamp": 61000, "requests": 1, "tokens": 15},
    {"timestamp": 121000, "requests": 8, "tokens": 900},
    {"timestamp": 181000, "requests": 0, "tokens": 0}
  ]
}"#;

/// The body a router that has served nothing returns.
const ZERO_STATS: &str = r#"{
  "totalRequests": 0,
  "totalPromptTokens": 0,
  "totalCompletionTokens": 0,
  "totalCachedTokens": 0,
  "totalCost": 0,
  "byProvider": {},
  "byModel": {},
  "byAccount": {},
  "byApiKey": {},
  "byEndpoint": {},
  "last10Minutes": []
}"#;

#[test]
fn full_stats_body_parses_every_total_and_breakdown() {
    // Given: a realistic stats body with traffic across providers and models.
    let stats = parse_stats(FULL_STATS).expect("the documented shape parses");

    // Then: the totals are the server's numbers, not a placeholder.
    assert_eq!(stats.total_requests, 1_234_567);
    assert_eq!(stats.total_prompt_tokens, 89_012);
    assert_eq!(stats.total_completion_tokens, 34_567);
    assert_eq!(stats.total_cached_tokens, 4096);
    assert!((stats.total_cost - 12.5).abs() < f64::EPSILON);
    assert_eq!(stats.total_tokens(), 89_012 + 34_567);

    // And: both breakdowns carry every bucket, with per-bucket counters intact.
    assert_eq!(stats.by_provider.len(), 2);
    assert_eq!(stats.by_model.len(), 3);
    let busiest = stats.by_provider.first().expect("a provider row");
    assert_eq!(busiest.name, "openai");
    assert_eq!(busiest.requests, 100);
    assert_eq!(busiest.prompt_tokens, 1000);
    assert_eq!(busiest.completion_tokens, 500);
    assert_eq!(busiest.cached_tokens, 20);
    assert_eq!(busiest.total_tokens, 1500);
    assert_eq!(busiest.errors, 0);

    // And: an error count is preserved, so a failing provider is visible.
    let anthropic = stats
        .by_provider
        .iter()
        .find(|row| row.name == "anthropic")
        .expect("anthropic row");
    assert_eq!(anthropic.errors, 2);

    // And: the 10-minute series keeps its order and its empty minutes.
    assert_eq!(stats.last_ten_minutes.len(), 4);
    assert_eq!(
        stats
            .last_ten_minutes
            .first()
            .map(|minute| minute.timestamp),
        Some(1000)
    );
    assert_eq!(
        stats.last_ten_minutes.get(2).map(|minute| minute.requests),
        Some(8)
    );

    // And: it is not empty, so the panel renders data rather than an empty state.
    assert!(!stats.is_empty());
}

#[test]
fn an_all_zero_body_is_an_explicit_empty_state_not_a_failure() {
    // Given: the router answered, and has recorded nothing.
    let stats = parse_stats(ZERO_STATS).expect("a zeroed body is still a valid answer");

    // Then: it parses — a failure would claim the request itself went wrong.
    assert_eq!(stats.total_requests, 0);
    assert!(stats.by_provider.is_empty());
    assert!(stats.by_model.is_empty());

    // And: it reports itself as empty, which is what drives the
    // "no requests recorded yet" copy instead of a table of zeros.
    assert!(stats.is_empty());

    // And: the empty series summarises as nothing recorded, not as a gap in data.
    assert_eq!(
        sparkline_summary(&stats.last_ten_minutes),
        "No per-minute activity has been reported for the last 10 minutes."
    );
    assert!(sparkline(&stats.last_ten_minutes).is_empty());
}

#[test]
fn a_malformed_or_empty_body_fails_rather_than_rendering_zeros() {
    // A body that is not JSON at all.
    assert!(parse_stats("").is_none());
    assert!(parse_stats("not json").is_none());
    assert!(parse_stats("<html>502 Bad Gateway</html>").is_none());
    // Valid JSON of the wrong kind.
    assert!(parse_stats("[]").is_none());
    assert!(parse_stats("null").is_none());
    assert!(parse_stats("42").is_none());
    // An object that omits the one field the endpoint always sends: a contract
    // change must surface as a failure, not as a quiet router.
    assert!(parse_stats("{}").is_none());
    assert!(parse_stats(r#"{"byProvider":{}}"#).is_none());
    // Truncated mid-body.
    assert!(parse_stats(r#"{"totalRequests": 5, "byProvider": {"#).is_none());
}

#[test]
fn breakdown_rows_are_ordered_busiest_first_with_a_stable_tiebreak() {
    // Given: a parsed body whose maps arrive alphabetically from the server.
    let stats = parse_stats(FULL_STATS).expect("parses");

    // Then: providers are ordered by requests descending, not alphabetically.
    let providers: Vec<&str> = stats
        .by_provider
        .iter()
        .map(|row| row.name.as_str())
        .collect();
    assert_eq!(providers, ["openai", "anthropic"]);

    // And: so are models.
    let models: Vec<&str> = stats.by_model.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(models, ["gpt-5", "claude-sonnet-4.5", "gpt-5-mini"]);

    // And: equal request counts break by tokens, then by name, so rows do not
    // shuffle between polls.
    let mut rows = vec![
        row("zeta", 5, 10),
        row("alpha", 5, 10),
        row("middle", 5, 99),
        row("busiest", 9, 1),
    ];
    sort_breakdown(&mut rows);
    let ordered: Vec<&str> = rows.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(ordered, ["busiest", "middle", "alpha", "zeta"]);

    // And: sorting again does not move anything.
    let before = ordered.join(",");
    sort_breakdown(&mut rows);
    let after: Vec<&str> = rows.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(after.join(","), before);
}

#[test]
fn a_row_reports_its_share_without_dividing_by_a_zero_total() {
    let busiest = row("openai", 100, 1500);
    assert_eq!(busiest.share_percent(200), 50);
    assert_eq!(busiest.share_percent(100), 100);
    // A row cannot exceed the whole, even if the totals disagree.
    assert_eq!(busiest.share_percent(10), 100);
    // A zero total is not a division.
    assert_eq!(busiest.share_percent(0), 0);
}

#[test]
fn large_counts_are_grouped_so_a_bill_can_be_read() {
    // Grouped, never abbreviated: "1.2M" hides which million it was.
    assert_eq!(format_count(0), "0");
    assert_eq!(format_count(7), "7");
    assert_eq!(format_count(999), "999");
    assert_eq!(format_count(1000), "1,000");
    assert_eq!(format_count(12_345), "12,345");
    assert_eq!(format_count(999_999), "999,999");
    assert_eq!(format_count(1_234_567), "1,234,567");
    assert_eq!(format_count(1_000_000_000), "1,000,000,000");
    assert_eq!(format_count(u64::MAX), "18,446,744,073,709,551,615");

    // A counter the server did not send is marked as such, never shown as zero.
    assert_eq!(format_optional_count(Some(1000)), "1,000");
    assert_eq!(format_optional_count(Some(0)), "0");
    assert_eq!(format_optional_count(None), NO_READING);
    assert_ne!(format_optional_count(None), format_optional_count(Some(0)));

    // Cost keeps cents; a non-finite value is no reading rather than "NaN".
    assert_eq!(format_cost(0.0), "$0.00");
    assert_eq!(format_cost(12.5), "$12.50");
    assert_eq!(format_cost(1234.567), "$1234.57");
    assert_eq!(format_cost(f64::NAN), NO_READING);
    assert_eq!(format_cost(f64::INFINITY), NO_READING);

    // Latency switches unit at a second, with no float rounding on the way.
    assert_eq!(format_latency(0), "0 ms");
    assert_eq!(format_latency(999), "999 ms");
    assert_eq!(format_latency(1000), "1.0 s");
    assert_eq!(format_latency(1450), "1.4 s");
    assert_eq!(format_latency(62_500), "62.5 s");
}

#[test]
fn ages_are_relative_and_never_negative() {
    // Far enough past the epoch that a multi-day offset is a real subtraction.
    let now: u64 = 1_700_000_000_000;
    assert_eq!(format_age(now, now), "just now");
    assert_eq!(format_age(now - 3_000, now), "just now");
    assert_eq!(format_age(now - 30_000, now), "30s ago");
    assert_eq!(format_age(now - 5 * 60_000, now), "5m ago");
    assert_eq!(format_age(now - 3 * 3_600_000, now), "3h ago");
    assert_eq!(format_age(now - 2 * 86_400_000, now), "2d ago");
    // Clock skew between browser and router must not print a negative age.
    assert_eq!(format_age(now + 60_000, now), "just now");
}

#[test]
fn the_minute_strip_scales_to_its_peak_and_keeps_quiet_minutes_visible() {
    let series = [
        UsageMinute {
            timestamp: 0,
            requests: 0,
            tokens: 0,
        },
        UsageMinute {
            timestamp: 60_000,
            requests: 1,
            tokens: 15,
        },
        UsageMinute {
            timestamp: 120_000,
            requests: 100,
            tokens: 9000,
        },
    ];
    let bars = sparkline(&series);
    assert_eq!(bars.len(), 3);

    // A minute with no traffic is flat; one request is not, even against a peak
    // of 100, or the strip would claim the minute was idle.
    assert_eq!(bars.first().map(|bar| bar.height_percent), Some(0));
    let quiet = bars.get(1).expect("second bar");
    assert!(quiet.height_percent > 0, "one request must be visible");
    assert_eq!(bars.get(2).map(|bar| bar.height_percent), Some(100));

    // Labels count back from the newest minute.
    assert_eq!(
        bars.get(2).map(|bar| bar.label.as_str()),
        Some("this minute")
    );
    assert_eq!(
        bars.first().map(|bar| bar.label.as_str()),
        Some("2 min ago")
    );

    // The text alternative carries the same information as the bars.
    let summary = sparkline_summary(&series);
    assert!(summary.contains("101 requests"), "{summary}");
    assert!(summary.contains("9,015 tokens"), "{summary}");
    assert!(summary.contains("peaking at 100"), "{summary}");

    // A series that exists but saw nothing says so, distinctly from no series.
    let idle = [UsageMinute::default(), UsageMinute::default()];
    assert_eq!(
        sparkline_summary(&idle),
        "No requests in the last 10 minutes."
    );
}

#[test]
fn the_log_endpoint_parses_newest_first_and_an_empty_log_is_valid() {
    let body = r#"[
      {"id":"req_1","timestamp":1000,"provider":"openai","model":"gpt-5","connectionId":"conn_1","endpoint":"/v1/chat/completions","status":"success","statusCode":200,"promptTokens":10,"completionTokens":5,"totalTokens":15,"latencyMs":420,"error":null},
      {"id":"req_2","timestamp":9000,"provider":"anthropic","model":"claude-sonnet-4.5","connectionId":null,"endpoint":null,"status":"error","statusCode":429,"promptTokens":3,"completionTokens":0,"totalTokens":3,"latencyMs":1800,"error":"rate limited"}
    ]"#;
    let entries = parse_logs(body).expect("an array of records parses");

    // Newest first, whatever order the server sent.
    assert_eq!(entries.len(), 2);
    let newest = entries.first().expect("newest");
    assert_eq!(newest.id, "req_2");
    assert_eq!(newest.provider, "anthropic");
    assert_eq!(newest.status_code, Some(429));
    assert!(newest.failed());
    assert_eq!(newest.status_class(), "is-degraded");
    assert_eq!(newest.error.as_deref(), Some("rate limited"));
    // A null endpoint stays absent rather than becoming an empty label.
    assert_eq!(newest.endpoint, None);

    let oldest = entries.get(1).expect("oldest");
    assert_eq!(oldest.id, "req_1");
    assert!(!oldest.failed());
    assert_eq!(oldest.status_class(), "is-connected");
    assert_eq!(oldest.endpoint.as_deref(), Some("/v1/chat/completions"));
    assert_eq!(oldest.latency_ms, 420);

    // An empty log is a real answer; a non-array is not.
    assert_eq!(parse_logs("[]").map(|empty| empty.len()), Some(0));
    assert!(parse_logs("{}").is_none());
    assert!(parse_logs("").is_none());
    assert!(parse_logs("not json").is_none());
}

#[test]
fn a_usage_frame_decodes_its_counters_and_recent_requests() {
    let data = r#"{
      "liveTelemetry": true,
      "activeRequests": 2,
      "requestsToday": 1500,
      "tokensToday": 250000,
      "estimatedCost": "$4.20",
      "recentRequests": [
        {"id":"req_9","timestamp":9000,"provider":"openai","model":"gpt-5","status":"success","statusCode":200,"totalTokens":15,"latencyMs":300}
      ]
    }"#;
    let frame = parse_usage_frame(data).expect("a usage frame decodes");

    assert!(frame.live_telemetry);
    assert_eq!(frame.active_requests, Some(2));
    assert_eq!(frame.requests_today, Some(1500));
    assert_eq!(frame.tokens_today, Some(250_000));
    assert_eq!(frame.estimated_cost.as_deref(), Some("$4.20"));
    assert_eq!(frame.recent_requests.len(), 1);
    assert_eq!(
        frame
            .recent_requests
            .first()
            .map(|entry| entry.model.as_str()),
        Some("gpt-5")
    );

    // Only the counters that moved pulse, so the whole row does not flash on
    // every 2-second poll.
    let previous = LiveUsage {
        live_telemetry: true,
        active_requests: Some(2),
        requests_today: Some(1499),
        tokens_today: Some(250_000),
        estimated_cost: Some("$4.20".to_owned()),
        recent_requests: Vec::new(),
    };
    let changes = frame.changes_from(&previous);
    assert!(changes.requests_today);
    assert!(!changes.active_requests);
    assert!(!changes.tokens_today);
    assert!(!changes.estimated_cost);

    // The first frame after mount moves everything off the default.
    let first = frame.changes_from(&LiveUsage::default());
    assert!(first.active_requests && first.requests_today && first.tokens_today);

    // A frame that is not an object is not a frame.
    assert!(parse_usage_frame("[]").is_none());
    assert!(parse_usage_frame("").is_none());
    assert!(parse_usage_frame("null").is_none());
}

#[test]
fn a_frame_with_missing_fields_reports_no_reading_rather_than_zero() {
    // Given: a frame the events service sent with counters it could not read.
    let frame = parse_usage_frame("{}").expect("an empty object is still a frame");

    // Then: nothing is invented. Absent is absent.
    assert!(!frame.live_telemetry);
    assert_eq!(frame.active_requests, None);
    assert_eq!(frame.requests_today, None);
    assert_eq!(frame.tokens_today, None);
    assert_eq!(frame.estimated_cost, None);
    assert!(frame.recent_requests.is_empty());

    // And: the panel renders that as the no-reading marker, distinct from "0".
    assert_eq!(format_optional_count(frame.active_requests), NO_READING);
    assert_eq!(format_optional_count(Some(0)), "0");

    // A partial frame keeps what it has and marks only what is missing.
    let partial = parse_usage_frame(r#"{"liveTelemetry":true,"requestsToday":0}"#)
        .expect("a partial frame decodes");
    assert!(partial.live_telemetry);
    assert_eq!(partial.requests_today, Some(0));
    assert_eq!(partial.tokens_today, None);
    assert_eq!(format_optional_count(partial.requests_today), "0");
    assert_eq!(format_optional_count(partial.tokens_today), NO_READING);

    // A frame whose recentRequests entries omit names still identifies them.
    let unnamed = parse_usage_frame(r#"{"recentRequests":[{"timestamp":1}]}"#).expect("decodes");
    let entry = unnamed.recent_requests.first().expect("one entry");
    assert_eq!(entry.provider, "unknown");
    assert_eq!(entry.model, "unknown");
    // An unstated status is not reported as a failure.
    assert!(!entry.failed());
}

#[test]
fn the_period_selector_only_offers_values_the_api_accepts() {
    // Mirrors the guard in services/api-actix/src/usage.rs; anything else is a
    // 400, which would surface as a failed panel.
    let accepted = ["today", "24h", "7d", "30d", "60d", "all"];
    let offered: Vec<&str> = UsagePeriod::ALL
        .into_iter()
        .map(UsagePeriod::as_str)
        .collect();
    assert_eq!(offered, accepted);

    // Every option is labelled, described, and maps to its own request path.
    for period in UsagePeriod::ALL {
        assert!(!period.label().is_empty());
        assert!(!period.detail().is_empty());
        assert_eq!(
            period.stats_path(),
            format!("/api/usage/stats?period={}", period.as_str())
        );
    }

    // The default matches the endpoint's own default, so the first paint agrees
    // with what an omitted query parameter would have returned.
    assert_eq!(UsagePeriod::default().as_str(), "7d");
}

/// A breakdown row with the given request and token counts.
fn row(name: &str, requests: u64, total_tokens: u64) -> UsageBreakdownRow {
    UsageBreakdownRow {
        name: name.to_owned(),
        requests,
        total_tokens,
        ..UsageBreakdownRow::default()
    }
}
