use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ErrorEnvelope {
    pub error: ErrorBody,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ErrorBody {
    pub message: String,
    #[serde(rename = "type")]
    pub error_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResponsesFailedEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub response: ResponsesFailedResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResponsesFailedResponse {
    pub id: String,
    pub status: String,
    pub error: ErrorBody,
}

pub fn invalid_request_error(message: impl Into<String>) -> ErrorEnvelope {
    ErrorEnvelope {
        error: ErrorBody {
            message: message.into(),
            error_type: "invalid_request_error".to_owned(),
            code: None,
            model: None,
            stream: None,
        },
    }
}

pub fn provider_execution_error(model: impl Into<String>, stream: bool) -> ErrorEnvelope {
    ErrorEnvelope {
        error: ErrorBody {
            message: "Provider execution is not implemented in this Rust port slice yet".to_owned(),
            error_type: "not_implemented".to_owned(),
            code: Some("provider_execution_unimplemented".to_owned()),
            model: Some(model.into()),
            stream: Some(stream),
        },
    }
}

pub fn responses_failed_event(error: ErrorEnvelope) -> ResponsesFailedEvent {
    ResponsesFailedEvent {
        event_type: "response.failed".to_owned(),
        response: ResponsesFailedResponse {
            id: "resp_provider_execution_unimplemented".to_owned(),
            status: "failed".to_owned(),
            error: error.error,
        },
    }
}
