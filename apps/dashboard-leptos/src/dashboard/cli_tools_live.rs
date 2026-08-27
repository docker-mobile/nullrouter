//! Live CLI-tool state: which agents this machine actually has, and what one
//! tool's config write did.
//!
//! The panel this backs rendered `cli_tools()` — eight compile-time tiles, each
//! captioned "Unchecked", with a detail page whose every control was a disabled
//! preview. The list was a wish, not a reading: it named tools the router never
//! looked for and omitted ones it does check.
//!
//! Two properties shape everything below.
//!
//! * A tile may only describe a key `GET /api/cli-tools/all-statuses` returned.
//!   The tool list is *derived from the response*, never from a table here, so
//!   this build cannot claim to know about a tool the router does not report.
//! * `installed` is [`Option<bool>`]. A tool the router did not report on is
//!   [`Detection::Unknown`], which reads differently from
//!   [`Detection::Missing`]. Saying "not installed" when nothing was checked
//!   would be the same fabrication as saying "installed".
//!
//! Free of `leptos` and of `fetch`, so every branch is unit-testable on the
//! native target.

use crate::api::ApiError;
use serde::Serialize;
use serde_json::Value;

/// The batch endpoint that owns the tool list.
pub const ALL_STATUSES_PATH: &str = "/api/cli-tools/all-statuses";

/// The Cowork MCP registry endpoint.
pub const MCP_REGISTRY_PATH: &str = "/api/cli-tools/cowork-mcp-registry";

/// The Cowork MCP tool-discovery endpoint.
pub const MCP_TOOLS_PATH: &str = "/api/cli-tools/cowork-mcp-tools";

/// Shown where the router reported no value at all.
pub const NO_READING: &str = "—";

/// `GET`/`POST` path for one tool's settings.
///
/// Upstream has no `/api/cli-tools/claude` route: the per-tool resource is
/// `claude-settings`, and `all-statuses` is a fan-out over those handlers
/// (`src/app/api/cli-tools/all-statuses/route.js`). The suffix is added here so
/// a tile's id — which comes from the batch response — addresses the right
/// resource, and is not added twice if the id already carries it.
pub fn settings_path(tool_id: &str) -> String {
    let encoded = encode_path_segment(tool_id);
    if tool_id.ends_with("-settings") {
        format!("/api/cli-tools/{encoded}")
    } else {
        format!("/api/cli-tools/{encoded}-settings")
    }
}

/// Percent-encode everything outside RFC 3986 `unreserved`.
///
/// Tool ids arrive from the server and travel through a URL, so they are encoded
/// rather than trusted to be path-safe.
fn encode_path_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(hex_digit(byte >> 4));
            encoded.push(hex_digit(byte & 0x0f));
        }
    }
    encoded
}

/// One uppercase hex digit for a nibble.
const fn hex_digit(nibble: u8) -> char {
    match nibble {
        0 => '0',
        1 => '1',
        2 => '2',
        3 => '3',
        4 => '4',
        5 => '5',
        6 => '6',
        7 => '7',
        8 => '8',
        9 => '9',
        10 => 'A',
        11 => 'B',
        12 => 'C',
        13 => 'D',
        14 => 'E',
        _ => 'F',
    }
}

/// Whether the router found the tool's binary or config on this machine.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Detection {
    /// `installed: true`.
    Installed,
    /// `installed: false` — the router looked and did not find it.
    Missing,
    /// The router reported no `installed` field, so nothing was checked.
    #[default]
    Unknown,
}

impl Detection {
    /// Read the flag without inventing a default.
    const fn from_flag(flag: Option<bool>) -> Self {
        match flag {
            Some(true) => Self::Installed,
            Some(false) => Self::Missing,
            None => Self::Unknown,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Installed => "Detected",
            Self::Missing => "Not detected",
            Self::Unknown => "Not checked",
        }
    }

