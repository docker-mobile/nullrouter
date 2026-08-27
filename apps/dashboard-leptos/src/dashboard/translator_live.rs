//! Live translator state: load a step's file, translate it, save it, send it.
//!
//! The panel this backs rendered `translator_dashboard_state()` — seven steps
//! whose "preview" bodies were compile-time strings, with every button disabled
//! and a canned `{"status":"preview"}` standing in for a request nobody had
//! made. A user could read that page and believe they were looking at a
//! translated payload.
//!
//! Nothing here produces content. Every buffer is either something the user
//! typed, something `GET /api/translator/load` returned, or something
//! `POST /api/translator/translate` computed — and each of those is a distinct
//! [`StepSource`], shown on the step, so the origin of a buffer is never
//! ambiguous.
//!
//! Free of `leptos` and of `fetch`, so every branch is unit-testable natively.

use crate::api::ApiError;
use serde::Serialize;
use serde_json::Value;

/// `GET /api/translator/load`, which takes a `file` query parameter.
pub const LOAD_PATH: &str = "/api/translator/load";

/// `POST /api/translator/save`.
pub const SAVE_PATH: &str = "/api/translator/save";

/// `POST /api/translator/translate`.
pub const TRANSLATE_PATH: &str = "/api/translator/translate";

/// `POST /api/translator/send`.
pub const SEND_PATH: &str = "/api/translator/send";

/// Shown where the router reported no value.
pub const NO_READING: &str = "—";

/// One stage of the replay pipeline.
///
/// The variants are exactly the eight names `ALLOWED_FILES` accepts in
/// `services/api-actix/src/translator.rs`; a control here cannot name a file the
/// endpoint would reject with `400`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TranslatorFile {
    ClientRequest,
    SourceBody,
    OpenAiIntermediate,
    TargetRequest,
    ProviderResponse,
    OpenAiResponse,
    ClientResponse,
}

impl TranslatorFile {
    /// Every stage, in pipeline order.
    pub const ALL: [Self; 7] = [
        Self::ClientRequest,
        Self::SourceBody,
        Self::OpenAiIntermediate,
        Self::TargetRequest,
        Self::ProviderResponse,
        Self::OpenAiResponse,
        Self::ClientResponse,
    ];

    /// 1-based position in the pipeline.
    pub const fn index(self) -> u8 {
        match self {
            Self::ClientRequest => 1,
            Self::SourceBody => 2,
            Self::OpenAiIntermediate => 3,
            Self::TargetRequest => 4,
            Self::ProviderResponse => 5,
            Self::OpenAiResponse => 6,
            Self::ClientResponse => 7,
        }
    }

