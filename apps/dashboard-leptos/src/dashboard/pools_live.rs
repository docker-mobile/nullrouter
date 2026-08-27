//! Pure state for the Proxy Pools panel: parse pools, order them, and settle one
//! write.
//!
//! The panel this backs used to render `proxy_pools_dashboard_state()` — a
//! compile-time struct whose `entries` were always empty and whose `sample_entry`
//! was a made-up "Cloudflare edge relay" pointing at
//! `cloudflare-relay.example.workers.dev`. It drew that row under the heading
//! "Proxy pool sample row state" next to an empty-state card, so a user with no
//! pools saw a fabricated one, and a user with three saw none of them.
//!
//! Everything here exists so that cannot happen again:
//!
//! * [`parse_pools`] is the only way a row reaches the panel, so a row can only
//!   describe something `GET /api/proxy-pools` returned.
//! * [`PoolList::is_empty`] is a state the panel renders as itself. No branch
//!   substitutes a sample.
//! * [`PoolList::take`] / [`PoolList::restore`] make an optimistic delete
//!   reversible, and [`settle_delete`] knows that the state service answers `409`
//!   with `boundConnectionCount` when a pool is still assigned to a connection —
//!   a refusal the row must survive.
//! * [`settle_test`] distinguishes "the upstream rejected this proxy" from "this
//!   build does not test proxies", because `nullrouter-api` answers `501` for
//!   every pool and calling that a failed proxy would be a lie.
//!
//! Kept free of `leptos` and of `fetch` so it is unit-testable on the native
//! target; `ui/proxy_pools.rs` owns the signals.

use crate::api::ApiError;
use serde::{Deserialize, Serialize};

/// The endpoint that owns the pool list.
pub const POOLS_PATH: &str = "/api/proxy-pools";

/// The list plus `boundConnectionCount` on every row.
///
/// The count is opt-in upstream (`includeUsage`), and the panel needs it: it is
/// what makes "2 bound" honest and what explains a `409` before the user has to
/// guess why a delete was refused.
pub const POOLS_USAGE_PATH: &str = "/api/proxy-pools?includeUsage=true";

/// `GET`/`PUT`/`DELETE` path for one pool.
pub fn pool_path(id: &str) -> String {
    format!("{POOLS_PATH}/{}", encode_path_segment(id))
}

/// `POST` path that tests one pool.
///
/// The extra segment routes this away from the state service
/// (`is_collection_or_item_except` in `services/gateway-pingora/src/routing.rs`),
/// so it lands on `nullrouter-api`, which answers `501 unsupported`. That is why
/// [`TestOutcome::Unsupported`] is a first-class result rather than a failure.
pub fn pool_test_path(id: &str) -> String {
    format!("{}/test", pool_path(id))
}

/// Percent-encode everything outside RFC 3986 `unreserved`.
///
/// Ids are minted by the state service, but they still travel through a URL, so
/// nothing is trusted to be path-safe. Shared with the Combos and Pricing panels
/// rather than copied into each one.
pub fn encode_path_segment(value: &str) -> String {
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
///
/// A match rather than a lookup table so no index can be out of range and the
/// function is total without a fallback that would mis-encode a byte.
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

/// The `{"proxyPools":[...]}` envelope from `GET /api/proxy-pools`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PoolsEnvelope {
    proxy_pools: Vec<Pool>,
}

/// The `{"proxyPool":{...}}` envelope from `POST`, `GET {id}`, and `PUT {id}`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PoolEnvelope {
    proxy_pool: Pool,
}

