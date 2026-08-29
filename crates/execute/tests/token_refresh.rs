//! The OAuth refresh exchange, against a real socket.
//!
//! `refresh.rs` unit-tests the grant body, the headers, and how a reply is settled.
//! What these add is the exchange itself: that the documented body and headers reach
//! a token endpoint, that a rotation is read back, and that a rejection is
//! distinguished from a transient failure — the difference between "the user must
//! re-authorise" and "try again", which no amount of unit testing of `settle` proves
//! is wired to the wire.

mod mock_upstream;

use mock_upstream::{MockResponse, MockUpstream};
use nullrouter_execute::Executor;
use nullrouter_execute::credentials::Credentials;
use nullrouter_execute::refresh::{RefreshCache, RefreshError};
use serde_json::json;

fn oauth_credentials() -> Credentials {
    Credentials {
        access_token: Some("old-access".to_owned()),
        refresh_token: Some("refresh-1".to_owned()),
        connection_id: "conn_1".to_owned(),
        connection_name: "oauth".to_owned(),
        ..Credentials::default()
    }
}

#[tokio::test]
async fn a_form_grant_reaches_the_endpoint_and_the_rotation_is_read_back() {
    let server = MockUpstream::start(vec![MockResponse::json(
        200,
        r#"{"access_token":"new-access","refresh_token":"refresh-2","expires_in":3600}"#,
    )])
    .await;
    let url = format!("http://{}/oauth/token", server.addr);

    let refreshed = Executor::new()
        .refresh_at(&url, "codex", &oauth_credentials(), "refresh-1")
        .await
        .expect("a refresh");

    assert_eq!(refreshed.access_token, "new-access");
    assert_eq!(refreshed.refresh_token, "refresh-2");
    // `expires_in` is relative; what is stored has to be absolute, or the next
    // expiry check has nothing to compare against.
    assert!(
        refreshed
            .expires_at
            .as_deref()
            .is_some_and(|at| at.ends_with('Z') && at.len() == 20),
        "got {:?}",
        refreshed.expires_at
    );

    let requests = server.requests();
    let sent = requests.first().expect("the endpoint was called");
    assert_eq!(sent.method, "POST");
    assert_eq!(
        sent.headers.get("content-type").map(String::as_str),
        Some("application/x-www-form-urlencoded")
    );
    assert!(
        sent.body.contains("grant_type=refresh_token"),
        "{}",
        sent.body
    );
    assert!(
        sent.body.contains("refresh_token=refresh-1"),
        "{}",
        sent.body
    );
    assert!(sent.body.contains("client_id="), "{}", sent.body);
    // Codex's refresh declares a scope; omitting it narrows the new token.
    assert!(sent.body.contains("scope="), "{}", sent.body);
}

#[tokio::test]
async fn a_json_grant_is_sent_as_json_because_the_endpoint_rejects_a_form() {
    let server = MockUpstream::start(vec![MockResponse::json(
        200,
        r#"{"access_token":"a","refresh_token":"b","expires_in":60}"#,
    )])
    .await;
    let url = format!("http://{}/v1/oauth/token", server.addr);

    Executor::new()
        .refresh_at(&url, "claude", &oauth_credentials(), "refresh-1")
        .await
        .expect("a refresh");

    let requests = server.requests();
    let sent = requests.first().expect("the endpoint was called");
    assert_eq!(
        sent.headers.get("content-type").map(String::as_str),
        Some("application/json")
    );
    let parsed: serde_json::Value = serde_json::from_str(&sent.body).expect("a JSON body");
    assert_eq!(parsed.get("grant_type"), Some(&json!("refresh_token")));
    assert_eq!(parsed.get("refresh_token"), Some(&json!("refresh-1")));
}

#[tokio::test]
async fn a_provider_that_rotates_no_token_keeps_the_one_it_was_given() {
    let server = MockUpstream::start(vec![MockResponse::json(
        200,
        r#"{"access_token":"new-access","expires_in":600}"#,
    )])
    .await;
    let url = format!("http://{}/oauth/token", server.addr);

    let refreshed = Executor::new()
        .refresh_at(&url, "codex", &oauth_credentials(), "refresh-1")
        .await
        .expect("a refresh");
    // Losing it here would make this the last refresh the connection ever gets.
    assert_eq!(refreshed.refresh_token, "refresh-1");
}

