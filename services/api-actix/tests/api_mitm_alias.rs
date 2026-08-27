#![allow(clippy::future_not_send)]

mod api_mitm_support;

use actix_web::http::{Method, StatusCode, header};
use serde_json::{Value, json};

use api_mitm_support::{TestResult, request, request_json};

const MITM_URI: &str = "/api/cli-tools/antigravity-mitm";
const ALIAS_URI: &str = "/api/cli-tools/antigravity-mitm/alias";
const INVALID_JSON: &str = "Invalid JSON body";
const REQUIRED_MAPPINGS: &str = "tool and mappings required";
const METHOD_NOT_ALLOWED: &str = "Method not allowed";

fn assert_error(response: &(StatusCode, Value), status: StatusCode, error: &str) {
    assert_eq!(response, &(status, json!({ "error": error })));
}

#[actix_rt::test]
async fn mitm_alias_get_and_put_match_the_dns_guard_contract() -> TestResult {
    // Given: nullrouter-api has no aliases or enabled MITM DNS state.
    // When: aliases are read or mapping updates cross the JSON boundary.
    let get = request_json(
        Method::GET,
        "/api/cli-tools/antigravity-mitm/alias?tool=x",
        "",
    )
    .await?;
    assert_eq!(get, (StatusCode::OK, json!({"aliases": {}})));
    for body in ["", "{"] {
        assert_error(
            &request_json(Method::PUT, ALIAS_URI, body).await?,
            StatusCode::BAD_REQUEST,
            INVALID_JSON,
        );
    }
    for body in [
        "{}",
        r#"{"tool":"antigravity"}"#,
        r#"{"tool":"","mappings":{}}"#,
        r#"{"tool":"   ","mappings":{}}"#,
        r#"{"tool":"antigravity","mappings":null}"#,
        r#"{"tool":"antigravity","mappings":[]}"#,
        r#"{"tool":"antigravity","mappings":"model"}"#,
        r#"{"tool":"antigravity","mappings":true}"#,
    ] {
        assert_error(
            &request_json(Method::PUT, ALIAS_URI, body).await?,
            StatusCode::BAD_REQUEST,
            REQUIRED_MAPPINGS,
        );
    }
    // Then: a valid object is denied by the deterministic DNS guard without writes.
    assert_error(
        &request_json(
            Method::PUT,
            ALIAS_URI,
            r#"{"tool":"antigravity","mappings":{"Default":"openai/gpt-4.1"}}"#,
        )
        .await?,
        StatusCode::FORBIDDEN,
        "DNS must be enabled for antigravity before editing model mappings",
    );
    Ok(())
}

#[actix_rt::test]
async fn mitm_routes_answer_options_and_json_method_not_allowed() -> TestResult {
    // Given: both explicit resources expose only their assigned methods.
    // When: callers use OPTIONS or an unsupported method.
    for (uri, allow) in [
        (MITM_URI, "GET, POST, DELETE, PATCH, OPTIONS"),
        (ALIAS_URI, "GET, PUT, OPTIONS"),
    ] {
        let response = request(Method::OPTIONS, uri, "").await?;
        assert_eq!(response.status, StatusCode::NO_CONTENT);
        assert_eq!(
            response
                .headers
                .get(header::ALLOW)
                .and_then(|value| value.to_str().ok()),
            Some(allow)
        );
        assert!(response.body.is_empty());
    }
    let brew = Method::from_bytes(b"BREW")?;
    for method in [Method::PUT, brew.clone()] {
        let response = request(method, MITM_URI, "{}").await?;
        assert_eq!(response.status, StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(
            response
                .headers
                .get(header::ALLOW)
                .and_then(|value| value.to_str().ok()),
            Some("GET, POST, DELETE, PATCH, OPTIONS")
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&response.body)?,
            json!({"error": METHOD_NOT_ALLOWED})
        );
    }
    // Then: every alias-only unsupported method returns the same JSON 405.
    for method in [Method::POST, Method::PATCH, Method::DELETE, brew] {
        let response = request(method, ALIAS_URI, "{}").await?;
        assert_eq!(response.status, StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(
            response
                .headers
                .get(header::ALLOW)
                .and_then(|value| value.to_str().ok()),
            Some("GET, PUT, OPTIONS")
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&response.body)?,
            json!({"error": METHOD_NOT_ALLOWED})
        );
    }
    Ok(())
}
