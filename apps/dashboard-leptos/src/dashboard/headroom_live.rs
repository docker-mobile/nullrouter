//! Pure state for the Headroom panel: what the host holds, and what this build
//! refuses to change.
//!
//! Headroom is an external Python subsystem. `nullrouter-api` detects it for
//! real — which interpreter answered, which compression extras are installed —
//! but refuses to install extras or restart the proxy, because it does not own
//! that environment and has no supervisor for a detached daemon.
//!
//! That split is the whole reason this module exists as its own layer. A panel
//! that rendered a plain button for a refused action would look live and do
//! nothing; worse, a panel that reported an install as done would tell someone
//! their prompts are being compressed while every request is billed at full
//! size. So:
//!
//! * [`ExtrasReport`] can only describe what `GET /api/headroom/extras`
//!   returned. `installed` carries no `serde` default — a missing flag is a
//!   shape change, not a "no".
//! * [`ExtraRow::installed`] is an `Option`: an extra the router listed but did
//!   not report a state for renders as "state not reported", never as "off".
//! * [`InstallSupport`] and [`ActionSupport`] are read from the report *before*
//!   any control is drawn, so an unsupported action renders as an explanation
//!   rather than as a button.
//! * [`settle_action`] maps a `501` to [`ActionOutcome::Refused`] and only ever
//!   reports [`ActionOutcome::Completed`] for a 2xx that said `success: true`.
//!
//! Kept free of `leptos` so every branch above is unit-testable on the native
//! target; `ui/headroom.rs` owns the signals.

use crate::api::ApiError;
use serde::Deserialize;
use std::collections::BTreeMap;

/// `GET` the extras report, `POST` an install request.
pub const EXTRAS_PATH: &str = "/api/headroom/extras";

/// `GET` the install log tail. Upstream's UI polls this for progress.
pub const EXTRAS_LOG_PATH: &str = "/api/headroom/extras?log=1";

/// `POST` a proxy restart.
pub const RESTART_PATH: &str = "/api/headroom/restart";

/// The minimum Python this subsystem needs, stated when the router does not.
const FALLBACK_MIN_PYTHON: &str = "3.10";

/// The extras report, as `GET /api/headroom/extras` returns it.
///
/// `available`, `installed`, `version`, and `extras` are upstream's own shape.
/// The rest is what `nullrouter-api` adds so this panel can be honest without a
/// second request.
///
/// `available` and `installed` carry no `serde` default. They are the identity
/// of the report: an absent `available` would render as "this build offers no
/// extras", and an absent `installed` defaulted to `false` would claim
/// `headroom-ai` is missing — neither is something the router said. An absent
/// one is a shape change, so it fails the parse and surfaces as an error.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExtrasReport {
    /// Compression extras this build tracks, in the order to display them.
    pub available: Vec<String>,
    /// Whether the interpreter holds `headroom-ai` at all.
    pub installed: bool,
    /// Installed `headroom-ai` version. `None` when nothing is installed.
    #[serde(default)]
    pub version: Option<String>,
    /// Per-extra installed state, keyed as `available` names them.
    pub extras: BTreeMap<String, bool>,
    /// Path of the interpreter that answered, or `None` when none did.
    #[serde(default)]
    pub python: Option<String>,
    #[serde(default)]
    pub python_version: Option<String>,
    #[serde(default)]
    pub python_min_version: Option<String>,
    /// Whether this build performs installs. Absent means no: a missing
    /// capability flag must never enable a control that mutates a host.
    #[serde(default)]
    pub install_supported: bool,
    #[serde(default)]
    pub install_message: Option<String>,
    #[serde(default)]
    pub restart_supported: bool,
    #[serde(default)]
    pub restart_message: Option<String>,
}

