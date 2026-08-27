#![allow(clippy::future_not_send)]

use actix_web::{
    App,
    body::to_bytes,
    http::{Method, StatusCode, header},
    test, web,
};
use nullrouter_state::{StateStore, configure};
use serde_json::{Value, json};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

struct JsonRequest<'a> {
    method: Method,
    uri: &'a str,
    body: &'a str,
}

struct JsonResponse {
    status: StatusCode,
    body: Value,
}

struct PoolSeed<'a> {
    name: &'a str,
    proxy_url: &'a str,
    no_proxy: &'a str,
    proxy_type: &'a str,
    is_active: bool,
    strict_proxy: bool,
}

const fn request<'a>(method: Method, uri: &'a str, body: &'a str) -> JsonRequest<'a> {
    JsonRequest { method, uri, body }
}

async fn request_json(store: StateStore, request: JsonRequest<'_>) -> TestResult<JsonResponse> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(store))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::default()
        .method(request.method)
        .uri(request.uri)
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(request.body.to_owned())
        .to_request();
    let res = test::call_service(&app, req).await;
    let status = res.status();
    let body = to_bytes(res.into_body()).await?;
    Ok(JsonResponse {
        status,
        body: serde_json::from_slice(&body)?,
    })
}

async fn get_json(store: StateStore, uri: &str) -> TestResult<JsonResponse> {
    request_json(store, request(Method::GET, uri, "")).await
}

async fn create_pool(store: StateStore, seed: PoolSeed<'_>) -> TestResult<JsonResponse> {
    let body = json!({
        "name": seed.name,
        "proxyUrl": seed.proxy_url,
        "noProxy": seed.no_proxy,
        "type": seed.proxy_type,
        "isActive": seed.is_active,
        "strictProxy": seed.strict_proxy,
    })
    .to_string();
    request_json(store, request(Method::POST, "/api/proxy-pools", &body)).await
}

fn field<'a>(json: &'a Value, name: &str) -> TestResult<&'a Value> {
    json.get(name)
        .ok_or_else(|| test_error(format!("missing field {name}")))
}

fn string_field(json: &Value, name: &str) -> TestResult<String> {
    field(json, name)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| test_error(format!("{name} is not a string")))
}

fn proxy_pools(json: &Value) -> TestResult<&Vec<Value>> {
    field(json, "proxyPools")?
        .as_array()
        .ok_or_else(|| test_error("proxyPools is not an array"))
}

fn proxy_pool_ids(json: &Value) -> TestResult<Vec<String>> {
    proxy_pools(json)?
        .iter()
        .map(|pool| string_field(pool, "id"))
        .collect()
}

fn pool_by_id<'a>(json: &'a Value, id: &str) -> TestResult<&'a Value> {
    proxy_pools(json)?
        .iter()
        .find(|pool| field(pool, "id").ok().and_then(Value::as_str) == Some(id))
        .ok_or_else(|| test_error(format!("missing proxy pool {id}")))
}

