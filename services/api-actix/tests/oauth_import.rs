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
    post_to("/api/oauth/gitlab/pat", state_addr, body).await
}

async fn post_to(uri: &str, state_addr: &str, body: &Value) -> TestResult<(StatusCode, Value)> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(AppConfig::new("0.5.20")))
            .app_data(web::Data::new(StateClient::new(state_addr)))
            .app_data(web::Data::new(RuntimeClient::new("127.0.0.1:1")))
            .app_data(web::Data::new(nullrouter_api::TunnelManager::new()))
            .configure(configure),
    )
    .await;
    let request = test::TestRequest::default()
        .method(Method::POST)
        .uri(uri)
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

    // `codex/import-token` used to be listed here and is now implemented, so what remains is the two
    // families that genuinely need a consent screen, plus the catch-all for an action that does not
    // exist at all.
    for (path, provider, action) in [
        ("/api/oauth/kiro/social-authorize", "kiro", "social-authorize"),
        ("/api/oauth/kiro/social-exchange", "kiro", "social-exchange"),
        ("/api/oauth/cursor/import", "cursor", "import"),
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

/// Points the Amazon Q base at a stub for the duration of a case.
///
/// The base only. A region named in a request is still pattern-checked, which the refusal case below
/// asserts — so this does not relax the check that keeps a caller from choosing the host.
struct KiroBase {
    _lock: std::sync::MutexGuard<'static, ()>,
    previous: Option<std::ffi::OsString>,
}

impl KiroBase {
    fn pointing_at(addr: &str) -> Self {
        let lock = env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = std::env::var_os("NULLROUTER_KIRO_Q_BASE");
        // SAFETY: the lock above is held, so no other case in this binary reads or writes this
        // variable while it is being set.
        unsafe { std::env::set_var("NULLROUTER_KIRO_Q_BASE", format!("http://{addr}")) };
        Self {
            _lock: lock,
            previous,
        }
    }
}

impl Drop for KiroBase {
    fn drop(&mut self) {
        match &self.previous {
            // SAFETY: the lock is still held until this guard finishes dropping.
            Some(previous) => unsafe { std::env::set_var("NULLROUTER_KIRO_Q_BASE", previous) },
            // SAFETY: as above.
            None => unsafe { std::env::remove_var("NULLROUTER_KIRO_Q_BASE") },
        }
    }
}

/// A `ListAvailableModels` answer with one model, which is what a usable key returns.
fn models_body() -> String {
    json!({ "models": [{ "modelId": "claude-sonnet-4" }] }).to_string()
}

#[actix_rt::test]
async fn a_verified_kiro_api_key_is_recorded_and_never_returned() -> TestResult {
    // Given: Amazon Q accepts the key and lists a model, and the state service accepts the write.
    let (amazon, amazon_seen) = stub(200, models_body()).await;
    let _base = KiroBase::pointing_at(&amazon);
    let (state, state_seen) = stub(200, json!({ "id": "conn-kiro" }).to_string()).await;

    // When: the key is imported.
    let (status, body) = post_to(
        "/api/oauth/kiro/api-key",
        &state,
        &json!({ "apiKey": "KIRO-KEY-SENTINEL-abc123" }),
    )
    .await?;

    // Then: it succeeds, and the key is not in the response — it is stored, and it went to AWS;
    // returning it would put a long-lived credential into a browser history.
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.get("success"), Some(&Value::Bool(true)));
    assert!(
        !body.to_string().contains("KIRO-KEY-SENTINEL"),
        "the key was echoed back: {body}"
    );

    // And the verification really happened, with the header that makes Amazon Q read the credential
    // as an API key rather than an SSO token.
    let asked = amazon_seen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .join("\n");
    assert!(asked.contains("ListAvailableModels"), "{asked}");
    assert!(asked.contains("origin=AI_EDITOR"), "{asked}");
    // Lowercased, because that is how it goes on the wire.
    assert!(
        asked.to_ascii_lowercase().contains("tokentype: api_key"),
        "{asked}"
    );
    assert!(asked.contains("Bearer KIRO-KEY-SENTINEL-abc123"), "{asked}");

    // And the connection was recorded with the fields the refresh path reads.
    let written = state_seen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .join("\n");
    assert!(written.contains("\"provider\":\"kiro\""), "{written}");
    assert!(written.contains("\"authType\":\"api_key\""), "{written}");
    // authMethod is what stops the proactive refresh from selecting a credential with no refresh
    // token — trying to refresh one fails on every request.
    assert!(written.contains("\"authMethod\":\"api_key\""), "{written}");
    assert!(written.contains("\"refreshToken\":null"), "{written}");
    Ok(())
}

