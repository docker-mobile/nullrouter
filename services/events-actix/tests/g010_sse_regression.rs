#![allow(clippy::future_not_send)]

use actix_web::{
    App,
    http::{
        StatusCode,
        header::{self, HeaderValue},
    },
    test,
};
use nullrouter_events::configure;
use serde_json::Value;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[derive(Debug)]
struct TextResponse {
    status: StatusCode,
    content_type: String,
    body: String,
}

#[derive(Debug, Clone, Copy)]
struct StreamProbe<'a> {
    uri: &'a str,
    expected_event: &'a str,
}

/// A closed loopback port, so the usage stream emits its offline frame.
const UNREACHABLE_STATE_ADDR: &str = "127.0.0.1:1";

/// Read a bounded prefix of a response body.
///
/// `/api/usage/stream` is a live stream that never ends, so reading to
/// completion would hang; two frames are enough to assert the event shape.
async fn get_text(uri: &str) -> TestResult<TextResponse> {
    use actix_web::body::MessageBody;

    let app = test::init_service(
        App::new()
            .app_data(actix_web::web::Data::new(
                nullrouter_events::UsageReader::new(UNREACHABLE_STATE_ADDR),
            ))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::get().uri(uri).to_request();
    let res = test::call_service(&app, req).await;
    let status = res.status();
    let content_type = content_type(res.headers().get(header::CONTENT_TYPE))?;

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
    let body = String::from_utf8(collected)?;

    Ok(TextResponse {
        status,
        content_type,
        body,
    })
}

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}

fn content_type(value: Option<&HeaderValue>) -> TestResult<String> {
    value
        .ok_or_else(|| test_error("missing content-type header"))?
        .to_str()
        .map(str::to_owned)
        .map_err(|error| test_error(format!("invalid content-type header: {error}")))
}

fn parse_event_names(body: &str) -> TestResult<Vec<String>> {
    let mut events = Vec::new();

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
        serde_json::from_str::<Value>(&data)
            .map_err(|error| test_error(format!("invalid SSE JSON data for {event}: {error}")))?;
        events.push(event);
    }

    Ok(events)
}

fn assert_markup_free_event_stream(body: &str) {
    let normalized = body.to_ascii_lowercase();

    assert!(
        !normalized.contains("<!doctype html"),
        "event-stream response contained an HTML doctype: {body}"
    );
    assert!(
        !normalized.contains("<html"),
        "event-stream response contained an HTML document tag: {body}"
    );
}

#[actix_web::test]
async fn usage_and_translator_streams_remain_parseable_event_streams_after_g010() -> TestResult {
    // Given: media-provider pages use usage logs while stream regressions keep SSE compatibility.
    let probes = [
        StreamProbe {
            uri: "/api/usage/stream",
            expected_event: "usage",
        },
        StreamProbe {
            uri: "/api/translator/console-logs/stream",
            expected_event: "console_logs",
        },
    ];

    // When: each browser-facing SSE route is requested.
    for probe in probes {
        let response = get_text(probe.uri).await?;
        let events = parse_event_names(&response.body)?;

        // Then: each route stays parseable as event-stream data and does not fall through to HTML.
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.content_type, "text/event-stream; charset=utf-8");
        assert_markup_free_event_stream(&response.body);
        assert!(events.iter().any(|event| event == "connected"));
        assert!(events.iter().any(|event| event == probe.expected_event));
    }

    Ok(())
}
