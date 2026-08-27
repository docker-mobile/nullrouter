use nullrouter_gateway::{GatewayConfig, RouteKind};

#[test]
fn route_for_path_selects_catalog_when_path_is_catalog_or_state_api() {
    // Given: the gateway has the default internal upstream map.
    let config = GatewayConfig::default();

    for path in [
        "/api/catalog",
        "/api/catalog/providers",
        "/api/state",
        "/api/state/runtime",
    ] {
        // When: a catalog-owned public API path is routed.
        let route = config.route_for_path(path);

        // Then: it is sent to the catalog upstream through the gateway.
        assert_eq!(route, RouteKind::Catalog, "{path}");
    }
}

#[test]
fn route_for_path_selects_runtime_when_path_is_v1_or_v1beta() {
    // Given: the gateway has the default internal upstream map.
    let config = GatewayConfig::default();

    for path in [
        "/v1",
        "/v1/models",
        "/v1beta",
        "/v1beta/models",
        "/api/v1",
        "/api/v1/models",
        "/api/v1/chat/completions",
        "/api/v1beta/models",
    ] {
        // When: an OpenAI-compatible runtime path is routed.
        let route = config.route_for_path(path);

        // Then: it is sent to the runtime upstream before generic API matching.
        assert_eq!(route, RouteKind::Runtime, "{path}");
    }
}

#[test]
fn route_for_path_selects_runtime_for_media_runtime_endpoints() {
    // Given: the gateway has the default internal upstream map.
    let config = GatewayConfig::default();

    for path in [
        "/v1/embeddings",
        "/v1/audio/speech",
        "/v1/images/generations",
        "/v1/search",
        "/v1/web/fetch",
    ] {
        // When: a media-provider runtime path is routed.
        let route = config.route_for_path(path);

        // Then: it is sent to the runtime upstream.
        assert_eq!(route, RouteKind::Runtime, "{path}");
    }
}

#[test]
fn route_for_path_selects_events_for_streaming_api_paths() {
    // Given: the gateway has the default internal upstream map.
    let config = GatewayConfig::default();

    for path in [
        "/api/usage/stream",
        "/api/translator/console-logs/stream",
        "/api/mcp",
        "/api/mcp/sessions",
    ] {
        // When: an event-stream API path is routed.
        let route = config.route_for_path(path);

        // Then: it is sent to the events upstream before generic API matching.
        assert_eq!(route, RouteKind::Events, "{path}");
    }
}

#[test]
fn route_for_path_selects_api_for_remaining_api_paths() {
    // Given: the gateway has the default internal upstream map.
    let config = GatewayConfig::default();

    for path in [
        "/api",
        "/api/health",
        "/api/pricing",
        "/api/catalogue",
        "/api/states",
        "/api/providers/client",
        "/api/providers/openai/models",
        "/api/providers/openai/test",
        "/api/tunnel/status",
        "/api/usage/stream/history",
        "/api/translator/console-logs/stream/history",
    ] {
        // When: a general API path is routed.
        let route = config.route_for_path(path);

        // Then: it remains on the API upstream.
        assert_eq!(route, RouteKind::Api, "{path}");
    }
}

#[test]
fn route_for_path_selects_api_for_media_provider_support_paths() {
    // Given: the gateway has the default internal upstream map.
    let config = GatewayConfig::default();

    for path in [
        "/api/media-providers/tts/voices",
        "/api/media-providers/tts/openai/voices",
        "/api/models/alias",
        "/api/usage/logs",
    ] {
        // When: a media-provider support API path is routed.
        let route = config.route_for_path(path);

        // Then: it remains on the API upstream instead of state/runtime fallbacks.
        assert_eq!(route, RouteKind::Api, "{path}");
    }
}

#[test]
fn route_for_path_selects_state_for_local_db_api_paths() {
    // Given: the gateway has the default internal upstream map.
    let config = GatewayConfig::default();

    for path in [
        "/api/keys",
        "/api/keys/key_1",
        "/api/provider-nodes",
        "/api/provider-nodes/custom-embedding-1",
        "/api/provider-nodes/validate",
        "/api/providers",
        "/api/providers/connection_1",
        "/api/combos",
        "/api/combos/combo_1",
        "/api/proxy-pools",
        "/api/proxy-pools/pool_1",
        "/api/settings",
        "/api/settings/require-login",
    ] {
        // When: a local-DB-backed public API path is routed.
        let route = config.route_for_path(path);

        // Then: it is sent to the state upstream instead of the generic API service.
        assert_eq!(route, RouteKind::State, "{path}");
    }
}

#[test]
fn route_for_path_leaves_nested_provider_proxy_and_settings_tools_on_api() {
    // Given: the gateway only sends exact stateful CRUD families to nullrouter-state.
    let config = GatewayConfig::default();

    for path in [
        "/api/providers/client",
        "/api/providers/openai/models",
        "/api/providers/openai/test",
        "/api/proxy-pools/pool_1/test",
        "/api/proxy-pools/vercel-deploy",
        "/api/settings/database",
        "/api/settings/proxy-test",
    ] {
        // When: an adjacent tool/default route is routed.
        let route = config.route_for_path(path);

        // Then: it remains on nullrouter-api for the existing behavior.
        assert_eq!(route, RouteKind::Api, "{path}");
    }
}

#[test]
fn route_for_path_selects_dashboard_for_media_provider_shell_routes() {
    // Given: the gateway has the default internal upstream map.
    let config = GatewayConfig::default();

    for path in [
        "/dashboard/media-providers/embedding",
        "/dashboard/media-providers/embedding/openai",
        "/dashboard/media-providers/tts/openai",
        "/dashboard/media-providers/combo/combo_1",
    ] {
        // When: a nested media-provider dashboard shell path is routed.
        let route = config.route_for_path(path);

        // Then: it falls back to the dashboard host.
        assert_eq!(route, RouteKind::Dashboard, "{path}");
    }
}

#[test]
fn route_for_path_selects_dashboard_for_everything_else() {
    // Given: the gateway has the default internal upstream map.
    let config = GatewayConfig::default();

    for path in [
        "/",
        "/dashboard",
        "/dashboard/endpoint",
        "/dashboard/providers/new",
        "/dashboard/providers/openai",
        "/dashboard/cli-tools/codex",
        "/dashboard/settings/pricing",
        "/static/styles.css",
        "/favicon.ico",
        "/v1alpha/models",
        "/v1betaish/models",
    ] {
        // When: a non-API path is routed.
        let route = config.route_for_path(path);

        // Then: it falls back to the dashboard host.
        assert_eq!(route, RouteKind::Dashboard, "{path}");
    }
}
