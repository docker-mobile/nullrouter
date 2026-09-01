//! `POST /api/oauth/gitlab/pat` against a stub GitLab and a stub state service.
//!
//! This route is one of the ten under `/api/oauth/` that need no consent screen: the user pastes a
//! Personal Access Token they already hold, and the route verifies it and records a connection. The
//! suite covers the two things that distinguish a working import from one that merely looks like it —
//! that a token is verified before anything is stored, and that it never comes back in a response.

#![allow(clippy::future_not_send)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "free helpers here are not #[test] fns, so clippy.toml's allow-expect-in-tests does \
              not cover them, and indexing a Value is the assertion"
)]

use std::sync::{Arc, Mutex};

use actix_web::{
    App,
    body::to_bytes,
    http::{Method, StatusCode, header},
    test, web,
};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;

use nullrouter_api::{AppConfig, RuntimeClient, StateClient, configure};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A stub HTTP server answering with one canned response, recording what it received.
async fn stub(status: u16, body: String) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind loopback");
    let addr = listener.local_addr().expect("addr").to_string();
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&seen);

    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let body = body.clone();
            let recorded = Arc::clone(&recorded);
            tokio::spawn(async move {
                let mut buffer = vec![0_u8; 16384];
                let read = stream.read(&mut buffer).await.unwrap_or(0);
                recorded
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(String::from_utf8_lossy(buffer.get(..read).unwrap_or_default()).into_owned());
                let response = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                     Connection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });

    (addr, seen)
}

async fn post(state_addr: &str, body: &Value) -> TestResult<(StatusCode, Value)> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(AppConfig::new("0.5.20")))
            .app_data(web::Data::new(StateClient::new(state_addr)))
            .app_data(web::Data::new(RuntimeClient::new("127.0.0.1:1")))
            .configure(configure),
    )
    .await;
    let request = test::TestRequest::default()
        .method(Method::POST)
        .uri("/api/oauth/gitlab/pat")
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(serde_json::to_string(body)?)
        .to_request();
    let response = test::call_service(&app, request).await;
    let status = response.status();
    let bytes = to_bytes(response.into_body()).await?;
    Ok((status, serde_json::from_slice(&bytes)?))
}

/// Points GitLab's default base at the stub for the duration of a case.
///
/// The default only. A base named in a request still goes through the https and non-local checks,
/// which the refusal case below asserts.
struct GitlabBase {
    _lock: std::sync::MutexGuard<'static, ()>,
    previous: Option<std::ffi::OsString>,
}

fn env_lock() -> &'static Mutex<()> {
    static LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

impl GitlabBase {
    fn new(value: &str) -> Self {
        let lock = env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = std::env::var_os("NULLROUTER_GITLAB_BASE");
        // SAFETY: the lock is held, so no other case in this binary reads or writes it here.
        unsafe { std::env::set_var("NULLROUTER_GITLAB_BASE", value) };
        Self {
            _lock: lock,
            previous,
        }
    }
}

impl Drop for GitlabBase {
    fn drop(&mut self) {
        match self.previous.take() {
            // SAFETY: the lock is still held until this guard finishes dropping.
            Some(value) => unsafe { std::env::set_var("NULLROUTER_GITLAB_BASE", value) },
            // SAFETY: as above.
            None => unsafe { std::env::remove_var("NULLROUTER_GITLAB_BASE") },
        }
    }
}

#[actix_web::test]
async fn a_verified_token_is_recorded_and_never_returned() -> TestResult {
    // Given: a GitLab that accepts the token, and a state service that accepts the connection.
    let (gitlab_addr, gitlab_seen) = stub(
        200,
        json!({
            "username": "someone",
            "name": "Some One",
            "email": "someone@example.com",
        })
        .to_string(),
    )
    .await;
    let (state_addr, state_seen) = stub(201, json!({"connection": {"id": "conn_1"}}).to_string()).await;
    let _base = GitlabBase::new(&format!("http://{gitlab_addr}"));

    // When: a PAT is imported.
    let (status, body) = post(&state_addr, &json!({"token": "glpat-secret"})).await?;

    // Then: it succeeds, and the response carries nothing but that.
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["success"], true, "{body}");
    assert!(
        !serde_json::to_string(&body)?.contains("glpat-secret"),
        "the token must not come back: a dashboard would put it in a browser history"
    );

    // The token went to GitLab in `Private-Token`, which is the header GitLab accepts a PAT in — a
    // bearer header there is rejected, so the two are not interchangeable.
    let gitlab_requests = gitlab_seen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(gitlab_requests.len(), 1, "exactly one verification call");
    let sent = &gitlab_requests[0];
    assert!(sent.starts_with("GET /api/v4/user"), "{sent}");
    assert!(
        sent.to_lowercase().contains("private-token: glpat-secret"),
        "the PAT header is missing: {sent}"
    );

    // And the connection recorded carries the identity read back from GitLab, so the dashboard shows
    // whose account it is rather than a bare "gitlab".
    let recorded = state_seen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(recorded.len(), 1, "exactly one connection recorded");
    let stored = &recorded[0];
    assert!(stored.starts_with("POST /api/providers"), "{stored}");
    assert!(stored.contains("\"name\":\"Some One\""), "{stored}");
    assert!(stored.contains("someone@example.com"), "{stored}");
    // `authKind` is what tells the refresh path to leave this connection alone: a PAT has no refresh
    // token, so trying to refresh one would fail on every request.
    assert!(stored.contains("personal_access_token"), "{stored}");
    Ok(())
}

