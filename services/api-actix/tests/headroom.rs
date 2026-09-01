//! Contract tests for the headroom extras and restart surface.
//!
//! The property every test here defends is the same one: this build detects for
//! real and mutates nothing. A `GET` may only report what the host actually
//! holds, and a `POST` must refuse in a way a caller cannot mistake for success
//! — because a user who believes compression is active, and is not, pays for
//! full-size requests without knowing it.
//!
//! The `GET` assertions are deliberately shape-and-invariant assertions rather
//! than value assertions: whether this machine has Python 3.10+, or
//! `headroom-ai`, is a property of the machine. Asserting either way would make
//! the suite pass or fail on the developer's environment instead of on the code.

#![allow(
    clippy::future_not_send,
    reason = "actix test services are single-threaded and not Send"
)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test assertions read clearer with direct unwrap than with error plumbing"
)]

use actix_web::{
    App,
    body::to_bytes,
    http::{Method, StatusCode, header},
    test, web,
};
use serde_json::Value;

use nullrouter_api::{AppConfig, RuntimeClient, StateClient, TunnelManager, configure};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A closed loopback port: nothing here reads state or runtime, so the clients
/// exist only to satisfy the app's `app_data`.
const UNREACHABLE_STATE_ADDR: &str = "127.0.0.1:1";

const fn app_config() -> AppConfig {
    AppConfig::new("0.5.20")
}

async fn request_json(method: Method, uri: &str, body: &str) -> TestResult<(StatusCode, Value)> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(StateClient::new(UNREACHABLE_STATE_ADDR)))
            .app_data(web::Data::new(RuntimeClient::new(UNREACHABLE_STATE_ADDR)))
            .app_data(web::Data::new(TunnelManager::new()))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::default()
        .method(method)
        .uri(uri)
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(body.to_owned())
        .to_request();

    let res = test::call_service(&app, req).await;
    let status = res.status();
    let body = to_bytes(res.into_body()).await?;
    let json = serde_json::from_slice(&body)?;
    Ok((status, json))
}

async fn get_json(uri: &str) -> TestResult<(StatusCode, Value)> {
    request_json(Method::GET, uri, "").await
}

async fn post_json(uri: &str, body: &str) -> TestResult<(StatusCode, Value)> {
    request_json(Method::POST, uri, body).await
}

fn field<'a>(json: &'a Value, name: &str) -> TestResult<&'a Value> {
    json.get(name)
        .ok_or_else(|| test_error(format!("missing field {name}")))
}

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}

#[actix_rt::test]
async fn extras_reports_the_available_compression_extras_and_python_detection() -> TestResult {
    // Given: whatever Python and headroom-ai this machine happens to hold.

    // When: the dashboard asks what compression extras exist and which are on.
    let (status, body) = get_json("/api/headroom/extras").await?;

    // Then: the upstream shape is present, with the closed extras list.
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        field(&body, "available")?,
        &serde_json::json!(["code", "ml"])
    );
    assert!(field(&body, "installed")?.is_boolean());
    // `version` is a string or null — never absent, never a fabricated "0.0.0".
    let version = field(&body, "version")?;
    assert!(
        version.is_null() || version.is_string(),
        "version should be a string or null, got {version}"
    );

    // Every advertised extra has a reported state, so no row renders as unknown.
    let extras = field(&body, "extras")?
        .as_object()
        .ok_or_else(|| test_error("extras should be an object"))?;
    for extra in ["code", "ml"] {
        let state = extras
            .get(extra)
            .ok_or_else(|| test_error(format!("extras.{extra} missing")))?;
        assert!(state.is_boolean(), "extras.{extra} should be a boolean");
    }

    // Python detection is reported as a path or an explicit null, alongside the
    // minimum this build requires, so the panel can name what is missing.
    let python = field(&body, "python")?;
    assert!(
        python.is_null() || python.is_string(),
        "python should be a path or null, got {python}"
    );
    assert_eq!(field(&body, "pythonMinVersion")?, "3.10");

    // An extra cannot be installed while headroom-ai is not: that combination
    // would be a self-contradictory report.
    if !field(&body, "installed")?.as_bool().unwrap_or(false) {
        for extra in ["code", "ml"] {
            assert_eq!(
                extras.get(extra),
                Some(&Value::Bool(false)),
                "extras.{extra} claims installed while headroom-ai is not"
            );
        }
        assert!(field(&body, "version")?.is_null());
    }

    // The refusals are advertised before any button is pressed, so the panel can
    // render an unsupported state instead of a control that does nothing.
    assert_eq!(field(&body, "installSupported")?, false);
    assert_eq!(field(&body, "restartSupported")?, false);
    assert!(
        field(&body, "installMessage")?
            .as_str()
            .is_some_and(|message| !message.is_empty())
    );
    Ok(())
}

