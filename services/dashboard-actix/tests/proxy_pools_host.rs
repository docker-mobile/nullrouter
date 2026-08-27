use actix_web::{
    App,
    body::to_bytes,
    http::{StatusCode, header},
    test,
};
use nullrouter_dashboard_host::DashboardConfig;
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

macro_rules! dashboard_shell_for {
    ($app:expr, $path:expr) => {{
        let path = $path;
        let response =
            test::call_service(&$app, test::TestRequest::get().uri(path).to_request()).await;

        assert_eq!(response.status(), StatusCode::OK, "{path}");
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        assert!(content_type.starts_with("text/html"), "{path}");
        assert!(!content_type.starts_with("application/json"), "{path}");

        let body = to_bytes(response.into_body())
            .await
            .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
        let html = std::str::from_utf8(&body)?;
        assert!(
            html.contains("import init from \"/pkg/dashboard_leptos.js\";"),
            "{path}"
        );
        assert!(
            html.contains("await init(\"/pkg/dashboard_leptos_bg.wasm\");"),
            "{path}"
        );
        assert!(
            html.contains("<link rel=\"modulepreload\" href=\"/pkg/dashboard_leptos.js\">"),
            "{path}"
        );
        assert!(html.contains("<div id=\"dashboard-root\"></div>"), "{path}");
        assert!(!html.trim_start().starts_with('{'), "{path}");
        html.to_owned()
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
async fn proxy_pools_dashboard_serves_wasm_shell_when_requested() -> TestResult {
    // Given: the dashboard host has the built Leptos package available.
    let (_root, config) = fixture_config()?;
    let app = test::init_service(App::new().configure(config.into_configurer())).await;

    // When: the Proxy Pools dashboard route is requested directly.
    let html = dashboard_shell_for!(app, "/dashboard/proxy-pools");

    // Then: the host returns the dashboard bootstrap, not API JSON or a 404 page.
    assert!(html.contains("Endpoint &amp; Key"));
    assert!(html.contains("data-dashboard-host=\"actix\""));
    Ok(())
}

#[actix_web::test]
async fn proxy_pools_api_routes_are_not_dashboard_pages_when_requested() -> TestResult {
    // Given: only the dashboard host service is mounted.
    let (_root, config) = fixture_config()?;
    let app = test::init_service(App::new().configure(config.into_configurer())).await;

    for path in ["/api/proxy-pools", "/api/proxy-pools/vercel-deploy"] {
        // When: a Proxy Pools API path is requested against the dashboard host.
        let response =
            test::call_service(&app, test::TestRequest::get().uri(path).to_request()).await;

        // Then: the host leaves the API route unowned instead of serving the dashboard shell.
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        assert!(!content_type.starts_with("text/html"), "{path}");
        let body = to_bytes(response.into_body())
            .await
            .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
        let text = std::str::from_utf8(&body)?;
        assert!(
            !text.contains("import init from \"/pkg/dashboard_leptos.js\";"),
            "{path}"
        );
        assert!(
            !text.contains("<div id=\"dashboard-root\"></div>"),
            "{path}"
        );
    }
    Ok(())
}

#[actix_web::test]
async fn existing_dashboard_routes_keep_serving_wasm_shell_when_requested() -> TestResult {
    // Given: the dashboard host has the built Leptos package available.
    let (_root, config) = fixture_config()?;
    let app = test::init_service(App::new().configure(config.into_configurer())).await;

    for path in [
        "/dashboard/basic-chat",
        "/dashboard/providers/openai",
        "/dashboard/media-providers/embedding",
    ] {
        // When: an existing dashboard route is requested directly.
        let html = dashboard_shell_for!(app, path);

        // Then: the route still serves the WASM dashboard bootstrap.
        assert!(html.contains("Endpoint &amp; Key"), "{path}");
    }
    Ok(())
}
