//! Live MITM state: the status the router reports, and the writes it refuses.
//!
//! The MITM panel rendered `mitm_dashboard_state()` — a `const` claiming the
//! server was stopped, the certificate absent, and DNS off for every tool. That
//! happened to match reality, but only by coincidence: nothing had asked. The
//! same page would have shown "Stopped" over a running proxy.
//!
//! `GET /api/cli-tools/antigravity-mitm` and `.../alias` are real endpoints, so
//! the status is now read. What they cannot do is change anything: the MITM proxy
//! subsystem is not ported (see the "deliberately not ported" list in
//! `crates/execute`), so
//!
//! * `POST` (start) and `DELETE` (stop) answer `501` with
//!   `{"unsupported":true,"message":…}`,
//! * `PATCH` (DNS enable/disable, trust-cert) answers `501` the same way,
//! * `PUT .../alias` answers `403`, because DNS was never enabled.
//!
//! [`WriteOutcome`] models that so the panel can leave its controls live and
//! report the router's own refusal, rather than disabling them and asserting a
//! reason of its own. A disabled button explains nothing; a button that reports
//! "not supported by nullrouter-api" explains exactly what happened.
//!
//! Kept free of `leptos` and of `fetch` so every branch is unit testable on the
//! native target.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::api::ApiError;

/// Status and control endpoint for the MITM proxy.
pub const MITM_PATH: &str = "/api/cli-tools/antigravity-mitm";

/// Model-alias endpoint.
pub const ALIAS_PATH: &str = "/api/cli-tools/antigravity-mitm/alias";

/// Stated on the panel, at all times, whatever the status says.
///
/// The controls are live and the readings are real, so without this a user could
/// reasonably conclude the feature works and something merely went wrong.
pub const SUBSYSTEM_NOTICE: &str = "The MITM proxy subsystem is not implemented in this port. Status below is read from the router; starting the proxy, trusting the certificate, switching DNS, and saving model mappings are all refused by the API.";

/// Fallback when a refusal carried no message.
const REFUSED_WITHOUT_REASON: &str = "The router refused the change without giving a reason.";

/// What the router reports about the MITM proxy.
///
/// Every field is `Option` where the endpoint could omit it, so "the router did
/// not say" and "the router said no" stay distinguishable. `running`,
/// `cert_exists`, and `cert_trusted` are required: they are the three claims the
/// status card is built on, and defaulting a missing one to `false` would assert
/// something the router never said.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MitmStatus {
    pub running: bool,
    pub pid: Option<u32>,
    pub cert_exists: bool,
    pub cert_trusted: bool,
    /// Per-tool DNS redirect state, as the router reports it.
    pub dns: BTreeMap<String, bool>,
    pub is_admin: bool,
    pub needs_sudo_password: bool,
    pub is_windows: bool,
    /// The base URL the proxy would forward to.
    pub router_base_url: Option<String>,
}

impl MitmStatus {
    /// Status-pill text for the server card.
    pub const fn status_label(&self) -> &'static str {
        if self.running { "Running" } else { "Stopped" }
    }

    /// Status-pill class, reusing the shared `is-*` vocabulary.
    pub const fn status_class(&self) -> &'static str {
        if self.running {
            "is-connected"
        } else {
            "is-idle"
        }
    }

    /// The three prerequisite checks, in the order upstream shows them.
    pub fn checks(&self) -> [MitmCheck; 3] {
        [
            MitmCheck {
                label: "Cert",
                ok: self.cert_exists,
                detail: if self.cert_exists {
                    "Local CA certificate present."
                } else {
                    "No local CA certificate has been generated."
                },
            },
            MitmCheck {
                label: "Trusted",
                ok: self.cert_trusted,
                detail: if self.cert_trusted {
                    "Certificate is trusted by this system."
                } else {
                    "Certificate is not trusted by this system."
                },
            },
            MitmCheck {
                label: "Server",
                ok: self.running,
                detail: if self.running {
                    "Proxy process is running."
                } else {
                    "Proxy process is not running."
                },
            },
        ]
    }

    /// DNS state for one tool, or `None` when the router did not report it.
    ///
    /// `None` rather than `false`: an unlisted tool is one this build's status
    /// endpoint does not track, which is not the same as one whose DNS is off.
    pub fn dns_for(&self, tool_id: &str) -> Option<bool> {
        self.dns.get(tool_id).copied()
    }

    /// How a tool's DNS state reads in the row.
    pub fn dns_label(&self, tool_id: &str) -> &'static str {
        match self.dns_for(tool_id) {
            Some(true) => "DNS on",
            Some(false) => "DNS off",
            None => "DNS not reported",
        }
    }

    /// Pill class for a tool's DNS state.
    pub fn dns_class(&self, tool_id: &str) -> &'static str {
        match self.dns_for(tool_id) {
            Some(true) => "is-connected",
            Some(false) | None => "is-idle",
        }
    }

    /// The process id as display text, naming its absence.
    pub fn pid_label(&self) -> String {
        self.pid
            .map_or_else(|| String::from("no process"), |pid| format!("pid {pid}"))
    }

    /// Whether elevation would be required, in words.
    pub const fn privilege_note(&self) -> &'static str {
        if self.is_admin {
            "Running with administrator rights."
        } else if self.needs_sudo_password {
            "Would need an elevated password to bind port 443."
        } else {
            "Not running with administrator rights."
        }
    }
}

