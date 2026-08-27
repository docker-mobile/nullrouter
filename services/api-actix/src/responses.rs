use actix_web::{
    HttpResponse, HttpResponseBuilder,
    http::{
        StatusCode,
        header::{
            ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS,
            ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_EXPOSE_HEADERS, CACHE_CONTROL, CONNECTION,
            CONTENT_TYPE,
        },
    },
};
use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
pub(super) struct ErrorMessage {
    pub error: &'static str,
}

pub(super) const fn error(error: &'static str) -> ErrorMessage {
    ErrorMessage { error }
}

pub(super) fn json<T>(status: StatusCode, body: &T) -> HttpResponse
where
    T: Serialize,
{
    let mut builder = HttpResponse::build(status);
    apply_cors(&mut builder);
    builder.json(body)
}

pub(super) fn text(status: StatusCode, body: &'static str) -> HttpResponse {
    let mut builder = HttpResponse::build(status);
    apply_cors(&mut builder);
    builder.body(body)
}

pub(super) fn empty(status: StatusCode) -> HttpResponse {
    let mut builder = HttpResponse::build(status);
    apply_cors(&mut builder);
    builder.finish()
}

/// Relay an upstream body with its own content type.
///
/// Used when forwarding to another service, so a JSON or SSE reply reaches the
/// client unchanged instead of being re-serialized.
pub(super) fn passthrough(status: StatusCode, content_type: &str, body: String) -> HttpResponse {
    let mut builder = HttpResponse::build(status);
    apply_cors(&mut builder);
    builder.insert_header((
        actix_web::http::header::CONTENT_TYPE,
        content_type.to_owned(),
    ));
    if content_type.contains("text/event-stream") {
        // Keep SSE unbuffered through any intermediary.
        builder.insert_header((actix_web::http::header::CACHE_CONTROL, "no-cache"));
        builder.insert_header(("X-Accel-Buffering", "no"));
    }
    builder.body(body)
}

pub(super) fn sse_json<T>(status: StatusCode, body: &T) -> HttpResponse
where
    T: Serialize,
{
    let payload = serialize_sse_payload(body);
    sse_response(status, format!("data: {payload}\n\ndata: [DONE]\n\n"))
}

pub(super) fn sse_event_json<T>(status: StatusCode, event: &'static str, body: &T) -> HttpResponse
where
    T: Serialize,
{
    let payload = serialize_sse_payload(body);
    sse_response(
        status,
        format!("event: {event}\ndata: {payload}\n\ndata: [DONE]\n\n"),
    )
}

fn serialize_sse_payload<T>(body: &T) -> String
where
    T: Serialize,
{
    serde_json::to_string(body).unwrap_or_else(|_| {
        r#"{"error":{"message":"Failed to serialize response","type":"internal_error"}}"#.to_owned()
    })
}

fn sse_response(status: StatusCode, body: String) -> HttpResponse {
    let mut builder = HttpResponse::build(status);
    apply_cors(&mut builder);
    builder.insert_header((CONTENT_TYPE, "text/event-stream; charset=utf-8"));
    builder.insert_header((CACHE_CONTROL, "no-cache"));
    builder.insert_header((CONNECTION, "keep-alive"));
    builder.insert_header(("X-Accel-Buffering", "no"));
    builder.body(body)
}

fn apply_cors(builder: &mut HttpResponseBuilder) {
    builder.insert_header((ACCESS_CONTROL_ALLOW_ORIGIN, "*"));
    builder.insert_header((
        ACCESS_CONTROL_ALLOW_METHODS,
        "GET, POST, PUT, PATCH, DELETE, OPTIONS",
    ));
    builder.insert_header((ACCESS_CONTROL_ALLOW_HEADERS, "*"));
    builder.insert_header((ACCESS_CONTROL_EXPOSE_HEADERS, "*"));
}
