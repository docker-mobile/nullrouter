//! Real provider connection tests.
//!
//! `POST /api/providers/{id}/test` used to answer `501 unsupported`. It now performs
//! an actual upstream call through the runtime and reports what happened.
//!
//! The probe is deliberately the smallest billable request that still proves the
//! whole path: one short user turn, `max_tokens: 1`, non-streaming. A test that
//! spent a real completion would make "check my key" an expensive operation, and a
//! test that skipped the provider entirely would only prove the router is up.
//!
//! Two things this must never do: report success it did not observe, and echo a
//! secret. The provider's own error text is relayed so a wrong key says so, but it
//! is scrubbed first — upstream error bodies sometimes quote the offending header.

use std::time::Instant;

use serde::Serialize;
use serde_json::Value;

/// The probe body sent upstream.
///
/// `max_tokens: 1` keeps the cost of a connection test at one token. Some providers
/// reject `max_tokens: 0`, so one is the floor rather than zero.
pub(crate) fn probe_body(model: &str) -> Value {
    serde_json::json!({
        "model": model,
        "max_tokens": 1,
        "stream": false,
        "messages": [{ "role": "user", "content": "ping" }],
    })
}

/// What a connection test observed.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProbeResult {
    /// Whether the provider answered successfully.
    pub success: bool,
    /// The upstream status, when there was one. `None` means the call never landed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// The model the probe used.
    pub model: String,
    /// Round-trip latency in milliseconds.
    pub latency_ms: u64,
    /// A scrubbed message. Present on failure, absent on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Render a result as a JSON object the caller can add its own ids to.
///
/// Going through `to_value` rather than a hand-written `json!` literal is what keeps
/// the `skip_serializing_if` above meaningful: a literal would emit `"error": null`
/// on a pass, and a client testing `"error" in body` would read every success as a
/// failure.
pub(crate) fn to_object(result: &ProbeResult) -> serde_json::Map<String, Value> {
    if let Ok(Value::Object(map)) = serde_json::to_value(result) {
        return map;
    }
    // A struct of scalars cannot fail to serialize; if it somehow did, report the
    // outcome rather than an empty body.
    let mut fallback = serde_json::Map::new();
    fallback.insert("success".to_owned(), result.success.into());
    fallback.insert("model".to_owned(), result.model.clone().into());
    fallback
}

/// Strip anything secret-shaped from text bound for a client.
///
/// Upstream error bodies sometimes quote the header that failed, which means the key
/// itself can appear in a message this route is about to relay. Bearer tokens and
/// long key-shaped runs are replaced rather than the whole body being suppressed:
/// the provider's actual complaint is the useful part of a failed test.
pub(crate) fn scrub_secrets(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for token in text.split_inclusive(|character: char| {
        character.is_whitespace() || character == '"' || character == ','
    }) {
        let trimmed = token.trim_end_matches(['"', ',', ' ', '\n', '\r', '\t']);
        if looks_secret(trimmed) {
            out.push_str("[redacted]");
            // Keep whatever delimiter followed, so the message still reads.
            if let Some(tail) = token.strip_prefix(trimmed) {
                out.push_str(tail);
            }
        } else {
            out.push_str(token);
        }
    }
    out
}

/// Whether a token looks like a credential rather than prose.
///
/// Deliberately broad: a false positive costs a redacted word in an error message,
/// while a false negative leaks a key into a dashboard panel.
fn looks_secret(token: &str) -> bool {
    const PREFIXES: [&str; 6] = ["sk-", "sk_", "pk-", "Bearer", "bearer", "ghp_"];
    if PREFIXES.iter().any(|prefix| token.starts_with(prefix)) && token.len() >= 8 {
        return true;
    }
    // A long unbroken run of key-ish characters is treated as a secret even without
    // a known prefix: most vendors use their own.
    token.len() >= 24
        && token
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        && token.chars().any(char::is_numeric)
        && token.chars().any(char::is_alphabetic)
}

/// Read a human-usable message out of an upstream error body.
///
/// Falls back to a bounded slice of the raw body when it is not the shape any
/// vendor documents, because an opaque body is still more useful than "failed".
pub(crate) fn error_message(status: u16, body: &str) -> String {
    const MAX_RELAYED: usize = 400;
    let parsed = serde_json::from_str::<Value>(body).ok();
    let message = parsed.as_ref().and_then(|value| {
        value
            .pointer("/error/message")
            .or_else(|| value.pointer("/error"))
            .or_else(|| value.get("message"))
            .map(|found| match found {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            })
    });
    let raw = message.unwrap_or_else(|| {
        let trimmed = body.trim();
        if trimmed.is_empty() {
            format!("provider returned {status} with an empty body")
        } else {
            trimmed.chars().take(MAX_RELAYED).collect()
        }
    });
    scrub_secrets(&raw)
}

/// The model string to probe a connection with.
///
/// Prefers the connection's own `defaultModel`, then the registry's first model for
/// that provider. `None` when neither exists — a provider with no known model cannot
/// be probed, and inventing an id would report "model not found" as a connection
/// failure and send the user to debug the wrong thing.
///
/// The result is always `provider/model` so the runtime routes to this provider
/// rather than inferring one from a bare id.
pub(crate) fn probe_model(connection: &Value) -> Option<String> {
    let provider = connection
        .get("provider")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|provider| !provider.is_empty())?;

    let configured = connection
        .get("defaultModel")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty());
    if let Some(model) = configured {
        // A configured model may already carry its provider prefix.
        if model.contains('/') {
            return Some(model.to_owned());
        }
        return Some(format!("{provider}/{model}"));
    }

    let first = nullrouter_providers::models_for_provider(provider)
        .first()
        .map(|model| model.id.clone())?;
    Some(format!("{provider}/{first}"))
}

