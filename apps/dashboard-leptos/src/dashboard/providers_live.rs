//! Pure state for the Providers panel: parse connections, order them, and
//! settle one write.
//!
//! The panel this backs used to render `provider_groups()` — compile-time tiles
//! built from `nullrouter-contracts` fixtures. It showed "OAuth Providers" and
//! "API Key Providers" cards for accounts nobody had configured, and hid the
//! ones that existed, because nothing here ever asked the router what it held.
//!
//! Everything in this module exists to make that impossible:
//!
//! * [`parse_connections`] is the only way a connection reaches the panel, so a
//!   card can only describe a row `GET /api/providers` actually returned.
//! * [`ConnectionList::is_empty`] is a state the panel must render as itself.
//!   Zero connections means zero cards plus an invitation to add one; there is
//!   no branch that substitutes a fixture.
//! * The public API strips `apiKey`, `accessToken`, and `refreshToken`
//!   ([`ProviderConnection::public`] in `services/state-actix/src/store.rs`), so
//!   [`Connection`] has no field for them and [`AuthKind::credential_note`]
//!   describes where the secret lives instead of inventing a masked value.
//! * [`ConnectionList::take`] / [`ConnectionList::restore`] make an optimistic
//!   delete reversible, so the row can be removed on click and put back at its
//!   original position when the `DELETE` is refused.
//!
//! Kept free of `leptos` and of `fetch` so it is unit-testable on the native
//! target; the panel in `ui/providers.rs` owns the signals.

use crate::api::{ApiError, DetailedResponse};
use serde::{Deserialize, Serialize};

/// The endpoint that owns the configured-connection list.
pub const CONNECTIONS_PATH: &str = "/api/providers";

/// `GET`/`DELETE` path for one connection.
///
/// Ids are minted by the state service (`connection_<millis>_<n>`), but the id
/// still travels through a URL, so anything outside the unreserved set is
/// percent-encoded rather than trusted to be path-safe.
pub fn connection_path(id: &str) -> String {
    format!("{CONNECTIONS_PATH}/{}", encode_path_segment(id))
}

/// `POST` path that tests one connection.
///
/// Served by `nullrouter-api`, not the state service: the gateway routes
/// `/api/providers/{id}/test` away from state because of the extra segment
/// (`is_collection_or_item_except` in `services/gateway-pingora/src/routing.rs`).
/// It performs a real one-token upstream call, so the response distinguishes a
/// refused credential (`502`) from a test that never ran (`503`/`400`/`404`) — see
/// [`settle_test`].
pub fn connection_test_path(id: &str) -> String {
    format!("{}/test", connection_path(id))
}

/// Percent-encode everything outside RFC 3986 `unreserved`.
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
///
/// Written as a match rather than a lookup table so no index can be out of
/// range, and so the function is total without a fallback that would silently
/// mis-encode a byte.
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

/// The `{"connections":[...]}` envelope returned by `GET /api/providers`.
#[derive(Debug, Deserialize)]
struct ConnectionsEnvelope {
    connections: Vec<Connection>,
}

/// The `{"connection":{...}}` envelope returned by `POST /api/providers`.
#[derive(Debug, Deserialize)]
struct ConnectionEnvelope {
    connection: Connection,
}

/// One configured provider connection, as the public API reports it.
///
/// Mirrors the serialised half of `ProviderConnection` in
/// `services/state-actix/src/store.rs`. The secret fields are absent by
/// construction: `public()` clears them before the record leaves the service, so
/// a field here could only ever hold `None`, and having one would invite a
/// "•••• 1234" rendering that the dashboard cannot honestly produce.
///
/// `id`, `provider`, `authType`, and `name` carry no `serde` default. They are
/// the identity of the row — a card titled with a defaulted empty name, or a
/// delete button pointed at an empty id, would be a fabrication. An absent one
/// is a shape change, so it fails the parse and surfaces as an error.
/// `isActive` is likewise required: it is a routing claim, and a missing flag
/// defaulted to `false` would read as "this connection is disabled" — something
/// the server never said.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub id: String,
    pub provider: String,
    pub auth_type: String,
    pub name: String,
    pub is_active: bool,
    /// Fill-first ordering rank. `skip_serializing_if` never applies to it in
    /// the state service, but `nullrouter-api`'s stub shape omits it, so an
    /// absent priority renders as "unset" instead of a fabricated `0`.
    #[serde(default)]
    pub priority: Option<u32>,
    #[serde(default)]
    pub global_priority: Option<u32>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub default_model: Option<String>,
    /// Raw upstream test status. Interpreted by [`TestStatus`], but kept as
    /// received so an unrecognised value can still be shown verbatim.
    #[serde(default)]
    pub test_status: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub last_error_at: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

