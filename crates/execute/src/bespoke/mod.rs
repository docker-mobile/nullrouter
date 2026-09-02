//! Per-provider request shaping that the generic executor cannot express.
//!
//! Most providers differ only in their URL, auth descriptor, and wire format, all
//! of which the registry describes. A few need something the registry has no field
//! for — an envelope around the body, a per-request header, a URL whose path
//! depends on whether the request streams. Upstream expresses those as executor
//! subclasses (`open-sse/executors/*.js`); here they are three small hooks applied
//! by [`crate::Executor::execute`], so the shared retry, fallback, and streaming
//! machinery stays in one place.

use std::fmt::Write as _;

use nullrouter_providers::{Format, registry, target_format};
use serde_json::{Value, json};

use crate::credentials::Credentials;

pub(crate) mod grok_web;

/// The `x-session-id` header `CommandCode` expects on every request.
const SESSION_HEADER: &str = "x-session-id";

/// Wrap a request body in the envelope its provider requires.
///
/// Returns `None` when the body goes out as-is, which is the common case.
pub(crate) fn envelope(provider: &str, body: &Value, credentials: &Credentials) -> Option<Value> {
    // grok.com takes a payload of its own rather than a chat-completions body, so it replaces the
    // body outright instead of wrapping it.
    if target_format(provider) == Format::GrokWeb {
        let messages = body
            .get("messages")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        let model = body
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mapping = grok_web::model_for(model);
        return Some(grok_web::payload(
            &mapping,
            &grok_web::flatten_messages(messages),
        ));
    }
    if target_format(provider) != Format::GeminiCli {
        return None;
    }
    // Cloud Code Assist wraps the Gemini payload: `{ project, model, request }`.
    // A body already in that shape is left alone, so a retry does not double-wrap.
    if body.get("request").is_some() && body.get("model").is_some() {
        return None;
    }
    let model = body.get("model").cloned().unwrap_or(Value::Null);
    let project = credentials
        .setting("projectId")
        .map(|project| json!(project))
        .or_else(|| body.get("project").cloned())
        .unwrap_or(Value::Null);
    Some(json!({ "project": project, "model": model, "request": body }))
}

/// Headers that replace the registry's auth for providers whose credential is not a header token.
///
/// Returns `None` for every provider whose auth a descriptor can express, which is nearly all of them.
/// A `Some` result causes the caller to strip `Authorization` and `x-api-key` first: a web endpoint
/// authenticated by cookie rejects a request that also carries a bearer token.
pub(crate) fn auth_override(
    provider: &str,
    credentials: &Credentials,
) -> Option<Vec<(String, String)>> {
    if target_format(provider) != Format::GrokWeb {
        return None;
    }
    // grok.com authenticates with the `sso` cookie from a signed-in browser session. The user pastes
    // it as an API key because that is the field a panel offers, but it is a cookie on the wire.
    let token = credentials
        .api_key
        .as_deref()
        .or(credentials.access_token.as_deref())?;
    Some(vec![("Cookie".to_owned(), grok_web::cookie_header(token))])
}

/// Headers this provider needs beyond the registry's own.
///
/// Applied after `build_headers`, so these win: a provider that needs a specific
/// `User-Agent` is not served by the generic one.
pub(crate) fn extra_headers(provider: &str, model: &str, stream: bool) -> Vec<(String, String)> {
    match target_format(provider) {
        Format::GeminiCli => {
            let transport = registry::transport(provider);
            let cli_version = transport
                .and_then(|transport| transport.cli_version.as_deref())
                .unwrap_or("0.0.0");
            let api_client = transport
                .and_then(|transport| transport.api_client.as_deref())
                .unwrap_or_default();
            let mut headers = vec![
                // Cloud Code Assist keys quota off the CLI's own user agent, so the
                // generic reqwest one is not interchangeable.
                (
                    "User-Agent".to_owned(),
                    format!("GeminiCLI/{cli_version} (model: {model})"),
                ),
                (
                    "Accept".to_owned(),
                    if stream {
                        "text/event-stream".to_owned()
                    } else {
                        "application/json".to_owned()
                    },
                ),
            ];
            if !api_client.is_empty() {
                headers.push(("X-Goog-Api-Client".to_owned(), api_client.to_owned()));
            }
            headers
        }
        // A correlation id per request. Upstream sends a fresh UUID on every call,
        // so one is minted rather than reused across a connection.
        Format::CommandCode => vec![(SESSION_HEADER.to_owned(), session_id())],
        // grok.com needs the browser headers its own front end sends. The credential is a session
        // cookie rather than a bearer token, so it is added by `auth_override` instead of here — this
        // hook has no access to credentials.
        Format::GrokWeb => grok_web::headers(None),
        _ => Vec::new(),
    }
}

/// The suffix appended to a provider's base URL for this request.
///
/// Cloud Code Assist selects the method in the URL, and the streaming method needs
/// `alt=sse` or it answers with a single JSON blob.
pub(crate) fn url_suffix(provider: &str, stream: bool) -> Option<&'static str> {
    if target_format(provider) != Format::GeminiCli {
        return None;
    }
    Some(if stream {
        ":streamGenerateContent?alt=sse"
    } else {
        ":generateContent"
    })
}

