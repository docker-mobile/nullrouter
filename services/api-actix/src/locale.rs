//! `/api/locale` -- the dashboard's language preference.
//!
//! Held in a cookie rather than in stored state, because the preference belongs to a browser and not
//! to the install: two operators sharing one router should not change each other's language. The
//! dashboard already reads a `locale` cookie to pick its message table, so this endpoint writes the
//! thing that is actually consulted instead of a second copy that nothing reads.
//!
//! Before this, `POST` validated the tag and answered `success: true` while storing nothing, and
//! `GET` always replied `en`/`default` -- so a client that set a locale and read it back was told its
//! change had not happened.

use actix_web::{
    HttpRequest, HttpResponse,
    cookie::{Cookie, SameSite, time},
    http::StatusCode,
    web,
};
use serde::Deserialize;

use crate::{json_body, responses};

/// The cookie the dashboard reads in `detect_locale`. The name is a contract between the two.
const LOCALE_COOKIE: &str = "locale";

/// A year. The preference is a convenience, not a credential, and re-picking a language every session
/// would be worse than a long-lived cookie.
const LOCALE_COOKIE_DAYS: i64 = 365;

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

#[allow(clippy::future_not_send)]
async fn current(request: HttpRequest) -> HttpResponse {
    let Some(value) = request
        .cookie(LOCALE_COOKIE)
        .map(|cookie| cookie.value().to_owned())
        .filter(|value| !value.is_empty())
    else {
        return responses::json(
            StatusCode::OK,
            &serde_json::json!({
                "locale": "en",
                "source": "default",
            }),
        );
    };
    let locale = normalize_locale(&value);
    if !SUPPORTED_LOCALES.contains(&locale) {
        // An unknown or expired value in the cookie is treated as unset rather than reported as the
        // current locale: the dashboard would then try to load a file that does not exist.
        return responses::json(
            StatusCode::OK,
            &serde_json::json!({
                "locale": "en",
                "source": "default",
            }),
        );
    }
    responses::json(
        StatusCode::OK,
        &serde_json::json!({
            "locale": locale,
            "source": "cookie",
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
    let cookie = Cookie::build(LOCALE_COOKIE, normalized)
        .path("/")
        .same_site(SameSite::Lax)
        .max_age(time::Duration::days(LOCALE_COOKIE_DAYS))
        .finish();
    let mut response = responses::json(
        StatusCode::OK,
        &serde_json::json!({
            "success": true,
            "locale": normalized,
        }),
    );
    // The only way this fails is a value that cannot go in a header, and `normalized` is one of the
    // fixed tags above. Reported rather than ignored anyway: silently dropping the cookie would put
    // this endpoint back to answering success while changing nothing.
    if response.add_cookie(&cookie).is_err() {
        return responses::json(
            StatusCode::INTERNAL_SERVER_ERROR,
            &responses::error("the locale cookie could not be set"),
        );
    }
    response
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
