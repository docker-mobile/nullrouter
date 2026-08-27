use std::collections::BTreeMap;

use actix_web::{HttpResponse, http::StatusCode, web};
use serde::Serialize;

use crate::responses;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct VoicesResponse {
    voices: Vec<serde_json::Value>,
    languages: Vec<serde_json::Value>,
    by_lang: BTreeMap<String, serde_json::Value>,
}

pub(super) fn configure(config: &mut web::ServiceConfig) {
    config
        .service(web::resource("/api/media-providers/tts/voices").route(web::get().to(voices)))
        .service(
            web::resource("/api/media-providers/tts/{provider}/voices")
                .route(web::get().to(provider_voices)),
        );
}

async fn voices() -> HttpResponse {
    responses::json(StatusCode::OK, &empty_voices())
}

async fn provider_voices() -> HttpResponse {
    responses::json(StatusCode::OK, &empty_voices())
}

const fn empty_voices() -> VoicesResponse {
    VoicesResponse {
        voices: Vec::new(),
        languages: Vec::new(),
        by_lang: BTreeMap::new(),
    }
}