/// A random v4 UUID.
///
/// A counter would be cheaper, but a session id that collides across processes
/// makes two users' requests indistinguishable in a provider's own logs, which is
/// exactly what the header exists to prevent.
fn session_id() -> String {
    let mut bytes = [0_u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        // No entropy source: fall back to a time-derived id rather than sending a
        // fixed one, which would make every request look like the same session.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        bytes = nanos.to_be_bytes();
    }
    // Version 4, variant 1, per RFC 4122.
    if let Some(byte) = bytes.get_mut(6) {
        *byte = (*byte & 0x0F) | 0x40;
    }
    if let Some(byte) = bytes.get_mut(8) {
        *byte = (*byte & 0x3F) | 0x80;
    }
    let hex = bytes
        .iter()
        .fold(String::with_capacity(32), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        });
    [
        hex.get(..8),
        hex.get(8..12),
        hex.get(12..16),
        hex.get(16..20),
        hex.get(20..32),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("-")
}

#[cfg(test)]
mod tests {
    use super::{envelope, extra_headers, session_id, url_suffix};
    use crate::credentials::Credentials;
    use serde_json::json;

    fn with_project(project: &str) -> Credentials {
        let mut credentials = Credentials::default();
        credentials
            .provider_specific_data
            .insert("projectId".to_owned(), json!(project));
        credentials
    }

    #[test]
    fn gemini_cli_bodies_are_wrapped_in_the_cloud_code_envelope() {
        let body = json!({ "model": "gemini-2.5-pro", "contents": [] });
        let wrapped = envelope("gemini-cli", &body, &with_project("proj-1")).expect("wrapped");

        assert_eq!(wrapped.get("project"), Some(&json!("proj-1")));
        assert_eq!(wrapped.get("model"), Some(&json!("gemini-2.5-pro")));
        // The original payload becomes `request`, untouched.
        assert_eq!(wrapped.get("request"), Some(&body));
    }

    #[test]
    fn an_already_wrapped_body_is_not_wrapped_again() {
        // A retry must not produce `{ request: { request: … } }`.
        let wrapped = json!({ "project": "p", "model": "m", "request": { "contents": [] } });
        assert!(envelope("gemini-cli", &wrapped, &Credentials::default()).is_none());
    }

    #[test]
    fn a_project_falls_back_to_the_body_when_the_connection_names_none() {
        let body = json!({ "model": "m", "project": "from-body" });
        let wrapped = envelope("gemini-cli", &body, &Credentials::default()).expect("wrapped");
        assert_eq!(wrapped.get("project"), Some(&json!("from-body")));
    }

    #[test]
    fn other_providers_are_not_enveloped() {
        let body = json!({ "model": "gpt-5" });
        assert!(envelope("openai", &body, &Credentials::default()).is_none());
        assert!(envelope("anthropic", &body, &Credentials::default()).is_none());
        assert!(envelope("commandcode", &body, &Credentials::default()).is_none());
    }

    #[test]
    fn gemini_cli_sends_the_cli_user_agent_and_api_client() {
        let headers = extra_headers("gemini-cli", "gemini-2.5-pro", true);
        let agent = headers
            .iter()
            .find(|(name, _)| name == "User-Agent")
            .map(|(_, value)| value.clone())
            .expect("a user agent");
        // Quota is keyed off this, so the model has to appear in it.
        assert!(agent.starts_with("GeminiCLI/"), "got {agent}");
        assert!(agent.contains("gemini-2.5-pro"), "got {agent}");
        assert!(
            headers.iter().any(|(name, _)| name == "X-Goog-Api-Client"),
            "got {headers:?}"
        );
        // Streaming and non-streaming ask for different content types.
        assert!(
            headers
                .iter()
                .any(|(name, value)| name == "Accept" && value == "text/event-stream")
        );
        assert!(
            extra_headers("gemini-cli", "m", false)
                .iter()
                .any(|(name, value)| name == "Accept" && value == "application/json")
        );
    }

    #[test]
    fn commandcode_gets_a_fresh_session_id_per_request() {
        let first = extra_headers("commandcode", "cc", true);
        let second = extra_headers("commandcode", "cc", true);
        let read = |headers: &[(String, String)]| {
            headers
                .iter()
                .find(|(name, _)| name == "x-session-id")
                .map(|(_, value)| value.clone())
                .expect("a session id")
        };
        let (a, b) = (read(&first), read(&second));
        // A reused id makes two requests indistinguishable in the provider's logs.
        assert_ne!(a, b, "the session id must not be reused");
        // v4 UUID shape.
        assert_eq!(a.len(), 36, "got {a}");
        assert_eq!(a.split('-').count(), 5, "got {a}");
    }

    #[test]
    fn providers_with_no_hook_get_no_extra_headers() {
        assert!(extra_headers("openai", "gpt-5", true).is_empty());
        assert!(extra_headers("anthropic", "claude-sonnet-4-5", false).is_empty());
    }

    #[test]
    fn gemini_cli_selects_its_method_in_the_url() {
        // Without `alt=sse` the streaming method answers with one JSON blob.
        assert_eq!(
            url_suffix("gemini-cli", true),
            Some(":streamGenerateContent?alt=sse")
        );
        assert_eq!(url_suffix("gemini-cli", false), Some(":generateContent"));
        assert_eq!(url_suffix("openai", true), None);
    }

    #[test]
    fn session_ids_are_v4_shaped() {
        let id = session_id();
        let fields: Vec<&str> = id.split('-').collect();
        assert_eq!(
            fields.iter().map(|field| field.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12],
            "got {id}"
        );
        // Version nibble.
        assert!(
            fields.get(2).is_some_and(|field| field.starts_with('4')),
            "got {id}"
        );
        // Variant nibble.
        assert!(
            fields.get(3).is_some_and(|field| matches!(
                field.as_bytes().first(),
                Some(b'8' | b'9' | b'a' | b'b')
            )),
            "got {id}"
        );
        assert!(id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
    }
}
