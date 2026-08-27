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
async fn dashboard_shell_css_exposes_frozen_layout_and_primitive_tokens() -> TestResult {
    // Given: the CSS modules consumed by every hosted dashboard route.
    let app =
        test::init_service(App::new().configure(DashboardConfig::default().into_configurer()))
            .await;
    let cases = [
        (
            "/assets/dashboard/base.css",
            [
                "font-family: \"Inter\"",
                "--shadow-elev:",
                "--shadow-focus:",
            ]
            .as_slice(),
        ),
        (
            "/assets/dashboard/sidebar.css",
            ["width: 288px", ".nr-media-navigation", ".nr-media-nav-item"].as_slice(),
        ),
        (
            "/assets/dashboard/workspace.css",
            [
                ".nr-header-control",
                ".nr-header-popover",
                ".nr-search-empty",
                ".nr-search-field input::-webkit-search-cancel-button",
            ]
            .as_slice(),
        ),
        (
            "/assets/dashboard/cards.css",
            [
                "border-radius: 14px",
                "box-shadow: var(--shadow-soft)",
                "grid-template-columns: auto minmax(0, 1fr) 32px",
            ]
            .as_slice(),
        ),
        (
            "/assets/dashboard/responsive.css",
            ["@media (max-width: 1023px)", "@media (min-width: 1024px)"].as_slice(),
        ),
        (
            "/assets/dashboard/top-pages.css",
            [".nr-top-page .nr-button"].as_slice(),
        ),
    ];

    // When: each stylesheet is requested through the Actix asset boundary.
    // Then: source-faithful shell and primitive markers are present.
    for (path, expected) in cases {
        let response =
            test::call_service(&app, test::TestRequest::get().uri(path).to_request()).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        let body = to_bytes(response.into_body()).await?;
        let css = std::str::from_utf8(&body)?;
        for marker in expected {
            assert!(css.contains(marker), "missing {marker:?} in {path}");
        }
    }
    Ok(())
}