impl Connection {
    /// How this connection authenticates upstream.
    pub fn auth_kind(&self) -> AuthKind {
        AuthKind::from_wire(&self.auth_type)
    }

    /// The last test result the router recorded for this connection.
    pub fn test_status(&self) -> TestStatus {
        TestStatus::from_wire(self.test_status.as_deref())
    }

    /// Registry display name for the provider, falling back to its raw id.
    pub fn provider_label(&self) -> String {
        provider_label(&self.provider)
    }

    /// Secondary identity line: the account email when the router knows one.
    ///
    /// `None` rather than a placeholder, because most API-key connections have
    /// no email and inventing one would misdescribe the account.
    pub fn account_label(&self) -> Option<&str> {
        self.email
            .as_deref()
            .map(str::trim)
            .filter(|email| !email.is_empty())
    }

    /// Priority as display text, naming the absence when there is none.
    pub fn priority_label(&self) -> String {
        self.priority
            .map_or_else(|| String::from("unset"), |priority| priority.to_string())
    }

    /// Heading id, used to label the card's region for assistive technology.
    pub fn heading_id(&self) -> String {
        format!("nr-connection-heading-{}", dom_suffix(&self.id))
    }

    /// Id of this card's live status region.
    pub fn status_id(&self) -> String {
        format!("nr-connection-status-{}", dom_suffix(&self.id))
    }

    /// Accessible label for the delete control, naming the row it destroys.
    ///
    /// A bare "Delete" is ambiguous once several cards are on screen, and this
    /// action is irreversible.
    pub fn delete_label(&self) -> String {
        format!(
            "Delete connection {} ({})",
            self.name,
            self.provider_label()
        )
    }

    /// Accessible label for the test control.
    pub fn test_label(&self) -> String {
        format!("Test connection {}", self.name)
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

/// Authentication mechanism behind a connection.
///
/// Upstream stores this as a free string (`authType: "apikey" | "oauth" |
/// "cookie" | "none"`), so unknown values are preserved rather than coerced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuthKind {
    ApiKey,
    OAuth,
    Cookie,
    None,
    Other(String),
}

impl AuthKind {
    fn from_wire(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "apikey" | "api_key" | "api-key" => Self::ApiKey,
            "oauth" | "oauth2" => Self::OAuth,
            "cookie" | "webcookie" => Self::Cookie,
            "none" => Self::None,
            "" => Self::Other(String::from("unspecified")),
            other => Self::Other(other.to_owned()),
        }
    }

    pub const fn label(&self) -> &str {
        match self {
            Self::ApiKey => "API key",
            Self::OAuth => "OAuth",
            Self::Cookie => "Browser cookie",
            Self::None => "No authentication",
            Self::Other(value) => value.as_str(),
        }
    }

    /// Where the credential lives, stated without inventing its value.
    ///
    /// `GET /api/providers` redacts every secret, so the dashboard genuinely
    /// does not know the key — not even its length or last four characters. It
    /// says so.
    pub const fn credential_note(&self) -> &'static str {
        match self {
            Self::ApiKey => "Key stored by the router. Never sent to this page.",
            Self::OAuth => "OAuth tokens stored by the router. Never sent to this page.",
            Self::Cookie => "Browser session stored by the router. Never sent to this page.",
            Self::None => "This upstream needs no credential.",
            Self::Other(_) => "Credential stored by the router. Never sent to this page.",
        }
    }
}

