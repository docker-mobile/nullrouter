use actix_web::{App, body::to_bytes, http::StatusCode, test};
use nullrouter_dashboard_host::DashboardConfig;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The fonts the shell preloads.
///
/// `inter-latin.woff2` is the interface face and the stylesheet has a matching `@font-face` for it. The
/// Material Symbols icon fonts used to be listed here; the dashboard now draws every icon as inline SVG
/// geometry from `apps/dashboard-leptos/src/ui/icon.rs`, so nothing references those files and preloading
/// them was a 33 KB request for a font no element set. They remain on disk because deleting a binary asset
/// is outside what this change is for.
const FONT_ASSETS: [&str; 1] = ["/assets/fonts/inter-latin.woff2"];

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

    // Then: each font is preloaded and served as a nonempty WOFF2 asset.
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
