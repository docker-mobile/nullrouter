mod store;
mod types;

use actix_web::{HttpResponse, http::StatusCode, web};
use serde::{Deserialize, Serialize};

use crate::{StoreError, responses, store::StateStore};

use self::types::{
    ProviderNodeRequest, ProviderNodeResponse, ProviderNodesResponse, RequestMode, ValidateRequest,
    ValidationResponse, has_http_scheme, provider_node_input_from_request,
};

pub(crate) use self::types::{ProviderNode, is_compatible_llm_provider};

pub(crate) fn configure(config: &mut web::ServiceConfig) {
    config
        .service(
            web::resource("/api/provider-nodes")
                .route(web::get().to(list_provider_nodes))
                .route(web::post().to(create_provider_node))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/api/provider-nodes/validate")
                .route(web::post().to(validate_provider_node))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/api/provider-nodes/{id}")
                .route(web::get().to(get_provider_node))
                .route(web::put().to(update_provider_node))
                .route(web::delete().to(delete_provider_node))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        );
}

async fn options() -> HttpResponse {
    responses::no_content()
}

async fn list_provider_nodes(store: web::Data<StateStore>) -> HttpResponse {
    store_json(
        StatusCode::OK,
        store
            .list_provider_nodes()
            .map(|nodes| ProviderNodesResponse { nodes }),
    )
}

async fn create_provider_node(store: web::Data<StateStore>, body: web::Bytes) -> HttpResponse {
    let request = match parse_json::<ProviderNodeRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let input = match provider_node_input_from_request(request, RequestMode::Create) {
        Ok(input) => input,
        Err(response) => return response,
    };
    store_json(
        StatusCode::CREATED,
        store
            .create_provider_node(input)
            .map(|node| ProviderNodeResponse { node }),
    )
}

async fn get_provider_node(store: web::Data<StateStore>, path: web::Path<String>) -> HttpResponse {
    match store.get_provider_node(&path) {
        Ok(Some(node)) => responses::json(StatusCode::OK, &ProviderNodeResponse { node }),
        Ok(None) => not_found(),
        Err(_) => internal_error(),
    }
}

async fn update_provider_node(
    store: web::Data<StateStore>,
    path: web::Path<String>,
    body: web::Bytes,
) -> HttpResponse {
    let request = match parse_json::<ProviderNodeRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let existing = match store.get_provider_node(&path) {
        Ok(Some(node)) => node,
        Ok(None) => return not_found(),
        Err(_) => return internal_error(),
    };
    let input = match provider_node_input_from_request(
        request,
        RequestMode::Update {
            node_type: &existing.node_type,
        },
    ) {
        Ok(input) => input,
        Err(response) => return response,
    };
    match store.update_provider_node(&path, input) {
        Ok(Some(node)) => responses::json(StatusCode::OK, &ProviderNodeResponse { node }),
        Ok(None) => not_found(),
        Err(_) => internal_error(),
    }
}

async fn delete_provider_node(
    store: web::Data<StateStore>,
    path: web::Path<String>,
) -> HttpResponse {
    match store.delete_provider_node(&path) {
        Ok(true) => responses::json(StatusCode::OK, &serde_json::json!({ "success": true })),
        Ok(false) => not_found(),
        Err(_) => internal_error(),
    }
}

async fn validate_provider_node(body: web::Bytes) -> HttpResponse {
    let request = match parse_json::<ValidateRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let base_url = request.base_url.as_deref().unwrap_or_default().trim();
    let api_key = request.api_key.as_deref().unwrap_or_default().trim();
    if base_url.is_empty() || api_key.is_empty() {
        return bad_request("Base URL and API key required");
    }
    if !has_http_scheme(base_url) {
        return bad_request("Invalid URL format");
    }
    let _ = (request.node_type, request.model_id);
    responses::json(
        StatusCode::OK,
        &ValidationResponse {
            valid: false,
            error: Some("Provider node validation is not supported"),
            method: None,
        },
    )
}

fn parse_json<T>(body: &[u8]) -> Result<T, HttpResponse>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_slice(body).map_err(|_| bad_request("Invalid JSON body"))
}

fn store_json<T>(status: StatusCode, result: Result<T, StoreError>) -> HttpResponse
where
    T: Serialize,
{
    result.map_or_else(|_| internal_error(), |body| responses::json(status, &body))
}

fn bad_request(message: &'static str) -> HttpResponse {
    responses::json(StatusCode::BAD_REQUEST, &responses::error(message))
}

fn not_found() -> HttpResponse {
    responses::json(
        StatusCode::NOT_FOUND,
        &responses::error("Provider node not found"),
    )
}

fn internal_error() -> HttpResponse {
    responses::json(
        StatusCode::INTERNAL_SERVER_ERROR,
        &responses::error("State service error"),
    )
}
