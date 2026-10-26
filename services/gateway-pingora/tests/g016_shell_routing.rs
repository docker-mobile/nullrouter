use nullrouter_gateway::{GatewayConfig, RouteKind};

#[test]
fn g016_shell_routes_and_assets_remain_owned_by_dashboard_host() {
    // Given: the public one-port gateway uses its default microservice map.
    let config = GatewayConfig::default();
    // Only assets that are actually committed. `sidebar.css` and `workspace.css` were listed here and
    // exist in no directory: the assertion passed anyway, because the gateway classifies `/assets/` by
    // prefix, so it could not tell a served file from a missing one. A test that cannot fail for the reason
    // it appears to test is worse than no test.
    let paths = [
        "/dashboard",
        "/dashboard/providers",
        "/dashboard/media-providers/embedding",
        "/dashboard/media-providers/image",
        "/dashboard/media-providers/tts",
        "/dashboard/media-providers/stt",
        "/dashboard/media-providers/web",
        "/dashboard/mitm",
        "/assets/dashboard.css",
        "/assets/dashboard/app.css",
        "/assets/fonts/inter-latin.woff2",
    ];

    // When: shell pages, the stylesheet, and the interface font are classified.
    // Then: every request remains on nullrouter-dashboard-host behind Pingora.
    for path in paths {
        assert_eq!(config.route_for_path(path), RouteKind::Dashboard, "{path}");
    }
}