/// The last recorded outcome of testing a connection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TestStatus {
    /// Upstream `active` / `ok` / `success`.
    Passing,
    /// Upstream `error`.
    Failing,
    /// Upstream `unavailable`.
    Unavailable,
    /// Upstream `expired`: credential needs re-authentication.
    Expired,
    /// Upstream `testing`: a test was in flight when this was written.
    Testing,
    /// No status recorded, or a value this build does not recognise.
    Untested,
}

impl TestStatus {
    fn from_wire(value: Option<&str>) -> Self {
        match value
            .map(str::trim)
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "active" | "ok" | "success" => Self::Passing,
            "error" | "failed" => Self::Failing,
            "unavailable" => Self::Unavailable,
            "expired" => Self::Expired,
            "testing" => Self::Testing,
            _ => Self::Untested,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Passing => "Last test passed",
            Self::Failing => "Last test failed",
            Self::Unavailable => "Upstream unavailable",
            Self::Expired => "Credential expired",
            Self::Testing => "Test in progress",
            Self::Untested => "Never tested",
        }
    }

    /// Status-pill class, reusing the shared `is-*` vocabulary.
    pub const fn class_name(self) -> &'static str {
        match self {
            Self::Passing => "is-connected",
            Self::Failing | Self::Expired => "is-degraded",
            Self::Unavailable | Self::Testing | Self::Untested => "is-idle",
        }
    }
}

/// The configured connections, ordered for display.
///
/// Ordering is applied on construction so the panel never re-sorts while
/// rendering, and so an optimistic insert lands where the server would put it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConnectionList {
    connections: Vec<Connection>,
}

impl ConnectionList {
    /// Order `connections` and take ownership of them.
    pub fn new(mut connections: Vec<Connection>) -> Self {
        connections.sort_by(compare_connections);
        Self { connections }
    }

    pub fn connections(&self) -> &[Connection] {
        &self.connections
    }

    /// `true` when the router holds no connections at all.
    ///
    /// The panel renders this as the empty state. It is the reason this module
    /// exists: the old panel could not distinguish "none configured" from
    /// "never asked", so it always drew fixtures.
    pub const fn is_empty(&self) -> bool {
        self.connections.is_empty()
    }

    pub const fn len(&self) -> usize {
        self.connections.len()
    }

    /// How many connections are enabled for routing.
    pub fn active_count(&self) -> usize {
        self.connections
            .iter()
            .filter(|connection| connection.is_active)
            .count()
    }

    /// How many distinct providers are configured.
    pub fn provider_count(&self) -> usize {
        let mut providers: Vec<&str> = self
            .connections
            .iter()
            .map(|connection| connection.provider.as_str())
            .collect();
        providers.sort_unstable();
        providers.dedup();
        providers.len()
    }

    /// Connections grouped by provider, in the display order of the flat list.
    ///
    /// Grouping is derived rather than stored: the server returns a flat list,
    /// and a group is only ever a view of rows that exist.
    pub fn groups(&self) -> Vec<ProviderGroupLive> {
        let mut groups: Vec<ProviderGroupLive> = Vec::new();
        for connection in &self.connections {
            match groups
                .iter_mut()
                .find(|group| group.provider == connection.provider)
            {
                Some(group) => group.connections.push(connection.clone()),
                None => groups.push(ProviderGroupLive {
                    provider: connection.provider.clone(),
                    label: connection.provider_label(),
                    accent: provider_accent(&connection.provider).to_owned(),
                    connections: vec![connection.clone()],
                }),
            }
        }
        groups
    }

    /// Remove one connection, remembering where it was.
    ///
    /// `None` when the id is not present, so a double-click on delete cannot
    /// manufacture a second rollback slot.
    pub fn take(&mut self, id: &str) -> Option<PendingDelete> {
        let index = self
            .connections
            .iter()
            .position(|connection| connection.id == id)?;
        let connection = Box::new(self.connections.remove(index));
        Some(PendingDelete { index, connection })
    }

    /// Put a removed connection back at its original index.
    ///
    /// The index is clamped because the list may have been refreshed while the
    /// `DELETE` was in flight; the row returns rather than being dropped, and
    /// re-sorting keeps it in the right place either way.
    pub fn restore(&mut self, pending: PendingDelete) {
        let index = pending.index.min(self.connections.len());
        self.connections.insert(index, *pending.connection);
        self.connections.sort_by(compare_connections);
    }

