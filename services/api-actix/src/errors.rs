use actix_web::{HttpResponse, ResponseError, http::StatusCode};
use nullrouter_contracts::invalid_request_error;

use crate::responses;

#[derive(Debug, thiserror::Error)]
pub(super) enum ApiError {
    #[error("{0}")]
    BadRequest(&'static str),
}

impl ResponseError for ApiError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
        }
    }

    fn error_response(&self) -> HttpResponse {
        match self {
            Self::BadRequest(message) => {
                responses::json(StatusCode::BAD_REQUEST, &invalid_request_error(*message))
            }
        }
    }
}
