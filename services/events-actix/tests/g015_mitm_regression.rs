#![allow(clippy::future_not_send)]

use actix_web::{
    App,
    http::{
        Method, StatusCode,
        header::{self, HeaderMap},
    },
    test,
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

const SSE_CONTENT_TYPE: &str = "text/event-stream; charset=utf-8";

#[derive(Debug)]
struct TestResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: String,
}

#[derive(Debug)]
struct SseFrame {
    event: String,
    data: Value,
}

async fn request(method: Method, uri: &str, accept: Option<&str>) -> TestResult<TestResponse> {
    let app = test::init_service(
        App::new()
            .app_data(actix_web::web::Data::new(
                nullrouter_events::UsageReader::new(UNREACHABLE_STATE_ADDR),
            ))
            .configure(configure),
    )
    .await;
    let mut builder = test::TestRequest::default().method(method).uri(uri);
    if let Some(accept) = accept {
        builder = builder.insert_header((header::ACCEPT, accept));
    }
    let response = test::call_service(&app, builder.to_request()).await;
    let status = response.status();
    let headers = response.headers().clone();
    // Bounded: `/api/usage/stream` is a live stream and never completes.
    let body = String::from_utf8(read_sse_prefix(response).await?)?;

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
        .get(name)
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

fn assert_not_html(response: &TestResponse) -> TestResult {
    let body = response.body.to_ascii_lowercase();
    for marker in ["<!doctype html", "<html", "<body", "</html>"] {
        assert!(
            !body.contains(marker),
            "response contained HTML marker {marker}"
        );
    }
    if let Some(content_type) = response.headers.get(header::CONTENT_TYPE) {
        assert!(!content_type.to_str()?.starts_with("text/html"));
    }
    Ok(())
}

fn parse_sse(body: &str) -> TestResult<Vec<SseFrame>> {
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

#[actix_web::test]
async fn g015_events_service_leaves_mitm_status_and_alias_to_api() -> TestResult {
    // Given: MITM status routes are owned by nullrouter-api, not nullrouter-events.
    let routes = [
        "/api/cli-tools/antigravity-mitm",
        "/api/cli-tools/antigravity-mitm/alias",
    ];

    // When: the events service receives direct requests for those paths.
    for route in routes {
        let response = request(Method::GET, route, None).await?;

        // Then: both paths are 404s without a framework HTML document.
        assert_eq!(response.status, StatusCode::NOT_FOUND, "{route}");
        assert_not_html(&response)?;
    }

    Ok(())
}

#[actix_web::test]
async fn g015_owned_streams_keep_sse_frames_onmessage_data_and_cors() -> TestResult {
    // Given: browsers connect to the two events-service streams.
    let usage = request(Method::GET, "/api/usage/stream", Some("text/event-stream")).await?;
    let translator = request(
        Method::GET,
        "/api/translator/console-logs/stream",
        Some("text/event-stream"),
    )
    .await?;

    // When: each response is decoded as an EventSource stream.
    let usage_frames = parse_sse(&usage.body)?;
    let translator_frames = parse_sse(&translator.body)?;

    // Then: usage keeps its established connected and default usage frames.
    assert_eq!(usage.status, StatusCode::OK);
    assert_eq!(
        header_value(&usage.headers, &header::CONTENT_TYPE)?,
        SSE_CONTENT_TYPE
    );
    assert_cors(&usage.headers)?;
    assert_eq!(
        usage_frames
            .iter()
            .map(|frame| frame.event.as_str())
            .collect::<Vec<_>>(),
        ["connected", "usage"]
    );
    let connected = frame_data(&usage_frames, "connected")?;
    assert_eq!(field(connected, "service")?, "nullrouter-events");
    assert_eq!(field(connected, "stream")?, "usage");
    assert_eq!(field(connected, "connected")?, true);
    let usage_data = frame_data(&usage_frames, "usage")?;
    assert_eq!(field(usage_data, "liveTelemetry")?, false);
    assert_eq!(field(usage_data, "activeRequests")?, 0);
    assert_eq!(field(usage_data, "requestsToday")?, 0);
    assert_eq!(field(usage_data, "tokensToday")?, 0);

    // Then: translator keeps named frames and a JSON `message` frame for onmessage.data.
    assert_eq!(translator.status, StatusCode::OK);
    assert_eq!(
        header_value(&translator.headers, &header::CONTENT_TYPE)?,
        SSE_CONTENT_TYPE
    );
    assert_cors(&translator.headers)?;
    assert_eq!(
        translator_frames
            .iter()
            .map(|frame| frame.event.as_str())
            .collect::<Vec<_>>(),
        ["connected", "console_logs", "message"]
    );
    let translator_connected = frame_data(&translator_frames, "connected")?;
    assert_eq!(field(translator_connected, "service")?, "nullrouter-events");
    assert_eq!(
        field(translator_connected, "stream")?,
        "translator.console_logs"
    );
    assert_eq!(field(translator_connected, "connected")?, true);
    let named_logs = frame_data(&translator_frames, "console_logs")?;
    let onmessage_data = frame_data(&translator_frames, "message")?;
    for data in [named_logs, onmessage_data] {
        assert_eq!(field(data, "type")?, "init");
        assert_eq!(field(data, "liveCapture")?, false);
        assert!(field(data, "logs")?.as_array().is_some_and(Vec::is_empty));
        assert!(data.get("live_capture").is_none());
    }

    Ok(())
}
