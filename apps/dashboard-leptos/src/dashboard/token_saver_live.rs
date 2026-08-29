//! Pure state for the Token Saver panel: what the router reports about PXPIPE.
//!
//! PXPIPE renders bulky Claude-format context as dense PNGs, which bill by pixel
//! rather than by token. This module holds every decision the panel makes about what
//! the router said, kept free of `leptos` so each branch is unit-testable on the
//! native target; `ui/token_saver.rs` owns the signals.
//!
//! The decisions worth naming, because each is a way the panel could mislead:
//!
//! * [`Status::running`] carries no `serde` default. A missing flag is a shape change,
//!   not a "no": rendering "Stopped" for a reply that never mentioned the worker would
//!   send someone to start a worker that is already running.
//! * [`RunState::Unknown`] exists because the worker lives in the runtime service. When
//!   that service is unreachable the running state is genuinely unknown, and the panel
//!   says so rather than picking a side.
//! * [`Savings`] are labelled estimates throughout. They are computed from character
//!   counts and pixel areas, not from provider-billed usage, and a panel that presented
//!   them as billing would be lying about money.
//! * [`reason_label`] keeps the package's own refusal names. `unsupported_model` is the
//!   most likely reason a user sees nothing happen — the package images only a few
//!   model families by default — and collapsing it into "not compressed" would send
//!   them looking for a fault that is a setting.

use crate::api::ApiError;
use serde::Deserialize;

/// `GET` the install and worker state.
pub const STATUS_PATH: &str = "/api/pxpipe/status";
/// `POST` (or `GET`) the health checklist.
pub const HEALTH_PATH: &str = "/api/pxpipe/health";
/// `GET` the aggregates.
pub const STATS_PATH: &str = "/api/pxpipe/stats";
/// `GET` the install log and recent events.
pub const LOGS_PATH: &str = "/api/pxpipe/logs?limit=50";
/// `POST` an install or repair.
pub const INSTALL_PATH: &str = "/api/pxpipe/install";
/// `POST` to warm the worker.
pub const START_PATH: &str = "/api/pxpipe/start";
/// `POST` to drop it.
pub const STOP_PATH: &str = "/api/pxpipe/stop";
/// `POST` to reload it.
pub const RESTART_PATH: &str = "/api/pxpipe/restart";
/// `PUT` the settings this panel owns.
pub const SETTINGS_PATH: &str = "/api/settings";

/// The install and worker state, as `GET /api/pxpipe/status` returns it.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// Whether the package is present *and* usable.
    pub installed: bool,
    /// Whether the transform is loaded and ready.
    ///
    /// No `serde` default on purpose: absent is a shape change, not a "no".
    pub running: bool,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub uptime_ms: u64,
    /// Whether the host has an `npm` to install with.
    #[serde(default)]
    pub npm_available: bool,
    /// Whether the host has a `node` to run the transform with.
    #[serde(default)]
    pub node_available: bool,
    /// The Node the loaded worker reports.
    #[serde(default)]
    pub node_version: Option<String>,
    /// The package's declared `engines.node`.
    #[serde(default)]
    pub requires_node: Option<String>,
    /// How the transform is reached. `worker` in this build.
    #[serde(default)]
    pub mode: Option<String>,
}

/// What the panel can say about whether compression is happening.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunState {
    /// Loaded and self-tested.
    Healthy,
    /// Loaded, but the checklist has not passed.
    Running,
    /// Installed and not loaded.
    Stopped,
    /// Nothing installed.
    NotInstalled,
    /// The service holding the worker could not be reached, so this is genuinely
    /// not known. Distinct from `Stopped`, which is a claim.
    Unknown(String),
}

impl RunState {
    /// The label for a status pill.
    pub const fn label(&self) -> &str {
        match self {
            Self::Healthy => "Healthy",
            Self::Running => "Running",
            Self::Stopped => "Stopped",
            Self::NotInstalled => "Not installed",
            Self::Unknown(_) => "Unknown",
        }
    }

