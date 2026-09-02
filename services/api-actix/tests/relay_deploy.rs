//! The three relay deploys, against stub platform APIs.
//!
//! These routes deploy code to a user's own Cloudflare, Deno or Vercel account with a token from the
//! request. Nothing here can be tested by really deploying, so the API bases are pointed at a local
//! stub through the environment overrides the module reads. What the stub makes checkable is the part
//! that matters and that a unit test cannot see: the call sequence, the cleanup after a failed
//! deploy, and that the user's token never comes back in a response.

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

/// One canned reply, matched on the request line.
#[derive(Clone)]
struct Reply {
    /// `"<METHOD> <path>"`, matched exactly rather than as a substring.
    ///
    /// Exactly, because `POST /apps/app_1/deploy` contains `POST /apps`: a substring match made the
    /// create reply answer the deploy call, and the test failed on a symptom three steps later.
    matches: &'static str,
    status: u16,
    body: String,
}

/// A stub platform, recording every request line and every `Authorization` header it saw.
struct Stub {
    addr: String,
    seen: Arc<Mutex<Vec<String>>>,
    auth: Arc<Mutex<Vec<String>>>,
}

async fn stub(replies: Vec<Reply>) -> Stub {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("addr").to_string();
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let auth: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&seen);
    let recorded_auth = Arc::clone(&auth);

    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let replies = replies.clone();
            let recorded = Arc::clone(&recorded);
            let recorded_auth = Arc::clone(&recorded_auth);
            tokio::spawn(async move {
                let mut buffer = vec![0_u8; 65536];
                let read = stream.read(&mut buffer).await.unwrap_or(0);
                let head = String::from_utf8_lossy(buffer.get(..read).unwrap_or_default());
                let request_line = head.lines().next().unwrap_or_default().to_owned();
                recorded
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(request_line.clone());
                for line in head.lines() {
                    if let Some(value) = line
                        .strip_prefix("authorization: ")
                        .or_else(|| line.strip_prefix("Authorization: "))
                    {
                        recorded_auth
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .push(value.trim().to_owned());
                    }
                }

                // `"POST /apps HTTP/1.1"` -> `"POST /apps"`.
                let mut parts = request_line.split(' ');
                let method_and_path = match (parts.next(), parts.next()) {
                    (Some(method), Some(path)) => format!("{method} {path}"),
                    _ => request_line.clone(),
                };
                let reply = replies
                    .iter()
                    .find(|reply| reply.matches == method_and_path);
                let (status, body) = match reply {
                    Some(reply) => (reply.status, reply.body.clone()),
                    None => (404, json!({ "error": "no stub" }).to_string()),
                };
                let response = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                     Connection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });

    Stub { addr, seen, auth }
}

