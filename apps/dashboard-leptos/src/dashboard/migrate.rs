//! Parsing and gating for the 9Router import panel.
//!
//! The panel writes to a user's real provider store, so two things must hold no
//! matter what the server sends back:
//!
//! 1. Nothing is rendered as a count unless the server produced that count.
//!    Every field of [`ImportReport`] is required; a shape change upstream
//!    surfaces as a visible failure rather than a table of zeros that reads as
//!    "you have nothing to import".
//! 2. An import cannot run before a preview has succeeded. That rule lives in
//!    [`import_gate`] rather than in the view, so it is testable on the native
//!    target and cannot be lost in a refactor of the markup.
//!
//! `web_sys` only exists on wasm, so the fetch call is cfg-gated exactly as
//! `crate::api` does it and the native arm reports the absence instead of
//! pretending a request happened.

use serde::Deserialize;

use crate::api::ApiError;

/// The endpoint this panel drives.
pub const MIGRATE_PATH: &str = "/api/migrate/9router";

/// What an import found and applied.
///
/// Mirrors `nullrouter-state`'s `ImportReport`. Deliberately has no `Default`:
/// an all-zero report must only ever exist because the server sent one.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    /// Absolute path the data was read from.
    pub source: String,
    /// Either `sqlite` or `json`.
    pub format: String,
    pub connections_found: usize,
    pub connections_imported: usize,
    pub combos_found: usize,
    pub combos_imported: usize,
    pub proxy_pools_found: usize,
    pub proxy_pools_imported: usize,
    pub api_keys_found: usize,
    /// Always `0`: nullrouter stores key digests, so a plaintext 9Router key
    /// cannot be turned back into a usable record.
    pub api_keys_imported: usize,
    pub settings_imported: bool,
    /// Per-record problems that did not abort the import.
    #[serde(default)]
    pub warnings: Vec<String>,
}

impl ImportReport {
    /// Per-kind rows for the preview table, in display order.
    pub const fn rows(&self) -> [ReportRow; 4] {
        [
            ReportRow {
                label: "Provider connections",
                found: self.connections_found,
                importable: self.connections_imported,
                note: None,
            },
            ReportRow {
                label: "Combos",
                found: self.combos_found,
                importable: self.combos_imported,
                note: None,
            },
            ReportRow {
                label: "Proxy pools",
                found: self.proxy_pools_found,
                importable: self.proxy_pools_imported,
                note: None,
            },
            ReportRow {
                label: "API keys",
                found: self.api_keys_found,
                importable: self.api_keys_imported,
                note: Some("Cannot be imported — re-issue them here"),
            },
        ]
    }

    /// How many records this import would write, settings counted as one.
    ///
    /// API keys are excluded because the server never imports them, so a store
    /// holding only keys has nothing to write and the import stays gated.
    pub const fn pending_writes(&self) -> usize {
        self.connections_imported
            + self.combos_imported
            + self.proxy_pools_imported
            + if self.settings_imported { 1 } else { 0 }
    }

    /// `true` when the source held nothing at all.
    pub const fn found_nothing(&self) -> bool {
        self.connections_found == 0
            && self.combos_found == 0
            && self.proxy_pools_found == 0
            && self.api_keys_found == 0
    }
}

/// One row of the found/importable table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReportRow {
    pub label: &'static str,
    pub found: usize,
    pub importable: usize,
    /// Why this kind is not fully importable, when it is not.
    pub note: Option<&'static str>,
}

impl ReportRow {
    /// Records present at the source that this import will not write.
    pub const fn skipped(&self) -> usize {
        self.found.saturating_sub(self.importable)
    }
}

/// No 9Router installation was found.
///
/// Not an error state in the UI: it is the expected answer for anyone who never
/// ran 9Router, or who keeps it outside the default location.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MissingInstall {
    /// The server's message, rendered verbatim.
    pub message: String,
    /// Directories the server probed, split out of `message` for listing.
    pub searched: Vec<String>,
}

/// A request the server declined to serve.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Rejected {
    /// Machine-readable code, e.g. `state_unavailable`.
    pub error: String,
    /// Human-readable detail from the server, rendered verbatim.
    pub message: String,
}

/// What one call to the migrate endpoint produced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// The server read a 9Router installation.
    Completed { dry_run: bool, report: ImportReport },
    /// Nothing was found to import.
    Missing(MissingInstall),
    /// The server refused, e.g. because the state service is down.
    Refused(Rejected),
}

impl Outcome {
    /// The report, when one was produced.
    pub const fn report(&self) -> Option<&ImportReport> {
        match self {
            Self::Completed { report, .. } => Some(report),
            Self::Missing(_) | Self::Refused(_) => None,
        }
    }

    /// `true` when this came from a dry run rather than a write.
    pub const fn is_dry_run(&self) -> bool {
        matches!(self, Self::Completed { dry_run: true, .. })
    }
}

/// The envelope every response shares.
///
/// `ok` is optional so a body that is merely well-formed JSON — `{}`, or an
/// unrelated object — is rejected as a decode failure instead of being read as
/// a refusal with empty text.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Envelope {
    ok: Option<bool>,
    #[serde(default)]
    dry_run: bool,
    #[serde(default)]
    report: Option<ImportReport>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

