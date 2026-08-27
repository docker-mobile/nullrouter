use nullrouter_gateway::{GatewayConfig, RouteKind};

#[test]
fn console_log_dashboard_shell_routes_to_dashboard() {
    // Given: console log has a dashboard shell route.
    let config = GatewayConfig::default();

    // When: the public console-log dashboard route is requested.
    let route = config.route_for_path("/dashboard/console-log");

    // Then: it is served by the dashboard upstream.
    assert_eq!(route, RouteKind::Dashboard);
}

#[test]
fn console_log_collection_route_stays_on_api() {
    // Given: translator console-log collection endpoints are API-service owned.
    let config = GatewayConfig::default();

    // When: the console-log collection route is requested.
    let route = config.route_for_path("/api/translator/console-logs");

    // Then: it remains on the API upstream instead of events.
    assert_eq!(route, RouteKind::Api);
}

#[test]
fn console_log_stream_routes_to_events_before_generic_api() {
    // Given: translator console-log streaming is event-service owned.
    let config = GatewayConfig::default();

    // When: the console-log stream endpoint is requested.
    let route = config.route_for_path("/api/translator/console-logs/stream");

    // Then: it is routed before generic API matching.
    assert_eq!(route, RouteKind::Events);
}

#[test]
fn console_log_parity_keeps_existing_route_owners() {
    // Given: console-log parity must not disturb completed gateway routes.
    let config = GatewayConfig::default();

    for (path, expected) in [
        ("/dashboard/translator", RouteKind::Dashboard),
        ("/api/usage/stream", RouteKind::Events),
        ("/v1/models", RouteKind::Runtime),
    ] {
        // When: an existing route is requested.
        let route = config.route_for_path(path);

        // Then: it keeps its previously assigned upstream owner.
        assert_eq!(route, expected, "{path}");
    }
}