    /// The `file` query/body value.
    pub const fn file_name(self) -> &'static str {
        match self {
            Self::ClientRequest => "1_req_client.json",
            Self::SourceBody => "2_req_source.json",
            Self::OpenAiIntermediate => "3_req_openai.json",
            Self::TargetRequest => "4_req_target.json",
            Self::ProviderResponse => "5_res_provider.txt",
            Self::OpenAiResponse => "6_res_openai.txt",
            Self::ClientResponse => "7_res_client.txt",
        }
    }

    /// The alternate name the API also accepts for this stage, if any.
    pub const fn alternate_file(self) -> Option<&'static str> {
        match self {
            Self::ClientResponse => Some("7_res_client.json"),
            _ => None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::ClientRequest => "Client Request",
            Self::SourceBody => "Source Body",
            Self::OpenAiIntermediate => "OpenAI Intermediate",
            Self::TargetRequest => "Target Request",
            Self::ProviderResponse => "Provider Response",
            Self::OpenAiResponse => "OpenAI Response",
            Self::ClientResponse => "Client Response",
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::ClientRequest => "Raw request from client",
            Self::SourceBody => "After initial conversion",
            Self::OpenAiIntermediate => "source → openai",
            Self::TargetRequest => "openai → target + URL + headers",
            Self::ProviderResponse => "Raw SSE from provider",
            Self::OpenAiResponse => "target → openai (response)",
            Self::ClientResponse => "Final response to client",
        }
    }

    /// `json` or `text`, matching the file's extension.
    pub const fn language(self) -> &'static str {
        match self {
            Self::ClientRequest
            | Self::SourceBody
            | Self::OpenAiIntermediate
            | Self::TargetRequest => "json",
            Self::ProviderResponse | Self::OpenAiResponse | Self::ClientResponse => "text",
        }
    }

    /// Whether this buffer is expected to hold JSON, so Format applies.
    pub const fn is_json(self) -> bool {
        matches!(
            self,
            Self::ClientRequest | Self::SourceBody | Self::OpenAiIntermediate | Self::TargetRequest
        )
    }

    /// The translate step this stage can start, when it can start one.
    ///
    /// `POST /api/translator/translate` accepts steps 1-3 only, so stages 4-7
    /// offer no translate control rather than one that would be answered `400`.
    pub const fn translate_step(self) -> Option<TranslateStep> {
        match self {
            Self::ClientRequest => Some(TranslateStep::Detect),
            Self::SourceBody => Some(TranslateStep::ToOpenAi),
            Self::OpenAiIntermediate => Some(TranslateStep::ToTarget),
            _ => None,
        }
    }

    /// `true` when this stage's buffer is what `POST /api/translator/send` takes.
    pub const fn is_sendable(self) -> bool {
        matches!(self, Self::TargetRequest)
    }

    /// The `?file=` path for this stage.
    pub const fn load_path(self) -> &'static str {
        match self {
            Self::ClientRequest => "/api/translator/load?file=1_req_client.json",
            Self::SourceBody => "/api/translator/load?file=2_req_source.json",
            Self::OpenAiIntermediate => "/api/translator/load?file=3_req_openai.json",
            Self::TargetRequest => "/api/translator/load?file=4_req_target.json",
            Self::ProviderResponse => "/api/translator/load?file=5_res_provider.txt",
            Self::OpenAiResponse => "/api/translator/load?file=6_res_openai.txt",
            Self::ClientResponse => "/api/translator/load?file=7_res_client.txt",
        }
    }

    /// DOM id for this stage's editor, so its label can point at it.
    pub fn editor_id(self) -> String {
        format!("nr-translator-editor-{}", self.index())
    }

    /// DOM id for this stage's status line.
    pub fn status_id(self) -> String {
        format!("nr-translator-status-{}", self.index())
    }
}

/// Which half of the pipeline a translate call runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TranslateStep {
    /// Step 1: detect provider, model, and the source/target formats.
    Detect,
    /// Step 2: source → OpenAI intermediate.
    ToOpenAi,
    /// Step 3: OpenAI intermediate → target, with URL and headers.
    ToTarget,
}

impl TranslateStep {
    pub const fn number(self) -> u8 {
        match self {
            Self::Detect => 1,
            Self::ToOpenAi => 2,
            Self::ToTarget => 3,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Detect => "Detect",
            Self::ToOpenAi => "→ OpenAI",
            Self::ToTarget => "→ Target",
        }
    }

    /// The stage that receives this step's output, when one does.
    pub const fn writes_into(self) -> Option<TranslatorFile> {
        match self {
            Self::Detect => None,
            Self::ToOpenAi => Some(TranslatorFile::OpenAiIntermediate),
            Self::ToTarget => Some(TranslatorFile::TargetRequest),
        }
    }

    /// What a user is told this control will do.
    pub const fn detail(self) -> &'static str {
        match self {
            Self::Detect => "Ask the router which provider, model, and formats this body maps to.",
            Self::ToOpenAi => "Translate this body into the OpenAI intermediate form.",
            Self::ToTarget => "Translate the intermediate body into the target provider's form.",
        }
    }
}

/// Where a step's buffer came from.
///
/// Rendered on the step, because "you typed this" and "the router computed this"
/// must never look the same. There is deliberately no `Preview` variant.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StepSource {
    /// Nothing has been loaded or typed.
    #[default]
    Empty,
    /// `GET /api/translator/load` returned this content.
    Loaded,
    /// The user typed or pasted it.
    Edited,
    /// `POST /api/translator/translate` produced it.
    Translated,
    /// `POST /api/translator/send` returned it.
    Received,
}