#[actix_web::test]
async fn a_token_gitlab_rejects_is_not_recorded() -> TestResult {
    // Given: a GitLab that refuses the token.
    let (gitlab_addr, _gitlab_seen) = stub(401, json!({"message": "401 Unauthorized"}).to_string()).await;
    let (state_addr, state_seen) = stub(201, json!({}).to_string()).await;
    let _base = GitlabBase::new(&format!("http://{gitlab_addr}"));

    // When: the import runs.
    let (status, body) = post(&state_addr, &json!({"token": "glpat-wrong"})).await?;

    // Then: nothing is stored. Recording an unverified token produces a connection that fails on
    // first real use, by which point the cause is several steps away.
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|error| error.contains("verification failed")),
        "{body}"
    );
    assert!(
        state_seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty(),
        "a rejected token must not be recorded"
    );
    Ok(())
}

#[actix_web::test]
async fn a_missing_token_is_refused_before_any_call() -> TestResult {
    let (state_addr, state_seen) = stub(201, json!({}).to_string()).await;

    for body in [json!({}), json!({"token": ""}), json!({"token": "   "})] {
        let (status, response) = post(&state_addr, &body).await?;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body} -> {response}");
        assert_eq!(response["error"], "Personal Access Token is required");
    }
    assert!(
        state_seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty(),
        "a refused request must not reach state"
    );
    Ok(())
}

#[actix_web::test]
async fn a_base_url_that_would_disclose_or_pivot_is_refused_before_the_token_is_sent() -> TestResult {
    // The caller names this host and a credential is sent to it. Each of these is either a way to
    // put a token on the wire in clear, or a way to make this service reach something the caller
    // cannot — the internal services on 20129-20135 among them.
    let (state_addr, state_seen) = stub(201, json!({}).to_string()).await;

    for base in [
        "http://gitlab.example.com",
        "http://127.0.0.1:20134",
        "https://localhost/gitlab",
        "https://10.0.0.5",
        "https://[::1]",
        "ftp://gitlab.example.com",
        "not a url",
    ] {
        let (status, response) = post(&state_addr, &json!({"token": "glpat-x", "baseUrl": base})).await?;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{base} -> {response}");
        assert_eq!(response["success"], false, "{base}");
        assert!(
            !serde_json::to_string(&response)?.contains("glpat-x"),
            "{base}: the token leaked into the refusal"
        );
    }
    assert!(
        state_seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty(),
        "nothing should have been recorded"
    );
    Ok(())
}

#[actix_web::test]
async fn the_rest_of_the_oauth_family_still_answers_a_stated_501() -> TestResult {
    // The implemented route is registered before the catch-all, so this checks the catch-all still
    // has everything else — a routing order mistake would turn a 501 into a 404 or swallow the
    // implemented one.
    let (state_addr, _seen) = stub(201, json!({}).to_string()).await;
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(AppConfig::new("0.5.20")))
            .app_data(web::Data::new(StateClient::new(&state_addr)))
            .app_data(web::Data::new(RuntimeClient::new("127.0.0.1:1")))
            .configure(configure),
    )
    .await;

    for (path, provider, action) in [
        ("/api/oauth/kiro/social-authorize", "kiro", "social-authorize"),
        ("/api/oauth/codex/import-token", "codex", "import-token"),
        ("/api/oauth/gitlab/other", "gitlab", "other"),
    ] {
        let request = test::TestRequest::default()
            .method(Method::POST)
            .uri(path)
            .insert_header((header::CONTENT_TYPE, "application/json"))
            .set_payload("{}")
            .to_request();
        let response = test::call_service(&app, request).await;
        let status = response.status();
        let bytes = to_bytes(response.into_body()).await?;
        let body: Value = serde_json::from_slice(&bytes)?;

        assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{path}");
        assert_eq!(body["unsupported"], true, "{path}");
        // The provider and action are named, so a user reads which thing is unported rather than a
        // bare "not supported".
        assert_eq!(body["provider"], provider, "{path}");
        assert_eq!(body["action"], action, "{path}");
    }
    Ok(())
}