/// One prerequisite the server card reports.
///
/// `detail` exists so the check is never conveyed by a glyph and a colour alone:
/// it is the text alternative the row exposes to assistive technology.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MitmCheck {
    pub label: &'static str,
    pub ok: bool,
    pub detail: &'static str,
}

impl MitmCheck {
    /// Accessible label, stating the value rather than implying it.
    pub fn aria_label(&self) -> String {
        format!("{}: {}", self.label, if self.ok { "yes" } else { "no" })
    }
}

/// A `bool` field, when present.
fn flag(value: &Value, key: &str) -> Option<bool> {
    value.get(key).and_then(Value::as_bool)
}

/// A `bool` field, defaulting to `false`.
fn flag_or_false(value: &Value, key: &str) -> bool {
    flag(value, key).unwrap_or(false)
}

/// A string field, when present and non-empty.
fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|found| !found.is_empty())
        .map(ToOwned::to_owned)
}

/// Parse a `GET /api/cli-tools/antigravity-mitm` body.
///
/// `None` when the body is not a JSON object or omits `running` — the field the
/// endpoint always sends. A shape change therefore surfaces as a visible failure
/// rather than a card confidently reporting "Stopped".
pub fn parse_status(body: &str) -> Option<MitmStatus> {
    let value: Value = serde_json::from_str(body).ok()?;
    let running = flag(&value, "running")?;

    let dns = value
        .get("dnsStatus")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(tool, state)| state.as_bool().map(|state| (tool.clone(), state)))
                .collect()
        })
        .unwrap_or_default();

    Some(MitmStatus {
        running,
        pid: value
            .get("pid")
            .and_then(Value::as_u64)
            .and_then(|pid| u32::try_from(pid).ok()),
        cert_exists: flag_or_false(&value, "certExists"),
        cert_trusted: flag_or_false(&value, "certTrusted"),
        dns,
        is_admin: flag_or_false(&value, "isAdmin"),
        needs_sudo_password: flag_or_false(&value, "needsSudoPassword"),
        is_windows: flag_or_false(&value, "isWin"),
        router_base_url: text(&value, "mitmRouterBaseUrl"),
    })
}

/// Parse a `GET /api/cli-tools/antigravity-mitm/alias` body.
///
/// `None` when the body has no `aliases` object. An empty object is a valid, and
/// meaningful, answer: no mapping has been saved — which is the only state this
/// build can be in, since saving is refused.
pub fn parse_aliases(body: &str) -> Option<BTreeMap<String, String>> {
    let value: Value = serde_json::from_str(body).ok()?;
    let map = value.get("aliases").and_then(Value::as_object)?;

    Some(
        map.iter()
            .filter_map(|(model, alias)| {
                alias
                    .as_str()
                    .map(|alias| (model.clone(), alias.to_owned()))
            })
            .collect(),
    )
}

/// The body for `POST /api/cli-tools/antigravity-mitm`.
///
/// `apiKey` is required by the handler — a blank one is a `400` before the `501`
/// — so the composer's value is sent as given and the router judges it.
pub fn start_body(api_key: &str, router_base_url: &str) -> String {
    let payload = serde_json::json!({
        "apiKey": api_key.trim(),
        "mitmRouterBaseUrl": router_base_url.trim(),
    });
    payload.to_string()
}

/// The body for `PUT .../alias`.
pub fn alias_body(tool_id: &str, mappings: &BTreeMap<String, String>) -> String {
    serde_json::json!({ "tool": tool_id, "mappings": mappings }).to_string()
}

/// Why DNS and certificate trust have no control in this panel.
///
/// Both are `PATCH /api/cli-tools/antigravity-mitm` actions, and the dashboard's
/// shared HTTP client has no `PATCH` verb, so this panel cannot issue the request
/// at all. That is stated rather than dressed up as a disabled toggle: the reason
/// is a missing client verb plus an unported subsystem, not a precondition the
/// user could satisfy.
pub const DNS_READ_ONLY_NOTICE: &str = "DNS redirects and certificate trust are reported here but cannot be changed from this page: both are PATCH actions, which this dashboard's HTTP client does not send, and the API refuses them in any case.";

/// A control the panel offers.
///
/// Only the three the shared client can actually send. A control for an action
/// this build cannot issue would be a button that does nothing on click, which is
/// worse than an explained absence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MitmAction {
    Start,
    Stop,
    SaveMappings,
}

