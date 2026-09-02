//! Request and response handling for `cursor`, whose API is Connect-RPC carrying protobuf.
//!
//! Ports `open-sse/executors/cursor.js`. This is the only provider here that is not JSON on the wire, so
//! it is the one place a hook returning a `Value` cannot serve: [`body`] returns bytes, and the executor
//! sends those instead of serialising the request.
//!
//! Cursor's schema is unpublished. Every field number in [`request`] and [`response`] was established by
//! observing the IDE, several fields carry no known meaning, and a Cursor release can add one at any time —
//! so the decoder tolerates unknown fields rather than refusing them.
//!
//! **Two endpoints, one ported.** Upstream sends plain-text turns to `AgentService` and anything with tool
//! calls to the older `ChatService`. `AgentService` is HTTP/2 duplex: the client sends a run request, the
//! server asks for IDE file context mid-stream, and the client must answer on the same open stream before
//! the response continues. That needs a bidirectional stream this executor's request/response shape has no
//! room for, so only the `ChatService` path is ported here. Its request builder is complete, including the
//! tool encoding, and [`prefers_agent_service`] reports when a request would have taken the other path so
//! the caller can say so rather than fail obscurely.

pub(crate) mod headers;
pub(crate) mod protobuf;
pub(crate) mod request;
pub(crate) mod response;

use serde_json::Value;

use crate::credentials::Credentials;

/// Whether upstream would have sent this request to `AgentService` rather than `ChatService`.
///
/// True for a plain-text conversation. Cursor retired `ChatService`, and it rejects a request carrying tool
/// schemas — which many clients attach even to a plain turn — so upstream routes those to the newer
/// endpoint. That endpoint needs HTTP/2 duplex, which is not ported; this reports the condition so a caller
/// can name it.
pub fn prefers_agent_service(body: &Value) -> bool {
    body.get("messages")
        .and_then(Value::as_array)
        .is_some_and(|messages| request::is_plain_text(messages))
}

/// The framed protobuf body for a Cursor request.
pub(crate) fn body(body: &Value, ids: &request::Ids) -> Vec<u8> {
    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let tools = body
        .get("tools")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let reasoning = body.get("reasoning_effort").and_then(Value::as_str);
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();
    // Upstream reads the inbound `user-agent` to force agent mode for Claude Code. That header does not
    // reach this hook, so an explicit body flag stands in; a client that sends tools gets agent mode
    // anyway.
    let force_agent = body
        .get("nullrouter_force_agent")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    request::build(messages, model, tools, reasoning, force_agent, ids)
}

/// The headers a Cursor request carries.
///
/// The machine id is the connection's own when it has one, and derived from the token otherwise. It is not
/// invented per request: Cursor treats it as a device identity, and a new one each time looks like a new
/// device on every call.
pub(crate) fn request_headers(
    credentials: &Credentials,
    ids: &Nonces,
    millis: u128,
) -> Vec<(String, String)> {
    let token = credentials
        .access_token
        .as_deref()
        .or(credentials.api_key.as_deref())
        .unwrap_or_default();
    let machine_id = credentials
        .setting("machineId")
        .map_or_else(|| headers::derived_machine_id(token), str::to_owned);
    // Cursor's privacy switch. Defaults on, and only an explicit `false` turns it off — with it off Cursor
    // may retain the conversation, which is not this router's decision to make for a user.
    let ghost_mode = credentials
        .provider_specific_data
        .get("ghostMode")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    headers::build(&headers::Request {
        token,
        machine_id: &machine_id,
        ghost_mode,
        nonces: ids,
        millis,
    })
}

/// The per-request identifiers Cursor's headers carry.
///
/// Held in a struct so a test can pin them; they are random in a real request.
#[derive(Debug, Clone)]
pub(crate) struct Nonces {
    /// `x-request-id`.
    pub(crate) request_id: String,
    /// `x-cursor-config-version`.
    pub(crate) config_version: String,
    /// `x-amzn-trace-id`, without the `Root=` prefix.
    pub(crate) trace_id: String,
}

impl Nonces {
    /// Fresh identifiers for one request.
    pub(crate) fn generate() -> Self {
        Self {
            request_id: super::session_id(),
            config_version: super::session_id(),
            trace_id: super::session_id(),
        }
    }
}

impl request::Ids {
    /// Fresh identifiers for one request's message set.
    pub(crate) fn generate(message_count: usize, millis: u128) -> Self {
        Self {
            per_message: (0..message_count)
                .map(|_index| super::session_id())
                .collect(),
            conversation_id: super::session_id(),
            timestamp: iso8601(millis),
        }
    }
}

/// An ISO-8601 timestamp with milliseconds, as JavaScript's `toISOString` writes one.
///
/// Written out rather than pulling in a date crate for one field: Cursor reads it as an opaque string, and
/// the conversion from a Unix millisecond count is a fixed calculation.
fn iso8601(millis: u128) -> String {
    let total_seconds = millis / 1000;
    let sub_milli = millis % 1000;
    let days = total_seconds / 86_400;
    let seconds_today = total_seconds % 86_400;
    let (hour, minute, second) = (
        seconds_today / 3600,
        (seconds_today % 3600) / 60,
        seconds_today % 60,
    );
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{sub_milli:03}Z")
}

