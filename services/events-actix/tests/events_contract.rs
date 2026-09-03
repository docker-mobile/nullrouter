#![allow(clippy::future_not_send)]

use actix_web::body::MessageBody;
use actix_web::{
    App,
    http::{
        StatusCode,
        header::{self, HeaderMap},
    },
    test, web,
};
use nullrouter_events::configure;
use serde_json::Value;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[derive(Debug)]
struct TestBodyResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: web::Bytes,
}

#[derive(Debug)]
struct SseFrame {
    event: String,
    data: Value,
}

/// A closed loopback port: usage reads fail, so the stream emits its offline
/// frame — which is exactly the default shape these contract tests assert.
const UNREACHABLE_STATE_ADDR: &str = "127.0.0.1:1";

/// Point the console-log stream at a closed port for one test.
///
/// The test used to rely on no process happening to own :20134. The full workspace suite starts
/// services in other integration-test binaries, and processes can overlap, so that was a race: an
/// unrelated state service turned the expected outage into a valid empty buffer and eventually a
/// keepalive. The address is read when Actix configures [`nullrouter_events::LogReader`], so the
/// guard surrounds app construction as well as reading the stream.
struct ConsoleLogStateAddr {
    _lock: std::sync::MutexGuard<'static, ()>,
    previous: Option<std::ffi::OsString>,
}

impl ConsoleLogStateAddr {
    fn unreachable() -> Self {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        let lock = LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = std::env::var_os("NULLROUTER_STATE_ADDR");
        // SAFETY: `lock` serialises every mutation in this test binary, and the guard restores the
        // inherited value before releasing it.
        unsafe { std::env::set_var("NULLROUTER_STATE_ADDR", UNREACHABLE_STATE_ADDR) };
        Self {
            _lock: lock,
            previous,
        }
    }
}

impl Drop for ConsoleLogStateAddr {
    fn drop(&mut self) {
        match &self.previous {
            // SAFETY: the mutex is held until this guard finishes dropping.
            Some(previous) => unsafe { std::env::set_var("NULLROUTER_STATE_ADDR", previous) },
            // SAFETY: as above.
            None => unsafe { std::env::remove_var("NULLROUTER_STATE_ADDR") },
        }
    }
}

fn test_app_data() -> web::Data<nullrouter_events::UsageReader> {
    web::Data::new(nullrouter_events::UsageReader::new(UNREACHABLE_STATE_ADDR))
}

async fn get(uri: &str) -> TestResult<TestBodyResponse> {
    let app = test::init_service(App::new().app_data(test_app_data()).configure(configure)).await;
    let req = test::TestRequest::get().uri(uri).to_request();
    let res = test::call_service(&app, req).await;
    let status = res.status();
    let headers = res.headers().clone();
    let body = test::read_body(res).await;

    Ok(TestBodyResponse {
        status,
        headers,
        body,
    })
}

/// Read only the first `frames` SSE events from a response.
///
/// The usage stream is live and never ends, so a bounded read is required;
/// reading to completion would hang.
async fn get_stream_prefix(uri: &str, frames: usize) -> TestResult<TestBodyResponse> {
    let app = test::init_service(App::new().app_data(test_app_data()).configure(configure)).await;
    let req = test::TestRequest::get().uri(uri).to_request();
    let res = test::call_service(&app, req).await;
    let status = res.status();
    let headers = res.headers().clone();

    let mut collected: Vec<u8> = Vec::new();
    let mut body = res.into_body();
    let mut stream = std::pin::Pin::new(&mut body);
    // Count completed frames by their blank-line terminators.
    while collected
        .windows(2)
        .filter(|window| *window == b"\n\n")
        .count()
        < frames
    {
        match futures_util::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await {
            Some(Ok(chunk)) => collected.extend_from_slice(chunk.as_ref()),
            Some(Err(_)) | None => break,
        }
    }

    Ok(TestBodyResponse {
        status,
        headers,
        body: web::Bytes::from(collected),
    })
}

async fn post_json(uri: &str, payload: &str) -> TestResult<TestBodyResponse> {
    let app = test::init_service(App::new().configure(configure)).await;
    let req = test::TestRequest::post()
        .uri(uri)
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(payload.to_owned())
        .to_request();
    let res = test::call_service(&app, req).await;
    let status = res.status();
    let headers = res.headers().clone();
    let body = test::read_body(res).await;

    Ok(TestBodyResponse {
        status,
        headers,
        body,
    })
}

async fn options(uri: &str) -> TestResult<TestBodyResponse> {
    let app = test::init_service(App::new().configure(configure)).await;
    let req = test::TestRequest::default()
        .method(actix_web::http::Method::OPTIONS)
        .uri(uri)
        .to_request();
    let res = test::call_service(&app, req).await;
    let status = res.status();
    let headers = res.headers().clone();
    let body = test::read_body(res).await;

    Ok(TestBodyResponse {
        status,
        headers,
        body,
    })
}

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}

fn header_value<'a>(headers: &'a HeaderMap, name: &header::HeaderName) -> TestResult<&'a str> {
    headers
        .get(name.as_str())
        .ok_or_else(|| test_error(format!("missing header {name}")))?
        .to_str()
        .map_err(|error| test_error(format!("invalid header {name}: {error}")))
}

