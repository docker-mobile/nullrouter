use actix_web::{HttpResponse, ResponseError, http::StatusCode};
use thiserror::Error;

use crate::{
    contracts::{ErrorBody, ErrorEnvelope},
    responses,
};

#[derive(Debug, Error)]
pub(crate) enum ApiError {
    #[error("invalid JSON body")]
    InvalidJson,
    #[error("request body exceeds the configured limit")]
    BodyTooLarge,
    #[error("password is required")]
    PasswordRequired,
    #[error("peer identity is unavailable")]
    PeerIdentityUnavailable,
    #[error("loopback peer is required")]
    LoopbackRequired,
    #[error("authentication service state is unavailable")]
    InternalStateUnavailable,
}

impl ResponseError for ApiError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::InvalidJson | Self::PasswordRequired | Self::PeerIdentityUnavailable => {
                StatusCode::BAD_REQUEST
            }
            Self::BodyTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::LoopbackRequired => StatusCode::FORBIDDEN,
            Self::InternalStateUnavailable => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn error_response(&self) -> HttpResponse {
        let (code, message) = match self {
            Self::InvalidJson => ("invalid_json", "Invalid JSON body"),
            Self::BodyTooLarge => ("body_too_large", "Request body is too large"),
            Self::PasswordRequired => ("password_required", "Password is required"),
            Self::PeerIdentityUnavailable => {
                ("peer_identity_unavailable", "Peer identity is unavailable")
            }
            Self::LoopbackRequired => ("loopback_required", "Loopback peer required"),
            Self::InternalStateUnavailable => (
                "internal_state_unavailable",
                "Authentication state is unavailable",
            ),
        };
        responses::json(
            self.status_code(),
            &ErrorEnvelope {
                error: ErrorBody {
                    code,
                    message,
                    error_type: "request_error",
                },
            },
        )
    }
}

pub(crate) fn protocol_error(
    status: StatusCode,
    code: &'static str,
    message: &'static str,
) -> HttpResponse {
    responses::json(
        status,
        &ErrorEnvelope {
            error: ErrorBody {
                code,
                message,
                error_type: "request_error",
            },
        },
    )
}