impl ExtrasReport {
    /// Whether a suitable interpreter was found, and what to say about it.
    pub fn python(&self) -> PythonStatus {
        match self
            .python
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())
        {
            Some(path) => PythonStatus::Detected {
                path: path.to_owned(),
                version: self
                    .python_version
                    .as_deref()
                    .map(str::trim)
                    .filter(|version| !version.is_empty())
                    .map(str::to_owned),
            },
            None => PythonStatus::Missing {
                minimum: self.min_python().to_owned(),
            },
        }
    }

    /// The minimum Python version to quote to the user.
    pub fn min_python(&self) -> &str {
        self.python_min_version
            .as_deref()
            .map(str::trim)
            .filter(|version| !version.is_empty())
            .unwrap_or(FALLBACK_MIN_PYTHON)
    }

    /// Installed `headroom-ai` version as display text.
    ///
    /// Names the absence rather than rendering an empty string, so a row never
    /// reads as a version of "".
    pub fn version_label(&self) -> String {
        self.version
            .as_deref()
            .map(str::trim)
            .filter(|version| !version.is_empty())
            .map_or_else(
                || {
                    if self.installed {
                        String::from("version not reported")
                    } else {
                        String::from("not installed")
                    }
                },
                str::to_owned,
            )
    }

    /// One row per extra, in the order the router listed them.
    ///
    /// Any extra the router reported a state for but did not list in `available`
    /// is appended, so nothing the server said is hidden by this panel's idea of
    /// what exists.
    pub fn rows(&self) -> Vec<ExtraRow> {
        let mut rows: Vec<ExtraRow> = self
            .available
            .iter()
            .map(|name| ExtraRow::new(name, self.extras.get(name).copied()))
            .collect();
        for (name, installed) in &self.extras {
            if !self.available.iter().any(|listed| listed == name) {
                rows.push(ExtraRow::new(name, Some(*installed)));
            }
        }
        rows
    }

    /// How many extras are installed, out of how many exist.
    ///
    /// Only rows with a reported state are counted, so an unreported extra does
    /// not silently become part of the "off" tally.
    pub fn installed_count(&self) -> (usize, usize) {
        let rows = self.rows();
        let installed = rows
            .iter()
            .filter(|row| row.installed == Some(true))
            .count();
        (installed, rows.len())
    }

    /// The `pip` command that would install the extras this host is missing.
    ///
    /// This exists because the install action is refused: without it the panel
    /// could only say "not supported" and leave the user with nowhere to go. It
    /// is guidance, not a state claim — it says what to run, and nothing about
    /// what is installed.
    ///
    /// The interpreter is the one the router reported, so the command targets the
    /// same environment the extras were read from. When none was detected there
    /// is no interpreter to name, and `python3` is the honest placeholder: the
    /// user has to install Python first anyway.
    ///
    /// The requirement is single-quoted because `[` and `]` are glob characters
    /// in every shell this dashboard is likely to be read in.
    pub fn manual_install_command(&self) -> String {
        let mut extras: Vec<String> = vec![String::from("proxy")];
        extras.extend(
            self.rows()
                .into_iter()
                .filter(|row| row.installed != Some(true))
                .map(|row| row.name),
        );
        let python = self
            .python
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .unwrap_or("python3");
        format!(
            "{python} -m pip install --upgrade 'headroom-ai[{}]'",
            extras.join(",")
        )
    }

    /// Whether installing extras is possible here, and why not when it is not.
    pub fn install_support(&self) -> ActionSupport {
        ActionSupport::from_parts(
            self.install_supported,
            self.install_message.as_deref(),
            "This build does not install headroom extras. Nothing was installed.",
        )
    }

    /// Whether restarting the proxy is possible here, and why not when it is not.
    pub fn restart_support(&self) -> ActionSupport {
        ActionSupport::from_parts(
            self.restart_supported,
            self.restart_message.as_deref(),
            "This build does not restart the headroom proxy. Nothing was restarted.",
        )
    }
}

/// Whether an interpreter was found, and what the panel should say.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PythonStatus {
    Detected {
        path: String,
        /// `Some("3.12")` when the router reported it.
        version: Option<String>,
    },
    Missing {
        minimum: String,
    },
}