#[actix_rt::test]
async fn extras_log_query_returns_the_install_log_tail() -> TestResult {
    // Given: this build never writes an install log, but one may exist from
    // upstream 9Router sharing the same data directory.

    // When: the panel polls for install progress the way upstream's UI does.
    let (status, body) = get_json("/api/headroom/extras?log=1").await?;

    // Then: a `log` string is always present — empty when no log exists, which
    // is upstream's behaviour for an absent file.
    assert_eq!(status, StatusCode::OK);
    let log = field(&body, "log")?;
    assert!(log.is_string(), "log should be a string, got {log}");
    // The extras report must not leak into the log branch: `?log=1` answers
    // only the log, as upstream's early return does.
    assert!(body.get("available").is_none());

    // Where it was read from is stated, so an empty log is explainable.
    let path = field(&body, "logPath")?;
    assert!(path.is_null() || path.is_string());
    if path.is_null() {
        assert_eq!(log, "", "no log file, so there can be no log content");
    }
    Ok(())
}

#[actix_rt::test]
async fn extras_ignores_an_unrelated_query_and_reports_detection() -> TestResult {
    // Given: only `log=1` selects the log branch upstream.

    // When: another value is passed.
    let (status, body) = get_json("/api/headroom/extras?log=0&unknown=x").await?;

    // Then: the detection report is returned, not the log.
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        field(&body, "available")?,
        &serde_json::json!(["code", "ml"])
    );
    assert!(body.get("log").is_none());
    Ok(())
}

#[actix_rt::test]
async fn install_rejects_a_malformed_body_before_answering_about_extras() -> TestResult {
    // Given: a caller sends JSON that does not parse.

    // When: the install endpoint receives it.
    let (status, body) = post_json("/api/headroom/extras", "{\"extras\": [").await?;

    // Then: it is a bad request, not a considered refusal — the request was
    // never understood, so nothing about extras is reported.
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(field(&body, "error")?, "Invalid JSON body");
    assert!(body.get("spec").is_none());
    Ok(())
}

