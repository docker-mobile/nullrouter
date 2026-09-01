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

async fn load(query: web::Query<LoadQuery>, state: web::Data<crate::StateClient>) -> HttpResponse {
    let Some(file) = query.file.as_deref().filter(|file| allowed_file(file)) else {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Valid file parameter required"),
        );
    };
    match state.translator_log(file).await {
        Ok(Some(content)) => responses::json(
            StatusCode::OK,
            &LoadResponse {
                success: true,
                content: Some(content),
                error: None,
            },
        ),
        Ok(None) => responses::json(
            StatusCode::OK,
            &LoadResponse {
                success: false,
                content: None,
                error: Some("File not found"),
            },
        ),
        // Upstream cannot distinguish these, because its files are on local disk. Here the
        // panes live in the state service, so "not saved yet" and "state is down" are
        // genuinely different and the second should not read as the first.
        Err(()) => responses::json(
            StatusCode::SERVICE_UNAVAILABLE,
            &LoadResponse {
                success: false,
                content: None,
                error: Some("nullrouter-state is unreachable"),
            },
        ),
    }
}

async fn save(body: web::Bytes, state: web::Data<crate::StateClient>) -> HttpResponse {
    let request = match json_body::parse::<SaveRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let Some(file) = request.file.as_deref().filter(|file| allowed_file(file)) else {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("File and content required"),
        );
    };
    let Some(content) = request.content.as_deref() else {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("File and content required"),
        );
    };

    match state.save_translator_log(file, content).await {
        Ok(()) => responses::json(
            StatusCode::OK,
            &SuccessResponse {
                success: true,
                unsupported: false,
                error: None,
            },
        ),
        Err(()) => responses::json(
            StatusCode::SERVICE_UNAVAILABLE,
            &SuccessResponse {
                success: false,
                unsupported: false,
                error: Some("nullrouter-state is unreachable"),
            },
        ),
    }
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

async fn translate(body: web::Bytes, runtime: web::Data<crate::RuntimeClient>) -> HttpResponse {
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
    // Step 5 is this port's own addition: a response translation. Upstream's inspector has
    // action buttons for steps 1, 3 and Send only, leaving its two response panes to be
    // filled in by hand.
    if !matches!(step, 1..=3 | 5) {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Invalid step (1-3, or 5 for a response)"),
        );
    }

    // Proxied to the runtime, which owns the translation engine, the node-prefix resolution
    // step 1 needs, and the credentials steps 3 and 5 need. See `RuntimeClient::translator_step`.
    match runtime.translator_step(&body).await {
        Some(reply) => responses::passthrough(
            StatusCode::from_u16(reply.status).unwrap_or(StatusCode::BAD_GATEWAY),
            &reply.content_type,
            reply.body,
        ),
        None => responses::json(
            StatusCode::SERVICE_UNAVAILABLE,
            &SuccessResponse {
                success: false,
                unsupported: false,
                error: Some("nullrouter-runtime is unreachable"),
            },
        ),
    }
}

/// The buffered log lines, read from the state service that holds them.
///
/// Upstream's response is `{success, logs: string[]}` and that is preserved, so an unmodified
/// dashboard renders unchanged. The structured `lines` alongside it are this port's: with eight
/// processes writing to one buffer, a bare string is not traceable to the service that logged it.
async fn console_logs(state: web::Data<crate::StateClient>) -> HttpResponse {
    match state.console_logs(None).await {
        Some(page) => {
            let logs = page.get("logs").cloned().unwrap_or(serde_json::json!([]));
            let mut body = serde_json::json!({ "success": true, "logs": logs });
            for key in ["lines", "cursor", "generation"] {
                if let Some(value) = page.get(key) {
                    responses::insert_key(&mut body, key, value.clone());
                }
            }
            responses::json(StatusCode::OK, &body)
        }
        // The buffer is in another process, so its being unreachable is a real condition rather than
        // "no logs". Reported as such: an empty list here would read as a quiet router, which is the
        // opposite of what a user checking their logs needs to know.
        None => responses::json(
            StatusCode::SERVICE_UNAVAILABLE,
            &serde_json::json!({
                "success": false,
                "logs": [],
                "error": "The console-log buffer is held by nullrouter-state, which did not answer.",
            }),
        ),
    }
}

async fn delete_console_logs(state: web::Data<crate::StateClient>) -> HttpResponse {
    if state.clear_console_logs().await {
        return responses::json(StatusCode::OK, &ConsoleLogDeleteResponse { success: true });
    }
    responses::json(
        StatusCode::SERVICE_UNAVAILABLE,
        &serde_json::json!({
            "success": false,
            "error": "The console-log buffer is held by nullrouter-state, which did not answer.",
        }),
    )
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
