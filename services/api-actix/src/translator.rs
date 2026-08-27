use actix_web::{
    HttpResponse,
    http::{Method, StatusCode},
    web,
};
use serde::{Deserialize, Serialize};

use crate::{handlers, json_body, responses};

const TRANSLATOR_UNSUPPORTED: &str = "Translator execution is not supported by nullrouter-api";
const ALLOWED_FILES: [&str; 8] = [
    "1_req_client.json",
    "2_req_source.json",
    "3_req_openai.json",
    "4_req_target.json",
    "5_res_provider.txt",
    "6_res_openai.txt",
    "7_res_client.txt",
    "7_res_client.json",
];

#[derive(Debug, Deserialize)]
struct LoadQuery {
    file: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SaveRequest {
    file: Option<String>,
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SendRequest {
    provider: Option<String>,
    model: Option<String>,
    body: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct TranslateRequest {
    step: Option<u8>,
    body: Option<serde_json::Value>,
    provider: Option<String>,
    model: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct LoadResponse {
    success: bool,
    content: Option<String>,
    error: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct SuccessResponse {
    success: bool,
    unsupported: bool,
    error: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
struct TranslateResponse {
    success: bool,
    result: serde_json::Value,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct ConsoleLogs {
    success: bool,
    logs: &'static [serde_json::Value],
}

#[derive(Debug, Clone, Copy, Serialize)]
struct ConsoleLogDeleteResponse {
    success: bool,
}

pub(super) fn configure(config: &mut web::ServiceConfig) {
    config
        .service(web::resource("/api/translator/load").route(web::get().to(load)))
        .service(
            web::resource("/api/translator/save")
                .route(web::post().to(save))
                .route(web::method(Method::OPTIONS).to(handlers::no_content)),
        )
        .service(
            web::resource("/api/translator/send")
                .route(web::post().to(send))
                .route(web::method(Method::OPTIONS).to(handlers::no_content)),
        )
        .service(
            web::resource("/api/translator/translate")
                .route(web::post().to(translate))
                .route(web::method(Method::OPTIONS).to(handlers::no_content)),
        )
        .service(
            web::resource("/api/translator/console-logs")
                .route(web::get().to(console_logs))
                .route(web::delete().to(delete_console_logs))
                .route(web::method(Method::OPTIONS).to(handlers::no_content))
                .route(web::route().to(method_not_allowed)),
        )
        .service(
            web::resource("/api/translator/console-logs/stream").route(web::get().to(console_logs)),
        );
}

async fn load(query: web::Query<LoadQuery>) -> HttpResponse {
    if query.file.as_deref().is_none_or(|file| !allowed_file(file)) {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Valid file parameter required"),
        );
    }
    responses::json(
        StatusCode::OK,
        &LoadResponse {
            success: false,
            content: None,
            error: Some("File not found"),
        },
    )
}

async fn save(body: web::Bytes) -> HttpResponse {
    let request = match json_body::parse::<SaveRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    if request
        .file
        .as_deref()
        .is_none_or(|file| !allowed_file(file))
        || request.content.is_none()
    {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("File and content required"),
        );
    }
    responses::json(
        StatusCode::OK,
        &SuccessResponse {
            success: false,
            unsupported: true,
            error: Some("Translator log persistence is not supported"),
        },
    )
}

async fn send(body: web::Bytes) -> HttpResponse {
    let request = match json_body::parse::<SendRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    if request.provider.as_deref().is_none_or(str::is_empty)
        || request.model.as_deref().is_none_or(str::is_empty)
        || request.body.is_none()
    {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("provider, model, and body required"),
        );
    }
    responses::json(
        StatusCode::NOT_IMPLEMENTED,
        &SuccessResponse {
            success: false,
            unsupported: true,
            error: Some(TRANSLATOR_UNSUPPORTED),
        },
    )
}

async fn translate(body: web::Bytes) -> HttpResponse {
    let request = match json_body::parse::<TranslateRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let Some(step) = request.step else {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Step and body required"),
        );
    };
    if request.body.is_none() {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Step and body required"),
        );
    }
    if !matches!(step, 1..=3) {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Invalid step (1-3)"),
        );
    }

    responses::json(
        StatusCode::OK,
        &TranslateResponse {
            success: true,
            result: serde_json::json!({
                "step": step,
                "provider": request.provider,
                "model": request.model,
                "sourceFormat": "unknown",
                "targetFormat": "unknown",
                "body": {},
            }),
        },
    )
}

async fn console_logs() -> HttpResponse {
    responses::json(
        StatusCode::OK,
        &ConsoleLogs {
            success: true,
            logs: &[],
        },
    )
}

async fn delete_console_logs() -> HttpResponse {
    responses::json(StatusCode::OK, &ConsoleLogDeleteResponse { success: true })
}

async fn method_not_allowed() -> HttpResponse {
    responses::json(
        StatusCode::METHOD_NOT_ALLOWED,
        &responses::error("Method not allowed"),
    )
}

fn allowed_file(file: &str) -> bool {
    ALLOWED_FILES.contains(&file)
}