/// One proxy pool entry, as `GET /api/proxy-pools` reports it.
///
/// Mirrors `ProxyPool` in `services/state-actix/src/store.rs`.
///
/// `id`, `name`, `proxyUrl`, `type`, `isActive`, and `strictProxy` carry no
/// `serde` default. They are the row's identity and its two routing claims: a
/// card titled with a defaulted empty name, a delete button pointed at an empty
/// id, or an `isActive` defaulted to `false` would each state something the
/// server did not. The state service always serialises all six, so an absent one
/// is a shape change and must fail the parse rather than render as a guess.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Pool {
    pub id: String,
    pub name: String,
    pub proxy_url: String,
    pub is_active: bool,
    pub strict_proxy: bool,
    /// `http`, `cloudflare`, `vercel`, or `deno` upstream. Kept as received so an
    /// unrecognised value is shown verbatim instead of being coerced to `http`,
    /// which would misdescribe how the pool routes.
    #[serde(rename = "type")]
    pub proxy_type: String,
    /// Comma-separated bypass list. Empty means "no bypasses", which is a real
    /// answer, so this defaults rather than failing the parse.
    #[serde(default)]
    pub no_proxy: String,
    #[serde(default)]
    pub test_status: Option<String>,
    #[serde(default)]
    pub last_tested_at: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    /// How many connections use this pool. Present only with `includeUsage=true`,
    /// so `None` means "not reported" and renders as nothing at all — never `0`,
    /// which would read as "safe to delete".
    #[serde(default)]
    pub bound_connection_count: Option<usize>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

impl Pool {
    /// How this pool reaches upstream.
    pub fn kind(&self) -> PoolKind {
        PoolKind::from_wire(&self.proxy_type)
    }

    /// The last test result the router recorded.
    pub fn status(&self) -> PoolTestStatus {
        PoolTestStatus::from_wire(self.test_status.as_deref())
    }

    /// The bypass list, when there is one.
    ///
    /// `None` rather than an empty chip: most pools bypass nothing.
    pub fn no_proxy_label(&self) -> Option<&str> {
        Some(self.no_proxy.trim()).filter(|value| !value.is_empty())
    }

    /// "2 bound", only when the server reported a count.
    pub fn bound_label(&self) -> Option<String> {
        self.bound_connection_count
            .map(|count| format!("{count} bound"))
    }

    /// When this pool was last tested, or `"never"`.
    ///
    /// The stored form is `unix-ms:<millis>`; [`format_timestamp`] renders it as
    /// UTC. An unparseable value is shown as received rather than dropped.
    pub fn last_tested_label(&self) -> String {
        self.last_tested_at
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map_or_else(|| String::from("never"), format_timestamp)
    }

    /// Whether a failed proxy stops the request or falls back to a direct one.
    pub const fn strict_label(&self) -> &'static str {
        if self.strict_proxy {
            "strict proxy"
        } else {
            "fallback allowed"
        }
    }

    /// Heading id, used to label this row's region for assistive technology.
    pub fn heading_id(&self) -> String {
        format!("nr-pool-heading-{}", dom_suffix(&self.id))
    }

    /// Id of this row's live status region.
    pub fn status_id(&self) -> String {
        format!("nr-pool-status-{}", dom_suffix(&self.id))
    }

    /// Accessible label for the delete control, naming the row it destroys.
    ///
    /// A bare "Delete" is ambiguous once several rows are on screen, and this
    /// action is irreversible.
    pub fn delete_label(&self) -> String {
        format!("Delete proxy pool {}", self.name)
    }

    pub fn test_label(&self) -> String {
        format!("Test proxy pool {}", self.name)
    }

    pub fn edit_label(&self) -> String {
        format!("Edit proxy pool {}", self.name)
    }

    /// Accessible label for the activate/deactivate control.
    ///
    /// Says which way it will move, because the button's own text is the state it
    /// switches to and that is ambiguous read out of context.
    pub fn toggle_label(&self) -> String {
        if self.is_active {
            format!("Deactivate proxy pool {}", self.name)
        } else {
            format!("Activate proxy pool {}", self.name)
        }
    }

    /// The word on the toggle button.
    pub const fn toggle_text(&self) -> &'static str {
        if self.is_active { "Disable" } else { "Enable" }
    }

    /// Routing state in words.
    pub const fn active_label(&self) -> &'static str {
        if self.is_active { "active" } else { "inactive" }
    }
}

/// Reduce an id to characters that are safe in a DOM id.
fn dom_suffix(id: &str) -> String {
    id.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

/// How a pool reaches upstream.
///
/// Upstream stores this as a free string, so an unknown value is preserved and
/// displayed rather than folded into `Http`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PoolKind {
    Http,
    Cloudflare,
    Vercel,
    Deno,
    Other(String),
}

