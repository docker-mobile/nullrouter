use crate::RouteKind;

pub(crate) fn route_for_path(path: &str) -> RouteKind {
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
    } else if is_api_path(path) {
        RouteKind::Api
    } else {
        RouteKind::Dashboard
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

fn is_api_path(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/")
}
