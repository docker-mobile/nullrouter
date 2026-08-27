use nullrouter_gateway::{GatewayConfig, RouteKind};

#[test]
fn translator_dashboard_shell_routes_to_dashboard() {
    // Given: translator has a dashboard shell route.
    let config = GatewayConfig::default();

    // When: the public translator dashboard route is requested.
    let route = config.route_for_path("/dashboard/translator");

    // Then: it is served by the dashboard upstream.
    assert_eq!(route, RouteKind::Dashboard);
}

#[test]
fn translator_action_routes_stay_on_api() {
    // Given: translator action endpoints are API-service owned.
    let config = GatewayConfig::default();

    for path in [
        "/api/translator/load",
        "/api/translator/save",
        "/api/translator/translate",
        "/api/translator/send",
        "/api/translator/console-logs",
    ] {
        // When: a translator action route is requested.
        let route = config.route_for_path(path);

        // Then: it remains on the API upstream instead of dashboard or events.
        assert_eq!(route, RouteKind::Api, "{path}");
    }
}

#[test]
fn translator_console_log_stream_routes_to_events() {
    // Given: translator console-log streaming is event-service owned.
    let config = GatewayConfig::default();

    // When: the translator console-log stream endpoint is requested.
    let route = config.route_for_path("/api/translator/console-logs/stream");

    // Then: it is routed before generic API matching.
    assert_eq!(route, RouteKind::Events);
}

#[test]
fn translator_parity_keeps_existing_route_owners() {
    // Given: translator parity must not disturb completed gateway routes.
    let config = GatewayConfig::default();

    for (path, expected) in [
        ("/api/usage/stream", RouteKind::Events),
        ("/v1/chat/completions", RouteKind::Runtime),
        ("/dashboard/proxy-pools", RouteKind::Dashboard),
    ] {
        // When: an existing route is requested.
        let route = config.route_for_path(path);

        // Then: it keeps its previously assigned upstream owner.
        assert_eq!(route, expected, "{path}");
    }
}
