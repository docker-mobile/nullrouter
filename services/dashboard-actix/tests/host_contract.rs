use actix_web::{
    App,
    body::to_bytes,
    http::{StatusCode, header},
    test,
};
use nullrouter_dashboard_host::DashboardConfig;
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn write_file(root: &std::path::Path, path: &str, contents: &[u8]) -> std::io::Result<()> {
    let destination = root.join(path);
    let parent = destination.parent().unwrap_or(root);
    std::fs::create_dir_all(parent)?;
    std::fs::write(destination, contents)
}

fn fixture_config() -> Result<(TempDir, DashboardConfig), Box<dyn std::error::Error>> {
    let root = TempDir::new()?;
    write_file(
        root.path(),
        "pkg/dashboard_leptos.js",
        b"export function hydrate() {}\n",
    )?;
    write_file(root.path(), "pkg/dashboard_leptos_bg.wasm", b"\0asm")?;
    write_file(root.path(), "providers/openai.png", b"\x89PNG\r\n\x1a\n")?;
    write_file(
        root.path(),
        "assets/favicon.svg",
        br#"<svg xmlns="http://www.w3.org/2000/svg"/>"#,
    )?;
    let config = DashboardConfig::new(root.path());
    Ok((root, config))
}

#[actix_web::test]
async fn health_returns_service_status_when_requested() -> TestResult {
    let (_root, config) = fixture_config()?;
    let app = test::init_service(App::new().configure(config.into_configurer())).await;

    let response =
        test::call_service(&app, test::TestRequest::get().uri("/health").to_request()).await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body())
        .await
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
    let value: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(value.get("ok"), Some(&serde_json::json!(true)));
    assert_eq!(
        value.get("service"),
        Some(&serde_json::json!("nullrouter-dashboard-host"))
    );
    Ok(())
}

#[actix_web::test]
async fn fallback_routes_return_html_bootstrap_when_requested() -> TestResult {
    let (_root, config) = fixture_config()?;
    let app = test::init_service(App::new().configure(config.into_configurer())).await;

    for path in [
        "/dashboard",
        "/dashboard/endpoint",
        "/dashboard/providers",
        "/dashboard/providers/new",
        "/dashboard/providers/openai",
        "/dashboard/providers/not-real",
        "/dashboard/media-providers/web",
        "/dashboard/media-providers/web/",
        "/dashboard/media-providers/embedding",
        "/dashboard/media-providers/embedding/",
        "/dashboard/media-providers/embedding/openai",
        "/dashboard/media-providers/embedding/openai/",
        "/dashboard/media-providers/embedding/not-real",
        "/dashboard/media-providers/tts/openai",
        "/dashboard/media-providers/tts/openai/",
        "/dashboard/media-providers/tts/not-real",
        "/dashboard/media-providers/combo/combo_1",
        "/dashboard/media-providers/combo/combo_1/",
        "/dashboard/media-providers/combo/not-real",
        "/dashboard/media-providers/not-real/openai",
        "/dashboard/usage",
        "/dashboard/combos",
        "/dashboard/cli-tools",
        "/dashboard/cli-tools/codex",
        "/dashboard/cli-tools/not-real",
        "/dashboard/quota",
        "/dashboard/token-saver",
        "/dashboard/skills",
        "/dashboard/settings",
        "/dashboard/settings/pricing",
        "/dashboard/profile",
        "/dashboard/basic-chat",
        "/dashboard/proxy-pools",
        "/dashboard/translator",
        "/dashboard/console-log",
        "/dashboard/mitm",
    ] {
        let response =
            test::call_service(&app, test::TestRequest::get().uri(path).to_request()).await;

        assert_eq!(response.status(), StatusCode::OK, "{path}");
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        assert!(content_type.starts_with("text/html"), "{path}");
        let body = to_bytes(response.into_body())
            .await
            .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
        let html = std::str::from_utf8(&body)?;
        assert!(html.contains(r#"import init from "/pkg/dashboard_leptos.js";"#));
        assert!(html.contains(r#"await init("/pkg/dashboard_leptos_bg.wasm");"#));
        assert!(html.contains(r#"<link rel="modulepreload" href="/pkg/dashboard_leptos.js">"#));
        assert!(html.contains(r#"<link rel="preload" href="/pkg/dashboard_leptos_bg.wasm" as="fetch" type="application/wasm" crossorigin>"#));
        assert!(html.contains(r#"<div id="dashboard-root"></div>"#));
        assert!(html.contains("nullrouter"));
        // No trace of the old branding in anything served to a browser. Checked
        // case-insensitively so neither spelling can creep back into the shell.
        assert!(
            !html.to_ascii_lowercase().contains("9router"),
            "the shell must not carry the old branding"
        );
    }
    Ok(())
}

#[actix_web::test]
async fn pkg_assets_are_served_from_static_pkg_when_requested() -> TestResult {
    let (_root, config) = fixture_config()?;
    let app = test::init_service(App::new().configure(config.into_configurer())).await;

    let js = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/pkg/dashboard_leptos.js")
            .to_request(),
    )
    .await;
    assert_eq!(js.status(), StatusCode::OK);
    assert_eq!(
        js.headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/javascript")
    );

    let wasm = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/pkg/dashboard_leptos_bg.wasm")
            .to_request(),
    )
    .await;
    assert_eq!(wasm.status(), StatusCode::OK);
    assert_eq!(
        wasm.headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/wasm")
    );
    Ok(())
}

#[actix_web::test]
async fn provider_and_static_assets_are_served_when_requested() -> TestResult {
    let (_root, config) = fixture_config()?;
    let app = test::init_service(App::new().configure(config.into_configurer())).await;

    let provider = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/providers/openai.png")
            .to_request(),
    )
    .await;
    assert_eq!(provider.status(), StatusCode::OK);
    assert_eq!(
        provider
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("image/png")
    );

    let asset = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/assets/favicon.svg")
            .to_request(),
    )
    .await;
    assert_eq!(asset.status(), StatusCode::OK);
    assert_eq!(
        asset
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("image/svg+xml")
    );
    Ok(())
}

#[actix_web::test]
async fn root_favicon_alias_serves_next_public_asset_when_requested() -> TestResult {
    let (_root, config) = fixture_config()?;
    let app = test::init_service(App::new().configure(config.into_configurer())).await;

    let response = test::call_service(
        &app,
        test::TestRequest::get().uri("/favicon.svg").to_request(),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("image/svg+xml")
    );
    assert_eq!(
        response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("public, max-age=0, must-revalidate")
    );
    Ok(())
}

#[actix_web::test]
async fn api_routes_are_not_owned_by_dashboard_host_when_requested() -> TestResult {
    let (_root, config) = fixture_config()?;
    let app = test::init_service(App::new().configure(config.into_configurer())).await;

    for path in [
        "/api/health",
        "/api/providers/new",
        "/api/providers/openai",
        "/api/cli-tools/codex",
        "/api/dashboard/providers/new",
    ] {
        let response =
            test::call_service(&app, test::TestRequest::get().uri(path).to_request()).await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
    }
    Ok(())
}
