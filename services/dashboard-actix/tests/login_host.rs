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

#[actix_web::test]
async fn login_shell_preserves_frozen_host_boundaries_when_requested() -> TestResult {
    // Given: the standalone dashboard host with its normal top-level routes.
    let root = TempDir::new()?;
    let config = DashboardConfig::new(root.path());
    let app = test::init_service(App::new().configure(config.into_configurer())).await;

    // When: a browser requests the branded login page.
    let login = html_for!(app, "/login");

    // Then: the password form and auth endpoints are present without dashboard WASM.
    assert!(login.contains("<title>9Router Login</title>"));
    assert!(login.contains(
        r#"<form id="password-form" class="nr-auth-form" method="post" action="/api/auth/login">"#
    ));
    assert!(login.contains(
        r#"<input id="password" name="password" type="password" autocomplete="current-password""#
    ));
    assert!(login.contains(r#"name="newPassword" type="password""#));
    assert!(login.contains(r#"id="auth-error" class="nr-auth-error" role="alert""#));
    assert!(login.contains(r#"id="login-button" class="nr-button nr-button-primary""#));
    assert!(login.contains(r#""/api/auth/status""#));
    assert!(login.contains(r#""/api/auth/login""#));
    assert!(login.contains("/api/auth/oidc/start"));
    assert!(!login.contains(r#"import init from "/pkg/dashboard_leptos.js";"#));
    assert!(!login.contains(r#"<div id="dashboard-root"></div>"#));
    Ok(())
}

#[actix_web::test]
async fn login_uses_cookie_aware_safe_dashboard_redirect_when_rendered() -> TestResult {
    // Given: a login request carrying an untrusted post-authentication target.
    let root = TempDir::new()?;
    let config = DashboardConfig::new(root.path());
    let app = test::init_service(App::new().configure(config.into_configurer())).await;

    // When: the dashboard host renders the login shell.
    let login = html_for!(app, "/login?next=https%3A%2F%2Fevil.example%2Fdashboard");

    // Then: cookies are explicit and navigation is constrained to this origin's dashboard.
    assert!(login.matches(r#"credentials: "same-origin""#).count() >= 2);
    // An existing session is the ONLY thing that skips this screen. Honouring a
    // `requireLogin: false` from the status body would be an auth bypass driven
    // by a JSON field, so the check must stay on `authenticated` alone.
    assert!(login.contains("status.authenticated === true"));
    assert!(
        !login.contains("requireLogin === false"),
        "login must not skip auth on a requireLogin flag"
    );
    assert!(login.contains(r#"new URLSearchParams(window.location.search).get("next")"#));
    assert!(login.contains("new URL(requestedTarget, window.location.origin)"));
    assert!(login.contains("target.origin !== window.location.origin"));
    assert!(login.contains(r#"target.pathname === "/dashboard""#));
    assert!(login.contains(r#"target.pathname.startsWith("/dashboard/")"#));
    assert!(login.contains("return `${target.pathname}${target.search}${target.hash}`;"));
    assert!(login.contains("window.location.replace(dashboardTarget());"));
    assert!(login.matches("redirectToDashboard();").count() >= 2);
    assert!(!login.contains("window.location.assign("));
    Ok(())
}

#[actix_web::test]
async fn login_bounds_auth_errors_and_submission_states_when_rejected() -> TestResult {
    // Given: the branded login shell served independently of the auth service.
    let root = TempDir::new()?;
    let config = DashboardConfig::new(root.path());
    let app = test::init_service(App::new().configure(config.into_configurer())).await;

    // When: the browser-side auth contract is inspected.
    let login = html_for!(app, "/login");

    // Then: failures are status-shaped, bounded, and cannot trigger duplicate submissions.
    assert!(login.contains("let submitting = false;"));
    assert!(login.contains("let retryAfter = 0;"));
    assert!(login.contains("let retryInterval = 0;"));
    assert!(login.contains("if (submitting || retryAfter > 0) return;"));
    assert!(login.contains("button.disabled = submitting || retryAfter > 0;"));
    assert!(login.contains("response.status === 401"));
    assert!(login.contains("response.status === 429"));
    assert!(login.contains(r#"response.headers.get("Retry-After")"#));
    assert!(login.contains("Math.min(Math.max(Math.ceil(seconds), 0), 3600)"));
    assert!(login.contains("window.setInterval"));
    assert!(login.contains("retryAfter -= 1;"));
    assert!(login.contains("Invalid password."));
    assert!(login.contains("Too many failed attempts. Try again later."));
    assert!(login.contains("Sign-in service is unavailable. Please try again."));
    assert!(login.contains("const controller = new AbortController();"));
    assert!(login.contains("controller.abort()"));
    assert!(!login.contains("setError(data.error"));
    assert!(!login.contains("hint.textContent = data.resetHint"));
    Ok(())
}
