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

fn write_file(root: &std::path::Path, path: &str, contents: &[u8]) -> std::io::Result<()> {
    let destination = root.join(path);
    let parent = destination.parent().unwrap_or(root);
    std::fs::create_dir_all(parent)?;
    std::fs::write(destination, contents)
}

fn fixture_config() -> Result<(TempDir, DashboardConfig), Box<dyn std::error::Error>> {
    let root = TempDir::new()?;
    write_file(root.path(), "assets/dashboard.css", b":root {}\n")?;
    write_file(root.path(), "assets/favicon.svg", b"<svg></svg>")?;
    write_file(
        root.path(),
        "pkg/dashboard_leptos.js",
        b"export default async function init() {}\n",
    )?;
    write_file(root.path(), "pkg/dashboard_leptos_bg.wasm", b"\0asm")?;
    let config = DashboardConfig::new(root.path());
    Ok((root, config))
}

#[actix_web::test]
async fn top_level_pages_match_upstream_contract_when_requested() -> TestResult {
    let (_root, config) = fixture_config()?;
    let app = test::init_service(App::new().configure(config.into_configurer())).await;

    let root = test::call_service(&app, test::TestRequest::get().uri("/").to_request()).await;
    assert_eq!(root.status(), StatusCode::FOUND);
    assert_eq!(
        root.headers()
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok()),
        Some("/dashboard")
    );

    let landing = html_for!(app, "/landing");
    assert!(landing.contains("One Endpoint for"));
    assert!(landing.contains("All AI Providers"));
    assert!(landing.contains("npx 9router"));
    assert!(landing.contains("http://localhost:20128"));
    assert!(!landing.contains(r#"import init from "/pkg/dashboard_leptos.js";"#));
    assert!(!landing.contains(r#"<div id="dashboard-root"></div>"#));

    let login = html_for!(app, "/login");
    assert!(login.contains("Enter your password"));
    assert!(login.contains(r#"type="password""#));
    assert!(login.contains("/api/auth/status"));
    assert!(login.contains("/api/auth/login"));
    assert!(login.contains("/api/auth/oidc/start"));
    assert!(!login.contains(r#"import init from "/pkg/dashboard_leptos.js";"#));

    let callback = html_for!(app, "/callback?code=ok&state=s1");
    assert!(callback.contains("Authorization Successful"));
    assert!(callback.contains("Copy This URL"));
    assert!(callback.contains("oauth_callback"));
    assert!(callback.contains("BroadcastChannel"));
    assert!(callback.contains("localStorage"));
    assert!(callback.contains("http://localhost:1455"));
    assert!(!callback.contains("postMessage(message, \"*\")"));
    assert!(!callback.contains(r#"import init from "/pkg/dashboard_leptos.js";"#));

    let dashboard = html_for!(app, "/dashboard/settings");
    assert!(dashboard.contains(r#"import init from "/pkg/dashboard_leptos.js";"#));
    assert!(dashboard.contains(r#"<div id="dashboard-root"></div>"#));
    Ok(())
}

#[actix_web::test]
async fn top_level_pages_keep_boundary_states_safe_when_requested() -> TestResult {
    let (_root, config) = fixture_config()?;
    let app = test::init_service(App::new().configure(config.into_configurer())).await;

    for path in ["/callback", "/callback?error=access_denied"] {
        let callback = html_for!(app, path);
        assert!(callback.contains("Processing"));
        assert!(callback.contains("Copy This URL"));
        assert!(callback.contains("errorDescription"));
        assert!(callback.contains("expectedOrigins"));
        assert!(callback.contains("window.location.origin"));
        assert!(callback.contains("http://localhost:1455"));
        assert!(
            !callback
                .contains("postMessage({ type: \"oauth_callback\", data: callbackData }, \"*\")")
        );
    }

    let api = test::call_service(
        &app,
        test::TestRequest::get().uri("/api/health").to_request(),
    )
    .await;
    assert_eq!(api.status(), StatusCode::NOT_FOUND);

    let traversal = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/assets/../../Cargo.toml")
            .to_request(),
    )
    .await;
    assert_eq!(traversal.status(), StatusCode::NOT_FOUND);
    Ok(())
}
