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
    // The quickstart command, which has to be one that actually works: this page used to show an
    // `npx` invocation, and there is no root `package.json`, so it would have failed for anyone who
    // typed it. `./run.sh` is the entry point this repository really ships.
    assert!(landing.contains("$ ./run.sh"));
    assert!(
        !landing.contains("npx"),
        "no npx install path exists in this repository"
    );
    assert!(landing.contains("http://localhost:20128"));
    assert!(!landing.contains(r#"import init from "/pkg/dashboard_leptos.js";"#));
    assert!(!landing.contains(r#"<div id="dashboard-root"></div>"#));

    // The login screen is served by the WASM bundle, so the host owes only the
    // mount point. Its copy and endpoints are asserted against the bundle's own
    // visible contract in `apps/dashboard-leptos`, and its behaviour in
    // `tests/login_live.rs`; checking them here would be checking a shell that no
    // longer carries them.
    let login = html_for!(app, "/login");
    assert!(login.contains(r#"import init from "/pkg/dashboard_leptos.js";"#));
    assert!(login.contains("<title>nullrouter Login</title>"));
    assert!(
        !login.contains("<form"),
        "a fallback form would bypass the bundle's redirect sanitiser"
    );

    // The callback screen is served by the WASM bundle too. Its relay channels and
    // panels are asserted against the bundle's visible contract, and the
    // relay-origin decision — which is the security-relevant part, since the
    // payload is an authorization code — in `callback_live`'s own tests.
    let callback = html_for!(app, "/callback?code=ok&state=s1");
    assert!(callback.contains(r#"import init from "/pkg/dashboard_leptos.js";"#));
    assert!(callback.contains("<title>nullrouter OAuth Callback</title>"));
    // The manual-copy fallback needs no script to be useful, so it stays in the
    // shell. Nothing else about the flow does.
    assert!(callback.contains("Copy This URL"));
    for absent in ["BroadcastChannel", "localStorage", "postMessage", "1455"] {
        assert!(
            !callback.contains(absent),
            "{absent} should no longer be implemented in the shell"
        );
    }

    let dashboard = html_for!(app, "/dashboard/settings");
    assert!(dashboard.contains(r#"import init from "/pkg/dashboard_leptos.js";"#));
    assert!(dashboard.contains(r#"<div id="dashboard-root"></div>"#));
    Ok(())
}

#[actix_web::test]
async fn top_level_pages_keep_boundary_states_safe_when_requested() -> TestResult {
    let (_root, config) = fixture_config()?;
    let app = test::init_service(App::new().configure(config.into_configurer())).await;

    // The callback screen runs from the WASM bundle, so the shell is the same bytes
    // whatever the provider sent back. The panel states and the relay's origin
    // restriction — the part that decides who receives an authorization code — are
    // asserted in `apps/dashboard-leptos/tests/callback_live.rs`.
    for path in ["/callback", "/callback?error=access_denied"] {
        let callback = html_for!(app, path);
        assert!(callback.contains(r#"import init from "/pkg/dashboard_leptos.js";"#));
        // The manual-copy fallback works without script, so it stays in the shell.
        assert!(callback.contains("Copy This URL"));
        // Nothing about the grant may be reflected into the served page.
        assert!(!callback.contains("access_denied"), "{path}");
        assert!(!callback.contains("postMessage"), "{path}");
        assert!(!callback.contains("1455"), "{path}");
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
