use actix_web::{HttpResponse, http::StatusCode, web};
use serde::Deserialize;

use crate::{json_body, responses};

#[derive(Debug, Deserialize)]
struct LocaleRequest {
    locale: Option<String>,
}

const SUPPORTED_LOCALES: &[&str] = &[
    "en", "vi", "zh-CN", "zh-TW", "ja", "pt-BR", "pt-PT", "ko", "es", "de", "fr", "he", "ar", "ru",
    "pl", "cs", "nl", "tr", "uk", "tl", "id", "th", "hi", "bn", "ur", "ro", "sv", "it", "el", "hu",
    "fi", "da", "no", "fa",
];

pub(super) fn configure(config: &mut web::ServiceConfig) {
    config.service(
        web::resource("/api/locale")
            .route(web::get().to(current))
            .route(web::post().to(update))
            .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
    );
}

async fn current() -> HttpResponse {
    responses::json(
        StatusCode::OK,
        &serde_json::json!({
            "locale": "en",
            "source": "default",
        }),
    )
}

async fn update(body: web::Bytes) -> HttpResponse {
    let request = match json_body::parse::<LocaleRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let Some(locale) = request.locale.as_deref().map(str::trim) else {
        return invalid_locale();
    };
    let normalized = normalize_locale(locale);
    if !SUPPORTED_LOCALES.contains(&normalized) {
        return invalid_locale();
    }
    responses::json(
        StatusCode::OK,
        &serde_json::json!({
            "success": true,
            "locale": normalized,
        }),
    )
}

fn normalize_locale(locale: &str) -> &'static str {
    match locale {
        "zh" | "zh-CN" => "zh-CN",
        "en" => "en",
        "vi" => "vi",
        "zh-TW" => "zh-TW",
        "ja" => "ja",
        "pt-BR" => "pt-BR",
        "pt-PT" => "pt-PT",
        "ko" => "ko",
        "es" => "es",
        "de" => "de",
        "fr" => "fr",
        "he" => "he",
        "ar" => "ar",
        "ru" => "ru",
        "pl" => "pl",
        "cs" => "cs",
        "nl" => "nl",
        "tr" => "tr",
        "uk" => "uk",
        "tl" => "tl",
        "id" => "id",
        "th" => "th",
        "hi" => "hi",
        "bn" => "bn",
        "ur" => "ur",
        "ro" => "ro",
        "sv" => "sv",
        "it" => "it",
        "el" => "el",
        "hu" => "hu",
        "fi" => "fi",
        "da" => "da",
        "no" => "no",
        "fa" => "fa",
        _ => "",
    }
}

fn invalid_locale() -> HttpResponse {
    responses::json(StatusCode::BAD_REQUEST, &responses::error("Invalid locale"))
}

async fn options() -> HttpResponse {
    responses::empty(StatusCode::NO_CONTENT)
}
