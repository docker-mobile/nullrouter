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
    responses, video,
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

/// Whether this request is another router's model probe.
///
/// A compatible provider's base URL is typed by its owner and can point at another
/// router, or at this one. If both probe on `/v1/models`, they probe each other on every
/// call, forever. Answering a marked request from configuration alone terminates the
/// chain at one hop. Upstream sets and honours the same header, so the guard holds in a
/// mixed deployment too — which is why the name keeps its `9r` spelling.
fn is_internal_probe(request: &actix_web::HttpRequest) -> bool {
    request
        .headers()
        .get(nullrouter_execute::probe::INTERNAL_PROBE_HEADER)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.trim() == "1")
}

pub(crate) async fn openai_models(
    runtime: web::Data<Runtime>,
    request: actix_web::HttpRequest,
) -> HttpResponse {
    responses::json(
        StatusCode::OK,
        &runtime
            .models_list_with(&["llm"], !is_internal_probe(&request))
            .await,
    )
}

pub(crate) async fn openai_models_by_kind(
    runtime: web::Data<Runtime>,
    kind: web::Path<String>,
    request: actix_web::HttpRequest,
) -> HttpResponse {
    let kind = kind.into_inner();
    // `image-to-text` is spelled `imageToText` in the registry's serviceKinds.
    let resolved = match kind.as_str() {
        "image-to-text" => "imageToText",
        other => other,
    };
    responses::json(
        StatusCode::OK,
        &runtime
            .models_list_with(&[resolved], !is_internal_probe(&request))
            .await,
    )
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
            // Read inside `execute_chat`, once per request.
            pxpipe: None,
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

/// `POST /v1/videos/{generations,edits,extensions}` — create an async video job.
pub(crate) async fn video_create(
    runtime: web::Data<Runtime>,
    request: HttpRequest,
    path: web::Path<String>,
    body: web::Bytes,
) -> Result<HttpResponse, RuntimeError> {
    if let Some(rejection) = runtime.enforce_api_key(extract_api_key(&request)).await {
        return Ok(rejection);
    }
    let segment = path.into_inner();
    let Some(action) = video::VideoAction::parse(&segment) else {
        // The route pattern already restricts this, so reaching here means the
        // pattern and the parser disagree — answered rather than assumed.
        return Ok(responses::json(
            StatusCode::NOT_FOUND,
            &nullrouter_execute::build_error_body(404, &format!("Unknown video action: {segment}")),
        ));
    };

    Ok(runtime
        .execute_video(&video::VideoRequest {
            endpoint: request.path(),
            action: Some(action),
            job_id: None,
            body: &body,
            content_type: header_str(&request, actix_web::http::header::CONTENT_TYPE.as_str()),
            idempotency_key: header_str(&request, "idempotency-key"),
            preferred_connection: preferred_connection(&request),
        })
        .await)
}

/// `GET /v1/videos/{id}` — poll a job.
pub(crate) async fn video_status(
    runtime: web::Data<Runtime>,
    request: HttpRequest,
    path: web::Path<String>,
) -> Result<HttpResponse, RuntimeError> {
    if let Some(rejection) = runtime.enforce_api_key(extract_api_key(&request)).await {
        return Ok(rejection);
    }
    let job_id = path.into_inner();
    if job_id.trim().is_empty() {
        return Ok(responses::json(
            StatusCode::BAD_REQUEST,
            &nullrouter_execute::build_error_body(400, "Missing video request id"),
        ));
    }

    Ok(runtime
        .execute_video(&video::VideoRequest {
            endpoint: request.path(),
            action: None,
            job_id: Some(&job_id),
            body: &[],
            content_type: None,
            idempotency_key: None,
            preferred_connection: preferred_connection(&request),
        })
        .await)
}

/// The account a client is pinning to.
///
/// `x-connection-id` is what upstream's clients send; the header this router emits
/// on a create is also accepted, so a client that simply echoes back what it
/// received works without rewriting the name.
fn preferred_connection(request: &HttpRequest) -> Option<&str> {
    header_str(request, "x-connection-id").or_else(|| header_str(request, video::CONNECTION_HEADER))
}

/// A request header as a string, when present and valid UTF-8.
fn header_str<'a>(request: &'a HttpRequest, name: &str) -> Option<&'a str> {
    request
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
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
            pxpipe: None,
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
            // Passthrough sends the client's own bytes; nothing reshapes them.
            pxpipe: None,
        })
        .await)
}
