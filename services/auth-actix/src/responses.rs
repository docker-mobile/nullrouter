use actix_web::{
    HttpResponse, HttpResponseBuilder,
    cookie::Cookie,
    http::{StatusCode, header},
};
use serde::Serialize;

pub(crate) fn json<T>(status: StatusCode, body: &T) -> HttpResponse
where
    T: Serialize,
{
    builder(status).json(body)
}

pub(crate) fn json_with_cookie<T>(
    status: StatusCode,
    body: &T,
    cookie: Cookie<'static>,
) -> HttpResponse
where
    T: Serialize,
{
    let mut response = builder(status);
    response.cookie(cookie);
    response.json(body)
}

pub(crate) fn no_content() -> HttpResponse {
    builder(StatusCode::NO_CONTENT).finish()
}

pub(crate) fn builder(status: StatusCode) -> HttpResponseBuilder {
    let mut response = HttpResponse::build(status);
    response.insert_header((header::CACHE_CONTROL, "no-store"));
    response.insert_header((header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"));
    response.insert_header((header::ACCESS_CONTROL_ALLOW_METHODS, "GET, POST, OPTIONS"));
    response.insert_header((header::ACCESS_CONTROL_ALLOW_HEADERS, "content-type"));
    response
}
