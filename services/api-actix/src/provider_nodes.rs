use actix_web::{HttpResponse, http::StatusCode, web};
use serde::{Deserialize, Serialize};

use crate::{json_body, responses};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderNodeRequest {
    name: Option<String>,
    prefix: Option<String>,
    api_type: Option<String>,
    base_url: Option<String>,
    #[serde(rename = "type")]
    node_type: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ValidateRequest {
    base_url: Option<String>,
    api_key: Option<String>,
    #[serde(rename = "type")]
    node_type: Option<String>,
    model_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct ProviderNodesResponse {
    nodes: Vec<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct ProviderNodeResponse {
    node: ProviderNode,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderNode {
    id: String,
    name: String,
    prefix: String,
    #[serde(rename = "type")]
    node_type: String,
    api_type: Option<String>,
    base_url: String,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct ValidationResponse {
    valid: bool,
    error: Option<&'static str>,
    method: Option<&'static str>,
}

pub(super) fn configure(config: &mut web::ServiceConfig) {
    config
        .service(
            web::resource("/api/provider-nodes")
                .route(web::get().to(list))
                .route(web::post().to(create)),
        )
        .service(web::resource("/api/provider-nodes/validate").route(web::post().to(validate)))
        .service(
            web::resource("/api/provider-nodes/{id}")
                .route(web::get().to(unknown))
                .route(web::put().to(update_unknown))
                .route(web::delete().to(unknown)),
        );
}

async fn list() -> HttpResponse {
    responses::json(StatusCode::OK, &ProviderNodesResponse { nodes: Vec::new() })
}

async fn create(body: web::Bytes) -> HttpResponse {
    let request = match json_body::parse::<ProviderNodeRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let node = match parse_provider_node(request) {
        Ok(node) => node,
        Err(response) => return response,
    };
    responses::json(StatusCode::CREATED, &ProviderNodeResponse { node })
}

async fn validate(body: web::Bytes) -> HttpResponse {
    let request = match json_body::parse::<ValidateRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let base_url = request.base_url.as_deref().unwrap_or_default().trim();
    let api_key = request.api_key.as_deref().unwrap_or_default().trim();
    if base_url.is_empty() || api_key.is_empty() {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Base URL and API key required"),
        );
    }
    if !has_http_scheme(base_url) {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Invalid URL format"),
        );
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

async fn update_unknown(body: web::Bytes) -> HttpResponse {
    let request = match json_body::parse::<ProviderNodeRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let _ = request;
    unknown().await
}

async fn unknown() -> HttpResponse {
    responses::json(
        StatusCode::NOT_FOUND,
        &responses::error("Provider node not found"),
    )
}

fn parse_provider_node(request: ProviderNodeRequest) -> Result<ProviderNode, HttpResponse> {
    let Some(name) = request
        .name
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
    else {
        return Err(responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Name is required"),
        ));
    };
    let Some(prefix) = request
        .prefix
        .map(|prefix| prefix.trim().to_owned())
        .filter(|prefix| !prefix.is_empty())
    else {
        return Err(responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Prefix is required"),
        ));
    };
    let node_type = request
        .node_type
        .unwrap_or_else(|| "openai-compatible".to_owned());
    if !matches!(
        node_type.as_str(),
        "openai-compatible" | "anthropic-compatible" | "custom-embedding"
    ) {
        return Err(responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Invalid provider node type"),
        ));
    }
    let api_type = request.api_type;
    if node_type == "openai-compatible"
        && !matches!(api_type.as_deref(), Some("chat" | "responses"))
    {
        return Err(responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Invalid OpenAI compatible API type"),
        ));
    }
    Ok(ProviderNode {
        id: stable_node_id(&prefix),
        name,
        prefix,
        base_url: request
            .base_url
            .unwrap_or_else(|| "https://api.openai.com/v1".to_owned()),
        node_type,
        api_type,
    })
}

fn stable_node_id(prefix: &str) -> String {
    format!("provider_node_{prefix}")
}

fn has_http_scheme(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}