    /// The pill's modifier class.
    pub const fn tone(&self) -> &'static str {
        match self {
            Self::Healthy => "is-ready",
            // Running-but-unverified and unknown share a tone deliberately: both mean
            // "the panel cannot confirm compression is happening", which is what the
            // colour is for. The label beside it says which.
            Self::Running | Self::Unknown(_) => "is-degraded",
            Self::Stopped | Self::NotInstalled => "is-idle",
        }
    }
}

/// The health checklist, as `POST /api/pxpipe/health` returns it.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    #[serde(default)]
    pub healthy: bool,
    #[serde(default)]
    pub checks: Vec<HealthStep>,
    #[serde(default)]
    pub error: Option<String>,
}

/// One line of the checklist.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HealthStep {
    pub id: String,
    pub label: String,
    /// No default: a step whose outcome was not reported must not render as passing.
    pub ok: bool,
    #[serde(default)]
    pub detail: Option<String>,
}

/// The aggregates, as `GET /api/pxpipe/stats` returns them.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    #[serde(default)]
    pub windows: Windows,
    #[serde(default)]
    pub timeline: Vec<DayTotals>,
    #[serde(default)]
    pub recent: Vec<Event>,
}

/// Every window the panel offers.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Windows {
    #[serde(default)]
    pub all: Savings,
    #[serde(default)]
    pub today: Savings,
    #[serde(default)]
    pub yesterday: Savings,
    #[serde(default)]
    pub last7d: Savings,
    #[serde(default)]
    pub last30d: Savings,
}

impl Windows {
    /// The totals for one window id.
    pub const fn window(&self, id: WindowId) -> &Savings {
        match id {
            WindowId::Today => &self.today,
            WindowId::Yesterday => &self.yesterday,
            WindowId::Last7d => &self.last7d,
            WindowId::Last30d => &self.last30d,
            WindowId::All => &self.all,
        }
    }
}

/// Which window the panel is showing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WindowId {
    Today,
    Yesterday,
    /// The default, as upstream's is.
    #[default]
    Last7d,
    Last30d,
    All,
}

impl WindowId {
    /// Every window, in display order.
    pub const ALL: [Self; 5] = [
        Self::Today,
        Self::Yesterday,
        Self::Last7d,
        Self::Last30d,
        Self::All,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Today => "Today",
            Self::Yesterday => "Yesterday",
            Self::Last7d => "7 days",
            Self::Last30d => "30 days",
            Self::All => "All time",
        }
    }
}

/// One window's counters.
///
/// Every token figure here is an **estimate**: text characters over an assumed
/// characters-per-token, plus image pixels over Anthropic's pixels-per-token. The
/// panel labels them as such, because the only ground truth for what a request cost
/// is the provider-billed usage on the Usage page.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Savings {
    #[serde(default)]
    pub requests: u64,
    #[serde(default)]
    pub compressed: u64,
    /// Requests the saver declined to touch.
    #[serde(default)]
    pub bypassed: u64,
    /// Requests where the saver failed. Counted apart from a decline.
    #[serde(default)]
    pub errors: u64,
    #[serde(default)]
    pub tokens_before_est: u64,
    #[serde(default)]
    pub tokens_after_est: u64,
    #[serde(default)]
    pub tokens_saved_est: u64,
    #[serde(default)]
    pub saved_pct: f64,
    #[serde(default)]
    pub images_generated: u64,
    #[serde(default)]
    pub avg_compression_ms: u64,
}

/// One day of the timeline.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DayTotals {
    pub date: String,
    #[serde(default)]
    pub tokens_saved_est: u64,
    #[serde(default)]
    pub compressed: u64,
    #[serde(default)]
    pub requests: u64,
}

/// One recorded attempt.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    #[serde(default)]
    pub ts: u64,
    #[serde(default)]
    pub applied: bool,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub original_chars: u64,
    #[serde(default)]
    pub tokens_before_est: u64,
    #[serde(default)]
    pub tokens_after_est: u64,
    #[serde(default)]
    pub tokens_saved_est: u64,
    #[serde(default)]
    pub image_count: u64,
    #[serde(default)]
    pub duration_ms: u64,
}

