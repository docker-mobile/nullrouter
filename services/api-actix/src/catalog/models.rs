//! `/api/models/availability` -- which models the router is currently holding back, and a request to
//! stop holding one back early.
//!
//! The operator-owned model settings that used to live here -- `disabled`, `custom`, and `alias` --
//! moved to `nullrouter-state`, because they have to survive a restart and this service has nowhere to
//! put them. They answered `success: true` and stored nothing while they were here.
//!
//! Availability stayed. It is not configuration: it is the cooldown state of a live process, derived
//! from request failures, and there is nothing to persist. It reports an empty set because this build
//! does not yet surface the execute crate's cooldown table over HTTP -- an honest empty read, not a
//! write that claims to have happened.

use actix_web::{HttpResponse, http::StatusCode, web};
use serde::{Deserialize, Serialize};

use crate::{json_body, responses};

#[derive(Debug, Deserialize)]
struct AvailabilityRequest {
    action: Option<String>,
    provider: Option<String>,
    model: Option<String>,
}

#[derive(Debug, Serialize)]
struct AvailabilityResponse {
    models: Vec<serde_json::Value>,
    #[serde(rename = "unavailableCount")]
    unavailable_count: u8,
}

#[derive(Debug, Serialize)]
struct AvailabilityClearResponse {
    ok: bool,
}

pub(super) fn configure(config: &mut web::ServiceConfig) {
    config.service(
        web::resource("/api/models/availability")
            .route(web::get().to(availability))
            .route(web::post().to(clear_availability)),
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