#[actix_rt::test]
async fn a_key_that_lists_no_models_is_refused() -> TestResult {
    // Given: Amazon Q answers 200 with an empty list. This is what a key scoped to nothing returns,
    // and treating reachable as usable would record a connection that fails on its first request.
    let (amazon, _seen) = stub(200, json!({ "models": [] }).to_string()).await;
    let _base = KiroBase::pointing_at(&amazon);
    let (state, state_seen) = stub(200, json!({ "id": "conn" }).to_string()).await;

    // When: the key is imported.
    let (status, body) = post_to(
        "/api/oauth/kiro/api-key",
        &state,
        &json!({ "apiKey": "scoped-to-nothing" }),
    )
    .await?;

    // Then: refused, and nothing was recorded.
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert!(
        body.get("error")
            .and_then(Value::as_str)
            .is_some_and(|error| error.contains("no available models")),
        "{body}"
    );
    assert!(
        state_seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty(),
        "a rejected key must not reach the state service"
    );
    Ok(())
}

#[actix_rt::test]
async fn a_key_amazon_rejects_is_not_recorded_and_its_body_is_not_reflected() -> TestResult {
    // Given: Amazon Q rejects the key, and its error document quotes the credential back — which is
    // exactly why upstream drops the body here.
    let (amazon, _seen) = stub(
        403,
        json!({ "message": "The token KIRO-KEY-SENTINEL-abc123 is not authorized" }).to_string(),
    )
    .await;
    let _base = KiroBase::pointing_at(&amazon);
    let (state, state_seen) = stub(200, json!({ "id": "conn" }).to_string()).await;

    // When: the key is imported.
    let (status, body) = post_to(
        "/api/oauth/kiro/api-key",
        &state,
        &json!({ "apiKey": "KIRO-KEY-SENTINEL-abc123" }),
    )
    .await?;

    // Then: refused, with the status kept and the body dropped.
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    let error = body.get("error").and_then(Value::as_str).ok_or("no error")?;
    assert!(error.contains("403"), "the status is useful: {error}");
    assert!(
        !error.contains("KIRO-KEY-SENTINEL"),
        "the provider body leaked the credential back: {error}"
    );
    assert!(
        state_seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty(),
        "a rejected key must not reach the state service"
    );
    Ok(())
}

#[actix_rt::test]
async fn a_region_that_would_choose_the_host_is_refused_before_the_key_is_sent() -> TestResult {
    // Given: the region becomes the first label of `q.<region>.amazonaws.com`, so an unchecked value
    // is a way to pick which host receives the caller's bearer token.
    let (state, state_seen) = stub(200, json!({ "id": "conn" }).to_string()).await;

    for hostile in [
        "us-east-1.evil.example.com",
        "../../evil",
        "evil",
        "US-EAST-1",
        "us-east-1x",
        "us-east-123",
        "a-b-1",
        "us--1",
        "us-east-1/x",
        "us-east-1?x=y",
        "us-east-1#x",
        "us-east-1:8080",
        "",
    ] {
        // When: it is supplied.
        let (status, body) = post_to(
            "/api/oauth/kiro/api-key",
            &state,
            &json!({ "apiKey": "some-key", "region": hostile }),
        )
        .await?;

        // Then: an empty region falls back to the default and gets as far as the network; every
        // other shape is refused outright.
        if hostile.is_empty() {
            assert_ne!(status, StatusCode::OK, "{body}");
            continue;
        }
        assert_eq!(status, StatusCode::BAD_REQUEST, "{hostile:?}: {body}");
        assert_eq!(
            body.get("error").and_then(Value::as_str),
            Some("Invalid region"),
            "{hostile:?}: {body}"
        );
    }

    assert!(
        state_seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty(),
        "no refused region may reach the state service"
    );
    Ok(())
}

