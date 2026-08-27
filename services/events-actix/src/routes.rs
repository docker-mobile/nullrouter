use actix_web::{
    HttpResponse, HttpResponseBuilder, guard,
    http::{StatusCode, header},
    web,
};
use serde::{Deserialize, Serialize};

use crate::SERVICE_NAME;

const SSE_CONTENT_TYPE: &str = "text/event-stream; charset=utf-8";
const CORS_ALLOW_HEADERS: &str = "content-type, authorization";
const CORS_ALLOW_METHODS: &str = "GET, POST, OPTIONS";
const NO_ACTIVE_MCP_BACKEND: &str = "no active MCP backend";

pub fn configure(config: &mut web::ServiceConfig) {
    config
        .route("/health", web::get().to(health))
        .service(
            web::resource("/api/usage/stream")
                .route(web::get().to(usage_stream))
                .route(web::route().guard(guard::Options()).to(options)),
        )
        .service(
            web::resource("/api/translator/console-logs/stream")
                .route(web::get().to(console_logs_stream))
                .route(web::route().guard(guard::Options()).to(options)),
        )
        .service(
            web::resource("/api/mcp/{plugin}/sse")
                .route(web::get().to(mcp_sse))
                .route(web::route().guard(guard::Options()).to(options)),
        )
        .service(
            web::resource("/api/mcp/{plugin}/message")
                .route(web::post().to(mcp_message))
                .route(web::route().guard(guard::Options()).to(options)),
        );
}

async fn health() -> HttpResponse {
    json_response(
        StatusCode::OK,
        &HealthResponse {
            ok: true,
            service: SERVICE_NAME,
        },
    )
}

/// Live usage telemetry.
///
/// Streams a `usage` event per tick for as long as the client stays connected,
/// rather than the single static frame this route used to return. Dropping the
/// response ends the stream, so a disconnect stops the polling.
async fn usage_stream(reader: web::Data<crate::usage_stream::UsageReader>) -> HttpResponse {
    use futures_util::stream;

    let reader = reader.into_inner();
    let body = stream::unfold(true, move |first| {
        let reader = std::sync::Arc::clone(&reader);
        async move {
            if !first {
                tokio::time::sleep(crate::usage_stream::poll_interval()).await;
            }
            let mut frame = String::new();
            if first {
                frame.push_str(&crate::usage_stream::connected_frame(SERVICE_NAME));
            }
            let snapshot = reader.snapshot().await;
            frame.push_str(&crate::usage_stream::sse_event("usage", &snapshot));
            Some((
                Ok::<actix_web::web::Bytes, actix_web::Error>(actix_web::web::Bytes::from(frame)),
                false,
            ))
        }
    });

    let mut builder = HttpResponse::Ok();
    insert_sse_headers(&mut builder);
    builder.streaming(body)
}

async fn console_logs_stream() -> HttpResponse {
    sse_body_response(console_logs_stream_body())
}

async fn mcp_sse(path: web::Path<PluginPath>) -> HttpResponse {
    let plugin = path.into_inner().plugin;
    sse_body_response(mcp_stream_body(&plugin))
}

async fn mcp_message(path: web::Path<PluginPath>, body: web::Bytes) -> HttpResponse {
    let plugin = path.into_inner().plugin;
    let message = match parse_mcp_message(&body) {
        Ok(message) => message,
        Err(error) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                &McpMessageError {
                    error: "invalid_json",
                    plugin: &plugin,
                    message: format!("request body must be valid JSON: {error}"),
                },
            );
        }
    };

    json_response(
        StatusCode::SERVICE_UNAVAILABLE,
        &McpMessageDefaultResponse {
            ok: false,
            plugin: &plugin,
            backend_connected: false,
            error: "mcp_backend_unavailable",
            message: "MCP messages cannot be delivered because no backend is connected",
            message_kind: message.kind(),
        },
    )
}

async fn options() -> HttpResponse {
    let mut builder = HttpResponse::NoContent();
    insert_cors_headers(&mut builder);
    builder.finish()
}

fn console_logs_stream_body() -> Result<String, serde_json::Error> {
    let mut body = String::new();
    push_sse_event(
        &mut body,
        "connected",
        &ConnectedPayload {
            service: SERVICE_NAME,
            stream: "translator.console_logs",
            connected: true,
        },
    )?;
    push_sse_event(
        &mut body,
        "console_logs",
        &ConsoleLogsPayload {
            kind: "init",
            logs: &[],
            live_capture: false,
        },
    )?;
    push_sse_event(
        &mut body,
        "message",
        &ConsoleLogsPayload {
            kind: "init",
            logs: &[],
            live_capture: false,
        },
    )?;
    Ok(body)
}

