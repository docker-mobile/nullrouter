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

use std::time::Instant;

use actix_web::{HttpResponse, http::StatusCode, web};
use serde::Deserialize;

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

pub(super) fn configure(config: &mut web::ServiceConfig) {
    config.service(
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
}
