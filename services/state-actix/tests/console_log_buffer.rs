//! The console-log buffer over its real routes.
//!
//! What this suite exists to pin is the reason the buffer lives in this service at all: the list the
//! API service serves and the stream the events service serves must read the *same* lines. Both read
//! them through the routes exercised here, so a change that made ingest and read disagree — a cursor
//! off by one, a clear that reset numbering — would show up as a pane that duplicates or loses lines
//! rather than as a failing unit test.

#![allow(clippy::future_not_send)]
#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "indexing a serde_json::Value is the assertion: a shape that does not match is a test \
              failure, which is what the panic reports"
)]

use actix_web::{
    App,
    body::to_bytes,
    http::{Method, StatusCode, header},
    test, web,
};
use serde_json::{Value, json};

use nullrouter_state::{StateStore, configure};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const PATH: &str = "/internal/v1/console-logs";

async fn call(method: Method, uri: &str, body: &Value) -> TestResult<(StatusCode, Value)> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(StateStore::memory()))
            .configure(configure),
    )
    .await;
    let request = test::TestRequest::default()
        .method(method)
        .uri(uri)
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(serde_json::to_string(body)?)
        .to_request();
    let response = test::call_service(&app, request).await;
    let status = response.status();
    let bytes = to_bytes(response.into_body()).await?;
    Ok((status, serde_json::from_slice(&bytes)?))
}

/// One app instance, so a sequence of calls shares the buffer the way the real process does.
///
/// `call` above builds a fresh app per request, which is right for one-shot checks but would give
/// every step its own empty buffer — the exact bug this suite is meant to catch.
macro_rules! with_app {
    ($app:ident, $body:block) => {{
        let $app = test::init_service(
            App::new()
                .app_data(web::Data::new(StateStore::memory()))
                .configure(configure),
        )
        .await;
        $body
    }};
}

/// One request against a shared app instance.
///
/// A macro rather than a function because naming the request type `test::init_service` returns would
/// mean depending on `actix-http` directly for one signature.
macro_rules! send {
    ($app:expr, $method:expr, $uri:expr) => {
        send!($app, $method, $uri, None::<&Value>)
    };
    ($app:expr, $method:expr, $uri:expr, $body:expr) => {{
        let mut request = test::TestRequest::default().method($method).uri($uri);
        if let Some(body) = $body {
            request = request
                .insert_header((header::CONTENT_TYPE, "application/json"))
                .set_payload(serde_json::to_string(body)?);
        }
        let response = test::call_service(&$app, request.to_request()).await;
        let status = response.status();
        let bytes = to_bytes(response.into_body()).await?;
        (status, serde_json::from_slice::<Value>(&bytes)?)
    }};
}

fn batch(service: &str, messages: &[&str]) -> Value {
    json!({
        "service": service,
        "lines": messages
            .iter()
            .map(|message| json!({"level": "info", "message": message}))
            .collect::<Vec<Value>>(),
    })
}

#[actix_web::test]
async fn lines_from_several_services_land_in_one_buffer_in_arrival_order() -> TestResult {
    with_app!(app, {
        // Given: three services shipping their own lines, which is the whole point of putting the
        // buffer here rather than in whichever process happens to serve a route.
        for (service, messages) in [
            ("nullrouter-api", &["api one"][..]),
            ("nullrouter-runtime", &["runtime one", "runtime two"][..]),
            ("nullrouter-state", &["state one"][..]),
        ] {
            let (status, body) =
                send!(app, Method::POST, PATH, Some(&batch(service, messages)));
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["accepted"], messages.len());
        }

        // When: the buffer is read the way the API service's list does.
        let (status, page) = send!(app, Method::GET, PATH);
        assert_eq!(status, StatusCode::OK);

        // Then: all four lines are there, in the order they arrived, each naming its service.
        let logs = page["logs"].as_array().expect("logs");
        assert_eq!(logs.len(), 4, "{page}");
        assert_eq!(logs[0], "[nullrouter-api] info api one");
        assert_eq!(logs[1], "[nullrouter-runtime] info runtime one");
        assert_eq!(logs[3], "[nullrouter-state] info state one");
        Ok(())
    })
}