    /// Status-pill class, from the shared `is-*` vocabulary.
    pub const fn class_name(self) -> &'static str {
        match self {
            Self::Installed => "is-connected",
            Self::Missing | Self::Unknown => "is-idle",
        }
    }

    /// What the label means, in a sentence.
    pub const fn detail(self) -> &'static str {
        match self {
            Self::Installed => "The router found this tool on the machine.",
            Self::Missing => "The router looked for this tool and did not find it.",
            Self::Unknown => {
                "The router did not report whether this tool is installed, so nothing was checked."
            }
        }
    }
}

/// Whether the tool's config already routes through 9Router.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Routing {
    Configured,
    NotConfigured,
    /// No `has9Router` field, so the config was not inspected.
    #[default]
    Unknown,
}

impl Routing {
    const fn from_flag(flag: Option<bool>) -> Self {
        match flag {
            Some(true) => Self::Configured,
            Some(false) => Self::NotConfigured,
            None => Self::Unknown,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Configured => "Routed here",
            Self::NotConfigured => "Not routed",
            Self::Unknown => "Config not read",
        }
    }

    pub const fn class_name(self) -> &'static str {
        match self {
            Self::Configured => "is-connected",
            Self::NotConfigured => "is-idle",
            Self::Unknown => "is-degraded",
        }
    }
}

/// One tool's status, exactly as the router reported it.
///
/// Every field is optional because the upstream handlers answer with different
/// subsets: a tool that is not installed carries `installed: false`, `config:
/// null` and a `message`, while an installed one adds `has9Router` and
/// `configPath`. Absence is preserved rather than defaulted.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct ToolStatus {
    pub installed: Option<bool>,
    pub has_nine_router: Option<bool>,
    /// Raw config text the handler read, when it read one.
    pub config: Option<String>,
    /// Some handlers return parsed settings rather than raw text.
    pub settings: Option<String>,
    pub config_path: Option<String>,
    /// Handler-supplied explanation, e.g. "Codex CLI is not installed".
    pub message: Option<String>,
    /// Handler-supplied failure, when the handler answered with one.
    pub error: Option<String>,
}

impl ToolStatus {
    pub const fn detection(&self) -> Detection {
        Detection::from_flag(self.installed)
    }

    pub const fn routing(&self) -> Routing {
        Routing::from_flag(self.has_nine_router)
    }

    /// The sentence to show under the tool's name.
    ///
    /// Prefers what the router said over anything composed here; falls back to
    /// the detection's own wording when the router said nothing.
    pub fn summary(&self) -> &str {
        self.error
            .as_deref()
            .or(self.message.as_deref())
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| self.detection().detail())
    }

    /// Config text to show, preferring raw config over parsed settings.
    pub fn config_text(&self) -> Option<&str> {
        self.config
            .as_deref()
            .or(self.settings.as_deref())
            .map(str::trim)
            .filter(|text| !text.is_empty())
    }
}

/// One row of the tool list.
///
/// `status` is `None` when the batch response mapped this tool to `null` —
/// upstream's fan-out does that when a handler throws
/// (`all-statuses/route.js`). "The check failed" is not "not installed", so it
/// stays a distinct state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolEntry {
    /// Key as the router returned it, e.g. `deepseek-tui`.
    pub id: String,
    /// Display name, from the registry below or derived from the id.
    pub label: String,
    pub status: Option<ToolStatus>,
}

impl ToolEntry {
    pub fn detection(&self) -> Detection {
        self.status
            .as_ref()
            .map_or(Detection::Unknown, ToolStatus::detection)
    }

    pub fn routing(&self) -> Routing {
        self.status
            .as_ref()
            .map_or(Routing::Unknown, ToolStatus::routing)
    }

    /// The sentence under the tool name.
    pub fn summary(&self) -> &str {
        self.status.as_ref().map_or(
            "The router returned no status for this tool, so its state is unknown.",
            ToolStatus::summary,
        )
    }

