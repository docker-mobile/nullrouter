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
        // Registered here rather than demanded from every caller: `configure` is the whole public
        // surface of this crate, so a route that needed extra `app_data` would break each existing
        // caller and every test at once. `app_data` does not replace a registration the caller
        // already made, so a `main` that wants to reap children at shutdown can still supply its
        // own bridge and keep the handle.
        .app_data(web::Data::new(crate::mcp::bridge::Bridge::default()))
        .app_data(web::Data::new(crate::console_logs::LogReader::default()))
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

/// Resolve the plugin and attach a listener, or say why not.
///
/// The refusal shape is the one this route already returned when there was no backend at all, so a
/// dashboard that handled "not connected" keeps working: an unknown plugin and an unstartable
/// server are both `backend_connected: false` with a reason, differing only in the code.
async fn mcp_attach(
    bridge: &crate::mcp::bridge::Bridge,
    plugin: &str,
) -> Result<crate::mcp::bridge::Listener, HttpResponse> {
    let Some(spec) = crate::mcp::plugins::find(plugin) else {
        let error = crate::mcp::bridge::SpawnError::UnknownPlugin;
        let message = error.message();
        return Err(json_response(
            StatusCode::NOT_FOUND,
            &McpMessageDefaultResponse {
                ok: false,
                plugin,
                backend_connected: false,
                error: error.code(),
                message: &message,
                message_kind: "attach",
            },
        ));
    };
    bridge.attach(spec).await.map_err(|error| {
        let message = error.message();
        json_response(
            StatusCode::SERVICE_UNAVAILABLE,
            &McpMessageDefaultResponse {
                ok: false,
                plugin,
                backend_connected: false,
                error: error.code(),
                message: &message,
                message_kind: "attach",
            },
        )
    })
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

/// Stream the router's log output for as long as the client stays connected.
///
/// The opening `connected` frame is kept — it is the contract this route already had — and is now
/// followed by real frames polled from the buffer in the state service. See [`crate::console_logs`]
/// for why the buffer is not here.
async fn console_logs_stream(reader: web::Data<crate::console_logs::LogReader>) -> HttpResponse {
    use futures_util::stream;

    let opening = match console_logs_stream_body() {
        Ok(opening) => opening,
        Err(error) => return sse_body_response(Err(error)),
    };

    let reader = reader.into_inner();
    // The opening frame is carried in the fold's state rather than captured, because the closure is
    // `FnMut` and cannot move it out — `Option::take` is what makes it a once-only send.
    let body = stream::unfold(
        (Some(opening), crate::console_logs::StreamState::default()),
        move |(mut opening, mut state)| {
            let reader = std::sync::Arc::clone(&reader);
            async move {
                // The opening frames are yielded alone, before any poll. Two reasons, and the
                // second is the load-bearing one: a client gets the established
                // `connected`/`console_logs`/`message` sequence as its first chunk exactly as it
                // always did, and a reader that bounds itself to those frames — which several
                // suites do, because this stream never ends — does not have a live frame arrive in
                // the middle of the sequence it is counting.
                if let Some(opening) = opening.take() {
                    return Some((
                        Ok::<actix_web::web::Bytes, actix_web::Error>(actix_web::web::Bytes::from(
                            opening,
                        )),
                        (None, state),
                    ));
                }

                actix_web::rt::time::sleep(crate::console_logs::poll_interval()).await;
                let page = reader.poll(state.cursor).await;
                let mut frame = String::new();
                if let Some(next) = crate::console_logs::next_frame(&mut state, page.as_ref()) {
                    frame.push_str(&next);
                }
                // A tick with nothing to say still yields, with an empty body. Returning `None`
                // would end the stream, which is the one thing a live log view must not do because
                // the router went quiet for half a second.
                Some((
                    Ok::<actix_web::web::Bytes, actix_web::Error>(actix_web::web::Bytes::from(
                        frame,
                    )),
                    (opening, state),
                ))
            }
        },
    );

    let mut builder = HttpResponse::Ok();
    insert_sse_headers(&mut builder);
    builder.streaming(body)
}

/// Relay one MCP server's stdout as SSE for as long as the client stays connected.
///
/// The `endpoint` event goes first, as upstream does, so the client learns where to POST before any
/// server frame arrives. Frames after that are the child's own JSON-RPC lines, filtered.
async fn mcp_sse(
    bridge: web::Data<crate::mcp::bridge::Bridge>,
    path: web::Path<PluginPath>,
) -> HttpResponse {
    use futures_util::stream;

    let plugin = path.into_inner().plugin;

    // A plugin that cannot spawn still gets a connected stream reporting `backendConnected: false`,
    // which is the contract this route already had and the honest answer: the SSE side really is
    // connected, and the backend really is not. Returning an error status instead would make a
    // dashboard treat an un-spawnable plugin as a broken route.
    let listener = mcp_attach(bridge.get_ref(), &plugin).await.ok();
    let connected = listener.is_some();

    let opening = match mcp_stream_body(&plugin, connected) {
        Ok(opening) => opening,
        Err(error) => return sse_body_response(Err(error)),
    };

    // Nothing to relay without a child: emit the opening frames and end, exactly as before.
    let Some(listener) = listener else {
        return sse_body_response(Ok(opening));
    };

    // `unfold` owns the listener, so the child is reaped when the response body is dropped —
    // which is what a client disconnect does. No separate disconnect watcher is needed.
    let body = stream::unfold(Some((listener, opening)), move |state| async move {
        let (mut listener, opening) = state?;
        if !opening.is_empty() {
            return Some((
                Ok::<web::Bytes, actix_web::Error>(web::Bytes::from(opening)),
                Some((listener, String::new())),
            ));
        }
        match listener.next_frame().await {
            Some(frame) => {
                let mut chunk = String::new();
                push_sse_event(
                    &mut chunk,
                    "message",
                    &serde_json::json!({ "frame": frame }),
                )
                .unwrap_or_default();
                Some((Ok(web::Bytes::from(chunk)), Some((listener, String::new()))))
            }
            None => {
                // Child's stdout closed. Detach explicitly rather than relying on drop order, so
                // the reap happens before the response completes.
                listener.detach().await;
                None
            }
        }
    });

    let mut builder = HttpResponse::Ok();
    insert_cors_headers(&mut builder);
    builder
        .content_type("text/event-stream")
        .insert_header(("cache-control", "no-cache, no-transform"))
        .streaming(Box::pin(body))
}

async fn mcp_message(
    bridge: web::Data<crate::mcp::bridge::Bridge>,
    path: web::Path<PluginPath>,
    body: web::Bytes,
) -> HttpResponse {
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

    // Delivered to the child's stdin. The reply is not returned here: MCP answers on the SSE
    // stream, correlated by JSON-RPC id, so a body here would be a second answer the client would
    // have to reconcile. Upstream does the same.
    match bridge.send(&plugin, &String::from_utf8_lossy(&body)).await {
        Ok(true) => json_response(
            StatusCode::ACCEPTED,
            &McpMessageDefaultResponse {
                ok: true,
                plugin: &plugin,
                backend_connected: true,
                error: "",
                message: "delivered to the MCP server; the reply arrives on the SSE stream",
                message_kind: message.kind(),
            },
        ),
        // No session: nothing is listening, so there is no stream for a reply to arrive on.
        Ok(false) => json_response(
            StatusCode::SERVICE_UNAVAILABLE,
            &McpMessageDefaultResponse {
                ok: false,
                plugin: &plugin,
                backend_connected: false,
                error: "mcp_backend_unavailable",
                message: "no MCP server is running for this plugin; open its SSE stream first",
                message_kind: message.kind(),
            },
        ),
        Err(reason) => {
            let detail = format!("the MCP server stopped accepting input: {reason}");
            json_response(
                StatusCode::BAD_GATEWAY,
                &McpMessageDefaultResponse {
                    ok: false,
                    plugin: &plugin,
                    backend_connected: false,
                    error: "mcp_write_failed",
                    message: &detail,
                    message_kind: message.kind(),
                },
            )
        }
    }
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

/// The three frames every MCP stream opens with.
///
/// `backend_connected` is a parameter rather than a constant `false` now that a whitelisted plugin
/// really can have a child behind it. The frame order and field names are unchanged, so a client
/// written against the honest-but-empty version keeps working.
fn mcp_stream_body(plugin: &str, backend_connected: bool) -> Result<String, serde_json::Error> {
    let endpoint = format!("/api/mcp/{plugin}/message");
    let mut body = String::new();
    push_sse_event(
        &mut body,
        "endpoint",
        &McpEndpointPayload {
            plugin,
            endpoint: &endpoint,
            backend_connected,
        },
    )?;
    push_sse_event(
        &mut body,
        "connected",
        &McpConnectedPayload {
            plugin,
            sse_connected: true,
            backend_connected,
            message: if backend_connected {
                "SSE stream is connected; MCP backend is running"
            } else {
                "SSE stream is connected; MCP backend is not connected"
            },
        },
    )?;
    push_sse_event(
        &mut body,
        "backend_state",
        &McpBackendStatePayload {
            plugin,
            connected: backend_connected,
            reason: if backend_connected {
                "MCP backend is running"
            } else {
                NO_ACTIVE_MCP_BACKEND
            },
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
    /// Borrowed rather than `&'static`: a spawn failure's reason comes from the OS at run time.
    message: &'a str,
    message_kind: &'static str,
}

#[derive(Debug, Serialize)]
struct ServiceError {
    error: &'static str,
    message: String,
}