impl StepSource {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Empty => "Empty",
            Self::Loaded => "Loaded from disk",
            Self::Edited => "Edited here",
            Self::Translated => "Translated by the router",
            Self::Received => "Returned by the provider",
        }
    }

    pub const fn class_name(self) -> &'static str {
        match self {
            Self::Empty => "is-idle",
            Self::Loaded | Self::Translated | Self::Received => "is-connected",
            Self::Edited => "is-degraded",
        }
    }
}

/// How `GET /api/translator/load` ended.
///
/// The endpoint answers `200 {"success":false,"error":"File not found"}` for an
/// absent file (and upstream answers `404`), so "there is no such log yet" is a
/// first-class result and not an error the user should act on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LoadOutcome {
    Loaded(String),
    /// The file does not exist yet.
    Missing(String),
    /// The request or the body failed.
    Rejected(ApiError),
}

/// Parse a load response.
///
/// `Rejected(Body)` when the body is not a JSON object: a body this build cannot
/// read must not be shown as file content.
fn read_load(body: &str) -> LoadOutcome {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return LoadOutcome::Rejected(ApiError::Body);
    };
    if !value.is_object() {
        return LoadOutcome::Rejected(ApiError::Body);
    }
    if let Some(content) = value.get("content").and_then(Value::as_str) {
        // Present and a string, including the empty string: an empty log file is
        // a real thing to have loaded.
        return LoadOutcome::Loaded(content.to_owned());
    }
    LoadOutcome::Missing(
        text(&value, "error")
            .unwrap_or_else(|| String::from("The router returned no content for this file.")),
    )
}

/// A string field, when present and non-empty after trimming.
fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|found| !found.is_empty())
        .map(ToOwned::to_owned)
}

/// A boolean field, or `None` when absent.
fn flag(value: &Value, key: &str) -> Option<bool> {
    value.get(key).and_then(Value::as_bool)
}

/// How `POST /api/translator/save` ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SaveOutcome {
    /// The router wrote the file.
    Written,
    /// The router answered `unsupported`: nothing was written.
    Unsupported(String),
    /// The router refused the write and said why.
    Refused(String),
    Rejected(ApiError),
}

impl SaveOutcome {
    pub fn message(&self) -> String {
        match self {
            Self::Written => String::from("Saved to logs/translator."),
            // Both carry the router's own explanation, so both read the same
            // way. They stay separate variants because only one of them means
            // "this build never writes files".
            Self::Unsupported(detail) | Self::Refused(detail) => format!("Not saved. {detail}"),
            Self::Rejected(error) => format!("Not saved. {}", error.message()),
        }
    }

    /// `true` only when a file was actually written.
    pub const fn wrote_file(&self) -> bool {
        matches!(self, Self::Written)
    }
}

/// Interpret a save response.
pub fn settle_save(response: Result<&str, ApiError>) -> SaveOutcome {
    let body = match response {
        Ok(body) => body,
        Err(ApiError::Status(501)) => {
            return SaveOutcome::Unsupported(String::from(
                "This build does not write translator log files.",
            ));
        }
        Err(error) => return SaveOutcome::Rejected(error),
    };
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return SaveOutcome::Rejected(ApiError::Body);
    };
    let detail = text(&value, "error").or_else(|| text(&value, "message"));
    if flag(&value, "unsupported") == Some(true) {
        return SaveOutcome::Unsupported(
            detail.unwrap_or_else(|| String::from("The router reported saving as unsupported.")),
        );
    }
    if flag(&value, "success") == Some(true) {
        return SaveOutcome::Written;
    }
    SaveOutcome::Refused(
        detail.unwrap_or_else(|| String::from("It did not say why the file was not written.")),
    )
}

/// The `{provider, model, sourceFormat, targetFormat}` a translate call reported.
///
/// Every field is optional: the endpoint returns different subsets per step, and
/// a format it could not determine comes back as the literal `"unknown"`, which
/// is normalised away here so the badge reads "—" instead of asserting a format
/// named "unknown".
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TranslationMeta {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub source_format: Option<String>,
    pub target_format: Option<String>,
    /// Target URL, which step 3 builds through the provider executor.
    pub url: Option<String>,
}

