use actix_web::{
    App,
    body::to_bytes,
    http::{StatusCode, header},
    test,
};
use nullrouter_dashboard_host::DashboardConfig;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const MITM_PROVIDER_ASSETS: [&str; 3] = ["antigravity.png", "copilot.png", "kiro.png"];

#[actix_web::test]
async fn mitm_dashboard_serves_wasm_bootstrap_when_requested() -> TestResult {
    // Given: the dashboard host uses its checked-in static assets.
    let app =
        test::init_service(App::new().configure(DashboardConfig::default().into_configurer()))
            .await;

    // When: the MITM dashboard route is requested directly.
    let response = test::call_service(
        &app,
        test::TestRequest::get().uri("/dashboard/mitm").to_request(),
    )
    .await;

    // Then: the host returns HTML containing the WASM bootstrap.
    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    assert!(content_type.starts_with("text/html"));
    let body = to_bytes(response.into_body()).await?;
    let html = std::str::from_utf8(&body)?;
    assert!(html.contains(r#"import init from "/pkg/dashboard_leptos.js";"#));
    assert!(html.contains(r#"await init("/pkg/dashboard_leptos_bg.wasm");"#));
    Ok(())
}

#[actix_web::test]
async fn mitm_provider_assets_serve_nonempty_pngs_when_requested() -> TestResult {
    // Given: the dashboard host uses its checked-in provider assets.
    let app =
        test::init_service(App::new().configure(DashboardConfig::default().into_configurer()))
            .await;

    for asset in MITM_PROVIDER_ASSETS {
        // When: a requested MITM provider image is fetched from the host.
        let response = test::call_service(
            &app,
            test::TestRequest::get()
                .uri(&format!("/providers/{asset}"))
                .to_request(),
        )
        .await;

        // Then: the image is a nonempty PNG response.
        assert_eq!(response.status(), StatusCode::OK, "{asset}");
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("image/png"),
            "{asset}"
        );
        let body = to_bytes(response.into_body()).await?;
        assert!(!body.is_empty(), "{asset}");
    }
    Ok(())
}
