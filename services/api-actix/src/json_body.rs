use actix_web::{HttpResponse, http::StatusCode};
use serde::de::DeserializeOwned;

use crate::responses;

pub(super) fn parse<T>(body: &[u8]) -> Result<T, HttpResponse>
where
    T: DeserializeOwned,
{
    serde_json::from_slice(body).map_err(|_| {
        responses::json(
            StatusCode::BAD_REQUEST,
            &responses::error("Invalid JSON body"),
        )
    })
}

pub(super) fn parse_optional<T>(body: &[u8]) -> Result<Option<T>, HttpResponse>
where
    T: DeserializeOwned,
{
    if body.is_empty() {
        return Ok(None);
    }
    parse(body).map(Some)
}