impl TranslationMeta {
    /// Overlay everything `other` knows, keeping what it does not.
    fn merge(&mut self, other: Self) {
        if other.provider.is_some() {
            self.provider = other.provider;
        }
        if other.model.is_some() {
            self.model = other.model;
        }
        if other.source_format.is_some() {
            self.source_format = other.source_format;
        }
        if other.target_format.is_some() {
            self.target_format = other.target_format;
        }
        if other.url.is_some() {
            self.url = other.url;
        }
    }

    /// A field's value, or the no-reading marker.
    fn reading(value: Option<&String>) -> String {
        value.map_or_else(|| NO_READING.to_owned(), Clone::clone)
    }

    /// The four badges the panel shows, as `(label, value)`.
    pub fn badges(&self) -> [(&'static str, String); 4] {
        [
            ("src", Self::reading(self.source_format.as_ref())),
            ("dst", Self::reading(self.target_format.as_ref())),
            ("provider", Self::reading(self.provider.as_ref())),
            ("model", Self::reading(self.model.as_ref())),
        ]
    }
}

/// `"unknown"` and `""` are non-answers, so they are dropped rather than shown.
fn meaningful(value: Option<String>) -> Option<String> {
    value.filter(|found| !found.eq_ignore_ascii_case("unknown"))
}

/// One translate call's result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Translation {
    pub meta: TranslationMeta,
    /// The translated body, pretty-printed. `None` when the router returned a
    /// result with no body — which step 1 always does, and which a stubbed build
    /// does for every step.
    pub body: Option<String>,
    /// Request headers step 3 built, pretty-printed, when it returned any.
    pub headers: Option<String>,
}

impl Translation {
    /// What to tell the user when the router returned no body.
    ///
    /// This is the case a fixture would have papered over: the call succeeded and
    /// produced nothing, which is not the same as a translated payload.
    pub const fn empty_note() -> &'static str {
        "The router answered without a translated body, so nothing was written into the next step."
    }
}

/// How a translate call ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TranslateOutcome {
    Translated(Box<Translation>),
    /// The router refused the call and said why.
    Refused(String),
    Rejected(ApiError),
}

/// Pretty-print a JSON value, or `None` when it carries nothing.
///
/// An empty object or array is treated as nothing: `{}` rendered in an editor
/// looks like a translated body, and it is not one.
fn render_body(value: Option<&Value>) -> Option<String> {
    match value {
        None | Some(Value::Null) => None,
        Some(Value::Object(map)) if map.is_empty() => None,
        Some(Value::Array(entries)) if entries.is_empty() => None,
        Some(Value::String(text)) => Some(text.trim())
            .filter(|text| !text.is_empty())
            .map(ToOwned::to_owned),
        Some(other) => serde_json::to_string_pretty(other).ok(),
    }
}

/// Read the metadata a translate result carries.
fn translation_meta(result: &Value) -> TranslationMeta {
    TranslationMeta {
        provider: meaningful(text(result, "provider")),
        model: meaningful(text(result, "model")),
        source_format: meaningful(text(result, "sourceFormat")),
        target_format: meaningful(text(result, "targetFormat")),
        url: text(result, "url"),
    }
}

/// Interpret a `POST /api/translator/translate` response.
pub fn settle_translate(response: Result<&str, ApiError>) -> TranslateOutcome {
    let body = match response {
        Ok(body) => body,
        Err(error) => return TranslateOutcome::Rejected(error),
    };
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return TranslateOutcome::Rejected(ApiError::Body);
    };
    let detail = text(&value, "error").or_else(|| text(&value, "message"));
    if flag(&value, "success") != Some(true) {
        return TranslateOutcome::Refused(
            detail.unwrap_or_else(|| String::from("The router did not say why.")),
        );
    }
    let Some(result) = value.get("result") else {
        return TranslateOutcome::Rejected(ApiError::Body);
    };

    TranslateOutcome::Translated(Box::new(Translation {
        meta: translation_meta(result),
        body: render_body(result.get("body")),
        headers: render_body(result.get("headers")),
    }))
}

/// How `POST /api/translator/send` ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SendOutcome {
    /// The provider answered. The body is its response verbatim — an SSE stream
    /// when streaming was requested — and is shown as received, unparsed.
    Answered(String),
    /// The router refused to send, or the provider rejected the request.
    Refused(String),
    /// This build does not run provider requests.
    Unsupported(String),
    Rejected(ApiError),
}

