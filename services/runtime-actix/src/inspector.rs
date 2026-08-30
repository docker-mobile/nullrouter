//! The translator inspector's steps, run against the real engine.
//!
//! Ports `POST /api/translator/translate`. The dashboard shows the request as it passes
//! through each stage — client body, source format, OpenAI intermediate, provider body — so a
//! user can see exactly what a provider is being sent and why a translation went wrong.
//!
//! **Here rather than in `nullrouter-api`, because every step needs something this service
//! owns.** Step 1 resolves the model, which means the user-defined node prefixes that live in
//! the connection store. Step 3 builds the outbound URL and headers, which means credentials
//! and auth descriptors. Implementing it in the API service would have meant a second copy of
//! both, and a second copy is a second thing to drift — an inspector that showed a different
//! translation from the one the live path performs would be worse than no inspector.
//!
//! `nullrouter-api` proxies to these routes, the way it already does for pxpipe.
//!
//! One addition beyond upstream: a response step. Upstream's inspector has action buttons for
//! steps 1, 3 and Send only; its "OpenAI Response" and "Client Response" panes are
//! display-only and never populated, so a user inspecting a *response* translation has to
//! paste chunks in by hand. The engine that does it is right here, so step 5 runs it.

use actix_web::{HttpResponse, http::StatusCode, web};
use nullrouter_providers::{Format, detect_format, target_format};
use nullrouter_translate::{RequestRoute, state::StreamState};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{Runtime, responses};

/// One inspector step request.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StepRequest {
    step: Option<u8>,
    body: Option<Value>,
    provider: Option<String>,
    model: Option<String>,
    /// Step 5 only: the provider's raw response chunks to translate back.
    #[serde(default)]
    chunks: Option<Vec<Value>>,
}

pub(crate) fn configure(config: &mut web::ServiceConfig) {
    config.service(
        web::resource("/internal/translator/step")
            .route(web::post().to(step))
            .route(web::method(actix_web::http::Method::OPTIONS).to(no_content)),
    );
}

async fn no_content() -> HttpResponse {
    responses::empty(StatusCode::NO_CONTENT)
}

fn bad_request(message: &str) -> HttpResponse {
    responses::json(
        StatusCode::BAD_REQUEST,
        &json!({ "success": false, "error": message }),
    )
}

/// The dashboard nests the payload under `body` at some steps and sends it bare at others,
/// matching upstream's `body.body || body`.
fn payload(body: &Value) -> &Value {
    body.get("body")
        .filter(|inner| !inner.is_null())
        .unwrap_or(body)
}

/// `stream` defaults to true when absent, as the live path does.
fn stream_flag(body: &Value) -> bool {
    body.get("stream").and_then(Value::as_bool).unwrap_or(true)
}

async fn step(runtime: web::Data<Runtime>, body: web::Bytes) -> HttpResponse {
    let Ok(request) = serde_json::from_slice::<StepRequest>(&body) else {
        return bad_request("Invalid JSON body");
    };
    let Some(step) = request.step else {
        return bad_request("Step and body required");
    };

    match step {
        1 => step_one(&runtime, request).await,
        2 => step_two(&runtime, request).await,
        3 => step_three(&runtime, request).await,
        5 => step_five(request),
        _ => bad_request("Invalid step (1-3, or 5 for a response)"),
    }
}

/// Step 1: what is this request, and where is it going?
///
/// Resolves the model the way the live path does — including a user-defined node prefix, so a
/// compatible connection reports its real provider id rather than the prefix.
async fn step_one(runtime: &Runtime, request: StepRequest) -> HttpResponse {
    let Some(body) = request.body else {
        return bad_request("Step and body required");
    };
    let client_body = payload(&body);
    let Some(requested) = client_body.get("model").and_then(Value::as_str) else {
        return bad_request("body.model is required to resolve a provider");
    };

    let Some(target) = runtime.inspector_target(requested).await else {
        return bad_request("model did not resolve to a provider");
    };
    let source = detect_format(client_body);
    let dispatch = target_format(&target.provider);

    responses::json(
        StatusCode::OK,
        &json!({
            "success": true,
            "result": {
                "provider": target.provider,
                "model": target.model,
                "sourceFormat": source.as_str(),
                "targetFormat": dispatch.as_str(),
            },
        }),
    )
}