impl Event {
    /// How this attempt should be shown.
    pub const fn outcome(&self) -> EventTone {
        if self.applied {
            return EventTone::Compressed;
        }
        match self.reason.as_bytes() {
            b"timeout" | b"transform_error" | b"parse_error" | b"worker_gone" | b"load_error"
            | b"node_unsupported" => EventTone::Failed,
            _ => EventTone::Bypassed,
        }
    }
}

/// The three ways an attempt reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventTone {
    Compressed,
    /// The saver declined. Not a fault.
    Bypassed,
    /// The saver broke.
    Failed,
}

impl EventTone {
    pub const fn class(self) -> &'static str {
        match self {
            Self::Compressed => "is-ready",
            Self::Bypassed => "is-idle",
            Self::Failed => "is-degraded",
        }
    }
}

/// The install log and recent events.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Logs {
    #[serde(default)]
    pub install_log: String,
    /// Worker stderr, when a worker has run.
    #[serde(default)]
    pub worker_log: Option<String>,
    #[serde(default)]
    pub events: Vec<Event>,
}

/// The settings this panel owns, read from `GET /api/settings`.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// No default: an absent flag is a shape change. A toggle rendered "off" for a
    /// reply that never carried the key asserts a stored value that may be `true`.
    pub pxpipe_enabled: bool,
    #[serde(default)]
    pub pxpipe_auto_install: bool,
    #[serde(default)]
    pub pxpipe_min_chars: u64,
    #[serde(default)]
    pub pxpipe_timeout_ms: u64,
}

/// What the router said about a control action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActionOutcome {
    /// The action was performed.
    Completed(String),
    /// The router refused, and said why. A code when it gave one.
    Refused {
        code: Option<String>,
        message: String,
    },
    /// The call itself failed.
    Failed(ApiError),
}

/// The envelope a refusal arrives in.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActionEnvelope {
    #[serde(default)]
    success: Option<bool>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    running: Option<bool>,
    #[serde(default)]
    installed: Option<bool>,
}

/// Read one control reply.
///
/// A 2xx that carries a status — `running` present — is a success even with no
/// `success` field, because that is the shape the control routes answer with. A
/// non-2xx is a refusal when it says why, and a bare failure when it does not: a
/// refusal with no message tells a user nothing and must not be shown as one.
pub fn settle_action(status: u16, body: &str) -> ActionOutcome {
    let succeeded = (200..300).contains(&status);
    let Ok(envelope) = serde_json::from_str::<ActionEnvelope>(body) else {
        return ActionOutcome::Failed(if succeeded {
            ApiError::Body
        } else {
            ApiError::Status(status)
        });
    };
    let stated = envelope
        .error
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty());

    if succeeded && envelope.success != Some(false) {
        // A status-shaped reply is the success case for start/stop/restart.
        if envelope.running.is_some() || envelope.installed.is_some() {
            return ActionOutcome::Completed(
                stated.unwrap_or_else(|| String::from("The router reported this action done.")),
            );
        }
        if envelope.success == Some(true) {
            return ActionOutcome::Completed(
                stated.unwrap_or_else(|| String::from("The router reported this action done.")),
            );
        }
    }

    match stated {
        Some(message) => ActionOutcome::Refused {
            code: envelope
                .code
                .map(|code| code.trim().to_owned())
                .filter(|code| !code.is_empty()),
            message,
        },
        None => ActionOutcome::Failed(if succeeded {
            ApiError::Body
        } else {
            ApiError::Status(status)
        }),
    }
}

