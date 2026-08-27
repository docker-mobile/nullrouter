//! The `/api/usage/*` dashboard surface.
//!
//! Every number here comes from `nullrouter-state`: this service owns no usage
//! storage, so each handler is a projection of what state recorded, reshaped
//! into the JSON upstream's dashboard consumes. Where the port has no data
//! source at all — provider-side quota, in-flight request counts — the response
//! says so explicitly rather than returning a plausible-looking value.

use std::collections::BTreeMap;

use actix_web::{HttpResponse, http::StatusCode, web};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    responses,
    state_client::{StateClient, urlencode},
};

#[derive(Debug, Deserialize)]
struct PeriodQuery {
    period: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RequestDetailsQuery {
    page: Option<u32>,
    #[serde(rename = "pageSize")]
    page_size: Option<u32>,
    provider: Option<String>,
    model: Option<String>,
    #[serde(rename = "connectionId")]
    connection_id: Option<String>,
    status: Option<String>,
    #[serde(rename = "startDate")]
    start_date: Option<String>,
    #[serde(rename = "endDate")]
    end_date: Option<String>,
}

#[derive(Debug, Serialize)]
struct PendingRequests {
    #[serde(rename = "byModel")]
    by_model: BTreeMap<String, serde_json::Value>,
    #[serde(rename = "byAccount")]
    by_account: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct UsageStatsResponse {
    #[serde(rename = "totalRequests")]
    total_requests: u64,
    #[serde(rename = "totalPromptTokens")]
    total_prompt_tokens: u64,
    #[serde(rename = "totalCompletionTokens")]
    total_completion_tokens: u64,
    #[serde(rename = "totalCachedTokens")]
    total_cached_tokens: u64,
    #[serde(rename = "totalCost")]
    total_cost: u64,
    #[serde(rename = "byProvider")]
    by_provider: BTreeMap<String, serde_json::Value>,
    #[serde(rename = "byModel")]
    by_model: BTreeMap<String, serde_json::Value>,
    #[serde(rename = "byAccount")]
    by_account: BTreeMap<String, serde_json::Value>,
    #[serde(rename = "byApiKey")]
    by_api_key: BTreeMap<String, serde_json::Value>,
    #[serde(rename = "byEndpoint")]
    by_endpoint: BTreeMap<String, serde_json::Value>,
    #[serde(rename = "last10Minutes")]
    last_10_minutes: Vec<serde_json::Value>,
    pending: PendingRequests,
    #[serde(rename = "activeRequests")]
    active_requests: Vec<serde_json::Value>,
    #[serde(rename = "recentRequests")]
    recent_requests: Vec<serde_json::Value>,
    #[serde(rename = "errorProvider")]
    error_provider: &'static str,
}

#[derive(Debug, Serialize)]
struct UsageProvidersResponse {
    providers: Vec<serde_json::Value>,
}

pub(super) fn configure(config: &mut web::ServiceConfig) {
    config
        .service(web::resource("/api/usage/stats").route(web::get().to(stats)))
        .service(web::resource("/api/usage/history").route(web::get().to(history)))
        .service(web::resource("/api/usage/chart").route(web::get().to(chart)))
        .service(web::resource("/api/usage/logs").route(web::get().to(logs)))
        .service(web::resource("/api/usage/request-logs").route(web::get().to(logs)))
        .service(web::resource("/api/usage/providers").route(web::get().to(providers)))
        .service(web::resource("/api/usage/request-details").route(web::get().to(request_details)))
        .service(web::resource("/api/usage/{connection_id}").route(web::get().to(connection_usage)))
        .service(
            web::resource("/api/usage/{connection_id}/codex-reset-credits")
                .route(web::get().to(codex_reset_credits))
                .route(web::post().to(codex_reset_credits_post))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        );
}

/// Window length in milliseconds for a dashboard period selector.
fn period_window_ms(period: &str) -> u64 {
    const DAY_MS: u64 = 24 * 60 * 60 * 1000;
    match period {
        "today" | "24h" => DAY_MS,
        "30d" => 30 * DAY_MS,
        "60d" => 60 * DAY_MS,
        "all" => u64::MAX,
        // "7d" is the upstream default.
        _ => 7 * DAY_MS,
    }
}

/// Epoch millis at the start of a period window.
fn window_start_ms(period: &str) -> u64 {
    let window = period_window_ms(period);
    if window == u64::MAX {
        return 0;
    }
    now_millis().saturating_sub(window)
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

async fn stats(state: web::Data<StateClient>, query: web::Query<PeriodQuery>) -> HttpResponse {
    let period = query.period.as_deref().unwrap_or("7d");
    if !matches!(period, "today" | "24h" | "7d" | "30d" | "60d" | "all") {
        return responses::json(StatusCode::BAD_REQUEST, &responses::error("Invalid period"));
    }
    period_stats(&state, period).await
}

async fn history(state: web::Data<StateClient>) -> HttpResponse {
    // Upstream's `/history` calls `getUsageStats()` with no period, which
    // defaults to everything.
    period_stats(&state, "all").await
}

/// Aggregate stats for one period window.
///
/// `all` reads the lifetime counters, which survive record trimming; every other
/// period sums the records inside its window, so the selector actually narrows
/// the numbers. Falls back to the zeroed shape when state is unreachable, so a
/// state outage leaves the dashboard rendering rather than erroring.
async fn period_stats(state: &StateClient, period: &str) -> HttpResponse {
    let aggregates = if period == "all" {
        state.usage_stats().await
    } else {
        state.usage_aggregate(window_start_ms(period)).await
    };
    let Some(mut aggregates) = aggregates else {
        return responses::json(StatusCode::OK, &empty_stats());
    };

    // Lifetime counters carry no live telemetry, so merge it in. The window
    // aggregate already includes it; merging is idempotent either way.
    if let (Some(target), Some(live)) = (
        aggregates.as_object_mut(),
        state
            .usage_live()
            .await
            .and_then(|live| live.as_object().cloned()),
    ) {
        for (key, value) in live {
            target.insert(key, value);
        }
    }
    // Any key state did not report is filled from the zeroed shape, so the
    // response always carries the full contract.
    fill_missing_stats_keys(&mut aggregates);
    responses::json(StatusCode::OK, &aggregates)
}

/// Add any stats key state omitted, using the zeroed default for it.
///
/// Never overwrites a value state reported.
fn fill_missing_stats_keys(aggregates: &mut Value) {
    let Some(target) = aggregates.as_object_mut() else {
        return;
    };
    let Ok(Value::Object(defaults)) = serde_json::to_value(empty_stats()) else {
        return;
    };
    for (key, value) in defaults {
        target.entry(key).or_insert(value);
    }
}

async fn chart(state: web::Data<StateClient>, query: web::Query<PeriodQuery>) -> HttpResponse {
    let period = query.period.as_deref().unwrap_or("7d");
    if !matches!(period, "today" | "24h" | "7d" | "30d" | "60d") {
        return responses::json(StatusCode::BAD_REQUEST, &responses::error("Invalid period"));
    }

    // One bucket per record, ordered oldest-first for charting.
    let mut records = state.usage_records(window_start_ms(period), 1000).await;
    records.reverse();
    let series: Vec<serde_json::Value> = records
        .iter()
        .map(|record| {
            serde_json::json!({
                "timestamp": record.timestamp,
                "provider": record.provider,
                "model": record.model,
                "promptTokens": record.prompt_tokens,
                "completionTokens": record.completion_tokens,
                "totalTokens": record.total_tokens,
                "requests": 1,
            })
        })
        .collect();
    responses::json(StatusCode::OK, &series)
}

async fn logs(state: web::Data<StateClient>) -> HttpResponse {
    let records = state.usage_records(0, 200).await;
    let logs: Vec<serde_json::Value> = records
        .iter()
        .map(|record| {
            serde_json::json!({
                "id": record.id,
                "timestamp": record.timestamp,
                "provider": record.provider,
                "model": record.model,
                "connectionId": record.connection_id,
                "endpoint": record.endpoint,
                "status": record.status,
                "statusCode": record.status_code,
                "promptTokens": record.prompt_tokens,
                "completionTokens": record.completion_tokens,
                "totalTokens": record.total_tokens,
                "latencyMs": record.latency_ms,
                "error": record.error,
            })
        })
        .collect();
    responses::json(StatusCode::OK, &logs)
}

/// Providers seen in recorded usage, for the request-details filter.
///
/// Upstream reads `DISTINCT provider` from stored request details and resolves
/// each id through the configured provider nodes; state does both, since it owns
/// the records and the nodes.
async fn providers(state: web::Data<StateClient>) -> HttpResponse {
    let listed = state
        .usage_providers()
        .await
        .and_then(|body| body.get("providers").and_then(Value::as_array).cloned())
        .unwrap_or_default();

    responses::json(
        StatusCode::OK,
        &UsageProvidersResponse { providers: listed },
    )
}

async fn request_details(
    state: web::Data<StateClient>,
    query: web::Query<RequestDetailsQuery>,
) -> HttpResponse {
    let page = query.page.unwrap_or(1);
    if page < 1 {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Page must be >= 1"),
        );
    }
    let page_size = query.page_size.unwrap_or(20);
    if !(1..=100).contains(&page_size) {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("PageSize must be between 1 and 100"),
        );
    }