/// Step 2: the client's body as the OpenAI intermediate.
///
/// The first half of the pipeline. Everything pivots through OpenAI, so this is the shape a
/// translation bug shows up in first.
async fn step_two(runtime: &Runtime, request: StepRequest) -> HttpResponse {
    let Some(body) = request.body else {
        return bad_request("Step and body required");
    };
    let client_body = payload(&body);
    let Some(requested) = client_body.get("model").and_then(Value::as_str) else {
        return bad_request("body.model is required to resolve a provider");
    };
    let Some(target) = runtime.inspector_target(requested).await else {
        return bad_request("model did not resolve to a provider");
    };

    let source = detect_format(client_body);
    let translated = nullrouter_translate::translate_request(
        RequestRoute {
            source,
            target: Format::OpenAi,
            provider: &target.provider,
            model: &target.model,
        },
        client_body,
        stream_flag(client_body),
        0,
    );

    responses::json(
        StatusCode::OK,
        &json!({
            "success": true,
            "result": { "body": translated.body },
        }),
    )
}

/// Step 3: the OpenAI intermediate as the provider's own body, plus the URL and headers.
///
/// The URL and headers come from the connection, so this step is the one that shows a user
/// their request is going to the wrong host or carrying the wrong auth scheme.
async fn step_three(runtime: &Runtime, request: StepRequest) -> HttpResponse {
    let Some(body) = request.body else {
        return bad_request("Step and body required");
    };
    let openai_body = payload(&body);
    let Some(provider) = request
        .provider
        .as_deref()
        .filter(|value| !value.is_empty())
    else {
        return bad_request("provider and model required");
    };
    let Some(model) = request.model.as_deref().filter(|value| !value.is_empty()) else {
        return bad_request("provider and model required");
    };

    let dispatch = target_format(provider);
    let translated = nullrouter_translate::translate_request(
        RequestRoute {
            source: Format::OpenAi,
            target: dispatch,
            provider,
            model,
        },
        openai_body,
        stream_flag(openai_body),
        0,
    );

    // Redacted deliberately. The inspector's job is to show that auth is *present and in the
    // right header*, which a placeholder does; showing the key itself would put a live
    // credential in a dashboard pane, a screenshot, and a bug report.
    let wire = runtime.inspector_wire(provider, model, dispatch).await;

    responses::json(
        StatusCode::OK,
        &json!({
            "success": true,
            "result": {
                "body": translated.body,
                "url": wire.as_ref().map(|wire| wire.url.clone()),
                "headers": wire.as_ref().map(|wire| wire.headers.clone()),
                "toolNameMap": translated.tool_name_map,
                "connectionError": if wire.is_none() {
                    Some(format!("No active connection for provider: {provider}"))
                } else {
                    None
                },
            },
        }),
    )
}

/// Step 5: a provider's response chunks, back in the client's format.
///
/// Not in upstream, which leaves its response panes for hand-pasting. Runs the same
/// incremental translator the live stream uses, threading one `StreamState` through every
/// chunk — the state is what carries tool-call assembly and index mapping across chunks, so
/// translating them independently would produce something the live path never emits.
fn step_five(request: StepRequest) -> HttpResponse {
    let Some(chunks) = request.chunks else {
        return bad_request("chunks are required for a response step");
    };
    let Some(provider) = request
        .provider
        .as_deref()
        .filter(|value| !value.is_empty())
    else {
        return bad_request("provider is required to know the response format");
    };
    let source = request
        .body
        .as_ref()
        .and_then(|body| body.get("sourceFormat"))
        .and_then(Value::as_str)
        .and_then(Format::parse)
        .unwrap_or(Format::OpenAi);

    let dispatch = target_format(provider);
    // `Clock::System`, matching the live path: the inspector should show the framing a client
    // would actually receive, timestamps included.
    let mut state = StreamState::new(nullrouter_translate::state::Clock::System);
    let mut openai = Vec::new();
    let mut client = Vec::new();

    for chunk in &chunks {
        // Shown separately because the intermediate is where a bug is usually visible: a
        // pane holding the OpenAI form is upstream's step 6.
        //
        // Its own state, because these are two independent translations of the same chunks.
        // Sharing one would have the target->OpenAI pass consume tool-call assembly that the
        // target->client pass then needs.
        let mut intermediate_state = StreamState::new(nullrouter_translate::state::Clock::System);
        openai.extend(nullrouter_translate::translate_response(
            dispatch,
            Format::OpenAi,
            chunk,
            &mut intermediate_state,
        ));
        client.extend(nullrouter_translate::translate_response(
            dispatch, source, chunk, &mut state,
        ));
    }

    responses::json(
        StatusCode::OK,
        &json!({
            "success": true,
            "result": {
                "targetFormat": dispatch.as_str(),
                "sourceFormat": source.as_str(),
                "openai": openai,
                "client": client,
            },
        }),
    )
}

