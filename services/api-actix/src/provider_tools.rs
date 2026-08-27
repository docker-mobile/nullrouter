use actix_web::{HttpResponse, http::StatusCode, web};
use serde::Deserialize;

use crate::{json_body, responses};

#[derive(Debug, Deserialize)]
struct BatchRequest {
    mode: Option<String>,
    #[serde(rename = "providerId")]
    provider_id: Option<String>,
}

pub(super) fn configure(config: &mut web::ServiceConfig) {
    config
        .service(web::resource("/api/providers/{id}/models").route(web::get().to(models)))
        .service(
            web::resource("/api/providers/{id}/test")
                .route(web::post().to(test_provider))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(
            web::resource("/api/providers/{id}/test-models")
                .route(web::post().to(test_models))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        )
        .service(web::resource("/api/providers/kilo/free-models").route(web::get().to(kilo_models)))
        .service(
            web::resource("/api/providers/test-batch")
                .route(web::post().to(test_batch))
                .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
        );
}

async fn models(path: web::Path<String>) -> HttpResponse {
    let provider = path.into_inner();
    responses::json(
        StatusCode::OK,
        &serde_json::json!({
            "provider": provider,
            "models": [],
            "cached": true,
            "warning": "Live provider model discovery is not configured",
        }),
    )
}

async fn test_provider(path: web::Path<String>, body: web::Bytes) -> HttpResponse {
    match json_body::parse_optional::<serde_json::Value>(&body) {
        Ok(_) => {
            provider_test_unsupported(&path, "Provider testing is not supported by nullrouter-api")
        }
        Err(response) => response,
    }
}

async fn test_models(path: web::Path<String>, body: web::Bytes) -> HttpResponse {
    match json_body::parse_optional::<serde_json::Value>(&body) {
        Ok(_) => responses::json(
            StatusCode::NOT_IMPLEMENTED,
            &serde_json::json!({
                "provider": path.into_inner(),
                "connectionId": null,
                "results": [],
                "unsupported": true,
                "error": "Provider model testing is not supported by nullrouter-api",
            }),
        ),
        Err(response) => response,
    }
}

async fn kilo_models() -> HttpResponse {
    responses::json(
        StatusCode::OK,
        &serde_json::json!({
            "models": [],
            "cached": true,
            "warning": "Live Kilo model discovery is not configured",
        }),
    )
}

async fn test_batch(body: web::Bytes) -> HttpResponse {
    let request = match json_body::parse::<BatchRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let Some(mode) = request
        .mode
        .as_deref()
        .map(str::trim)
        .filter(|mode| !mode.is_empty())
    else {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("mode is required"),
        );
    };
    if !matches!(
        mode,
        "provider" | "oauth" | "free" | "apikey" | "compatible" | "all"
    ) {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Invalid mode. Use: provider, oauth, free, apikey, compatible, all"),
        );
    }
    responses::json(
        StatusCode::OK,
        &serde_json::json!({
            "mode": mode,
            "providerId": request.provider_id.unwrap_or_default(),
            "results": [],
            "summary": {
                "total": 0,
                "passed": 0,
                "failed": 0,
            },
        }),
    )
}

fn provider_test_unsupported(provider: &str, error: &'static str) -> HttpResponse {
    responses::json(
        StatusCode::NOT_IMPLEMENTED,
        &serde_json::json!({
            "provider": provider,
            "valid": false,
            "refreshed": false,
            "unsupported": true,
            "error": error,
        }),
    )
}

async fn options() -> HttpResponse {
    responses::empty(StatusCode::NO_CONTENT)
}