    // Every declared filter is forwarded, so a filter the caller sets narrows
    // the result set rather than being silently dropped.
    let mut params = vec![format!("page={page}"), format!("pageSize={page_size}")];
    for (name, value) in [
        ("provider", query.provider.as_deref()),
        ("model", query.model.as_deref()),
        ("connectionId", query.connection_id.as_deref()),
        ("status", query.status.as_deref()),
        ("startDate", query.start_date.as_deref()),
        ("endDate", query.end_date.as_deref()),
    ] {
        if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
            params.push(format!("{name}={}", urlencode(value)));
        }
    }

    let filter = serde_json::json!({
        "provider": query.provider,
        "model": query.model,
        "connectionId": query.connection_id,
        "status": query.status,
        "startDate": query.start_date,
        "endDate": query.end_date,
    });

    match state.usage_details(&params.join("&")).await {
        // A rejected filter (an unparseable date) is a client error, not an
        // empty page: answering 200 with everything would over-report.
        Some((status, _)) if status == StatusCode::BAD_REQUEST.as_u16() => responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Invalid startDate or endDate"),
        ),
        Some((status, body)) if status == StatusCode::OK.as_u16() => responses::json(
            StatusCode::OK,
            &details_page(&body, page, page_size, &filter),
        ),
        // Unreachable or erroring state degrades to an explicit empty page.
        _ => responses::json(
            StatusCode::OK,
            &details_page(&Value::Null, page, page_size, &filter),
        ),
    }
}

