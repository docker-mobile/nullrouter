use actix_web::{HttpResponse, http::StatusCode, web};
use serde::Deserialize;

use crate::{json_body, responses};

pub(super) fn configure(config: &mut web::ServiceConfig) {
    config
        .service(
            web::resource("/api/proxy-pools/{id}/test")
                .route(web::post().to(test_pool))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        );
    // The three relay deploys live in their own module: they are the only routes here that make
    // authenticated calls to a third-party platform on a user's behalf.
    crate::relay_deploy::configure(config);
}

async fn test_pool(path: web::Path<String>, body: web::Bytes) -> HttpResponse {
    match json_body::parse_optional::<serde_json::Value>(&body) {
        Ok(_) => responses::json(
            StatusCode::NOT_IMPLEMENTED,
            &serde_json::json!({
                "id": path.into_inner(),
                "ok": false,
                "status": null,
                "statusText": null,
                "error": "Proxy pool testing is not supported by nullrouter-api",
                "elapsedMs": 0,
                "unsupported": true,
            }),
        ),
        Err(response) => response,
    }
}

async fn options() -> HttpResponse {
    responses::empty(StatusCode::NO_CONTENT)
}
