//! Removing credentials from a log line before it is shown to anyone.
//!
//! Upstream does not do this. Its console pane shows whatever `console.log` produced, so anything that
//! logs a token — an error path printing a failed request's headers, a debug line dumping a body — puts
//! that token in a browser tab, and from there into a screenshot or a bug report. The pane is useful
//! enough to keep and that failure mode is not acceptable, so lines are scrubbed here instead.
//!
//! Two design choices worth stating:
//!
//! * **Scrubbing happens at the source.** The layer in this crate scrubs before the line enters the
//!   channel, so a credential never crosses even a loopback socket. The state service scrubs again at
//!   ingest, because anything may post to that endpoint and defence at one end only is not defence.
//! * **The patterns match the *shape* of a credential, not a list of providers.** A list would be
//!   wrong the moment a provider is added. What is recognised is `Bearer <token>`, a header that names
//!   a credential, a JSON field that names one, and the two token shapes that are self-identifying
//!   (an `sk-`-style key and a JWT).
//!
//! What is deliberately *not* attempted: finding a bare high-entropy string with no marker around it.
//! A 40-character hex blob may be an API key or a commit hash, and redacting every one of them would
//! make the pane useless for reading a stack trace. The markers above are what a leak actually looks
//! like, since a credential reaches a log by being formatted next to its own name.

/// What replaces a redacted value.
const REDACTED: &str = "[redacted]";

/// The shortest run of characters after a marker that is worth redacting.
///
/// Below this a match is more likely a placeholder (`Bearer x`, `token: ""`) than a credential, and
/// redacting it costs readability without protecting anything.
const MIN_SECRET: usize = 8;

/// Header and field names whose value is a credential.
///
/// Matched case-insensitively against both `Name: value` and `"name": "value"` forms.
const CREDENTIAL_NAMES: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "cookie",
    "set-cookie",
    "x-api-key",
    "x-goog-api-key",
    "api-key",
    "api_key",
    "apikey",
    "access_token",
    "accesstoken",
    "refresh_token",
    "refreshtoken",
    "id_token",
    "idtoken",
    "client_secret",
    "clientsecret",
    "password",
    "secret",
    "session_token",
    "sessiontoken",
    "x-cursor-checksum",
    "x-client-key",
];

/// Remove anything credential-shaped from a log line.
///
/// Returns the line unchanged when nothing matched, which is the common case — the allocation only
/// happens for a line that needed changing.
pub fn scrub(line: &str) -> String {
    let mut out = line.to_owned();
    out = scrub_bearer(&out);
    out = scrub_named(&out);
    out = scrub_self_identifying(&out);
    out
}

/// Whether a line still contains something credential-shaped.
///
/// For tests and for an assertion at a boundary; the scrubber is not expected to leave anything.
#[must_use]
pub fn looks_clean(line: &str) -> bool {
    scrub(line) == line
}

/// `Bearer <token>` in any casing.
fn scrub_bearer(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    let mut out = String::with_capacity(line.len());
    let mut cursor = 0_usize;
    while let Some(found) = lower.get(cursor..).and_then(|rest| rest.find("bearer ")) {
        let start = cursor.saturating_add(found);
        let value_start = start.saturating_add("bearer ".len());
        let Some(rest) = line.get(value_start..) else {
            break;
        };
        let token_len = rest
            .find(|character: char| !is_token_char(character))
            .unwrap_or(rest.len());
        if token_len < MIN_SECRET {
            // A placeholder rather than a credential; leave it readable.
            let next = value_start.saturating_add(token_len.max(1));
            out.push_str(line.get(cursor..next).unwrap_or_default());
            cursor = next;
            continue;
        }
        out.push_str(line.get(cursor..value_start).unwrap_or_default());
        out.push_str(REDACTED);
        cursor = value_start.saturating_add(token_len);
    }
    out.push_str(line.get(cursor..).unwrap_or_default());
    out
}

/// `Authorization: …`, `"api_key": "…"`, `api_key=…` — a name that means "credential" followed by one.
fn scrub_named(line: &str) -> String {
    let mut out = line.to_owned();
    for name in CREDENTIAL_NAMES {
        out = scrub_one_named(&out, name);
    }
    out
}

