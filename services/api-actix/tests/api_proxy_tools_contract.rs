#![allow(clippy::future_not_send)]

use actix_web::{
    App,
    body::to_bytes,
    http::{Method, StatusCode, header},
    test, web,
};
use serde_json::Value;

use nullrouter_api::{AppConfig, RuntimeClient, StateClient, configure};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A closed loopback port: usage reads fall back to the zeroed shape,
/// so these parity tests need no state service.
const UNREACHABLE_STATE_ADDR: &str = "127.0.0.1:1";

#[derive(Debug)]
struct JsonResponse {
    status: StatusCode,
    content_type: String,
    body: String,
    json: Value,
}

const fn app_config() -> AppConfig {
    AppConfig::new("0.5.20")
}

async fn request_json(method: Method, uri: &str, body: &str) -> TestResult<JsonResponse> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(StateClient::new(UNREACHABLE_STATE_ADDR)))
            .app_data(web::Data::new(RuntimeClient::new(UNREACHABLE_STATE_ADDR)))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::default()
        .method(method)
        .uri(uri)
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(body.to_owned())
        .to_request();

    let res = test::call_service(&app, req).await;
    let status = res.status();
    let content_type = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body_bytes = to_bytes(res.into_body()).await?;
    let body = std::str::from_utf8(&body_bytes)?.to_owned();
    let json = serde_json::from_slice(&body_bytes)?;

    Ok(JsonResponse {
        status,
        content_type,
        body,
        json,
    })
}

fn field<'a>(json: &'a Value, name: &str) -> TestResult<&'a Value> {
    json.get(name)
        .ok_or_else(|| test_error(format!("missing field {name}")))
}

fn assert_structured_json(response: &JsonResponse) {
    assert!(
        response.content_type.starts_with("application/json"),
        "content-type was {}",
        response.content_type
    );
    assert!(!response.body.contains("<html"), "body was HTML");
    assert!(!response.body.contains("<!DOCTYPE"), "body was HTML");
}

#[actix_rt::test]
async fn proxy_pool_test_returns_unsupported_json_when_network_testing_is_unavailable() -> TestResult
{
    // Given: nullrouter-api intentionally does not perform outbound proxy tests.

    // When: the dashboard asks to test a proxy pool entry.
    let response = request_json(Method::POST, "/api/proxy-pools/pool-1/test", "{}").await?;

    // Then: the route preserves the upstream JSON shape while declaring the capability unsupported.
    assert_eq!(response.status, StatusCode::NOT_IMPLEMENTED);
    assert_structured_json(&response);
    assert_eq!(field(&response.json, "id")?, "pool-1");
    assert_eq!(field(&response.json, "ok")?, false);
    assert_eq!(field(&response.json, "status")?, &Value::Null);
    assert_eq!(field(&response.json, "statusText")?, &Value::Null);
    assert_eq!(
        field(&response.json, "error")?,
        "Proxy pool testing is not supported by nullrouter-api"
    );
    assert_eq!(field(&response.json, "elapsedMs")?, 0);
    assert_eq!(field(&response.json, "unsupported")?, true);
    Ok(())
}

#[actix_rt::test]
async fn proxy_pool_deploy_routes_report_an_unreachable_platform() -> TestResult {
    // Given: the three deploy routes, with the platform APIs pointed at a closed port.
    //
    // Pointed deliberately: these routes really deploy now, so without the override this case would
    // make outbound requests to Cloudflare, Deno and Vercel with a dummy token every time the suite
    // ran. The success paths are covered in tests/relay_deploy.rs against a local stub.
    let _guards = (
        ApiBase::new("NULLROUTER_CLOUDFLARE_API", "http://127.0.0.1:1"),
        ApiBase::new("NULLROUTER_DENO_API", "http://127.0.0.1:1"),
        ApiBase::new("NULLROUTER_VERCEL_API", "http://127.0.0.1:1"),
    );

    let cases = [
        ("/api/proxy-pools/vercel-deploy", r#"{"vercelToken":"token"}"#),
        (
            "/api/proxy-pools/cloudflare-deploy",
            r#"{"accountId":"account","apiToken":"token"}"#,
        ),
        (
            "/api/proxy-pools/deno-deploy",
            r#"{"denoToken":"token","orgDomain":"example.com"}"#,
        ),
    ];

    for (uri, body) in cases {
        // When: the deploy is attempted.
        let response = request_json(Method::POST, uri, body).await?;

        // Then: it is a 502 naming the platform it could not reach, rather than a 500 that would
        // read as this router's own bug.
        assert_eq!(response.status, StatusCode::BAD_GATEWAY, "{uri}");
        assert_structured_json(&response);
        assert_eq!(field(&response.json, "success")?, false, "{uri}");
        assert!(
            field(&response.json, "error")?
                .as_str()
                .is_some_and(|error| error.contains("Could not reach")),
            "{uri}: {}",
            response.json
        );
    }
    Ok(())
}

/// Points one platform's API base somewhere harmless for the duration of a case.
struct ApiBase {
    name: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl ApiBase {
    fn new(name: &'static str, value: &str) -> Self {
        let previous = std::env::var_os(name);
        // SAFETY: this binary's cases run on one thread each and only this helper touches these
        // variables, which are restored on drop.
        unsafe { std::env::set_var(name, value) };
        Self { name, previous }
    }
}

impl Drop for ApiBase {
    fn drop(&mut self) {
        match self.previous.take() {
            // SAFETY: as above.
            Some(value) => unsafe { std::env::set_var(self.name, value) },
            // SAFETY: as above.
            None => unsafe { std::env::remove_var(self.name) },
        }
    }
}

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}