    /// Route to this tool's detail page.
    pub fn detail_href(&self) -> String {
        format!("/dashboard/cli-tools/{}", self.id)
    }

    /// Accessible label for the link into the detail page.
    pub fn open_label(&self) -> String {
        format!("Configure {}", self.label)
    }
}

/// The tools the router reported, ordered for display.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ToolList {
    tools: Vec<ToolEntry>,
}

impl ToolList {
    /// Order `tools` by display name and take ownership.
    ///
    /// Detected tools are not floated to the top: the list is a stable
    /// alphabetical index, so a tool does not move when a detection changes.
    pub fn new(mut tools: Vec<ToolEntry>) -> Self {
        tools.sort_by(|left, right| {
            left.label
                .to_ascii_lowercase()
                .cmp(&right.label.to_ascii_lowercase())
                .then_with(|| left.id.cmp(&right.id))
        });
        Self { tools }
    }

    pub fn tools(&self) -> &[ToolEntry] {
        &self.tools
    }

    /// `true` when the router reported no tools at all.
    ///
    /// Rendered as the empty state. The old panel could not express this: it
    /// always had eight tiles.
    pub const fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub const fn len(&self) -> usize {
        self.tools.len()
    }

    /// How many tools the router found on this machine.
    pub fn detected_count(&self) -> usize {
        self.tools
            .iter()
            .filter(|tool| tool.detection() == Detection::Installed)
            .count()
    }

    /// How many detected tools already route through 9Router.
    pub fn routed_count(&self) -> usize {
        self.tools
            .iter()
            .filter(|tool| tool.routing() == Routing::Configured)
            .count()
    }

    /// How many tools the router could not report on.
    pub fn unknown_count(&self) -> usize {
        self.tools
            .iter()
            .filter(|tool| tool.detection() == Detection::Unknown)
            .count()
    }

    /// One tool by id.
    pub fn tool(&self, id: &str) -> Option<&ToolEntry> {
        self.tools.iter().find(|tool| tool.id == id)
    }

    /// A one-line summary of the list, for the panel's status region.
    pub fn summary(&self) -> String {
        if self.is_empty() {
            return String::from("The router reported no CLI tools.");
        }
        format!(
            "{} of {} tools detected, {} already routed through 9Router.",
            self.detected_count(),
            self.len(),
            self.routed_count()
        )
    }
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

/// A string field, accepting a non-string by rendering it compactly.
///
/// `config` is text for most handlers but a parsed object for some
/// (`opencode-settings` returns JSON). Rendering the JSON is honest — it is what
/// the router sent — whereas dropping it would hide a config that exists.
fn text_or_json(value: &Value, key: &str) -> Option<String> {
    match value.get(key) {
        None | Some(Value::Null) => None,
        Some(Value::String(found)) => Some(found.trim())
            .filter(|found| !found.is_empty())
            .map(ToOwned::to_owned),
        Some(other) => serde_json::to_string_pretty(other).ok(),
    }
}

/// A boolean field, or `None` when absent or not a boolean.
fn flag(value: &Value, key: &str) -> Option<bool> {
    value.get(key).and_then(Value::as_bool)
}

/// Parse one tool status object.
fn tool_status(value: &Value) -> Option<ToolStatus> {
    if !value.is_object() {
        return None;
    }
    Some(ToolStatus {
        installed: flag(value, "installed"),
        // Upstream serialises `has9Router`; the Rust API's `has_9_router`
        // field lands on the same wire name through serde's camelCase rename.
        has_nine_router: flag(value, "has9Router").or_else(|| flag(value, "hasNineRouter")),
        config: text_or_json(value, "config"),
        settings: text_or_json(value, "settings"),
        config_path: text(value, "configPath"),
        message: text(value, "message"),
        error: text(value, "error"),
    })
}

/// Display name for a tool id, for a route that has an id but no list.
///
/// The detail page is reachable by URL before `all-statuses` has answered, so it
/// needs a heading without waiting for one.
pub fn tool_display_name(id: &str) -> String {
    tool_label(id)
}

/// Display name for a tool id.
///
/// The table is *labels only*: it never adds a tool to the list, and a key the
/// router returns that is absent here is still shown, titled from its id. So a
/// new upstream tool appears the day the router reports it.
fn tool_label(id: &str) -> String {
    match id {
        "claude" => "Claude Code",
        "codex" => "OpenAI Codex CLI",
        "opencode" => "OpenCode",
        "droid" => "Factory Droid",
        "openclaw" => "Open Claw",
        "hermes" => "Hermes Agent",
        "cowork" => "Claude Cowork",
        "copilot" => "GitHub Copilot",
        "cline" => "Cline",
        "kilo" => "Kilo Code",
        "deepseek-tui" => "DeepSeek TUI",
        "jcode" => "jcode",
        "grok-build" => "Grok Build",
        "devin" => "Devin CLI",
        "antigravity" => "Antigravity",
        "kiro" => "Kiro",
        other => return title_from_id(other),
    }
    .to_owned()
}

/// Title-case a tool id this build has no name for.
fn title_from_id(id: &str) -> String {
    let mut label = String::with_capacity(id.len());
    for word in id.split(['-', '_']).filter(|word| !word.is_empty()) {
        if !label.is_empty() {
            label.push(' ');
        }
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            label.extend(first.to_uppercase());
            label.push_str(chars.as_str());
        }
    }
    if label.is_empty() {
        id.to_owned()
    } else {
        label
    }
}