/// Read a status reply, distinguishing "not running" from "not known".
///
/// A `503` from this route means the runtime service — which holds the worker — could
/// not be reached. The install state in that reply is still this service's own and
/// worth showing; the running state is not knowable and must not be guessed.
pub fn settle_status(status: u16, body: &str) -> Result<Status, StatusProblem> {
    if (200..300).contains(&status) {
        return serde_json::from_str::<Status>(body).map_err(|_| StatusProblem::Unreadable);
    }
    let partial = serde_json::from_str::<PartialStatus>(body).ok();
    Err(StatusProblem::Unknown {
        message: partial
            .as_ref()
            .and_then(|partial| partial.error.clone())
            .unwrap_or_else(|| format!("The router answered {status}.")),
        installed: partial.as_ref().is_some_and(|partial| partial.installed),
        version: partial.and_then(|partial| partial.version),
    })
}

/// What can go wrong reading a status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StatusProblem {
    /// The reply did not have the documented shape.
    Unreadable,
    /// The running state is not knowable right now.
    Unknown {
        message: String,
        installed: bool,
        version: Option<String>,
    },
}

/// The part of a failed status reply that is still worth reading.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PartialStatus {
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    installed: bool,
    #[serde(default)]
    version: Option<String>,
}

/// How the panel should describe the router's state.
pub fn run_state(
    status: Option<&Status>,
    health: Option<&Health>,
    unknown: Option<&str>,
) -> RunState {
    if let Some(message) = unknown {
        return RunState::Unknown(message.to_owned());
    }
    let Some(status) = status else {
        return RunState::Unknown(String::from("The router has not answered yet."));
    };
    if !status.installed {
        return RunState::NotInstalled;
    }
    if !status.running {
        return RunState::Stopped;
    }
    if health.is_some_and(|health| health.healthy) {
        return RunState::Healthy;
    }
    RunState::Running
}

/// Whether this host can install the package at all, and what to say when it cannot.
pub fn install_blocker(status: Option<&Status>) -> Option<&'static str> {
    let status = status?;
    if !status.npm_available {
        return Some("This host has no npm on its PATH, so the package cannot be installed here.");
    }
    if !status.node_available {
        return Some(
            "This host has no node on its PATH, so the transform cannot be run even once installed.",
        );
    }
    None
}

/// Whether the running Node meets the package's requirement.
///
/// Reported because the failure it prevents is otherwise baffling: the package
/// installs, imports, and then fails every transform on a missing runtime global.
/// Only `>=x.y.z` is read — the form `pxpipe-proxy` declares — and anything else
/// answers `None` rather than guessing.
pub fn node_shortfall(status: Option<&Status>) -> Option<String> {
    let status = status?;
    let requirement = status.requires_node.as_deref()?;
    let running = status.node_version.as_deref()?;
    let wanted = requirement.trim().strip_prefix(">=")?.trim();
    if wanted.is_empty() || !wanted.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return None;
    }
    if version_parts(running) >= version_parts(wanted) {
        return None;
    }
    Some(format!(
        "The installed package requires Node {requirement}, and this host is running Node \
         {running}. Every transform will fail until Node is upgraded."
    ))
}