#[tokio::test]
async fn an_invalid_grant_is_permanent_so_the_user_is_told_to_re_authorise() {
    let server = MockUpstream::start(vec![MockResponse::json(
        400,
        r#"{"error":"invalid_grant","error_description":"Refresh token has expired"}"#,
    )])
    .await;
    let url = format!("http://{}/oauth/token", server.addr);

    let error = Executor::new()
        .refresh_at(&url, "codex", &oauth_credentials(), "refresh-1")
        .await
        .expect_err("a rejection");
    assert!(
        error.is_permanent(),
        "retrying a revoked token forever is worse than reporting it: {error:?}"
    );
    match error {
        RefreshError::Rejected { message } => {
            assert!(message.contains("expired"), "{message}");
        }
        other => panic!("expected a rejection, got {other:?}"),
    }
}

#[tokio::test]
async fn a_server_error_is_transient_so_the_existing_token_is_not_abandoned() {
    let server = MockUpstream::start(vec![MockResponse::json(503, "upstream is down")]).await;
    let url = format!("http://{}/oauth/token", server.addr);

    let error = Executor::new()
        .refresh_at(&url, "codex", &oauth_credentials(), "refresh-1")
        .await
        .expect_err("a failure");
    assert!(
        !error.is_permanent(),
        "a 503 says nothing about the token: {error:?}"
    );
}

#[tokio::test]
async fn an_unreachable_endpoint_is_transient_not_a_revoked_credential() {
    // Nothing listens on port 1.
    let error = Executor::new()
        .refresh_at(
            "http://127.0.0.1:1/oauth/token",
            "codex",
            &oauth_credentials(),
            "refresh-1",
        )
        .await
        .expect_err("a failure");
    assert!(!error.is_permanent(), "{error:?}");
}

#[tokio::test]
async fn a_provider_whose_refresh_is_not_a_grant_is_refused_without_a_request() {
    let server = MockUpstream::start(vec![MockResponse::json(200, "{}")]).await;
    // Kiro needs an AWS SSO-OIDC exchange. Sending it a standard grant would spend
    // the refresh token against an endpoint that cannot honour it.
    let error = Executor::new()
        .refresh_credentials("kiro", &oauth_credentials(), &RefreshCache::new())
        .await
        .expect_err("unsupported");
    assert_eq!(error, RefreshError::Unsupported);
    assert_eq!(server.request_count(), 0);
}

#[tokio::test]
async fn a_connection_with_no_refresh_token_is_refused_without_a_request() {
    let mut credentials = oauth_credentials();
    credentials.refresh_token = None;
    let error = Executor::new()
        .refresh_credentials("codex", &credentials, &RefreshCache::new())
        .await
        .expect_err("not configured");
    assert_eq!(error, RefreshError::NotConfigured);
}

#[tokio::test]
async fn the_cache_stops_a_second_caller_spending_the_same_token() {
    // Two queued replies. A second exchange would consume the second one, so the
    // request count is what proves the cache held.
    let server = MockUpstream::start(vec![
        MockResponse::json(
            200,
            r#"{"access_token":"first","refresh_token":"rotated","expires_in":60}"#,
        ),
        MockResponse::json(
            200,
            r#"{"access_token":"second","refresh_token":"rotated-again","expires_in":60}"#,
        ),
    ])
    .await;
    let url = format!("http://{}/oauth/token", server.addr);
    let cache = RefreshCache::new();
    let executor = Executor::new();
    let credentials = oauth_credentials();

    // Prime the cache the way `refresh_credentials` does.
    let first = executor
        .refresh_at(&url, "codex", &credentials, "refresh-1")
        .await;
    cache.put("codex", "refresh-1", &first);

    let reused = executor
        .refresh_credentials("codex", &credentials, &cache)
        .await
        .expect("the cached result");
    assert_eq!(reused.access_token, "first");
    assert_eq!(
        server.request_count(),
        1,
        "a second exchange would spend a token the first already rotated away"
    );
}
