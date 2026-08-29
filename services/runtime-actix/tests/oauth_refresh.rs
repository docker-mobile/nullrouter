//! An expiring OAuth token is refreshed before the provider call.
//!
//! The refresh token was stored and never used, so an OAuth connection worked until
//! its access token expired and then failed until the user re-authorised by hand.
//! These drive the real pipeline against a loopback state service and assert on
//! what the provider received — the point is that the *new* token goes upstream and
//! that the rotation is persisted.

#![allow(
    clippy::future_not_send,
    clippy::expect_used,
    reason = "test helper: failing to bind a loopback socket should abort the test"
)]

use std::sync::{Arc, Mutex};

use actix_web::{
    App,
    body::to_bytes,
    http::{Method, StatusCode, header},
    test, web,
};
use nullrouter_runtime::{Runtime, app_config, configure};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Clone)]
struct Reply {
    status: u16,
    body: String,
}

impl Reply {
    fn json(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            body: body.into(),
        }
    }
}

/// One request the stub saw: path, body, and the `Authorization` header.
#[derive(Debug, Clone)]
struct Seen {
    path: String,
    authorization: String,
}

#[derive(Debug)]
struct FakeServer {
    addr: std::net::SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl FakeServer {
    async fn start(routes: Vec<(&'static str, Reply)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("addr");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&seen);
        actix_web::rt::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let routes = routes.clone();
                let recorded = Arc::clone(&recorded);
                actix_web::rt::spawn(async move {
                    serve(stream, routes, recorded).await;
                });
            }
        });
        Self { addr, seen }
    }

    fn addr_string(&self) -> String {
        self.addr.to_string()
    }

    fn requests(&self) -> Vec<Seen> {
        self.seen
            .lock()
            .map(|seen| seen.clone())
            .unwrap_or_default()
    }

    fn request_to(&self, needle: &str) -> Option<Seen> {
        self.requests()
            .into_iter()
            .find(|seen| seen.path.contains(needle))
    }
}

async fn serve(
    mut stream: TcpStream,
    routes: Vec<(&'static str, Reply)>,
    seen: Arc<Mutex<Vec<Seen>>>,
) {
    let mut buffer = vec![0_u8; 65536];
    let mut filled = 0;
    let (head_end, content_length) = loop {
        let Ok(read) = stream
            .read(buffer.get_mut(filled..).unwrap_or_default())
            .await
        else {
            return;
        };
        if read == 0 {
            return;
        }
        filled += read;
        let text = String::from_utf8_lossy(buffer.get(..filled).unwrap_or_default()).into_owned();
        if let Some(index) = text.find("\r\n\r\n") {
            let length = text
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())?
                })
                .unwrap_or(0);
            break (index + 4, length);
        }
    };
    while filled < head_end + content_length {
        let Ok(read) = stream
            .read(buffer.get_mut(filled..).unwrap_or_default())
            .await
        else {
            break;
        };
        if read == 0 {
            break;
        }
        filled += read;
    }

    let raw = String::from_utf8_lossy(buffer.get(..filled).unwrap_or_default()).into_owned();
    let head = raw.get(..head_end.min(raw.len())).unwrap_or_default();
    let path = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or_default()
        .to_owned();
    let authorization = head
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
        .map(|(_, value)| value.trim().to_owned())
        .unwrap_or_default();
    let body = raw.get(head_end..).unwrap_or_default().to_owned();
    if let Ok(mut sink) = seen.lock() {
        sink.push(Seen {
            path: path.clone(),
            authorization,
        });
    }

    let reply = routes
        .iter()
        .find(|(suffix, _)| path.contains(suffix))
        .map_or_else(
            || Reply {
                status: 404,
                body: String::from(r#"{"error":"not found"}"#),
            },
            |(_, reply)| reply.clone(),
        );
    let response = format!(
        "HTTP/1.1 {} OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        reply.status,
        reply.body.len(),
        reply.body
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.flush().await;
}

/// RFC3339 for `offset_ms` from now.
fn at_offset(offset_ms: i64) -> String {
    let now = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis())
            .unwrap_or(0),
    )
    .unwrap_or(0);
    let seconds = (now + offset_ms) / 1000;
    // Rendered the same way the refresh module does, via a round-trip through it.
    let days = seconds / 86_400;
    let time = seconds % 86_400;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        time / 3600,
        (time % 3600) / 60,
        time % 60
    )
}

