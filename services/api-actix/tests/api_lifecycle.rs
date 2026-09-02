//! Shutdown, and refusing to replace our own binary.
//!
//! `POST /api/shutdown` used to answer 501. It now really stops the service — which is why
//! most of this file is about the ways it must *not* fire.
//!
//! Two properties matter more than the happy path:
//!
//! * **It is disabled unless a secret is configured**, and a wrong secret is refused. A route
//!   that stops the router is worth attacking, and it sits behind dashboard auth rather than
//!   nothing, so the secret is the second factor.
//! * **The response says which services keep running.** This port has eight; the route can
//!   stop one. A caller told `success: true` who then believes their router is off, while the
//!   gateway keeps serving `/v1` and spending provider credits, has been misled by a true
//!   statement. The `stillRunning` list and the warning exist to prevent that.

#![allow(
    clippy::future_not_send,
    clippy::expect_used,
    clippy::panic,
    reason = "test assertions read clearer with direct expect than with error plumbing"
)]
#![allow(
    clippy::indexing_slicing,
    reason = "indexing a serde_json::Value is the assertion: a shape that does not match \
              is a test failure, which is what the panic reports"
)]

use actix_web::{
    App,
    body::to_bytes,
    http::{Method, StatusCode, header},
    test, web,
};
use nullrouter_api::{AppConfig, RuntimeClient, ShutdownHandle, StateClient, configure};
use serde_json::Value;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const UNREACHABLE: &str = "127.0.0.1:1";
const SECRET_VAR: &str = "NULLROUTER_SHUTDOWN_SECRET";

const fn app_config() -> AppConfig {
    AppConfig::new("0.5.20")
}

/// One request against an in-process app, for the cases that are refused before anything stops.
///
/// `handle: false` models a service started without a shutdown handle — the route must then
/// report that it cannot stop anything rather than claiming success. `handle: true` registers one
/// that is never filled, which behaves the same way; authorised shutdowns therefore go through
/// [`shutdown_a_real_server`] instead, and this helper is for refusals only.
async fn post(uri: &str, token: Option<&str>, handle: bool) -> TestResult<(StatusCode, Value)> {
    let mut app = App::new()
        .app_data(web::Data::new(app_config()))
        .app_data(web::Data::new(StateClient::new(UNREACHABLE)))
        .app_data(web::Data::new(RuntimeClient::new(UNREACHABLE)));
    if handle {
        // Registered but never filled: enough to prove the authorisation decision happens before
        // the handle is consulted, which is why an unauthorised call is 401/403 and not 501.
        app = app.app_data(web::Data::new(ShutdownHandle::new()));
    }
    let app = test::init_service(app.configure(configure)).await;

    let mut req = test::TestRequest::default().method(Method::POST).uri(uri);
    if let Some(token) = token {
        req = req.insert_header((header::AUTHORIZATION, format!("Bearer {token}")));
    }
    let res = test::call_service(&app, req.to_request()).await;
    let status = res.status();
    let body = String::from_utf8(to_bytes(res.into_body()).await?.to_vec())?;
    Ok((status, serde_json::from_str(&body)?))
}

/// Tests here mutate a process-wide env var, so they must not run concurrently.
///
/// `cargo test` runs a file's tests on parallel threads of one process, and two tests setting
/// and removing the same variable would flake against each other. A mutex is simpler and more
/// honest than making the production code read the secret from somewhere test-injectable purely
/// to suit the tests.
///
/// The guard is held across the `await` deliberately. Each `#[actix_web::test]` runs on its own
/// single-threaded runtime, so a non-`Send` guard is fine here — and the alternative,
/// `block_on` inside an async test, nests one runtime inside another.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Sets the secret for the duration of the guard, and clears it on drop.
struct SecretGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl SecretGuard {
    fn set(secret: Option<&str>) -> Self {
        let lock = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // SAFETY: ENV_LOCK is held, so no other test in this process reads or writes it here.
        match secret {
            // SAFETY: ENV_LOCK is held, so no other test in this process reads or writes it here.
            Some(value) => unsafe { std::env::set_var(SECRET_VAR, value) },
            // SAFETY: as above.
            None => unsafe { std::env::remove_var(SECRET_VAR) },
        }
        Self { _lock: lock }
    }
}

