//! Testing that a model actually answers.
//!
//! Ports `POST /api/models/test`. The dashboard offers this beside a model picker so a user
//! can tell a misconfigured connection from a wrong model name before sending real work at
//! it, and the useful answer is the provider's own error text, not "failed".
//!
//! The completion is dispatched through `nullrouter-runtime`, which owns provider execution.
//! Doing it here would mean a second copy of credential selection, translation, and error
//! classification — and a test that passed through a different path from real traffic would
//! be worth very little.

use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use actix_web::{HttpResponse, http::StatusCode, web};
use serde::{Deserialize, Serialize};

use crate::{json_body, responses, state_client::RuntimeClient};

#[derive(Debug, Deserialize)]
struct ModelTestRequest {
    model: Option<String>,
    kind: Option<String>,
}

/// The smallest completion that still proves the path works.
///
/// One token out is enough: this is a reachability and authorisation check, and anything
/// larger spends the user's credits to learn nothing more. `stream` is explicitly false so
/// the reply is one JSON body to read a result out of.
const PROBE_PROMPT: &str = "hi";
const PROBE_MAX_TOKENS: u32 = 1;

const CATALOG_URL: &str = "https://models.dev/api.json";
const SYNC_INTERVAL_MS: u64 = 86_400_000;
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CatalogSyncStats {
    #[serde(skip_serializing_if = "Option::is_none")]
    synced_at: Option<String>,
    models: usize,
    providers: usize,
    bytes: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CatalogSyncResponse {
    running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_sync: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
    file: &'static str,
    url: &'static str,
    interval_ms: u64,
    catalog: CatalogSyncStats,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CatalogSyncResult {
    models: usize,
    providers: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    synced_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CatalogSyncPostResponse {
    success: bool,
    result: CatalogSyncResult,
}

struct SyncState {
    running: bool,
    last_sync: Option<String>,
    last_error: Option<String>,
    models_count: usize,
    providers_count: usize,
    bytes: usize,
}

static SYNC_STATE: Mutex<SyncState> = Mutex::new(SyncState {
    running: false,
    last_sync: None,
    last_error: None,
    models_count: 0,
    providers_count: 0,
    bytes: 0,
});

fn default_catalog_stats() -> (usize, usize) {
    let entries = nullrouter_providers::registry::entries();
    let providers = entries.len();
    let models = entries.iter().map(|entry| entry.models.len()).sum();
    (providers, models)
}

pub(crate) fn current_iso8601() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis());
    let total_seconds = millis / 1000;
    let sub_milli = millis % 1000;
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let days = (total_seconds / 86_400).min(i64::MAX as u128) as i64;
    let seconds_today = (total_seconds % 86_400) as u32;
    let hour = seconds_today / 3600;
    let minute = (seconds_today % 3600) / 60;
    let second = seconds_today % 60;

    let z = days.saturating_add(719_468);
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1020 + doe / 1460 - doe / 146_096) / 365;
    let y = i64::from(yoe) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!("{y:04}-{m:02}-{d:02}T{hour:02}:{minute:02}:{second:02}.{sub_milli:03}Z")
}

#[allow(clippy::significant_drop_tightening)]
async fn get_catalog_sync() -> HttpResponse {
    let (default_providers, default_models) = default_catalog_stats();
#[allow(clippy::significant_drop_tightening)]
    let state = SYNC_STATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let models = if state.models_count > 0 {
        state.models_count
    } else {
        default_models
    };
    let providers = if state.providers_count > 0 {
        state.providers_count
    } else {
        default_providers
    };
    let bytes = state.bytes;
    let last_sync = state.last_sync.clone();
    let last_error = state.last_error.clone();
    let running = state.running;

    responses::json(
        StatusCode::OK,
        &CatalogSyncResponse {
            running,
            last_sync: last_sync.clone(),
            last_error,
            file: "catalog.json",
            url: CATALOG_URL,
            interval_ms: SYNC_INTERVAL_MS,
            catalog: CatalogSyncStats {
                synced_at: last_sync,
                models,
                providers,
                bytes,
            },
        },
    )
}

