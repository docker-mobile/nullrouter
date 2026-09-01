use actix_web::{HttpResponse, http::StatusCode, web};

use crate::{json_body, responses};

mod cursor_local;
mod import;
mod kiro_local;

pub(super) fn configure(config: &mut web::ServiceConfig) {
    // The implemented routes go first: actix matches in registration order, and the catch-all below
    // would otherwise swallow them and answer 501.
    config
        .service(
            web::resource("/api/oauth/gitlab/pat")
                .route(web::post().to(import::gitlab_pat))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/api/oauth/kiro/api-key")
                .route(web::post().to(import::kiro_api_key))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/api/oauth/codex/import-token")
                .route(web::post().to(import::codex_import_token))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/api/oauth/kiro/import-cli-proxy")
                .route(web::post().to(import::kiro_import_cli_proxy))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/api/oauth/codex/bulk-import")
                .route(web::post().to(import::codex_bulk_import))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        // The only import route with a GET as well: the instructions for finding the token are part of
        // the feature, because this service deliberately does not read the user's Cursor database.
        .service(
            web::resource("/api/oauth/cursor/import")
                .route(web::get().to(import::cursor_import_instructions))
                .route(web::post().to(import::cursor_import))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/api/oauth/iflow/cookie")
                .route(web::post().to(import::iflow_cookie))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        // Host-only at the gateway: it answers with a credential read off this machine's disk.
        .service(
            web::resource("/api/oauth/cursor/auto-import")
                .route(web::get().to(cursor_local::cursor_auto_import))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        // Both routes read or exchange credentials from this host's own Kiro install.
        .service(
            web::resource("/api/oauth/kiro/import")
                .route(web::post().to(import::kiro_import))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/api/oauth/kiro/auto-import")
                .route(web::get().to(kiro_local::kiro_auto_import))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        // Everything else. Not all of it is out of reach — see the module docs on `import` — but
        // what is left either needs a provider's consent screen or is not ported yet, and both are
        // better as an explicit 501 naming the provider and action than as a wrong answer.
        .service(
            web::resource("/api/oauth/{tail:.*}")
                .route(web::get().to(helper_get))
                .route(web::post().to(helper_post))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        );
}

async fn helper_get(path: web::Path<String>) -> HttpResponse {
    let route = OAuthRoute::from_tail(&path);
    unsupported(&route)
}

async fn helper_post(path: web::Path<String>, body: web::Bytes) -> HttpResponse {
    let route = OAuthRoute::from_tail(&path);
    match json_body::parse_optional::<serde_json::Value>(&body) {
        Ok(_) => unsupported(&route),
        Err(response) => response,
    }
}

fn unsupported(route: &OAuthRoute) -> HttpResponse {
    responses::json(
        StatusCode::NOT_IMPLEMENTED,
        &serde_json::json!({
            "success": false,
            "unsupported": true,
            "provider": route.provider,
            "action": route.action,
            "error": "OAuth helper is not supported by nullrouter-api",
        }),
    )
}

#[derive(Debug)]
struct OAuthRoute {
    provider: String,
    action: String,
}

impl OAuthRoute {
    fn from_tail(tail: &str) -> Self {
        let mut parts = tail.split('/');
        let provider = parts.next().unwrap_or_default().to_owned();
        let action = parts.collect::<Vec<_>>().join("/");
        Self { provider, action }
    }
}

async fn options() -> HttpResponse {
    responses::empty(StatusCode::NO_CONTENT)
}
