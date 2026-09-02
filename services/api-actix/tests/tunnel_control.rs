//! The tunnel control surface as a caller sees it.
//!
//! Two things are being pinned here. First, that the operation catalog is genuinely the whole
//! surface: no request body can reach a subcommand that is not a row in it. Second, that a
//! missing binary is reported as a missing binary — the sandbox this runs in has neither
//! `cloudflared` nor `tailscale`, which makes it the right place to check that the
//! not-installed path is informative rather than a generic failure.
// actix's test service is single-threaded by design, so nothing here is `Send`. Same allow as
// every other route suite in this crate.
#![allow(clippy::future_not_send)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "free helpers in an integration test are not covered by clippy.toml's \
              allow-expect-in-tests, which only reaches #[test] functions"
)]

use actix_web::http::{Method, StatusCode, header};
use actix_web::{App, test, web};
use nullrouter_api::{AppConfig, RuntimeClient, StateClient, TunnelManager, configure};
use serde_json::Value;

/// A port nothing listens on, so no test reaches a real dependency.
const UNREACHABLE_STATE_ADDR: &str = "http://127.0.0.1:1";

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// Send one request through the full route table.
async fn call(method: Method, uri: &str, body: &str) -> TestResult<(StatusCode, Value)> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(AppConfig::new("0.5.20")))
            .app_data(web::Data::new(StateClient::new(UNREACHABLE_STATE_ADDR)))
            .app_data(web::Data::new(RuntimeClient::new(UNREACHABLE_STATE_ADDR)))
            .app_data(web::Data::new(TunnelManager::new()))
            .configure(configure),
    )
    .await;
    let request = test::TestRequest::default()
        .method(method)
        .uri(uri)
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(body.to_owned())
        .to_request();
    let response = test::call_service(&app, request).await;
    let status = response.status();
    let bytes = test::read_body(response).await;
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .map_err(|error| format!("{uri} returned unparseable body: {error}"))?
    };
    Ok((status, json))
}

/// GET a route.
async fn get(uri: &str) -> TestResult<(StatusCode, Value)> {
    call(Method::GET, uri, "").await
}

/// POST a route.
async fn post(uri: &str, body: &str) -> TestResult<(StatusCode, Value)> {
    call(Method::POST, uri, body).await
}

#[actix_rt::test]
async fn the_operation_catalog_is_discoverable() -> TestResult {
    // Given: a panel that wants to know what it may ask these binaries to do.

    // When: the catalog is listed.
    let (status, body) = get("/api/tunnel/operations").await?;

    // Then: every row describes itself well enough to render, including whether it mutates.
    assert_eq!(status, StatusCode::OK);
    let operations = body
        .get("operations")
        .and_then(Value::as_array)
        .ok_or("no operations array")?;
    assert!(operations.len() >= 12, "only {} rows", operations.len());

    for entry in operations {
        for required in ["id", "about", "tool", "effect", "mode", "available"] {
            assert!(
                entry.get(required).is_some(),
                "a row is missing {required}: {entry}"
            );
        }
        let effect = entry.get("effect").and_then(Value::as_str);
        assert!(
            matches!(effect, Some("read" | "mutate")),
            "unrenderable effect {effect:?}"
        );
    }

    // And both tools are reported, so a panel can explain unavailability once.
    let tools = body
        .get("tools")
        .and_then(Value::as_array)
        .ok_or("no tools")?;
    let names: Vec<&str> = tools
        .iter()
        .filter_map(|tool| tool.get("id").and_then(Value::as_str))
        .collect();
    assert!(names.contains(&"cloudflared"), "{names:?}");
    assert!(names.contains(&"tailscale"), "{names:?}");
    Ok(())
}

