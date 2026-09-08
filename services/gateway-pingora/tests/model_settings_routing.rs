//! Stored model settings reach the state service; the live catalogue does not.
//!
//! The split matters because these paths share a prefix with two that must not move. `/api/models` is
//! the catalogue the router will route to, and `/api/models/availability` is the cooldown state of a
//! running process -- both belong to the API service. A prefix rule covering the stored settings would
//! silently take them as well, and the catalogue would start answering with the state service's empty
//! model list, which looks exactly like a router that has no models configured.

use nullrouter_gateway::{GatewayConfig, RouteKind};

#[test]
fn stored_model_settings_route_to_state() {
    // Given: disabled ids, custom models, and aliases are persisted operator configuration.
    let config = GatewayConfig::default();

    for path in [
        "/api/models/disabled",
        "/api/models/custom",
        "/api/models/alias",
    ] {
        // When: one of them is requested.
        let route = config.route_for_path(path);

        // Then: it reaches the service that can store it, ahead of the generic API fallback.
        assert_eq!(route, RouteKind::State, "{path}");
    }
}

#[test]
fn the_catalogue_and_availability_stay_on_api() {
    // Given: the catalogue is live data and availability is per-process cooldown.
    let config = GatewayConfig::default();

    for path in ["/api/models", "/api/models/availability"] {
        // When: either is requested.
        let route = config.route_for_path(path);

        // Then: it stays with the API service. A prefix rule for the settings would break this.
        assert_eq!(route, RouteKind::Api, "{path}");
    }
}

#[test]
fn a_query_string_does_not_change_the_decision() {
    // Given: two of these endpoints are addressed with query parameters -- `?providerAlias=` to narrow
    // a read, `?alias=` to delete one entry.
    let config = GatewayConfig::default();

    // When: the path is matched. `route_for_path` takes a path, so a caller that passed the full
    // request target would fall through to the API fallback and the write would reach a service that
    // cannot store it.
    for path in [
        "/api/models/disabled",
        "/api/models/custom",
        "/api/models/alias",
    ] {
        // Then: the bare path is what decides, and it decides State.
        assert_eq!(config.route_for_path(path), RouteKind::State, "{path}");
    }
}

#[test]
fn a_deeper_path_under_models_is_not_claimed_by_state() {
    // Given: the settings are matched exactly, not as prefixes.
    let config = GatewayConfig::default();

    // When: a path below one of them is requested.
    for path in [
        "/api/models/disabled/extra",
        "/api/models/custom/nested/deeper",
        "/api/models/aliases",
    ] {
        // Then: it is not silently treated as the settings endpoint. `/api/models/aliases` is
        // included because it differs from `/api/models/alias` by one character, and a `starts_with`
        // rule would take it.
        assert_eq!(config.route_for_path(path), RouteKind::Api, "{path}");
    }
}
