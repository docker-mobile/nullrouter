use crate::RouteKind;

#[inline]
pub(crate) fn route_for_path(path: &str) -> RouteKind {
    // Fast path: direct runtime inference routes represent the hot path
    if path.starts_with("/v1/")
        || path == "/v1"
        || path.starts_with("/v1beta/")
        || path == "/v1beta"
    {
        return RouteKind::Runtime;
    }

    // Fast path: all non-API routes fall through to the dashboard host immediately
    let Some(api_subpath) = path.strip_prefix("/api") else {
        return RouteKind::Dashboard;
    };

    if api_subpath.is_empty() {
        return RouteKind::Api;
    }

    if is_events_path(path) {
        RouteKind::Events
    } else if is_auth_path(path) {
        RouteKind::Auth
    } else if is_runtime_path(path) {
        RouteKind::Runtime
    } else if is_catalog_path(path) {
        RouteKind::Catalog
    } else if is_state_path(path) {
        RouteKind::State
    } else {
        RouteKind::Api
    }
}

fn is_auth_path(path: &str) -> bool {
    path == "/api/auth" || path.starts_with("/api/auth/")
}

fn is_events_path(path: &str) -> bool {
    path == "/api/usage/stream"
        || path == "/api/translator/console-logs/stream"
        || path == "/api/mcp"
        || path.starts_with("/api/mcp/")
}

fn is_runtime_path(path: &str) -> bool {
    path == "/v1"
        || path.starts_with("/v1/")
        || path == "/v1beta"
        || path.starts_with("/v1beta/")
        || path == "/api/v1"
        || path.starts_with("/api/v1/")
        || path == "/api/v1beta"
        || path.starts_with("/api/v1beta/")
}

fn is_catalog_path(path: &str) -> bool {
    path == "/api/catalog"
        || path.starts_with("/api/catalog/")
        || path == "/api/state"
        || path.starts_with("/api/state/")
}

fn is_state_path(path: &str) -> bool {
    is_collection_or_item(path, "/api/keys")
        || is_collection_or_item(path, "/api/provider-nodes")
        || is_collection_or_item_except(
            path,
            "/api/providers",
            &[
                "client",
                "suggested-models",
                "validate",
                "test-batch",
                "kilo",
            ],
        )
        || is_collection_or_item(path, "/api/combos")
        || is_collection_or_item_except(
            path,
            "/api/proxy-pools",
            &["cloudflare-deploy", "deno-deploy", "vercel-deploy"],
        )
        || path == "/api/settings"
        || is_model_settings_path(path)
}

/// The three `/api/models/*` paths that are stored state rather than catalogue.
///
/// Listed exactly rather than matched by prefix: `/api/models` itself is the live catalogue and
/// `/api/models/availability` is per-process cooldown, both of which belong to the API service. A
/// prefix rule here would silently take them too, and the catalogue would start answering empty.
fn is_model_settings_path(path: &str) -> bool {
    matches!(
        path,
        "/api/models/disabled" | "/api/models/custom" | "/api/models/alias"
    )
}

fn is_collection_or_item(path: &str, collection: &str) -> bool {
    if path == collection {
        return true;
    }
    let Some(tail) = path
        .strip_prefix(collection)
        .and_then(|tail| tail.strip_prefix('/'))
    else {
        return false;
    };
    !tail.is_empty() && !tail.contains('/')
}

fn is_collection_or_item_except(path: &str, collection: &str, reserved_items: &[&str]) -> bool {
    if path == collection {
        return true;
    }
    let Some(tail) = path
        .strip_prefix(collection)
        .and_then(|tail| tail.strip_prefix('/'))
    else {
        return false;
    };
    !tail.is_empty() && !tail.contains('/') && !reserved_items.contains(&tail)
}