impl Drop for SecretGuard {
    fn drop(&mut self) {
        // Cleared on drop so a panicking test cannot leave the route enabled for the next one.
        // SAFETY: the lock is still held until this guard finishes dropping.
        unsafe { std::env::remove_var(SECRET_VAR) };
    }
}

#[actix_web::test]
async fn shutdown_is_disabled_when_no_secret_is_configured() -> TestResult {
    // The default posture. Upstream refuses too when SHUTDOWN_SECRET is unset, and
    // additionally refuses outright in production; this port's gate is the secret alone, so it
    // must actually be a gate.
    let _secret = SecretGuard::set(None);
    let (status, body) = post("/api/shutdown", Some("anything"), true).await?;

    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["success"], false);
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(SECRET_VAR),
        "the refusal should name the variable that enables it: {message:?}"
    );
    Ok(())
}

#[actix_web::test]
async fn an_empty_secret_does_not_enable_shutdown() -> TestResult {
    // `NULLROUTER_SHUTDOWN_SECRET=` is a plausible way to end up with an empty value, and it
    // must not become a route that anyone can trigger with an empty bearer token.
    let _secret = SecretGuard::set(Some(""));
    let (status, body) = post("/api/shutdown", Some(""), true).await?;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    Ok(())
}

#[actix_web::test]
async fn a_wrong_or_missing_token_is_unauthorized() -> TestResult {
    let _secret = SecretGuard::set(Some("secret-value"));
    for token in [None, Some(""), Some("wrong"), Some("secret-value-x")] {
        let (status, body) = post("/api/shutdown", token, true).await?;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "token {token:?} should be refused, got {body}"
        );
        assert_eq!(body["success"], false);
    }
    Ok(())
}

/// Post to a real running server whose shutdown handle is filled in, and confirm it stops.
///
/// Needed because a handle that is never set takes the "cannot stop itself" path: an
/// `init_service` version of an authorised-shutdown test would assert the reply shape while
/// proving nothing about the shutdown, and would still pass with the stop removed altogether.
///
/// Returns once the server has actually gone away, so every caller gets that check for free.
async fn shutdown_a_real_server(uri: &str, token: &str) -> TestResult<(u16, Value)> {
    let shutdown = web::Data::new(ShutdownHandle::new());
    let server = {
        let shutdown = shutdown.clone();
        actix_web::HttpServer::new(move || {
            App::new()
                .app_data(web::Data::new(app_config()))
                .app_data(web::Data::new(StateClient::new(UNREACHABLE)))
                .app_data(web::Data::new(RuntimeClient::new(UNREACHABLE)))
                .app_data(shutdown.clone())
                .configure(configure)
        })
        .workers(1)
        .bind("127.0.0.1:0")?
    };
    let port = server
        .addrs()
        .first()
        .map(std::net::SocketAddr::port)
        .ok_or("no bound address")?;
    let server = server.run();
    shutdown.set(server.handle());
    let serving = actix_web::rt::spawn(server);

    let response = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}{uri}"))
        .bearer_auth(token)
        .send()
        .await?;
    let status = response.status().as_u16();
    let body: Value = response.json().await?;

    // The reply is sent first and the stop follows after SHUTDOWN_DELAY, so the server must go
    // away on its own. If it does not, this hangs to the timeout rather than passing.
    // Three layers to unwrap, and each failure means something different: the timeout means it
    // never stopped, the join error means the server task panicked, and the inner error is the
    // server's own exit status.
    actix_web::rt::time::timeout(std::time::Duration::from_secs(10), serving)
        .await
        .map_err(|_| format!("the server did not stop within 10s of an authorised {uri}"))?
        .map_err(|error| format!("the server task panicked: {error}"))?
        .map_err(|error| format!("the server exited with an error: {error}"))?;

    // And the port is genuinely released, not merely no longer serving.
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    assert!(
        actix_web::rt::net::TcpStream::connect(address)
            .await
            .is_err(),
        "port {port} still accepts connections after {uri}"
    );
    Ok((status, body))
}