impl PoolKind {
    fn from_wire(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "http" | "https" | "socks" | "socks5" | "" => Self::Http,
            "cloudflare" => Self::Cloudflare,
            "vercel" => Self::Vercel,
            "deno" => Self::Deno,
            other => Self::Other(other.to_owned()),
        }
    }

    /// Badge text, or `None` for a plain proxy.
    ///
    /// A direct HTTP proxy gets no badge: every pool would carry one, so it would
    /// say nothing. A relay does, because it changes where traffic exits.
    pub const fn badge_label(&self) -> Option<&str> {
        match self {
            Self::Http => None,
            Self::Cloudflare => Some("cloudflare relay"),
            Self::Vercel => Some("vercel relay"),
            Self::Deno => Some("deno relay"),
            Self::Other(value) => Some(value.as_str()),
        }
    }

    /// The value to send back as `type`.
    pub const fn wire_value(&self) -> &str {
        match self {
            Self::Http => "http",
            Self::Cloudflare => "cloudflare",
            Self::Vercel => "vercel",
            Self::Deno => "deno",
            Self::Other(value) => value.as_str(),
        }
    }
}

/// The last recorded outcome of testing a pool.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PoolTestStatus {
    Passing,
    Failing,
    /// No status recorded, or a value this build does not recognise.
    Untested,
}

impl PoolTestStatus {
    fn from_wire(value: Option<&str>) -> Self {
        match value
            .map(str::trim)
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "active" | "ok" | "success" => Self::Passing,
            "error" | "failed" => Self::Failing,
            _ => Self::Untested,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Passing => "Last test passed",
            Self::Failing => "Last test failed",
            Self::Untested => "Never tested",
        }
    }

    /// Status-pill class, reusing the shared `is-*` vocabulary.
    pub const fn class_name(self) -> &'static str {
        match self {
            Self::Passing => "is-connected",
            Self::Failing => "is-degraded",
            Self::Untested => "is-idle",
        }
    }
}

/// Render a stored timestamp as UTC.
///
/// The state service writes `unix-ms:<millis>` (`timestamp()` in `store.rs`).
/// Anything else — a legacy ISO string, a value from an imported state file — is
/// returned as received: the dashboard would rather show an unfamiliar format
/// than drop a real timestamp or invent a plausible one.
pub fn format_timestamp(value: &str) -> String {
    let trimmed = value.trim();
    let Some(millis) = trimmed
        .strip_prefix("unix-ms:")
        .and_then(|digits| digits.trim().parse::<u64>().ok())
    else {
        return trimmed.to_owned();
    };
    iso_from_millis(millis)
}

/// `1717000000000` → `2024-05-29 18:26:40 UTC`.
fn iso_from_millis(millis: u64) -> String {
    let seconds = millis / 1000;
    let day_seconds = seconds % 86_400;
    let days = i64::try_from(seconds / 86_400).unwrap_or_default();
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        day_seconds / 3600,
        (day_seconds % 3600) / 60,
        day_seconds % 60,
    )
}

/// Days since 1970-01-01 → civil date (Howard Hinnant's `civil_from_days`).
///
/// Written in `u64`/`i64` with `try_from` so no cast can truncate, and total for
/// every input: the dashboard must never panic while formatting a date.
fn civil_from_days(days: i64) -> (i64, u64, u64) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = u64::try_from(shifted - era * 146_097).unwrap_or_default();
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_position = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_position + 2) / 5 + 1;
    let month = if month_position < 10 {
        month_position + 3
    } else {
        month_position - 9
    };
    let year = i64::try_from(year_of_era).unwrap_or_default() + era * 400;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// The configured pools, ordered for display.
///
/// Ordering is applied on construction so the panel never re-sorts while
/// rendering. Sorted by name rather than by the server's `updatedAt` descending:
/// every toggle rewrites `updatedAt`, so the server's order would make a row jump
/// to the top of the list the moment you disabled it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PoolList {
    pools: Vec<Pool>,
}

impl PoolList {
    pub fn new(mut pools: Vec<Pool>) -> Self {
        pools.sort_by(compare_pools);
        Self { pools }
    }

    pub fn pools(&self) -> &[Pool] {
        &self.pools
    }