/// Interpret a response body against its status code.
///
/// The status is needed because 404 carries the searched-paths message in its
/// body — the shared `api::request` helper discards bodies on non-2xx, which is
/// why this panel posts through [`post_with_status`] instead.
pub fn parse_response(status: u16, body: &str) -> Result<Outcome, ApiError> {
    let Ok(envelope) = serde_json::from_str::<Envelope>(body) else {
        // A body we cannot read is a decode failure, unless the status already
        // told us what went wrong.
        return Err(if (200..300).contains(&status) {
            ApiError::Body
        } else {
            ApiError::Status(status)
        });
    };

    match envelope.ok {
        Some(true) => envelope.report.map_or(Err(ApiError::Body), |report| {
            Ok(Outcome::Completed {
                dry_run: envelope.dry_run,
                report,
            })
        }),
        Some(false) => {
            let message = envelope.message.unwrap_or_default();
            let error = envelope.error.unwrap_or_default();
            if error == "no_9router_installation" || status == 404 {
                Ok(Outcome::Missing(MissingInstall {
                    searched: searched_paths(&message),
                    message,
                }))
            } else if error.is_empty() && message.is_empty() {
                // `{"ok":false}` alone says nothing a user could act on.
                Err(ApiError::Body)
            } else {
                Ok(Outcome::Refused(Rejected { error, message }))
            }
        }
        // No `ok` field: not this endpoint's envelope.
        None => Err(if (200..300).contains(&status) {
            ApiError::Body
        } else {
            ApiError::Status(status)
        }),
    }
}

/// Pull the probed directories out of a not-found message.
///
/// The server formats them as `... Searched: <a>, <b>`. A path containing a
/// comma would split wrongly, which is why the full message is always rendered
/// too — this list is a convenience, not the source of truth.
fn searched_paths(message: &str) -> Vec<String> {
    message
        .split_once("Searched:")
        .map(|(_, tail)| {
            tail.split(',')
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Whether the Import button may be pressed, and why not when it may not.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImportGate {
    /// A request is in flight.
    Busy,
    /// No successful preview has run yet.
    NeedsPreview,
    /// A preview ran and found nothing to write.
    NothingToImport,
    /// An import already ran against this preview; re-scan for a fresh one.
    AlreadyImported,
    /// A preview succeeded and found records to write.
    Ready,
}

impl ImportGate {
    /// `true` only when an import may start.
    pub const fn allows_import(self) -> bool {
        matches!(self, Self::Ready)
    }

    /// Why the button is disabled, for its `title` and the status region.
    pub const fn blocked_reason(self) -> Option<&'static str> {
        match self {
            Self::Busy => Some("A request is already running."),
            Self::NeedsPreview => {
                Some("Run a preview first, so you can see what would be written.")
            }
            Self::NothingToImport => Some("The preview found nothing that would be written."),
            Self::AlreadyImported => {
                Some("This preview has been imported. Re-scan to import again.")
            }
            Self::Ready => None,
        }
    }
}

/// Decide whether an import may run.
///
/// `preview` is the last dry-run result, `in_flight` whether any request is
/// open, and `imported` whether an import already consumed this preview. A
/// failed, missing, or refused preview never opens the gate: the only way in is
/// a dry run that reported writes.
pub const fn import_gate(preview: Option<&Outcome>, in_flight: bool, imported: bool) -> ImportGate {
    if in_flight {
        return ImportGate::Busy;
    }
    match preview {
        Some(Outcome::Completed { report, .. }) => {
            if report.pending_writes() == 0 {
                ImportGate::NothingToImport
            } else if imported {
                ImportGate::AlreadyImported
            } else {
                ImportGate::Ready
            }
        }
        Some(Outcome::Missing(_) | Outcome::Refused(_)) | None => ImportGate::NeedsPreview,
    }
}

/// The JSON body for one migrate call.
///
/// A blank directory is sent as `null` so the server runs its own discovery
/// rather than probing the empty path.
pub fn request_body(data_dir: &str, dry_run: bool) -> String {
    let trimmed = data_dir.trim();
    let dir = if trimmed.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::Value::String(trimmed.to_owned())
    };
    serde_json::json!({ "dataDir": dir, "dryRun": dry_run }).to_string()
}

/// One line describing what the panel is doing, for `aria-live`.
pub fn status_line(in_flight: Option<Phase>, preview: Option<&Outcome>) -> String {
    match in_flight {
        Some(Phase::Scan) => "Scanning for a 9Router installation…".to_owned(),
        Some(Phase::Import) => "Importing from 9Router…".to_owned(),
        None => match preview {
            Some(Outcome::Completed { report, .. }) => format!(
                "Found {} at {}. {} record(s) would be written.",
                report.format,
                report.source,
                report.pending_writes()
            ),
            Some(Outcome::Missing(_)) => "No 9Router installation found.".to_owned(),
            Some(Outcome::Refused(rejected)) => {
                format!("The import was refused: {}", rejected.message)
            }
            None => "Idle.".to_owned(),
        },
    }
}

/// Which request is open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    /// A dry run.
    Scan,
    /// A real import.
    Import,
}

/// `POST` to the migrate endpoint, keeping the status alongside the body.
///
/// Unlike `api::post` this does not throw the body away on a non-2xx status:
/// the 404 body is the only place the searched paths appear, and dropping it
/// would leave a user who keeps 9Router elsewhere with no way to find it.
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
/// The native target exists so this module's parsing and gating stay
/// unit-testable; a request there is a programming error, reported rather than
/// faked.
#[cfg(not(target_arch = "wasm32"))]
#[expect(
    clippy::unused_async,
    reason = "signature must match the wasm arm so callers need no cfg of their own"
)]
pub async fn post_with_status(_path: &str, _body: &str) -> Result<(u16, String), ApiError> {
    Err(ApiError::Environment)
}

/// Run one migrate call end to end.
pub async fn run_migrate(data_dir: String, dry_run: bool) -> Result<Outcome, ApiError> {
    let (status, body) = post_with_status(MIGRATE_PATH, &request_body(&data_dir, dry_run)).await?;
    parse_response(status, &body)
}
