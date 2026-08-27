use actix_web::{HttpResponse, ResponseError, http::StatusCode};
use nullrouter_contracts::invalid_request_error;

use crate::responses;

#[derive(Debug, thiserror::Error)]
pub(crate) enum RuntimeError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    NotFound(String),
}

impl RuntimeError {
    pub(crate) fn bad_request(message: &'static str) -> Self {
        Self::BadRequest(message.to_owned())
    }

    pub(crate) fn not_found_model(id: &str) -> Self {
        Self::NotFound(format!("Model not found: {id}"))
    }
}

impl ResponseError for RuntimeError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
        }
    }

    fn error_response(&self) -> HttpResponse {
        match self {
            Self::BadRequest(message) => {
                responses::json(StatusCode::BAD_REQUEST, &invalid_request_error(message))
            }
            Self::NotFound(message) => responses::json(
                StatusCode::NOT_FOUND,
                &serde_json::json!({
                    "error": {
                        "message": message,
                        "type": "not_found",
                    },
                }),
            ),
        }
    }
}
