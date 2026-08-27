use nullrouter_gateway::{GatewayConfig, RouteKind};

#[test]
fn proxy_pools_dashboard_shell_routes_to_dashboard() {
    // Given: proxy-pools has a dashboard shell route.
    let config = GatewayConfig::default();

    // When: the public dashboard route is requested.
    let route = config.route_for_path("/dashboard/proxy-pools");

    // Then: the gateway sends it to the dashboard upstream.
    assert_eq!(route, RouteKind::Dashboard);
}

#[test]
fn proxy_pools_crud_routes_to_state() {
    // Given: proxy-pools collection and item CRUD are state-service owned.
    let config = GatewayConfig::default();

    for path in ["/api/proxy-pools", "/api/proxy-pools/pool_1"] {
        // When: a proxy-pools CRUD route is requested.
        let route = config.route_for_path(path);

        // Then: it goes to state before the generic API fallback.
        assert_eq!(route, RouteKind::State, "{path}");
    }
}

#[test]
fn proxy_pools_test_route_stays_on_api() {
    // Given: proxy-pools test execution is API-service owned.
    let config = GatewayConfig::default();

    // When: an item test route is requested.
    let route = config.route_for_path("/api/proxy-pools/pool_1/test");

    // Then: it remains on the API upstream instead of state.
    assert_eq!(route, RouteKind::Api);
}

#[test]
fn proxy_pools_deploy_routes_stay_on_api() {
    // Given: proxy-pools deploy actions are API-service owned.
    let config = GatewayConfig::default();

    for path in [
        "/api/proxy-pools/vercel-deploy",
        "/api/proxy-pools/cloudflare-deploy",
        "/api/proxy-pools/deno-deploy",
    ] {
        // When: a deploy action route is requested.
        let route = config.route_for_path(path);

        // Then: it remains on the API upstream instead of state.
        assert_eq!(route, RouteKind::Api, "{path}");
    }
}

#[test]
fn runtime_and_events_routes_keep_existing_owners() {
    // Given: proxy-pools parity must not steal runtime or event routes.
    let config = GatewayConfig::default();

    for (path, expected) in [
        ("/v1/chat/completions", RouteKind::Runtime),
        ("/api/usage/stream", RouteKind::Events),
    ] {
        // When: an existing runtime or event route is requested.
        let route = config.route_for_path(path);

        // Then: it keeps its existing upstream owner.
        assert_eq!(route, expected, "{path}");
    }
}