    /// `true` when the router holds no pools at all.
    ///
    /// The panel renders this as the empty state. It is the reason this module
    /// exists: the old panel could not tell "none configured" from "never asked",
    /// so it always drew a sample row.
    pub const fn is_empty(&self) -> bool {
        self.pools.is_empty()
    }

    pub const fn len(&self) -> usize {
        self.pools.len()
    }

    pub fn active_count(&self) -> usize {
        self.pools.iter().filter(|pool| pool.is_active).count()
    }

    /// Every id, in display order. Backs "select all".
    pub fn ids(&self) -> Vec<String> {
        self.pools.iter().map(|pool| pool.id.clone()).collect()
    }

    pub fn get(&self, id: &str) -> Option<&Pool> {
        self.pools.iter().find(|pool| pool.id == id)
    }

    /// Remove one pool, remembering where it was.
    ///
    /// `None` when the id is not present, so a double-click on delete cannot
    /// manufacture a second rollback slot.
    pub fn take(&mut self, id: &str) -> Option<PendingDelete> {
        let index = self.pools.iter().position(|pool| pool.id == id)?;
        let pool = Box::new(self.pools.remove(index));
        Some(PendingDelete { index, pool })
    }

    /// Put a removed pool back at its original index.
    ///
    /// The index is clamped because the list may have been refreshed while the
    /// `DELETE` was in flight; the row returns rather than being dropped, and
    /// re-sorting keeps it in the right place either way.
    pub fn restore(&mut self, pending: PendingDelete) {
        let index = pending.index.min(self.pools.len());
        self.pools.insert(index, *pending.pool);
        self.pools.sort_by(compare_pools);
    }

    /// Add or replace a pool the server just confirmed, keeping order.
    pub fn upsert(&mut self, pool: Pool) {
        self.pools.retain(|existing| existing.id != pool.id);
        self.pools.push(pool);
        self.pools.sort_by(compare_pools);
    }

    /// Flip one pool's routing flag, returning the value it had.
    ///
    /// The previous value is what makes the optimistic toggle reversible: a
    /// refused `PUT` puts exactly that back, rather than assuming the negation.
    pub fn set_active(&mut self, id: &str, is_active: bool) -> Option<bool> {
        let pool = self.pools.iter_mut().find(|pool| pool.id == id)?;
        let previous = pool.is_active;
        pool.is_active = is_active;
        Some(previous)
    }

    /// Record what a live test learned about one pool.
    pub fn set_test_result(&mut self, id: &str, status: Option<String>, error: Option<String>) {
        if let Some(pool) = self.pools.iter_mut().find(|pool| pool.id == id) {
            pool.test_status = status;
            pool.last_error = error;
        }
    }
}

/// Display order: name, then id.
///
/// `id` is the final key so the order is total and does not shuffle between
/// renders when two pools share a name.
fn compare_pools(left: &Pool, right: &Pool) -> std::cmp::Ordering {
    left.name
        .to_ascii_lowercase()
        .cmp(&right.name.to_ascii_lowercase())
        .then_with(|| left.id.cmp(&right.id))
}

/// A pool removed optimistically, held until the `DELETE` settles.
///
/// Boxed because it travels inside [`DeleteSettlement`], and a
/// several-hundred-byte variant next to a unit one would make every settlement
/// pay for the rollback case.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingDelete {
    index: usize,
    pool: Box<Pool>,
}

impl PendingDelete {
    pub fn id(&self) -> &str {
        &self.pool.id
    }

    pub fn name(&self) -> &str {
        &self.pool.name
    }

    /// How many connections the server said were using this pool, when it said.
    pub const fn bound_connection_count(&self) -> Option<usize> {
        self.pool.bound_connection_count
    }
}

/// Parse a `GET /api/proxy-pools` body.
///
/// `None` on anything that is not a `proxyPools` array of well-formed rows, so
/// the panel reports a failure instead of rendering an empty list that reads as
/// "you have no proxy pools". An empty array is a success: it means exactly that,
/// and [`PoolList::is_empty`] carries it to the empty state.
pub fn parse_pools(body: &str) -> Option<PoolList> {
    serde_json::from_str::<PoolsEnvelope>(body)
        .ok()
        .map(|envelope| PoolList::new(envelope.proxy_pools))
}