impl PythonStatus {
    pub const fn is_detected(&self) -> bool {
        matches!(self, Self::Detected { .. })
    }

    /// Headline text. Carries the state in words, not in colour.
    pub fn label(&self) -> String {
        match self {
            Self::Detected {
                version: Some(version),
                ..
            } => format!("Python {version} detected"),
            Self::Detected { version: None, .. } => String::from("Python detected"),
            Self::Missing { minimum } => format!("No Python {minimum} or newer found"),
        }
    }

    /// Supporting line: where it is, or what to do about its absence.
    pub fn detail(&self) -> String {
        match self {
            Self::Detected { path, .. } => path.clone(),
            Self::Missing { minimum } => format!(
                "Headroom needs Python {minimum} or newer on this machine. Install it, then reload this panel."
            ),
        }
    }
}

/// One compression extra, as a row the panel can draw.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtraRow {
    /// Wire name, e.g. `code`.
    pub name: String,
    /// Installed state, or `None` when the router listed the extra without
    /// reporting one.
    pub installed: Option<bool>,
}

impl ExtraRow {
    fn new(name: &str, installed: Option<bool>) -> Self {
        Self {
            name: name.to_owned(),
            installed,
        }
    }

    /// Human name for the known extras, falling back to the wire name.
    pub fn label(&self) -> String {
        match self.name.as_str() {
            "code" => String::from("Code-aware compression"),
            "ml" => String::from("Kompress model"),
            other => other.to_owned(),
        }
    }

    /// What this extra buys, for the extras this build knows.
    ///
    /// An extra added upstream and not yet described here says so, rather than
    /// borrowing another extra's description.
    pub fn description(&self) -> String {
        match self.name.as_str() {
            "code" => String::from(
                "Tree-sitter AST compression for source files. Adds tree-sitter and tree-sitter-language-pack.",
            ),
            "ml" => String::from(
                "Kompress-v2 model-based compression. Adds torch and huggingface-hub, a multi-gigabyte download.",
            ),
            other => format!(
                "Reported by the router as `{other}`. This build has no description for it."
            ),
        }
    }

    /// Installed state as text.
    ///
    /// The state must be readable without colour, so every row carries this
    /// string and no row relies on a swatch to mean "on".
    pub const fn installed_label(&self) -> &'static str {
        match self.installed {
            Some(true) => "Installed",
            Some(false) => "Not installed",
            None => "State not reported",
        }
    }

    /// DOM id suffix for this row's controls.
    pub fn dom_suffix(&self) -> String {
        self.name
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                    ch
                } else {
                    '-'
                }
            })
            .collect()
    }
}

/// Whether one mutating action is available, and the reason when it is not.
///
/// The reason comes from the router where it gave one, so the panel repeats the
/// service's own explanation instead of inventing a parallel one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActionSupport {
    Supported,
    Unsupported { reason: String },
}

impl ActionSupport {
    fn from_parts(supported: bool, message: Option<&str>, fallback: &str) -> Self {
        if supported {
            return Self::Supported;
        }
        let reason = message
            .map(str::trim)
            .filter(|message| !message.is_empty())
            .unwrap_or(fallback);
        Self::Unsupported {
            reason: reason.to_owned(),
        }
    }

    pub const fn is_supported(&self) -> bool {
        matches!(self, Self::Supported)
    }

    /// The refusal text, or `None` when the action is available.
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Supported => None,
            Self::Unsupported { reason } => Some(reason.as_str()),
        }
    }
}

/// The `{"log":"...","logPath":...}` body from `?log=1`.
///
/// `log` carries no default: an absent field is a shape change, and defaulting
/// it to `""` would render as "the install log is empty" — a claim the router
/// never made.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LogTail {
    pub log: String,
    /// Where the router read the tail from, when a file exists.
    #[serde(default)]
    pub log_path: Option<String>,
}

impl LogTail {
    /// The tail split into lines, oldest first, blank lines dropped.
    pub fn lines(&self) -> Vec<&str> {
        self.log
            .lines()
            .map(str::trim_end)
            .filter(|line| !line.is_empty())
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.lines().is_empty()
    }