    /// Add a connection the server just created, keeping the list ordered.
    pub fn insert(&mut self, connection: Connection) {
        self.connections
            .retain(|existing| existing.id != connection.id);
        self.connections.push(connection);
        self.connections.sort_by(compare_connections);
    }

    /// Replace one connection's recorded test status after a live test.
    pub fn set_test_status(&mut self, id: &str, status: Option<String>) {
        if let Some(connection) = self
            .connections
            .iter_mut()
            .find(|connection| connection.id == id)
        {
            connection.test_status = status;
        }
    }
}

/// A connection removed optimistically, held until the `DELETE` settles.
///
/// The row is boxed because it travels inside [`DeleteSettlement`], and a
/// several-hundred-byte variant next to a unit one would make every settlement
/// pay for the rollback case.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingDelete {
    index: usize,
    connection: Box<Connection>,
}

impl PendingDelete {
    pub fn id(&self) -> &str {
        &self.connection.id
    }

    pub fn name(&self) -> &str {
        &self.connection.name
    }
}

/// Connections for one provider.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderGroupLive {
    /// Canonical provider id as stored on the connection.
    pub provider: String,
    /// Registry display name, or the raw id when the provider is unknown here.
    pub label: String,
    pub accent: String,
    pub connections: Vec<Connection>,
}

impl ProviderGroupLive {
    pub fn active_count(&self) -> usize {
        self.connections
            .iter()
            .filter(|connection| connection.is_active)
            .count()
    }

    /// "2 of 3 active", stated from the rows actually present.
    pub fn summary(&self) -> String {
        format!(
            "{} of {} active",
            self.active_count(),
            self.connections.len()
        )
    }
}

/// Display order: provider label, then priority, then name, then id.
///
/// Priority sorts ascending because the router's fill-first strategy tries the
/// lowest number first, so the list reads in the order traffic is offered.
/// A connection with no priority sorts last rather than as `0`, and `id` is the
/// final key so the order is total and does not shuffle between renders.
fn compare_connections(left: &Connection, right: &Connection) -> std::cmp::Ordering {
    provider_label(&left.provider)
        .to_ascii_lowercase()
        .cmp(&provider_label(&right.provider).to_ascii_lowercase())
        .then_with(|| left.provider.cmp(&right.provider))
        .then_with(|| {
            left.priority
                .unwrap_or(u32::MAX)
                .cmp(&right.priority.unwrap_or(u32::MAX))
        })
        .then_with(|| {
            left.name
                .to_ascii_lowercase()
                .cmp(&right.name.to_ascii_lowercase())
        })
        .then_with(|| left.id.cmp(&right.id))
}

/// Parse a `GET /api/providers` body.
///
/// `None` on anything that is not a `connections` array of well-formed rows, so
/// the panel reports a failure instead of rendering an empty list that reads as
/// "you have no providers". An empty array is a success: it means exactly that,
/// and [`ConnectionList::is_empty`] carries it to the empty state.
pub fn parse_connections(body: &str) -> Option<ConnectionList> {
    serde_json::from_str::<ConnectionsEnvelope>(body)
        .ok()
        .map(|envelope| ConnectionList::new(envelope.connections))
}

/// Parse the `{"connection":{...}}` body returned by `POST /api/providers`.
pub fn parse_connection(body: &str) -> Option<Connection> {
    serde_json::from_str::<ConnectionEnvelope>(body)
        .ok()
        .map(|envelope| envelope.connection)
}

/// How a `DELETE /api/providers/{id}` ended.
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
    /// The row must be put back, and the error shown.
    RolledBack {
        pending: PendingDelete,
        error: ApiError,
    },
}

/// Settle one optimistic delete.
///
/// A `404` settles as [`DeleteSettlement::Removed`]: the connection is not there
/// any more, which is the state the panel is already showing. Restoring the card
/// would put back a row that no longer exists.
pub fn settle_delete(pending: PendingDelete, outcome: DeleteOutcome) -> DeleteSettlement {
    match outcome {
        DeleteOutcome::Confirmed | DeleteOutcome::Rejected(ApiError::Status(404)) => {
            DeleteSettlement::Removed
        }
        DeleteOutcome::Rejected(error) => DeleteSettlement::RolledBack { pending, error },
    }
}