/// Parse `GET /api/cli-tools/all-statuses`.
///
/// `None` when the body is not a JSON object, so a shape change surfaces as a
/// visible failure. An object with no keys parses to an empty list — that is a
/// meaningful answer, and [`ToolList::is_empty`] renders it as one.
pub fn parse_all_statuses(body: &str) -> Option<ToolList> {
    let value: Value = serde_json::from_str(body).ok()?;
    let map = value.as_object()?;
    let tools = map
        .iter()
        .map(|(id, status)| ToolEntry {
            label: tool_label(id),
            id: id.clone(),
            status: tool_status(status),
        })
        .collect();
    Some(ToolList::new(tools))
}

/// Parse one `GET /api/cli-tools/{tool}-settings` body.
///
/// `None` when the body is not a JSON object.
pub fn parse_tool_status(body: &str) -> Option<ToolStatus> {
    let value: Value = serde_json::from_str(body).ok()?;
    tool_status(&value)
}

/// The config a user is applying to one tool.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ToolConfigDraft {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

/// Why a draft cannot be submitted.
///
/// Mirrors the handler's own rejection (`baseUrl, apiKey and model are
/// required`), so the form explains the problem before spending a request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DraftError {
    BaseUrlMissing,
    ApiKeyMissing,
    ModelMissing,
    /// The draft could not be encoded as JSON.
    Encoding,
}

impl DraftError {
    pub const fn message(self) -> &'static str {
        match self {
            Self::BaseUrlMissing => "Enter the base URL the tool should call.",
            Self::ApiKeyMissing => "Enter the API key the tool should send.",
            Self::ModelMissing => "Enter the model the tool should request.",
            Self::Encoding => "This configuration could not be encoded as a request.",
        }
    }
}

/// The `POST` body, serialised through `serde` so a key containing a quote or
/// backslash cannot break out of the payload.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ApplyRequest<'a> {
    base_url: &'a str,
    api_key: &'a str,
    model: &'a str,
}

impl ToolConfigDraft {
    /// Validate the draft and return the body to `POST`.
    pub fn apply_body(&self) -> Result<String, DraftError> {
        let base_url = self.base_url.trim();
        if base_url.is_empty() {
            return Err(DraftError::BaseUrlMissing);
        }
        let api_key = self.api_key.trim();
        if api_key.is_empty() {
            return Err(DraftError::ApiKeyMissing);
        }
        let model = self.model.trim();
        if model.is_empty() {
            return Err(DraftError::ModelMissing);
        }
        serde_json::to_string(&ApplyRequest {
            base_url,
            api_key,
            model,
        })
        .map_err(|_error| DraftError::Encoding)
    }

