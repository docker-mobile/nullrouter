use actix_web::{
    App,
    body::to_bytes,
    http::{Method, StatusCode, header},
    test, web,
};
use serde_json::Value;

use nullrouter_api::{AppConfig, configure};

pub(crate) type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const fn app_config() -> AppConfig {
    AppConfig::new("0.5.20")
}

pub(crate) struct ApiResponse {
    pub(crate) status: StatusCode,
    pub(crate) headers: header::HeaderMap,
    pub(crate) body: Vec<u8>,
}

pub(crate) async fn request(method: Method, uri: &str, body: &str) -> TestResult<ApiResponse> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
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
    let headers = res.headers().clone();
    let body = to_bytes(res.into_body()).await?.to_vec();
    Ok(ApiResponse {
        status,
        headers,
        body,
    })
}

pub(crate) async fn request_json(
    method: Method,
    uri: &str,
    body: &str,
) -> TestResult<(StatusCode, Value)> {
    let ApiResponse {
        status,
        headers: _headers,
        body,
    } = request(method, uri, body).await?;
    Ok((status, serde_json::from_slice(&body)?))
}