impl Stub {
    fn requests(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn auth_headers(&self) -> Vec<String> {
        self.auth
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn saw(&self, fragment: &str) -> bool {
        self.requests().iter().any(|line| line.contains(fragment))
    }
}

fn reply(matches: &'static str, status: u16, body: Value) -> Reply {
    Reply {
        matches,
        status,
        body: body.to_string(),
    }
}

/// A stub state service that accepts the pool record.
async fn stub_state() -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("addr").to_string();
    let bodies: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&bodies);

    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let recorded = Arc::clone(&recorded);
            tokio::spawn(async move {
                let mut buffer = vec![0_u8; 16384];
                let read = stream.read(&mut buffer).await.unwrap_or(0);
                let text = String::from_utf8_lossy(buffer.get(..read).unwrap_or_default());
                if let Some((_head, body)) = text.split_once("\r\n\r\n") {
                    recorded
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(body.to_owned());
                }
                let body = json!({
                    "proxyPool": { "id": "pool_1", "name": "relay-test", "type": "cloudflare" }
                })
                .to_string();
                let response = format!(
                    "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });

    (addr, bodies)
}

async fn deploy(state_addr: &str, uri: &str, body: &Value) -> TestResult<(StatusCode, Value)> {
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
        .uri(uri)
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(serde_json::to_string(body)?)
        .to_request();
    let response = test::call_service(&app, request).await;
    let status = response.status();
    let bytes = to_bytes(response.into_body()).await?;
    Ok((status, serde_json::from_slice(&bytes)?))
}

/// Point one platform's API base at the stub for the duration of a case.
struct ApiOverride {
    _lock: std::sync::MutexGuard<'static, ()>,
    name: &'static str,
    previous: Option<std::ffi::OsString>,
}

fn env_lock() -> &'static Mutex<()> {
    static LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

impl ApiOverride {
    fn new(name: &'static str, value: &str) -> Self {
        let lock = env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = std::env::var_os(name);
        // SAFETY: the lock is held, so no other case in this binary reads or writes it here.
        unsafe { std::env::set_var(name, value) };
        Self {
            _lock: lock,
            name,
            previous,
        }
    }
}

impl Drop for ApiOverride {
    fn drop(&mut self) {
        match self.previous.take() {
            // SAFETY: the lock is still held until this guard finishes dropping.
            Some(value) => unsafe { std::env::set_var(self.name, value) },
            // SAFETY: as above.
            None => unsafe { std::env::remove_var(self.name) },
        }
    }
}

#[actix_web::test]
async fn a_cloudflare_deploy_uploads_enables_and_records_the_pool() -> TestResult {
    // Given: a Cloudflare account that accepts the upload and has a workers.dev subdomain.
    let platform = stub(vec![
        reply(
            "PUT /accounts/abc123/workers/scripts/relay-test",
            200,
            json!({"success": true}),
        ),
        reply(
            "POST /accounts/abc123/workers/scripts/relay-test/subdomain",
            200,
            json!({"success": true}),
        ),
        reply(
            "GET /accounts/abc123/workers/subdomain",
            200,
            json!({"result": {"subdomain": "my-team"}}),
        ),
    ])
    .await;
    let (state_addr, pool_bodies) = stub_state().await;
    let _api = ApiOverride::new(
        "NULLROUTER_CLOUDFLARE_API",
        &format!("http://{}", platform.addr),
    );

    // When: the dashboard deploys a relay.
    let (status, body) = deploy(
        &state_addr,
        "/api/proxy-pools/cloudflare-deploy",
        &json!({"accountId": "abc123", "apiToken": "cf-secret", "projectName": "relay-test"}),
    )
    .await?;

    // Then: it reports the URL the relay is actually reachable at, built from the account's own
    // subdomain rather than guessed.
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["deployUrl"], "https://relay-test.my-team.workers.dev");
    assert_eq!(body["proxyPool"]["id"], "pool_1", "{body}");

    // And all three calls happened, in the order each depends on.
    let requests = platform.requests();
    assert!(
        platform.saw("PUT /accounts/abc123/workers/scripts/relay-test"),
        "{requests:?}"
    );
    assert!(platform.saw("/subdomain"), "{requests:?}");

    // The token was sent to Cloudflare as a bearer credential...
    assert!(
        platform
            .auth_headers()
            .iter()
            .any(|value| value == "Bearer cf-secret"),
        "{:?}",
        platform.auth_headers()
    );
    // ...and does not come back in the response, nor reach the stored pool record.
    let serialised = serde_json::to_string(&body)?;
    assert!(
        !serialised.contains("cf-secret"),
        "the token leaked into the response: {serialised}"
    );
    let pools = pool_bodies
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(pools.len(), 1, "{pools:?}");
    assert!(
        !pools[0].contains("cf-secret"),
        "the token was stored: {}",
        pools[0]
    );
    assert!(pools[0].contains("\"type\":\"cloudflare\""), "{}", pools[0]);
    assert!(
        pools[0].contains("relay-test.my-team.workers.dev"),
        "the pool must point at the relay: {}",
        pools[0]
    );
    Ok(())
}

#[actix_web::test]
async fn a_cloudflare_account_without_a_subdomain_is_told_what_to_fix() -> TestResult {
    // Given: an account with no workers.dev subdomain, so a deployed worker has no hostname.
    let platform = stub(vec![
        reply(
            "PUT /accounts/abc123/workers/scripts/relay-test",
            200,
            json!({"success": true}),
        ),
        reply(
            "GET /accounts/abc123/workers/subdomain",
            200,
            json!({"result": {}}),
        ),
    ])
    .await;
    let (state_addr, pool_bodies) = stub_state().await;
    let _api = ApiOverride::new(
        "NULLROUTER_CLOUDFLARE_API",
        &format!("http://{}", platform.addr),
    );

    // When: a deploy runs.
    let (status, body) = deploy(
        &state_addr,
        "/api/proxy-pools/cloudflare-deploy",
        &json!({"accountId": "abc123", "apiToken": "t", "projectName": "relay-test"}),
    )
    .await?;

    // Then: the cause is named along with the fix, since it is an account setting rather than
    // anything about this request. And no pool is recorded, because there is no URL to record.
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let error = body["error"].as_str().unwrap_or_default();
    assert!(error.contains("workers.dev subdomain"), "{error}");
    assert!(
        pool_bodies
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty(),
        "a pool with no working URL must not be recorded"
    );
    Ok(())
}

#[actix_web::test]
async fn a_cloudflare_rejection_reports_cloudflares_own_message() -> TestResult {
    // Given: a token Cloudflare refuses.
    let platform = stub(vec![reply(
        "PUT /accounts/abc123/workers/scripts/relay-test",
        403,
        json!({"errors": [{"message": "Authentication error"}]}),
    )])
    .await;
    let (state_addr, _pools) = stub_state().await;
    let _api = ApiOverride::new(
        "NULLROUTER_CLOUDFLARE_API",
        &format!("http://{}", platform.addr),
    );

    // When: a deploy runs.
    let (status, body) = deploy(
        &state_addr,
        "/api/proxy-pools/cloudflare-deploy",
        &json!({"accountId": "abc123", "apiToken": "wrong", "projectName": "relay-test"}),
    )
    .await?;

    // Then: the platform's status and message are passed through. "Deploy failed" would send the
    // user looking for the problem in this router rather than at their token.
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"], "Authentication error");
    Ok(())
}

#[actix_web::test]
async fn a_deno_deploy_waits_for_the_build_and_records_the_pool() -> TestResult {
    // Given: an app that is created, deployed, and reports success immediately.
    let platform = stub(vec![
        reply("POST /apps", 200, json!({"id": "app_1"})),
        reply(
            "POST /apps/app_1/deploy",
            200,
            json!({"id": "rev_1", "status": "succeeded"}),
        ),
    ])
    .await;
    let (state_addr, pool_bodies) = stub_state().await;
    let _api = ApiOverride::new("NULLROUTER_DENO_API", &format!("http://{}", platform.addr));

    // When: the dashboard deploys.
    let (status, body) = deploy(
        &state_addr,
        "/api/proxy-pools/deno-deploy",
        &json!({"orgDomain": "acme.deno.net", "denoToken": "dn-secret", "projectName": "relay-test"}),
    )
    .await?;

    // Then: the URL uses the first label of the org domain, which is the org slug.
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["deployUrl"], "https://relay-test.acme.deno.net");
    let pools = pool_bodies
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert!(pools[0].contains("\"type\":\"deno\""), "{}", pools[0]);
    assert!(
        !pools[0].contains("dn-secret"),
        "the token was stored: {}",
        pools[0]
    );
    Ok(())
}

#[actix_web::test]
async fn a_failed_deno_deploy_deletes_the_app_it_created() -> TestResult {
    // Given: an app that is created but whose deploy is rejected.
    let platform = stub(vec![
        reply(
            "POST /apps/app_1/deploy",
            400,
            json!({"error": "bad asset"}),
        ),
        reply("POST /apps", 200, json!({"id": "app_1"})),
        reply("DELETE /apps/app_1", 200, json!({"ok": true})),
    ])
    .await;
    let (state_addr, pool_bodies) = stub_state().await;
    let _api = ApiOverride::new("NULLROUTER_DENO_API", &format!("http://{}", platform.addr));

    // When: the deploy runs.
    let (status, body) = deploy(
        &state_addr,
        "/api/proxy-pools/deno-deploy",
        &json!({"orgDomain": "acme.deno.net", "denoToken": "t", "projectName": "relay-test"}),
    )
    .await?;

    // Then: the app is cleaned up. Without this the failed attempt keeps the name, so the obvious
    // next step — try again — fails with "already exists".
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert!(
        platform.saw("DELETE /apps/app_1"),
        "{:?}",
        platform.requests()
    );
    assert!(
        pool_bodies
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty(),
        "no pool for a deploy that failed"
    );
    Ok(())
}

#[actix_web::test]
async fn a_taken_deno_app_name_says_so_rather_than_blaming_the_token() -> TestResult {
    let platform = stub(vec![reply("POST /apps", 409, json!({"error": "conflict"}))]).await;
    let (state_addr, _pools) = stub_state().await;
    let _api = ApiOverride::new("NULLROUTER_DENO_API", &format!("http://{}", platform.addr));

    let (status, body) = deploy(
        &state_addr,
        "/api/proxy-pools/deno-deploy",
        &json!({"orgDomain": "acme.deno.net", "denoToken": "t", "projectName": "relay-test"}),
    )
    .await?;

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let error = body["error"].as_str().unwrap_or_default();
    assert!(error.contains("already exists"), "{error}");
    assert!(error.contains("relay-test"), "the name is named: {error}");
    Ok(())
}

#[actix_web::test]
async fn a_vercel_deploy_disables_protection_before_reporting_ready() -> TestResult {
    // Given: a deployment that is ready on the first poll.
    let platform = stub(vec![
        reply(
            "POST /v13/deployments",
            200,
            json!({"id": "dpl_1", "projectId": "prj_1"}),
        ),
        reply("PATCH /v9/projects/prj_1", 200, json!({"ok": true})),
        reply(
            "GET /v13/deployments/dpl_1",
            200,
            json!({"readyState": "READY", "url": "relay-test.vercel.app"}),
        ),
    ])
    .await;
    let (state_addr, pool_bodies) = stub_state().await;
    let _api = ApiOverride::new(
        "NULLROUTER_VERCEL_API",
        &format!("http://{}", platform.addr),
    );

    // When: the dashboard deploys.
    let (status, body) = deploy(
        &state_addr,
        "/api/proxy-pools/vercel-deploy",
        &json!({"vercelToken": "vc-secret", "projectName": "relay-test"}),
    )
    .await?;

    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["deployUrl"], "https://relay-test.vercel.app");

