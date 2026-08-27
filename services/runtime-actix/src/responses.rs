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

pub(crate) fn json<T>(status: StatusCode, body: &T) -> HttpResponse
where
    T: Serialize,
{
    let mut builder = HttpResponse::build(status);
    apply_cors(&mut builder);
    builder.json(body)
}

pub(crate) fn empty(status: StatusCode) -> HttpResponse {
    let mut builder = HttpResponse::build(status);
    apply_cors(&mut builder);
    builder.finish()
}

/// Stream SSE frames as they are produced.
///
/// Preferred over [`sse_body`] for provider responses: the client receives each
/// frame at the provider's latency instead of after the full completion.
pub(crate) fn sse_stream<S>(status: StatusCode, body: S) -> HttpResponse
where
    S: futures_util::Stream<Item = Result<actix_web::web::Bytes, actix_web::Error>> + 'static,
{
    let mut builder = HttpResponse::build(status);
    apply_cors(&mut builder);
    apply_sse_headers(&mut builder);
    builder.streaming(body)
}

/// Send an already-framed SSE body.
pub(crate) fn sse_body(status: StatusCode, body: String) -> HttpResponse {
    sse_response(status, body)
}

fn sse_response(status: StatusCode, body: String) -> HttpResponse {
    let mut builder = HttpResponse::build(status);
    apply_cors(&mut builder);
    apply_sse_headers(&mut builder);
    builder.body(body)
}

/// SSE headers, including the hints that keep intermediaries from buffering.
fn apply_sse_headers(builder: &mut HttpResponseBuilder) {
    builder.insert_header((CONTENT_TYPE, "text/event-stream; charset=utf-8"));
    builder.insert_header((CACHE_CONTROL, "no-cache"));
    builder.insert_header((CONNECTION, "keep-alive"));
    // Without this nginx-class proxies buffer the whole stream, defeating
    // incremental delivery.
    builder.insert_header(("X-Accel-Buffering", "no"));
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
