use std::collections::BTreeMap;

use actix_web::{HttpResponse, http::StatusCode, web};
use serde::{Deserialize, Serialize};

use crate::{json_body, responses};

#[derive(Debug, Deserialize)]
struct AvailabilityRequest {
    action: Option<String>,
    provider: Option<String>,
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DisabledModelsRequest {
    #[serde(rename = "providerAlias")]
    provider_alias: Option<String>,
    ids: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct CustomModelRequest {
    #[serde(rename = "providerAlias")]
    provider_alias: Option<String>,
    id: Option<String>,
    #[serde(rename = "type")]
    model_type: Option<String>,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AliasRequest {
    model: Option<String>,
    alias: Option<String>,
}

#[derive(Debug, Serialize)]
struct AvailabilityResponse {
    models: Vec<serde_json::Value>,
    #[serde(rename = "unavailableCount")]
    unavailable_count: u8,
}

#[derive(Debug, Serialize)]
struct DisabledModelsResponse {
    disabled: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Serialize)]
struct DisabledProviderResponse {
    ids: Vec<String>,
}

#[derive(Debug, Serialize)]
struct CustomModelsResponse {
    models: Vec<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct AliasListResponse {
    aliases: BTreeMap<String, String>,
}

#[derive(Debug, Serialize)]
struct SuccessResponse {
    success: bool,
}

#[derive(Debug, Serialize)]
struct AvailabilityClearResponse {
    ok: bool,
}

#[derive(Debug, Serialize)]
struct CustomModelAddResponse {
    success: bool,
    added: bool,
}

#[derive(Debug, Serialize)]
struct AliasSetResponse {
    success: bool,
    model: String,
    alias: String,
}

pub(super) fn configure(config: &mut web::ServiceConfig) {
    config
        .service(
            web::resource("/api/models/availability")
                .route(web::get().to(availability))
                .route(web::post().to(clear_availability)),
        )
        .service(
            web::resource("/api/models/disabled")
                .route(web::get().to(disabled_models))
                .route(web::post().to(disable_models))
                .route(web::delete().to(enable_models)),
        )
        .service(
            web::resource("/api/models/custom")
                .route(web::get().to(custom_models))
                .route(web::post().to(add_custom_model))
                .route(web::delete().to(delete_custom_model)),
        )
        .service(
            web::resource("/api/models/alias")
                .route(web::get().to(model_aliases))
                .route(web::put().to(set_model_alias))
                .route(web::delete().to(delete_model_alias)),
        );
}

async fn availability() -> HttpResponse {
    responses::json(
        StatusCode::OK,
        &AvailabilityResponse {
            models: Vec::new(),
            unavailable_count: 0,
        },
    )
}

async fn clear_availability(body: web::Bytes) -> HttpResponse {
    let request = match json_body::parse::<AvailabilityRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let is_valid = request.action.as_deref() == Some("clearCooldown")
        && request
            .provider
            .as_deref()
            .is_some_and(|provider| !provider.is_empty())
        && request
            .model
            .as_deref()
            .is_some_and(|model| !model.is_empty());
    if !is_valid {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Invalid request"),
        );
    }
    responses::json(StatusCode::OK, &AvailabilityClearResponse { ok: true })
}

async fn disabled_models(query: web::Query<BTreeMap<String, String>>) -> HttpResponse {
    if query
        .get("providerAlias")
        .is_some_and(|provider| !provider.is_empty())
    {
        return responses::json(
            StatusCode::OK,
            &DisabledProviderResponse { ids: Vec::new() },
        );
    }
    responses::json(
        StatusCode::OK,
        &DisabledModelsResponse {
            disabled: BTreeMap::new(),
        },
    )
}

async fn disable_models(body: web::Bytes) -> HttpResponse {
    let request = match json_body::parse::<DisabledModelsRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    if request.provider_alias.as_deref().is_none_or(str::is_empty) || request.ids.is_none() {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("providerAlias and ids[] required"),
        );
    }
    responses::json(StatusCode::OK, &SuccessResponse { success: true })
}

async fn enable_models(query: web::Query<BTreeMap<String, String>>) -> HttpResponse {
    if query.get("providerAlias").is_none_or(String::is_empty) {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("providerAlias required"),
        );
    }
    responses::json(StatusCode::OK, &SuccessResponse { success: true })
}

async fn custom_models() -> HttpResponse {
    responses::json(StatusCode::OK, &CustomModelsResponse { models: Vec::new() })
}

async fn add_custom_model(body: web::Bytes) -> HttpResponse {
    let request = match json_body::parse::<CustomModelRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    if request.provider_alias.as_deref().is_none_or(str::is_empty)
        || request.id.as_deref().is_none_or(str::is_empty)
    {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("providerAlias and id required"),
        );
    }
    let _ = request.model_type;
    let _ = request.name;
    responses::json(
        StatusCode::OK,
        &CustomModelAddResponse {
            success: true,
            added: true,
        },
    )
}

async fn delete_custom_model(query: web::Query<BTreeMap<String, String>>) -> HttpResponse {
    let provider_alias = query.get("providerAlias").map(String::as_str);
    let id = query.get("id").map(String::as_str);
    if provider_alias.is_none_or(str::is_empty) || id.is_none_or(str::is_empty) {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("providerAlias and id required"),
        );
    }
    responses::json(StatusCode::OK, &SuccessResponse { success: true })
}

async fn model_aliases() -> HttpResponse {
    responses::json(
        StatusCode::OK,
        &AliasListResponse {
            aliases: BTreeMap::new(),
        },
    )
}

async fn set_model_alias(body: web::Bytes) -> HttpResponse {
    let request = match json_body::parse::<AliasRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let Some(model) = request.model.filter(|model| !model.is_empty()) else {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Model and alias required"),
        );
    };
    let Some(alias) = request.alias.filter(|alias| !alias.is_empty()) else {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Model and alias required"),
        );
    };
    responses::json(
        StatusCode::OK,
        &AliasSetResponse {
            success: true,
            model,
            alias,
        },
    )
}

async fn delete_model_alias(query: web::Query<BTreeMap<String, String>>) -> HttpResponse {
    if query.get("alias").is_none_or(String::is_empty) {
        return responses::json(StatusCode::BAD_REQUEST, &responses::error("Alias required"));
    }
    responses::json(StatusCode::OK, &SuccessResponse { success: true })
}