#[actix_rt::test]
async fn a_credential_parameter_is_marked_so_a_panel_can_hide_it() -> TestResult {
    // Given: the named-tunnel operation takes a Cloudflare token.

    // When: the catalog is listed.
    let (_status, body) = get("/api/tunnel/operations").await?;

    // Then: the token is flagged secret, so a panel renders a password field and does not log
    // or persist it. Upstream has no such marking because it puts the token in argv.
    let operations = body
        .get("operations")
        .and_then(Value::as_array)
        .ok_or("no operations")?;
    let named = operations
        .iter()
        .find(|entry| entry.get("id").and_then(Value::as_str) == Some("cloudflared.tunnel.run"))
        .ok_or("no named tunnel row")?;
    let token = named
        .get("params")
        .and_then(Value::as_array)
        .and_then(|params| params.first())
        .ok_or("no params")?;

    assert_eq!(token.get("name").and_then(Value::as_str), Some("token"));
    assert_eq!(token.get("secret"), Some(&Value::Bool(true)));
    assert_eq!(token.get("required"), Some(&Value::Bool(true)));
    Ok(())
}

#[actix_rt::test]
async fn an_operation_that_is_not_in_the_catalog_is_refused() -> TestResult {
    // Given: the catalog is the entire surface.

    // When: ids that are not rows are posted, including ones shaped like an injection.
    for hostile in [
        "cloudflared.access.ssh",
        "tailscale.file.cp",
        "cloudflared.tunnel.run;id",
        "..%2f..%2fbin%2fsh",
        "TAILSCALE.STATUS",
        "tailscale",
    ] {
        let uri = format!("/api/tunnel/operations/{hostile}");
        let (status, body) = post(&uri, "{}").await?;

        // Then: it is a bad request, never an attempt. 404 would be acceptable from actix's
        // own matching, but anything 2xx would mean something ran.
        assert!(
            status == StatusCode::BAD_REQUEST || status == StatusCode::NOT_FOUND,
            "{hostile} returned {status}: {body}"
        );
    }
    Ok(())
}

#[actix_rt::test]
async fn a_required_parameter_is_demanded_before_anything_runs() -> TestResult {
    // Given: `tailscale.cert` cannot mean anything without a hostname.

    // When: it is called with no arguments.
    let (status, body) = post("/api/tunnel/operations/tailscale.cert", "{}").await?;

    // Then: the missing parameter is named, and no process was started to find out.
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body.get("message")
            .and_then(Value::as_str)
            .is_some_and(|message| message.contains("hostname")),
        "{body}"
    );
    Ok(())
}

#[actix_rt::test]
async fn an_undeclared_parameter_is_dropped_rather_than_forwarded() -> TestResult {
    // Given: a body carrying a name no operation declares.

    // When: a read operation is called with it. cloudflared is absent here, so the answer is
    // 503; what matters is that the extra name changed nothing about how far it got.
    let (status, body) = post(
        "/api/tunnel/operations/cloudflared.version",
        r#"{"args":{"config":"/etc/passwd","--help":"1","port":"22"}}"#,
    )
    .await?;

    // Then: it failed on the missing binary, not on the stray arguments.
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .ok_or("no message")?;
    assert!(message.contains("cloudflared"), "{message}");
    assert!(!message.contains("/etc/passwd"), "{message}");
    Ok(())
}

#[actix_rt::test]
async fn a_missing_binary_is_reported_as_a_dependency_not_a_bad_request() -> TestResult {
    // Given: neither binary is installed in this sandbox.

    // When: one operation per tool is called.
    let (cloudflare_status, cloudflare) =
        post("/api/tunnel/operations/cloudflared.version", "{}").await?;
    let (tailscale_status, tailscale) =
        post("/api/tunnel/operations/tailscale.version", "{}").await?;

    // Then: both are 503 with the program named. A 400 would blame the caller for the
    // operator's environment, and a 500 would say nothing at all.
    for (status, body, program) in [
        (cloudflare_status, &cloudflare, "cloudflared"),
        (tailscale_status, &tailscale, "tailscale"),
    ] {
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert!(
            body.get("message")
                .and_then(Value::as_str)
                .is_some_and(|message| message.contains(program)),
            "{body}"
        );
    }
    Ok(())
}