#[actix_rt::test]
async fn a_real_aws_region_shape_is_accepted() -> TestResult {
    // The other half: the check must not be so tight that real regions fail.
    let (amazon, _seen) = stub(200, models_body()).await;
    let _base = KiroBase::pointing_at(&amazon);
    let (state, _state_seen) = stub(200, json!({ "id": "conn" }).to_string()).await;

    for region in [
        "us-east-1",
        "eu-west-2",
        "ap-southeast-1",
        "ap-northeast-3",
        "me-central-1",
    ] {
        let (status, body) = post_to(
            "/api/oauth/kiro/api-key",
            &state,
            &json!({ "apiKey": "some-key", "region": region }),
        )
        .await?;

        assert_eq!(status, StatusCode::OK, "{region:?} was refused: {body}");
    }

    // A four-segment region is refused, and that is faithful rather than a gap here: upstream's own
    // AWS_REGION_PATTERN is `^[a-z]{2}-[a-z]+-\d{1,2}$`, so `us-gov-west-1` and `us-iso-east-1` fail
    // its check too. GovCloud is out of reach in 9Router for the same reason it is out of reach here,
    // and loosening the pattern unilaterally would widen a check whose whole job is narrowness.
    let (gov_status, gov_body) = post_to(
        "/api/oauth/kiro/api-key",
        &state,
        &json!({ "apiKey": "some-key", "region": "us-gov-west-1" }),
    )
    .await?;
    assert_eq!(gov_status, StatusCode::BAD_REQUEST, "{gov_body}");
    Ok(())
}

#[actix_rt::test]
async fn a_missing_kiro_key_is_refused_before_any_call() -> TestResult {
    let (state, state_seen) = stub(200, json!({ "id": "conn" }).to_string()).await;

    for body in [json!({}), json!({ "apiKey": "" }), json!({ "apiKey": "   " })] {
        let (status, response) = post_to("/api/oauth/kiro/api-key", &state, &body).await?;

        assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
        assert_eq!(
            response.get("error").and_then(Value::as_str),
            Some("API key is required")
        );
    }
    assert!(
        state_seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    );
    Ok(())
}

/// A `ChatGPT` access token carrying the namespaced claims `OpenAI` issues.
const CODEX_JWT: &str = "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.eyJodHRwczovL2FwaS5vcGVuYWkuY29tL3Byb2ZpbGUiOnsiZW1haWwiOiJwZXJzb25AZXhhbXBsZS5jb20ifSwiaHR0cHM6Ly9hcGkub3BlbmFpLmNvbS9hdXRoIjp7ImNoYXRncHRfYWNjb3VudF9pZCI6ImFjY3QtOWY4ZSIsImNoYXRncHRfcGxhbl90eXBlIjoicGx1cyJ9LCJleHAiOjIwMDAwMDAwMDB9.sig";

/// One carrying only the top-level fallbacks.
const CODEX_JWT_TOPLEVEL: &str = "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.eyJwcmVmZXJyZWRfdXNlcm5hbWUiOiJmYWxsYmFja0BleGFtcGxlLmNvbSIsImFjY291bnRfaWQiOiJhY2N0LXRvcCIsInBsYW5fdHlwZSI6InRlYW0ifQ.sig";