impl SendOutcome {
    pub fn message(&self) -> String {
        match self {
            Self::Answered(_) => String::from("The provider answered; its response is below."),
            // Same sentence, different reasons: `Unsupported` means this build
            // never sends, `Refused` means this request was rejected.
            Self::Refused(detail) | Self::Unsupported(detail) => {
                format!("Nothing was sent. {detail}")
            }
            Self::Rejected(error) => format!("Nothing was sent. {}", error.message()),
        }
    }
}

/// Interpret a send response.
///
/// A successful send returns `text/event-stream`, not JSON, so a body that does
/// not parse as a JSON envelope is the provider's stream and is kept verbatim. A
/// JSON body carrying `success: false` is a refusal.
pub fn settle_send(response: Result<&str, ApiError>) -> SendOutcome {
    let body = match response {
        Ok(body) => body,
        Err(ApiError::Status(501)) => {
            return SendOutcome::Unsupported(String::from(
                "This build does not run provider requests.",
            ));
        }
        Err(error) => return SendOutcome::Rejected(error),
    };

    let refusal = serde_json::from_str::<Value>(body)
        .ok()
        .filter(|value| value.is_object() && flag(value, "success") == Some(false));
    if let Some(value) = refusal {
        let detail = text(&value, "error")
            .or_else(|| text(&value, "message"))
            .unwrap_or_else(|| String::from("The router did not say why."));
        return if flag(&value, "unsupported") == Some(true) {
            SendOutcome::Unsupported(detail)
        } else {
            SendOutcome::Refused(detail)
        };
    }
    if body.trim().is_empty() {
        return SendOutcome::Refused(String::from("The provider returned an empty response."));
    }
    SendOutcome::Answered(body.to_owned())
}

/// Why a translate or send call cannot be made from the current buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestError {
    /// The buffer is empty, so there is nothing to translate or send.
    BodyEmpty,
    /// The buffer is not valid JSON, and these endpoints take a JSON body.
    BodyInvalid,
    /// Step 3 and send both need a provider, which only step 1 can report.
    ProviderMissing,
    /// Likewise for the model.
    ModelMissing,
    /// The request could not be encoded.
    Encoding,
}

impl RequestError {
    pub const fn message(self) -> &'static str {
        match self {
            Self::BodyEmpty => "This step is empty. Load a file or paste a body first.",
            Self::BodyInvalid => "This step is not valid JSON, so the router cannot translate it.",
            Self::ProviderMissing => {
                "No provider is known yet. Run Detect on the client request first."
            }
            Self::ModelMissing => "No model is known yet. Run Detect on the client request first.",
            Self::Encoding => "This body could not be encoded as a request.",
        }
    }
}

/// The `POST /api/translator/save` body.
#[derive(Debug, Serialize)]
struct SaveRequest<'a> {
    file: &'a str,
    content: &'a str,
}

/// Build a save body for one stage.
pub fn save_body(file: TranslatorFile, content: &str) -> Result<String, RequestError> {
    serde_json::to_string(&SaveRequest {
        file: file.file_name(),
        content,
    })
    .map_err(|_error| RequestError::Encoding)
}

/// The `POST /api/translator/translate` body.
#[derive(Debug, Serialize)]
struct TranslateRequest<'a> {
    step: u8,
    body: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<&'a str>,
}

/// Build a translate body.
///
/// The buffer is parsed here rather than interpolated, so an unbalanced brace in
/// the editor is reported as [`RequestError::BodyInvalid`] instead of producing a
/// malformed request. Steps 3 requires a provider and model, mirroring the
/// endpoint's own check.
pub fn translate_body(
    step: TranslateStep,
    buffer: &str,
    meta: &TranslationMeta,
) -> Result<String, RequestError> {
    let trimmed = buffer.trim();
    if trimmed.is_empty() {
        return Err(RequestError::BodyEmpty);
    }
    let body: Value = serde_json::from_str(trimmed).map_err(|_error| RequestError::BodyInvalid)?;

    let (provider, model) = if step == TranslateStep::ToTarget {
        let provider = meta
            .provider
            .as_deref()
            .ok_or(RequestError::ProviderMissing)?;
        let model = meta.model.as_deref().ok_or(RequestError::ModelMissing)?;
        (Some(provider), Some(model))
    } else {
        (meta.provider.as_deref(), meta.model.as_deref())
    };

    serde_json::to_string(&TranslateRequest {
        step: step.number(),
        body,
        provider,
        model,
    })
    .map_err(|_error| RequestError::Encoding)
}

