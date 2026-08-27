//! Runtime HTTP handlers.
//!
//! `future_not_send` is allowed module-wide: these handlers take `HttpRequest`
//! to record the real request path for format detection and usage attribution,
//! and `HttpRequest` is deliberately `!Send`. Actix drives handlers on a
//! per-worker single-threaded executor, so `Send` is not required.
#![allow(
    clippy::future_not_send,
    reason = "actix handlers run on a !Send per-worker executor; HttpRequest is !Send by design"
)]

use actix_web::{HttpRequest, HttpResponse, http::StatusCode, web};
use nullrouter_execute::build_error_body;
use nullrouter_providers::{Format, detect_format, detect_format_by_endpoint};
use serde_json::Value;

use crate::{
    AppConfig,
    errors::RuntimeError,
    models,
    pipeline::{ChatContext, Runtime},
    requests::{
        ChatPayload, CountTokensRequest, ModelInfoQuery, gemini_model_from_tail, parse_json,
    },
    responses,
};

/// Whether an endpoint streams by default when the body omits `stream`.
#[derive(Debug, Clone, Copy)]
enum StreamDefault {
    Disabled,
    Enabled,
}

impl StreamDefault {
    const fn enabled(self) -> bool {
        match self {
            Self::Disabled => false,
            Self::Enabled => true,
        }
    }
}

pub(crate) async fn health(config: web::Data<AppConfig>) -> HttpResponse {
    responses::json(StatusCode::OK, &models::health(config.service_name))
}

pub(crate) async fn no_content() -> HttpResponse {
    responses::empty(StatusCode::NO_CONTENT)
}

pub(crate) async fn openai_models(runtime: web::Data<Runtime>) -> HttpResponse {
    responses::json(StatusCode::OK, &runtime.models_list(&["llm"]).await)
}

pub(crate) async fn openai_models_by_kind(
    runtime: web::Data<Runtime>,
    kind: web::Path<String>,
) -> HttpResponse {
    let kind = kind.into_inner();
    // `image-to-text` is spelled `imageToText` in the registry's serviceKinds.
    let resolved = match kind.as_str() {
        "image-to-text" => "imageToText",
        other => other,
    };
    responses::json(StatusCode::OK, &runtime.models_list(&[resolved]).await)
}

pub(crate) async fn model_info(
    query: web::Query<ModelInfoQuery>,
) -> Result<HttpResponse, RuntimeError> {
    let id = query.required_id()?;
    let info =
        models::model_info(id, query.kind()).ok_or_else(|| RuntimeError::not_found_model(id))?;
    Ok(responses::json(StatusCode::OK, &info))
}

pub(crate) async fn gemini_models() -> HttpResponse {
    responses::json(StatusCode::OK, &models::gemini_models())
}

/// Native Gemini `/v1beta/models/{model}:generateContent`.
pub(crate) async fn gemini_generation(
    runtime: web::Data<Runtime>,
    request: HttpRequest,
    path: web::Path<String>,
    body: web::Bytes,
) -> Result<HttpResponse, RuntimeError> {
    if let Some(rejection) = runtime.enforce_api_key(extract_api_key(&request)).await {
        return Ok(rejection);
    }
    let payload: Value = parse_json(&body)?;
    let tail = path.into_inner();
    let stream = tail.contains(":streamGenerateContent");
    let model = gemini_model_from_tail(&tail);

    Ok(runtime
        .execute_chat(ChatContext {
            endpoint: request.path(),
            body: payload,
            stream,
            // The body is already Gemini-shaped on this endpoint.
            source_format: Format::Gemini,
            requested_model: model,
        })
        .await)
}

pub(crate) async fn chat_completions(
    runtime: web::Data<Runtime>,
    request: HttpRequest,
    body: web::Bytes,
) -> Result<HttpResponse, RuntimeError> {
    chat_entrypoint(&runtime, &request, &body, StreamDefault::Disabled).await
}

pub(crate) async fn responses_endpoint(
    runtime: web::Data<Runtime>,
    request: HttpRequest,
    body: web::Bytes,
) -> Result<HttpResponse, RuntimeError> {
    chat_entrypoint(&runtime, &request, &body, StreamDefault::Enabled).await
}

pub(crate) async fn messages(
    runtime: web::Data<Runtime>,
    request: HttpRequest,
    body: web::Bytes,
) -> Result<HttpResponse, RuntimeError> {
    chat_entrypoint(&runtime, &request, &body, StreamDefault::Enabled).await
}

pub(crate) async fn api_chat(
    runtime: web::Data<Runtime>,
    request: HttpRequest,
    body: web::Bytes,
) -> Result<HttpResponse, RuntimeError> {
    chat_entrypoint(&runtime, &request, &body, StreamDefault::Disabled).await
}

pub(crate) async fn embeddings(
    runtime: web::Data<Runtime>,
    request: HttpRequest,
    body: web::Bytes,
) -> Result<HttpResponse, RuntimeError> {
    passthrough_entrypoint(&runtime, &request, &body, &["model", "input"]).await
}

pub(crate) async fn image_generations(
    runtime: web::Data<Runtime>,
    request: HttpRequest,
    body: web::Bytes,
) -> Result<HttpResponse, RuntimeError> {
    passthrough_entrypoint(&runtime, &request, &body, &["model", "prompt"]).await
}

