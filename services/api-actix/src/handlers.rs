use actix_web::{HttpResponse, http::StatusCode, web};
use nullrouter_contracts::{
    ChatRequest, health_response, init_response, keys_response, model_list,
    provider_execution_error, providers_client_response, responses_failed_event, settings_response,
    version_response,
};
use serde::Deserialize;

use crate::{AppConfig, errors::ApiError, models, responses};

pub(super) async fn health() -> HttpResponse {
    responses::json(StatusCode::OK, &health_response())
}

pub(super) async fn init() -> HttpResponse {
    responses::text(StatusCode::OK, init_response())
}

pub(super) async fn no_content() -> HttpResponse {
    responses::empty(StatusCode::NO_CONTENT)
}

pub(super) async fn not_found() -> HttpResponse {
    responses::json(StatusCode::NOT_FOUND, &responses::error("Route not found"))
}

pub(super) async fn version(config: web::Data<AppConfig>) -> HttpResponse {
    responses::json(StatusCode::OK, &version_response(config.version))
}

pub(super) async fn status(config: web::Data<AppConfig>) -> HttpResponse {
    responses::json(
        StatusCode::OK,
        &serde_json::json!({
            "ok": true,
            "service": "nullrouter-api",
            "version": config.version,
        }),
    )
}

pub(super) async fn api_models() -> HttpResponse {
    responses::json(StatusCode::OK, &models::dashboard_models())
}

pub(super) async fn openai_models() -> HttpResponse {
    responses::json(StatusCode::OK, &model_list())
}

pub(super) async fn settings() -> HttpResponse {
    responses::json(StatusCode::OK, &settings_response())
}

pub(super) async fn keys() -> HttpResponse {
    responses::json(StatusCode::OK, &keys_response())
}

pub(super) async fn providers_client() -> HttpResponse {
    responses::json(StatusCode::OK, &providers_client_response())
}

#[derive(Debug, Clone, Copy)]
enum StreamDefault {
    Disabled,
    Enabled,
}

#[derive(Debug, Clone, Copy)]
enum StreamFailureFormat {
    OpenAiData,
    ResponsesApiEvent,
}

#[derive(Debug, Deserialize)]
struct DashboardChatRequest {
    model: Option<String>,
    messages: Option<Vec<serde_json::Value>>,
    stream: Option<bool>,
}

pub(super) async fn chat_completions(body: web::Bytes) -> Result<HttpResponse, ApiError> {
    chat_entrypoint(
        &body,
        StreamDefault::Disabled,
        StreamFailureFormat::OpenAiData,
    )
}

pub(super) async fn responses_endpoint(body: web::Bytes) -> Result<HttpResponse, ApiError> {
    chat_entrypoint(
        &body,
        StreamDefault::Enabled,
        StreamFailureFormat::ResponsesApiEvent,
    )
}

pub(super) async fn messages(body: web::Bytes) -> Result<HttpResponse, ApiError> {
    chat_entrypoint(
        &body,
        StreamDefault::Enabled,
        StreamFailureFormat::OpenAiData,
    )
}

/// The dashboard's basic-chat endpoint.
///
/// Validates the request, then forwards it to `nullrouter-runtime`, which owns
/// provider execution. Relays the runtime's status, content type, and body so
/// both JSON and SSE replies pass through unchanged.
pub(super) async fn dashboard_chat_completions(
    runtime: web::Data<crate::state_client::RuntimeClient>,
    body: web::Bytes,
) -> Result<HttpResponse, ApiError> {
    let request: DashboardChatRequest =
        serde_json::from_slice(&body).map_err(|_| ApiError::BadRequest("Invalid JSON body"))?;
    let model = required_model(request.model)?;
    if request.messages.is_none() {
        return Err(ApiError::BadRequest("Missing required field: messages"));
    }
    let stream = request.stream.unwrap_or(false);

    let Some(forwarded) = runtime.forward_chat(&body).await else {
        // The runtime is down: report it explicitly rather than as a transport error.
        return Ok(provider_execution_response(
            &model,
            stream,
            StreamFailureFormat::OpenAiData,
        ));
    };

    let status = StatusCode::from_u16(forwarded.status).unwrap_or(StatusCode::BAD_GATEWAY);
    Ok(responses::passthrough(
        status,
        &forwarded.content_type,
        forwarded.body,
    ))
}

fn chat_entrypoint(
    body: &[u8],
    stream_default: StreamDefault,
    stream_failure_format: StreamFailureFormat,
) -> Result<HttpResponse, ApiError> {
    let request: ChatRequest =
        serde_json::from_slice(body).map_err(|_| ApiError::BadRequest("Invalid JSON body"))?;
    let model = required_model(request.model)?;

    let stream = request.stream.unwrap_or(match stream_default {
        StreamDefault::Disabled => false,
        StreamDefault::Enabled => true,
    });

    Ok(provider_execution_response(
        &model,
        stream,
        stream_failure_format,
    ))
}

fn required_model(model: Option<String>) -> Result<String, ApiError> {
    let model = model.unwrap_or_default();
    if model.trim().is_empty() {
        return Err(ApiError::BadRequest("Missing required field: model"));
    }

    Ok(model)
}

fn provider_execution_response(
    model: &str,
    stream: bool,
    stream_failure_format: StreamFailureFormat,
) -> HttpResponse {
    let error = provider_execution_error(model, stream);

    if stream {
        match stream_failure_format {
            StreamFailureFormat::OpenAiData => {
                responses::sse_json(StatusCode::NOT_IMPLEMENTED, &error)
            }
            StreamFailureFormat::ResponsesApiEvent => {
                let event = responses_failed_event(error);
                responses::sse_event_json(StatusCode::NOT_IMPLEMENTED, "response.failed", &event)
            }
        }
    } else {
        responses::json(StatusCode::NOT_IMPLEMENTED, &error)
    }
}