fn version_parts(version: &str) -> (u64, u64, u64) {
    let mut parts = version
        .trim()
        .trim_start_matches('v')
        .split(['.', '-', '+'])
        .map(|part| part.parse::<u64>().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

/// A human sentence for one machine-readable refusal reason.
///
/// The package's own names are kept and each is given its real meaning, because the
/// difference between them is the difference between "raise your setting", "use a
/// different model", and "something is broken".
pub fn reason_label(reason: &str) -> &str {
    match reason {
        "applied" => "Compressed",
        "disabled" => "Turned off",
        "missing_body" => "Empty request",
        "unsupported_format" => "Not a Claude-format request",
        "below_threshold" => "Below this router's size threshold",
        "below_min_chars" => "Below the package's compressible-content threshold",
        "below_min_tokens" => "Below the package's token threshold",
        "not_profitable" => "Imaging would have cost more than it saved",
        "unsupported_model" => "This model is not in the package's imaging list",
        "compress_disabled" => "Compression turned off inside the package",
        "image_limit" => "Would have exceeded the provider's image limit",
        "parse_error" => "The package could not read the request",
        "passthrough" => "Left unchanged by the package",
        "timeout" => "Timed out and was abandoned",
        "transform_error" => "The transform failed",
        "not_installed" => "The package is not installed",
        "node_missing" => "No node on the host",
        "node_unsupported" => "The host's Node is too old for the package",
        "load_error" => "The package would not load",
        "not_loaded" => "The transform was not loaded",
        "worker_gone" => "The transform process stopped",
        "encode_error" => "The request could not be handed to the transform",
        other => other,
    }
}

/// `4_310` as `4.3K`, matching upstream's own formatting.
pub fn format_tokens(value: u64) -> String {
    // Integer arithmetic rather than a float divide: these are display strings, and
    // a token count large enough to lose f64 precision would render wrongly.
    if value >= 1_000_000 {
        return format!("{}.{:02}M", value / 1_000_000, value % 1_000_000 / 10_000);
    }
    if value >= 1_000 {
        return format!("{}.{}K", value / 1_000, value % 1_000 / 100);
    }
    value.to_string()
}

/// `5_400_000` ms as `1h30m`.
pub fn format_uptime(millis: u64) -> String {
    if millis == 0 {
        return String::from("—");
    }
    let minutes = millis / 60_000;
    let hours = minutes / 60;
    if hours > 0 {
        return format!("{hours}h{:02}m", minutes % 60);
    }
    format!("{minutes}m")
}

/// The JSON body that turns the saver on or off.
pub fn enabled_body(enabled: bool) -> String {
    format!("{{\"pxpipeEnabled\":{enabled}}}")
}

/// The JSON body that sets the threshold.
pub fn min_chars_body(min_chars: u64) -> String {
    format!("{{\"pxpipeMinChars\":{min_chars}}}")
}

// ── requests ────────────────────────────────────────────────────────────────────

/// `GET` a path, keeping the status code.
///
/// `crate::api::get` maps a non-2xx to [`ApiError::Status`] and discards the payload.
/// Here the payload is the point: a `503` from the status route still carries the
/// install state, and the sentence explaining why the running state is unknown.
#[cfg(target_arch = "wasm32")]
pub async fn get_with_status(path: &str) -> Result<(u16, String), ApiError> {
    use wasm_bindgen::JsCast as _;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{Request, RequestCache, RequestCredentials, RequestInit, Response};

    let init = RequestInit::new();
    init.set_method("GET");
    init.set_credentials(RequestCredentials::SameOrigin);
    init.set_cache(RequestCache::NoStore);

    let request =
        Request::new_with_str_and_init(path, &init).map_err(|_| ApiError::RequestBuild)?;
    let window = web_sys::window().ok_or(ApiError::Environment)?;
    let response = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|_| ApiError::Network)?
        .dyn_into::<Response>()
        .map_err(|_| ApiError::Body)?;
    let status = response.status();
    let text = JsFuture::from(response.text().map_err(|_| ApiError::Body)?)
        .await
        .map_err(|_| ApiError::Body)?
        .as_string()
        .ok_or(ApiError::Body)?;
    Ok((status, text))
}

/// Native builds have no browser to fetch from.
#[cfg(not(target_arch = "wasm32"))]
#[expect(
    clippy::unused_async,
    reason = "signature must match the wasm arm so callers need no cfg of their own"
)]
pub async fn get_with_status(_path: &str) -> Result<(u16, String), ApiError> {
    Err(ApiError::Environment)
}

/// `GET /api/pxpipe/status`.
pub async fn load_status() -> Result<Status, StatusProblem> {
    match get_with_status(STATUS_PATH).await {
        Ok((status, body)) => settle_status(status, &body),
        Err(error) => Err(StatusProblem::Unknown {
            message: format!("The status could not be read. {}", error.message()),
            installed: false,
            version: None,
        }),
    }
}