pub(crate) async fn audio_speech(
    runtime: web::Data<Runtime>,
    request: HttpRequest,
    body: web::Bytes,
) -> Result<HttpResponse, RuntimeError> {
    passthrough_entrypoint(&runtime, &request, &body, &["model", "input"]).await
}

pub(crate) async fn audio_transcriptions(
    runtime: web::Data<Runtime>,
    request: HttpRequest,
    body: web::Bytes,
) -> Result<HttpResponse, RuntimeError> {
    passthrough_entrypoint(&runtime, &request, &body, &["model", "file"]).await
}

pub(crate) async fn search(
    runtime: web::Data<Runtime>,
    request: HttpRequest,
    body: web::Bytes,
) -> Result<HttpResponse, RuntimeError> {
    passthrough_entrypoint(&runtime, &request, &body, &["query"]).await
}

pub(crate) async fn web_fetch(
    runtime: web::Data<Runtime>,
    request: HttpRequest,
    body: web::Bytes,
) -> Result<HttpResponse, RuntimeError> {
    passthrough_entrypoint(&runtime, &request, &body, &["url"]).await
}

pub(crate) async fn responses_compact(
    runtime: web::Data<Runtime>,
    request: HttpRequest,
    body: web::Bytes,
) -> Result<HttpResponse, RuntimeError> {
    chat_entrypoint(&runtime, &request, &body, StreamDefault::Disabled).await
}

pub(crate) async fn count_tokens(body: web::Bytes) -> Result<HttpResponse, RuntimeError> {
    let request: CountTokensRequest = parse_json(&body)?;
    Ok(responses::json(
        StatusCode::OK,
        &models::count_tokens(request.input_tokens()),
    ))
}

pub(crate) async fn audio_voices() -> HttpResponse {
    responses::json(StatusCode::OK, &models::voices())
}

pub(crate) async fn not_found() -> HttpResponse {
    responses::json(
        StatusCode::NOT_FOUND,
        &serde_json::json!({
            "error": {
                "message": "Runtime route not found",
                "type": "not_found",
            },
        }),
    )
}

/// Extract the client's API key (upstream `extractApiKey`): `Authorization:
/// Bearer <key>` first, then Anthropic's `x-api-key`.
fn extract_api_key(request: &HttpRequest) -> Option<&str> {
    if let Some(bearer) = request
        .headers()
        .get(actix_web::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    {
        return Some(bearer);
    }
    request
        .headers()
        .get("x-api-key")
        .and_then(|value| value.to_str().ok())
}

/// Shared entry point for the chat-shaped endpoints.
async fn chat_entrypoint(
    runtime: &Runtime,
    request: &HttpRequest,
    body: &[u8],
    stream_default: StreamDefault,
) -> Result<HttpResponse, RuntimeError> {
    if let Some(rejection) = runtime.enforce_api_key(extract_api_key(request)).await {
        return Ok(rejection);
    }
    let payload: Value = parse_json(body)?;
    let typed: ChatPayload = parse_json(body)?;
    let model = typed.required_model()?.to_owned();
    let endpoint = request.path();

    // Endpoint wins over body shape, matching upstream precedence.
    let source_format =
        detect_format_by_endpoint(endpoint, &payload).unwrap_or_else(|| detect_format(&payload));

    let stream = typed.stream(stream_default.enabled() || source_format.always_streams());

    Ok(runtime
        .execute_chat(ChatContext {
            endpoint,
            body: payload,
            stream,
            source_format,
            requested_model: model,
        })
        .await)
}

/// Entry point for the non-chat provider endpoints (embeddings, images, audio,
/// search, fetch).
///
/// These have no translation matrix: the body is forwarded to the provider in
/// the shape the client sent it.
async fn passthrough_entrypoint(
    runtime: &Runtime,
    request: &HttpRequest,
    body: &[u8],
    required: &[&str],
) -> Result<HttpResponse, RuntimeError> {
    if let Some(rejection) = runtime.enforce_api_key(extract_api_key(request)).await {
        return Ok(rejection);
    }
    let payload: Value = parse_json(body)?;

    // Upstream validates the routing target before the endpoint-specific
    // fields (`required_model_and_input`, `required_provider_and_query`, ...),
    // so that order is preserved here. `provider` is accepted in place of
    // `model` on the search/fetch endpoints.
    let has_model = payload.get("model").is_some();
    let model = payload
        .get("model")
        .and_then(Value::as_str)
        .or_else(|| payload.get("provider").and_then(Value::as_str))
        .map(str::trim)
        .filter(|model| !model.is_empty());
    let Some(model) = model else {
        // The message names whichever field this endpoint documents.
        let message = if has_model || required.contains(&"model") {
            "Missing required field: model"
        } else {
            "Missing required field: provider (or model)"
        };
        return Ok(responses::json(
            StatusCode::BAD_REQUEST,
            &build_error_body(400, message),
        ));
    };

    for field in required {
        if *field == "model" {
            continue;
        }
        let present = payload.get(*field).is_some_and(|value| !value.is_null());
        if !present {
            return Ok(responses::json(
                StatusCode::BAD_REQUEST,
                &build_error_body(400, &format!("Missing required field: {field}")),
            ));
        }
    }

    Ok(runtime
        .execute_passthrough(ChatContext {
            endpoint: request.path(),
            body: payload.clone(),
            stream: false,
            // No translation: the provider receives the client's shape.
            source_format: Format::OpenAi,
            requested_model: model.to_owned(),
        })
        .await)
}