/// Result of `POST /api/providers/{id}/test`.
///
/// Three outcomes, not two. A provider that refuses the credential is a verdict
/// worth recording; a router that could not reach its own state service, or a
/// connection that names no model, tested nothing at all — reporting that as a
/// failed key would send the user to replace a key that may be fine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TestOutcome {
    Passed,
    /// The upstream was called and refused. Carries the provider's own message.
    Failed(String),
    /// Nothing was tested. Carries the reason.
    NotTested(String),
    /// The request itself did not complete.
    Rejected(ApiError),
}

impl TestOutcome {
    pub fn message(&self) -> String {
        match self {
            Self::Passed => String::from("Connection test passed."),
            Self::Failed(reason) => format!("Connection test failed: {reason}"),
            Self::NotTested(reason) => format!("Nothing was tested: {reason}"),
            Self::Rejected(error) => error.message().to_owned(),
        }
    }

    /// The `testStatus` to record locally, or `None` when nothing was learned.
    pub const fn recorded_status(&self) -> Option<&'static str> {
        match self {
            Self::Passed => Some("active"),
            Self::Failed(_) => Some("error"),
            Self::NotTested(_) | Self::Rejected(_) => None,
        }
    }
}

/// The body returned by the test endpoint.
#[derive(Debug, Default, Deserialize)]
struct TestResponse {
    #[serde(default)]
    valid: bool,
    #[serde(default)]
    success: bool,
    #[serde(default)]
    error: Option<String>,
}

impl TestResponse {
    /// The endpoint's message, when it sent a usable one.
    fn reason(self) -> Option<String> {
        self.error
            .map(|error| error.trim().to_owned())
            .filter(|error| !error.is_empty())
    }
}

/// Interpret a test response.
///
/// The endpoint's status carries the distinction the panel needs:
///
/// * `200` — the upstream answered successfully.
/// * `502` — the upstream was called and refused. Its own (scrubbed) message is in
///   the body, so it is read out rather than replaced with a generic string: "invalid
///   key" and "model not found" send the user to different places.
/// * `503`/`400`/`404` — nothing was tested, for a reason the body explains.
///
/// A non-2xx therefore has to be inspected, not collapsed into [`ApiError::Status`];
/// that is why this takes the detailed response.
pub fn settle_test(response: Result<DetailedResponse, ApiError>) -> TestOutcome {
    let Ok(response) = response else {
        // `Err` is only a transport or environment failure here.
        return TestOutcome::Rejected(response.err().unwrap_or(ApiError::Network));
    };
    let parsed = serde_json::from_str::<TestResponse>(&response.body);
    let status = response.status;
    match parsed {
        Ok(parsed) if response.ok => {
            if parsed.valid || parsed.success {
                TestOutcome::Passed
            } else {
                // A 2xx that reports failure: trust the body over the status.
                TestOutcome::Failed(
                    parsed
                        .reason()
                        .unwrap_or_else(|| String::from("the upstream rejected the credential")),
                )
            }
        }
        Ok(parsed) => {
            let reason = parsed
                .reason()
                .unwrap_or_else(|| format!("the router answered {status}"));
            if status == 502 {
                TestOutcome::Failed(reason)
            } else {
                TestOutcome::NotTested(reason)
            }
        }
        // Unreadable body: a 2xx cannot be called a pass, and a refusal cannot be
        // attributed to the credential.
        Err(_error) if response.ok => TestOutcome::Rejected(ApiError::Body),
        Err(_error) => TestOutcome::NotTested(format!("the router answered {status}")),
    }
}

/// A connection the user is composing.
///
/// Only the fields `POST /api/providers` accepts as an API-key create:
/// `ProviderRequest` in `services/state-actix/src/routes.rs` forces
/// `authType: "apikey"` and `isActive: true`, so no control here can claim to
/// set anything else.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConnectionDraft {
    pub provider: String,
    pub name: String,
    pub api_key: String,
}

/// Why a draft cannot be submitted.
///
/// Mirrors the state service's own rejections so the form explains the problem
/// before spending a request on it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DraftError {
    ProviderMissing,
    ApiKeyMissing,
    ProviderUnknown,
}