    /// What to show in place of an empty log.
    ///
    /// Distinguishes "the router found no log file" from "the file is there and
    /// has nothing in it", because only the first tells the user the install
    /// history lives somewhere else.
    pub fn placeholder(&self) -> String {
        match self
            .log_path
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())
        {
            Some(path) => format!("No install output logged yet in {path}."),
            None => String::from(
                "No headroom install log on this machine. This build never writes one, so nothing appears here until an install runs elsewhere.",
            ),
        }
    }
}

/// How a mutating request ended.
///
/// [`Refused`](Self::Refused) is a first-class outcome, not a failure: the
/// router understood the request and declined it. Rendering it as an error would
/// suggest retrying might work.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActionOutcome {
    /// The router declined, and said why.
    Refused(Refusal),
    /// The router reported the action done. This build's API never answers this,
    /// but a client must not mislabel a genuine success if one ever arrives.
    Completed { message: String },
    /// The request itself did not produce a usable answer.
    Failed(ApiError),
}

impl ActionOutcome {
    /// Status text for the panel.
    pub fn message(&self) -> String {
        match self {
            Self::Refused(refusal) => refusal.message.clone(),
            Self::Completed { message } => message.clone(),
            Self::Failed(error) => error.message().to_owned(),
        }
    }

    /// Whether anything on the host actually changed.
    ///
    /// The one question the panel must never get wrong.
    pub const fn changed_the_host(&self) -> bool {
        matches!(self, Self::Completed { .. })
    }
}

/// A refusal, with everything the user needs to act on it themselves.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Refusal {
    /// Machine-readable reason, e.g. `UNSUPPORTED` or `EXTERNAL_PROXY`.
    pub code: Option<String>,
    pub message: String,
    /// The pip requirement that was not installed, when the router named one.
    pub spec: Option<String>,
    /// Requested names this build does not recognise.
    pub ignored: Vec<String>,
}

/// The refusal/success envelope both mutating endpoints share.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActionEnvelope {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    spec: Option<String>,
    #[serde(default)]
    ignored: Vec<String>,
}

/// Interpret a mutating response.
///
/// `Completed` requires both a 2xx **and** `success: true`. Everything else is a
/// refusal when the body explains itself, and a failure when it does not. That
/// asymmetry is deliberate: mislabelling a refusal as success is the one error
/// that costs the user money, so success has to be stated twice.
pub fn settle_action(status: u16, body: &str) -> ActionOutcome {
    let succeeded = (200..300).contains(&status);
    let Ok(envelope) = serde_json::from_str::<ActionEnvelope>(body) else {
        // No readable body: fall back to the status, never to an assumption
        // about what happened on the host.
        return ActionOutcome::Failed(if succeeded {
            ApiError::Body
        } else {
            ApiError::Status(status)
        });
    };

    let stated = envelope
        .error
        .or(envelope.message)
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty());

    if succeeded && envelope.success {
        return ActionOutcome::Completed {
            message: stated
                .unwrap_or_else(|| String::from("The router reported this action done.")),
        };
    }

    match stated {
        Some(message) => ActionOutcome::Refused(Refusal {
            code: envelope
                .code
                .map(|code| code.trim().to_owned())
                .filter(|code| !code.is_empty()),
            message,
            spec: envelope
                .spec
                .map(|spec| spec.trim().to_owned())
                .filter(|spec| !spec.is_empty()),
            ignored: envelope.ignored,
        }),
        // A 2xx that says neither success nor why not tells the user nothing,
        // and must not be shown as either outcome.
        None => ActionOutcome::Failed(if succeeded {
            ApiError::Body
        } else {
            ApiError::Status(status)
        }),
    }
}

/// Parse the extras report.
///
/// `None` on any body that is not a well-formed report, so the panel shows a
/// failure rather than an empty extras list that reads as "no extras exist".
pub fn parse_report(body: &str) -> Option<ExtrasReport> {
    serde_json::from_str::<ExtrasReport>(body).ok()
}

