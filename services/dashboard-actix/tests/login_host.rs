//! The sign-in shell's host boundary.
//!
//! This page used to carry its whole implementation as inline JavaScript, and these
//! tests asserted that script's source text. The logic now lives in the Leptos/WASM
//! bundle, so what the host owes is narrower and checked here: serve a mount point,
//! carry no application logic of its own, and say something useful when WASM cannot
//! run. The behaviour those old assertions protected — the redirect sanitiser, the
//! auth-skip rule, bounded lockout countdowns, status-shaped errors — is covered
//! directly in `apps/dashboard-leptos/tests/login_live.rs`.

use actix_web::{
    App,
    body::to_bytes,
    http::{StatusCode, header},
    test,
};
use nullrouter_dashboard_host::DashboardConfig;
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

macro_rules! html_for {
    ($app:expr, $path:expr) => {{
        let response =
            test::call_service(&$app, test::TestRequest::get().uri($path).to_request()).await;
        assert_eq!(response.status(), StatusCode::OK, "{}", $path);
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        assert!(content_type.starts_with("text/html"), "{}", $path);
        let body = to_bytes(response.into_body())
            .await
            .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
        std::str::from_utf8(&body)?.to_owned()
    }};
}

/// The `<script>` contents of a page, concatenated.
fn scripts(html: &str) -> String {
    let mut collected = String::new();
    let mut rest = html;
    while let Some(open) = rest.find("<script") {
        let Some(after_tag) = rest.get(open..).and_then(|tail| tail.find('>')) else {
            break;
        };
        let body_start = open + after_tag + 1;
        let Some(tail) = rest.get(body_start..) else {
            break;
        };
        let Some(close) = tail.find("</script>") else {
            break;
        };
        collected.push_str(tail.get(..close).unwrap_or_default());
        rest = tail.get(close + "</script>".len()..).unwrap_or_default();
    }
    collected
}

#[actix_web::test]
async fn login_shell_mounts_the_wasm_bundle() -> TestResult {
    // Given: the standalone dashboard host with its normal top-level routes.
    let root = TempDir::new()?;
    let config = DashboardConfig::new(root.path());
    let app = test::init_service(App::new().configure(config.into_configurer())).await;

    // When: a browser requests the login page.
    let login = html_for!(app, "/login");

    // Then: it is a shell for the same bundle the dashboard uses. `/pkg/*` is
    // public at the gateway, which is what lets this load before a session exists.
    assert!(login.contains(r#"import init from "/pkg/dashboard_leptos.js";"#));
    assert!(login.contains(r#"await init("/pkg/dashboard_leptos_bg.wasm");"#));
    assert!(login.contains(r#"<link rel="modulepreload" href="/pkg/dashboard_leptos.js">"#));
    assert!(login.contains("<title>nullrouter Login</title>"));
    assert!(login.contains(r#"<link rel="stylesheet" href="/assets/dashboard.css">"#));
    Ok(())
}

#[actix_web::test]
async fn login_shell_carries_no_application_logic_of_its_own() -> TestResult {
    // Given: the login screen's logic moved into the WASM bundle. Any of it left
    // behind here would be a second implementation to keep in step — and the one
    // that is not type-checked.
    let root = TempDir::new()?;
    let config = DashboardConfig::new(root.path());
    let app = test::init_service(App::new().configure(config.into_configurer())).await;

    // When: the shell's scripts are inspected.
    let login = html_for!(app, "/login");
    let script = scripts(&login);

    // Then: the module that boots the bundle is still two statements. Counted on its own rather
    // than over every script on the page, because the theme bootstrap is also inline and has to
    // be: it sets the colour scheme before the first paint, which is the one thing that cannot
    // wait for a wasm bundle to arrive over the network. It carries no application logic, and the
    // absent-substring checks below run against the whole page either way.
    let boot = script
        .split_once("import init")
        .map(|(_, rest)| format!("import init{rest}"))
        .unwrap_or_default();
    assert!(
        boot.lines().filter(|line| !line.trim().is_empty()).count() <= 3,
        "the boot script should be two statements, got: {boot}"
    );
    // And: the only inline script beside it is the pre-paint theme bootstrap.
    assert!(
        script.contains("prefers-color-scheme"),
        "the theme bootstrap should still run before first paint: {script}"
    );
    for absent in [
        // Fetching and submitting.
        "fetch(",
        "addEventListener",
        "preventDefault",
        // The state machine that used to live here.
        "submitting",
        "retryAfter",
        "retryInterval",
        "setInterval",
        "mustChangePassword",
        // Redirect handling — the part that must not be duplicated.
        "URLSearchParams",
        "window.location.replace",
        "window.location.assign",
        "dashboardTarget",
        // Error mapping.
        "Invalid password",
        "response.status",
    ] {
        assert!(
            !script.contains(absent),
            "{absent} is still implemented in the shell's script: {script}"
        );
    }
    Ok(())
}

#[actix_web::test]
async fn login_shell_offers_no_form_that_would_bypass_the_sanitiser() -> TestResult {
    // Given: a plain HTML form posting to /api/auth/login would work without WASM —
    // and would skip the redirect sanitiser and the lockout countdown entirely.
    // A degraded sign-in that silently loses those is worse than none.
    let root = TempDir::new()?;
    let config = DashboardConfig::new(root.path());
    let app = test::init_service(App::new().configure(config.into_configurer())).await;

    // When: the shell is served.
    let login = html_for!(app, "/login");

    // Then: there is no server-posting form, and the reason the screen is empty is
    // stated rather than left blank.
    assert!(
        !login.contains("<form"),
        "the shell must not carry a fallback form: {login}"
    );
    assert!(login.contains("<noscript>"));
    assert!(login.contains("Sign-in needs JavaScript and WebAssembly enabled."));
    Ok(())
}

#[actix_web::test]
async fn an_untrusted_next_parameter_changes_nothing_about_the_shell() -> TestResult {
    // Given: `?next=` is attacker-controllable. The shell must not reflect it into
    // the page at all — sanitising happens in the bundle, from `window.location`,
    // so there is nothing here for a hostile value to reach.
    let root = TempDir::new()?;
    let config = DashboardConfig::new(root.path());
    let app = test::init_service(App::new().configure(config.into_configurer())).await;

    // When: the page is requested with a hostile target and without one.
    let hostile = html_for!(
        app,
        "/login?next=https%3A%2F%2Fevil.example%2Fdashboard&x=%3Cscript%3E"
    );
    let plain = html_for!(app, "/login");

    // Then: the served bytes are identical, so the parameter cannot influence it.
    assert_eq!(
        hostile, plain,
        "the shell must be independent of its query string"
    );
    assert!(!hostile.contains("evil.example"));
    Ok(())
}
