//! Live Basic Chat state: what is sent, what comes back, and what can be picked.
//!
//! The panel this backs rendered a transcript of two hard-coded bubbles
//! explaining that execution was unwired, above a composer whose textarea was
//! `disabled`. Provider execution has since landed (`crates/execute`, wired
//! through `runtime-actix`), and `POST /api/dashboard/chat/completions` forwards
//! to it, so the composer can send for real.
//!
//! Three rules shape this module:
//!
//! * A model can only be offered if a connection for its provider exists.
//!   [`model_options`] derives the menu from `GET /api/providers` joined with the
//!   embedded registry, so the menu never lists a model the router has no
//!   credential for.
//! * A reply is only rendered if the response carried one. [`parse_reply`]
//!   returns `None` for a body with no assistant content, which surfaces as a
//!   visible failure rather than an empty bubble.
//! * An error is rendered as itself. The runtime's OpenAI-shaped
//!   `{"error":{"message":…}}` envelope is read and quoted, so a provider's own
//!   explanation reaches the user instead of a generic "request failed".
//!
//! Kept free of `leptos` and of `fetch` so every derivation is unit testable on
//! the native target.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::ApiError;
use crate::dashboard::providers_live::{ConnectionList, provider_label};

/// The endpoint that executes a dashboard chat turn.
pub const CHAT_PATH: &str = "/api/dashboard/chat/completions";

/// Who authored a transcript entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    User,
    Assistant,
}

impl Role {
    /// Wire value for the request payload.
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }

    /// Bubble label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::User => "You",
            Self::Assistant => "Assistant",
        }
    }

    /// Bubble class, matching the existing transcript vocabulary.
    pub const fn class_name(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

/// One entry in the transcript.
///
/// `model` is recorded per assistant turn rather than read from the composer at
/// render time, so switching models does not retroactively relabel earlier
/// replies.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Turn {
    pub role: Role,
    pub text: String,
    /// The model that produced this turn, for assistant entries.
    pub model: Option<String>,
    /// `true` when this entry is an error report rather than a reply.
    pub is_error: bool,
}

impl Turn {
    /// A message the user sent.
    pub fn user(text: String) -> Self {
        Self {
            role: Role::User,
            text,
            model: None,
            is_error: false,
        }
    }

    /// A reply the router returned.
    pub fn assistant(text: String, model: String) -> Self {
        Self {
            role: Role::Assistant,
            text,
            model: Some(model),
            is_error: false,
        }
    }

    /// A failure, rendered in the transcript where the reply would have been.
    ///
    /// In the transcript rather than a banner because a failed turn belongs in
    /// sequence: it says which message did not get an answer.
    pub fn failure(text: String) -> Self {
        Self {
            role: Role::Assistant,
            text,
            model: None,
            is_error: true,
        }
    }

    /// Stable key for the transcript list.
    pub fn key(&self, index: usize) -> String {
        format!("{index}-{}", self.role.as_wire())
    }
}

/// One `messages[]` element of the request.
#[derive(Debug, Serialize)]
struct WireMessage<'turn> {
    role: &'static str,
    content: &'turn str,
}

/// The `POST /api/dashboard/chat/completions` body.
///
/// `stream` is `false`: the endpoint relays a streaming body unchanged, but this
/// panel renders a complete reply, and claiming to stream while buffering would
/// be a worse lie than not streaming. Built through `serde` so a message
/// containing a quote or backslash cannot break out of the payload.
#[derive(Debug, Serialize)]
struct ChatRequest<'turn> {
    model: &'turn str,
    messages: Vec<WireMessage<'turn>>,
    stream: bool,
}

/// Why a draft cannot be sent.
///
/// Mirrors the endpoint's own rejections (`model` and `messages` are both
/// required) so the composer explains the problem before spending a request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DraftError {
    Empty,
    NoModel,
}

impl DraftError {
    pub const fn message(self) -> &'static str {
        match self {
            Self::Empty => "Type a message first.",
            Self::NoModel => "Choose a model. None of the connected providers offer one yet.",
        }
    }
}

/// Build the request body for a new turn.
///
/// The whole transcript is sent, minus failure entries: an error report is this
/// dashboard's own text, and replaying it to a provider as if the assistant had
/// said it would corrupt the conversation.
pub fn request_body(history: &[Turn], draft: &str, model: &str) -> Result<String, DraftError> {
    let draft = draft.trim();
    if draft.is_empty() {
        return Err(DraftError::Empty);
    }
    let model = model.trim();
    if model.is_empty() {
        return Err(DraftError::NoModel);
    }

    let mut messages: Vec<WireMessage<'_>> = history
        .iter()
        .filter(|turn| !turn.is_error)
        .map(|turn| WireMessage {
            role: turn.role.as_wire(),
            content: turn.text.as_str(),
        })
        .collect();
    messages.push(WireMessage {
        role: Role::User.as_wire(),
        content: draft,
    });

    serde_json::to_string(&ChatRequest {
        model,
        messages,
        stream: false,
    })
    .map_err(|_error| DraftError::Empty)
}

