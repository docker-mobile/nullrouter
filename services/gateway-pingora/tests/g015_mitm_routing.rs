use nullrouter_gateway::{GatewayConfig, RouteKind};

#[test]
fn mitm_routes_select_expected_upstreams() {
    // Given: the gateway uses its default routing configuration.
    let config = GatewayConfig::default();

    for (path, expected) in [
        ("/api/cli-tools/antigravity-mitm", RouteKind::Api),
        ("/api/cli-tools/antigravity-mitm/alias", RouteKind::Api),
        ("/dashboard/mitm", RouteKind::Dashboard),
        ("/dashboard/mitm/", RouteKind::Dashboard),
    ] {
        // When: a MITM route is requested.
        let route = config.route_for_path(path);

        // Then: it is sent to the expected upstream service.
        assert_eq!(route, expected, "{path}");
    }
}