#[actix_rt::test]
async fn the_status_route_reports_structure_even_with_nothing_installed() -> TestResult {
    // Given: nothing is installed and no tunnel has ever been started.

    // When: status is requested.
    let (status, body) = get("/api/tunnel/status").await?;

    // Then: every field a panel binds to is present and false rather than absent.
    assert_eq!(status, StatusCode::OK);
    let tunnel = body.get("tunnel").ok_or("no tunnel section")?;
    assert_eq!(tunnel.get("enabled"), Some(&Value::Bool(false)));
    assert_eq!(tunnel.get("running"), Some(&Value::Bool(false)));
    assert_eq!(tunnel.get("installed"), Some(&Value::Bool(false)));
    assert_eq!(tunnel.get("state").and_then(Value::as_str), Some("stopped"));
    assert_eq!(tunnel.get("url").and_then(Value::as_str), Some(""));
    assert_eq!(tunnel.get("restarts"), Some(&Value::from(0)));

    let tailscale = body.get("tailscale").ok_or("no tailscale section")?;
    assert_eq!(tailscale.get("installed"), Some(&Value::Bool(false)));
    assert_eq!(tailscale.get("loggedIn"), Some(&Value::Bool(false)));
    assert_eq!(tailscale.get("funnelActive"), Some(&Value::Bool(false)));

    // And the download section says why no download will ever be in progress.
    let download = body.get("download").ok_or("no download section")?;
    assert_eq!(download.get("inProgress"), Some(&Value::Bool(false)));
    assert!(
        download
            .get("message")
            .and_then(Value::as_str)
            .is_some_and(|message| message.contains("never downloads")),
        "{download}"
    );
    Ok(())
}

#[actix_rt::test]
async fn the_tailscale_check_route_never_claims_a_password_or_a_brew() -> TestResult {
    // Given: upstream offers `brew install tailscale` and caches a sudo password to run
    // tailscaled in TUN mode. This port does neither, and the payload has to say so rather
    // than leave a panel offering buttons that cannot work.

    // When: the check is requested.
    let (status, body) = get("/api/tunnel/tailscale-check").await?;

    // Then: both capabilities are reported absent, permanently.
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.get("brewAvailable"), Some(&Value::Bool(false)));
    assert_eq!(body.get("hasCachedPassword"), Some(&Value::Bool(false)));
    assert_eq!(body.get("installed"), Some(&Value::Bool(false)));
    assert_eq!(body.get("daemonInstalled"), Some(&Value::Bool(false)));
    // The platform is reported honestly, because an install hint depends on it.
    assert!(
        body.get("platform")
            .and_then(Value::as_str)
            .is_some_and(|platform| !platform.is_empty()),
        "{body}"
    );
    Ok(())
}