fn assert_cors(headers: &HeaderMap) -> TestResult {
    assert_eq!(
        header_value(headers, &header::ACCESS_CONTROL_ALLOW_ORIGIN)?,
        "*"
    );
    assert_eq!(
        header_value(headers, &header::ACCESS_CONTROL_ALLOW_HEADERS)?,
        "content-type, authorization"
    );
    Ok(())
}

fn assert_sse_headers(headers: &HeaderMap) -> TestResult {
    assert_eq!(
        header_value(headers, &header::CONTENT_TYPE)?,
        "text/event-stream; charset=utf-8"
    );
    assert_eq!(
        header_value(headers, &header::CACHE_CONTROL)?,
        "no-cache, no-transform"
    );
    assert_eq!(header_value(headers, &header::CONNECTION)?, "keep-alive");
    assert_eq!(
        header_value(
            headers,
            &header::HeaderName::from_static("x-accel-buffering")
        )?,
        "no"
    );
    assert_cors(headers)
}

fn parse_sse(body: &[u8]) -> TestResult<Vec<SseFrame>> {
    let body = std::str::from_utf8(body)?;
    let mut frames = Vec::new();

    for raw_frame in body.split("\n\n").filter(|frame| !frame.trim().is_empty()) {
        let mut event = None;
        let mut data = String::new();

        for line in raw_frame.lines() {
            if let Some(value) = line.strip_prefix("event: ") {
                event = Some(value.to_owned());
            } else if let Some(value) = line.strip_prefix("data: ") {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(value);
            }
        }

        let event = event.ok_or_else(|| test_error("missing SSE event field"))?;
        let data = serde_json::from_str(&data)
            .map_err(|error| test_error(format!("invalid SSE JSON data for {event}: {error}")))?;
        frames.push(SseFrame { event, data });
    }

    Ok(frames)
}

fn frame_data<'a>(frames: &'a [SseFrame], event: &str) -> TestResult<&'a Value> {
    frames
        .iter()
        .find(|frame| frame.event == event)
        .map(|frame| &frame.data)
        .ok_or_else(|| test_error(format!("missing SSE event {event}")))
}

fn field<'a>(json: &'a Value, name: &str) -> TestResult<&'a Value> {
    json.get(name)
        .ok_or_else(|| test_error(format!("missing field {name}")))
}

fn array_field<'a>(json: &'a Value, name: &str) -> TestResult<&'a [Value]> {
    field(json, name)?
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| test_error(format!("{name} is an array")))
}

#[actix_web::test]
async fn health_returns_events_service_status_when_requested() -> TestResult {
    // Given: the events service routes are configured.

    // When: health is requested.
    let response = get("/health").await?;
    let json: Value = serde_json::from_slice(&response.body)?;

    // Then: the service reports the events identity as JSON.
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(
        json,
        serde_json::json!({ "ok": true, "service": "nullrouter-events" })
    );
    Ok(())
}

#[actix_web::test]
async fn usage_stream_returns_parseable_default_sse_when_requested() -> TestResult {
    // Given: no live usage telemetry backend is connected.

    // When: the first two frames of the live usage stream are read. The stream
    // never ends, so only a bounded prefix is consumed.
    let response = get_stream_prefix("/api/usage/stream", 2).await?;
    let frames = parse_sse(&response.body)?;

    // Then: the stream is event-stream encoded and opens with default usage data.
    assert_eq!(response.status, StatusCode::OK);
    assert_sse_headers(&response.headers)?;

    let connected = frame_data(&frames, "connected")?;
    assert_eq!(field(connected, "service")?, "nullrouter-events");
    assert_eq!(field(connected, "stream")?, "usage");
    assert_eq!(field(connected, "connected")?, true);

    let usage = frame_data(&frames, "usage")?;
    assert_eq!(field(usage, "liveTelemetry")?, false);
    assert_eq!(field(usage, "activeRequests")?, 0);
    assert_eq!(field(usage, "requestsToday")?, 0);
    assert_eq!(field(usage, "tokensToday")?, 0);
    assert!(array_field(usage, "recentRequests")?.is_empty());
    Ok(())
}

#[actix_web::test]
async fn console_logs_stream_returns_parseable_default_sse_when_requested() -> TestResult {
    // Given: no state service to hold the buffer, so nothing can be captured.
    let _addr = ConsoleLogStateAddr::unreachable();

    // When: the opening frames are read. This stream is now live and never ends — it polls the
    // buffer in the state service — so only a bounded prefix is consumed. Reading to completion
    // would hang, which is what it did when this test was written against a static body.
    let response = get_stream_prefix("/api/translator/console-logs/stream", 2).await?;
    let frames = parse_sse(&response.body)?;

    // Then: the stream is event-stream encoded and opens with the same two frames it always did.
    assert_eq!(response.status, StatusCode::OK);
    assert_sse_headers(&response.headers)?;

    let connected = frame_data(&frames, "connected")?;
    assert_eq!(field(connected, "stream")?, "translator.console_logs");
    assert_eq!(field(connected, "connected")?, true);

    let logs = frame_data(&frames, "console_logs")?;
    assert_eq!(field(logs, "type")?, "init");
    // `liveCapture: false` in the opening frame, because at this point nothing has been read yet.
    // Whether capture is actually live is answered by the polled frames that follow.
    assert_eq!(field(logs, "liveCapture")?, false);
    assert!(array_field(logs, "logs")?.is_empty());
    Ok(())
}