fn scrub_one_named(line: &str, name: &str) -> String {
    let lower = line.to_ascii_lowercase();
    let mut out = String::with_capacity(line.len());
    let mut cursor = 0_usize;
    while let Some(found) = lower.get(cursor..).and_then(|rest| rest.find(name)) {
        let name_start = cursor.saturating_add(found);
        let name_end = name_start.saturating_add(name.len());

        // The name must stand alone: `api_key` inside `my_api_keyring` is not a credential field, and
        // a leading word character means this is part of a longer identifier.
        let preceded_by_word = line
            .get(..name_start)
            .and_then(|head| head.chars().next_back())
            .is_some_and(|character| character.is_alphanumeric() || character == '_');
        let Some(after_name) = line.get(name_end..) else {
            break;
        };

        // Between the name and its value: an optional closing quote, then a separator.
        let mut offset = 0_usize;
        let mut separated = false;
        for character in after_name.chars() {
            match character {
                '"' | '\'' | ' ' | '\t' => offset = offset.saturating_add(character.len_utf8()),
                ':' | '=' => {
                    separated = true;
                    offset = offset.saturating_add(character.len_utf8());
                    break;
                }
                _other => break,
            }
        }
        if preceded_by_word || !separated {
            out.push_str(line.get(cursor..name_end).unwrap_or_default());
            cursor = name_end;
            continue;
        }

        // Then optional whitespace and an optional opening quote.
        let value_region = after_name.get(offset..).unwrap_or_default();
        let lead: usize = value_region
            .chars()
            .take_while(|character| matches!(character, ' ' | '\t' | '"' | '\''))
            .map(char::len_utf8)
            .sum();
        let value_start = name_end.saturating_add(offset).saturating_add(lead);
        let Some(value) = line.get(value_start..) else {
            break;
        };
        // A header value may contain spaces (`Bearer x`, a cookie pair), so the value runs to the end
        // of the line, a quote, or a structural character.
        let value_len = value
            .find(['"', '\'', ',', '}', '\n', ';'])
            .unwrap_or(value.len());
        let trimmed = value.get(..value_len).unwrap_or_default().trim_end();
        // No length floor here, unlike the bare-`Bearer` pass: the field *name* is the evidence, so an
        // `Authorization` value is a credential however short it is. An empty value is left alone,
        // because there is nothing to hide and `token: ""` is a readable line.
        if trimmed.is_empty() {
            out.push_str(line.get(cursor..value_start).unwrap_or_default());
            cursor = value_start;
            continue;
        }
        out.push_str(line.get(cursor..value_start).unwrap_or_default());
        out.push_str(REDACTED);
        cursor = value_start.saturating_add(trimmed.len());
    }
    out.push_str(line.get(cursor..).unwrap_or_default());
    out
}

/// Token shapes that identify themselves without a surrounding name.
///
/// An `sk-`-prefixed key, a Google `ya29.` token, and a JWT are all recognisable on their own, and all
/// three appear in logs detached from any field name — in a URL, in an error message quoting a body.
fn scrub_self_identifying(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    for word in line.split_inclusive(is_separator) {
        let trailing: String = word
            .chars()
            .rev()
            .take_while(|character| is_separator(*character))
            .collect();
        let core = word
            .get(..word.len().saturating_sub(trailing.len()))
            .unwrap_or(word);
        if is_secret_shaped(core) {
            out.push_str(REDACTED);
        } else {
            out.push_str(core);
        }
        out.extend(trailing.chars().rev());
    }
    out
}

/// Whether a bare word is a credential on its own evidence.
fn is_secret_shaped(word: &str) -> bool {
    if word.len() < MIN_SECRET {
        return false;
    }
    // `sk-…`, `sk_live_…`, and the many provider variants on that prefix.
    let lower = word.to_ascii_lowercase();
    if (lower.starts_with("sk-") || lower.starts_with("sk_")) && word.len() >= 12 {
        return true;
    }
    // Google OAuth access tokens.
    if lower.starts_with("ya29.") {
        return true;
    }
    // A JWT: three base64url segments. The header always begins `eyJ`, which is `{"` encoded, so this
    // does not match an arbitrary dotted identifier.
    if word.starts_with("eyJ") && word.split('.').count() == 3 {
        return true;
    }
    false
}

/// Characters that may appear inside a token.
const fn is_token_char(character: char) -> bool {
    character.is_ascii_alphanumeric()
        || matches!(character, '-' | '_' | '.' | '~' | '+' | '/' | '=')
}

/// Characters that end a word for the self-identifying pass.
///
/// `=`, `?`, and `&` are included because a token in a query string (`?key=sk-…`) is one word otherwise,
/// and a word that merely *contains* a token does not match the prefixes below. Every byte is preserved
/// on reassembly, so splitting more finely only ever helps detection.
const fn is_separator(character: char) -> bool {
    character.is_whitespace()
        || matches!(
            character,
            '"' | '\''
                | ','
                | ';'
                | '}'
                | ')'
                | ']'
                | '='
                | '?'
                | '&'
                | '('
                | '['
                | '<'
                | '>'
                | '|'
        )
}

#[cfg(test)]
mod tests {
    use super::{looks_clean, scrub};

