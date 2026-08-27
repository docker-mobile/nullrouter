#![allow(clippy::future_not_send)]

use actix_web::{
    App,
    http::{
        Method, StatusCode,
        header::{self, HeaderMap},
    },
    test, web,
};
use nullrouter_events::configure;
use serde_json::Value;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
/// A closed loopback port, so the live usage stream emits its offline frame.
const UNREACHABLE_STATE_ADDR: &str = "127.0.0.1:1";

/// Read only the first two SSE frames.
///
/// `/api/usage/stream` is now a live stream that never ends, so reading a
/// response to completion would hang.
async fn read_sse_prefix(
    res: actix_web::dev::ServiceResponse<actix_web::body::BoxBody>,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    use actix_web::body::MessageBody;

    let mut collected: Vec<u8> = Vec::new();
    let mut body = res.into_body();
    let mut stream = std::pin::Pin::new(&mut body);
    while collected
        .windows(2)
        .filter(|window| *window == b"\n\n")
        .count()
        < 2
    {
        match futures_util::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await {
            Some(Ok(chunk)) => collected.extend_from_slice(chunk.as_ref()),
            Some(Err(_)) | None => break,
        }
    }
    Ok(collected)
}

#[derive(Debug)]
struct TestResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: web::Bytes,
}

#[derive(Debug)]
struct SseFrame {
    event: String,
    data: Value,
}

async fn request(method: Method, uri: &str) -> TestResult<TestResponse> {
    let app = test::init_service(
        App::new()
            .app_data(actix_web::web::Data::new(
                nullrouter_events::UsageReader::new(UNREACHABLE_STATE_ADDR),
            ))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::default()
        .method(method)
        .uri(uri)
        .insert_header((header::ACCEPT, "text/event-stream"))
        .to_request();
    let res = test::call_service(&app, req).await;
    let status = res.status();
    let headers = res.headers().clone();
    let body = actix_web::web::Bytes::from(read_sse_prefix(res).await?);

    Ok(TestResponse {
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
        header_value(headers, &header::ACCESS_CONTROL_ALLOW_METHODS)?,
        "GET, POST, OPTIONS"
    );
    assert_eq!(
        header_value(headers, &header::ACCESS_CONTROL_ALLOW_HEADERS)?,
        "content-type, authorization"
    );
    Ok(())
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
async fn g013_translator_console_logs_stream_matches_dashboard_sse_contract() -> TestResult {
    // Given: no translator console log backend is connected.

    // When: the dashboard opens the translator console logs SSE route.
    let response = request(Method::GET, "/api/translator/console-logs/stream").await?;
    let frames = parse_sse(&response.body)?;

    // Then: the response is an event stream with parseable default translator frames.
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(
        header_value(&response.headers, &header::CONTENT_TYPE)?,
        "text/event-stream; charset=utf-8"
    );
    assert_cors(&response.headers)?;

    let connected = frame_data(&frames, "connected")?;
    assert_eq!(field(connected, "service")?, "nullrouter-events");
    assert_eq!(field(connected, "stream")?, "translator.console_logs");
    assert_eq!(field(connected, "connected")?, true);

    let logs = frame_data(&frames, "console_logs")?;
    assert_eq!(field(logs, "type")?, "init");
    assert!(array_field(logs, "logs")?.is_empty());
    assert_eq!(field(logs, "liveCapture")?, false);
    assert!(logs.get("live_capture").is_none());
    Ok(())
}

#[actix_web::test]
async fn g013_translator_console_logs_options_returns_cors_preflight() -> TestResult {
    // Given: browsers preflight the translator console logs stream route.

    // When: the route receives an OPTIONS request.
    let response = request(Method::OPTIONS, "/api/translator/console-logs/stream").await?;

    // Then: the service returns the shared no-content CORS response.
    assert_eq!(response.status, StatusCode::NO_CONTENT);
    assert_cors(&response.headers)?;
    assert!(response.body.is_empty());
    Ok(())
}

#[actix_web::test]
async fn g013_neighboring_event_stream_routes_remain_parseable() -> TestResult {
    // Given: translator parity work shares the events service with usage and MCP streams.
    let routes = [
        ("/api/usage/stream", ["connected", "usage"].as_slice()),
        (
            "/api/mcp/test/sse",
            ["endpoint", "connected", "backend_state"].as_slice(),
        ),
    ];

    // When: neighboring SSE routes are requested.
    for (uri, expected_events) in routes {
        let response = request(Method::GET, uri).await?;
        let frames = parse_sse(&response.body)?;

        // Then: they still return parseable event-stream frames.
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(
            header_value(&response.headers, &header::CONTENT_TYPE)?,
            "text/event-stream; charset=utf-8"
        );
        for expected_event in expected_events {
            frame_data(&frames, expected_event)?;
        }
    }

    Ok(())
}
