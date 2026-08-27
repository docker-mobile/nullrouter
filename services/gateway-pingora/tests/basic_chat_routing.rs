use nullrouter_gateway::{GatewayConfig, RouteKind};

#[test]
fn basic_chat_shell_routes_to_dashboard() {
    // Given: the gateway owns public routing for dashboard shell paths.
    let config = GatewayConfig::default();

    // When: the Basic Chat dashboard route is requested.
    let route = config.route_for_path("/dashboard/basic-chat");

    // Then: it is served by the dashboard host.
    assert_eq!(route, RouteKind::Dashboard);
}

#[test]
fn basic_chat_structured_endpoint_routes_to_api() {
    // Given: structured dashboard APIs remain on nullrouter-api.
    let config = GatewayConfig::default();

    // When: the Basic Chat structured endpoint is requested.
    let route = config.route_for_path("/api/dashboard/chat/completions");

    // Then: it is routed to the API service, not runtime or dashboard.
    assert_eq!(route, RouteKind::Api);
}

#[test]
fn openai_chat_entrypoints_route_to_runtime() {
    // Given: runtime owns OpenAI-compatible chat entrypoints.
    let config = GatewayConfig::default();

    for path in ["/v1/chat/completions", "/v1/responses", "/v1/messages"] {
        // When: a public runtime chat endpoint is requested.
        let route = config.route_for_path(path);

        // Then: it remains on the runtime upstream.
        assert_eq!(route, RouteKind::Runtime, "{path}");
    }
}

#[test]
fn usage_stream_stays_on_events() {
    // Given: usage streaming is event-service owned.
    let config = GatewayConfig::default();

    // When: the usage stream endpoint is requested.
    let route = config.route_for_path("/api/usage/stream");

    // Then: it is routed before generic API matching.
    assert_eq!(route, RouteKind::Events);
}

#[test]
fn state_and_catalog_routes_keep_existing_owners() {
    // Given: Basic Chat routing must not disturb state/catalog ownership.
    let config = GatewayConfig::default();

    for (path, expected) in [
        ("/api/catalog", RouteKind::Catalog),
        ("/api/catalog/providers", RouteKind::Catalog),
        ("/api/state", RouteKind::Catalog),
        ("/api/keys", RouteKind::State),
        ("/api/providers", RouteKind::State),
    ] {
        // When: an existing non-chat API route is requested.
        let route = config.route_for_path(path);

        // Then: it keeps the previously assigned upstream owner.
        assert_eq!(route, expected, "{path}");
    }
}
