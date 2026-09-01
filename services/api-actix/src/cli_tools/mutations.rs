//! Applying and revoking a tool's config: what to write, where, and how to undo it.
//!
//! Kept apart from [`super::spec`] on purpose. That module answers "where does this tool keep its
//! config and how do I tell whether it points here" and is read by `GET`. This one answers "what
//! does an apply change", which is only ever reached by a write. The split also keeps the write
//! descriptors next to the primitives in [`super::write`] that carry them out.
//!
//! # Why a list of targets rather than one file
//!
//! Six of the thirteen writable tools write two files: codex splits config from credentials
//! (`config.toml` + `auth.json`), cline splits state from secrets, kilo writes its own auth file
//! and then VS Code's settings. A descriptor holding a single path could not express any of them,
//! and the credential half is the half that matters — an apply that wrote the base URL but not the
//! key leaves the tool pointing here with nothing to authenticate with.
//!
//! # Why apply and revoke are function pointers
//!
//! The mutations do not share a shape. codex clears its root keys only when they still name this
//! router; cline puts its provider fields *back* to `cline` rather than deleting them; droid
//! rewrites an array by id prefix; hermes edits YAML text by block. Transcribing each as a small
//! function beside a comment naming its upstream source keeps every divergence visible, where an
//! enum of merge operations would hide them behind a shared abstraction that fits none of them.

use serde::Deserialize;
use serde_json::{Map, Value};

use super::spec::{ConfigFile, Format, Root};
use super::write;

/// The default key upstream writes when a request omits one.
///
/// Only some tools do this — copilot, opencode, droid and grok-build — and it is a placeholder,
/// not a credential: those tools reject an empty key outright, so upstream gives them something
/// syntactically valid to hold.
pub(crate) const PLACEHOLDER_KEY: &str = "sk_9router";

/// The provider name this router registers itself under in every config it writes.
///
/// Load-bearing: it is what [`super::spec`]'s markers grep for, and what a config written by
/// upstream already contains. Renaming it would make this port stop recognising configs it wrote.
const PROVIDER: &str = "9router";

/// The display name, which is capitalised differently from [`PROVIDER`] and is also matched
/// exactly — copilot searches its array for `name === "9Router"`.
const DISPLAY: &str = "9Router";

/// One request body, covering every field any tool reads.
///
/// A union rather than thirteen structs. The dashboard posts one shape per tool and the fields do
/// not collide, so a shared struct with everything optional accepts exactly what upstream accepts.
/// Per-tool required fields are checked by [`Writer::validate`] before any file is touched.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Payload {
    pub(crate) base_url: Option<String>,
    pub(crate) api_key: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) models: Option<Vec<Value>>,
    pub(crate) active_model: Option<String>,
    pub(crate) subagent_model: Option<String>,
    pub(crate) subagent_models: Option<Value>,
    pub(crate) agent_models: Option<Map<String, Value>>,
    pub(crate) context_window: Option<Value>,
    /// Claude Code takes its whole environment block rather than named fields.
    pub(crate) env: Option<Map<String, Value>>,
    pub(crate) max_context_tokens: Option<Value>,
    /// Cowork's MCP half.
    pub(crate) local_plugins: Option<Vec<Value>>,
    pub(crate) custom_plugins: Option<Vec<Value>>,
}

impl Payload {
    /// The base URL with `/v1` appended, as most tools want it.
    fn base_v1(&self) -> String {
        write::with_v1(self.base_url.as_deref().unwrap_or_default())
    }

    /// The base URL with any `/v1` stripped, as cline wants it.
    fn base_bare(&self) -> String {
        write::without_v1(self.base_url.as_deref().unwrap_or_default())
    }

    fn key(&self) -> &str {
        self.api_key.as_deref().unwrap_or_default()
    }

    /// The key, or the placeholder, for the tools that default it.
    fn key_or_placeholder(&self) -> &str {
        match self.api_key.as_deref() {
            Some(key) if !key.trim().is_empty() => key,
            Some(_) | None => PLACEHOLDER_KEY,
        }
    }

    fn model(&self) -> &str {
        self.model.as_deref().unwrap_or_default()
    }

    /// Every model name this payload names, from `models[]` or the single `model`.
    ///
    /// `models[]` entries are either strings or objects with an `id`/`name`, depending on which
    /// dashboard pane posted them, so both are accepted.
    fn model_names(&self) -> Vec<String> {
        if let Some(models) = self.models.as_ref() {
            let names: Vec<String> = models.iter().filter_map(model_name).collect();
            if !names.is_empty() {
                return names;
            }
        }
        let single = self.model();
        if single.is_empty() {
            Vec::new()
        } else {
            vec![single.to_owned()]
        }
    }
}

/// A model name out of a `models[]` entry, whichever shape it came in.
fn model_name(entry: &Value) -> Option<String> {
    match entry {
        Value::String(name) if !name.is_empty() => Some(name.clone()),
        Value::Object(map) => map
            .get("id")
            .or_else(|| map.get("name"))
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .map(str::to_owned),
        _ => None,
    }
}

/// Whether a target's failure fails the whole apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Required {
    /// A write failure is reported and the request fails.
    Yes,
    /// A write failure is collected as a warning and the request still succeeds, because upstream
    /// wraps this write in its own `try {} catch {}`. Kilo's VS Code settings are the case: a user
    /// without VS Code installed should still get their `auth.json` written.
    BestEffort,
}

/// What to do with a file a revoke emptied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OnEmpty {
    /// Leave `{}` on disk.
    Keep,
    /// Unlink it. Codex does this to `auth.json`, and it matters: an empty `auth.json` makes Codex
    /// treat api-key mode as configured-but-blank rather than falling back to a ChatGPT login.
    Delete,
}

/// One file an apply touches.
pub(crate) struct Target {
    pub(crate) config: ConfigFile,
    pub(crate) required: Required,
    pub(crate) on_empty: OnEmpty,
    /// Merge the payload in. Runs against the parsed document, or against a `Value::String` for
    /// the text formats.
    pub(crate) apply: fn(&mut Value, &Payload),
    /// Take it back out, leaving everything else in the file alone.
    pub(crate) revoke: fn(&mut Value),
}

impl std::fmt::Debug for Target {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Target")
            .field("config", &self.config)
            .field("required", &self.required)
            .field("on_empty", &self.on_empty)
            .finish_non_exhaustive()
    }
}

/// A validator that accepts anything, for the tools upstream guards nothing on.
fn no_requirements(_payload: &Payload) -> Result<(), &'static str> {
    Ok(())
}

/// Upstream's most common guard: `if (!baseUrl || !apiKey || !model)`.
fn base_key_and_model(payload: &Payload) -> Result<(), &'static str> {
    let missing = payload.base_url.as_deref().is_none_or(str::is_empty)
        || payload.api_key.as_deref().is_none_or(str::is_empty)
        || payload.model.as_deref().is_none_or(str::is_empty);
    if missing {
        return Err("baseUrl, apiKey and model are required");
    }
    Ok(())
}