/// State handing out an `anthropic-compatible-*` connection pointed at `provider`.
///
/// `anthropic-compatible-*` is used because its base URL comes from the connection,
/// so the provider call can be pointed at a loopback socket. The refresh endpoint
/// itself is the real one, which is why the refresh in these tests fails rather than
/// succeeding — what is asserted is which token went upstream and whether the
/// request was still served.
async fn state_with(
    expires_at: &str,
    refresh_token: Option<&str>,
    provider_base: &str,
) -> FakeServer {
    let mut credentials = json!({
        "connectionId": "conn_oauth",
        "connectionName": "oauth",
        "accessToken": "old-access-token",
        "expiresAt": expires_at,
        "providerSpecificData": { "baseUrl": provider_base },
    });
    if let Some(token) = refresh_token
        && let Some(object) = credentials.as_object_mut()
    {
        object.insert("refreshToken".to_owned(), json!(token));
    }
    FakeServer::start(vec![
        (
            "/internal/v1/credentials/select",
            Reply::json(json!({ "status": "selected", "credentials": credentials }).to_string()),
        ),
        ("/internal/v1/credentials/clear-error", Reply::json("{}")),
        ("/internal/v1/credentials/unavailable", Reply::json("{}")),
        (
            "/internal/v1/credentials/refresh",
            Reply::json(r#"{"ok":true}"#),
        ),
        ("/internal/v1/usage", Reply::json(r#"{"ok":true}"#)),
        (
            "/internal/v1/routing-context",
            Reply::json(r#"{"combos":[],"connections":[],"settings":{}}"#),
        ),
    ])
    .await
}

async fn claude_provider() -> FakeServer {
    FakeServer::start(vec![(
        "/messages",
        Reply::json(
            json!({
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "model": "claude-sonnet-4-5",
                "content": [{ "type": "text", "text": "pong" }],
                "stop_reason": "end_turn",
                "usage": { "input_tokens": 2, "output_tokens": 1 },
            })
            .to_string(),
        ),
    )])
    .await
}

async fn post(state_addr: &str, body: &str) -> TestResult<(StatusCode, Value)> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(Runtime::with_state_addr(state_addr)))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::default()
        .method(Method::POST)
        .uri("/v1/chat/completions")
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(body.to_owned())
        .to_request();
    let res = test::call_service(&app, req).await;
    let status = res.status();
    let raw = String::from_utf8(to_bytes(res.into_body()).await?.to_vec())?;
    Ok((
        status,
        serde_json::from_str(&raw).unwrap_or(Value::String(raw)),
    ))
}

const REQUEST: &str = r#"{"model":"anthropic-compatible-oauth/claude-sonnet-4-5","stream":false,"messages":[{"role":"user","content":"ping"}]}"#;

#[actix_rt::test]
async fn a_token_far_from_expiry_is_used_as_is() -> TestResult {
    let provider = claude_provider().await;
    let state = state_with(
        &at_offset(24 * 60 * 60 * 1000),
        Some("refresh-1"),
        &format!("http://{}", provider.addr),
    )
    .await;

    let (status, body) = post(&state.addr_string(), REQUEST).await?;
    assert_eq!(status, StatusCode::OK, "{body}");

    // The existing token went upstream unchanged.
    let sent = provider
        .request_to("/messages")
        .expect("provider was called");
    assert!(
        sent.authorization.contains("old-access-token"),
        "got {}",
        sent.authorization
    );
    // And nothing was persisted, because nothing was refreshed.
    assert!(
        state
            .request_to("/internal/v1/credentials/refresh")
            .is_none(),
        "an unexpired token must not be refreshed"
    );
    Ok(())
}

#[actix_rt::test]
async fn a_connection_with_no_refresh_token_is_still_served() -> TestResult {
    // An API-key connection has nothing to refresh, and an expiring one with no
    // refresh token cannot be helped — either way the request must still go out.
    let provider = claude_provider().await;
    let state = state_with(
        &at_offset(-60 * 1000),
        None,
        &format!("http://{}", provider.addr),
    )
    .await;

    let (status, body) = post(&state.addr_string(), REQUEST).await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(provider.request_to("/messages").is_some());
    assert!(
        state
            .request_to("/internal/v1/credentials/refresh")
            .is_none()
    );
    Ok(())
}

#[actix_rt::test]
async fn an_expiring_token_on_a_provider_with_no_grant_is_used_as_is() -> TestResult {
    // `anthropic-compatible-*` connections are configured with a base URL rather
    // than an OAuth client, so there is no grant to send even when a refresh token
    // is present and the access token is about to expire. The request must still be
    // served with the token it has.
    let provider = claude_provider().await;
    let state = state_with(
        &at_offset(30 * 1000),
        Some("refresh-1"),
        &format!("http://{}", provider.addr),
    )
    .await;

    let (status, body) = post(&state.addr_string(), REQUEST).await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    let sent = provider
        .request_to("/messages")
        .expect("provider was called");
    assert!(
        sent.authorization.contains("old-access-token"),
        "got {}",
        sent.authorization
    );
    // No grant attempted, so nothing to persist and no cooldown written.
    assert!(
        state
            .request_to("/internal/v1/credentials/refresh")
            .is_none()
    );
    assert!(
        state
            .request_to("/internal/v1/credentials/unavailable")
            .is_none(),
        "a provider that cannot be refreshed must not be locked out for it"
    );
    assert_eq!(
        body.pointer("/choices/0/message/content"),
        Some(&json!("pong")),
        "{body}"
    );
    Ok(())
}