/// Days since the Unix epoch to a calendar date.
///
/// Howard Hinnant's `civil_from_days`, which is the standard closed form for this and handles leap years
/// and centuries without a table.
const fn civil_from_days(days: u128) -> (u128, u128, u128) {
    // Shift the era so that March is month 0, which removes the leap-day special case from the arithmetic.
    let shifted = days.saturating_add(719_468);
    let era = shifted / 146_097;
    let day_of_era = shifted % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Nonces, body, iso8601, prefers_agent_service, request, request_headers};
    use crate::credentials::Credentials;

    fn credentials() -> Credentials {
        Credentials {
            access_token: Some("user_01ABC::the-token".to_owned()),
            connection_id: "conn_cursor".to_owned(),
            ..Credentials::default()
        }
    }

    #[test]
    fn a_plain_turn_is_recognised_as_belonging_to_the_newer_endpoint() {
        // Reported rather than silently mishandled: `AgentService` needs HTTP/2 duplex, which is not ported.
        assert!(prefers_agent_service(&json!({
            "messages": [{ "role": "user", "content": "hi" }],
        })));
        assert!(!prefers_agent_service(&json!({
            "messages": [{ "role": "tool", "content": "result" }],
        })));
    }

    #[test]
    fn the_body_is_bytes_rather_than_json() {
        // Cursor is the one provider here that is not JSON on the wire.
        let request_body = json!({
            "model": "claude-4.5-sonnet",
            "messages": [{ "role": "user", "content": "hi" }],
        });
        let ids = request::Ids {
            per_message: vec!["m0".to_owned()],
            conversation_id: "c0".to_owned(),
            timestamp: "2026-09-01T00:00:00.000Z".to_owned(),
        };
        let bytes = body(&request_body, &ids);
        // A Connect frame: a zero flag byte then a big-endian length.
        assert_eq!(bytes.first(), Some(&0x00));
        assert!(bytes.len() > 5);
    }

    #[test]
    fn the_machine_id_comes_from_the_connection_when_it_has_one() {
        // Cursor treats it as a device identity, so a fresh one per request looks like a new device each
        // call.
        let mut with_id = credentials();
        with_id
            .provider_specific_data
            .insert("machineId".to_owned(), json!("stored-machine-id"));
        let nonces = Nonces {
            request_id: "req".to_owned(),
            config_version: "cfg".to_owned(),
            trace_id: "trace".to_owned(),
        };
        let read = |headers: &[(String, String)], name: &str| {
            headers
                .iter()
                .find(|(key, _value)| key == name)
                .map(|(_key, value)| value.clone())
                .unwrap_or_default()
        };
        let stored = request_headers(&with_id, &nonces, 1_700_000_000_000);
        assert!(
            read(&stored, "x-cursor-checksum").ends_with("stored-machine-id"),
            "got {}",
            read(&stored, "x-cursor-checksum")
        );

        // With none stored, one is derived from the token — deterministically, so it is stable per account.
        let derived = request_headers(&credentials(), &nonces, 1_700_000_000_000);
        let again = request_headers(&credentials(), &nonces, 1_700_000_000_000);
        assert_eq!(
            read(&derived, "x-cursor-checksum"),
            read(&again, "x-cursor-checksum")
        );
        assert_ne!(
            read(&derived, "x-cursor-checksum"),
            read(&stored, "x-cursor-checksum")
        );
        // The prefix is stripped from the bearer token, or every derived value disagrees with it.
        assert_eq!(read(&derived, "authorization"), "Bearer the-token");
    }

    #[test]
    fn ghost_mode_is_on_unless_the_connection_turns_it_off() {
        let nonces = Nonces {
            request_id: "req".to_owned(),
            config_version: "cfg".to_owned(),
            trace_id: "trace".to_owned(),
        };
        let read = |credentials: &Credentials| {
            request_headers(credentials, &nonces, 0)
                .iter()
                .find(|(key, _value)| key == "x-ghost-mode")
                .map(|(_key, value)| value.clone())
                .unwrap_or_default()
        };
        assert_eq!(read(&credentials()), "true");

        let mut off = credentials();
        off.provider_specific_data
            .insert("ghostMode".to_owned(), json!(false));
        assert_eq!(read(&off), "false");

        // Anything other than an explicit `false` leaves it on: with it off Cursor may retain the
        // conversation, which is not this router's call to make for a user.
        let mut odd = credentials();
        odd.provider_specific_data
            .insert("ghostMode".to_owned(), json!("no"));
        assert_eq!(read(&odd), "true");
    }

    #[test]
    fn nonces_differ_between_requests() {
        let first = Nonces::generate();
        let second = Nonces::generate();
        assert_ne!(first.request_id, second.request_id);
        assert_ne!(first.trace_id, second.trace_id);
        // And a request id is not reused as the trace id within one request.
        assert_ne!(first.request_id, first.trace_id);
    }

    #[test]
    fn timestamps_are_iso8601_with_milliseconds() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso8601(1_700_000_000_123), "2023-11-14T22:13:20.123Z");
        // A leap day, which is where a hand-rolled conversion usually breaks.
        assert_eq!(iso8601(1_709_164_800_000), "2024-02-29T00:00:00.000Z");
        // And a century boundary that is not a leap year in the Gregorian calendar.
        assert_eq!(iso8601(4_102_444_800_000), "2100-01-01T00:00:00.000Z");
    }
}
