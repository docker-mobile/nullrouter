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
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
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
                    .push(
                        String::from_utf8_lossy(buffer.get(..read).unwrap_or_default())
                            .into_owned(),
                    );
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
    let (state_addr, state_seen) =
        stub(201, json!({"connection": {"id": "conn_1"}}).to_string()).await;
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
    let (gitlab_addr, _gitlab_seen) =
        stub(401, json!({"message": "401 Unauthorized"}).to_string()).await;
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
async fn a_base_url_that_would_disclose_or_pivot_is_refused_before_the_token_is_sent() -> TestResult
{
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
        let (status, response) =
            post(&state_addr, &json!({"token": "glpat-x", "baseUrl": base})).await?;
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

    // `codex/import-token` and `cursor/import` used to be listed here and are now implemented, so what
    // remains is the two families that genuinely need a consent screen — a provider-hosted login and a
    // redirect this service cannot receive — plus the catch-all for an action that does not exist.
    for (path, provider, action) in [
        (
            "/api/oauth/kiro/social-authorize",
            "kiro",
            "social-authorize",
        ),
        ("/api/oauth/kiro/social-exchange", "kiro", "social-exchange"),
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
    let error = body
        .get("error")
        .and_then(Value::as_str)
        .ok_or("no error")?;
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

    for body in [
        json!({}),
        json!({ "apiKey": "" }),
        json!({ "apiKey": "   " }),
    ] {
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
    assert!(
        written.contains(r#""authType":"access_token""#),
        "{written}"
    );
    assert!(
        written.contains(r#""authMethod":"access_token""#),
        "{written}"
    );
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

    for opaque in [
        "sk-not-a-jwt-at-all",
        "a.b",
        "a.b.c.d",
        "...",
        "%%%.%%%.%%%",
    ] {
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

    for body in [
        json!({}),
        json!({ "accessToken": "" }),
        json!({ "accessToken": "  " }),
    ] {
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

/// An access token whose claims name a Microsoft work account.
const MS_ACCESS_TOKEN: &str = "eyJhbGciOiJub25lIn0.eyJwcmVmZXJyZWRfdXNlcm5hbWUiOiJ3b3JrZXJAY29udG9zby5jb20iLCJleHAiOjIwMDAwMDAwMDB9.sig";

/// A `CLIProxyAPI` document with every required field present.
fn cli_proxy_document() -> Value {
    json!({
        "auth_method": "external_idp",
        "access_token": MS_ACCESS_TOKEN,
        "refresh_token": "refresh-value-abc",
        "client_id": "11111111-2222-3333-4444-555555555555",
        "token_endpoint": "https://login.microsoftonline.com/common/oauth2/v2.0/token",
        "profile_arn": "arn:aws:codewhisperer:us-east-1:123456789012:profile/ABCDEF",
        "scopes": ["openid", "profile", "offline_access"],
    })
}

#[actix_rt::test]
async fn a_cli_proxy_document_is_normalised_and_recorded() -> TestResult {
    let (state, seen) = stub(200, json!({ "id": "conn-cli" }).to_string()).await;

    let (status, response) = post_to(
        "/api/oauth/kiro/import-cli-proxy",
        &state,
        &cli_proxy_document(),
    )
    .await?;

    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response.get("success"), Some(&Value::Bool(true)));
    // The label comes from the token's claims, since a work account rarely carries a plain email.
    assert_eq!(
        response
            .get("connection")
            .and_then(|connection| connection.get("email"))
            .and_then(Value::as_str),
        Some("worker@contoso.com"),
        "{response}"
    );

    let written = seen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .join("\n");
    // The scope list is joined into the space-separated form the refresh grant needs.
    assert!(
        written.contains("openid profile offline_access"),
        "{written}"
    );
    assert!(written.contains("external_idp"), "{written}");
    assert!(written.contains("CLIProxyAPI"), "{written}");
    // The endpoint is stored, because the refresh needs it.
    assert!(written.contains("login.microsoftonline.com"), "{written}");
    Ok(())
}

#[actix_rt::test]
async fn a_token_endpoint_that_is_not_microsofts_is_refused() -> TestResult {
    // This is the check that matters most in the whole route, and it is not about the import: the
    // endpoint is stored, and every later refresh posts the refresh token to whatever was stored. An
    // unvalidated value here would have this service hand a long-lived credential to an endpoint of
    // the caller's choosing, on a schedule, forever.
    let (state, seen) = stub(200, json!({ "id": "conn" }).to_string()).await;

    for (endpoint, expected) in [
        ("http://login.microsoftonline.com/token", "must use https"),
        ("https://evil.example.com/token", "Microsoft login endpoint"),
        (
            "https://login.microsoftonline.com.evil.example.com/token",
            "Microsoft login endpoint",
        ),
        (
            "https://attacker/login.microsoftonline.com",
            "Microsoft login endpoint",
        ),
        ("not a url", "valid URL"),
        ("", "required"),
    ] {
        let mut document = cli_proxy_document();
        document["token_endpoint"] = Value::String(endpoint.to_owned());

        let (status, response) =
            post_to("/api/oauth/kiro/import-cli-proxy", &state, &document).await?;

        assert_eq!(status, StatusCode::BAD_REQUEST, "{endpoint:?}: {response}");
        assert!(
            response
                .get("error")
                .and_then(Value::as_str)
                .is_some_and(|error| error.contains(expected)),
            "{endpoint:?} gave {response}"
        );
    }

    assert!(
        seen.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty(),
        "no refused endpoint may reach the state service"
    );
    Ok(())
}

#[actix_rt::test]
async fn every_required_cli_proxy_field_is_demanded_by_name() -> TestResult {
    let (state, _seen) = stub(200, json!({ "id": "conn" }).to_string()).await;

    for (field, message) in [
        ("access_token", "access_token is required"),
        ("refresh_token", "refresh_token is required"),
        ("client_id", "client_id is required"),
        ("scopes", "scopes is required"),
        ("profile_arn", "profile_arn is required"),
    ] {
        let mut document = cli_proxy_document();
        document
            .as_object_mut()
            .ok_or("document is an object")?
            .remove(field);

        let (status, response) =
            post_to("/api/oauth/kiro/import-cli-proxy", &state, &document).await?;

        assert_eq!(status, StatusCode::BAD_REQUEST, "{field}: {response}");
        assert_eq!(
            response.get("error").and_then(Value::as_str),
            Some(message),
            "{field}: {response}"
        );
    }
    Ok(())
}

#[actix_rt::test]
async fn a_document_for_another_auth_method_is_refused() -> TestResult {
    // Half-importing it would report a missing field rather than the real mismatch.
    let (state, _seen) = stub(200, json!({ "id": "conn" }).to_string()).await;
    let mut document = cli_proxy_document();
    document["auth_method"] = Value::String("social".to_owned());

    let (status, response) = post_to("/api/oauth/kiro/import-cli-proxy", &state, &document).await?;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
    assert!(
        response
            .get("error")
            .and_then(Value::as_str)
            .is_some_and(|error| error.contains("Only external_idp")),
        "{response}"
    );
    Ok(())
}

#[actix_rt::test]
async fn a_cli_proxy_document_is_accepted_at_any_of_its_wrappers() -> TestResult {
    // A user pastes whatever their file gave them, so the document is read at three keys, as the body
    // itself, and as a JSON string holding the document.
    let (state, _seen) = stub(200, json!({ "id": "conn" }).to_string()).await;
    let inner = cli_proxy_document();

    for wrapped in [
        json!({ "cliProxyAuth": inner.clone() }),
        json!({ "auth": inner.clone() }),
        json!({ "json": inner.clone() }),
        inner.clone(),
        json!({ "cliProxyAuth": serde_json::to_string(&inner)? }),
    ] {
        let (status, response) =
            post_to("/api/oauth/kiro/import-cli-proxy", &state, &wrapped).await?;

        assert_eq!(status, StatusCode::OK, "{response}");
    }

    // But a string that is not JSON is named as such rather than reported as a missing field.
    let (status, response) = post_to(
        "/api/oauth/kiro/import-cli-proxy",
        &state,
        &json!({ "cliProxyAuth": "not json at all" }),
    )
    .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
    assert!(
        response
            .get("error")
            .and_then(Value::as_str)
            .is_some_and(|error| error.contains("invalid")),
        "{response}"
    );
    Ok(())
}

#[actix_rt::test]
async fn a_bulk_import_reports_each_account_by_index() -> TestResult {
    // The reason the route exists: one bad entry in a pasted export must not discard the rest.
    let (state, _seen) = stub(200, json!({ "id": "conn" }).to_string()).await;

    let (status, response) = post_to(
        "/api/oauth/codex/bulk-import",
        &state,
        &json!([
            { "accessToken": CODEX_JWT },
            { "notAToken": true },
            { "accessToken": "opaque-but-present" },
            "not an object at all",
        ]),
    )
    .await?;

    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response.get("success"), Some(&Value::from(2)), "{response}");
    assert_eq!(response.get("failed"), Some(&Value::from(2)), "{response}");

    let results = response
        .get("results")
        .and_then(Value::as_array)
        .ok_or("no results")?;
    assert_eq!(results.len(), 4, "{response}");
    // Indexed, so a caller can match a failure to the entry that caused it.
    for (position, result) in results.iter().enumerate() {
        assert_eq!(
            result.get("index"),
            Some(&Value::from(position)),
            "{result}"
        );
    }
    assert_eq!(
        results.first().and_then(|r| r.get("ok")),
        Some(&Value::Bool(true))
    );
    assert_eq!(
        results.get(1).and_then(|r| r.get("ok")),
        Some(&Value::Bool(false))
    );
    assert!(
        results
            .get(1)
            .and_then(|r| r.get("error"))
            .and_then(Value::as_str)
            .is_some_and(|error| error.contains("accessToken")),
        "{response}"
    );
    assert!(
        results
            .get(3)
            .and_then(|r| r.get("error"))
            .and_then(Value::as_str)
            .is_some_and(|error| error.contains("not an object")),
        "{response}"
    );
    Ok(())
}

#[actix_rt::test]
async fn a_bulk_import_accepts_all_three_body_shapes() -> TestResult {
    let (state, _seen) = stub(200, json!({ "id": "conn" }).to_string()).await;
    let one = json!({ "accessToken": CODEX_JWT });

    for shape in [
        json!([one.clone()]),
        json!({ "accounts": [one.clone()] }),
        one.clone(),
    ] {
        let (status, response) = post_to("/api/oauth/codex/bulk-import", &state, &shape).await?;

        assert_eq!(status, StatusCode::OK, "{response}");
        assert_eq!(response.get("success"), Some(&Value::from(1)), "{response}");
    }
    Ok(())
}

#[actix_rt::test]
async fn an_empty_bulk_import_is_refused() -> TestResult {
    let (state, seen) = stub(200, json!({ "id": "conn" }).to_string()).await;

    for shape in [
        json!([]),
        json!({ "accounts": [] }),
        json!("text"),
        json!(7),
    ] {
        let (status, response) = post_to("/api/oauth/codex/bulk-import", &state, &shape).await?;

        assert_eq!(status, StatusCode::BAD_REQUEST, "{shape}: {response}");
        assert_eq!(
            response.get("error").and_then(Value::as_str),
            Some("No accounts provided"),
            "{shape}: {response}"
        );
    }
    assert!(
        seen.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    );
    Ok(())
}

#[actix_rt::test]
async fn a_bulk_import_only_writes_the_fields_it_allows() -> TestResult {
    // The divergence from upstream, and the reason for it. Upstream spreads the item into the record
    // and strips five names, so every field it did not think of is writable — a caller can set
    // `priority` and reorder someone's provider list, or set `isActive` on a credential that should
    // not be live. An import decides what a credential is, not where it sits in a routing order.
    let (state, seen) = stub(200, json!({ "id": "conn" }).to_string()).await;

    let (status, response) = post_to(
        "/api/oauth/codex/bulk-import",
        &state,
        &json!([{
            "accessToken": CODEX_JWT,
            "email": "kept@example.com",
            // None of the rest may reach the record.
            "priority": 1,
            "isActive": false,
            "id": "chosen-id",
            "provider": "anthropic",
            "authType": "api_key",
            "createdAt": "1999-01-01T00:00:00Z",
            "lastRefreshAt": "1999-01-01T00:00:00Z",
            "somethingNew": "surprise",
            "providerSpecificData": {
                "chatgptPlanType": "pro",
                "sudoPassword": "hunter2",
            },
        }]),
    )
    .await?;

    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response.get("success"), Some(&Value::from(1)), "{response}");

    let written = seen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .join("\n");
    // What was asked for and allowed.
    assert!(written.contains("kept@example.com"), "{written}");
    assert!(written.contains("\"chatgptPlanType\":\"pro\""), "{written}");
    // The provider and authType are ours, not the caller's.
    assert!(written.contains("\"provider\":\"codex\""), "{written}");
    assert!(written.contains("\"authType\":\"oauth\""), "{written}");
    assert!(!written.contains("anthropic"), "{written}");
    // And nothing else the caller sent.
    for rejected in [
        "priority",
        "isActive",
        "chosen-id",
        "1999-01-01",
        "somethingNew",
        "surprise",
        "sudoPassword",
        "hunter2",
    ] {
        assert!(
            !written.contains(rejected),
            "{rejected:?} reached the record: {written}"
        );
    }
    Ok(())
}

#[actix_rt::test]
async fn a_bulk_import_backfills_labels_and_computes_an_absolute_expiry() -> TestResult {
    let (state, seen) = stub(200, json!({ "id": "conn" }).to_string()).await;

    let (status, response) = post_to(
        "/api/oauth/codex/bulk-import",
        &state,
        &json!([{
            "accessToken": "opaque-token",
            // The id token carries the claims, and is preferred over the access token for them.
            "idToken": CODEX_JWT,
            "expiresIn": 3600,
        }]),
    )
    .await?;

    assert_eq!(status, StatusCode::OK, "{response}");
    let written = seen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .join("\n");
    // Labels backfilled from the id token.
    assert!(written.contains("person@example.com"), "{written}");
    assert!(written.contains("acct-9f8e"), "{written}");
    // A stated lifetime became an absolute expiry, because the store keeps absolutes and a lifetime
    // is only meaningful at the instant it was issued.
    assert!(written.contains("expiresAt"), "{written}");
    assert!(!written.contains("expiresIn"), "{written}");
    Ok(())
}

#[actix_rt::test]
async fn an_oversized_bulk_import_is_refused_before_any_write() -> TestResult {
    // Each item is a serial round trip, so an unbounded list holds a worker for as long as the list.
    // Upstream has no cap at all.
    let (state, seen) = stub(200, json!({ "id": "conn" }).to_string()).await;
    let many: Vec<Value> = (0..201)
        .map(|index| json!({ "accessToken": format!("token-{index}") }))
        .collect();

    let (status, response) =
        post_to("/api/oauth/codex/bulk-import", &state, &Value::Array(many)).await?;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
    assert!(
        response
            .get("error")
            .and_then(Value::as_str)
            .is_some_and(|error| error.contains("200")),
        "{response}"
    );
    assert!(
        seen.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty(),
        "an oversized import must be refused before anything is written"
    );
    Ok(())
}

/// A Cursor token carrying both an email and a subject claim.
const CURSOR_JWT: &str = "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.eyJlbWFpbCI6ImRldkBleGFtcGxlLmNvbSIsInN1YiI6InVzZXJfYWJjMTIzIn0.sig";

/// One with only a subject, which supplies both the label and the user id.
const CURSOR_JWT_SUB_ONLY: &str =
    "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.eyJzdWIiOiJ1c2VyX29ubHlfc3ViIn0.sig";

/// A machine id of the shape Cursor writes: hex, hyphenated, 32 hex digits.
const CURSOR_MACHINE_ID: &str = "7f3a1b9c4d5e6f708192a3b4c5d6e7f8";

async fn cursor_import(state_addr: &str, body: &Value) -> TestResult<(StatusCode, Value)> {
    post_to("/api/oauth/cursor/import", state_addr, body).await
}

#[actix_rt::test]
async fn a_cursor_token_is_recorded_with_its_machine_id() -> TestResult {
    // Given: a token and machine id copied out of Cursor's local database. Nothing is verified against
    // Cursor: its API speaks protobuf and offers no probe endpoint, so upstream checks the shape and
    // defers real validation to first use. This route says the same thing rather than implying more.
    let (state, seen) = stub(200, json!({ "id": "conn-cursor" }).to_string()).await;

    // When: it is imported.
    let (status, body) = cursor_import(
        &state,
        &json!({ "accessToken": CURSOR_JWT, "machineId": CURSOR_MACHINE_ID }),
    )
    .await?;

    // Then: the connection is recorded, and the machine id travels with it — every later request needs
    // it, so a record without one would be unusable.
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.get("success"), Some(&Value::Bool(true)), "{body}");
    let written = seen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .join("\n");
    assert!(written.contains("\"provider\":\"cursor\""), "{written}");
    assert!(
        written.contains(&format!("\"machineId\":\"{CURSOR_MACHINE_ID}\"")),
        "{written}"
    );
    assert!(written.contains("dev@example.com"), "{written}");
    assert!(written.contains("\"userId\":\"user_abc123\""), "{written}");
    // Cursor publishes no refresh endpoint, so a refresh token would be a value nothing can use.
    assert!(written.contains("\"refreshToken\":null"), "{written}");
    // And the token itself is never echoed back to the caller.
    assert!(!body.to_string().contains(CURSOR_JWT), "{body}");
    Ok(())
}

#[actix_rt::test]
async fn a_cursor_token_without_an_email_claim_falls_back_to_its_subject() -> TestResult {
    // Upstream's `email || sub`. A token with no email still names an account well enough to label the
    // connection, and a connection with no label at all is one the user cannot tell apart in the panel.
    let (state, seen) = stub(200, json!({ "id": "conn-cursor" }).to_string()).await;

    let (status, body) = cursor_import(
        &state,
        &json!({ "accessToken": CURSOR_JWT_SUB_ONLY, "machineId": CURSOR_MACHINE_ID }),
    )
    .await?;

    assert_eq!(status, StatusCode::OK, "{body}");
    let written = seen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .join("\n");
    assert!(written.contains("\"email\":\"user_only_sub\""), "{written}");
    assert!(
        written.contains("\"userId\":\"user_only_sub\""),
        "{written}"
    );
    Ok(())
}

#[actix_rt::test]
async fn a_cursor_token_that_is_not_a_jwt_is_still_imported() -> TestResult {
    // The claims are labels, not a requirement. Cursor has issued opaque tokens, and refusing one for
    // not being a JWT would block an import that would have worked.
    let (state, seen) = stub(200, json!({ "id": "conn-cursor" }).to_string()).await;
    let opaque = "o".repeat(64);

    let (status, body) = cursor_import(
        &state,
        &json!({ "accessToken": opaque, "machineId": CURSOR_MACHINE_ID }),
    )
    .await?;

    assert_eq!(status, StatusCode::OK, "{body}");
    let written = seen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .join("\n");
    // No claims to read, so the label is the provider name and the email is absent rather than wrong.
    assert!(written.contains("\"name\":\"cursor\""), "{written}");
    assert!(written.contains("\"email\":null"), "{written}");
    Ok(())
}

#[actix_rt::test]
async fn a_short_cursor_token_and_a_bad_machine_id_are_each_refused_by_name() -> TestResult {
    // Four refusals, each naming the field at fault. A single "invalid request" would leave the user
    // guessing which of the two values they mis-copied out of a SQLite database.
    let (state, seen) = stub(200, json!({ "id": "conn" }).to_string()).await;

    for (body, expected) in [
        (
            json!({ "machineId": CURSOR_MACHINE_ID }),
            "Access token is required",
        ),
        (
            json!({ "accessToken": CURSOR_JWT }),
            "Machine ID is required",
        ),
        (
            json!({ "accessToken": "too-short", "machineId": CURSOR_MACHINE_ID }),
            "Invalid token format. Token appears too short.",
        ),
        (
            // Right length, but not hex: the shape check is on the characters, not the count alone.
            json!({ "accessToken": CURSOR_JWT, "machineId": "z".repeat(32) }),
            "Invalid machine ID format. Expected UUID format.",
        ),
    ] {
        let (status, response) = cursor_import(&state, &body).await?;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
        assert_eq!(
            response.get("error").and_then(Value::as_str),
            Some(expected),
            "{response}"
        );
    }

    assert!(
        seen.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty(),
        "a refused import must not reach the state service"
    );
    Ok(())
}

#[actix_rt::test]
async fn a_hyphenated_cursor_machine_id_is_accepted() -> TestResult {
    // Cursor writes a hyphenated UUID. Upstream strips the hyphens before counting, so a value that is
    // 36 characters with hyphens and 32 without is the ordinary case, not an exception.
    let (state, seen) = stub(200, json!({ "id": "conn" }).to_string()).await;
    let hyphenated = "7f3a1b9c-4d5e-6f70-8192-a3b4c5d6e7f8";

    let (status, body) = cursor_import(
        &state,
        &json!({ "accessToken": CURSOR_JWT, "machineId": hyphenated }),
    )
    .await?;

    assert_eq!(status, StatusCode::OK, "{body}");
    let written = seen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .join("\n");
    // Stored as the user copied it, hyphens included: it is sent verbatim to Cursor later.
    assert!(
        written.contains(&format!("\"machineId\":\"{hyphenated}\"")),
        "{written}"
    );
    Ok(())
}

#[actix_rt::test]
async fn the_cursor_instructions_name_every_path_and_field() -> TestResult {
    // The GET half of the route. It exists because this service deliberately does not read the user's
    // Cursor database: it tells them which two values to extract and where from.
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(AppConfig::new("0.5.20")))
            .app_data(web::Data::new(StateClient::new("127.0.0.1:1")))
            .app_data(web::Data::new(RuntimeClient::new("127.0.0.1:1")))
            .app_data(web::Data::new(nullrouter_api::TunnelManager::new()))
            .configure(configure),
    )
    .await;
    let request = test::TestRequest::default()
        .method(Method::GET)
        .uri("/api/oauth/cursor/import")
        .to_request();
    let response = test::call_service(&app, request).await;
    let status = response.status();
    let body: Value = serde_json::from_slice(&to_bytes(response.into_body()).await?)?;

    assert_eq!(status, StatusCode::OK, "{body}");
    let rendered = body.to_string();
    // The three platform paths, so a user on any of them can find the file.
    for path in [
        "~/.config/Cursor/User/globalStorage/state.vscdb",
        "Library/Application Support/Cursor",
        "%APPDATA%",
    ] {
        assert!(rendered.contains(path), "{path} is missing: {rendered}");
    }
    // The two database keys, which are the whole point of the instructions.
    assert!(rendered.contains("cursorAuth/accessToken"), "{rendered}");
    assert!(rendered.contains("storage.serviceMachineId"), "{rendered}");
    // And the fields the form must collect, named as the POST half expects them.
    let required = body
        .get("requiredFields")
        .and_then(Value::as_array)
        .expect("requiredFields is an array");
    let names: Vec<&str> = required
        .iter()
        .filter_map(|field| field.get("name").and_then(Value::as_str))
        .collect();
    assert_eq!(names, ["accessToken", "machineId"], "{rendered}");
    Ok(())
}

/// Points iFlow's platform host at a stub for the duration of a case.
struct IflowBase {
    _lock: std::sync::MutexGuard<'static, ()>,
    previous: Option<std::ffi::OsString>,
}

impl IflowBase {
    fn pointing_at(addr: &str) -> Self {
        let lock = env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = std::env::var_os("NULLROUTER_IFLOW_BASE");
        // SAFETY: the lock above is held, so no other case in this binary reads or writes this
        // variable while it is being set.
        unsafe { std::env::set_var("NULLROUTER_IFLOW_BASE", format!("http://{addr}")) };
        Self {
            _lock: lock,
            previous,
        }
    }
}

impl Drop for IflowBase {
    fn drop(&mut self) {
        match &self.previous {
            // SAFETY: the lock is still held until this guard finishes dropping.
            Some(previous) => unsafe { std::env::set_var("NULLROUTER_IFLOW_BASE", previous) },
            // SAFETY: as above.
            None => unsafe { std::env::remove_var("NULLROUTER_IFLOW_BASE") },
        }
    }
}

/// A stub answering GET and POST differently, which is what this route's two calls need.
///
/// The single-response `stub` above cannot express it: the first call reads a name and the second mints
/// a key, and a test that answered both the same way would not notice if the route sent them in the
/// wrong order or skipped one.
async fn iflow_stub(get_body: String, post_body: String) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("addr").to_string();
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&seen);

    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let get_body = get_body.clone();
            let post_body = post_body.clone();
            let recorded = Arc::clone(&recorded);
            tokio::spawn(async move {
                let mut buffer = vec![0_u8; 16384];
                let read = stream.read(&mut buffer).await.unwrap_or(0);
                let request =
                    String::from_utf8_lossy(buffer.get(..read).unwrap_or_default()).into_owned();
                let body = if request.starts_with("POST") {
                    post_body
                } else {
                    get_body
                };
                recorded
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(request);
                let response = format!(
                    "HTTP/1.1 200 X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                     Connection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });

    (addr, seen)
}

/// iFlow's read answer: the key's name, which the rotation call needs.
fn iflow_current() -> String {
    json!({ "success": true, "data": { "name": "nullrouter-key", "apiKey": "old-key-value" } })
        .to_string()
}

/// iFlow's rotation answer: a freshly minted key.
fn iflow_issued() -> String {
    json!({
        "success": true,
        "data": {
            "name": "nullrouter-key",
            "apiKey": "sk-iflow-0123456789abcdef",
            "expireTime": "2027-01-01T00:00:00Z",
        },
    })
    .to_string()
}

async fn iflow_cookie(state_addr: &str, body: &Value) -> TestResult<(StatusCode, Value)> {
    post_to("/api/oauth/iflow/cookie", state_addr, body).await
}

#[actix_rt::test]
async fn an_iflow_cookie_mints_a_key_and_records_it() -> TestResult {
    // Given: a session cookie from the user's browser. iFlow issues nothing a panel can hold, so the
    // route reads the current key's name and posts it back, which mints a new key on the account.
    let (iflow, calls) = iflow_stub(iflow_current(), iflow_issued()).await;
    let _base = IflowBase::pointing_at(&iflow);
    let (state, seen) = stub(200, json!({ "id": "conn-iflow" }).to_string()).await;

    // When: the cookie is submitted.
    let (status, body) = iflow_cookie(
        &state,
        &json!({ "cookie": "BXAuth=session-value; other=x" }),
    )
    .await?;

    // Then: the key is recorded, and only a prefix comes back.
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.get("success"), Some(&Value::Bool(true)), "{body}");
    let returned = body.to_string();
    // Ten characters, as upstream's `substring(0, 10)` has it.
    assert!(returned.contains("sk-iflow-0..."), "{returned}");
    assert!(
        !returned.contains("sk-iflow-0123456789abcdef"),
        "the whole key must not come back: {returned}"
    );

    let requests = calls
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert_eq!(requests.len(), 2, "both calls must happen: {requests:?}");
    assert!(requests[0].starts_with("GET "), "{:?}", requests[0]);
    assert!(requests[1].starts_with("POST "), "{:?}", requests[1]);
    // The rotation call names the key it is rotating, which is what makes it a rotation rather than a
    // second key.
    assert!(requests[1].contains("nullrouter-key"), "{:?}", requests[1]);

    let written = seen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .join("\n");
    assert!(written.contains("\"provider\":\"iflow\""), "{written}");
    assert!(written.contains("\"authType\":\"cookie\""), "{written}");
    assert!(
        written.contains("sk-iflow-0123456789abcdef"),
        "the whole key is what gets stored: {written}"
    );
    assert!(written.contains("2027-01-01T00:00:00Z"), "{written}");
    Ok(())
}

#[actix_rt::test]
async fn only_the_session_field_of_an_iflow_cookie_is_sent_or_stored() -> TestResult {
    // The divergence from upstream, and why. Upstream sends the whole pasted string to iFlow and
    // narrows it to BXAuth only just before storing — so every unrelated cookie in a clipboard paste is
    // disclosed to iFlow first. Narrowing before the call sends what the call needs and nothing else.
    let (iflow, calls) = iflow_stub(iflow_current(), iflow_issued()).await;
    let _base = IflowBase::pointing_at(&iflow);
    let (state, seen) = stub(200, json!({ "id": "conn-iflow" }).to_string()).await;

    let (status, body) = iflow_cookie(
        &state,
        &json!({
            "cookie": "_ga=GA1.2.tracking; BXAuth=session-value; sessionid=someone-elses-site",
        }),
    )
    .await?;

    assert_eq!(status, StatusCode::OK, "{body}");
    let sent = calls
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .join("\n");
    assert!(sent.contains("BXAuth=session-value;"), "{sent}");
    for leaked in ["GA1.2.tracking", "someone-elses-site"] {
        assert!(
            !sent.contains(leaked),
            "{leaked:?} was disclosed to iFlow: {sent}"
        );
    }
    let written = seen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .join("\n");
    for leaked in ["GA1.2.tracking", "someone-elses-site"] {
        assert!(
            !written.contains(leaked),
            "{leaked:?} was stored: {written}"
        );
    }
    Ok(())
}

#[actix_rt::test]
async fn an_iflow_cookie_without_a_session_field_is_refused_before_any_call() -> TestResult {
    // Nothing here reaches iFlow: a cookie with no session field cannot authenticate, so sending it
    // would be a pointless disclosure of whatever else it contained.
    let (iflow, calls) = iflow_stub(iflow_current(), iflow_issued()).await;
    let _base = IflowBase::pointing_at(&iflow);
    let (state, seen) = stub(200, json!({ "id": "conn" }).to_string()).await;

    for (cookie, expected) in [
        ("", "Cookie is required"),
        ("_ga=GA1.2.tracking", "Cookie must contain BXAuth field"),
        // Present but empty, which is the case a `contains` check alone would let through.
        ("BXAuth=; other=x", "Cookie must contain BXAuth field"),
        (
            "BXAuth=value\r\nX-Injected: yes",
            "The cookie contains a control character, so it is not a cookie header.",
        ),
    ] {
        let (status, response) = iflow_cookie(&state, &json!({ "cookie": cookie })).await?;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{cookie:?}: {response}");
        assert_eq!(
            response.get("error").and_then(Value::as_str),
            Some(expected),
            "{cookie:?}"
        );
    }

    assert!(
        calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty(),
        "a refused cookie must not reach iFlow"
    );
    assert!(
        seen.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty(),
        "and nothing must be recorded"
    );
    Ok(())
}

#[actix_rt::test]
async fn an_iflow_rejection_is_reported_without_reflecting_its_body() -> TestResult {
    // iFlow answers 200 with `success: false` for a stale cookie, so the status alone does not say
    // whether the call worked. The body is not reflected: it can quote the cookie back.
    let rejected = json!({
        "success": false,
        "message": "login required, cookie BXAuth=session-value is expired",
    })
    .to_string();
    let (iflow, _calls) = iflow_stub(rejected, iflow_issued()).await;
    let _base = IflowBase::pointing_at(&iflow);
    let (state, seen) = stub(200, json!({ "id": "conn" }).to_string()).await;

    let (status, response) =
        iflow_cookie(&state, &json!({ "cookie": "BXAuth=session-value;" })).await?;

    assert_eq!(status, StatusCode::BAD_GATEWAY, "{response}");
    let rendered = response.to_string();
    assert!(
        rendered.contains("expired"),
        "the refusal should say what to do: {rendered}"
    );
    assert!(
        !rendered.contains("session-value"),
        "the cookie must not be reflected: {rendered}"
    );
    assert!(
        seen.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty(),
        "a rejected cookie must not be recorded"
    );
    Ok(())
}

#[actix_rt::test]
async fn an_iflow_answer_without_a_key_is_not_recorded() -> TestResult {
    // The second call succeeded but minted nothing. Storing a connection with no key would produce a
    // credential that fails at first use, which is worse than a refusal here.
    let keyless = json!({ "success": true, "data": { "name": "nullrouter-key" } }).to_string();
    let (iflow, _calls) = iflow_stub(iflow_current(), keyless).await;
    let _base = IflowBase::pointing_at(&iflow);
    let (state, seen) = stub(200, json!({ "id": "conn" }).to_string()).await;

    let (status, response) =
        iflow_cookie(&state, &json!({ "cookie": "BXAuth=session-value;" })).await?;

    assert_eq!(status, StatusCode::BAD_GATEWAY, "{response}");
    assert_eq!(
        response.get("error").and_then(Value::as_str),
        Some("Missing API key in response"),
        "{response}"
    );
    assert!(
        seen.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty(),
        "{response}"
    );
    Ok(())
}

/// A refresh token of the shape a Kiro login leaves behind.
const KIRO_REFRESH: &str = "aorAAAAAGsomething-long-enough";

/// An access token carrying an email claim, so the recorded connection can be labelled.
const KIRO_JWT: &str = "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.eyJlbWFpbCI6ImRldkBleGFtcGxlLmNvbSIsInN1YiI6InVzZXJfYWJjMTIzIn0.sig";

/// Points one of Kiro's two refresh hosts at a stub for the duration of a case.
struct KiroRefreshBase {
    _lock: std::sync::MutexGuard<'static, ()>,
    name: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl KiroRefreshBase {
    fn social(addr: &str) -> Self {
        Self::set("NULLROUTER_KIRO_SOCIAL_BASE", addr)
    }

    fn oidc(addr: &str) -> Self {
        Self::set("NULLROUTER_KIRO_OIDC_BASE", addr)
    }

    fn set(name: &'static str, addr: &str) -> Self {
        let lock = env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = std::env::var_os(name);
        // SAFETY: the lock above is held, so no other case in this binary reads or writes it here.
        unsafe { std::env::set_var(name, format!("http://{addr}")) };
        Self {
            _lock: lock,
            name,
            previous,
        }
    }
}

impl Drop for KiroRefreshBase {
    fn drop(&mut self) {
        match self.previous.take() {
            // SAFETY: the lock is still held until this guard finishes dropping.
            Some(value) => unsafe { std::env::set_var(self.name, value) },
            // SAFETY: as above.
            None => unsafe { std::env::remove_var(self.name) },
        }
    }
}

/// What either Kiro refresh endpoint answers on success.
fn kiro_refreshed(rotated: bool) -> String {
    let mut document = json!({
        "accessToken": KIRO_JWT,
        "profileArn": "arn:aws:codewhisperer:us-east-1:1:profile/ABC",
        "expiresIn": 900,
    });
    if rotated && let Some(object) = document.as_object_mut() {
        object.insert("refreshToken".to_owned(), json!("aorAAAAAGrotated-token"));
    }
    document.to_string()
}

async fn kiro_import(state_addr: &str, body: &Value) -> TestResult<(StatusCode, Value)> {
    post_to("/api/oauth/kiro/import", state_addr, body).await
}

#[actix_rt::test]
async fn a_social_kiro_token_is_refreshed_and_recorded() -> TestResult {
    // Given: a refresh token from a Google or GitHub login to Kiro. Kiro publishes no endpoint that
    // would accept it in a read-only probe, so the check is the refresh itself — which also means the
    // connection is recorded with a live access token rather than one minted on first use.
    let (kiro, calls) = stub(200, kiro_refreshed(true)).await;
    let _base = KiroRefreshBase::social(&kiro);
    let (state, seen) = stub(200, json!({ "id": "conn-kiro" }).to_string()).await;

    let (status, body) = kiro_import(&state, &json!({ "refreshToken": KIRO_REFRESH })).await?;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.get("success"), Some(&Value::Bool(true)), "{body}");

    // The social service, not the AWS one: sending this token to SSO-OIDC would spend it against an
    // endpoint that cannot honour it.
    let sent = calls
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .join("\n");
    assert!(sent.starts_with("POST /refreshToken"), "{sent}");
    assert!(sent.contains(KIRO_REFRESH), "{sent}");

    let written = seen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .join("\n");
    assert!(written.contains("\"provider\":\"kiro\""), "{written}");
    assert!(written.contains("\"authMethod\":\"imported\""), "{written}");
    assert!(written.contains("dev@example.com"), "{written}");
    // The endpoint rotated the token, so the new one is what gets stored — keeping the old one would
    // make the next refresh fail.
    assert!(written.contains("aorAAAAAGrotated-token"), "{written}");
    // No client credentials on a social login.
    assert!(!written.contains("clientSecret"), "{written}");
    // And the credential never comes back in the response.
    assert!(!body.to_string().contains(KIRO_JWT), "{body}");
    Ok(())
}

#[actix_rt::test]
async fn an_idc_kiro_token_goes_to_the_regional_aws_endpoint() -> TestResult {
    // An organisation login. The client credentials decide the protocol, and they are stored because the
    // next refresh cannot happen without them.
    let (aws, calls) = stub(200, kiro_refreshed(false)).await;
    let _base = KiroRefreshBase::oidc(&aws);
    let (state, seen) = stub(200, json!({ "id": "conn-kiro" }).to_string()).await;

    let (status, body) = kiro_import(
        &state,
        &json!({
            "refreshToken": KIRO_REFRESH,
            "clientId": "client-abc",
            "clientSecret": "secret-xyz",
            "region": "eu-west-1",
        }),
    )
    .await?;

    assert_eq!(status, StatusCode::OK, "{body}");
    let sent = calls
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .join("\n");
    assert!(sent.starts_with("POST /token"), "{sent}");
    // camelCase, not a form-encoded OAuth grant: this endpoint reads `grantType`, which is exactly why
    // the generic refresh path excludes kiro rather than trying.
    assert!(sent.contains("\"grantType\":\"refresh_token\""), "{sent}");
    assert!(sent.contains("\"clientSecret\":\"secret-xyz\""), "{sent}");

    let written = seen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .join("\n");
    assert!(written.contains("\"authMethod\":\"idc\""), "{written}");
    assert!(written.contains("\"provider\":\"Enterprise\""), "{written}");
    assert!(written.contains("\"region\":\"eu-west-1\""), "{written}");
    assert!(written.contains("\"clientId\":\"client-abc\""), "{written}");
    // No rotation in this answer, so the submitted token is kept rather than dropped.
    assert!(written.contains(KIRO_REFRESH), "{written}");
    Ok(())
}

#[actix_rt::test]
async fn half_a_client_credential_pair_is_refused_before_any_call() -> TestResult {
    // One alone cannot authenticate an SSO-OIDC refresh, and treating it as a social login would send an
    // organisation's token to the wrong service — spending it in the process.
    let (kiro, calls) = stub(200, kiro_refreshed(false)).await;
    let _base = KiroRefreshBase::social(&kiro);
    let (state, seen) = stub(200, json!({ "id": "conn" }).to_string()).await;

    for body in [
        json!({ "refreshToken": KIRO_REFRESH, "clientId": "client-abc" }),
        json!({ "refreshToken": KIRO_REFRESH, "clientSecret": "secret-xyz" }),
    ] {
        let (status, response) = kiro_import(&state, &body).await?;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
        assert!(
            response
                .get("error")
                .and_then(Value::as_str)
                .is_some_and(|error| error.contains("both clientId and clientSecret")),
            "{response}"
        );
    }

    // And a missing token is named for what it is.
    let (status, response) = kiro_import(&state, &json!({})).await?;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
    assert_eq!(
        response.get("error").and_then(Value::as_str),
        Some("Refresh token is required"),
        "{response}"
    );

    assert!(
        calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty(),
        "nothing may be spent on a refusal"
    );
    assert!(
        seen.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty(),
        "and nothing recorded"
    );
    Ok(())
}

#[actix_rt::test]
async fn a_kiro_region_that_would_choose_the_host_is_refused() -> TestResult {
    // The region is interpolated into a hostname, so it is pattern-checked first. Upstream's own check
    // has the same shape — and the same consequence, that `us-gov-west-1` is refused.
    let (state, seen) = stub(200, json!({ "id": "conn" }).to_string()).await;

    for region in ["us-east-1.evil.example.com", "../../etc", "US-EAST-1", ""] {
        let body = json!({
            "refreshToken": KIRO_REFRESH,
            "clientId": "client-abc",
            "clientSecret": "secret-xyz",
            "region": region,
        });
        let (status, response) = kiro_import(&state, &body).await?;
        // An empty region falls back to the default rather than being refused, so it is the one value
        // here that does not 400 — but it must not reach a stub-less network either, so this asserts it
        // is not a success recorded from nowhere.
        if region.is_empty() {
            assert_ne!(status, StatusCode::OK, "{response}");
        } else {
            assert_eq!(status, StatusCode::BAD_REQUEST, "{region:?}: {response}");
        }
    }

    assert!(
        seen.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty(),
        "a refused region must not produce a connection"
    );
    Ok(())
}

#[actix_rt::test]
async fn a_refused_kiro_refresh_does_not_reflect_the_bodys_contents() -> TestResult {
    // AWS's OIDC error document quotes the request back, which on some failures includes the client
    // secret that was in it. Upstream returns `await response.text()` verbatim.
    let refused = json!({
        "error": "invalid_grant",
        "error_description": "clientSecret secret-xyz is not valid for clientId client-abc",
    })
    .to_string();
    let (aws, _calls) = stub(400, refused).await;
    let _base = KiroRefreshBase::oidc(&aws);
    let (state, seen) = stub(200, json!({ "id": "conn" }).to_string()).await;

    let (status, response) = kiro_import(
        &state,
        &json!({
            "refreshToken": KIRO_REFRESH,
            "clientId": "client-abc",
            "clientSecret": "secret-xyz",
        }),
    )
    .await?;

    assert_eq!(status, StatusCode::BAD_GATEWAY, "{response}");
    let rendered = response.to_string();
    assert!(
        !rendered.contains("secret-xyz"),
        "the client secret must not be reflected: {rendered}"
    );
    assert!(
        rendered.contains("expired") || rendered.contains("other Kiro login method"),
        "the refusal should say what to try: {rendered}"
    );
    assert!(
        seen.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty(),
        "a refused refresh must not be recorded"
    );
    Ok(())
}

#[actix_rt::test]
async fn a_kiro_answer_without_an_access_token_is_not_recorded() -> TestResult {
    // A 200 that carries no token leaves nothing to store. Recording the connection anyway would
    // produce a provider that fails on its first real request.
    let (kiro, _calls) = stub(200, json!({ "expiresIn": 900 }).to_string()).await;
    let _base = KiroRefreshBase::social(&kiro);
    let (state, seen) = stub(200, json!({ "id": "conn" }).to_string()).await;

    let (status, response) = kiro_import(&state, &json!({ "refreshToken": KIRO_REFRESH })).await?;

    assert_eq!(status, StatusCode::BAD_GATEWAY, "{response}");
    assert!(
        response
            .get("error")
            .and_then(Value::as_str)
            .is_some_and(|error| error.contains("no access token")),
        "{response}"
    );
    assert!(
        seen.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty(),
        "{response}"
    );
    Ok(())
}

#[actix_rt::test]
async fn a_supplied_profile_arn_wins_over_the_one_the_refresh_returned() -> TestResult {
    // The panel resolves an ARN from Kiro IDE's own profile file, where the region has already been
    // normalised for the runtime gateway. When it sends one, it is the more specific value.
    let (kiro, _calls) = stub(200, kiro_refreshed(false)).await;
    let _base = KiroRefreshBase::social(&kiro);
    let (state, seen) = stub(200, json!({ "id": "conn" }).to_string()).await;

    let (status, body) = kiro_import(
        &state,
        &json!({
            "refreshToken": KIRO_REFRESH,
            "profileArn": "arn:aws:codewhisperer:us-east-1:9:profile/SUPPLIED",
        }),
    )
    .await?;

    assert_eq!(status, StatusCode::OK, "{body}");
    let written = seen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .join("\n");
    assert!(written.contains("profile/SUPPLIED"), "{written}");
    assert!(!written.contains("profile/ABC"), "{written}");
    Ok(())
}
