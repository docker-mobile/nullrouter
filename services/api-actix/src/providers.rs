use actix_web::{HttpResponse, http::StatusCode, web};
use serde::{Deserialize, Serialize};

use crate::{json_body, responses};

#[derive(Debug, Deserialize)]
struct SuggestedModelsQuery {
    url: Option<String>,
    #[serde(rename = "type")]
    filter_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ValidateProviderRequest {
    provider: Option<String>,
    #[serde(rename = "apiKey")]
    api_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateProviderRequest {
    provider: Option<String>,
    #[serde(rename = "apiKey")]
    api_key: Option<String>,
    name: Option<String>,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
}

#[derive(Debug, Serialize)]
struct ProvidersResponse {
    connections: Vec<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct SuggestedModelsResponse {
    data: Vec<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct ProviderValidationResponse {
    valid: bool,
    error: Option<&'static str>,
}

#[derive(Debug, Serialize)]
struct ProviderCreateResponse {
    connection: ProviderConnection,
}

#[derive(Debug, Serialize)]
struct ProviderConnection {
    id: String,
    provider: String,
    #[serde(rename = "authType")]
    auth_type: &'static str,
    name: String,
    #[serde(rename = "isActive")]
    is_active: bool,
    #[serde(rename = "testStatus")]
    test_status: &'static str,
}

pub(super) fn configure(config: &mut web::ServiceConfig) {
    config
        .service(
            web::resource("/api/providers")
                .route(web::get().to(list))
                .route(web::post().to(create)),
        )
        .service(
            web::resource("/api/providers/suggested-models").route(web::get().to(suggested_models)),
        )
        .service(web::resource("/api/providers/validate").route(web::post().to(validate)))
        .service(
            web::resource("/api/providers/{id}")
                .route(web::get().to(unknown))
                .route(web::put().to(update_unknown))
                .route(web::delete().to(unknown)),
        );
}

async fn list() -> HttpResponse {
    responses::json(
        StatusCode::OK,
        &ProvidersResponse {
            connections: Vec::new(),
        },
    )
}

/// How long to wait for a provider's public catalogue.
///
/// A dashboard list, so a stale-but-quick answer beats a spinner. Upstream sets no timeout
/// at all, which leaves the route hanging on whatever the catalogue host does.
const CATALOGUE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// The models a provider publishes, filtered to the useful subset.
///
/// **The `url` must be one the registry itself declares.** Upstream fetches whatever URL the
/// caller passes, which is a server-side request forgery primitive: the route is behind
/// dashboard auth, but an authenticated request could still make the server probe hosts the
/// caller cannot reach, and read back whatever came out. Checking against the registry costs
/// nothing, because the dashboard only ever passes a URL it read from the registry.
///
/// This is a deliberate divergence and the one place this route is stricter than upstream.
/// The filters themselves, and the empty-list-on-failure behaviour, are faithful.
async fn suggested_models(query: web::Query<SuggestedModelsQuery>) -> HttpResponse {
    let Some(url) = query.url.as_deref().filter(|value| !value.is_empty()) else {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Missing url or type"),
        );
    };
    let Some(filter_type) = query
        .filter_type
        .as_deref()
        .filter(|value| !value.is_empty())
    else {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Missing url or type"),
        );
    };

    if !nullrouter_providers::registry::declares_models_url(url) {
        // Deliberately says which URLs are allowed rather than just refusing: the caller is
        // the dashboard, and a mismatch means the registry and the dashboard disagree.
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error(
                "url is not a model catalogue declared by any provider in the registry",
            ),
        );
    }

    let Ok(client) = reqwest::Client::builder()
        .timeout(CATALOGUE_TIMEOUT)
        .build()
    else {
        return responses::json(
            StatusCode::OK,
            &SuggestedModelsResponse { data: Vec::new() },
        );
    };

    // Every failure past this point is an empty list, as upstream does: this is a
    // convenience list beside a text field the user can always type into, so a catalogue
    // being down must not present as a dashboard error.
    let body = match client.get(url).send().await {
        Ok(response) if response.status().is_success() => response.text().await.unwrap_or_default(),
        Ok(response) => {
            tracing::info!(
                url,
                status = response.status().as_u16(),
                "model catalogue refused"
            );
            return responses::json(
                StatusCode::OK,
                &SuggestedModelsResponse { data: Vec::new() },
            );
        }
        Err(error) => {
            tracing::info!(url, %error, "model catalogue unreachable");
            return responses::json(
                StatusCode::OK,
                &SuggestedModelsResponse { data: Vec::new() },
            );
        }
    };

    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&body) else {
        return responses::json(
            StatusCode::OK,
            &SuggestedModelsResponse { data: Vec::new() },
        );
    };

    // An unknown filter is a 400 rather than an empty list. An empty list here would read
    // as "this provider publishes no free models", which is a different claim.
    let Some(models) = nullrouter_providers::suggested::filter_catalogue(filter_type, &parsed)
    else {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Unknown filter type"),
        );
    };

    responses::json(StatusCode::OK, &serde_json::json!({ "data": models }))
}

async fn create(body: web::Bytes) -> HttpResponse {
    let request = match json_body::parse::<CreateProviderRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let provider = request
        .provider
        .as_deref()
        .unwrap_or_default()
        .trim()
        .to_owned();

    if !matches!(
        provider.as_str(),
        "openai" | "anthropic" | "gemini" | "ollama-local"
    ) {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Invalid provider"),
        );
    }
    if provider != "ollama-local"
        && request
            .api_key
            .as_deref()
            .is_none_or(|api_key| api_key.trim().is_empty())
    {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("API Key is required"),
        );
    }
    let name = request
        .name
        .or(request.display_name)
        .unwrap_or_else(|| provider.clone());

    responses::json(
        StatusCode::CREATED,
        &ProviderCreateResponse {
            connection: ProviderConnection {
                id: format!("connection_{provider}"),
                provider,
                auth_type: "apikey",
                name,
                is_active: true,
                test_status: "unknown",
            },
        },
    )
}

async fn validate(body: web::Bytes) -> HttpResponse {
    let request = match json_body::parse::<ValidateProviderRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let provider = request.provider.as_deref().unwrap_or_default().trim();
    let api_key = request.api_key.as_deref().unwrap_or_default().trim();

    if provider.is_empty() || (api_key.is_empty() && provider != "ollama-local") {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Provider and API key required"),
        );
    }

    responses::json(
        StatusCode::OK,
        &ProviderValidationResponse {
            valid: false,
            error: Some("Provider validation not supported"),
        },
    )
}

async fn update_unknown(body: web::Bytes) -> HttpResponse {
    let value = match json_body::parse::<serde_json::Value>(&body) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let _ = value;
    unknown().await
}

async fn unknown() -> HttpResponse {
    responses::json(
        StatusCode::NOT_FOUND,
        &responses::error("Connection not found"),
    )
}