    /// The blocking validation error, for disabling submit before a click.
    pub fn validation_error(&self) -> Option<DraftError> {
        self.apply_body().err()
    }
}

/// How a config write ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ApplyOutcome {
    /// The handler confirmed the write.
    Applied {
        message: String,
        config_path: Option<String>,
    },
    /// The build answered `unsupported`: nothing was written. Distinct from a
    /// failure, because no config file was touched.
    Unsupported(String),
    /// The handler refused the write and said why.
    Refused(String),
    /// The request itself did not complete.
    Rejected(ApiError),
}

impl ApplyOutcome {
    /// The sentence to show on the form.
    pub fn message(&self) -> String {
        match self {
            Self::Applied {
                message,
                config_path,
            } => config_path.as_ref().map_or_else(
                || message.clone(),
                |path| format!("{message} Written to {path}."),
            ),
            Self::Unsupported(detail) => {
                format!("Nothing was written. {detail}")
            }
            Self::Refused(detail) => format!("The router refused the change. {detail}"),
            Self::Rejected(error) => error.message().to_owned(),
        }
    }

    /// `true` only when a config file was actually written.
    pub const fn wrote_config(&self) -> bool {
        matches!(self, Self::Applied { .. })
    }

    /// Status-pill class for the outcome.
    pub const fn class_name(&self) -> &'static str {
        match self {
            Self::Applied { .. } => "is-connected",
            Self::Unsupported(_) => "is-idle",
            Self::Refused(_) | Self::Rejected(_) => "is-degraded",
        }
    }
}

/// Interpret a `POST /api/cli-tools/{tool}-settings` response.
///
/// `response` is `Ok` only for a 2xx. `nullrouter-api` answers `501` with
/// `unsupported: true` for every tool, which arrives as [`ApiError::Status`] and
/// must not be shown as "write failed" — nothing was attempted.
pub fn settle_apply(response: Result<&str, ApiError>) -> ApplyOutcome {
    let body = match response {
        Ok(body) => body,
        Err(ApiError::Status(501)) => {
            return ApplyOutcome::Unsupported(String::from(
                "This build does not write CLI tool configuration files.",
            ));
        }
        Err(error) => return ApplyOutcome::Rejected(error),
    };

    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return ApplyOutcome::Rejected(ApiError::Body);
    };
    let detail = text(&value, "message").or_else(|| text(&value, "error"));

    if flag(&value, "unsupported") == Some(true) {
        return ApplyOutcome::Unsupported(
            detail
                .unwrap_or_else(|| String::from("The router reported this write as unsupported.")),
        );
    }
    if flag(&value, "success") == Some(true) {
        return ApplyOutcome::Applied {
            message: detail.unwrap_or_else(|| String::from("Configuration applied.")),
            config_path: text(&value, "configPath"),
        };
    }
    ApplyOutcome::Refused(
        detail.unwrap_or_else(|| String::from("It did not say why the write was not applied.")),
    )
}

/// The Cowork MCP registry, as `GET /api/cli-tools/cowork-mcp-registry` reports.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct McpRegistry {
    /// Server names the registry holds.
    pub servers: Vec<String>,
    /// The registry's own count, which can exceed the names it listed.
    pub total: Option<u64>,
    pub cached: Option<bool>,
    /// `true` when this build does not implement MCP discovery.
    pub unsupported: bool,
    pub message: Option<String>,
}

impl McpRegistry {
    /// `true` when discovery worked and found nothing.
    pub const fn is_empty(&self) -> bool {
        self.servers.is_empty() && !self.unsupported
    }