/// Turn a runtime reply into a probe result.
pub(crate) fn settle(model: &str, started: Instant, reply: Option<(u16, String)>) -> ProbeResult {
    let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let Some((status, body)) = reply else {
        return ProbeResult {
            success: false,
            status: None,
            model: model.to_owned(),
            latency_ms,
            error: Some(String::from(
                "the runtime service is unreachable, so the provider was never called",
            )),
        };
    };
    // Only a 2xx is a pass. A 501 from the runtime means the provider needs an
    // executor this build does not have, which is a real failure for a test that
    // claims to prove the connection works.
    if (200..300).contains(&status) {
        return ProbeResult {
            success: true,
            status: Some(status),
            model: model.to_owned(),
            latency_ms,
            error: None,
        };
    }
    ProbeResult {
        success: false,
        status: Some(status),
        model: model.to_owned(),
        latency_ms,
        error: Some(error_message(status, &body)),
    }
}

#[cfg(test)]
mod tests {
    use super::{error_message, probe_body, scrub_secrets, settle};
    use std::time::Instant;

    #[test]
    fn the_probe_costs_one_token_and_does_not_stream() {
        // A connection test that spent a real completion would make "check my key"
        // an expensive operation.
        let body = probe_body("openai/gpt-5");
        assert_eq!(body.get("max_tokens"), Some(&serde_json::json!(1)));
        assert_eq!(body.get("stream"), Some(&serde_json::json!(false)));
        assert_eq!(body.get("model"), Some(&serde_json::json!("openai/gpt-5")));
    }

    #[test]
    fn a_bearer_token_never_survives_into_a_relayed_message() {
        // Upstream bodies sometimes quote the header that failed.
        let leaked = "Incorrect API key provided: sk-abc123def456ghi789jkl. Check your key.";
        let scrubbed = scrub_secrets(leaked);
        assert!(!scrubbed.contains("sk-abc123def456ghi789jkl"), "{scrubbed}");
        assert!(scrubbed.contains("[redacted]"), "{scrubbed}");
        // The provider's actual complaint survives, or the test result is useless.
        assert!(
            scrubbed.contains("Incorrect API key provided"),
            "{scrubbed}"
        );

        for secret in [
            "Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9",
            "ghp_16C7e42F292c6912E7710c838347Ae178B4a",
            "sk_live_51H8xKzLkdIwHu7ix",
        ] {
            let scrubbed = scrub_secrets(&format!("failed with {secret} today"));
            assert!(!scrubbed.contains(secret), "{secret} leaked: {scrubbed}");
        }
    }

    #[test]
    fn ordinary_prose_is_not_redacted() {
        // A false positive costs a word; being too eager would make every message
        // unreadable.
        for plain in [
            "Rate limit exceeded, retry in 30 seconds",
            "model not found: gpt-4o-mini",
            "insufficient_quota",
        ] {
            assert_eq!(scrub_secrets(plain), plain);
        }
    }

    #[test]
    fn an_error_message_is_read_from_any_common_shape() {
        assert_eq!(
            error_message(401, r#"{"error":{"message":"bad key"}}"#),
            "bad key"
        );
        assert_eq!(error_message(401, r#"{"error":"bad key"}"#), "bad key");
        assert_eq!(error_message(401, r#"{"message":"bad key"}"#), "bad key");
        // An opaque body is still relayed, bounded, rather than becoming "failed".
        assert_eq!(error_message(500, "upstream exploded"), "upstream exploded");
        assert!(error_message(500, "").contains("500"));
        // A very long body is truncated rather than relayed whole.
        let long = "x".repeat(5000);
        assert!(error_message(500, &long).len() <= 400);
    }

    #[test]
    fn only_a_2xx_counts_as_a_passing_test() {
        let ok = settle("m", Instant::now(), Some((200, String::from("{}"))));
        assert!(ok.success);
        assert_eq!(ok.error, None, "a pass must not carry an error");

        // A 501 means the provider needs an unported executor. Reporting that as a
        // pass would tell the user a connection works when no request can use it.
        let unported = settle(
            "m",
            Instant::now(),
            Some((
                501,
                String::from(r#"{"error":{"message":"needs an executor"}}"#),
            )),
        );
        assert!(!unported.success);
        assert_eq!(unported.status, Some(501));
        assert_eq!(unported.error.as_deref(), Some("needs an executor"));
    }

    #[test]
    fn an_unreachable_runtime_is_reported_as_never_called() {
        // Distinct from a provider failure: the user's credentials were not tested
        // at all, and saying "failed" would send them to debug the wrong thing.
        let result = settle("m", Instant::now(), None);
        assert!(!result.success);
        assert_eq!(result.status, None);
        assert!(
            result
                .error
                .as_deref()
                .is_some_and(|error| error.contains("never called")),
            "{:?}",
            result.error
        );
    }
}