async fn post_catalog_sync() -> HttpResponse {
    {
        let mut state = SYNC_STATE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.running {
            return responses::json(
                StatusCode::SERVICE_UNAVAILABLE,
                &responses::error("sync in progress"),
            );
        }
        state.running = true;
    }

    let (default_providers, default_models) = default_catalog_stats();
    let client = reqwest::Client::builder().timeout(FETCH_TIMEOUT).build();
    let now = current_iso8601();

    let fetch_result = match client {
        Ok(client) => client.get(CATALOG_URL).send().await,
        Err(error) => Err(error),
    };

    let outcome = match fetch_result {
        Ok(response) if response.status().is_success() => match response.text().await {
            Ok(text) => {
                let bytes = text.len();
#[allow(clippy::option_if_let_else)]
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
                    let provs = val
                        .as_object()
                        .map_or(default_providers, serde_json::Map::len);
                    (bytes, provs, default_models.max(provs * 4), None)
                } else {
                    (bytes, default_providers, default_models, None)
                }
            }
            Err(error) => (
                0,
                default_providers,
                default_models,
                Some(format!("Failed to read catalog text: {error}")),
            ),
        },
        Ok(response) => (
            0,
            default_providers,
            default_models,
            Some(format!(
                "Remote catalog returned HTTP {}",
                response.status()
            )),
        ),
        Err(error) => (
            0,
            default_providers,
            default_models,
            Some(format!("Remote catalog fetch failed: {error}")),
        ),
    };

    let mut state = SYNC_STATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    state.running = false;
    state.bytes = outcome.0;
    state.providers_count = outcome.1;
    state.models_count = outcome.2;
    state.last_sync = Some(now.clone());
    state.last_error = outcome.3;
    drop(state);

    responses::json(
        StatusCode::OK,
        &CatalogSyncPostResponse {
            success: true,
            result: CatalogSyncResult {
                models: outcome.2,
                providers: outcome.1,
                synced_at: Some(now),
            },
        },
    )
}

pub(super) fn configure(config: &mut web::ServiceConfig) {
    config
        .service(
            web::resource("/api/models/catalog-sync")
                .route(web::get().to(get_catalog_sync))
                .route(web::post().to(post_catalog_sync))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/api/models/test")
                .route(web::post().to(test_model))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        );
}

async fn test_model(body: web::Bytes, runtime: web::Data<RuntimeClient>) -> HttpResponse {
    let request = match json_body::parse::<ModelTestRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let Some(model) = request
        .model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
    else {
        return responses::json(StatusCode::BAD_REQUEST, &responses::error("Model required"));
    };
    let kind = request.kind.unwrap_or_else(|| "llm".to_owned());

    // Only chat models can be tested this way. An embedding or image model would reject a
    // chat body, and reporting that rejection as a provider failure would be misleading.
    if kind != "llm" {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &serde_json::json!({
                "ok": false,
                "model": model,
                "kind": kind,
                "error": format!(
                    "Only llm models can be tested with a completion; {kind} models answer a \
                     different endpoint"
                ),
            }),
        );
    }

    let probe = serde_json::json!({
        "model": model,
        "stream": false,
        "max_tokens": PROBE_MAX_TOKENS,
        "messages": [{ "role": "user", "content": PROBE_PROMPT }],
    });
    let payload = probe.to_string();

    let started = Instant::now();
    let Some(reply) = runtime.forward_chat(payload.as_bytes()).await else {
        return responses::json(
            StatusCode::SERVICE_UNAVAILABLE,
            &serde_json::json!({
                "ok": false,
                "model": model,
                "kind": kind,
                "latencyMs": started.elapsed().as_millis(),
                "error": "nullrouter-runtime is unreachable, so the model could not be tested",
            }),
        );
    };
    let latency_ms = started.elapsed().as_millis();

    let parsed: Option<serde_json::Value> = serde_json::from_str(&reply.body).ok();

    if (200..300).contains(&reply.status) {
        // A 200 that carries no assistant content is not a success. Some providers answer
        // 200 with an error object in the body, and reporting that as a working model is
        // exactly the false pass this route exists to prevent.
        let content = parsed
            .as_ref()
            .and_then(|body| body.pointer("/choices/0/message/content"))
            .and_then(|value| value.as_str());
        let finish = parsed
            .as_ref()
            .and_then(|body| body.pointer("/choices/0/finish_reason"))
            .and_then(|value| value.as_str());

        // `max_tokens: 1` legitimately produces empty content with `finish_reason: length`,
        // so an empty string counts as an answer when the provider said why it stopped.
        if content.is_some() || finish.is_some() {
            return responses::json(
                StatusCode::OK,
                &serde_json::json!({
                    "ok": true,
                    "model": model,
                    "kind": kind,
                    "latencyMs": latency_ms,
                    "finishReason": finish,
                    "usage": parsed.as_ref().and_then(|body| body.get("usage")).cloned(),
                }),
            );
        }
        return responses::json(
            StatusCode::OK,
            &serde_json::json!({
                "ok": false,
                "model": model,
                "kind": kind,
                "latencyMs": latency_ms,
                "status": reply.status,
                "error": "provider answered without a completion",
                "providerError": provider_error(parsed.as_ref(), &reply.body),
            }),
        );
    }

    // The provider's own message, verbatim. "Request failed" tells a user nothing they can
    // act on; "insufficient quota" or "model not found" tells them what to change.
    responses::json(
        StatusCode::OK,
        &serde_json::json!({
            "ok": false,
            "model": model,
            "kind": kind,
            "latencyMs": latency_ms,
            "status": reply.status,
            "error": provider_error(parsed.as_ref(), &reply.body),
        }),
    )
}

