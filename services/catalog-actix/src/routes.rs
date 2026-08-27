use actix_web::{Responder, web};
use serde::Serialize;

use crate::{catalog, state};

pub fn configure(config: &mut web::ServiceConfig) {
    config
        .route("/health", web::get().to(health))
        .route("/api/catalog", web::get().to(route_catalog))
        .route("/api/catalog/routes", web::get().to(route_catalog))
        .route("/api/catalog/providers", web::get().to(provider_catalog))
        .route("/api/state/settings", web::get().to(settings))
        .route("/api/state/keys", web::get().to(keys))
        .route("/api/state/usage", web::get().to(usage));
}

#[derive(Debug, Clone, Copy, Serialize)]
struct Health {
    ok: bool,
    service: &'static str,
}

async fn health() -> impl Responder {
    web::Json(Health {
        ok: true,
        service: "nullrouter-catalog",
    })
}

async fn route_catalog() -> impl Responder {
    web::Json(catalog::route_catalog())
}

async fn provider_catalog() -> impl Responder {
    web::Json(catalog::provider_catalog())
}

async fn settings() -> impl Responder {
    web::Json(state::settings())
}

async fn keys() -> impl Responder {
    web::Json(state::keys())
}

async fn usage() -> impl Responder {
    web::Json(state::usage())
}