impl DraftError {
    pub const fn message(self) -> &'static str {
        match self {
            Self::ProviderMissing => "Choose a provider.",
            Self::ApiKeyMissing => "This provider needs an API key.",
            Self::ProviderUnknown => "That provider is not in this build's registry.",
        }
    }
}

/// The `POST /api/providers` request body.
///
/// Serialised through `serde` so a key or name containing a quote or backslash
/// cannot break out of the payload.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateRequest<'a> {
    provider: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    api_key: Option<&'a str>,
}

impl ConnectionDraft {
    /// Validate the draft, returning the body to `POST`.
    ///
    /// `name` is omitted when blank: the service falls back to the provider id,
    /// which is a better default than a name this page made up.
    pub fn create_body(&self) -> Result<String, DraftError> {
        let provider = self.provider.trim();
        if provider.is_empty() {
            return Err(DraftError::ProviderMissing);
        }
        let option = catalog_option(provider).ok_or(DraftError::ProviderUnknown)?;
        let api_key = self.api_key.trim();
        if option.requires_api_key && api_key.is_empty() {
            return Err(DraftError::ApiKeyMissing);
        }
        let name = self.name.trim();
        let request = CreateRequest {
            provider,
            name: Some(name).filter(|name| !name.is_empty()),
            api_key: Some(api_key).filter(|key| !key.is_empty()),
        };
        serde_json::to_string(&request).map_err(|_error| DraftError::ProviderUnknown)
    }

    /// The blocking validation error, for disabling submit before a click.
    pub fn validation_error(&self) -> Option<DraftError> {
        self.create_body().err()
    }
}

/// One provider the registry offers, as the create form and catalog show it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogOption {
    pub id: String,
    pub name: String,
    /// Registry category: `apikey`, `oauth`, `freeTier`, `free`, `webCookie`.
    pub category: String,
    pub accent: String,
    /// Short glyph for the tile, derived from the registry or the name.
    pub initials: String,
    pub model_count: usize,
    /// Whether `POST /api/providers` requires a key for this provider.
    pub requires_api_key: bool,
    /// Whether this provider can be added with an API key at all.
    pub api_key_capable: bool,
}

impl CatalogOption {
    /// How this provider is authenticated, in words.
    pub const fn auth_label(&self) -> &'static str {
        if !self.api_key_capable {
            return "OAuth or browser sign-in";
        }
        if self.requires_api_key {
            "API key"
        } else {
            "No credential"
        }
    }

    /// Why a provider is absent from the create form, when it is.
    ///
    /// The create endpoint is API-key only; OAuth secrets arrive through the
    /// internal refresh endpoint, so offering an OAuth provider in this form
    /// would promise a flow that does not exist here.
    pub const fn unavailable_note(&self) -> Option<&'static str> {
        if self.api_key_capable {
            None
        } else {
            Some(
                "Needs an OAuth or browser sign-in flow, which this build does not run from the dashboard.",
            )
        }
    }
}

/// Providers that can be created from this form, by display name.
pub fn api_key_catalog() -> Vec<CatalogOption> {
    let mut options: Vec<CatalogOption> = catalog()
        .into_iter()
        .filter(|option| option.api_key_capable)
        .collect();
    options.sort_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
    });
    options
}

/// Every provider in the registry, by display name.
///
/// This is a catalog of what *could* be configured. It is deliberately never
/// mixed into the configured-connection list: a registry entry is a capability
/// of this build, not an account someone holds.
pub fn catalog() -> Vec<CatalogOption> {
    let mut options: Vec<CatalogOption> = nullrouter_providers::entries()
        .iter()
        .map(|entry| {
            let category = entry
                .category
                .clone()
                .unwrap_or_else(|| String::from("apikey"));
            let display = entry.display.as_ref();
            let name = display
                .and_then(|display| display.name.clone())
                .unwrap_or_else(|| entry.id.clone());
            let api_key_capable = !matches!(category.as_str(), "oauth" | "webCookie");
            CatalogOption {
                initials: display
                    .and_then(|display| display.text_icon.clone())
                    .unwrap_or_else(|| initials_from(&name)),
                accent: display
                    .and_then(|display| display.color.clone())
                    .unwrap_or_else(|| String::from(FALLBACK_ACCENT)),
                model_count: entry.models.len(),
                // `ollama-local` is the one provider the state service lets
                // through without a key (`create_provider`).
                requires_api_key: api_key_capable && entry.id != "ollama-local",
                api_key_capable,
                id: entry.id.clone(),
                name,
                category,
            }
        })
        .collect();
    options.sort_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
    });
    options
}