/// `if (!baseUrl)` alone, for the tools that default the key and take models as a list.
fn base_url_only(payload: &Payload) -> Result<(), &'static str> {
    if payload.base_url.as_deref().is_none_or(str::is_empty) {
        return Err("baseUrl is required");
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Claude Code — ~/.claude/settings.json
// ---------------------------------------------------------------------------------------------

/// Upstream takes the whole `env` block rather than named fields, so the keys are the caller's.
///
/// `ANTHROPIC_BASE_URL` is the one it normalises, and `hasCompletedOnboarding` is set so Claude
/// Code does not run its first-run wizard over a config that is already complete.
fn claude_apply(document: &mut Value, payload: &Payload) {
    write::set_path(document, &["hasCompletedOnboarding"], Value::Bool(true));
    for (key, value) in payload.env.iter().flatten() {
        let value = if key == "ANTHROPIC_BASE_URL" {
            match value.as_str() {
                Some(url) if !url.is_empty() => Value::String(write::with_v1(url)),
                Some(_) | None => value.clone(),
            }
        } else {
            value.clone()
        };
        write::set_path(document, &["env", key], value);
    }
    // Only set when a concrete value is chosen; anything falsy removes the key so Claude Code
    // falls back to the model's own window.
    match truthy_string(payload.max_context_tokens.as_ref()) {
        Some(tokens) => {
            write::set_path(document, &["env", MAX_CONTEXT_TOKENS], Value::String(tokens));
        }
        None => write::remove_path(document, &["env", MAX_CONTEXT_TOKENS]),
    }
}

/// The env keys a reset removes. Upstream's `RESET_ENV_KEYS`, in its order.
const CLAUDE_RESET_ENV_KEYS: &[&str] = &[
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "API_TIMEOUT_MS",
    MAX_CONTEXT_TOKENS,
];

const MAX_CONTEXT_TOKENS: &str = "CLAUDE_CODE_MAX_CONTEXT_TOKENS";

/// Only the keys upstream lists, and `env` itself once they are gone.
///
/// `hasCompletedOnboarding` is deliberately left set: upstream does not clear it either, and
/// un-setting it would make Claude Code re-run its wizard because the user revoked a base URL.
fn claude_revoke(document: &mut Value) {
    for key in CLAUDE_RESET_ENV_KEYS {
        write::remove_path(document, &["env", key]);
    }
}

/// A JSON value's string form when it is truthy in JS terms.
///
/// `maxContextTokens` arrives as a number from one dashboard pane and a string from another, and
/// upstream's `if (maxContextTokens)` treats `0` and `""` as absent. `String(...)` is applied to
/// whatever survives, which is why a number comes back without quotes.
fn truthy_string(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) if !text.is_empty() => Some(text.clone()),
        Value::Number(number) if number.as_f64().is_some_and(|value| value != 0.0) => {
            Some(number.to_string())
        }
        Value::Bool(true) => Some("true".to_owned()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// Codex — ~/.codex/config.toml + ~/.codex/auth.json
// ---------------------------------------------------------------------------------------------

/// The provider block, plus the subagent model, plus the two root keys that select it.
///
/// `wire_api: "responses"` is upstream's, and it is not a detail: Codex speaks the Responses API
/// to this provider, so writing `chat` here would make every request 404.
fn codex_config_apply(document: &mut Value, payload: &Payload) {
    write::set_path(document, &["model"], Value::String(payload.model().to_owned()));
    write::set_path(document, &["model_provider"], Value::String(PROVIDER.to_owned()));
    write::set_path(
        document,
        &["model_providers", PROVIDER],
        serde_json::json!({
            "name": DISPLAY,
            "base_url": payload.base_v1(),
            "wire_api": "responses",
        }),
    );
    // Upstream falls back to the main model rather than leaving subagents unset.
    let subagent = match payload.subagent_model.as_deref() {
        Some(model) if !model.is_empty() => model,
        Some(_) | None => payload.model(),
    };
    write::set_path(
        document,
        &["agents", "subagent"],
        serde_json::json!({ "model": subagent }),
    );
}

/// Clears the root keys **only when they still name this router**.
///
/// The condition is upstream's and it is the right one: a user who has since pointed Codex at
/// another provider must not have their `model` deleted by a revoke of ours.
fn codex_config_revoke(document: &mut Value) {
    if document.get("model_provider").and_then(Value::as_str) == Some(PROVIDER) {
        write::remove_path(document, &["model"]);
        write::remove_path(document, &["model_provider"]);
    }
    write::remove_path(document, &["model_providers", PROVIDER]);
    write::remove_path(document, &["agents", "subagent"]);
}

/// Codex reads `auth.json` before the config, so the key goes here, not in `config.toml`.
///
/// Existing tokens are left alone: upstream keeps them so a user can switch back to their ChatGPT
/// login without logging in again.
fn codex_auth_apply(document: &mut Value, payload: &Payload) {
    write::set_path(
        document,
        &["OPENAI_API_KEY"],
        Value::String(payload.key().to_owned()),
    );
    write::set_path(document, &["auth_mode"], Value::String("apikey".to_owned()));
}

fn codex_auth_revoke(document: &mut Value) {
    write::remove_path(document, &["OPENAI_API_KEY"]);
    write::remove_path(document, &["auth_mode"]);
}

// ---------------------------------------------------------------------------------------------
// Cline — ~/.cline/data/globalState.json + secrets.json
// ---------------------------------------------------------------------------------------------

/// Cline wants the origin **without** `/v1`; it appends its own path.
///
/// Both act and plan mode are set, because Cline routes the two independently and configuring only
/// one leaves half the editor pointed elsewhere.
fn cline_state_apply(document: &mut Value, payload: &Payload) {
    let model = Value::String(payload.model().to_owned());
    write::set_path(document, &["actModeApiProvider"], Value::String("openai".to_owned()));
    write::set_path(document, &["planModeApiProvider"], Value::String("openai".to_owned()));
    write::set_path(document, &["openAiBaseUrl"], Value::String(payload.base_bare()));
    write::set_path(document, &["openAiModelId"], model.clone());
    write::set_path(document, &["planModeOpenAiModelId"], model);
}

/// Puts the provider *back* to `cline` rather than deleting it.
///
/// Upstream's choice, and the correct one: Cline treats a missing `actModeApiProvider` as
/// unconfigured and shows a setup prompt, whereas `cline` is its own default. Guarded on the
/// provider still being `openai`, so a user who has since moved to another provider keeps it.
fn cline_state_revoke(document: &mut Value) {
    if document.get("actModeApiProvider").and_then(Value::as_str) != Some("openai") {
        return;
    }
    write::remove_path(document, &["openAiBaseUrl"]);
    write::remove_path(document, &["openAiModelId"]);
    write::remove_path(document, &["planModeOpenAiModelId"]);
    write::set_path(document, &["actModeApiProvider"], Value::String("cline".to_owned()));
    write::set_path(document, &["planModeApiProvider"], Value::String("cline".to_owned()));
}

fn cline_secrets_apply(document: &mut Value, payload: &Payload) {
    write::set_path(
        document,
        &["openAiApiKey"],
        Value::String(payload.key().to_owned()),
    );
}

fn cline_secrets_revoke(document: &mut Value) {
    write::remove_path(document, &["openAiApiKey"]);
}

// ---------------------------------------------------------------------------------------------
// Kilo Code — ~/.local/share/kilo/auth.json + VS Code user settings
// ---------------------------------------------------------------------------------------------

fn kilo_auth_apply(document: &mut Value, payload: &Payload) {
    write::set_path(
        document,
        &["openai-compatible"],
        serde_json::json!({
            "type": "api-key",
            "apiKey": payload.key(),
            "baseUrl": payload.base_v1(),
            "model": payload.model(),
        }),
    );
}

/// Both spellings, because a config written by an older upstream uses the other one.
fn kilo_auth_revoke(document: &mut Value) {
    write::remove_path(document, &["openai-compatible"]);
    write::remove_path(document, &[PROVIDER]);
}

/// VS Code's settings keys are literally dotted — `"kilocode.customProvider"` is one key, not a
/// nested object. Passed as a single segment for that reason: nesting it would write
/// `{"kilocode": {"customProvider": ...}}`, which the extension does not read.
///
/// Note `baseURL` here against `baseUrl` in `auth.json`. That is the extension's spelling, and
/// [`super::spec`]'s kilo marker accepts both for the same reason.
fn kilo_vscode_apply(document: &mut Value, payload: &Payload) {
    write::set_path(
        document,
        &["kilocode.customProvider"],
        serde_json::json!({
            "name": DISPLAY,
            "baseURL": payload.base_v1(),
            "apiKey": payload.key(),
        }),
    );
    write::set_path(
        document,
        &["kilocode.defaultModel"],
        Value::String(payload.model().to_owned()),
    );
}

fn kilo_vscode_revoke(document: &mut Value) {
    write::remove_path(document, &["kilocode.customProvider"]);
    write::remove_path(document, &["kilocode.defaultModel"]);
}

// ---------------------------------------------------------------------------------------------
// GitHub Copilot — VS Code's chatLanguageModels.json, a top-level array
// ---------------------------------------------------------------------------------------------

/// The one config that is an array, upserted by `name`.
///
/// Two things here are upstream's and neither is obvious. The endpoint is
/// `{baseUrl}/chat/completions#models.ai.azure.com` with **no** `/v1` normalisation — the fragment
/// is how Copilot is told to speak the Azure dialect, and appending `/v1` would break the URL it
/// builds. And `vendor: "azure"` follows from that: Copilot only accepts a custom endpoint under a
/// vendor it knows.
fn copilot_apply(document: &mut Value, payload: &Payload) {
    let endpoint = format!(
        "{}/chat/completions#models.ai.azure.com",
        payload.base_url.as_deref().unwrap_or_default()
    );
    let models: Vec<Value> = payload
        .model_names()
        .into_iter()
        .map(|id| {
            serde_json::json!({
                "id": id,
                "name": id,
                "url": endpoint,
                "toolCalling": true,
                "vision": false,
                "maxInputTokens": 128_000,
                "maxOutputTokens": 16_000,
            })
        })
        .collect();
    let entry = serde_json::json!({
        "name": DISPLAY,
        "vendor": "azure",
        "apiKey": payload.key_or_placeholder(),
        "models": models,
    });

    // A non-array document is replaced with one, matching upstream's
    // `Array.isArray(parsed) ? parsed : []`. The alternative — merging into an object — would
    // produce a file Copilot silently ignores.
    with_array(document, |entries| {
        match entries
            .iter()
            .position(|existing| existing.get("name").and_then(Value::as_str) == Some(DISPLAY))
        {
            Some(index) => {
                if let Some(slot) = entries.get_mut(index) {
                    *slot = entry;
                }
            }
            None => entries.push(entry),
        }
    });
}

fn copilot_revoke(document: &mut Value) {
    with_array(document, |entries| {
        entries.retain(|entry| entry.get("name").and_then(Value::as_str) != Some(DISPLAY));
    });
}

/// Run `action` against the document as an array, replacing it with one if it is not.
///
/// A closure rather than a `&mut Vec` return so there is no branch for "just made it an array and
/// it still is not one" — a branch that cannot be reached is a branch that cannot be tested, and
/// the only ways to write it are an `unwrap` the lints deny or a silent no-op.
fn with_array(document: &mut Value, action: impl FnOnce(&mut Vec<Value>)) {
    if !document.is_array() {
        *document = Value::Array(Vec::new());
    }
    if let Some(entries) = document.as_array_mut() {
        action(entries);
    }
}

// ---------------------------------------------------------------------------------------------
// opencode — ~/.config/opencode/opencode.json
// ---------------------------------------------------------------------------------------------

/// The provider entry, preserving models a previous apply put there.
///
/// `npm: "@ai-sdk/openai-compatible"` tells opencode which client to load, so it must survive a
/// merge; the read-modify-write below starts from the existing entry for that reason rather than
/// building a fresh one.
fn opencode_apply(document: &mut Value, payload: &Payload) {
    let names = payload.model_names();
    let existing = document
        .get("provider")
        .and_then(|providers| providers.get(PROVIDER))
        .cloned();
    let mut provider = existing.unwrap_or_else(|| {
        serde_json::json!({ "npm": "@ai-sdk/openai-compatible", "options": {}, "models": {} })
    });

    write::set_path(&mut provider, &["options", "baseURL"], Value::String(payload.base_v1()));
    write::set_path(
        &mut provider,
        &["options", "apiKey"],
        Value::String(payload.key_or_placeholder().to_owned()),
    );
    for name in &names {
        write::set_path(
            &mut provider,
            &["models", name],
            serde_json::json!({
                "name": name,
                "modalities": {"input": ["text", "image"], "output": ["text"]},
            }),
        );
    }
    write::set_path(document, &["provider", PROVIDER], provider);

    // An explicitly empty `activeModel` clears the selection rather than defaulting to the first
    // model — that is how the dashboard's "no default" option is sent.
    match payload.active_model.as_deref() {
        Some("") => write::set_path(document, &["model"], Value::String(String::new())),
        Some(active) => {
            write::set_path(document, &["model"], Value::String(qualified(active)));
        }
        None => {
            if let Some(first) = names.first() {
                write::set_path(document, &["model"], Value::String(qualified(first)));
            }
        }
    }

    let subagent = match payload.subagent_model.as_deref() {
        Some(model) if !model.is_empty() => Some(model.to_owned()),
        Some(_) | None => names.first().cloned(),
    };
    if let Some(subagent) = subagent {
        write::set_path(
            document,
            &["agent", "explorer"],
            serde_json::json!({
                "description": "Fast explorer subagent for codebase exploration",
                "mode": "subagent",
                "model": qualified(&subagent),
            }),
        );
    }
}

/// Removes the provider, and the selection only while it still names this provider.
fn opencode_revoke(document: &mut Value) {
    write::remove_path(document, &["provider", PROVIDER]);
    if is_qualified(document.get("model")) {
        write::remove_path(document, &["model"]);
    }
    if is_qualified(document.get("agent").and_then(|agent| agent.get("explorer")).and_then(|explorer| explorer.get("model"))) {
        write::remove_path(document, &["agent", "explorer"]);
    }
}

/// A model name in `9router/{model}` form, which opencode and openclaw both use.
fn qualified(model: &str) -> String {
    format!("{PROVIDER}/{model}")
}

fn is_qualified(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|model| model.starts_with(&format!("{PROVIDER}/")))
}

// ---------------------------------------------------------------------------------------------
// Factory Droid — ~/.factory/settings.json, customModels as an array
// ---------------------------------------------------------------------------------------------

/// The id prefix droid entries carry. Matched by **prefix**, not equality: the entries are
/// `custom:9Router-0`, `-1`, and so on, so an equality test would miss every one of them.
const DROID_ID_PREFIX: &str = "custom:9Router";

/// Droid's own placeholder, which is **not** [`PLACEHOLDER_KEY`].
///
/// Upstream writes `"your_api_key"` here where it writes `sk_9router` elsewhere. Kept as upstream
/// has it: the string is what a user sees in their settings file when they applied without a key,
/// and changing it would make this port's output differ from the dashboard that wrote it.
const DROID_PLACEHOLDER_KEY: &str = "your_api_key";

/// Rebuilds the router's entries, leaving the user's other custom models alone.
///
/// The default model is moved to the front rather than flagged: droid takes the first entry as the
/// default, so ordering *is* the setting. An explicitly empty `activeModel` skips the reorder,
/// which is how "no default" is expressed.
fn droid_apply(document: &mut Value, payload: &Payload) {
    let names = payload.model_names();
    let key = match payload.api_key.as_deref() {
        Some(key) if !key.trim().is_empty() => key,
        Some(_) | None => DROID_PLACEHOLDER_KEY,
    };
    let base = payload.base_v1();

    let mut models: Vec<Value> = document
        .get("customModels")
        .and_then(Value::as_array)
        .map(|models| {
            models
                .iter()
                .filter(|model| !has_droid_prefix(model))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    // Where our own entries start, once the user's are kept. See the divergence below.
    let ours_begin = models.len();

    for (index, name) in names.iter().enumerate() {
        models.push(serde_json::json!({
            "model": name,
            "id": format!("{DROID_ID_PREFIX}-{index}"),
            "index": index,
            "baseUrl": base,
            "apiKey": key,
            "displayName": name,
            "maxOutputTokens": 131_072,
            "noImageSupport": false,
            "provider": "openai",
        }));
    }

    // An empty `activeModel` means "set no default", so the reorder is skipped entirely.
    //
    // DIVERGENCE: the offset by `ours_begin` is this port's. Upstream resolves the chosen model to
    // a position in `modelsArray` and then splices that position out of the *merged* array, whose
    // leading entries are the user's own custom models. So a user holding one custom model who
    // picks the third of three offered models gets the second one as their default — off by
    // exactly the number of entries they had. With no custom models of their own the two agree,
    // which is why the bug survives upstream. Reproducing it would mean the dashboard's chosen
    // default silently is not the one droid uses.
    let default_index = match payload.active_model.as_deref() {
        Some("") => None,
        Some(active) => Some(
            ours_begin + names.iter().position(|name| name == active).unwrap_or(0),
        ),
        None => Some(ours_begin),
    };
    if let Some(index) = default_index
        && index < models.len()
    {
        let entry = models.remove(index);
        models.insert(0, entry);
        for (position, model) in models.iter_mut().enumerate() {
            write::set_path(model, &["index"], Value::from(position));
        }
    }

    write::set_path(document, &["customModels"], Value::Array(models));
    // Upstream drops the key when nothing is left, so an unconfigured droid has no empty array.
    prune_empty_array(document, "customModels");
}

fn droid_revoke(document: &mut Value) {
    let Some(models) = document.get_mut("customModels").and_then(Value::as_array_mut) else {
        return;
    };
    models.retain(|model| !has_droid_prefix(model));
    prune_empty_array(document, "customModels");
}

fn has_droid_prefix(model: &Value) -> bool {
    model
        .get("id")
        .and_then(Value::as_str)
        .is_some_and(|id| id.starts_with(DROID_ID_PREFIX))
}

/// Drop an array-valued key once it is empty.
fn prune_empty_array(document: &mut Value, key: &str) {
    let empty = document
        .get(key)
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty);
    if empty
        && let Some(map) = document.as_object_mut()
    {
        map.remove(key);
    }
}

// ---------------------------------------------------------------------------------------------
// Hermes — ~/.hermes/config.yaml + ~/.hermes/.env
// ---------------------------------------------------------------------------------------------

/// Replaces the `model:` block and leaves the rest of the YAML byte-identical.
///
/// The document for a text format is a `Value::String`, so this edits the string rather than a
/// parsed tree — which is the whole point: Hermes' config is hand-written, and a round trip through
/// a YAML library would rewrite the user's quoting, key order and comments.
fn hermes_config_apply(document: &mut Value, payload: &Payload) {
    let block = super::yaml_block::build_model_block(payload.model(), &payload.base_v1());
    let text = document.as_str().unwrap_or_default();
    *document = Value::String(super::yaml_block::upsert_model_block(text, &block));
}

fn hermes_config_revoke(document: &mut Value) {
    let text = document.as_str().unwrap_or_default();
    *document = Value::String(super::yaml_block::remove_model_block(text));
}

/// The key goes in `.env`, never in the YAML.
///
/// The block Hermes gets written references `${OPENAI_API_KEY}` literally, and Hermes expands it
/// from the environment or from this file. So a key inlined in `config.yaml` would end up in
/// whatever dotfile repo the user keeps their config in.
///
/// Written only when the caller supplied one, per upstream's `if (apiKey)`. With no key the
/// document is left untouched, which the executor sees as "nothing changed" and skips the write —
/// so an apply without a key does not create an empty `.env`.
fn hermes_env_apply(document: &mut Value, payload: &Payload) {
    let key = payload.key();
    if key.is_empty() {
        return;
    }
    let text = document.as_str().unwrap_or_default();
    *document = Value::String(write::upsert_env(text, HERMES_KEY_VAR, key));
}

/// Deliberately a no-op, matching upstream's `DELETE`, which never opens `.env`.
///
/// Worth stating because the omission looks like one: `OPENAI_API_KEY` is a generic name a user may
/// well be using for real OpenAI, and after a revoke the YAML block that referenced it is gone, so
/// nothing here reads it. Removing a variable this port does not own is the worse of the two
/// mistakes.
fn hermes_env_revoke(_document: &mut Value) {}

const HERMES_KEY_VAR: &str = "OPENAI_API_KEY";

// ---------------------------------------------------------------------------------------------
// DeepSeek TUI — ~/.deepseek/config.toml
// ---------------------------------------------------------------------------------------------

/// Points the `openai` provider here and selects it.
///
/// Note what is *not* written: no `9router` string appears anywhere in this file. That is why
/// [`super::spec`]'s deepseek marker tests `provider == "openai"` plus a local `base_url` instead
/// of grepping for a name — a text search would report "not configured" straight after this
/// succeeds.
///
/// DIVERGENCE: this merges where upstream replaces. Upstream's apply writes a freshly built file
/// containing only these four keys, and its revoke writes a two-line default, so either one throws
/// away a user's other provider sections and any unrelated settings. The backup this port takes
/// makes that survivable rather than fine. Merging reaches the same end state — the provider is
/// selected and points here — without deleting configuration nobody asked it to touch.
fn deepseek_apply(document: &mut Value, payload: &Payload) {
    write::set_path(document, &["provider"], Value::String("openai".to_owned()));
    write::set_path(
        document,
        &["providers", "openai", "base_url"],
        Value::String(payload.base_v1()),
    );
    write::set_path(
        document,
        &["providers", "openai", "api_key"],
        Value::String(payload.key_or_placeholder().to_owned()),
    );
    write::set_path(
        document,
        &["providers", "openai", "model"],
        Value::String(payload.model().to_owned()),
    );
}

/// Restores DeepSeek's own default provider, and drops the `openai` section only while it still
/// points at a local router.
///
/// The guard matters: a user who has since put their real OpenAI key in that section must keep it.
/// Upstream cannot make this distinction because it replaces the whole file.
fn deepseek_revoke(document: &mut Value) {
    if document.get("provider").and_then(Value::as_str) == Some("openai") {
        write::set_path(document, &["provider"], Value::String("deepseek".to_owned()));
    }
    let ours = document
        .get("providers")
        .and_then(|providers| providers.get("openai"))
        .and_then(|openai| openai.get("base_url"))
        .and_then(Value::as_str)
        .is_some_and(points_at_a_local_router);
    if ours {
        write::remove_path(document, &["providers", "openai"]);
    }
}

/// The local-URL test upstream uses for deepseek and hermes: `/localhost|127\.0\.0\.1|0\.0\.0\.0/`.
///
/// A different set from the one cline and kilo use — it accepts `0.0.0.0` and does not accept a
/// `9router` hostname. [`super::spec`] keeps the two apart for the same reason, and merging them
/// here would make a revoke delete a section the status route does not consider ours.
fn points_at_a_local_router(url: &str) -> bool {
    url.contains("localhost") || url.contains("127.0.0.1") || url.contains("0.0.0.0")
}

// ---------------------------------------------------------------------------------------------
// jcode — ~/.jcode/config.toml + $XDG_CONFIG_HOME/jcode/provider-9router.env
// ---------------------------------------------------------------------------------------------

/// jcode's key is not in this file: `api_key_env` and `env_file` name where to find it.
///
/// So the two targets are coupled by those two strings, and a config written without the `.env`
/// leaves jcode looking up a variable nothing sets. `requires_api_key: true` is what makes jcode
/// fail loudly in that case rather than sending an unauthenticated request.
fn jcode_config_apply(document: &mut Value, payload: &Payload) {
    let default_model = payload
        .model_names()
        .first()
        .cloned()
        // Upstream's literal fallback. Kept rather than made empty: jcode treats a missing
        // `default_model` as unconfigured, and this is the string a user's config already has.
        .unwrap_or_else(|| "cc/claude-opus-4-7".to_owned());
    write::set_path(
        document,
        &["providers", PROVIDER],
        serde_json::json!({
            "type": "openai-compatible",
            "base_url": payload.base_v1(),
            "auth": "bearer",
            "api_key_env": JCODE_KEY_VAR,
            "env_file": JCODE_ENV_FILE,
            "default_model": default_model,
            "requires_api_key": true,
        }),
    );
}

fn jcode_config_revoke(document: &mut Value) {
    write::remove_path(document, &["providers", PROVIDER]);
}

fn jcode_env_apply(document: &mut Value, payload: &Payload) {
    let text = document.as_str().unwrap_or_default();
    *document = Value::String(write::upsert_env(text, JCODE_KEY_VAR, payload.key()));
}

/// Removed here, unlike hermes', because this variable is ours: the name is router-specific and
/// only the provider entry this revoke just deleted ever read it.
fn jcode_env_revoke(document: &mut Value) {
    let text = document.as_str().unwrap_or_default();
    *document = Value::String(write::remove_env(text, JCODE_KEY_VAR));
}

const JCODE_KEY_VAR: &str = "JCODE_9ROUTER_API_KEY";
const JCODE_ENV_FILE: &str = "provider-9router.env";

// ---------------------------------------------------------------------------------------------
// OpenClaw — ~/.openclaw/openclaw.json, plus a models.json per agent directory
// ---------------------------------------------------------------------------------------------

/// OpenClaw's own placeholder, which matches droid's rather than [`PLACEHOLDER_KEY`].
const OPENCLAW_PLACEHOLDER_KEY: &str = "your_api_key";

/// The provider entry, the default model, and the allowlist that gates it.
///
/// Three things have to agree or the model is written but unusable: `models.providers.9router`
/// supplies the endpoint, `agents.defaults.model.primary` selects it, and
/// `agents.defaults.models` is an allowlist that OpenClaw checks the selection against. Writing
/// the first two without the third leaves a config that looks right and refuses to run.
fn openclaw_apply(document: &mut Value, payload: &Payload) {
    let base = payload.base_v1();
    let key = match payload.api_key.as_deref() {
        Some(key) if !key.trim().is_empty() => key,
        Some(_) | None => OPENCLAW_PLACEHOLDER_KEY,
    };
    let primary = payload.model().to_owned();

    // Every model this apply should allow: the default, plus any per-agent override.
    let mut all: Vec<String> = vec![primary.clone()];
    for model in payload.agent_models.iter().flat_map(|models| models.values()) {
        if let Some(model) = model.as_str().filter(|model| !model.is_empty())
            && !all.iter().any(|existing| existing == model)
        {
            all.push(model.to_owned());
        }
    }

    // Stale `9router/*` entries go first, so a re-apply that drops a model drops its allowance too.
    retain_unqualified(document);

    write::set_path(
        document,
        &["agents", "defaults", "model", "primary"],
        Value::String(qualified(&primary)),
    );
    for model in &all {
        write::set_path(
            document,
            &["agents", "defaults", "models", &qualified(model)],
            Value::Object(Map::new()),
        );
    }
    write::set_path(
        document,
        &["models", "providers", PROVIDER],
        serde_json::json!({
            "baseUrl": base,
            "apiKey": key,
            "api": "openai-completions",
            "models": all.iter().map(|model| serde_json::json!({
                "id": model,
                // Upstream's `m.split("/").pop() || m`: the display name is the last path
                // segment, so `anthropic/opus` shows as `opus`.
                "name": model.rsplit('/').next().unwrap_or(model),
            })).collect::<Vec<Value>>(),
        }),
    );

    // Per-agent selections in `agents.list`. An agent whose model this payload does not name has
    // its own left alone unless it still points at us, in which case the key is dropped so the
    // agent falls back to the default rather than keeping a model no longer in the allowlist.
    let overrides = payload.agent_models.clone().unwrap_or_default();
    if let Some(agents) = document
        .get_mut("agents")
        .and_then(|agents| agents.get_mut("list"))
        .and_then(Value::as_array_mut)
    {
        for agent in agents.iter_mut() {
            let id = agent.get("id").and_then(Value::as_str).unwrap_or_default().to_owned();
            let selected = overrides.get(&id).and_then(Value::as_str).filter(|model| !model.is_empty());
            match selected {
                Some(model) => {
                    write::set_path(agent, &["model"], Value::String(qualified(model)));
                }
                None => {
                    if agent_points_at_us(agent)
                        && let Some(map) = agent.as_object_mut()
                    {
                        map.remove("model");
                    }
                }
            }
        }
    }
}

/// Removes the provider, its allowlist entries, and the selection while it still names us.
fn openclaw_revoke(document: &mut Value) {
    write::remove_path(document, &["models", "providers", PROVIDER]);
    retain_unqualified(document);
    let primary_is_ours = document
        .get("agents")
        .and_then(|agents| agents.get("defaults"))
        .and_then(|defaults| defaults.get("model"))
        .and_then(|model| model.get("primary"));
    if is_qualified(primary_is_ours) {
        write::remove_path(document, &["agents", "defaults", "model", "primary"]);
    }
    if let Some(agents) = document
        .get_mut("agents")
        .and_then(|agents| agents.get_mut("list"))
        .and_then(Value::as_array_mut)
    {
        for agent in agents.iter_mut() {
            if agent_points_at_us(agent)
                && let Some(map) = agent.as_object_mut()
            {
                map.remove("model");
            }
        }
    }
}

/// Drop every `9router/*` key from the default allowlist.
fn retain_unqualified(document: &mut Value) {
    let Some(models) = document
        .get_mut("agents")
        .and_then(|agents| agents.get_mut("defaults"))
        .and_then(|defaults| defaults.get_mut("models"))
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    models.retain(|key, _| !key.starts_with(&format!("{PROVIDER}/")));
}

/// Whether an agent's model still names this provider.
///
/// The field is either a plain string or `{primary, fallbacks}`, which is upstream's
/// `resolveAgentModel`. Reading only the string form would leave the object form behind.
fn agent_points_at_us(agent: &Value) -> bool {
    let model = agent.get("model");
    match model {
        Some(Value::String(_)) => is_qualified(model),
        Some(object) => is_qualified(object.get("primary")),
        None => false,
    }
}

/// The per-agent `models.json` files, whose directories are named in the settings document.
///
/// DIVERGENCE: upstream creates `agentDir` if it does not exist. This port writes only to a
/// directory that is already there. `agentDir` is a path out of a config file being used as a
/// destination, so creating it means a settings file saying `../../.ssh` gets a directory tree; and
/// an agent directory that does not exist yet belongs to an agent OpenClaw has not set up, which
/// has nothing to read the file anyway. Skipped directories are reported as warnings rather than
/// swallowed.
fn openclaw_agent_models(
    document: &Value,
    payload: &Payload,
    direction: Direction,
    outcome: &mut Outcome,
) -> Result<(), write::WriteError> {
    let Some(agents) = document
        .get("agents")
        .and_then(|agents| agents.get("list"))
        .and_then(Value::as_array)
    else {
        return Ok(());
    };
    let overrides = payload.agent_models.clone().unwrap_or_default();
    let base = payload.base_v1();
    let key = match payload.api_key.as_deref() {
        Some(key) if !key.trim().is_empty() => key,
        Some(_) | None => OPENCLAW_PLACEHOLDER_KEY,
    };

    for agent in agents {
        let Some(directory) = agent.get("agentDir").and_then(Value::as_str) else {
            continue;
        };
        let directory = std::path::Path::new(directory);
        if !directory.is_dir() {
            outcome.warnings.push(format!(
                "Skipped {}: the agent directory does not exist, so nothing there would read it.",
                directory.join("models.json").display()
            ));
            continue;
        }
        let path = directory.join("models.json");
        let mut models = write::read_for_merge(&path, Format::Json)?;
        let before = models.clone();
        match direction {
            Direction::Apply => {
                let id = overrides
                    .get(agent.get("id").and_then(Value::as_str).unwrap_or_default())
                    .and_then(Value::as_str)
                    .filter(|model| !model.is_empty())
                    .unwrap_or_else(|| payload.model());
                write::set_path(
                    &mut models,
                    &["providers", PROVIDER],
                    serde_json::json!({
                        "baseUrl": base,
                        "apiKey": key,
                        "api": "openai-completions",
                        "models": [{"id": id, "name": id.rsplit('/').next().unwrap_or(id)}],
                    }),
                );
            }
            Direction::Revoke => write::remove_path(&mut models, &["providers", PROVIDER]),
        }
        if models == before {
            continue;
        }
        let text = write::serialise(&models, Format::Json)?;
        let backup_wanted = path.exists() && !write::backup_path(&path).exists();
        write::write_atomically(&path, &text)?;
        if backup_wanted {
            outcome.backed_up.push(write::backup_path(&path));
        }
        outcome.written.push(path);
    }
    Ok(())
}

/// How one tool is written.
#[derive(Debug)]
pub(crate) struct Writer {
    /// Checked before any file is opened, so a rejected request changes nothing. Upstream's own
    /// guard, verbatim per tool: codex wants all three of base URL, key and model; copilot wants
    /// only a base URL.
    pub(crate) validate: fn(&Payload) -> Result<(), &'static str>,
    pub(crate) targets: &'static [Target],
    /// Files whose paths are named *inside* the config a target just wrote.
    ///
    /// Only openclaw needs this: it keeps a list of agents, each of which may name its own
    /// directory, and each of those gets a `models.json`. The paths cannot go in `targets` because
    /// they are not known until the settings file has been read. It runs after the targets, with
    /// the document they produced.
    pub(crate) derived: Option<DerivedWriter>,
}

/// Signature of the derived-write hook. See [`Writer::derived`].
type DerivedWriter =
    fn(&Value, &Payload, Direction, &mut Outcome) -> Result<(), write::WriteError>;

/// What an apply or revoke did, for the response body.
#[derive(Debug, Default)]
pub(crate) struct Outcome {
    /// The files actually written, in the order they were written.
    pub(crate) written: Vec<std::path::PathBuf>,
    /// Backups taken during this call.
    pub(crate) backed_up: Vec<std::path::PathBuf>,
    /// Best-effort targets that failed, reported rather than swallowed. Upstream discards these
    /// silently, which leaves a user whose VS Code settings are read-only with no way to find out
    /// why their editor never picked the config up.
    pub(crate) warnings: Vec<String>,
    /// True when a revoke found nothing to reset.
    pub(crate) nothing_to_do: bool,
}

/// Which direction a mutation goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    Apply,
    Revoke,
}

impl Writer {
    /// Run every target, in order.
    ///
    /// Ordering is upstream's, and for the two-file tools it puts the config before the credential.
    /// Not arbitrary: if the second write fails, the state left behind is a tool pointing here with
    /// no key, which fails loudly on first use — the reverse leaves a key on disk for a provider
    /// the tool is not configured to call.
    pub(crate) fn run(
        &self,
        direction: Direction,
        payload: &Payload,
    ) -> Result<Outcome, write::WriteError> {
        let mut outcome = Outcome::default();
        let mut last_document = Value::Null;
        for target in self.targets {
            match target.run(direction, payload, &mut outcome) {
                Ok(document) => last_document = document,
                Err(error) if target.required == Required::BestEffort => {
                    outcome.warnings.push(error.message());
                }
                Err(error) => return Err(error),
            }
        }
        if let Some(derived) = self.derived {
            derived(&last_document, payload, direction, &mut outcome)?;
        }
        // Every target reported "no file, nothing to reset": that is the whole answer, not a
        // partial one.
        outcome.nothing_to_do = direction == Direction::Revoke && outcome.written.is_empty();
        Ok(outcome)
    }
}

impl Target {
    /// Returns the document as it now stands, for [`Writer::derived`] to read paths out of.
    fn run(
        &self,
        direction: Direction,
        payload: &Payload,
        outcome: &mut Outcome,
    ) -> Result<Value, write::WriteError> {
        let path = self.config.resolve().ok_or(write::WriteError::NoHome)?;

        // A revoke of a file that is not there has nothing to remove, and creating one to hold the
        // absence of our keys would leave a config file behind for a tool the user never set up.
        // Upstream returns "No settings file to reset" for the same case.
        if direction == Direction::Revoke && !path.exists() {
            return Ok(Value::Null);
        }

        let mut document = write::read_for_merge(&path, self.config.format)?;
        let before = document.clone();
        match direction {
            Direction::Apply => (self.apply)(&mut document, payload),
            Direction::Revoke => (self.revoke)(&mut document),
        }
        // Nothing changed, so nothing is written. This keeps a revoke from rewriting — and taking a
        // backup of — a file that never held our keys.
        if document == before {
            return Ok(document);
        }

        if direction == Direction::Revoke
            && self.on_empty == OnEmpty::Delete
            && document.as_object().is_some_and(Map::is_empty)
        {
            std::fs::remove_file(&path).map_err(|error| write::WriteError::Io {
                path: path.clone(),
                detail: error.to_string(),
            })?;
            outcome.written.push(path);
            return Ok(document);
        }

        let text = write::serialise(&document, self.config.format)?;
        let backup_wanted = path.exists() && !write::backup_path(&path).exists();
        write::write_atomically(&path, &text)?;
        if backup_wanted {
            outcome.backed_up.push(write::backup_path(&path));
        }
        outcome.written.push(path);
        Ok(document)
    }
}

// Directory roots. Named constants rather than inline literals because a `&[Root]` built inside a
// `const fn` from its parameter is a temporary; only a literal like these is promoted to `'static`.
const CLAUDE_ROOT: &[Root] = &[Root::Home(&[".claude"])];
const CODEX_ROOT: &[Root] = &[Root::Home(&[".codex"])];
const CLINE_ROOT: &[Root] = &[Root::Home(&[".cline", "data"])];
const KILO_ROOT: &[Root] = &[Root::Home(&[".local", "share", "kilo"])];
const VSCODE_ROOT: &[Root] = &[Root::XdgConfig(&["Code", "User"])];

/// A JSON config file below a root.
///
/// `roots` is taken already borrowed rather than built here: a `&[Root::Home(..)]` constructed
/// inside a `const fn` from its own parameter is a temporary, and only a literal gets promoted to
/// `'static`.
const fn json_at(roots: &'static [Root], file: &'static [&'static str]) -> ConfigFile {
    ConfigFile {
        roots,
        segments: file,
        format: Format::Json,
        indirect: None,
    }
}

/// The write descriptor for a tool, or `None` when there is nothing to write.
///
/// Matched on the id rather than stored in [`super::spec::TOOLS`] so the detection table stays a
/// description of where configs live, with nothing about mutation in it. A tool missing from here
/// is not writable, which is checked against `spec`'s own `writable` flag by a test — the two must
/// not disagree, or the dashboard would offer a toggle that 501s.
pub(crate) fn writer_for(tool_id: &str) -> Option<&'static Writer> {
    match tool_id {
        "claude-settings" => Some(&CLAUDE),
        "codex-settings" => Some(&CODEX),
        "cline-settings" => Some(&CLINE),
        "kilo-settings" => Some(&KILO),
        "copilot-settings" => Some(&COPILOT),
        "opencode-settings" => Some(&OPENCODE),
        "droid-settings" => Some(&DROID),
        "hermes-settings" => Some(&HERMES),
        "deepseek-tui-settings" => Some(&DEEPSEEK),
        "jcode-settings" => Some(&JCODE),
        "openclaw-settings" => Some(&OPENCLAW),
        _ => None,
    }
}

const OPENCLAW_ROOT: &[Root] = &[Root::Home(&[".openclaw"])];

static OPENCLAW: Writer = Writer {
    validate: |payload| {
        if payload.base_url.as_deref().is_none_or(str::is_empty)
            || payload.model.as_deref().is_none_or(str::is_empty)
        {
            return Err("baseUrl and model are required");
        }
        Ok(())
    },
    targets: &[Target {
        config: json_at(OPENCLAW_ROOT, &["openclaw.json"]),
        required: Required::Yes,
        on_empty: OnEmpty::Keep,
        apply: openclaw_apply,
        revoke: openclaw_revoke,
    }],
    derived: Some(openclaw_agent_models),
};

const JCODE_ROOT: &[Root] = &[Root::Home(&[".jcode"])];
/// jcode's env file sits under XDG, not beside its config — upstream reads `$XDG_CONFIG_HOME`
/// directly here while resolving the config from `$HOME`.
const JCODE_ENV_ROOT: &[Root] = &[Root::XdgConfig(&["jcode"])];

static JCODE: Writer = Writer {
    // `if (!baseUrl || !apiKey)` — no model requirement, since `default_model` has a fallback.
    validate: |payload| {
        if payload.base_url.as_deref().is_none_or(str::is_empty)
            || payload.api_key.as_deref().is_none_or(str::is_empty)
        {
            return Err("baseUrl and apiKey are required");
        }
        Ok(())
    },
    targets: &[
        Target {
            config: ConfigFile {
                roots: JCODE_ROOT,
                segments: &["config.toml"],
                format: Format::Toml,
                indirect: None,
            },
            required: Required::Yes,
            on_empty: OnEmpty::Keep,
            apply: jcode_config_apply,
            revoke: jcode_config_revoke,
        },
        Target {
            config: ConfigFile {
                roots: JCODE_ENV_ROOT,
                segments: &[JCODE_ENV_FILE],
                format: Format::DotEnv,
                indirect: None,
            },
            required: Required::Yes,
            on_empty: OnEmpty::Keep,
            apply: jcode_env_apply,
            revoke: jcode_env_revoke,
        },
    ],
    derived: None,
};

const HERMES_ROOT: &[Root] = &[Root::Home(&[".hermes"])];
const DEEPSEEK_ROOT: &[Root] = &[Root::Home(&[".deepseek"])];

static HERMES: Writer = Writer {
    // `if (!baseUrl || !model)` — the key is optional here, and its absence means `.env` is left
    // alone rather than written blank.
    validate: |payload| {
        if payload.base_url.as_deref().is_none_or(str::is_empty)
            || payload.model.as_deref().is_none_or(str::is_empty)
        {
            return Err("baseUrl and model are required");
        }
        Ok(())
    },
    targets: &[
        Target {
            config: ConfigFile {
                roots: HERMES_ROOT,
                segments: &["config.yaml"],
                format: Format::YamlBlock,
                indirect: None,
            },
            required: Required::Yes,
            on_empty: OnEmpty::Keep,
            apply: hermes_config_apply,
            revoke: hermes_config_revoke,
        },
        Target {
            config: ConfigFile {
                roots: HERMES_ROOT,
                segments: &[".env"],
                format: Format::DotEnv,
                indirect: None,
            },
            required: Required::Yes,
            on_empty: OnEmpty::Keep,
            apply: hermes_env_apply,
            revoke: hermes_env_revoke,
        },
    ],
    derived: None,
};

static DEEPSEEK: Writer = Writer {
    validate: |payload| {
        if payload.base_url.as_deref().is_none_or(str::is_empty)
            || payload.model.as_deref().is_none_or(str::is_empty)
        {
            return Err("baseUrl and model are required");
        }
        Ok(())
    },
    targets: &[Target {
        config: ConfigFile {
            roots: DEEPSEEK_ROOT,
            segments: &["config.toml"],
            format: Format::Toml,
            indirect: None,
        },
        required: Required::Yes,
        on_empty: OnEmpty::Keep,
        apply: deepseek_apply,
        revoke: deepseek_revoke,
    }],
    derived: None,
};

const OPENCODE_ROOT: &[Root] = &[Root::XdgConfig(&["opencode"])];
const DROID_ROOT: &[Root] = &[Root::Home(&[".factory"])];

static COPILOT: Writer = Writer {
    // `if (!baseUrl || !models?.length)` — the key is defaulted, so it is not required.
    validate: |payload| {
        if payload.base_url.as_deref().is_none_or(str::is_empty) {
            return Err("baseUrl and models are required");
        }
        if payload.model_names().is_empty() {
            return Err("baseUrl and models are required");
        }
        Ok(())
    },
    targets: &[Target {
        config: json_at(VSCODE_ROOT, &["chatLanguageModels.json"]),
        required: Required::Yes,
        on_empty: OnEmpty::Keep,
        apply: copilot_apply,
        revoke: copilot_revoke,
    }],
    derived: None,
};

static OPENCODE: Writer = Writer {
    validate: |payload| {
        if payload.base_url.as_deref().is_none_or(str::is_empty)
            || payload.model_names().is_empty()
        {
            return Err("baseUrl and at least one model are required");
        }
        Ok(())
    },
    targets: &[Target {
        config: json_at(OPENCODE_ROOT, &["opencode.json"]),
        required: Required::Yes,
        on_empty: OnEmpty::Keep,
        apply: opencode_apply,
        revoke: opencode_revoke,
    }],
    derived: None,
};

static DROID: Writer = Writer {
    validate: |payload| {
        if payload.base_url.as_deref().is_none_or(str::is_empty)
            || payload.model_names().is_empty()
        {
            return Err("baseUrl and at least one model are required");
        }
        Ok(())
    },
    targets: &[Target {
        config: json_at(DROID_ROOT, &["settings.json"]),
        required: Required::Yes,
        on_empty: OnEmpty::Keep,
        apply: droid_apply,
        revoke: droid_revoke,
    }],
    derived: None,
};

static CLAUDE: Writer = Writer {
    // Upstream checks `!env || typeof env !== "object"`, which a missing `env` fails.
    validate: |payload| {
        if payload.env.is_none() {
            return Err("Invalid env object");
        }
        Ok(())
    },
    targets: &[Target {
        config: json_at(CLAUDE_ROOT, &["settings.json"]),
        required: Required::Yes,
        on_empty: OnEmpty::Keep,
        apply: claude_apply,
        revoke: claude_revoke,
    }],
    derived: None,
};

static CODEX: Writer = Writer {
    validate: base_key_and_model,
    targets: &[
        Target {
            config: ConfigFile {
                roots: CODEX_ROOT,
                segments: &["config.toml"],
                format: Format::Toml,
                indirect: None,
            },
            required: Required::Yes,
            on_empty: OnEmpty::Keep,
            apply: codex_config_apply,
            revoke: codex_config_revoke,
        },
        Target {
            config: json_at(CODEX_ROOT, &["auth.json"]),
            required: Required::Yes,
            // An empty `auth.json` reads as api-key mode with a blank key, which stops Codex
            // falling back to a ChatGPT login. Upstream unlinks it for that reason.
            on_empty: OnEmpty::Delete,
            apply: codex_auth_apply,
            revoke: codex_auth_revoke,
        },
    ],
    derived: None,
};

static CLINE: Writer = Writer {
    validate: base_key_and_model,
    targets: &[
        Target {
            config: json_at(CLINE_ROOT, &["globalState.json"]),
            required: Required::Yes,
            on_empty: OnEmpty::Keep,
            apply: cline_state_apply,
            revoke: cline_state_revoke,
        },
        Target {
            config: json_at(CLINE_ROOT, &["secrets.json"]),
            required: Required::Yes,
            on_empty: OnEmpty::Keep,
            apply: cline_secrets_apply,
            revoke: cline_secrets_revoke,
        },
    ],
    derived: None,
};

static KILO: Writer = Writer {
    validate: base_key_and_model,
    targets: &[
        Target {
            config: json_at(KILO_ROOT, &["auth.json"]),
            required: Required::Yes,
            on_empty: OnEmpty::Keep,
            apply: kilo_auth_apply,
            revoke: kilo_auth_revoke,
        },
        Target {
            config: json_at(VSCODE_ROOT, &["settings.json"]),
            // Upstream wraps this write in its own `try {} catch {}`: a user without VS Code
            // installed should still get `auth.json`, and Kilo works from that alone.
            required: Required::BestEffort,
            on_empty: OnEmpty::Keep,
            apply: kilo_vscode_apply,
            revoke: kilo_vscode_revoke,
        },
    ],
    derived: None,
};