#[actix_rt::test]
async fn proxy_pool_routes_match_upstream_crud_filter_usage_and_type_contract() -> TestResult {
    let store = StateStore::memory();

    let http = create_pool(
        store.clone(),
        PoolSeed {
            name: "HTTP pool",
            proxy_url: "http://127.0.0.1:8888",
            no_proxy: "",
            proxy_type: "http",
            is_active: true,
            strict_proxy: false,
        },
    )
    .await?;
    assert_eq!(http.status, StatusCode::CREATED);
    let http_pool = field(&http.body, "proxyPool")?;
    assert_eq!(field(http_pool, "type")?, "http");
    let http_id = string_field(http_pool, "id")?;

    let vercel = create_pool(
        store.clone(),
        PoolSeed {
            name: "Vercel pool",
            proxy_url: "https://vercel.example",
            no_proxy: "",
            proxy_type: "vercel",
            is_active: true,
            strict_proxy: false,
        },
    )
    .await?;
    assert_eq!(vercel.status, StatusCode::CREATED);
    let vercel_pool = field(&vercel.body, "proxyPool")?;
    assert_eq!(field(vercel_pool, "type")?, "vercel");
    let vercel_id = string_field(vercel_pool, "id")?;

    let cloudflare = create_pool(
        store.clone(),
        PoolSeed {
            name: "Cloudflare pool",
            proxy_url: "https://cloudflare.example",
            no_proxy: "",
            proxy_type: "cloudflare",
            is_active: false,
            strict_proxy: false,
        },
    )
    .await?;
    assert_eq!(cloudflare.status, StatusCode::CREATED);
    let cloudflare_pool = field(&cloudflare.body, "proxyPool")?;
    assert_eq!(field(cloudflare_pool, "type")?, "cloudflare");
    let cloudflare_id = string_field(cloudflare_pool, "id")?;

    let deno = create_pool(
        store.clone(),
        PoolSeed {
            name: " Deno pool ",
            proxy_url: " https://deno.example ",
            no_proxy: " localhost,127.0.0.1 ",
            proxy_type: "deno",
            is_active: true,
            strict_proxy: true,
        },
    )
    .await?;
    assert_eq!(deno.status, StatusCode::CREATED);
    let deno_pool = field(&deno.body, "proxyPool")?;
    assert_eq!(field(deno_pool, "name")?, "Deno pool");
    assert_eq!(field(deno_pool, "proxyUrl")?, "https://deno.example");
    assert_eq!(field(deno_pool, "noProxy")?, "localhost,127.0.0.1");
    assert_eq!(field(deno_pool, "type")?, "deno");
    assert_eq!(field(deno_pool, "isActive")?, true);
    assert_eq!(field(deno_pool, "strictProxy")?, true);
    assert_eq!(field(deno_pool, "testStatus")?, "unknown");
    assert!(field(deno_pool, "lastTestedAt")?.is_null());
    assert!(field(deno_pool, "lastError")?.is_null());
    let deno_id = string_field(deno_pool, "id")?;

    let provider_body = json!({
        "provider": "openai",
        "apiKey": "sk-state",
        "name": "Usage Provider",
        "proxyPoolId": vercel_id,
    })
    .to_string();
    let provider = request_json(
        store.clone(),
        request(Method::POST, "/api/providers", &provider_body),
    )
    .await?;
    assert_eq!(provider.status, StatusCode::CREATED);

    let active = get_json(store.clone(), "/api/proxy-pools?isActive=true").await?;
    assert_eq!(active.status, StatusCode::OK);
    let active_ids = proxy_pool_ids(&active.body)?;
    assert_eq!(active_ids.len(), 3);
    assert!(active_ids.iter().any(|id| id == &http_id));
    assert!(active_ids.iter().any(|id| id == &vercel_id));
    assert!(active_ids.iter().any(|id| id == &deno_id));

    let inactive = get_json(store.clone(), "/api/proxy-pools?isActive=false").await?;
    assert_eq!(inactive.status, StatusCode::OK);
    let inactive_ids = proxy_pool_ids(&inactive.body)?;
    assert_eq!(inactive_ids, vec![cloudflare_id]);

    let usage = get_json(store.clone(), "/api/proxy-pools?includeUsage=true").await?;
    assert_eq!(usage.status, StatusCode::OK);
    assert_eq!(
        field(pool_by_id(&usage.body, &vercel_id)?, "boundConnectionCount")?,
        1
    );
    assert_eq!(
        field(pool_by_id(&usage.body, &deno_id)?, "boundConnectionCount")?,
        0
    );

    let fetched = get_json(store.clone(), &format!("/api/proxy-pools/{deno_id}")).await?;
    assert_eq!(fetched.status, StatusCode::OK);
    assert_eq!(
        field(field(&fetched.body, "proxyPool")?, "id")?,
        deno_id.as_str()
    );

    let update_body = json!({
        "name": "Updated Deno",
        "proxyUrl": " https://updated.example ",
        "noProxy": " intranet ",
        "type": "deno",
        "isActive": false,
        "strictProxy": false,
    })
    .to_string();
    let updated = request_json(
        store.clone(),
        request(
            Method::PUT,
            &format!("/api/proxy-pools/{deno_id}"),
            &update_body,
        ),
    )
    .await?;
    assert_eq!(updated.status, StatusCode::OK);
    let updated_pool = field(&updated.body, "proxyPool")?;
    assert_eq!(field(updated_pool, "name")?, "Updated Deno");
    assert_eq!(field(updated_pool, "proxyUrl")?, "https://updated.example");
    assert_eq!(field(updated_pool, "noProxy")?, "intranet");
    assert_eq!(field(updated_pool, "type")?, "http");
    assert_eq!(field(updated_pool, "isActive")?, false);
    assert_eq!(field(updated_pool, "strictProxy")?, false);
    assert_eq!(field(updated_pool, "testStatus")?, "unknown");
    assert!(field(updated_pool, "lastTestedAt")?.is_null());
    assert!(field(updated_pool, "lastError")?.is_null());

    let deleted = request_json(
        store,
        request(Method::DELETE, &format!("/api/proxy-pools/{http_id}"), ""),
    )
    .await?;
    assert_eq!(deleted.status, StatusCode::OK);
    assert_eq!(field(&deleted.body, "success")?, true);
    Ok(())
}

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}