/// Parse the install log tail.
pub fn parse_log(body: &str) -> Option<LogTail> {
    serde_json::from_str::<LogTail>(body).ok()
}

/// The `POST` body that asks for these extras.
///
/// Only the names given are sent; the router applies its own whitelist and
/// reports back anything it did not recognise.
pub fn install_body(extras: &[String]) -> String {
    serde_json::json!({ "extras": extras }).to_string()
}

// ── requests ────────────────────────────────────────────────────────────────
//
// Thin wrappers whose results are already interpreted above, so a caller cannot
// forget that a `501` is a refusal rather than a failure.
//
// The mutating pair goes through [`post_with_status`] rather than
// `crate::api::post`, because that helper treats a non-2xx as an error and drops
// the body — and the body is exactly where the router explains what it declined
// to do and what the user can run instead.

/// `GET /api/headroom/extras`.
pub async fn load_report() -> Result<ExtrasReport, ApiError> {
    let body = crate::api::get(EXTRAS_PATH).await?;
    parse_report(&body).ok_or(ApiError::Body)
}

/// `GET /api/headroom/extras?log=1`.
pub async fn load_log() -> Result<LogTail, ApiError> {
    let body = crate::api::get(EXTRAS_LOG_PATH).await?;
    parse_log(&body).ok_or(ApiError::Body)
}

/// `POST /api/headroom/extras`.
pub async fn install_extras(extras: Vec<String>) -> ActionOutcome {
    match post_with_status(EXTRAS_PATH, &install_body(&extras)).await {
        Ok((status, body)) => settle_action(status, &body),
        Err(error) => ActionOutcome::Failed(error),
    }
}

/// `POST /api/headroom/restart`.
pub async fn restart_proxy() -> ActionOutcome {
    match post_with_status(RESTART_PATH, "{}").await {
        Ok((status, body)) => settle_action(status, &body),
        Err(error) => ActionOutcome::Failed(error),
    }
}

/// `POST` a JSON body and keep both the status and the body.
///
/// `crate::api::post` maps a non-2xx to [`ApiError::Status`] and discards the
/// payload. Here the payload is the point: a refusal carries the code, the
/// reason, and the requirement string the user can run themselves.
#[cfg(target_arch = "wasm32")]
pub async fn post_with_status(path: &str, body: &str) -> Result<(u16, String), ApiError> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{Request, RequestCache, RequestCredentials, RequestInit, Response};

    let init = RequestInit::new();
    init.set_method("POST");
    init.set_credentials(RequestCredentials::SameOrigin);
    init.set_cache(RequestCache::NoStore);
    init.set_body(&wasm_bindgen::JsValue::from_str(body));

    let request =
        Request::new_with_str_and_init(path, &init).map_err(|_| ApiError::RequestBuild)?;
    request
        .headers()
        .set("content-type", "application/json")
        .map_err(|_| ApiError::RequestBuild)?;

    let window = web_sys::window().ok_or(ApiError::Environment)?;
    let response = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|_| ApiError::Network)?
        .dyn_into::<Response>()
        .map_err(|_| ApiError::Body)?;

    let status = response.status();
    let text = JsFuture::from(response.text().map_err(|_| ApiError::Body)?)
        .await
        .map_err(|_| ApiError::Body)?;
    text.as_string()
        .map(|body| (status, body))
        .ok_or(ApiError::Body)
}

/// Native builds have no browser to fetch from.
///
/// The native target exists so every branch above stays unit-testable; a request
/// there is a programming error, reported rather than faked. In particular it
/// must not return a synthetic 2xx, which [`settle_action`] would read as a
/// completed install.
#[cfg(not(target_arch = "wasm32"))]
#[expect(
    clippy::unused_async,
    reason = "signature must match the wasm arm so callers need no cfg of their own"
)]
pub async fn post_with_status(_path: &str, _body: &str) -> Result<(u16, String), ApiError> {
    Err(ApiError::Environment)
}
