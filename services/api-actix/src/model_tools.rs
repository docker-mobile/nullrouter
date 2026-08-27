use actix_web::{HttpResponse, http::StatusCode, web};
use serde::Deserialize;

use crate::{json_body, responses};

#[derive(Debug, Deserialize)]
struct ModelTestRequest {
    model: Option<String>,
    kind: Option<String>,
}

pub(super) fn configure(config: &mut web::ServiceConfig) {
    config.service(
        web::resource("/api/models/test")
            .route(web::post().to(test_model))
            .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
    );
}

async fn test_model(body: web::Bytes) -> HttpResponse {
    let request = match json_body::parse::<ModelTestRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let Some(model) = request
        .model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
    else {
        return responses::json(StatusCode::BAD_REQUEST, &responses::error("Model required"));
    };
    responses::json(
        StatusCode::NOT_IMPLEMENTED,
        &serde_json::json!({
            "ok": false,
            "model": model,
            "kind": request.kind.unwrap_or_else(|| "llm".to_owned()),
            "unsupported": true,
            "error": "Model testing is not supported by nullrouter-api",
        }),
    )
}

async fn options() -> HttpResponse {
    responses::empty(StatusCode::NO_CONTENT)
}
