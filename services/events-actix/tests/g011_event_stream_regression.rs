#![allow(clippy::future_not_send)]

use actix_web::{
    App,
    http::{
        StatusCode,
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

const USAGE_EVENTS: &[&str] = &["connected", "usage"];
const TRANSLATOR_EVENTS: &[&str] = &["connected", "console_logs"];
const MCP_EVENTS: &[&str] = &["endpoint", "connected", "backend_state"];

#[derive(Debug)]
struct StreamResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: String,
}

#[derive(Debug, Clone, Copy)]
struct G011StreamProbe<'a> {
    uri: &'a str,
    expected_events: &'a [&'a str],
}

async fn get_stream(uri: &str) -> TestResult<StreamResponse> {
    let app = test::init_service(
        App::new()
            .app_data(actix_web::web::Data::new(
                nullrouter_events::UsageReader::new(UNREACHABLE_STATE_ADDR),
            ))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::get()
        .uri(uri)
        .insert_header((header::ACCEPT, "text/event-stream"))
        .to_request();
    let res = test::call_service(&app, req).await;
    let status = res.status();
    let headers = res.headers().clone();
    let body = String::from_utf8(read_sse_prefix(res).await?)?;

    Ok(StreamResponse {
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
        "G011 event-stream response contained an HTML doctype: {body}"
    );
    assert!(
        !normalized.contains("<html"),
        "G011 event-stream response contained an HTML document tag: {body}"
    );
}

#[actix_web::test]
async fn g011_event_stream_routes_remain_parseable_when_gateway_forwards_them() -> TestResult {
    // Given: the gateway forwards event-stream route paths to the events service.
    let probes = [
        G011StreamProbe {
            uri: "/api/usage/stream",
            expected_events: USAGE_EVENTS,
        },
        G011StreamProbe {
            uri: "/api/translator/console-logs/stream",
            expected_events: TRANSLATOR_EVENTS,
        },
        G011StreamProbe {
            uri: "/api/mcp/test/sse",
            expected_events: MCP_EVENTS,
        },
    ];

    // When: each existing events surface is requested with an SSE Accept header.
    for probe in probes {
        let response = get_stream(probe.uri).await?;
        let events = parse_event_names(&response.body)?;

        // Then: the route stays parseable as event-stream data instead of HTML or JSON fallback.
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(
            header_value(&response.headers, &header::CONTENT_TYPE)?,
            "text/event-stream; charset=utf-8"
        );
        assert_markup_free_event_stream(&response.body);
        for expected_event in probe.expected_events {
            assert!(
                events.iter().any(|event| event == expected_event),
                "G011 missing SSE event {expected_event} in {}: {events:?}",
                probe.uri
            );
        }
    }

    Ok(())
}
