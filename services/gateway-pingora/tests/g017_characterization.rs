use nullrouter_gateway::{GatewayConfig, RouteKind};

#[test]
fn existing_route_precedence_is_preserved_before_auth_boundary_changes() {
    // Given: the gateway has the route ownership established through G016.
    let config = GatewayConfig::default();

    for (path, expected) in [
        ("/api/usage/stream", RouteKind::Events),
        ("/v1/chat/completions", RouteKind::Runtime),
        ("/api/catalog/providers", RouteKind::Catalog),
        ("/api/keys/key_1", RouteKind::State),
        ("/api/translator/load", RouteKind::Api),
        ("/dashboard/translator", RouteKind::Dashboard),
    ] {
        // When: an existing route is classified.
        let actual = config.route_for_path(path);

        // Then: its established upstream owner remains unchanged.
        assert_eq!(actual, expected, "{path}");
    }
}
