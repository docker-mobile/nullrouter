//! A locale that was set reads back as set.
//!
//! `POST /api/locale` used to validate the tag, answer `success: true`, and store nothing, while `GET`
//! always replied `en`/`default`. A client that set a language and read it back was told its change
//! had not happened. The preference now goes in the `locale` cookie -- the same one the dashboard
//! reads to choose its message table -- so the write lands somewhere that is actually consulted.
//!
//! A cookie rather than stored state on purpose: the preference belongs to a browser, not to the
//! install, and two operators sharing one router should not change each other's language.
#![allow(clippy::future_not_send)]

use actix_web::{
    App,
    body::to_bytes,
    cookie::Cookie,
    http::{Method, StatusCode, header},
    test, web,
};
use serde_json::Value;

use nullrouter_api::{AppConfig, RuntimeClient, StateClient, TunnelManager, configure};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const UNREACHABLE_STATE_ADDR: &str = "127.0.0.1:1";

const fn app_config() -> AppConfig {
    AppConfig::new("0.5.20")
}

/// The `locale` cookie a response set, if any.
struct Outcome {
    status: StatusCode,
    body: Value,
    locale_cookie: Option<String>,
}

async fn call(
    method: Method,
    uri: &str,
    body: &str,
    sent_cookie: Option<&str>,
) -> TestResult<Outcome> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(StateClient::new(UNREACHABLE_STATE_ADDR)))
            .app_data(web::Data::new(RuntimeClient::new(UNREACHABLE_STATE_ADDR)))
            .app_data(web::Data::new(TunnelManager::new()))
            .configure(configure),
    )
    .await;
    let mut request = test::TestRequest::default()
        .method(method)
        .uri(uri)
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(body.to_owned());
    if let Some(value) = sent_cookie {
        request = request.cookie(Cookie::new("locale", value));
    }
    let response = test::call_service(&app, request.to_request()).await;
    let status = response.status();
    let locale_cookie = response
        .response()
        .cookies()
        .find(|cookie| cookie.name() == "locale")
        .map(|cookie| cookie.value().to_owned());
    let bytes = to_bytes(response.into_body()).await?;
    Ok(Outcome {
        status,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        locale_cookie,
    })
}

fn field<'a>(json: &'a Value, name: &str) -> TestResult<&'a Value> {
    json.get(name).ok_or_else(|| {
        let error: Box<dyn std::error::Error> =
            Box::new(std::io::Error::other(format!("missing field {name}")));
        error
    })
}

#[actix_rt::test]
async fn a_set_locale_reads_back_as_set() -> TestResult {
    // With nothing sent, the default is reported and named as a default rather than as a choice.
    let fresh = call(Method::GET, "/api/locale", "", None).await?;
    assert_eq!(fresh.status, StatusCode::OK);
    assert_eq!(field(&fresh.body, "locale")?, "en");
    assert_eq!(field(&fresh.body, "source")?, "default");

    // Setting one must actually set the cookie the dashboard reads. This is the assertion the stub
    // could not make: it answered `success: true` and sent no `Set-Cookie` at all.
    let set = call(Method::POST, "/api/locale", r#"{"locale":"ja"}"#, None).await?;
    assert_eq!(set.status, StatusCode::OK);
    assert_eq!(field(&set.body, "success")?, true);
    assert_eq!(field(&set.body, "locale")?, "ja");
    assert_eq!(
        set.locale_cookie.as_deref(),
        Some("ja"),
        "the response set no locale cookie, so nothing the dashboard reads changed"
    );

    // And reading it back with that cookie must agree, rather than reporting the default.
    let read_back = call(Method::GET, "/api/locale", "", Some("ja")).await?;
    assert_eq!(field(&read_back.body, "locale")?, "ja");
    assert_eq!(field(&read_back.body, "source")?, "cookie");
    Ok(())
}

#[actix_rt::test]
async fn an_alias_tag_is_normalised_on_the_way_in_and_out() -> TestResult {
    // `zh` is accepted and stored as `zh-CN`, because that is the file that exists. Reporting `zh`
    // back would name a message table the dashboard cannot load.
    let set = call(Method::POST, "/api/locale", r#"{"locale":"zh"}"#, None).await?;
    assert_eq!(field(&set.body, "locale")?, "zh-CN");
    assert_eq!(set.locale_cookie.as_deref(), Some("zh-CN"));

    let read_back = call(Method::GET, "/api/locale", "", Some("zh")).await?;
    assert_eq!(field(&read_back.body, "locale")?, "zh-CN");
    Ok(())
}

#[actix_rt::test]
async fn an_unusable_cookie_reads_as_unset() -> TestResult {
    // A tag from an older build, or one a user edited by hand, must not be echoed back as the current
    // locale: the dashboard would then request a file that does not exist and fall back anyway, after
    // a wasted round trip.
    for value in ["klingon", "", "../../etc/passwd", "en-US-x-hacked"] {
        let response = call(Method::GET, "/api/locale", "", Some(value)).await?;
        assert_eq!(response.status, StatusCode::OK, "{value}");
        assert_eq!(field(&response.body, "locale")?, "en", "{value}");
        assert_eq!(field(&response.body, "source")?, "default", "{value}");
    }
    Ok(())
}

#[actix_rt::test]
async fn an_unsupported_locale_is_refused_and_sets_nothing() -> TestResult {
    // The refusal has to leave the existing preference alone. Clearing it on a bad request would let
    // one typo reset a language that was set correctly.
    for body in [r#"{"locale":"klingon"}"#, r#"{"locale":"  "}"#, "{}"] {
        let response = call(Method::POST, "/api/locale", body, Some("ja")).await?;
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{body}");
        assert!(
            response.locale_cookie.is_none(),
            "a refused request touched the locale cookie: {body}"
        );
    }
    // The preference that was already there still reads back.
    let unchanged = call(Method::GET, "/api/locale", "", Some("ja")).await?;
    assert_eq!(field(&unchanged.body, "locale")?, "ja");
    Ok(())
}