/// `POST /api/pxpipe/health`.
pub async fn load_health() -> Result<Health, ApiError> {
    let (_status, body) =
        crate::dashboard::headroom_live::post_with_status(HEALTH_PATH, "").await?;
    // A failing checklist is a 503 with a readable body, and it is exactly what the
    // panel needs to render, so the status is not treated as an error here.
    serde_json::from_str(&body).map_err(|_| ApiError::Body)
}

/// `GET /api/pxpipe/stats`.
pub async fn load_stats() -> Result<Stats, ApiError> {
    let body = crate::api::get(STATS_PATH).await?;
    serde_json::from_str(&body).map_err(|_| ApiError::Body)
}

/// `GET /api/pxpipe/logs`.
pub async fn load_logs() -> Result<Logs, ApiError> {
    let body = crate::api::get(LOGS_PATH).await?;
    serde_json::from_str(&body).map_err(|_| ApiError::Body)
}

/// `GET /api/settings`, for the keys this panel owns.
pub async fn load_settings() -> Result<Settings, ApiError> {
    let body = crate::api::get(SETTINGS_PATH).await?;
    serde_json::from_str(&body).map_err(|_| ApiError::Body)
}

/// `POST` one control action.
pub async fn control(path: &str) -> ActionOutcome {
    match crate::dashboard::headroom_live::post_with_status(path, "").await {
        Ok((status, body)) => settle_action(status, &body),
        Err(error) => ActionOutcome::Failed(error),
    }
}

/// `PUT /api/settings` with one key.
pub async fn save_setting(body: String) -> Result<Settings, ApiError> {
    let reply = crate::api::put(SETTINGS_PATH, &body).await?;
    serde_json::from_str(&reply).map_err(|_| ApiError::Body)
}

#[cfg(test)]
mod tests {
    use super::{
        ActionOutcome, EventTone, Health, HealthStep, RunState, Settings, Status, StatusProblem,
        WindowId, Windows, enabled_body, format_tokens, format_uptime, install_blocker,
        min_chars_body, node_shortfall, reason_label, run_state, settle_action, settle_status,
    };
    use crate::api::ApiError;
    use crate::dashboard::token_saver_live::{Event, Savings};

    fn status() -> Status {
        Status {
            installed: true,
            running: true,
            version: Some("0.13.2".to_owned()),
            path: Some("/data/pxpipe/node_modules/pxpipe-proxy".to_owned()),
            uptime_ms: 90_000,
            npm_available: true,
            node_available: true,
            node_version: Some("22.14.0".to_owned()),
            requires_node: Some(">=20.19".to_owned()),
            mode: Some("worker".to_owned()),
        }
    }

    #[test]
    fn a_status_missing_the_running_flag_fails_the_parse() {
        // "Stopped" for a reply that never mentioned the worker would send someone to
        // start a worker that is already running.
        let body = r#"{"installed":true,"version":"0.13.2"}"#;
        assert_eq!(settle_status(200, body), Err(StatusProblem::Unreadable));
    }

    #[test]
    fn a_status_reply_is_read_whole() {
        let body = serde_json::json!({
            "installed": true, "running": true, "version": "0.13.2",
            "uptimeMs": 90_000, "npmAvailable": true, "nodeAvailable": true,
            "nodeVersion": "22.14.0", "requiresNode": ">=20.19", "mode": "worker",
            "path": "/data/pxpipe/node_modules/pxpipe-proxy",
        })
        .to_string();
        assert_eq!(settle_status(200, &body), Ok(status()));
    }

    #[test]
    fn an_unreachable_runtime_is_unknown_rather_than_stopped() {
        let body = serde_json::json!({
            "success": false,
            "code": "RUNTIME_UNREACHABLE",
            "error": "The runtime service holds the transform and could not be reached",
            "installed": true,
            "version": "0.13.2",
        })
        .to_string();
        match settle_status(503, &body) {
            Err(StatusProblem::Unknown {
                message,
                installed,
                version,
            }) => {
                assert!(message.contains("could not be reached"));
                // The install state is this service's own and still worth showing.
                assert!(installed);
                assert_eq!(version.as_deref(), Some("0.13.2"));
            }
            other => panic!("expected an unknown state, got {other:?}"),
        }
    }