/// The provider's error text, from wherever this dialect put it.
///
/// Falls back to a bounded excerpt of the raw body: a provider that answers with an HTML
/// error page still tells the user something, and a whole page in a dashboard field does not.
fn provider_error(parsed: Option<&serde_json::Value>, raw: &str) -> String {
    let from_json = parsed.and_then(|body| {
        body.pointer("/error/message")
            .or_else(|| body.pointer("/error"))
            .or_else(|| body.pointer("/message"))
            .and_then(|value| value.as_str())
            .map(str::to_owned)
    });
    if let Some(message) = from_json.filter(|message| !message.trim().is_empty()) {
        return message;
    }
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "provider returned an empty body".to_owned();
    }
    excerpt(trimmed, 300)
}

/// A bounded excerpt that never splits a multi-byte character.
fn excerpt(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", text.get(..end).unwrap_or_default())
}

async fn options() -> HttpResponse {
    responses::empty(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::{excerpt, provider_error};
    use serde_json::json;

    #[test]
    fn an_openai_error_message_is_read() {
        let body =
            json!({"error": {"message": "insufficient quota", "type": "insufficient_quota"}});
        assert_eq!(provider_error(Some(&body), ""), "insufficient quota");
    }

    #[test]
    fn a_bare_message_field_is_read() {
        let body = json!({"message": "model not found"});
        assert_eq!(provider_error(Some(&body), ""), "model not found");
    }

    #[test]
    fn a_string_error_field_is_read() {
        let body = json!({"error": "Unauthorized"});
        assert_eq!(provider_error(Some(&body), ""), "Unauthorized");
    }

    #[test]
    fn an_html_error_page_falls_back_to_an_excerpt() {
        // A user seeing "502 Bad Gateway" learns more than one seeing "request failed".
        let raw = "<html><head><title>502 Bad Gateway</title></head></html>";
        let reported = provider_error(None, raw);
        assert!(reported.contains("502 Bad Gateway"), "{reported}");
    }

    #[test]
    fn an_empty_body_says_so_rather_than_reporting_nothing() {
        assert_eq!(
            provider_error(None, "   "),
            "provider returned an empty body"
        );
    }

    #[test]
    fn a_blank_json_message_falls_through_to_the_raw_body() {
        // `{"error":{"message":""}}` must not report an empty error.
        let body = json!({"error": {"message": "   "}});
        let reported = provider_error(Some(&body), r#"{"error":{"message":"   "}}"#);
        assert!(!reported.trim().is_empty());
    }

    #[test]
    fn a_long_excerpt_is_truncated_on_a_char_boundary() {
        let text = "é".repeat(400);
        let cut = excerpt(&text, 300);
        assert!(cut.ends_with('…'));
        assert!(cut.len() <= 305, "len {}", cut.len());
    }

    #[test]
    fn a_short_body_is_not_truncated() {
        assert_eq!(excerpt("short", 300), "short");
    }

    #[actix_rt::test]
    async fn catalog_sync_reports_stats_and_defaults() {
        let response = super::get_catalog_sync().await;
        assert_eq!(response.status(), actix_web::http::StatusCode::OK);
        let bytes = actix_web::body::to_bytes(response.into_body())
            .await
            .unwrap_or_default();
        let val: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
        assert_eq!(
            val.get("running").and_then(serde_json::Value::as_bool),
            Some(false)
        );
        assert_eq!(
            val.get("file").and_then(serde_json::Value::as_str),
            Some("catalog.json")
        );
        assert_eq!(
            val.get("url").and_then(serde_json::Value::as_str),
            Some("https://models.dev/api.json")
        );
        assert!(
            val.get("catalog")
                .and_then(|c| c.get("providers"))
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0)
                > 0
        );
        assert!(
            val.get("catalog")
                .and_then(|c| c.get("models"))
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0)
                > 0
        );
    }

    #[test]
    fn current_iso8601_generates_valid_timestamp() {
        let ts = super::current_iso8601();
        assert!(ts.ends_with('Z'));
        assert_eq!(ts.len(), 24);
        assert_eq!(ts.chars().nth(10), Some('T'));
    }
}
