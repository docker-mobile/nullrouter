use actix_web::{HttpResponse, http::StatusCode};
use serde::{Deserialize, Serialize};

use crate::responses;

pub(super) const OPENAI_COMPATIBLE: &str = "openai-compatible";
pub(super) const ANTHROPIC_COMPATIBLE: &str = "anthropic-compatible";
pub(super) const CUSTOM_EMBEDDING: &str = "custom-embedding";

const OPENAI_DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
const ANTHROPIC_DEFAULT_BASE_URL: &str = "https://api.anthropic.com/v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProviderNode {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: String,
    pub name: String,
    pub prefix: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_type: Option<String>,
    pub base_url: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ProviderNodeInput {
    pub(super) node_type: String,
    pub(super) name: String,
    pub(super) prefix: String,
    pub(super) api_type: Option<String>,
    pub(super) base_url: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProviderNodeRequest {
    name: Option<String>,
    prefix: Option<String>,
    api_type: Option<String>,
    base_url: Option<String>,
    #[serde(rename = "type")]
    node_type: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ValidateRequest {
    pub(super) base_url: Option<String>,
    pub(super) api_key: Option<String>,
    #[serde(rename = "type")]
    pub(super) node_type: Option<String>,
    pub(super) model_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct ProviderNodesResponse<T> {
    pub(super) nodes: T,
}

#[derive(Debug, Serialize)]
pub(super) struct ProviderNodeResponse<T> {
    pub(super) node: T,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub(super) struct ValidationResponse {
    pub(super) valid: bool,
    pub(super) error: Option<&'static str>,
    pub(super) method: Option<&'static str>,
}

#[derive(Clone, Copy)]
pub(super) enum RequestMode<'a> {
    Create,
    Update { node_type: &'a str },
}

pub(super) fn provider_node_input_from_request(
    request: ProviderNodeRequest,
    mode: RequestMode<'_>,
) -> Result<ProviderNodeInput, HttpResponse> {
    let Some(name) = trim_optional(request.name) else {
        return Err(bad_request("Name is required"));
    };
    let Some(prefix) = trim_optional(request.prefix) else {
        return Err(bad_request("Prefix is required"));
    };
    let node_type = match mode {
        RequestMode::Create => request
            .node_type
            .unwrap_or_else(|| OPENAI_COMPATIBLE.to_owned()),
        RequestMode::Update { node_type } => node_type.to_owned(),
    };
    if !is_valid_node_type(&node_type) {
        return Err(bad_request("Invalid provider node type"));
    }
    let api_type = api_type_for_node(&node_type, request.api_type)?;
    let base_url = base_url_for_node(&node_type, request.base_url, mode)?;
    Ok(ProviderNodeInput {
        node_type,
        name,
        prefix,
        api_type,
        base_url,
    })
}

pub(super) fn trim_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

pub(super) fn has_http_scheme(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

fn base_url_for_node(
    node_type: &str,
    base_url: Option<String>,
    mode: RequestMode<'_>,
) -> Result<String, HttpResponse> {
    let base_url = match trim_optional(base_url) {
        Some(base_url) => base_url,
        None => match mode {
            RequestMode::Create => default_base_url(node_type).to_owned(),
            RequestMode::Update { .. } => return Err(bad_request("Base URL is required")),
        },
    };
    Ok(sanitize_base_url(node_type, &base_url))
}

fn api_type_for_node(
    node_type: &str,
    api_type: Option<String>,
) -> Result<Option<String>, HttpResponse> {
    if node_type != OPENAI_COMPATIBLE {
        return Ok(None);
    }
    let api_type = trim_optional(api_type);
    if matches!(api_type.as_deref(), Some("chat" | "responses")) {
        Ok(api_type)
    } else {
        Err(bad_request("Invalid OpenAI compatible API type"))
    }
}

fn sanitize_base_url(node_type: &str, base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    match node_type {
        CUSTOM_EMBEDDING => strip_suffix(trimmed, "/embeddings").to_owned(),
        ANTHROPIC_COMPATIBLE => strip_suffix(trimmed, "/messages").to_owned(),
        OPENAI_COMPATIBLE => base_url.trim().to_owned(),
        _ => trimmed.to_owned(),
    }
}

fn strip_suffix<'a>(value: &'a str, suffix: &str) -> &'a str {
    value.strip_suffix(suffix).unwrap_or(value)
}

fn default_base_url(node_type: &str) -> &'static str {
    if node_type == ANTHROPIC_COMPATIBLE {
        ANTHROPIC_DEFAULT_BASE_URL
    } else {
        OPENAI_DEFAULT_BASE_URL
    }
}

fn is_valid_node_type(node_type: &str) -> bool {
    matches!(
        node_type,
        OPENAI_COMPATIBLE | ANTHROPIC_COMPATIBLE | CUSTOM_EMBEDDING
    )
}

fn bad_request(message: &'static str) -> HttpResponse {
    responses::json(StatusCode::BAD_REQUEST, &responses::error(message))
}