#[actix_web::test]
async fn a_cursor_read_returns_only_new_lines() -> TestResult {
    with_app!(app, {
        // Given: a buffer the stream has already read once.
        send!(app, Method::POST, PATH, Some(&batch("api", &["one", "two"])));
        let (_status, first) = send!(app, Method::GET, PATH);
        let cursor = first["cursor"].as_u64().expect("a cursor");

        // When: more arrives and the stream polls with the cursor it was given.
        send!(app, Method::POST, PATH, Some(&batch("api", &["three"])));
        let (_status, second) =
            send!(app, Method::GET, &format!("{PATH}?cursor={cursor}"));

        // Then: only the new line comes back. Re-sending the first two would make the pane show
        // every line twice per tick.
        let logs = second["logs"].as_array().expect("logs");
        assert_eq!(logs.len(), 1, "{second}");
        assert_eq!(logs[0], "[api] info three");
        assert_eq!(second["dropped"], false);

        // And polling again with nothing new returns nothing, with the cursor still usable.
        let cursor = second["cursor"].as_u64().expect("a cursor");
        let (_status, third) =
            send!(app, Method::GET, &format!("{PATH}?cursor={cursor}"));
        assert_eq!(third["logs"].as_array().map(Vec::len), Some(0), "{third}");
        assert_eq!(third["cursor"], cursor, "an empty poll must not move the cursor");
        Ok(())
    })
}

#[actix_web::test]
async fn a_clear_empties_the_buffer_and_is_visible_to_a_poller() -> TestResult {
    with_app!(app, {
        // Given: a buffer with lines in it, already read once.
        send!(app, Method::POST, PATH, Some(&batch("api", &["before"])));
        let (_status, before) = send!(app, Method::GET, PATH);
        let generation = before["generation"].as_u64().expect("a generation");
        let cursor = before["cursor"].as_u64().expect("a cursor");

        // When: the API service's DELETE runs.
        let (status, cleared) = send!(app, Method::DELETE, PATH);
        assert_eq!(status, StatusCode::OK, "{cleared}");
        assert_eq!(cleared["success"], true);

        // Then: it is empty, and the generation moved — which is how a poller holding a cursor tells
        // "cleared" from "nothing new", the two being indistinguishable by line count alone.
        let (_status, after) = send!(app, Method::GET, PATH);
        assert_eq!(after["logs"].as_array().map(Vec::len), Some(0), "{after}");
        assert_eq!(after["generation"], generation + 1);

        // And numbering continues past the clear, so a stale cursor is not handed lines it has seen.
        send!(app, Method::POST, PATH, Some(&batch("api", &["after"])));
        let (_status, page) =
            send!(app, Method::GET, &format!("{PATH}?cursor={cursor}"));
        assert_eq!(page["logs"].as_array().map(Vec::len), Some(1), "{page}");
        assert_eq!(page["logs"][0], "[api] info after");
        Ok(())
    })
}

#[actix_web::test]
async fn the_buffer_is_bounded_and_says_when_a_reader_fell_behind() -> TestResult {
    with_app!(app, {
        // Given: more lines than the buffer keeps.
        let many: Vec<String> = (0..260).map(|index| format!("line {index}")).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        send!(app, Method::POST, PATH, Some(&batch("api", &refs)));

        // When: the whole buffer is read.
        let (_status, page) = send!(app, Method::GET, PATH);

        // Then: it is capped at upstream's 200 lines, holding the newest.
        let logs = page["logs"].as_array().expect("logs");
        assert_eq!(logs.len(), 200, "{}", logs.len());
        assert_eq!(logs[199], "[api] info line 259");

        // And a cursor from before the eviction is told it missed lines, rather than being handed a
        // tail that reads as continuous with what it already had.
        let (_status, stale) = send!(app, Method::GET, &format!("{PATH}?cursor=1"));
        assert_eq!(stale["dropped"], true, "{stale}");
        Ok(())
    })
}

#[actix_web::test]
async fn a_malformed_batch_is_refused_without_disturbing_the_buffer() -> TestResult {
    with_app!(app, {
        send!(app, Method::POST, PATH, Some(&batch("api", &["kept"])));

        for bad in [
            json!({"lines": [{"level": "info", "message": "no service"}]}),
            json!({"service": "api"}),
            json!({"service": "api", "lines": "not an array"}),
            json!([]),
        ] {
            let (status, body) = send!(app, Method::POST, PATH, Some(&bad));
            assert_eq!(status, StatusCode::BAD_REQUEST, "{bad} -> {body}");
        }

        let (_status, page) = send!(app, Method::GET, PATH);
        assert_eq!(page["logs"].as_array().map(Vec::len), Some(1), "{page}");
        Ok(())
    })
}

#[actix_web::test]
async fn an_empty_buffer_reads_as_empty_rather_than_failing() -> TestResult {
    // The state a freshly started router is in. A 404 or a 500 here would make the pane show an
    // error on every startup.
    let (status, page) = call(Method::GET, PATH, &Value::Null).await?;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["logs"].as_array().map(Vec::len), Some(0), "{page}");
    assert_eq!(page["dropped"], false);
    Ok(())
}