    #[test]
    fn the_run_state_never_guesses() {
        let healthy = Health {
            healthy: true,
            checks: vec![],
            error: None,
        };
        assert_eq!(
            run_state(Some(&status()), Some(&healthy), None),
            RunState::Healthy
        );
        // Loaded but not self-tested: running, not healthy.
        assert_eq!(run_state(Some(&status()), None, None), RunState::Running);
        assert_eq!(
            run_state(
                Some(&Status {
                    running: false,
                    ..status()
                }),
                None,
                None
            ),
            RunState::Stopped
        );
        assert_eq!(
            run_state(
                Some(&Status {
                    installed: false,
                    running: false,
                    ..status()
                }),
                None,
                None
            ),
            RunState::NotInstalled
        );
        // Nothing answered yet, and the service being down: both unknown, neither
        // rendered as stopped.
        assert!(matches!(run_state(None, None, None), RunState::Unknown(_)));
        assert!(matches!(
            run_state(Some(&status()), None, Some("service down")),
            RunState::Unknown(_)
        ));
        assert_eq!(RunState::Unknown(String::new()).label(), "Unknown");
        assert_eq!(RunState::Healthy.tone(), "is-ready");
    }

    #[test]
    fn a_control_reply_that_carries_a_status_is_a_success() {
        // start/stop/restart answer with the status rather than `success: true`.
        let body = r#"{"installed":true,"running":true,"mode":"worker"}"#;
        assert!(matches!(
            settle_action(200, body),
            ActionOutcome::Completed(_)
        ));
    }

    #[test]
    fn a_refusal_keeps_its_code_and_its_sentence() {
        let body = serde_json::json!({
            "success": false,
            "code": "NOT_INSTALLED",
            "error": "PXPIPE is not installed",
        })
        .to_string();
        assert_eq!(
            settle_action(409, &body),
            ActionOutcome::Refused {
                code: Some("NOT_INSTALLED".to_owned()),
                message: "PXPIPE is not installed".to_owned(),
            }
        );
    }

    #[test]
    fn a_reply_that_says_nothing_is_not_shown_as_either_outcome() {
        // A 2xx with no status and no success field, and a failure with no message:
        // neither tells a user anything, and showing either as done or refused would
        // invent a result.
        assert_eq!(
            settle_action(200, "{}"),
            ActionOutcome::Failed(ApiError::Body)
        );
        assert_eq!(
            settle_action(500, "{}"),
            ActionOutcome::Failed(ApiError::Status(500))
        );
        assert_eq!(
            settle_action(500, "not json"),
            ActionOutcome::Failed(ApiError::Status(500))
        );
    }

    #[test]
    fn a_host_without_npm_or_node_is_told_which_is_missing() {
        assert_eq!(install_blocker(Some(&status())), None);
        assert!(
            install_blocker(Some(&Status {
                npm_available: false,
                ..status()
            }))
            .is_some_and(|message| message.contains("npm"))
        );
        assert!(
            install_blocker(Some(&Status {
                node_available: false,
                ..status()
            }))
            .is_some_and(|message| message.contains("node"))
        );
        // Nothing known yet is not a blocker claim.
        assert_eq!(install_blocker(None), None);
    }

    #[test]
    fn an_under_version_node_is_named_as_the_cause() {
        // The real numbers: pxpipe-proxy 0.13 needs 20.19, and on 18 it installs,
        // imports, and then fails every transform on a missing global.
        let shortfall = node_shortfall(Some(&Status {
            node_version: Some("18.20.4".to_owned()),
            ..status()
        }));
        let message = shortfall.unwrap_or_default();
        assert!(message.contains("20.19"), "{message}");
        assert!(message.contains("18.20.4"), "{message}");

        // A satisfied requirement says nothing.
        assert_eq!(node_shortfall(Some(&status())), None);
        // A requirement form this does not read is not guessed at.
        assert_eq!(
            node_shortfall(Some(&Status {
                requires_node: Some("^20.19".to_owned()),
                node_version: Some("18.0.0".to_owned()),
                ..status()
            })),
            None
        );
        // Not a lexical comparison: 20.9 is below 20.19.
        assert!(
            node_shortfall(Some(&Status {
                node_version: Some("20.9.0".to_owned()),
                ..status()
            }))
            .is_some()
        );
    }