/// Parse the `{"proxyPool":{...}}` body returned by a create or an update.
pub fn parse_pool(body: &str) -> Option<Pool> {
    serde_json::from_str::<PoolEnvelope>(body)
        .ok()
        .map(|envelope| envelope.proxy_pool)
}

/// How a `DELETE /api/proxy-pools/{id}` ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeleteOutcome {
    /// 2xx: the router confirmed the removal.
    Confirmed,
    /// The request failed or was refused.
    Rejected(ApiError),
}

/// What the panel should do with an optimistically removed row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeleteSettlement {
    /// The removal stands.
    Removed,
    /// The row must be put back, and the reason shown.
    RolledBack {
        pending: PendingDelete,
        error: ApiError,
        /// What to say about it. Composed here because the interesting case —
        /// `409`, the pool is still assigned — needs the row's own bound count to
        /// be actionable, and the panel does not otherwise hold it.
        message: String,
    },
}

/// Settle one optimistic delete.
///
/// Two refusals get specific treatment:
///
/// * `404` settles as [`DeleteSettlement::Removed`]. The pool is not there any
///   more, which is the state the panel already shows; restoring the row would put
///   back something that no longer exists.
/// * `409` is the state service refusing to orphan a connection
///   (`DeleteProxyPoolResult::InUse`). The row comes back and the message names
///   the count, because "conflict" alone does not tell anyone what to do next.
pub fn settle_delete(pending: PendingDelete, outcome: DeleteOutcome) -> DeleteSettlement {
    match outcome {
        DeleteOutcome::Confirmed | DeleteOutcome::Rejected(ApiError::Status(404)) => {
            DeleteSettlement::Removed
        }
        DeleteOutcome::Rejected(error) => {
            let message = delete_refusal_message(&pending, error);
            DeleteSettlement::RolledBack {
                pending,
                error,
                message,
            }
        }
    }
}

/// Why a delete was refused, in words the user can act on.
fn delete_refusal_message(pending: &PendingDelete, error: ApiError) -> String {
    let name = pending.name();
    if error == ApiError::Status(409) {
        return pending.bound_connection_count().map_or_else(
            // The count is only present with `includeUsage=true`; say what is known
            // rather than guessing a number.
            || {
                format!(
                    "{name} is still assigned to at least one provider connection. Remove the \
                     proxy from those connections before deleting the pool."
                )
            },
            |count| {
                format!(
                    "{name} is still assigned to {}. Remove the proxy from {} before deleting the \
                     pool.",
                    plural(count, "connection"),
                    if count == 1 { "it" } else { "them" },
                )
            },
        );
    }
    format!("{name} was not deleted. {}", error.message())
}

/// How a routing toggle ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToggleSettlement {
    /// The router applied it and returned the row. Boxed for the same reason as
    /// [`PendingDelete`]: one large variant should not widen every settlement.
    Applied(Box<Pool>),
    /// The flag must go back to `previous`, and the reason be shown.
    RolledBack {
        previous: bool,
        error: ApiError,
        message: String,
    },
}

/// Settle one optimistic activate/deactivate.
///
/// A 2xx whose body does not parse is still a rollback: the router may well have
/// applied the change, but this page can no longer say what the row holds, and
/// showing the optimistic value would be a claim it cannot support. The panel
/// reloads after a rollback, so the server settles the disagreement.
pub fn settle_toggle(
    name: &str,
    previous: bool,
    response: Result<&str, ApiError>,
) -> ToggleSettlement {
    match response {
        Ok(body) => parse_pool(body).map_or_else(
            || ToggleSettlement::RolledBack {
                previous,
                error: ApiError::Body,
                message: format!("{name} may not have changed. {}", ApiError::Body.message()),
            },
            |pool| ToggleSettlement::Applied(Box::new(pool)),
        ),
        Err(error) => ToggleSettlement::RolledBack {
            previous,
            error,
            message: format!("{name} was not changed. {}", error.message()),
        },
    }
}

/// The `{"ok":bool,...}` body returned by the test endpoint.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TestResponse {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    unsupported: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    elapsed_ms: Option<u64>,
}

