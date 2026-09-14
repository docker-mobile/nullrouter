use actix_web::{App, body::to_bytes, http::StatusCode, test};
use nullrouter_dashboard_host::DashboardConfig;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const FONT_ASSETS: [&str; 2] = [
    "/assets/fonts/inter-latin.woff2",
    "/assets/fonts/material-symbols-g016.woff2",
];

#[actix_web::test]
async fn dashboard_shell_preloads_and_serves_local_fonts() -> TestResult {
    // Given: the checked-in dashboard host and its local static assets.
    let app =
        test::init_service(App::new().configure(DashboardConfig::default().into_configurer()))
            .await;

    // When: the shell HTML and each declared font are requested.
    let html_response = test::call_service(
        &app,
        test::TestRequest::get().uri("/dashboard").to_request(),
    )
    .await;
    let html = to_bytes(html_response.into_body()).await?;
    let html = std::str::from_utf8(&html)?;

    // Then: both fonts are preloaded and served as nonempty WOFF2 assets.
    for font in FONT_ASSETS {
        assert!(
            html.contains(&format!(
                r#"<link rel="preload" href="{font}" as="font" type="font/woff2" crossorigin>"#
            )),
            "missing font preload: {font}"
        );
        let response =
            test::call_service(&app, test::TestRequest::get().uri(font).to_request()).await;
        assert_eq!(response.status(), StatusCode::OK, "{font}");
        let body = to_bytes(response.into_body()).await?;
        assert!(body.starts_with(b"wOF2"), "invalid WOFF2 asset: {font}");
        assert!(
            body.len() > 3_000,
            "font asset is unexpectedly small: {font}"
        );
    }
    Ok(())
}

#[actix_web::test]
async fn dashboard_shell_css_exposes_current_token_layer() -> TestResult {
    // Given: the compiled stylesheet the WASM dashboard mounts.
    let app =
        test::init_service(App::new().configure(DashboardConfig::default().into_configurer()))
            .await;
    let response = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/assets/dashboard/app.css")
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body()).await?;
    let css = std::str::from_utf8(&body)?;
    for marker in [
        "--background:",
        "--sidebar:",
        ".dark",
        "prefers-reduced-motion",
        "bg-background",
    ] {
        assert!(css.contains(marker), "missing {marker:?} in app.css");
    }
    Ok(())
}