fn mcp_stream_body(plugin: &str) -> Result<String, serde_json::Error> {
    let endpoint = format!("/api/mcp/{plugin}/message");
    let mut body = String::new();
    push_sse_event(
        &mut body,
        "endpoint",
        &McpEndpointPayload {
            plugin,
            endpoint: &endpoint,
            backend_connected: false,
        },
    )?;
    push_sse_event(
        &mut body,
        "connected",
        &McpConnectedPayload {
            plugin,
            sse_connected: true,
            backend_connected: false,
            message: "SSE stream is connected; MCP backend is not connected",
        },
    )?;
    push_sse_event(
        &mut body,
        "backend_state",
        &McpBackendStatePayload {
            plugin,
            connected: false,
            reason: NO_ACTIVE_MCP_BACKEND,
        },
    )?;
    Ok(body)
}

fn parse_mcp_message(body: &[u8]) -> Result<McpMessage, serde_json::Error> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(McpMessage::default());
    }
    serde_json::from_slice(body)
}

fn sse_body_response(body: Result<String, serde_json::Error>) -> HttpResponse {
    match body {
        Ok(body) => {
            let mut builder = HttpResponse::Ok();
            insert_sse_headers(&mut builder);
            builder.body(body)
        }
        Err(error) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &ServiceError {
                error: "sse_encoding_failed",
                message: format!("failed to encode SSE frame: {error}"),
            },
        ),
    }
}

fn push_sse_event<T: Serialize>(
    body: &mut String,
    event: &str,
    data: &T,
) -> Result<(), serde_json::Error> {
    body.push_str("event: ");
    body.push_str(event);
    body.push('\n');
    body.push_str("data: ");
    body.push_str(&serde_json::to_string(data)?);
    body.push_str("\n\n");
    Ok(())
}

fn json_response<T: Serialize>(status: StatusCode, body: &T) -> HttpResponse {
    let mut builder = HttpResponse::build(status);
    insert_cors_headers(&mut builder);
    builder.json(body)
}

fn insert_sse_headers(builder: &mut HttpResponseBuilder) {
    insert_cors_headers(builder);
    builder.insert_header((header::CONTENT_TYPE, SSE_CONTENT_TYPE));
    builder.insert_header((header::CACHE_CONTROL, "no-cache, no-transform"));
    builder.insert_header((header::CONNECTION, "keep-alive"));
    builder.insert_header((header::HeaderName::from_static("x-accel-buffering"), "no"));
}

fn insert_cors_headers(builder: &mut HttpResponseBuilder) {
    builder.insert_header((header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"));
    builder.insert_header((header::ACCESS_CONTROL_ALLOW_METHODS, CORS_ALLOW_METHODS));
    builder.insert_header((header::ACCESS_CONTROL_ALLOW_HEADERS, CORS_ALLOW_HEADERS));
}

#[derive(Debug, Clone, Copy, Serialize)]
struct HealthResponse {
    ok: bool,
    service: &'static str,
}

#[derive(Debug, Deserialize)]
struct PluginPath {
    plugin: String,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct ConnectedPayload<'a> {
    service: &'static str,
    stream: &'a str,
    connected: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct ConsoleLogLine;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConsoleLogsPayload<'a> {
    /// Serialized as `type`, which is what the dashboard reads.
    #[serde(rename = "type")]
    kind: &'static str,
    logs: &'a [ConsoleLogLine],
    live_capture: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
struct McpEndpointPayload<'a> {
    plugin: &'a str,
    endpoint: &'a str,
    backend_connected: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
struct McpConnectedPayload<'a> {
    plugin: &'a str,
    sse_connected: bool,
    backend_connected: bool,
    message: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct McpBackendStatePayload<'a> {
    plugin: &'a str,
    connected: bool,
    reason: &'static str,
}

#[derive(Debug, Default, Deserialize)]
struct McpMessage {
    #[serde(default)]
    jsonrpc: Option<String>,
    #[serde(default)]
    method: Option<String>,
}

impl McpMessage {
    fn kind(&self) -> &'static str {
        if self
            .method
            .as_deref()
            .is_some_and(|method| !method.trim().is_empty())
        {
            "method"
        } else if self
            .jsonrpc
            .as_deref()
            .is_some_and(|version| !version.trim().is_empty())
        {
            "jsonrpc"
        } else {
            "default"
        }
    }
}

#[derive(Debug, Serialize)]
struct McpMessageError<'a> {
    error: &'static str,
    plugin: &'a str,
    message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct McpMessageDefaultResponse<'a> {
    ok: bool,
    plugin: &'a str,
    backend_connected: bool,
    error: &'static str,
    message: &'static str,
    message_kind: &'static str,
}

#[derive(Debug, Serialize)]
struct ServiceError {
    error: &'static str,
    message: String,
}