impl MitmAction {
    /// Button text.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Start => "Start Server",
            Self::Stop => "Stop Server",
            Self::SaveMappings => "Save Mappings",
        }
    }

    /// What is being attempted, for the status line.
    pub const fn attempt_note(self) -> &'static str {
        match self {
            Self::Start => "Starting the MITM proxy…",
            Self::Stop => "Stopping the MITM proxy…",
            Self::SaveMappings => "Saving model mappings…",
        }
    }
}

/// How one MITM write ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WriteOutcome {
    /// The router applied the change. Unreachable in this build, and kept
    /// because the panel must not be the thing that decides it is impossible: if
    /// a later build implements the subsystem, this arm is what lets the UI
    /// report success without being rewritten.
    Applied,
    /// The API answered that the subsystem is not supported.
    ///
    /// Carries the router's own message, so the panel quotes rather than
    /// paraphrases.
    Unsupported(String),
    /// The API refused for a stated reason (a `400`, or the alias `403`).
    Refused(String),
    /// The request did not complete.
    Rejected(ApiError),
}

impl WriteOutcome {
    /// Status text for the control's live region.
    pub fn message(&self) -> String {
        match self {
            Self::Applied => String::from("The router applied the change."),
            Self::Unsupported(detail) | Self::Refused(detail) => detail.clone(),
            Self::Rejected(error) => error.message().to_owned(),
        }
    }

    /// Pill class for the outcome.
    pub const fn class_name(&self) -> &'static str {
        match self {
            Self::Applied => "is-connected",
            Self::Unsupported(_) | Self::Refused(_) | Self::Rejected(_) => "is-degraded",
        }
    }

    /// `true` when nothing changed on the router.
    ///
    /// Every arm but `Applied`: the panel uses this to keep the status card's
    /// readings as they were, since a refused write means the previous reading is
    /// still current.
    pub const fn left_state_unchanged(&self) -> bool {
        !matches!(self, Self::Applied)
    }
}

/// Interpret a MITM write response.
///
/// `response` is `Ok` only for a 2xx, so the `501` and `403` this build always
/// returns arrive as [`ApiError::Status`]. The body is unavailable in that case —
/// `api::request` discards it on a non-2xx — so the message is reconstructed from
/// the status, which is the one thing that is certain about it.
pub fn settle_write(response: Result<&str, ApiError>) -> WriteOutcome {
    match response {
        Ok(body) => {
            let value = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
            // A 2xx that still says `unsupported`, which is how a relayed
            // envelope can arrive.
            if flag_or_false(&value, "unsupported") {
                return WriteOutcome::Unsupported(
                    text(&value, "message").unwrap_or_else(|| SUBSYSTEM_NOTICE.to_owned()),
                );
            }
            if let Some(error) = text(&value, "error") {
                return WriteOutcome::Refused(error);
            }
            WriteOutcome::Applied
        }
        Err(ApiError::Status(501)) => WriteOutcome::Unsupported(String::from(
            "Not supported by this build: the MITM proxy subsystem is not ported, so nothing was started or changed.",
        )),
        Err(ApiError::Status(403)) => WriteOutcome::Refused(String::from(
            "Refused: DNS must be enabled for this tool before model mappings can be saved, and DNS control is not available in this build.",
        )),
        Err(ApiError::Status(400)) => WriteOutcome::Refused(String::from(
            "Refused: the router rejected the request as incomplete. Check the API key and base URL.",
        )),
        Err(ApiError::Status(405)) => WriteOutcome::Refused(String::from(REFUSED_WITHOUT_REASON)),
        Err(error) => WriteOutcome::Rejected(error),
    }
}

// ── requests ────────────────────────────────────────────────────────────────

/// `GET /api/cli-tools/antigravity-mitm`.
pub async fn load_status() -> Result<MitmStatus, ApiError> {
    let body = crate::api::get(MITM_PATH).await?;
    parse_status(&body).ok_or(ApiError::Body)
}

/// `GET /api/cli-tools/antigravity-mitm/alias`.
pub async fn load_aliases() -> Result<BTreeMap<String, String>, ApiError> {
    let body = crate::api::get(ALIAS_PATH).await?;
    parse_aliases(&body).ok_or(ApiError::Body)
}

/// `POST /api/cli-tools/antigravity-mitm`.
pub async fn start_server(api_key: String, router_base_url: String) -> WriteOutcome {
    let body = start_body(&api_key, &router_base_url);
    let response = crate::api::post(MITM_PATH, &body).await;
    settle_write(response.as_deref().map_err(|error| *error))
}

/// `DELETE /api/cli-tools/antigravity-mitm`.
pub async fn stop_server() -> WriteOutcome {
    let response = crate::api::delete(MITM_PATH).await;
    settle_write(response.as_deref().map_err(|error| *error))
}

/// `PUT /api/cli-tools/antigravity-mitm/alias`.
pub async fn save_aliases(tool_id: &str, mappings: BTreeMap<String, String>) -> WriteOutcome {
    let body = alias_body(tool_id, &mappings);
    let response = crate::api::put(ALIAS_PATH, &body).await;
    settle_write(response.as_deref().map_err(|error| *error))
}