/// Result of `POST /api/proxy-pools/{id}/test`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TestOutcome {
    Passed {
        elapsed_ms: Option<u64>,
    },
    Failed(String),
    /// The build answered `501 unsupported`: proxy testing is not implemented.
    /// Distinct from a failure, because nothing was tested.
    Unsupported,
    /// The request itself did not complete.
    Rejected(ApiError),
}

impl TestOutcome {
    pub fn message(&self) -> String {
        match self {
            Self::Passed { elapsed_ms } => elapsed_ms.map_or_else(
                || String::from("Proxy test passed."),
                |elapsed| format!("Proxy test passed in {elapsed} ms."),
            ),
            Self::Failed(reason) => format!("Proxy test failed: {reason}"),
            Self::Unsupported => {
                String::from("This build does not run proxy tests. Nothing was tested.")
            }
            Self::Rejected(error) => error.message().to_owned(),
        }
    }

    /// The `testStatus` to record locally, or `None` when nothing was learned.
    pub const fn recorded_status(&self) -> Option<&'static str> {
        match self {
            Self::Passed { .. } => Some("active"),
            Self::Failed(_) => Some("error"),
            Self::Unsupported | Self::Rejected(_) => None,
        }
    }

    /// The `lastError` to record locally.
    pub fn recorded_error(&self) -> Option<String> {
        match self {
            Self::Failed(reason) => Some(reason.clone()),
            Self::Passed { .. } | Self::Unsupported | Self::Rejected(_) => None,
        }
    }
}

/// Interpret a test response.
///
/// `body` is `Ok` only for a 2xx. `nullrouter-api` answers `501` for every pool,
/// which arrives as [`ApiError::Status`] and maps to [`TestOutcome::Unsupported`]
/// — the panel must not show that as "proxy failed", because no proxy was dialled.
pub fn settle_test(response: Result<&str, ApiError>) -> TestOutcome {
    match response {
        Ok(body) => match serde_json::from_str::<TestResponse>(body) {
            Ok(parsed) if parsed.unsupported => TestOutcome::Unsupported,
            Ok(parsed) if parsed.ok => TestOutcome::Passed {
                elapsed_ms: parsed.elapsed_ms,
            },
            Ok(parsed) => TestOutcome::Failed(
                parsed
                    .error
                    .map(|error| error.trim().to_owned())
                    .filter(|error| !error.is_empty())
                    .unwrap_or_else(|| String::from("the proxy did not answer")),
            ),
            Err(_error) => TestOutcome::Rejected(ApiError::Body),
        },
        Err(ApiError::Status(501)) => TestOutcome::Unsupported,
        Err(error) => TestOutcome::Rejected(error),
    }
}

/// "1 connection" / "3 connections", so a count never reads as a fragment.
pub fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("{count} {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// A pool the user is composing, for either a create or an edit.
///
/// One struct for both because `POST /api/proxy-pools` and
/// `PUT /api/proxy-pools/{id}` take the same fields; `id` is what decides which
/// request the form sends.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PoolDraft {
    /// `Some` when editing an existing pool.
    pub id: Option<String>,
    pub name: String,
    pub proxy_url: String,
    pub no_proxy: String,
    pub is_active: bool,
    pub strict_proxy: bool,
    /// Wire value for `type`.
    pub kind: String,
}

impl Default for PoolDraft {
    /// Matches the state service's own create defaults (`create_proxy_pool`):
    /// `isActive: true`, `strictProxy: false`, `type: "http"`. The form must not
    /// show a default the server would not apply.
    fn default() -> Self {
        Self {
            id: None,
            name: String::new(),
            proxy_url: String::new(),
            no_proxy: String::new(),
            is_active: true,
            strict_proxy: false,
            kind: String::from("http"),
        }
    }
}

/// The create/update request body.
///
/// Serialised through `serde` so a name or URL containing a quote or backslash
/// cannot break out of the payload.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PoolRequest<'a> {
    name: &'a str,
    proxy_url: &'a str,
    no_proxy: &'a str,
    is_active: bool,
    strict_proxy: bool,
    #[serde(rename = "type")]
    proxy_type: &'a str,
}

