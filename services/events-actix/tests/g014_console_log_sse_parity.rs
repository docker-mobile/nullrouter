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
    let app = test::init_service(App::new().configure(configure)).await;
    let req = test::TestRequest::default()
        .method(method)
        .uri(uri)
        .insert_header((header::ACCEPT, "text/event-stream"))
        .to_request();
    let res = test::call_service(&app, req).await;
    let status = res.status();
    let headers = res.headers().clone();
    let body = read_opening_frames(res).await;

    Ok(TestResponse {
        status,
        headers,
        body,
    })
}

/// The first three frames of a response body, rather than all of it.
///
/// The console-log stream is live and never ends — it polls the buffer in the state service — so
/// reading to completion would hang. Three is the opening frame count this file asserts: `connected`,
/// the named `console_logs` init, and the unnamed duplicate an `onmessage` client reads. They are all
/// written into the stream's first yield, before any poll happens.
///
/// A non-streaming response yields once and then ends, so the loop terminates on `None` for the
/// other routes this helper serves.
async fn read_opening_frames(response: actix_web::dev::ServiceResponse) -> web::Bytes {
    use actix_web::body::MessageBody as _;

    const FRAMES: usize = 3;

    let mut collected: Vec<u8> = Vec::new();
    let mut body = response.into_body();
    let mut stream = std::pin::Pin::new(&mut body);
    while collected
        .windows(2)
        .filter(|window| *window == b"\n\n")
        .count()
        < FRAMES
    {
        match futures_util::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await {
            Some(Ok(chunk)) => collected.extend_from_slice(chunk.as_ref()),
            Some(Err(_)) | None => break,
        }
    }
    web::Bytes::from(collected)
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

        if data.is_empty() {
            return Err(test_error("missing SSE data field"));
        }

        let event = event.unwrap_or_else(|| "message".to_owned());
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

fn assert_init_console_logs_payload(json: &Value) -> TestResult {
    assert_eq!(field(json, "type")?, "init");
    assert!(array_field(json, "logs")?.is_empty());
    assert_eq!(field(json, "liveCapture")?, false);
    assert!(json.get("live_capture").is_none());
    Ok(())
}

#[actix_web::test]
async fn g014_console_logs_stream_supports_named_and_onmessage_init_frames() -> TestResult {
    // Given: no translator console log backend is connected.

    // When: the dashboard opens the translator console logs SSE route.
    let response = request(Method::GET, "/api/translator/console-logs/stream").await?;
    let frames = parse_sse(&response.body)?;

    // Then: prior named frames and the EventSource onmessage init frame are all parseable.
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

    assert_init_console_logs_payload(frame_data(&frames, "console_logs")?)?;
    assert_init_console_logs_payload(frame_data(&frames, "message")?)?;
    Ok(())
}

#[actix_web::test]
async fn g014_console_logs_options_preflight_remains_no_content() -> TestResult {
    // Given: browsers preflight the translator console logs stream route.

    // When: the route receives an OPTIONS request.
    let response = request(Method::OPTIONS, "/api/translator/console-logs/stream").await?;

    // Then: CORS remains a no-content preflight response.
    assert_eq!(response.status, StatusCode::NO_CONTENT);
    assert_cors(&response.headers)?;
    assert!(response.body.is_empty());
    Ok(())
}