    #[test]
    fn a_bearer_token_is_removed_but_the_line_stays_readable() {
        assert_eq!(
            scrub("upstream refused: Authorization: Bearer ya29.a0AfB_averylongtokenvalue"),
            "upstream refused: Authorization: [redacted]"
        );
        // The surrounding message survives, which is the point of scrubbing rather than dropping.
        let scrubbed = scrub("POST /v1/messages bearer sk-proj-abcdefghijklmnop failed with 401");
        assert!(scrubbed.contains("POST /v1/messages"), "{scrubbed}");
        assert!(scrubbed.contains("failed with 401"), "{scrubbed}");
        assert!(!scrubbed.contains("abcdefghijklmnop"), "{scrubbed}");
    }

    #[test]
    fn a_bare_short_bearer_is_left_alone_but_a_named_one_is_not() {
        // Two different pieces of evidence. A bare `Bearer x` with no field name has only the token
        // itself to go on, and one character is a placeholder rather than a credential.
        assert_eq!(
            scrub("see docs: bearer x for the header"),
            "see docs: bearer x for the header"
        );
        // But under a field name that *means* credential, the name is the evidence and length is
        // irrelevant — an `Authorization` value is a credential however short.
        assert_eq!(
            scrub("Authorization: Bearer x"),
            "Authorization: [redacted]"
        );
        // An empty value is left readable: there is nothing to hide.
        assert!(looks_clean("token: \"\""));
    }

    #[test]
    fn named_credential_fields_are_redacted_in_both_shapes() {
        // A header line and a JSON body are the two forms a credential reaches a log in.
        assert_eq!(
            scrub(r#"{"api_key":"sk-abcdefghijklmnopqrst","model":"gpt-5"}"#),
            r#"{"api_key":"[redacted]","model":"gpt-5"}"#
        );
        assert_eq!(
            scrub("x-api-key: abcdefghijklmnopqrstuvwx"),
            "x-api-key: [redacted]"
        );
        assert_eq!(
            scrub("Cookie: sso=averylongsessioncookievalue"),
            "Cookie: [redacted]"
        );
        // A refresh token in a JSON error body.
        let scrubbed = scrub(r#"refresh failed: {"refresh_token": "1//0gLongRefreshTokenValue"}"#);
        assert!(!scrubbed.contains("LongRefreshToken"), "{scrubbed}");
        assert!(scrubbed.contains("refresh failed"), "{scrubbed}");
    }

    #[test]
    fn a_name_inside_a_longer_identifier_is_not_treated_as_a_field() {
        // `my_api_keyring` is not a credential field, and redacting after it would eat real text.
        let line = "loaded my_api_keyring from disk successfully";
        assert_eq!(scrub(line), line);
        // Nor is a word that merely contains the name with no separator following.
        let bare = "the password requirements are documented";
        assert_eq!(scrub(bare), bare);
    }

    #[test]
    fn self_identifying_tokens_are_caught_without_a_field_name() {
        // These reach logs detached from any name — inside a URL, or quoted in an error.
        let scrubbed = scrub("GET https://api.example.com/v1?key=sk-abcdefghijklmnopqrs failed");
        assert!(!scrubbed.contains("abcdefghijklmnopqrs"), "{scrubbed}");

        let jwt = scrub("decoding eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjMifQ.c2lnbmF0dXJl now");
        assert!(!jwt.contains("eyJzdWIi"), "{jwt}");
        assert!(jwt.contains("decoding") && jwt.contains("now"), "{jwt}");

        let google = scrub("token ya29.a0AfB_verylongaccesstokenhere expired");
        assert!(!google.contains("verylongaccesstoken"), "{google}");
        assert!(google.contains("expired"), "{google}");
    }

    #[test]
    fn an_ordinary_log_line_is_returned_unchanged() {
        // The pane's whole value is reading these, so the scrubber must not touch them.
        for line in [
            "runtime: dispatched anthropic/claude-sonnet-4-5 in 412ms status=200",
            "thread 'main' panicked at src/lib.rs:42:9: assertion failed",
            "commit 9f2a1c4e8b7d6a5f3e2d1c0b9a8f7e6d5c4b3a29 built at 2026-09-01",
            "GET /api/settings 200 3ms",
        ] {
            assert!(looks_clean(line), "scrubber altered a clean line: {line}");
        }
    }

    #[test]
    fn a_hash_is_not_mistaken_for_a_credential() {
        // A 40-char hex blob may be a commit or a checksum. Redacting every high-entropy string would
        // make a stack trace unreadable, so only self-identifying shapes are matched.
        let line = "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert_eq!(scrub(line), line);
    }

    #[test]
    fn several_secrets_in_one_line_are_all_removed() {
        let scrubbed = scrub(
            r#"retry: {"api_key":"sk-firstlongkeyvalue","access_token":"ya29.secondlongvalue"}"#,
        );
        assert!(!scrubbed.contains("firstlongkey"), "{scrubbed}");
        assert!(!scrubbed.contains("secondlongvalue"), "{scrubbed}");
        assert_eq!(scrubbed.matches("[redacted]").count(), 2, "{scrubbed}");
    }
}
