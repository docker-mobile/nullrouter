#![allow(clippy::future_not_send)]

mod api_mitm_support;

use actix_web::http::{Method, StatusCode};
use serde_json::{Value, json};

use api_mitm_support::{TestResult, request_json};

const MITM_URI: &str = "/api/cli-tools/antigravity-mitm";
const MITM_UNSUPPORTED: &str = "Antigravity MITM control is not supported by nullrouter-api";
const INVALID_JSON: &str = "Invalid JSON body";
const INVALID_URL: &str = "Invalid MITM router URL";
const INVALID_PROTOCOL: &str = "MITM router URL must use http or https";
const REQUIRED_ACTION: &str = "tool and action required";
const INVALID_ACTION: &str = "action must be enable, disable, or trust-cert";

fn unsupported_json() -> Value {
    json!({
        "success": false,
        "unsupported": true,
        "message": MITM_UNSUPPORTED,
    })
}

fn assert_error(response: &(StatusCode, Value), status: StatusCode, error: &str) {
    assert_eq!(response, &(status, json!({ "error": error })));
}

fn assert_unsupported(response: &(StatusCode, Value)) {
    assert_eq!(response, &(StatusCode::NOT_IMPLEMENTED, unsupported_json()));
}

#[actix_rt::test]
async fn mitm_get_returns_the_safe_status_contract() -> TestResult {
    // Given: nullrouter-api must never inspect host MITM state.
    // When: the explicit MITM status endpoint is requested.
    let (status, body) = request_json(Method::GET, MITM_URI, "").await?;
    // Then: the complete deterministic status contract is returned with no extra fields.
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({
            "running": false,
            "pid": null,
            "certExists": false,
            "certTrusted": false,
            "dnsStatus": {
                "antigravity": false,
                "copilot": false,
                "cursor": false,
                "kiro": false,
            },
            "hasCachedPassword": false,
            "isWin": cfg!(windows),
            "needsSudoPassword": false,
            "isAdmin": false,
            "mitmRouterBaseUrl": "http://localhost:20128",
        })
    );
    Ok(())
}

#[actix_rt::test]
async fn mitm_post_validates_json_api_key_and_router_url() -> TestResult {
    // Given: starting MITM is unsupported but retains the upstream boundary contract.
    // When: malformed, incomplete, or invalid requests are posted.
    for body in ["", "{"] {
        assert_error(
            &request_json(Method::POST, MITM_URI, body).await?,
            StatusCode::BAD_REQUEST,
            INVALID_JSON,
        );
    }
    for body in [
        "{}",
        r#"{"apiKey":null}"#,
        r#"{"apiKey":""}"#,
        r#"{"apiKey":"   "}"#,
    ] {
        assert_error(
            &request_json(Method::POST, MITM_URI, body).await?,
            StatusCode::BAD_REQUEST,
            "Missing apiKey",
        );
    }
    for url in ["/relative", "router.example", "http:/missing-authority"] {
        let body = json!({"apiKey": "key", "mitmRouterBaseUrl": url}).to_string();
        assert_error(
            &request_json(Method::POST, MITM_URI, &body).await?,
            StatusCode::BAD_REQUEST,
            INVALID_URL,
        );
    }
    assert_error(
        &request_json(
            Method::POST,
            MITM_URI,
            r#"{"apiKey":"key","mitmRouterBaseUrl":"ftp://router.example"}"#,
        )
        .await?,
        StatusCode::BAD_REQUEST,
        INVALID_PROTOCOL,
    );
    // Then: blank/default and valid absolute HTTP(S) requests reach only the unsupported response.
    for body in [
        r#"{"apiKey":"key"}"#,
        r#"{"apiKey":"key","mitmRouterBaseUrl":""}"#,
        r#"{"apiKey":"key","mitmRouterBaseUrl":"   "}"#,
        r#"{"apiKey":" key ","mitmRouterBaseUrl":"http://localhost:20128"}"#,
        r#"{"apiKey":"key","mitmRouterBaseUrl":"https://router.example/path"}"#,
    ] {
        assert_unsupported(&request_json(Method::POST, MITM_URI, body).await?);
    }
    Ok(())
}

#[actix_rt::test]
async fn mitm_delete_ignores_every_body_shape() -> TestResult {
    // Given: DELETE must not parse or act on caller data.
    // When: it receives empty, malformed, or irrelevant bodies.
    for body in ["", "{", "not-json", r#"{"irrelevant":true}"#] {
        // Then: every body reaches the same side-effect-free unsupported response.
        assert_unsupported(&request_json(Method::DELETE, MITM_URI, body).await?);
    }
    Ok(())
}

#[actix_rt::test]
async fn mitm_patch_validates_required_fields_and_actions() -> TestResult {
    // Given: PATCH recognizes only three action envelopes.
    // When: malformed, incomplete, invalid, and valid actions are submitted.
    for body in ["", "{"] {
        assert_error(
            &request_json(Method::PATCH, MITM_URI, body).await?,
            StatusCode::BAD_REQUEST,
            INVALID_JSON,
        );
    }
    for body in [
        "{}",
        r#"{"tool":"antigravity"}"#,
        r#"{"action":"enable"}"#,
        r#"{"tool":"","action":"enable"}"#,
        r#"{"tool":"   ","action":"enable"}"#,
        r#"{"tool":"antigravity","action":"   "}"#,
    ] {
        assert_error(
            &request_json(Method::PATCH, MITM_URI, body).await?,
            StatusCode::BAD_REQUEST,
            REQUIRED_ACTION,
        );
    }
    assert_error(
        &request_json(
            Method::PATCH,
            MITM_URI,
            r#"{"tool":"antigravity","action":"restart"}"#,
        )
        .await?,
        StatusCode::BAD_REQUEST,
        INVALID_ACTION,
    );
    // Then: enable, disable, and trust-cert are accepted but never executed.
    for action in ["enable", "disable", "trust-cert"] {
        let body = if action == "trust-cert" {
            json!({"action": action}).to_string()
        } else {
            json!({"tool": "antigravity", "action": action}).to_string()
        };
        assert_unsupported(&request_json(Method::PATCH, MITM_URI, &body).await?);
    }
    Ok(())
}