/// A resolved outbound URL and its headers, with secrets replaced.
pub(crate) struct InspectorWire {
    pub url: String,
    pub headers: std::collections::BTreeMap<String, String>,
}

/// Header names whose values are credentials.
///
/// Matched case-insensitively, and by substring for the `-key`/`-token` families, so a
/// provider-specific spelling this port has not seen is redacted by default rather than
/// printed. Erring the other way would leak a key the first time a provider invented a new
/// header name.
pub(crate) fn is_secret_header(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower == "authorization"
        || lower == "cookie"
        || lower == "proxy-authorization"
        || lower.contains("api-key")
        || lower.contains("apikey")
        || lower.contains("-token")
        || lower.contains("token-")
        || lower.contains("secret")
        || lower.contains("password")
}

/// Replace a credential with a shape-preserving placeholder.
///
/// Keeps the scheme prefix (`Bearer`, `Basic`) because *which* scheme is being used is
/// exactly what a user inspecting an auth problem needs to see, and the scheme is not secret.
pub(crate) fn redact(value: &str) -> String {
    for scheme in ["Bearer ", "Basic ", "Token "] {
        if let Some(rest) = value.strip_prefix(scheme) {
            return format!("{scheme}<redacted:{} chars>", rest.trim().len());
        }
    }
    format!("<redacted:{} chars>", value.trim().len())
}

#[cfg(test)]
mod tests {
    use super::{is_secret_header, payload, redact, stream_flag};
    use serde_json::json;

    #[test]
    fn a_nested_body_is_unwrapped_and_a_bare_one_is_not() {
        let nested = json!({"body": {"model": "m"}});
        assert_eq!(payload(&nested), &json!({"model": "m"}));
        let bare = json!({"model": "m"});
        assert_eq!(payload(&bare), &json!({"model": "m"}));
    }

    #[test]
    fn a_null_body_field_does_not_unwrap_to_null() {
        // `{"body": null, "model": "m"}` must read as the outer object, not as null.
        let awkward = json!({"body": null, "model": "m"});
        assert_eq!(payload(&awkward), &json!({"body": null, "model": "m"}));
    }

    #[test]
    fn stream_defaults_to_true_when_absent() {
        // Matches the live path: an absent `stream` means stream.
        assert!(stream_flag(&json!({})));
        assert!(stream_flag(&json!({"stream": true})));
        assert!(!stream_flag(&json!({"stream": false})));
    }

    #[test]
    fn every_credential_header_family_is_recognised() {
        for name in [
            "Authorization",
            "authorization",
            "x-api-key",
            "X-API-Key",
            "api-key",
            "x-goog-api-key",
            "anthropic-api-key",
            "x-session-token",
            "token-value",
            "Cookie",
            "proxy-authorization",
            "x-client-secret",
            "x-password",
        ] {
            assert!(is_secret_header(name), "{name} should be redacted");
        }
    }

    #[test]
    fn ordinary_headers_are_not_redacted() {
        for name in [
            "content-type",
            "accept",
            "anthropic-version",
            "user-agent",
            "x-request-id",
        ] {
            assert!(!is_secret_header(name), "{name} should not be redacted");
        }
    }

    #[test]
    fn redaction_keeps_the_scheme_and_the_length() {
        // The scheme is what a user debugging auth needs; the key is not.
        assert_eq!(redact("Bearer sk-abcdef"), "Bearer <redacted:9 chars>");
        assert_eq!(redact("Basic dXNlcg=="), "Basic <redacted:8 chars>");
        assert_eq!(redact("sk-abcdef"), "<redacted:9 chars>");
    }

    #[test]
    fn redaction_never_returns_the_original() {
        for secret in ["sk-live-1234567890", "Bearer sk-live-1234567890", ""] {
            let redacted = redact(secret);
            assert!(!redacted.contains("1234567890"), "leaked: {redacted}");
        }
    }
}
