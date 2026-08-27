use std::collections::BTreeMap;

use serde::{Deserialize, de::DeserializeOwned};

use crate::errors::RuntimeError;

pub(crate) fn parse_json<T>(body: &[u8]) -> Result<T, RuntimeError>
where
    T: DeserializeOwned,
{
    serde_json::from_slice(body).map_err(|_| RuntimeError::bad_request("Invalid JSON body"))
}

#[derive(Debug, Deserialize)]
pub(crate) struct ModelInfoQuery {
    id: Option<String>,
    kind: Option<String>,
}

impl ModelInfoQuery {
    pub(crate) fn required_id(&self) -> Result<&str, RuntimeError> {
        required_text(self.id.as_deref(), "Missing required query param: id")
    }

    pub(crate) fn kind(&self) -> Option<&str> {
        self.kind
            .as_deref()
            .map(str::trim)
            .filter(|kind| !kind.is_empty())
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct ChatPayload {
    model: Option<String>,
    stream: Option<bool>,
}

impl ChatPayload {
    pub(crate) fn required_model(&self) -> Result<&str, RuntimeError> {
        required_text(self.model.as_deref(), "Missing required field: model")
    }

    pub(crate) const fn stream(&self, default: bool) -> bool {
        match self.stream {
            Some(stream) => stream,
            None => default,
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct CountTokensRequest {
    #[serde(default)]
    messages: Vec<TokenMessage>,
    system: Option<TokenValue>,
    tools: Option<TokenValue>,
}

impl CountTokensRequest {
    pub(crate) fn input_tokens(&self) -> usize {
        let system_chars = self.system.as_ref().map_or(0, TokenValue::value_chars);
        let tool_chars = self.tools.as_ref().map_or(0, TokenValue::value_chars);
        let message_chars = self
            .messages
            .iter()
            .map(TokenMessage::content_chars)
            .sum::<usize>();

        chars_to_tokens(system_chars + tool_chars + message_chars)
    }
}

#[derive(Debug, Deserialize)]
struct TokenMessage {
    content: Option<TokenValue>,
}

impl TokenMessage {
    fn content_chars(&self) -> usize {
        self.content
            .as_ref()
            .map_or(0, TokenValue::content_block_chars)
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum TokenValue {
    Text(String),
    Number(serde_json::Number),
    Bool(bool),
    Array(Vec<Self>),
    Object(BTreeMap<String, Self>),
    Null,
}

impl TokenValue {
    fn value_chars(&self) -> usize {
        match self {
            Self::Text(value) => value.len(),
            Self::Number(value) => value.to_string().len(),
            Self::Bool(value) => {
                if *value {
                    4
                } else {
                    5
                }
            }
            Self::Array(values) => values.iter().map(Self::value_chars).sum(),
            Self::Object(values) => values
                .iter()
                .map(|(key, value)| key.len() + value.value_chars())
                .sum(),
            Self::Null => 0,
        }
    }

    fn content_block_chars(&self) -> usize {
        match self {
            Self::Object(values) => match values.get("type").and_then(Self::as_text) {
                Some("text") => count_field(values, "text"),
                Some("tool_use") => count_field(values, "name") + count_field(values, "input"),
                Some("tool_result") => count_field(values, "content"),
                Some("thinking") => count_field(values, "thinking"),
                Some(_) | None => self.value_chars(),
            },
            Self::Array(values) => values.iter().map(Self::content_block_chars).sum(),
            Self::Text(_) | Self::Number(_) | Self::Bool(_) | Self::Null => self.value_chars(),
        }
    }

    const fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(value) => Some(value.as_str()),
            Self::Number(_) | Self::Bool(_) | Self::Array(_) | Self::Object(_) | Self::Null => None,
        }
    }
}

pub(crate) fn gemini_model_from_tail(tail: &str) -> String {
    let model = tail
        .strip_suffix(":streamGenerateContent")
        .or_else(|| tail.strip_suffix(":generateContent"))
        .unwrap_or(tail);

    if model.trim().is_empty() {
        "gemini/unknown".to_owned()
    } else {
        model.to_owned()
    }
}

fn required_text<'a>(
    value: Option<&'a str>,
    missing_message: &'static str,
) -> Result<&'a str, RuntimeError> {
    match value.map(str::trim) {
        Some(value) if !value.is_empty() => Ok(value),
        Some(_) | None => Err(RuntimeError::bad_request(missing_message)),
    }
}

fn count_field(values: &BTreeMap<String, TokenValue>, field: &str) -> usize {
    values.get(field).map_or(0, TokenValue::value_chars)
}

const fn chars_to_tokens(char_count: usize) -> usize {
    if char_count == 0 {
        0
    } else {
        char_count.div_ceil(4)
    }
}