#[actix_web::test]
async fn a_correct_token_stops_a_real_server_and_reports_what_keeps_running() -> TestResult {
    let _secret = SecretGuard::set(Some("secret-value"));
    let (status, body) = shutdown_a_real_server("/api/shutdown", "secret-value").await?;

    assert_eq!(status, 200, "{body}");
    assert_eq!(body["success"], true);
    assert_eq!(body["stopping"], "nullrouter-api");
    // The list is probed, so in a test with no siblings up it is legitimately empty. What must
    // hold is that the field is present and is a list — a caller reads it to decide whether to
    // tell the user their router is still live.
    assert!(
        body["stillRunning"].is_array(),
        "stillRunning must always be reported: {body}"
    );
    // And when siblings *are* up, the warning must accompany them. This is not hypothetical: the
    // suite may well run on a machine where the dev services are listening.
    if body["stillRunning"]
        .as_array()
        .is_some_and(|list| !list.is_empty())
    {
        let warning = body["warning"].as_str().unwrap_or_default();
        assert!(
            warning.contains("/v1"),
            "a warning listing live services should say /v1 is still served: {warning:?}"
        );
    }
    Ok(())
}

#[actix_web::test]
async fn a_service_with_no_handle_says_it_cannot_stop_itself() -> TestResult {
    // Rather than reporting a shutdown that will never happen. This is the failure mode the
    // OnceLock exists for: app data is registered before the server handle exists.
    let _secret = SecretGuard::set(Some("secret-value"));
    let (status, body) = post("/api/shutdown", Some("secret-value"), false).await?;

    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{body}");
    assert_eq!(body["success"], false);
    assert_eq!(body["unsupported"], true);
    Ok(())
}

#[actix_web::test]
async fn version_shutdown_is_gated_the_same_way() -> TestResult {
    // Upstream's manual-update shutdown has no auth at all. Sharing the gate here means a
    // second route cannot be used to bypass the first.
    {
        let _secret = SecretGuard::set(None);
        let (status, _) = post("/api/version/shutdown", Some("x"), true).await?;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
    {
        let _secret = SecretGuard::set(Some("s"));
        let (status, _) = post("/api/version/shutdown", Some("wrong"), true).await?;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    let _secret = SecretGuard::set(Some("s"));
    let (status, body) = shutdown_a_real_server("/api/version/shutdown", "s").await?;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["stopping"], "nullrouter-api");
    // Distinct message, same mechanism: this one is the "stop so I can replace the files" route.
    let message = body["message"].as_str().unwrap_or_default();
    assert!(message.contains("update"), "{message:?}");
    Ok(())
}

#[actix_web::test]
async fn a_self_replacing_update_is_refused_with_a_reason() -> TestResult {
    let (status, body) = post("/api/version/update", None, true).await?;

    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(body["success"], false);
    assert_eq!(body["unsupported"], true);
    // The compiled version is reported even though the update is refused: the caller asked
    // what is running, and that part is answerable.
    assert_eq!(body["currentVersion"], "0.5.20");
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("does not replace its own binary"),
        "the refusal must give the reason: {message:?}"
    );
    // And it should point somewhere useful rather than just refusing.
    assert!(
        message.contains("/api/version/shutdown"),
        "the refusal should name the route that does help: {message:?}"
    );
    Ok(())
}

#[actix_web::test]
async fn the_version_route_does_not_claim_to_be_up_to_date() -> TestResult {
    // No update channel is checked, so `latestVersion` must be null rather than echoing the
    // current version, and `hasUpdate` must not be a claim derived from nothing.
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(StateClient::new(UNREACHABLE)))
            .app_data(web::Data::new(RuntimeClient::new(UNREACHABLE)))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::default()
        .method(Method::GET)
        .uri("/api/version")
        .to_request();
    let res = test::call_service(&app, req).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body: Value = serde_json::from_str(&String::from_utf8(
        to_bytes(res.into_body()).await?.to_vec(),
    )?)?;

    assert_eq!(body["currentVersion"], "0.5.20");
    assert!(
        body["latestVersion"].is_null(),
        "latestVersion should be null when nothing was checked: {body}"
    );
    assert_eq!(body["hasUpdate"], false);
    Ok(())
}
