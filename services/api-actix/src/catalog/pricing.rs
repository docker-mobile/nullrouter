use std::collections::BTreeMap;

use actix_web::{HttpResponse, http::StatusCode, web};
use serde::Serialize;

use crate::{json_body, responses};

mod state;

use state::{PricingStore, parse_pricing_update};

#[derive(Debug, Serialize)]
struct TagsResponse {
    models: [OllamaModel; 2],
}

#[derive(Debug, Serialize)]
struct OllamaModel {
    name: &'static str,
    modified_at: &'static str,
    size: u64,
    digest: &'static str,
    details: OllamaDetails,
}

#[derive(Debug, Serialize)]
struct OllamaDetails {
    format: &'static str,
    family: &'static str,
    parameter_size: &'static str,
    quantization_level: &'static str,
}

pub(super) fn configure(config: &mut web::ServiceConfig) {
    config
        .app_data(web::Data::new(PricingStore::default()))
        .service(
            web::resource("/api/pricing")
                .route(web::get().to(pricing))
                .route(web::patch().to(update_pricing))
                .route(web::delete().to(reset_pricing))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(web::resource("/api/tags").route(web::get().to(tags)));
}

async fn pricing(store: web::Data<PricingStore>) -> HttpResponse {
    let catalog = match store.merged() {
        Ok(catalog) => catalog,
        Err(_error) => return internal_error("Failed to fetch pricing"),
    };
    responses::json(StatusCode::OK, &catalog)
}

async fn update_pricing(store: web::Data<PricingStore>, body: web::Bytes) -> HttpResponse {
    let value = match json_body::parse::<serde_json::Value>(&body) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let update = match parse_pricing_update(value) {
        Ok(update) => update,
        Err(error) => {
            return responses::json(
                StatusCode::BAD_REQUEST,
                &serde_json::json!({ "error": error }),
            );
        }
    };
    let user_pricing = match store.update(update) {
        Ok(user_pricing) => user_pricing,
        Err(_error) => return internal_error("Failed to update pricing"),
    };
    responses::json(StatusCode::OK, &user_pricing)
}

async fn reset_pricing(
    store: web::Data<PricingStore>,
    query: web::Query<BTreeMap<String, String>>,
) -> HttpResponse {
    let provider = query
        .get("provider")
        .filter(|value| !value.is_empty())
        .map(String::as_str);
    let model = query
        .get("model")
        .filter(|value| !value.is_empty())
        .map(String::as_str);
    let catalog = match store.reset(provider, model) {
        Ok(catalog) => catalog,
        Err(_error) => return internal_error("Failed to reset pricing"),
    };
    responses::json(StatusCode::OK, &catalog)
}

async fn options() -> HttpResponse {
    responses::empty(StatusCode::NO_CONTENT)
}

fn internal_error(error: &'static str) -> HttpResponse {
    responses::json(StatusCode::INTERNAL_SERVER_ERROR, &responses::error(error))
}

async fn tags() -> HttpResponse {
    responses::json(StatusCode::OK, &TAGS)
}

const TAGS: TagsResponse = TagsResponse {
    models: [
        OllamaModel {
            name: "llama3.2",
            modified_at: "2025-12-26T00:00:00Z",
            size: 2_000_000_000,
            digest: "abc123def456",
            details: OllamaDetails {
                format: "gguf",
                family: "llama",
                parameter_size: "3B",
                quantization_level: "Q4_K_M",
            },
        },
        OllamaModel {
            name: "qwen2.5",
            modified_at: "2025-12-26T00:00:00Z",
            size: 4_000_000_000,
            digest: "def456abc123",
            details: OllamaDetails {
                format: "gguf",
                family: "qwen",
                parameter_size: "7B",
                quantization_level: "Q4_K_M",
            },
        },
    ],
};