/// One catalog entry by provider id.
pub fn catalog_option(provider_id: &str) -> Option<CatalogOption> {
    catalog()
        .into_iter()
        .find(|option| option.id == provider_id)
}

/// Accent colour when the registry has no colour for a provider.
const FALLBACK_ACCENT: &str = "#e56a4a";

/// Registry display name for a provider id, or the id itself.
///
/// A connection can name a provider this build does not know (an older state
/// file, a renamed entry). The id is shown as-is rather than hidden, because the
/// connection is real either way.
pub fn provider_label(provider_id: &str) -> String {
    nullrouter_providers::entry(provider_id)
        .and_then(|entry| entry.display.as_ref())
        .and_then(|display| display.name.clone())
        .unwrap_or_else(|| provider_id.to_owned())
}

/// Registry accent colour for a provider id.
pub fn provider_accent(provider_id: &str) -> &'static str {
    nullrouter_providers::entry(provider_id)
        .and_then(|entry| entry.display.as_ref())
        .and_then(|display| display.color.as_deref())
        .unwrap_or(FALLBACK_ACCENT)
}

/// Registry glyph for a provider id, falling back to its initials.
pub fn provider_initials(provider_id: &str) -> String {
    nullrouter_providers::entry(provider_id)
        .and_then(|entry| entry.display.as_ref())
        .and_then(|display| display.text_icon.clone())
        .unwrap_or_else(|| initials_from(&provider_label(provider_id)))
}

// ── requests ────────────────────────────────────────────────────────────────
//
// Thin wrappers over `crate::api`, kept here so the panel holds signals and
// views only. Each one is a single request whose result is already interpreted by
// the functions above, so a caller cannot forget to distinguish "empty" from
// "failed", or "not tested" from "invalid".
//
// `api::request` is itself split on `target_arch`: the wasm arm performs the
// `fetch`, and the native arm returns `ApiError::Environment` rather than
// pretending to have contacted a router. That keeps every function below callable
// — and every branch above testable — on the native target.

/// `GET /api/providers`.
pub async fn load_connections() -> Result<ConnectionList, ApiError> {
    let body = crate::api::get(CONNECTIONS_PATH).await?;
    parse_connections(&body).ok_or(ApiError::Body)
}

/// `POST /api/providers`, returning the row the router created.
pub async fn create_connection(body: String) -> Result<Connection, ApiError> {
    let response = crate::api::post(CONNECTIONS_PATH, &body).await?;
    parse_connection(&response).ok_or(ApiError::Body)
}

/// `DELETE /api/providers/{id}`.
pub async fn delete_connection(id: &str) -> DeleteOutcome {
    match crate::api::delete(&connection_path(id)).await {
        Ok(_body) => DeleteOutcome::Confirmed,
        Err(error) => DeleteOutcome::Rejected(error),
    }
}

/// `POST /api/providers/{id}/test`.
///
/// The body is `{}` because the endpoint takes an optional payload and this
/// dashboard has nothing to add to it.
///
/// Uses [`crate::api::request_detailed`] rather than `post`: a failed test answers
/// `502` with the provider's own message, and `post` would discard that body in
/// favour of a bare status code.
pub async fn test_connection(id: &str) -> TestOutcome {
    let response = crate::api::request_detailed(
        crate::api::Method::Post,
        &connection_test_path(id),
        Some("{}"),
    )
    .await;
    settle_test(response)
}

/// Up to two leading alphanumerics of a name, uppercased.
fn initials_from(name: &str) -> String {
    let glyph: String = name
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .take(2)
        .collect();
    if glyph.is_empty() {
        String::from("··")
    } else {
        glyph.to_uppercase()
    }
}
