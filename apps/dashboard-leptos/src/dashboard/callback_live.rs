//! Provider OAuth callback state.
//!
//! This screen was 59 lines of inline JavaScript in the actix host's callback
//! shell. It is the page a provider redirects back to after an authorization
//! grant: it relays the `code`/`token` to whatever started the flow, then tells
//! the user they can close the tab.
//!
//! The relay is the interesting part, and it is why these derivations are worth
//! having as tested Rust: the payload contains an authorization code, and
//! [`relay_origins`] decides who is allowed to receive it.
//!
//! Note that the provider OAuth *flows* themselves (`/api/oauth/*`) are still
//! refused with a 501. This page is the landing surface for them, ported here so
//! the frontend is uniformly WASM rather than because the flows work yet.

use serde::{Deserialize, Serialize};

/// The loopback helper upstream relays to besides this origin.
///
/// Some vendor CLIs listen here for the grant. It is a fixed loopback port, never
/// a remote host, which is the only reason a second recipient is acceptable at
/// all — an authorization code is being handed over.
pub const HELPER_ORIGIN: &str = "http://localhost:1455";

/// The `BroadcastChannel` name and `localStorage` key the relay uses.
pub const RELAY_CHANNEL: &str = "oauth_callback";

/// The message type an opener window receives.
pub const RELAY_MESSAGE_TYPE: &str = "oauth_callback";

/// What a provider sent back on the callback URL.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallbackData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_description: Option<String>,
    /// The full callback URL, for the manual-copy fallback.
    pub full_url: String,
}

impl CallbackData {
    /// Whether the provider returned anything actionable.
    ///
    /// An `error` counts: the flow did conclude, and whatever started it needs to
    /// hear that it failed rather than waiting forever.
    pub fn is_conclusive(&self) -> bool {
        self.code.is_some() || self.token.is_some() || self.error.is_some()
    }
}

/// Read callback data from a query string and the page's own URL.
///
/// Empty parameters read as absent: a provider that sends `?code=` has not sent a
/// code, and treating the empty string as one would relay a grant of nothing.
pub fn parse_callback(query: &str, full_url: &str) -> CallbackData {
    let value = |name: &str| query_value(query, name);
    CallbackData {
        code: value("code"),
        token: value("token"),
        state: value("state"),
        error: value("error"),
        error_description: value("error_description"),
        full_url: full_url.to_owned(),
    }
}

/// One query parameter, percent-decoded. `None` when absent or blank.
///
/// Hand-rolled rather than pulling a URL crate into the WASM bundle for four
/// fields. Matches `URLSearchParams`: `+` is a space, `%XX` decodes, an invalid
/// escape is left as written rather than dropping the value.
fn query_value(query: &str, name: &str) -> Option<String> {
    query
        .trim_start_matches('?')
        .split('&')
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (percent_decode(key) == name).then(|| percent_decode(value))
        })
        .find(|value| !value.trim().is_empty())
}

/// Percent-decode a query component, treating `+` as a space.
fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while let Some(byte) = bytes.get(index) {
        match byte {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' => {
                // A truncated or non-hex escape is kept verbatim: dropping it
                // would silently alter a code or state value.
                let decoded = bytes
                    .get(index + 1..index + 3)
                    .and_then(|pair| std::str::from_utf8(pair).ok())
                    .and_then(|pair| u8::from_str_radix(pair, 16).ok());
                match decoded {
                    Some(value) => {
                        out.push(value);
                        index += 3;
                    }
                    None => {
                        out.push(b'%');
                        index += 1;
                    }
                }
            }
            other => {
                out.push(*other);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Which origins may receive the relayed grant.
///
/// This page's own origin, plus the fixed loopback helper. Deliberately not `"*"`:
/// the payload carries an authorization code, and a wildcard `postMessage` target
/// would hand it to any window that happened to open this page.
///
/// The helper is omitted when it is already this origin, so the message is not
/// delivered twice to the same recipient.
pub fn relay_origins(own_origin: &str) -> Vec<String> {
    let mut origins = Vec::with_capacity(2);
    if !own_origin.trim().is_empty() {
        origins.push(own_origin.to_owned());
    }
    if own_origin != HELPER_ORIGIN {
        origins.push(String::from(HELPER_ORIGIN));
    }
    origins
}

/// Which panel the screen shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Panel {
    /// Still relaying.
    #[default]
    Processing,
    /// Relayed; the tab can close.
    Success,
    /// Nothing actionable arrived, so the user copies the URL by hand.
    ManualCopy,
}

/// The panel to show for this callback.
pub fn panel_for(data: &CallbackData) -> Panel {
    if data.is_conclusive() {
        Panel::Success
    } else {
        Panel::ManualCopy
    }
}

/// The relay payload, as JSON.
///
/// Serialised through `serde_json` so a code or state containing a quote cannot
/// break out of the payload.
pub fn relay_payload(data: &CallbackData, timestamp_ms: i64) -> String {
    let mut value = serde_json::to_value(data).unwrap_or_else(|_error| serde_json::json!({}));
    if let Some(object) = value.as_object_mut() {
        object.insert("timestamp".to_owned(), serde_json::json!(timestamp_ms));
    }
    serde_json::to_string(&value).unwrap_or_else(|_error| String::from("{}"))
}

/// The `postMessage` envelope an opener window receives.
pub fn relay_envelope(data: &CallbackData) -> String {
    let envelope = serde_json::json!({
        "type": RELAY_MESSAGE_TYPE,
        "data": data,
    });
    serde_json::to_string(&envelope).unwrap_or_else(|_error| String::from("{}"))
}