impl PoolDraft {
    /// Prefill the form from an existing row.
    pub fn for_edit(pool: &Pool) -> Self {
        Self {
            id: Some(pool.id.clone()),
            name: pool.name.clone(),
            proxy_url: pool.proxy_url.clone(),
            no_proxy: pool.no_proxy.clone(),
            is_active: pool.is_active,
            strict_proxy: pool.strict_proxy,
            kind: pool.kind().wire_value().to_owned(),
        }
    }

    pub const fn is_edit(&self) -> bool {
        self.id.is_some()
    }

    /// Validate the draft and render the body to send.
    pub fn body(&self) -> Result<String, DraftError> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err(DraftError::NameMissing);
        }
        let proxy_url = self.proxy_url.trim();
        if proxy_url.is_empty() {
            return Err(DraftError::UrlMissing);
        }
        let request = PoolRequest {
            name,
            proxy_url,
            no_proxy: self.no_proxy.trim(),
            is_active: self.is_active,
            strict_proxy: self.strict_proxy,
            proxy_type: self.kind.trim(),
        };
        serde_json::to_string(&request).map_err(|_error| DraftError::Encode)
    }

    /// The blocking validation error, for disabling submit before a click.
    pub fn validation_error(&self) -> Option<DraftError> {
        self.body().err()
    }

    /// A note about the URL that is worth saying but must not block a save.
    ///
    /// The endpoint accepts any non-empty string, and upstream proxy URLs are not
    /// always scheme-prefixed, so a missing scheme is a hint rather than an error.
    /// Refusing it here would reject a value the router would have accepted.
    pub fn url_hint(&self) -> Option<&'static str> {
        let url = self.proxy_url.trim();
        if url.is_empty() || url.contains("://") {
            None
        } else {
            Some("No scheme in this URL. The router will use it as written; http:// is usual.")
        }
    }
}

/// Why a draft cannot be submitted.
///
/// Mirrors the state service's own rejections (`parse_proxy_pool_input`) so the
/// form explains the problem before spending a request on it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DraftError {
    NameMissing,
    UrlMissing,
    /// The body could not be encoded. Not reachable from text input, but reported
    /// rather than silently dropped.
    Encode,
}

impl DraftError {
    pub const fn message(self) -> &'static str {
        match self {
            Self::NameMissing => "Give this proxy pool a name.",
            Self::UrlMissing => "A proxy URL is required.",
            Self::Encode => "This entry could not be encoded as a request.",
        }
    }
}

/// What a pasted proxy list would create, and what could not be read.
///
/// Parsed before anything is sent so the user sees the plan first. Upstream's
/// batch import silently dropped unparseable lines; naming them is the point.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ImportPlan {
    pub drafts: Vec<PoolDraft>,
    pub rejected: Vec<ImportRejection>,
}

/// A line that could not be read as a proxy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportRejection {
    /// 1-based line number, so it matches what the textarea shows.
    pub line: usize,
    pub text: String,
}

impl ImportPlan {
    pub const fn is_empty(&self) -> bool {
        self.drafts.is_empty()
    }

    /// "2 proxies ready, 1 line unreadable".
    pub fn summary(&self) -> String {
        let ready = plural(self.drafts.len(), "proxy entry");
        if self.rejected.is_empty() {
            format!("{ready} ready to create.")
        } else {
            format!(
                "{ready} ready to create. {} could not be read.",
                plural(self.rejected.len(), "line"),
            )
        }
    }
}

/// Parse a pasted proxy list.
///
/// Accepts the two formats upstream documents:
///
/// * `protocol://user:pass@host:port` — used as written.
/// * `host:port:user:pass` / `host:port` — rewritten as an `http://` URL, which is
///   what the shorthand means.
///
/// Blank lines and `#` comments are skipped rather than rejected. Everything else
/// that does not fit lands in [`ImportPlan::rejected`] with its line number.
pub fn parse_import(text: &str) -> ImportPlan {
    let mut plan = ImportPlan::default();
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match import_line(line) {
            Some(draft) => plan.drafts.push(draft),
            None => plan.rejected.push(ImportRejection {
                line: index + 1,
                text: line.to_owned(),
            }),
        }
    }
    plan
}