#[actix_rt::test]
async fn install_refuses_an_empty_extras_list_instead_of_reporting_success() -> TestResult {
    // Given: upstream would still install the `proxy` base for an empty list, so
    // "empty" is not the same as "nothing to do".

    // When: an empty list is posted.
    let (status, body) = post_json("/api/headroom/extras", r#"{"extras":[]}"#).await?;

    // Then: the refusal is explicit, and the base spec that was NOT installed is
    // named. `success:false` is the load-bearing assertion: an empty-list
    // "success" would tell a user their install completed.
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(field(&body, "success")?, false);
    assert_eq!(field(&body, "unsupported")?, true);
    assert_eq!(field(&body, "code")?, "UNSUPPORTED");
    assert_eq!(field(&body, "requested")?, &serde_json::json!([]));
    assert_eq!(field(&body, "spec")?, "headroom-ai[proxy]");
    Ok(())
}

#[actix_rt::test]
async fn install_refuses_a_requested_extra_and_names_what_was_not_installed() -> TestResult {
    // Given: a real install request for both compression extras plus a name this
    // build does not track.

    // When: it is posted.
    let (status, body) = post_json(
        "/api/headroom/extras",
        r#"{"extras":["ml","image","code"]}"#,
    )
    .await?;

    // Then: nothing was installed, and the answer says so in a way a client
    // cannot read as a success.
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(field(&body, "success")?, false);
    assert_eq!(field(&body, "unsupported")?, true);
    // The recognised extras are echoed, and the unrecognised one is called out
    // rather than silently dropped.
    assert_eq!(
        field(&body, "requested")?,
        &serde_json::json!(["ml", "code"])
    );
    assert_eq!(field(&body, "ignored")?, &serde_json::json!(["image"]));
    // The requirement the user can run themselves, with the proxy base first.
    assert_eq!(field(&body, "spec")?, "headroom-ai[proxy,ml,code]");
    // No field may claim an installed state as a result of this call.
    assert!(body.get("installed").is_none());
    assert!(
        field(&body, "error")?
            .as_str()
            .is_some_and(|error| !error.is_empty())
    );
    Ok(())
}

#[actix_rt::test]
async fn install_tolerates_a_body_with_no_extras_field() -> TestResult {
    // Given: upstream treats a missing or wrongly-typed `extras` as "none
    // requested" rather than an error.

    // When: bodies without a usable list arrive, including an absent body.
    let (empty_status, empty) = post_json("/api/headroom/extras", "{}").await?;
    let (typed_status, typed) = post_json("/api/headroom/extras", r#"{"extras":"ml"}"#).await?;
    let (absent_status, absent) = post_json("/api/headroom/extras", "").await?;

    // Then: each is a refusal with no extras requested — never a 400, and never
    // a success.
    for (status, body) in [
        (empty_status, &empty),
        (typed_status, &typed),
        (absent_status, &absent),
    ] {
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
        assert_eq!(field(body, "success")?, false);
        assert_eq!(field(body, "requested")?, &serde_json::json!([]));
    }
    Ok(())
}

#[actix_rt::test]
async fn restart_refuses_explicitly_and_names_the_url_it_judged() -> TestResult {
    // Given: no HEADROOM_URL override in the test environment, so the default
    // loopback URL applies and upstream's external-proxy check passes.

    // When: a restart is requested.
    let (status, body) = post_json("/api/headroom/restart", "").await?;

    // Then: the refusal is explicit. `success:false` at 501 is what stops the
    // panel from telling a user compression was restarted when no process was
    // signalled.
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(field(&body, "success")?, false);
    assert_eq!(field(&body, "unsupported")?, true);
    assert_eq!(field(&body, "code")?, "UNSUPPORTED");
    // The URL and port that were evaluated, so the message is checkable.
    assert_eq!(field(&body, "url")?, "http://localhost:8787");
    assert_eq!(field(&body, "port")?, 8787);
    assert!(
        field(&body, "error")?
            .as_str()
            .is_some_and(|error| !error.is_empty())
    );
    Ok(())
}

#[actix_rt::test]
async fn process_control_routes_never_report_a_mutation_that_did_not_happen() -> TestResult {
    // Given: every mutating headroom route in this build is a refusal.

    // When: each is called.
    let (extras_status, extras) = post_json("/api/headroom/extras", r#"{"extras":["ml"]}"#).await?;
    let (restart_status, restart) = post_json("/api/headroom/restart", "").await?;
    let (start_status, start) = post_json("/api/headroom/start", "").await?;
    let (stop_status, stop) = post_json("/api/headroom/stop", "").await?;

    // Then: none of them is a 2xx, and none of them carries `success:true`.
    // This is the single invariant that keeps a user from being billed for
    // uncompressed requests they believe are compressed.
    for (status, body) in [
        (extras_status, &extras),
        (restart_status, &restart),
        (start_status, &start),
        (stop_status, &stop),
    ] {
        assert!(
            status.is_client_error() || status.is_server_error(),
            "a refused mutation must not answer 2xx, got {status}"
        );
        assert_eq!(field(body, "success")?, false);
        assert_eq!(field(body, "unsupported")?, true);
    }
    Ok(())
}

#[actix_rt::test]
async fn extras_answers_preflight_so_the_dashboard_can_post() -> TestResult {
    // Given: the dashboard posts JSON cross-origin in some deployments.

    // When: the browser preflights the extras and restart routes.
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(StateClient::new(UNREACHABLE_STATE_ADDR)))
            .app_data(web::Data::new(RuntimeClient::new(UNREACHABLE_STATE_ADDR)))
            .app_data(web::Data::new(TunnelManager::new()))
            .configure(configure),
    )
    .await;
    for uri in ["/api/headroom/extras", "/api/headroom/restart"] {
        let req = test::TestRequest::default()
            .method(Method::OPTIONS)
            .uri(uri)
            .to_request();
        let res = test::call_service(&app, req).await;

        // Then: preflight succeeds with no body.
        assert_eq!(res.status(), StatusCode::NO_CONTENT, "{uri}");
        assert!(
            res.headers()
                .contains_key(header::ACCESS_CONTROL_ALLOW_METHODS),
            "{uri} preflight is missing the allowed methods"
        );
    }
    Ok(())
}