#[actix_rt::test]
async fn a_codex_access_token_is_recorded_with_the_claims_it_carries() -> TestResult {
    // Given: a ChatGPT access token. There is nothing to verify it against — it is issued for the
    // ChatGPT surface, not for an API endpoint that would accept it in a probe — so the route reads
    // what the token says about itself and records it, as upstream does.
    let (state, seen) = stub(200, json!({ "id": "conn-codex" }).to_string()).await;

    // When: it is imported.
    let (status, body) = post_to(
        "/api/oauth/codex/import-token",
        &state,
        &json!({ "accessToken": CODEX_JWT }),
    )
    .await?;

    // Then: the namespaced claims are read for the labels the panel shows.
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.get("success"), Some(&Value::Bool(true)));
    let connection = body.get("connection").ok_or("no connection")?;
    assert_eq!(
        connection.get("email").and_then(Value::as_str),
        Some("person@example.com"),
        "{body}"
    );
    assert_eq!(
        connection.get("workspace").and_then(Value::as_str),
        Some("acct-9f8e"),
        "{body}"
    );
    assert_eq!(
        connection.get("plan").and_then(Value::as_str),
        Some("plus"),
        "{body}"
    );

    // And the token itself is not in the response, only in the record.
    assert!(!body.to_string().contains(CODEX_JWT), "{body}");
    let written = seen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .join("\n");
    assert!(written.contains(r#""authType":"access_token""#), "{written}");
    assert!(written.contains(r#""authMethod":"access_token""#), "{written}");
    assert!(written.contains("chatgptPlanType"), "{written}");
    // The expiry the token states is kept as a label, not as an authority: a token with no refresh
    // path is never proactively refreshed, so this is for display.
    assert!(written.contains("jwtExp"), "{written}");
    Ok(())
}

#[actix_rt::test]
async fn top_level_codex_claims_are_used_when_the_namespaced_ones_are_absent() -> TestResult {
    // A token from a device flow carries different keys than one from the web UI, and the panel needs
    // a label either way.
    let (state, _seen) = stub(200, json!({ "id": "conn" }).to_string()).await;

    let (status, body) = post_to(
        "/api/oauth/codex/import-token",
        &state,
        &json!({ "accessToken": CODEX_JWT_TOPLEVEL }),
    )
    .await?;

    assert_eq!(status, StatusCode::OK, "{body}");
    let connection = body.get("connection").ok_or("no connection")?;
    assert_eq!(
        connection.get("email").and_then(Value::as_str),
        Some("fallback@example.com"),
        "{body}"
    );
    assert_eq!(
        connection.get("workspace").and_then(Value::as_str),
        Some("acct-top"),
        "{body}"
    );
    assert_eq!(
        connection.get("plan").and_then(Value::as_str),
        Some("team"),
        "{body}"
    );
    Ok(())
}

#[actix_rt::test]
async fn a_codex_token_that_is_not_a_jwt_is_still_imported() -> TestResult {
    // Refusing an opaque token because it carries no readable metadata would reject a working
    // credential over a missing label. Upstream imports it; so does this.
    let (state, seen) = stub(200, json!({ "id": "conn" }).to_string()).await;

    for opaque in ["sk-not-a-jwt-at-all", "a.b", "a.b.c.d", "...", "%%%.%%%.%%%"] {
        let (status, body) = post_to(
            "/api/oauth/codex/import-token",
            &state,
            &json!({ "accessToken": opaque }),
        )
        .await?;

        assert_eq!(status, StatusCode::OK, "{opaque:?} was refused: {body}");
        let connection = body.get("connection").ok_or("no connection")?;
        // With no claims to read, the name falls back to a fixed label rather than to the token —
        // which would put a credential in the panel's connection list.
        assert_eq!(
            connection.get("name").and_then(Value::as_str),
            Some("ChatGPT Access Token"),
            "{opaque:?}: {body}"
        );
        assert_eq!(connection.get("email"), Some(&Value::Null), "{body}");
    }

    let written = seen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .join("\n");
    assert!(
        !written.contains(r#"ChatGPT Access Token","accessToken"#),
        "the label must not have replaced the token in the record"
    );
    Ok(())
}

#[actix_rt::test]
async fn a_supplied_codex_name_wins_over_the_claim() -> TestResult {
    let (state, _seen) = stub(200, json!({ "id": "conn" }).to_string()).await;

    let (status, body) = post_to(
        "/api/oauth/codex/import-token",
        &state,
        &json!({ "accessToken": CODEX_JWT, "name": "  Work account  " }),
    )
    .await?;

    assert_eq!(status, StatusCode::OK, "{body}");
    // Trimmed, and preferred over the email the token carries.
    assert_eq!(
        body.get("connection")
            .and_then(|connection| connection.get("name"))
            .and_then(Value::as_str),
        Some("Work account"),
        "{body}"
    );
    Ok(())
}

#[actix_rt::test]
async fn a_missing_codex_token_is_refused_before_anything_is_recorded() -> TestResult {
    let (state, seen) = stub(200, json!({ "id": "conn" }).to_string()).await;

    for body in [json!({}), json!({ "accessToken": "" }), json!({ "accessToken": "  " })] {
        let (status, response) = post_to("/api/oauth/codex/import-token", &state, &body).await?;

        assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
        assert_eq!(
            response.get("error").and_then(Value::as_str),
            Some("Access token is required")
        );
    }
    assert!(
        seen.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    );
    Ok(())
}
