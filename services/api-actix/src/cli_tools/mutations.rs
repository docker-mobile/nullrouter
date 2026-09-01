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

/// How one tool is written.
#[derive(Debug)]
pub(crate) struct Writer {
    /// Checked before any file is opened, so a rejected request changes nothing. Upstream's own
    /// guard, verbatim per tool: codex wants all three of base URL, key and model; copilot wants
    /// only a base URL.
    pub(crate) validate: fn(&Payload) -> Result<(), &'static str>,
    pub(crate) targets: &'static [Target],
}

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
        for target in self.targets {
            match target.run(direction, payload, &mut outcome) {
                Ok(()) => {}
                Err(error) if target.required == Required::BestEffort => {
                    outcome.warnings.push(error.message());
                }
                Err(error) => return Err(error),
            }
        }
        // Every target reported "no file, nothing to reset": that is the whole answer, not a
        // partial one.
        outcome.nothing_to_do = direction == Direction::Revoke && outcome.written.is_empty();
        Ok(outcome)
    }
}

impl Target {
    fn run(
        &self,
        direction: Direction,
        payload: &Payload,
        outcome: &mut Outcome,
    ) -> Result<(), write::WriteError> {
        let path = self.config.resolve().ok_or(write::WriteError::NoHome)?;

        // A revoke of a file that is not there has nothing to remove, and creating one to hold the
        // absence of our keys would leave a config file behind for a tool the user never set up.
        // Upstream returns "No settings file to reset" for the same case.
        if direction == Direction::Revoke && !path.exists() {
            return Ok(());
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
            return Ok(());
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
            return Ok(());
        }

        let text = write::serialise(&document, self.config.format)?;
        let backup_wanted = path.exists() && !write::backup_path(&path).exists();
        write::write_atomically(&path, &text)?;
        if backup_wanted {
            outcome.backed_up.push(write::backup_path(&path));
        }
        outcome.written.push(path);
        Ok(())
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
        _ => None,
    }
}

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
};
