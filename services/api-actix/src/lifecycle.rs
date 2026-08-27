use actix_web::{HttpResponse, http::StatusCode, web};
use serde::Serialize;

use crate::responses;

#[derive(Debug, Clone, Copy, Serialize)]
struct NoopLifecycleResponse {
    success: bool,
    unsupported: bool,
    message: &'static str,
}

pub(super) fn configure(config: &mut web::ServiceConfig) {
    config
        .service(web::resource("/api/shutdown").route(web::post().to(shutdown)))
        .service(web::resource("/api/version/update").route(web::post().to(update)))
        .service(web::resource("/api/version/shutdown").route(web::post().to(version_shutdown)));
}

async fn shutdown() -> HttpResponse {
    lifecycle_noop("Shutdown is not supported by nullrouter-api")
}

async fn update() -> HttpResponse {
    lifecycle_noop("Version update is not supported by nullrouter-api")
}

async fn version_shutdown() -> HttpResponse {
    lifecycle_noop("Version shutdown is not supported by nullrouter-api")
}

fn lifecycle_noop(message: &'static str) -> HttpResponse {
    responses::json(
        StatusCode::NOT_IMPLEMENTED,
        &NoopLifecycleResponse {
            success: false,
            unsupported: true,
            message,
        },
    )
}