/// Reshape a loopback record page into the dashboard's request-details envelope.
///
/// `details` and `pagination` are what upstream returns and what the dashboard
/// reads; `requests`, `total`, `page`, `pageSize`, and `totalPages` are kept
/// alongside them for callers of this port's earlier shape.
fn details_page(body: &Value, page: u32, page_size: u32, filter: &Value) -> Value {
    let records = body
        .get("records")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let details: Vec<Value> = records.iter().map(detail_row).collect();
    let total_items = body
        .get("totalItems")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let total_pages = body
        .get("totalPages")
        .and_then(Value::as_u64)
        .unwrap_or_default();

    serde_json::json!({
        "details": details,
        "pagination": {
            "page": page,
            "pageSize": page_size,
            "totalItems": total_items,
            "totalPages": total_pages,
            "hasNext": u64::from(page) < total_pages,
            "hasPrev": page > 1,
        },
        "requests": details,
        "total": total_items,
        "page": page,
        "pageSize": page_size,
        "totalPages": total_pages,
        "filter": filter,
    })
}

/// One record in the dashboard's request-detail shape.
///
/// Conversation payloads are reported as redacted, matching upstream: this port
/// never stores request or response bodies, so there is nothing to return.
fn detail_row(record: &Value) -> Value {
    let number = |name: &str| record.get(name).and_then(Value::as_u64).unwrap_or_default();
    serde_json::json!({
        "id": record.get("id").cloned().unwrap_or(Value::Null),
        "timestamp": record.get("timestamp").cloned().unwrap_or(Value::Null),
        "provider": record.get("provider").cloned().unwrap_or(Value::Null),
        "model": record.get("model").cloned().unwrap_or(Value::Null),
        "connectionId": record.get("connectionId").cloned().unwrap_or(Value::Null),
        "endpoint": record.get("endpoint").cloned().unwrap_or(Value::Null),
        "status": record.get("status").cloned().unwrap_or(Value::Null),
        "statusCode": record.get("statusCode").cloned().unwrap_or(Value::Null),
        "error": record.get("error").cloned().unwrap_or(Value::Null),
        "tokens": {
            "prompt_tokens": number("promptTokens"),
            "completion_tokens": number("completionTokens"),
            "cached_tokens": number("cachedTokens"),
            "total_tokens": number("totalTokens"),
        },
        "latency": {
            // Only total wall-clock time is recorded; time-to-first-token is not
            // measured, so it is reported as absent rather than as zero.
            "ttft": Value::Null,
            "total": number("latencyMs"),
        },
        "request": { "redacted": true },
        "providerRequest": { "redacted": true },
        "providerResponse": { "redacted": true },
        "response": { "redacted": true },
    })
}

