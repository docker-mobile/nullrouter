use actix_web::{HttpResponse, http::StatusCode, web};
use serde::{Deserialize, Serialize};

use crate::{json_body, responses};

#[derive(Debug, Deserialize)]
struct CreateKeyRequest {
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UpdateKeyRequest {
    #[serde(rename = "isActive")]
    is_active: Option<bool>,
}

#[derive(Debug, Serialize)]
struct KeyResponse {
    key: ApiKey,
}

#[derive(Debug, Serialize)]
struct ApiKey {
    key: String,
    name: String,
    id: String,
    #[serde(rename = "machineId")]
    machine_id: &'static str,
}

pub(super) fn configure(config: &mut web::ServiceConfig) {
    config.service(
        web::resource("/api/keys/{id}")
            .route(web::get().to(unknown))
            .route(web::put().to(update_unknown))
            .route(web::delete().to(unknown)),
    );
}

pub(super) async fn create(body: web::Bytes) -> HttpResponse {
    let request = match json_body::parse::<CreateKeyRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let Some(name) = request.name.filter(|name| !name.is_empty()) else {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Name is required"),
        );
    };
    responses::json(
        StatusCode::CREATED,
        &KeyResponse {
            key: ApiKey {
                key: format!("nr_{name}"),
                name,
                id: "key_deterministic".to_owned(),
                machine_id: "nullrouter-api",
            },
        },
    )
}

async fn update_unknown(body: web::Bytes) -> HttpResponse {
    let request = match json_body::parse::<UpdateKeyRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let _ = request.is_active;
    unknown().await
}

async fn unknown() -> HttpResponse {
    responses::json(StatusCode::NOT_FOUND, &responses::error("Key not found"))
}