/// The `POST /api/translator/send` body.
#[derive(Debug, Serialize)]
struct SendRequest<'a> {
    provider: &'a str,
    model: &'a str,
    body: Value,
}

/// Build a send body from the target-request buffer.
pub fn send_body(buffer: &str, meta: &TranslationMeta) -> Result<String, RequestError> {
    let trimmed = buffer.trim();
    if trimmed.is_empty() {
        return Err(RequestError::BodyEmpty);
    }
    let body: Value = serde_json::from_str(trimmed).map_err(|_error| RequestError::BodyInvalid)?;
    let provider = meta
        .provider
        .as_deref()
        .ok_or(RequestError::ProviderMissing)?;
    let model = meta.model.as_deref().ok_or(RequestError::ModelMissing)?;

    serde_json::to_string(&SendRequest {
        provider,
        model,
        body,
    })
    .map_err(|_error| RequestError::Encoding)
}

/// Pretty-print a JSON buffer in place.
///
/// `None` when the buffer is not JSON, so Format reports the parse failure
/// instead of silently replacing what the user typed.
pub fn format_json(buffer: &str) -> Option<String> {
    let value: Value = serde_json::from_str(buffer.trim()).ok()?;
    serde_json::to_string_pretty(&value).ok()
}

/// Overlay new metadata onto what is already known.
pub fn merge_meta(current: &TranslationMeta, next: TranslationMeta) -> TranslationMeta {
    let mut merged = current.clone();
    merged.merge(next);
    merged
}

// ── requests ────────────────────────────────────────────────────────────────

/// `GET /api/translator/load?file=…`.
pub async fn load_file(file: TranslatorFile) -> LoadOutcome {
    match crate::api::get(file.load_path()).await {
        Ok(body) => read_load(&body),
        Err(error) => LoadOutcome::Rejected(error),
    }
}

/// `POST /api/translator/save`.
pub async fn save_file(body: String) -> SaveOutcome {
    let response = crate::api::post(SAVE_PATH, &body).await;
    settle_save(response.as_deref().map_err(|error| *error))
}

/// `POST /api/translator/translate`.
pub async fn translate(body: String) -> TranslateOutcome {
    let response = crate::api::post(TRANSLATE_PATH, &body).await;
    settle_translate(response.as_deref().map_err(|error| *error))
}

/// `POST /api/translator/send`.
pub async fn send(body: String) -> SendOutcome {
    let response = crate::api::post(SEND_PATH, &body).await;
    settle_send(response.as_deref().map_err(|error| *error))
}

#[cfg(test)]
mod tests {
    use super::{
        RequestError, TranslateStep, TranslationMeta, TranslatorFile, format_json, translate_body,
    };

    #[test]
    fn every_stage_loads_its_own_file() {
        for file in TranslatorFile::ALL {
            assert!(
                file.load_path().ends_with(file.file_name()),
                "{} does not load {}",
                file.load_path(),
                file.file_name()
            );
        }
    }

    #[test]
    fn a_non_json_buffer_is_refused_before_a_request_is_spent() {
        let meta = TranslationMeta::default();
        assert_eq!(
            translate_body(TranslateStep::ToOpenAi, "{not json", &meta),
            Err(RequestError::BodyInvalid)
        );
        assert_eq!(
            translate_body(TranslateStep::ToOpenAi, "   ", &meta),
            Err(RequestError::BodyEmpty)
        );
    }

    #[test]
    fn formatting_reports_failure_rather_than_replacing_the_buffer() {
        assert!(format_json("{oops").is_none());
        assert_eq!(
            format_json(r#"{"a":1}"#).as_deref(),
            Some("{\n  \"a\": 1\n}")
        );
    }
}