    #[test]
    fn every_refusal_reason_gets_a_sentence_that_says_what_to_do() {
        // The one that matters most: the package images only a few model families by
        // default, and this is what a user sees when theirs is not one of them.
        assert!(reason_label("unsupported_model").contains("model"));
        assert!(reason_label("below_min_chars").contains("threshold"));
        assert!(reason_label("not_profitable").contains("cost"));
        assert!(reason_label("node_unsupported").contains("Node"));
        // An unknown reason is shown as itself rather than hidden.
        assert_eq!(reason_label("something-new"), "something-new");
    }

    #[test]
    fn a_failure_is_toned_differently_from_a_decline() {
        let applied = Event {
            applied: true,
            reason: "applied".to_owned(),
            ..Event::default()
        };
        assert_eq!(applied.outcome(), EventTone::Compressed);
        for reason in ["timeout", "transform_error", "parse_error", "worker_gone"] {
            let event = Event {
                reason: reason.to_owned(),
                ..Event::default()
            };
            assert_eq!(event.outcome(), EventTone::Failed, "{reason}");
        }
        for reason in ["below_min_chars", "unsupported_model", "disabled"] {
            let event = Event {
                reason: reason.to_owned(),
                ..Event::default()
            };
            assert_eq!(event.outcome(), EventTone::Bypassed, "{reason}");
        }
        assert_eq!(EventTone::Failed.class(), "is-degraded");
    }

    #[test]
    fn the_default_window_is_the_last_week() {
        assert_eq!(WindowId::default(), WindowId::Last7d);
        assert_eq!(WindowId::ALL.len(), 5);
        let windows = Windows {
            today: Savings {
                requests: 7,
                ..Savings::default()
            },
            ..Windows::default()
        };
        assert_eq!(windows.window(WindowId::Today).requests, 7);
        assert_eq!(windows.window(WindowId::All).requests, 0);
    }

    #[test]
    fn figures_are_formatted_as_upstream_formats_them() {
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(4_310), "4.3K");
        assert_eq!(format_tokens(2_500_000), "2.50M");
        assert_eq!(format_uptime(0), "—");
        assert_eq!(format_uptime(90_000), "1m");
        assert_eq!(format_uptime(5_400_000), "1h30m");
    }

    #[test]
    fn the_settings_body_carries_only_the_key_being_changed() {
        // A full settings object would resend every other field, and a stale one at
        // that: two tabs open on this page would overwrite each other's changes.
        assert_eq!(enabled_body(true), r#"{"pxpipeEnabled":true}"#);
        assert_eq!(min_chars_body(25_000), r#"{"pxpipeMinChars":25000}"#);
    }

    #[test]
    fn settings_missing_the_enabled_key_fail_the_parse() {
        // A toggle rendered "off" for a reply with no such key asserts a stored value
        // that may well be `true`.
        assert!(serde_json::from_str::<Settings>(r#"{"pxpipeMinChars":25000}"#).is_err());
        let settings: Settings =
            serde_json::from_str(r#"{"pxpipeEnabled":true,"pxpipeMinChars":30000}"#)
                .expect("parse");
        assert!(settings.pxpipe_enabled);
        assert_eq!(settings.pxpipe_min_chars, 30_000);
    }

    #[test]
    fn a_health_step_with_no_outcome_fails_the_parse() {
        assert!(
            serde_json::from_str::<HealthStep>(r#"{"id":"module","label":"Loads"}"#).is_err(),
            "a step whose outcome was not reported must not render as passing"
        );
    }
}