    /// The sentence describing the registry's state.
    pub fn summary(&self) -> String {
        if self.unsupported {
            return self.message.clone().unwrap_or_else(|| {
                String::from("This build does not discover Cowork MCP servers.")
            });
        }
        let listed = u64::try_from(self.servers.len()).unwrap_or(u64::MAX);
        match self.total.unwrap_or(listed) {
            0 => String::from("The registry is reachable and holds no MCP servers."),
            1 => String::from("The registry holds 1 MCP server."),
            count => format!("The registry holds {count} MCP servers."),
        }
    }
}

/// Parse the MCP registry body.
///
/// `None` when the body is not a JSON object. A server entry contributes a name
/// only when it has one, so an unnamed row is not rendered as a blank server.
pub fn parse_mcp_registry(body: &str) -> Option<McpRegistry> {
    let value: Value = serde_json::from_str(body).ok()?;
    if !value.is_object() {
        return None;
    }
    let servers = value
        .get("servers")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| match entry {
                    Value::String(name) => Some(name.trim())
                        .filter(|name| !name.is_empty())
                        .map(ToOwned::to_owned),
                    other => text(other, "name").or_else(|| text(other, "id")),
                })
                .collect()
        })
        .unwrap_or_default();

    Some(McpRegistry {
        servers,
        total: value.get("total").and_then(Value::as_u64),
        cached: flag(&value, "cached"),
        unsupported: flag(&value, "unsupported") == Some(true),
        message: text(&value, "message").or_else(|| text(&value, "error")),
    })
}

// ── requests ────────────────────────────────────────────────────────────────
//
// Thin wrappers over `crate::api`, so the panel holds signals and markup only.
// `api::request` is itself split on `target_arch`: the native arm returns
// `ApiError::Environment` rather than pretending to have contacted a router,
// which keeps every function here callable — and every branch above testable —
// off the browser.

/// `GET /api/cli-tools/all-statuses`.
pub async fn load_tools() -> Result<ToolList, ApiError> {
    let body = crate::api::get(ALL_STATUSES_PATH).await?;
    parse_all_statuses(&body).ok_or(ApiError::Body)
}

/// `GET /api/cli-tools/{tool}-settings`.
pub async fn load_tool_status(tool_id: &str) -> Result<ToolStatus, ApiError> {
    let body = crate::api::get(&settings_path(tool_id)).await?;
    parse_tool_status(&body).ok_or(ApiError::Body)
}

/// `POST /api/cli-tools/{tool}-settings`.
pub async fn apply_tool_config(tool_id: &str, body: String) -> ApplyOutcome {
    let response = crate::api::post(&settings_path(tool_id), &body).await;
    settle_apply(response.as_deref().map_err(|error| *error))
}

/// `GET /api/cli-tools/cowork-mcp-registry`.
pub async fn load_mcp_registry() -> Result<McpRegistry, ApiError> {
    let body = crate::api::get(MCP_REGISTRY_PATH).await?;
    parse_mcp_registry(&body).ok_or(ApiError::Body)
}

#[cfg(test)]
mod tests {
    use super::{Detection, parse_all_statuses, settings_path, title_from_id};

    #[test]
    fn settings_suffix_is_added_once() {
        assert_eq!(settings_path("claude"), "/api/cli-tools/claude-settings");
        assert_eq!(
            settings_path("claude-settings"),
            "/api/cli-tools/claude-settings"
        );
    }

    #[test]
    fn a_tool_the_router_did_not_report_on_is_unknown_not_missing() {
        let list = parse_all_statuses(r#"{"claude":null}"#).expect("an object parses");
        let tool = list.tools().first().expect("one tool");
        assert_eq!(tool.detection(), Detection::Unknown);
    }

    #[test]
    fn unknown_ids_are_titled_from_the_id() {
        assert_eq!(title_from_id("grok-build"), "Grok Build");
        assert_eq!(title_from_id("deepseek_tui"), "Deepseek Tui");
    }
}
