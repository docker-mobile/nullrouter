use std::time::Instant;

use actix_web::{HttpResponse, http::StatusCode, web};
use serde::Serialize;
use serde_json::Value;

use crate::{
    json_body, model_tools::current_iso8601, proxy_test, responses, state_client::StateClient,
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PoolTestResult {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    elapsed_ms: u64,
    tested_at: String,
}

pub(super) fn configure(config: &mut web::ServiceConfig) {
    config.service(
        web::resource("/api/proxy-pools/{id}/test")
            .route(web::post().to(test_pool))
            .route(web::method(actix_web::http::Method::OPTIONS).to(options)),
    );
    // The three relay deploys live in their own module: they are the only routes here that make
    // authenticated calls to a third-party platform on a user's behalf.
    crate::relay_deploy::configure(config);
}

async fn test_pool(
    state: web::Data<StateClient>,
    path: web::Path<String>,
    body: web::Bytes,
) -> HttpResponse {
    if let Err(response) = json_body::parse_optional::<Value>(&body) {
        return response;
    }
    let id = path.into_inner();
    let Some(pool) = state.get_proxy_pool(&id).await else {
        return responses::json(
            StatusCode::NOT_IMPLEMENTED,
            &serde_json::json!({
                "id": id,
                "ok": false,
                "status": null,
                "statusText": null,
                "error": "Proxy pool testing is not supported by nullrouter-api",
                "elapsedMs": 0,
                "unsupported": true,
            }),
        );
    };

    let proxy_url = pool
        .get("proxyUrl")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let proxy_type = pool.get("type").and_then(Value::as_str).unwrap_or("http");

    let started = Instant::now();
    let is_relay = matches!(proxy_type, "vercel" | "cloudflare" | "deno");

    let (ok, status, status_text, error) = if is_relay {
        test_relay(proxy_url).await
    } else {
        match proxy_test::validate(Some(proxy_url), None) {
            Ok((valid_proxy, target)) => {
                let outcome =
                    proxy_test::run(&valid_proxy, &target, proxy_test::DEFAULT_TIMEOUT).await;
                let text = outcome.status.map(|s| format!("HTTP {s}"));
                (outcome.ok, outcome.status, text, outcome.error)
            }
            Err(refusal) => (false, Some(400), None, Some(refusal.message())),
        }
    };

#[allow(clippy::cast_possible_truncation)]
    let elapsed_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    let tested_at = current_iso8601();

    let test_status = if ok { "active" } else { "error" };
    let pool_update = serde_json::json!({
        "testStatus": test_status,
        "isActive": ok,
        "lastTestedAt": tested_at.clone(),
        "lastError": error.clone(),
    });
    let _ = state.update_proxy_pool(&id, &pool_update).await;

    responses::json(
        StatusCode::OK,
        &PoolTestResult {
            ok,
            status,
            status_text,
            error,
            elapsed_ms,
            tested_at,
        },
    )
}

async fn test_relay(relay_url: &str) -> (bool, Option<u16>, Option<String>, Option<String>) {
    let Ok(client) = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    else {
        return (
            false,
            None,
            None,
            Some("Failed to build HTTP client".to_owned()),
        );
    };

    match client
        .get(relay_url)
        .header("x-relay-target", "https://httpbin.org")
        .header("x-relay-path", "/get")
        .send()
        .await
    {
        Ok(response) => {
            let status = response.status();
            let code = status.as_u16();
            let ok = status.is_success();
            let text = status.canonical_reason().unwrap_or_default().to_owned();
            let err = if ok {
                None
            } else {
                Some(format!("Relay answered HTTP {code}"))
            };
            (ok, Some(code), Some(text), err)
        }
        Err(error) => {
            let msg = if error.is_timeout() {
                "Relay test timed out".to_owned()
            } else {
                format!("Relay test failed: {error}")
            };
            (false, Some(500), None, Some(msg))
        }
    }
}

async fn options() -> HttpResponse {
    responses::empty(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use actix_web::{App, http::StatusCode, test, web};

    use crate::state_client::StateClient;

    #[actix_rt::test]
    async fn non_existent_proxy_pool_test_returns_501_unsupported() {
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(StateClient::new("http://127.0.0.1:9")))
                .configure(super::configure),
        )
        .await;

        let req = test::TestRequest::post()
            .uri("/api/proxy-pools/non-existent-pool/test")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
    }
}