/// The assistant text of a chat completion.
///
/// `None` when the body carries no assistant content at all: an empty first
/// choice, a missing `choices` array, or a body that is not JSON. The panel shows
/// that as a failure, because a blank bubble would read as a model that chose to
/// say nothing.
pub fn parse_reply(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    let choice = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())?;

    let message = choice.get("message");
    let content = message
        .and_then(|message| message.get("content"))
        // Non-streaming replies carry a string; a content-parts array is read
        // too, since the translators can produce one for provider-native shapes.
        .and_then(|content| {
            content.as_str().map(ToOwned::to_owned).or_else(|| {
                content.as_array().map(|parts| {
                    parts
                        .iter()
                        .filter_map(|part| part.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("")
                })
            })
        })
        .or_else(|| {
            // `delta` is the streaming shape; read it so a relayed frame is not
            // reported as an empty reply.
            choice
                .get("delta")
                .and_then(|delta| delta.get("content"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })?;

    let trimmed = content.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// The message from an OpenAI-shaped `{"error":{...}}` envelope.
///
/// Read from a failed response body so the provider's own words reach the user.
/// Also handles the flat `{"error":"…"}` form some of this port's own handlers
/// return.
pub fn parse_error(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    let error = value.get("error")?;

    let message = error
        .as_str()
        .map(ToOwned::to_owned)
        .or_else(|| {
            error
                .get("message")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
        .or_else(|| {
            value
                .get("message")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })?;

    let trimmed = message.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// How one send ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SendOutcome {
    /// The router returned assistant text.
    Replied(String),
    /// The request completed but carried no usable reply, or was refused.
    ///
    /// Carries text already fit to render, so the panel never has to decide
    /// between a provider's message and a generic one.
    Failed(String),
}

impl SendOutcome {
    /// Turn this outcome into the transcript entry it represents.
    pub fn into_turn(self, model: String) -> Turn {
        match self {
            Self::Replied(text) => Turn::assistant(text, model),
            Self::Failed(text) => Turn::failure(text),
        }
    }
}

/// Interpret a chat response.
///
/// `response` is `Ok` only for a 2xx. A `501` is reported with the body's own
/// message when it has one: this port answers `501` both when the runtime is
/// unreachable and when a provider is deliberately not implemented, and those
/// read very differently to a user.
pub fn settle_send(response: Result<&str, ApiError>) -> SendOutcome {
    match response {
        Ok(body) => match parse_reply(body) {
            Some(text) => SendOutcome::Replied(text),
            // A 2xx that carries an error envelope: the gateway relays the
            // provider's status, so this happens for a refusal wrapped in a 200.
            None => SendOutcome::Failed(parse_error(body).unwrap_or_else(|| {
                String::from("The router answered without a reply. Nothing was generated.")
            })),
        },
        Err(error) => SendOutcome::Failed(error.message().to_owned()),
    }
}

/// One model the composer can send to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelOption {
    /// The `model` value to send, always `provider/model`.
    pub request_model: String,
    /// Model id as the registry names it.
    pub model_id: String,
    pub provider_id: String,
    pub provider_name: String,
    /// The connection that makes this model reachable.
    pub connection_name: String,
    /// `true` when this is the connection's own configured default model.
    pub is_connection_default: bool,
}

impl ModelOption {
    /// Secondary line for the menu entry.
    pub fn detail(&self) -> String {
        if self.is_connection_default {
            format!("{} · connection default", self.connection_name)
        } else {
            self.connection_name.clone()
        }
    }
}

/// Models grouped by the provider that offers them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderModels {
    pub provider_id: String,
    pub provider_name: String,
    pub models: Vec<ModelOption>,
}

/// Derive the model menu from the configured connections.
///
/// Only active connections contribute: an inactive one is not in the routing
/// pool, so offering its models would promise a request the router would not
/// make. Each connection's provider is looked up in the embedded registry for
/// its chat models; the connection's own `defaultModel` is included even when the
/// registry does not list it, because the router will route it.
///
/// An empty result is meaningful and is rendered as the provider boundary: it
/// means no connection can currently serve a chat turn.
pub fn model_options(connections: &ConnectionList) -> Vec<ProviderModels> {
    let mut groups: Vec<ProviderModels> = Vec::new();

    for connection in connections
        .connections()
        .iter()
        .filter(|connection| connection.is_active)
    {
        let provider_name = connection.provider_label();
        let mut options: Vec<ModelOption> = registry_chat_models(&connection.provider)
            .into_iter()
            .map(|model_id| ModelOption {
                request_model: format!("{}/{model_id}", connection.provider),
                model_id,
                provider_id: connection.provider.clone(),
                provider_name: provider_name.clone(),
                connection_name: connection.name.clone(),
                is_connection_default: false,
            })
            .collect();

        if let Some(default_model) = connection
            .default_model
            .as_deref()
            .map(str::trim)
            .filter(|model| !model.is_empty())
        {
            match options
                .iter_mut()
                .find(|option| option.model_id == default_model)
            {
                Some(existing) => existing.is_connection_default = true,
                None => options.push(ModelOption {
                    request_model: format!("{}/{default_model}", connection.provider),
                    model_id: default_model.to_owned(),
                    provider_id: connection.provider.clone(),
                    provider_name: provider_name.clone(),
                    connection_name: connection.name.clone(),
                    is_connection_default: true,
                }),
            }
        }

        match groups
            .iter_mut()
            .find(|group| group.provider_id == connection.provider)
        {
            Some(group) => group.models.extend(options),
            None => groups.push(ProviderModels {
                provider_id: connection.provider.clone(),
                provider_name,
                models: options,
            }),
        }
    }

    for group in &mut groups {
        // The connection default first, then by id, so the entry a user is most
        // likely to want is not buried in an alphabetical list.
        group.models.sort_by(|left, right| {
            right
                .is_connection_default
                .cmp(&left.is_connection_default)
                .then_with(|| left.model_id.cmp(&right.model_id))
        });
        group
            .models
            .dedup_by(|left, right| left.request_model == right.request_model);
    }
    groups.sort_by(|left, right| {
        left.provider_name
            .to_ascii_lowercase()
            .cmp(&right.provider_name.to_ascii_lowercase())
            .then_with(|| left.provider_id.cmp(&right.provider_id))
    });
    groups
}

/// The chat-capable model ids the registry lists for a provider.
///
/// Non-chat kinds (image, tts, stt, embedding, search, fetch) are excluded: this
/// composer sends chat completions, and a model that cannot answer one must not
/// be selectable here.
fn registry_chat_models(provider_id: &str) -> Vec<String> {
    nullrouter_providers::entry(provider_id)
        .map(|entry| {
            entry
                .models
                .iter()
                .filter(|model| is_chat_kind(model.kind.as_deref()))
                .map(|model| model.id.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// Whether a registry `kind` is a chat model.
///
/// An absent kind is chat: the registry omits it for the common case, matching
/// upstream's own default.
fn is_chat_kind(kind: Option<&str>) -> bool {
    matches!(kind, None | Some("llm" | "chat" | "imageToText"))
}

/// The model to pre-select, if any.
///
/// A connection default is preferred over the first listed model, so the
/// composer opens on what the user configured.
pub fn default_model(groups: &[ProviderModels]) -> Option<String> {
    groups
        .iter()
        .flat_map(|group| group.models.iter())
        .find(|option| option.is_connection_default)
        .or_else(|| groups.first().and_then(|group| group.models.first()))
        .map(|option| option.request_model.clone())
}

/// How the composer describes the selected model, or the absence of one.
pub fn active_model_label(selected: Option<&str>) -> String {
    selected.map_or_else(|| String::from("No model"), ToOwned::to_owned)
}

/// Where a selected model comes from, for the composer's secondary line.
pub fn active_model_detail(groups: &[ProviderModels], selected: Option<&str>) -> String {
    let Some(selected) = selected else {
        return String::from("Connect a provider to choose a model");
    };
    groups
        .iter()
        .flat_map(|group| group.models.iter())
        .find(|option| option.request_model == selected)
        .map_or_else(
            || String::from("Not offered by any connected provider"),
            ModelOption::detail,
        )
}

/// Display name for a provider id, via the registry.
pub fn provider_display_name(provider_id: &str) -> String {
    provider_label(provider_id)
}

// ── requests ────────────────────────────────────────────────────────────────

/// `GET /api/providers`, for the model menu.
pub async fn load_connections() -> Result<ConnectionList, ApiError> {
    crate::dashboard::providers_live::load_connections().await
}

/// `POST /api/dashboard/chat/completions`.
pub async fn send_turn(body: String) -> SendOutcome {
    let response = crate::api::post(CHAT_PATH, &body).await;
    settle_send(response.as_deref().map_err(|error| *error))
}

/// The `messages` a request body would carry, for tests and for debugging.
///
/// Exposed because "the whole transcript is sent, minus failures" is a behaviour
/// worth asserting directly rather than through a JSON string comparison.
#[derive(Debug, Deserialize)]
pub struct SentMessage {
    pub role: String,
    pub content: String,
}

/// Read back the `messages` array of a body produced by [`request_body`].
pub fn sent_messages(body: &str) -> Option<Vec<SentMessage>> {
    let value: Value = serde_json::from_str(body).ok()?;
    serde_json::from_value(value.get("messages")?.clone()).ok()
}
