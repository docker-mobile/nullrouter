//! Readiness reports whether this host can serve, not merely whether it is running.
//!
//! The 503 case is the one worth testing. A probe that can only answer 200 is decoration: it would
//! have reported a healthy dashboard throughout the two occasions in this project's history when
//! every route returned 200 and rendered a blank page.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "free helpers in an integration test are not covered by clippy.toml's \
              allow-expect-in-tests, which only reaches #[test] functions"
)]

use actix_web::{App, body::to_bytes, http::StatusCode, test};
use nullrouter_dashboard_host::DashboardConfig;
use serde_json::Value;
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn write(root: &std::path::Path, relative: &str, bytes: &[u8]) -> std::io::Result<()> {
    let target = root.join(relative);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(target, bytes)
}

/// Every asset readiness requires, so a case can remove exactly one.
fn complete_root() -> Result<TempDir, Box<dyn std::error::Error>> {
    let root = TempDir::new()?;
    write(root.path(), "pkg/dashboard_leptos_bg.wasm", b"\0asm")?;
    write(root.path(), "assets/dashboard/app.css", b":root{}")?;
    Ok(root)
}

async fn probe(config: DashboardConfig, path: &str) -> (StatusCode, Value) {
    let app = test::init_service(App::new().configure(config.into_configurer())).await;
    let response = test::call_service(&app, test::TestRequest::get().uri(path).to_request()).await;
    let status = response.status();
    let bytes = to_bytes(response.into_body()).await.unwrap_or_default();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

#[actix_web::test]
async fn a_complete_deployment_is_ready() -> TestResult {
    let root = complete_root()?;
    for path in ["/readyz", "/ready"] {
        let (status, body) = probe(DashboardConfig::new(root.path()), path).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
        assert_eq!(body.get("ready"), Some(&Value::Bool(true)), "{path}");
    }
    Ok(())
}

#[actix_web::test]
async fn a_missing_bundle_is_not_ready_and_says_which_file() -> TestResult {
    let root = complete_root()?;
    std::fs::remove_file(root.path().join("pkg/dashboard_leptos_bg.wasm"))?;

    let (status, body) = probe(DashboardConfig::new(root.path()), "/readyz").await;

    // 503, so a Kubernetes readiness probe fails and traffic stops arriving. A 200 here is the bug
    // this test exists to prevent: the dashboard would answer every request with a blank page.
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body.get("ready"), Some(&Value::Bool(false)));

    // Naming the file is the difference between "not ready" and an operator knowing which build step
    // was skipped.
    let missing = body
        .get("missing")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_default();
    assert!(
        missing.contains("dashboard_leptos_bg.wasm"),
        "readiness did not name the missing bundle: {body}"
    );
    assert!(
        !missing.contains("app.css"),
        "a present file was reported missing: {body}"
    );
    Ok(())
}

#[actix_web::test]
async fn a_missing_stylesheet_is_not_ready() -> TestResult {
    // Checked separately from the bundle: a host that mounts the app but has no token layer renders
    // unstyled, which reads as broken to a user and would otherwise pass a bundle-only check.
    let root = complete_root()?;
    std::fs::remove_file(root.path().join("assets/dashboard/app.css"))?;

    let (status, body) = probe(DashboardConfig::new(root.path()), "/readyz").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    Ok(())
}

#[actix_web::test]
async fn liveness_stays_independent_of_readiness() -> TestResult {
    // An unready instance is still alive. If `/health` failed here too, an orchestrator would
    // restart a process whose problem is a missing file, forever, and never route traffic to a
    // replacement that has the same missing file.
    let root = TempDir::new()?;
    let app = test::init_service(
        App::new().configure(DashboardConfig::new(root.path()).into_configurer()),
    )
    .await;

    let health =
        test::call_service(&app, test::TestRequest::get().uri("/health").to_request()).await;
    assert_eq!(
        health.status(),
        StatusCode::OK,
        "liveness must not depend on assets"
    );

    let ready =
        test::call_service(&app, test::TestRequest::get().uri("/readyz").to_request()).await;
    assert_eq!(ready.status(), StatusCode::SERVICE_UNAVAILABLE);
    Ok(())
}
