use actix_web::{HttpResponse, http::StatusCode, web};

use crate::{json_body, responses};

mod import;

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
