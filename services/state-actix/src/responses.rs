use actix_web::{
    HttpResponse, HttpResponseBuilder,
    http::{
        StatusCode,
        header::{
            ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS,
            ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_EXPOSE_HEADERS,
        },
    },
};
use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct ErrorBody {
    error: &'static str,
}

pub(crate) const fn error(message: &'static str) -> ErrorBody {
    ErrorBody { error: message }
}

pub(crate) fn json<T>(status: StatusCode, body: &T) -> HttpResponse
where
    T: Serialize,
{
    let mut builder = HttpResponse::build(status);
    apply_cors(&mut builder);
    builder.json(body)
}

pub(crate) fn no_content() -> HttpResponse {
    let mut builder = HttpResponse::NoContent();
    apply_cors(&mut builder);
    builder.finish()
}

fn apply_cors(builder: &mut HttpResponseBuilder) {
    builder.insert_header((ACCESS_CONTROL_ALLOW_ORIGIN, "*"));
    builder.insert_header((
        ACCESS_CONTROL_ALLOW_METHODS,
        "GET, POST, PUT, PATCH, DELETE, OPTIONS",
    ));
    builder.insert_header((ACCESS_CONTROL_ALLOW_HEADERS, "*"));
    builder.insert_header((ACCESS_CONTROL_EXPOSE_HEADERS, "*"));
}
