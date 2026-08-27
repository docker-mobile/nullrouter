use nullrouter_gateway::{GatewayConfig, RouteKind};

#[test]
fn g016_shell_routes_and_assets_remain_owned_by_dashboard_host() {
    // Given: the public one-port gateway uses its default microservice map.
    let config = GatewayConfig::default();
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
        "/assets/dashboard/sidebar.css",
        "/assets/dashboard/workspace.css",
        "/assets/fonts/inter-latin.woff2",
        "/assets/fonts/material-symbols-g016.woff2",
    ];

    // When: shell pages, CSS modules, and local fonts are classified.
    // Then: every request remains on nullrouter-dashboard-host behind Pingora.
    for path in paths {
        assert_eq!(config.route_for_path(path), RouteKind::Dashboard, "{path}");
    }
}
