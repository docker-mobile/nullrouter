use actix_web::{HttpResponse, http::StatusCode, web};
use serde::{Deserialize, Serialize};

use crate::{json_body, responses};

#[derive(Debug, Deserialize)]
struct ComboRequest {
    name: Option<String>,
    models: Option<Vec<String>>,
    kind: Option<String>,
}

#[derive(Debug, Serialize)]
struct CombosResponse {
    combos: Vec<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct ComboResponse {
    id: String,
    name: String,
    kind: Option<String>,
    models: Vec<String>,
    #[serde(rename = "createdAt")]
    created_at: &'static str,
    #[serde(rename = "updatedAt")]
    updated_at: &'static str,
}

pub(super) fn configure(config: &mut web::ServiceConfig) {
    config
        .service(
            web::resource("/api/combos")
                .route(web::get().to(list))
                .route(web::post().to(create)),
        )
        .service(
            web::resource("/api/combos/{id}")
                .route(web::get().to(unknown))
                .route(web::put().to(update_unknown))
                .route(web::delete().to(unknown)),
        );
}

async fn list() -> HttpResponse {
    responses::json(StatusCode::OK, &CombosResponse { combos: Vec::new() })
}

async fn create(body: web::Bytes) -> HttpResponse {
    let request = match json_body::parse::<ComboRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let Some(name) = request.name.filter(|name| !name.is_empty()) else {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Name is required"),
        );
    };
    if !is_valid_combo_name(&name) {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Name can only contain letters, numbers, -, _ and ."),
        );
    }

    responses::json(
        StatusCode::CREATED,
        &ComboResponse {
            id: stable_combo_id(&name),
            name,
            kind: request.kind,
            models: request.models.unwrap_or_default(),
            created_at: "1970-01-01T00:00:00.000Z",
            updated_at: "1970-01-01T00:00:00.000Z",
        },
    )
}

async fn update_unknown(body: web::Bytes) -> HttpResponse {
    let request = match json_body::parse::<ComboRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    if let Some(name) = request.name.filter(|name| !name.is_empty())
        && !is_valid_combo_name(&name)
    {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Name can only contain letters, numbers, -, _ and ."),
        );
    }
    unknown().await
}

async fn unknown() -> HttpResponse {
    responses::json(StatusCode::NOT_FOUND, &responses::error("Combo not found"))
}

fn is_valid_combo_name(name: &str) -> bool {
    name.chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

fn stable_combo_id(name: &str) -> String {
    format!("combo_{name}")
}
