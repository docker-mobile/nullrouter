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
async fn basic_chat_serves_wasm_bootstrap_when_requested() -> TestResult {
    // Given: the dashboard host has the built Leptos package available.
    let (_root, config) = fixture_config()?;
    let app = test::init_service(App::new().configure(config.into_configurer())).await;

    // When: the Basic Chat dashboard route is requested directly.
    let response = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/dashboard/basic-chat")
            .to_request(),
    )
    .await;

    // Then: the host returns the dashboard HTML bootstrap, not a static 404.
    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    assert!(content_type.starts_with("text/html"));
    let body = to_bytes(response.into_body())
        .await
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
    let html = std::str::from_utf8(&body)?;
    assert!(html.contains(r#"import init from "/pkg/dashboard_leptos.js";"#));
    assert!(html.contains(r#"await init("/pkg/dashboard_leptos_bg.wasm");"#));
    assert!(
        html.contains(
            r#"<link rel="preload" href="/pkg/dashboard_leptos_bg.wasm" as="fetch" type="application/wasm" crossorigin>"#
        )
    );
    assert!(html.contains(r#"<div id="dashboard-root"></div>"#));
    Ok(())
}

#[actix_web::test]
async fn chat_completions_api_is_not_dashboard_html_when_posted() -> TestResult {
    // Given: only the dashboard host service is mounted.
    let (_root, config) = fixture_config()?;
    let app = test::init_service(App::new().configure(config.into_configurer())).await;

    // When: the dashboard chat API path receives the request shape owned by API/runtime.
    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/dashboard/chat/completions")
            .insert_header((header::CONTENT_TYPE, "application/json"))
            .set_payload(
                r#"{"model":"openai:gpt-4o-mini","stream":true,"messages":[{"role":"user","content":"hello"}]}"#,
            )
            .to_request(),
    )
    .await;

    // Then: the host does not answer with dashboard fallback HTML.
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    assert!(!content_type.starts_with("text/html"));
    let body = to_bytes(response.into_body())
        .await
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
    let text = std::str::from_utf8(&body)?;
    assert!(!text.contains(r#"import init from "/pkg/dashboard_leptos.js";"#));
    assert!(!text.contains(r#"<div id="dashboard-root"></div>"#));
    Ok(())
}