#[actix_web::test]
async fn the_console_log_stream_reports_an_unreachable_buffer_rather_than_going_quiet() -> TestResult
{
    // Given: no state service listening, which is what these tests run against.
    let _addr = ConsoleLogStateAddr::unreachable();

    // When: enough of the stream is read to get past the opening frames. Those are three —
    // `connected`, the named `console_logs` init, and its unnamed duplicate for `onmessage` — and
    // they arrive before any poll, so the fourth is the first polled frame.
    let response = get_stream_prefix("/api/translator/console-logs/stream", 4).await?;
    let body = String::from_utf8_lossy(&response.body).into_owned();

    // Then: the outage is said out loud. An empty pane would read as a quiet router, which is the
    // opposite of what someone opening their logs needs to know.
    assert!(
        body.contains("unreachable"),
        "the stream should report that the buffer cannot be read: {body}"
    );
    assert!(
        body.contains("\"liveCapture\":false"),
        "and mark capture as not live: {body}"
    );
    Ok(())
}

#[actix_web::test]
async fn mcp_sse_returns_plugin_scoped_default_sse_when_requested() -> TestResult {
    // Given: no MCP backend is connected for the test plugin.

    // When: the plugin SSE route is requested.
    let response = get("/api/mcp/test/sse").await?;
    let frames = parse_sse(&response.body)?;

    // Then: the stream advertises the plugin message endpoint without claiming delivery.
    assert_eq!(response.status, StatusCode::OK);
    assert_sse_headers(&response.headers)?;

    let endpoint = frame_data(&frames, "endpoint")?;
    assert_eq!(field(endpoint, "plugin")?, "test");
    assert_eq!(field(endpoint, "endpoint")?, "/api/mcp/test/message");
    assert_eq!(field(endpoint, "backendConnected")?, false);

    let connected = frame_data(&frames, "connected")?;
    assert_eq!(field(connected, "plugin")?, "test");
    assert_eq!(field(connected, "sseConnected")?, true);
    assert_eq!(field(connected, "backendConnected")?, false);

    let state = frame_data(&frames, "backend_state")?;
    assert_eq!(field(state, "plugin")?, "test");
    assert_eq!(field(state, "connected")?, false);
    assert_eq!(field(state, "reason")?, "no active MCP backend");
    Ok(())
}

#[actix_web::test]
async fn mcp_message_returns_structured_bad_request_when_json_is_malformed() -> TestResult {
    // Given: a plugin message request with malformed JSON.

    // When: the message endpoint receives the payload.
    let response = post_json("/api/mcp/test/message", "{").await?;
    let json: Value = serde_json::from_slice(&response.body)?;

    // Then: the service returns a structured JSON boundary error, not HTML.
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        header_value(&response.headers, &header::CONTENT_TYPE)?,
        "application/json"
    );
    assert_cors(&response.headers)?;
    assert_eq!(field(&json, "error")?, "invalid_json");
    assert_eq!(field(&json, "plugin")?, "test");
    Ok(())
}

#[actix_web::test]
async fn mcp_message_returns_structured_default_state_when_payload_is_empty() -> TestResult {
    // Given: a plugin message request with an empty default JSON object.

    // When: the message endpoint receives the payload.
    let response = post_json("/api/mcp/test/message", "{}").await?;
    let json: Value = serde_json::from_slice(&response.body)?;

    // Then: the service honestly reports that no backend is connected.
    assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        header_value(&response.headers, &header::CONTENT_TYPE)?,
        "application/json"
    );
    assert_cors(&response.headers)?;
    assert_eq!(field(&json, "ok")?, false);
    assert_eq!(field(&json, "plugin")?, "test");
    assert_eq!(field(&json, "backendConnected")?, false);
    assert_eq!(field(&json, "error")?, "mcp_backend_unavailable");
    Ok(())
}

#[actix_web::test]
async fn options_preflight_returns_cors_when_requested_for_event_routes() -> TestResult {
    // Given: browsers preflight the event-stream and MCP message routes.
    let routes = [
        "/api/usage/stream",
        "/api/translator/console-logs/stream",
        "/api/mcp/test/sse",
        "/api/mcp/test/message",
    ];

    // When: each route receives an OPTIONS request.
    for route in routes {
        let response = options(route).await?;

        // Then: each route exposes CORS preflight metadata without a body.
        assert_eq!(response.status, StatusCode::NO_CONTENT);
        assert_cors(&response.headers)?;
        assert_eq!(
            header_value(&response.headers, &header::ACCESS_CONTROL_ALLOW_METHODS)?,
            "GET, POST, OPTIONS"
        );
        assert!(response.body.is_empty());
    }
    Ok(())
}