#[actix_rt::test]
async fn a_named_tunnel_requires_a_token_and_never_echoes_it() -> TestResult {
    // Given: the named tunnel takes a credential.

    // When: it is called with no token, then with one.
    let (empty_status, empty) = post("/api/tunnel/named/enable", r#"{"token":""}"#).await?;
    let (with_status, with) = post(
        "/api/tunnel/named/enable",
        r#"{"token":"SECRET-VALUE-12345"}"#,
    )
    .await?;

    // Then: an empty token is a bad request, and a supplied one is never reflected back —
    // not in the success path and not in the missing-binary failure.
    assert_eq!(empty_status, StatusCode::BAD_REQUEST, "{empty}");
    assert_eq!(with_status, StatusCode::SERVICE_UNAVAILABLE, "{with}");
    let rendered = with.to_string();
    assert!(
        !rendered.contains("SECRET-VALUE-12345"),
        "the token was echoed back: {rendered}"
    );
    Ok(())
}

#[actix_rt::test]
async fn disable_is_safe_to_call_when_nothing_is_running() -> TestResult {
    // Given: no tunnel was ever started. Upstream's disable path runs `pkill` regardless,
    // which is how it kills processes it never started.

    // When: disable is called twice.
    let (first_status, first) = post("/api/tunnel/disable", "").await?;
    let (second_status, second) = post("/api/tunnel/disable", "").await?;

    // Then: both succeed and report that nothing was running.
    assert_eq!(first_status, StatusCode::OK, "{first}");
    assert_eq!(second_status, StatusCode::OK, "{second}");
    assert_eq!(first.get("success"), Some(&Value::Bool(true)));
    assert!(
        first
            .get("message")
            .and_then(Value::as_str)
            .is_some_and(|message| message.contains("NotRunning")),
        "{first}"
    );
    Ok(())
}

#[actix_rt::test]
async fn installing_system_software_stays_refused_with_a_reason() -> TestResult {
    // Given: upstream installs Tailscale by piping a downloaded script into `sudo sh` with the
    // user's password on the child's stdin.

    // When: the install route is called.
    let (status, body) = post("/api/tunnel/tailscale-install", "{}").await?;

    // Then: 501, and the refusal explains itself and says what to do instead.
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(body.get("success"), Some(&Value::Bool(false)));
    assert_eq!(body.get("unsupported"), Some(&Value::Bool(true)));
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .ok_or("no message")?;
    assert!(message.contains("sudo"), "{message}");
    assert!(
        body.get("hint")
            .and_then(Value::as_str)
            .is_some_and(|hint| hint.contains("tailscale.com/download")),
        "{body}"
    );
    Ok(())
}

#[actix_rt::test]
async fn a_hostile_parameter_value_never_reaches_a_process() -> TestResult {
    // Given: every value that could become an argument goes through a charset allowlist.

    // When: shell-shaped values are posted to operations that take parameters.
    for hostile in [
        "$(id)",
        "a;rm -rf /",
        "--config=/etc/shadow",
        "`id`",
        "a\nb",
        "../../etc/passwd",
    ] {
        let body = format!(
            r#"{{"args":{{"hostname":{},"port":{}}}}}"#,
            serde_json::to_string(hostile)?,
            serde_json::to_string(hostile)?
        );
        let (status, response) = post("/api/tunnel/operations/tailscale.cert", &body).await?;

        // Then: it is refused, and the refusal is about the value rather than a process
        // result. A 503 would mean the argv was built and only the binary was missing.
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{hostile:?} got past validation: {response}"
        );
    }
    Ok(())
}

#[actix_rt::test]
async fn an_empty_body_means_defaults_but_a_malformed_one_is_an_error() -> TestResult {
    // Given: the dashboard POSTs these routes with no body at all, so empty has to mean
    // defaults. The trap is treating an unreadable body the same way: a silently defaulted
    // port is worse than either a clear error or a respected value, because the caller is
    // told the operation ran with what they asked for when it did not.

    // When: the body is empty, whitespace, valid, and malformed in turn.
    let (empty_status, _empty) = post("/api/tunnel/enable", "").await?;
    let (blank_status, _blank) = post("/api/tunnel/enable", "  \n ").await?;
    let (valid_status, _valid) = post("/api/tunnel/enable", r#"{"port":20131}"#).await?;
    let (malformed_status, malformed) = post("/api/tunnel/enable", r#"{"port":"twenty"}"#).await?;
    let (garbage_status, garbage) = post("/api/tunnel/enable", "not json at all").await?;

    // Then: the first three reach the missing binary, and the last two are rejected as bad
    // requests rather than quietly defaulted.
    assert_eq!(empty_status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(blank_status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(valid_status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(malformed_status, StatusCode::BAD_REQUEST, "{malformed}");
    assert_eq!(garbage_status, StatusCode::BAD_REQUEST, "{garbage}");
    assert!(
        malformed
            .get("message")
            .and_then(Value::as_str)
            .is_some_and(|message| message.contains("invalid request body")),
        "{malformed}"
    );
    Ok(())
}

#[actix_rt::test]
async fn a_quick_tunnel_port_must_be_a_port() -> TestResult {
    // Given: the only caller-chosen part of a quick tunnel is which loopback port to expose.

    // When: a body carries something that is not a port.
    let (bad_status, bad) = post("/api/tunnel/enable", r#"{"port":"not-a-port"}"#).await?;
    // And when it carries a real one.
    let (good_status, good) = post("/api/tunnel/enable", r#"{"port":20131}"#).await?;

    // Then: the malformed body is rejected by deserialisation, and the valid one gets as far
    // as the missing binary. Neither can choose a host.
    assert_eq!(bad_status, StatusCode::BAD_REQUEST, "{bad}");
    assert_eq!(good_status, StatusCode::SERVICE_UNAVAILABLE, "{good}");
    Ok(())
}