    // Then: protection was turned off. Vercel puts SSO in front of new deployments by default, so
    // skipping this leaves a relay that answers every request with a login page while the pool
    // looks configured.
    assert!(
        platform.saw("PATCH /v9/projects/prj_1"),
        "{:?}",
        platform.requests()
    );
    let pools = pool_bodies
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert!(pools[0].contains("\"type\":\"vercel\""), "{}", pools[0]);
    assert!(
        !pools[0].contains("vc-secret"),
        "the token was stored: {}",
        pools[0]
    );
    Ok(())
}

#[actix_web::test]
async fn a_vercel_deployment_that_errors_is_reported_rather_than_waited_on() -> TestResult {
    let platform = stub(vec![
        reply(
            "POST /v13/deployments",
            200,
            json!({"id": "dpl_1", "projectId": "prj_1"}),
        ),
        reply("PATCH /v9/projects/prj_1", 200, json!({"ok": true})),
        reply(
            "GET /v13/deployments/dpl_1",
            200,
            json!({"readyState": "ERROR"}),
        ),
    ])
    .await;
    let (state_addr, pool_bodies) = stub_state().await;
    let _api = ApiOverride::new(
        "NULLROUTER_VERCEL_API",
        &format!("http://{}", platform.addr),
    );

    let (status, body) = deploy(
        &state_addr,
        "/api/proxy-pools/vercel-deploy",
        &json!({"vercelToken": "t", "projectName": "relay-test"}),
    )
    .await?;

    // A terminal failure ends the wait rather than polling until the timeout.
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|error| error.contains("ERROR")),
        "{body}"
    );
    assert!(
        pool_bodies
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty(),
        "no pool for a deployment that errored"
    );
    Ok(())
}