/// Provider-side quota for one connection.
///
/// The connection lookup is real: an unknown id is a 404. The quota itself is
/// not — none of upstream's per-provider usage APIs are ported, so this returns
/// upstream's own "not implemented" envelope for the provider rather than
/// inventing limits. The recorded local usage for the connection travels
/// alongside it, since that part is real.
async fn connection_usage(state: web::Data<StateClient>, path: web::Path<String>) -> HttpResponse {
    let connection_id = path.into_inner();
    let Some((status, body)) = state.usage_connection(&connection_id).await else {
        // State unreachable: the connection cannot be confirmed to exist.
        return connection_not_found();
    };
    if status != StatusCode::OK.as_u16() {
        return connection_not_found();
    }

    let provider = body
        .get("provider")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    responses::json(
        StatusCode::OK,
        &serde_json::json!({
            // Upstream's shape for a provider with no usage handler.
            "message": format!("Usage API not implemented for {provider}"),
            "provider": provider,
            "connectionId": connection_id,
            "quotas": [],
            // Real, locally recorded usage for this connection.
            "recorded": body.get("usage").cloned().unwrap_or(Value::Null),
        }),
    )
}

async fn codex_reset_credits(
    state: web::Data<StateClient>,
    path: web::Path<String>,
) -> HttpResponse {
    codex_reset(&state, &path.into_inner()).await
}

async fn codex_reset_credits_post(
    state: web::Data<StateClient>,
    path: web::Path<String>,
    body: web::Bytes,
) -> HttpResponse {
    // The parse result is dropped before the await, so the future stays `Send`.
    if let Err(response) = crate::json_body::parse_optional::<serde_json::Value>(&body) {
        return response;
    }
    codex_reset(&state, &path.into_inner()).await
}

/// Codex reset credits, as far as this port can answer.
///
/// The connection checks are real and run in upstream's order: unknown id →
/// 404, non-Codex provider → 400, wrong auth type → 400. Beyond that the call
/// needs OpenAI's rate-limit credit API, which is not ported, so an eligible
/// connection gets an explicit 501 rather than a fabricated credit balance.
async fn codex_reset(state: &StateClient, connection_id: &str) -> HttpResponse {
    let Some((status, body)) = state.usage_connection(connection_id).await else {
        return connection_not_found();
    };
    if status != StatusCode::OK.as_u16() {
        return connection_not_found();
    }

    let field = |name: &str| body.get(name).and_then(Value::as_str).unwrap_or_default();
    if field("provider") != "codex" {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Codex reset credits are only available for Codex connections."),
        );
    }
    if !matches!(field("authType"), "oauth" | "access_token") {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Codex reset credits require an OAuth or access-token connection."),
        );
    }

    responses::json(
        StatusCode::NOT_IMPLEMENTED,
        &serde_json::json!({
            "code": "not_implemented",
            "reset": false,
            "windows_reset": Value::Null,
            "error": "Codex reset credits are not supported by nullrouter-api",
            "message": "The Codex rate-limit credit API is not ported; no credit was read or consumed.",
        }),
    )
}

fn connection_not_found() -> HttpResponse {
    responses::json(
        StatusCode::NOT_FOUND,
        &responses::error("Connection not found"),
    )
}

async fn options() -> HttpResponse {
    responses::empty(StatusCode::NO_CONTENT)
}

const fn empty_stats() -> UsageStatsResponse {
    UsageStatsResponse {
        total_requests: 0,
        total_prompt_tokens: 0,
        total_completion_tokens: 0,
        total_cached_tokens: 0,
        total_cost: 0,
        by_provider: BTreeMap::new(),
        by_model: BTreeMap::new(),
        by_account: BTreeMap::new(),
        by_api_key: BTreeMap::new(),
        by_endpoint: BTreeMap::new(),
        last_10_minutes: Vec::new(),
        pending: PendingRequests {
            by_model: BTreeMap::new(),
            by_account: BTreeMap::new(),
        },
        active_requests: Vec::new(),
        recent_requests: Vec::new(),
        error_provider: "",
    }
}