/// One import line as a draft, or `None` when it cannot be read.
fn import_line(line: &str) -> Option<PoolDraft> {
    if let Some((_scheme, rest)) = line.split_once("://") {
        let host = rest.rsplit('@').next().unwrap_or(rest);
        return Some(import_draft(host, line));
    }
    let parts: Vec<&str> = line.split(':').collect();
    let host = parts.first().copied().filter(|host| !host.is_empty())?;
    let port = parts.get(1).copied().filter(|port| !port.is_empty())?;
    // A port has to be a port; otherwise this is not the shorthand and guessing
    // would create a pool pointed somewhere the user did not name.
    port.parse::<u16>().ok()?;
    let authority = match (parts.get(2), parts.get(3)) {
        (Some(user), Some(password)) if !user.is_empty() && !password.is_empty() => {
            format!("{user}:{password}@{host}:{port}")
        }
        _ if parts.len() > 2 => return None,
        _ => format!("{host}:{port}"),
    };
    let label = format!("{host}:{port}");
    Some(import_draft(&label, &format!("http://{authority}")))
}

/// A draft named after its endpoint.
///
/// The name is the host and port rather than a generated "Imported proxy 1": it
/// is the one piece of identity the pasted line actually carries.
fn import_draft(label: &str, proxy_url: &str) -> PoolDraft {
    PoolDraft {
        name: label.trim().to_owned(),
        proxy_url: proxy_url.to_owned(),
        ..PoolDraft::default()
    }
}

// ── requests ────────────────────────────────────────────────────────────────
//
// Thin wrappers over `crate::api`, kept here so the panel holds signals and views
// only. Each returns a result the functions above have already interpreted, so a
// caller cannot forget to distinguish "empty" from "failed", or "unsupported" from
// "the proxy is broken".
//
// `api::request` is split on `target_arch`: the wasm arm performs the `fetch`, the
// native arm returns `ApiError::Environment` rather than pretending to have
// contacted a router. That keeps every function below callable — and every branch
// above testable — on the native target.

/// `GET /api/proxy-pools?includeUsage=true`.
pub async fn load_pools() -> Result<PoolList, ApiError> {
    let body = crate::api::get(POOLS_USAGE_PATH).await?;
    parse_pools(&body).ok_or(ApiError::Body)
}

/// `POST /api/proxy-pools`, returning the row the router created.
pub async fn create_pool(body: String) -> Result<Pool, ApiError> {
    let response = crate::api::post(POOLS_PATH, &body).await?;
    parse_pool(&response).ok_or(ApiError::Body)
}

/// `PUT /api/proxy-pools/{id}`, returning the row the router stored.
pub async fn update_pool(id: &str, body: String) -> Result<Pool, ApiError> {
    let response = crate::api::put(&pool_path(id), &body).await?;
    parse_pool(&response).ok_or(ApiError::Body)
}

/// `PUT /api/proxy-pools/{id}` with only the routing flag.
///
/// A partial update: `ProxyPoolRequest` leaves every absent field alone, so this
/// cannot clobber a URL or a bypass list the user did not touch.
pub async fn set_pool_active(name: &str, id: &str, is_active: bool) -> ToggleSettlement {
    let body = format!(r#"{{"isActive":{is_active}}}"#);
    let response = crate::api::put(&pool_path(id), &body).await;
    settle_toggle(
        name,
        !is_active,
        response.as_deref().map_err(|error| *error),
    )
}

/// `DELETE /api/proxy-pools/{id}`.
pub async fn delete_pool(id: &str) -> DeleteOutcome {
    match crate::api::delete(&pool_path(id)).await {
        Ok(_body) => DeleteOutcome::Confirmed,
        Err(error) => DeleteOutcome::Rejected(error),
    }
}

/// `POST /api/proxy-pools/{id}/test`.
///
/// The body is `{}` because the endpoint takes an optional payload and this
/// dashboard has nothing to add to it.
pub async fn test_pool(id: &str) -> TestOutcome {
    let response = crate::api::post(&pool_test_path(id), "{}").await;
    settle_test(response.as_deref().map_err(|error| *error))
}