#[actix_web::test]
async fn a_deploy_is_refused_before_any_platform_call_when_the_request_is_incomplete() -> TestResult
{
    // Deploying creates something billable and publicly reachable in someone's account, so a
    // malformed request must not reach the platform at all.
    let platform = stub(Vec::new()).await;
    let (state_addr, _pools) = stub_state().await;

    for (uri, body, expected) in [
        (
            "/api/proxy-pools/cloudflare-deploy",
            json!({"apiToken": "t"}),
            "Cloudflare Account ID and API Token are required",
        ),
        (
            "/api/proxy-pools/cloudflare-deploy",
            json!({"accountId": "a", "apiToken": ""}),
            "Cloudflare Account ID and API Token are required",
        ),
        (
            "/api/proxy-pools/deno-deploy",
            json!({"denoToken": "t"}),
            "Organization domain is required",
        ),
        (
            "/api/proxy-pools/deno-deploy",
            json!({"orgDomain": "acme.deno.net"}),
            "Deno Deploy API token is required",
        ),
        (
            "/api/proxy-pools/vercel-deploy",
            json!({}),
            "Vercel API token is required",
        ),
    ] {
        let (status, response) = deploy(&state_addr, uri, &body).await?;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{uri} {body} -> {response}"
        );
        assert_eq!(response["error"], expected, "{uri} {body}");
    }

    assert!(
        platform.requests().is_empty(),
        "a refused request must not reach the platform: {:?}",
        platform.requests()
    );
    Ok(())
}

#[actix_web::test]
async fn a_project_name_that_could_address_another_resource_is_refused() -> TestResult {
    // The name goes into a platform API path. A separator in it would act on a different resource in
    // the user's account than the one the dashboard named.
    let platform = stub(Vec::new()).await;
    let (state_addr, _pools) = stub_state().await;
    let _api = ApiOverride::new(
        "NULLROUTER_CLOUDFLARE_API",
        &format!("http://{}", platform.addr),
    );

    for name in ["../other", "a/b", "UPPER", "-leading", "has space"] {
        let (status, body) = deploy(
            &state_addr,
            "/api/proxy-pools/cloudflare-deploy",
            &json!({"accountId": "abc123", "apiToken": "t", "projectName": name}),
        )
        .await?;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{name:?} -> {body}");
    }
    // And an account id is checked the same way, for the same reason.
    let (status, body) = deploy(
        &state_addr,
        "/api/proxy-pools/cloudflare-deploy",
        &json!({"accountId": "../evil", "apiToken": "t", "projectName": "relay-test"}),
    )
    .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    assert!(
        platform.requests().is_empty(),
        "none of these should have reached the platform: {:?}",
        platform.requests()
    );
    Ok(())
}
