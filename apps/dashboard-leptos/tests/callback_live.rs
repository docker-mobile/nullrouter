//! Provider OAuth callback derivations.
//!
//! This logic was 59 lines of inline JavaScript. The reason it is worth testing
//! directly is [`relay_origins`]: this page holds an authorization code and hands
//! it to another window, so who may receive it is a security decision, not markup.

use nullrouter_dashboard_wasm::dashboard::callback_live::{
    CallbackData, HELPER_ORIGIN, Panel, RELAY_CHANNEL, RELAY_MESSAGE_TYPE, panel_for,
    parse_callback, relay_envelope, relay_origins, relay_payload,
};

const ORIGIN: &str = "https://router.example";

#[test]
fn the_grant_is_never_relayed_to_a_wildcard_origin() {
    // Given: the payload carries an authorization code. A `postMessage` to `"*"`
    // would hand that code to whatever window happened to open this page.
    let origins = relay_origins(ORIGIN);
    assert!(
        !origins.iter().any(|origin| origin == "*"),
        "a wildcard target would leak the grant: {origins:?}"
    );
    // Only this origin and the fixed loopback helper.
    assert_eq!(origins, vec![ORIGIN.to_owned(), HELPER_ORIGIN.to_owned()]);
}

#[test]
fn the_helper_origin_is_loopback_only() {
    // Given: a second recipient is acceptable only because it cannot be remote.
    assert!(
        HELPER_ORIGIN.starts_with("http://localhost:")
            || HELPER_ORIGIN.starts_with("http://127.0.0.1:"),
        "the helper must be loopback: {HELPER_ORIGIN}"
    );
}

#[test]
fn the_helper_is_not_listed_twice_when_it_is_this_origin() {
    // Given: a router served from the helper's own port. Posting twice to one
    // recipient would deliver the grant twice.
    let origins = relay_origins(HELPER_ORIGIN);
    assert_eq!(origins, vec![HELPER_ORIGIN.to_owned()]);
}

#[test]
fn a_blank_origin_does_not_become_an_empty_target() {
    // Given: `location.origin` can be an empty string in odd contexts. An empty
    // `postMessage` target is not a same-origin restriction.
    let origins = relay_origins("");
    assert_eq!(origins, vec![HELPER_ORIGIN.to_owned()]);
    assert!(!origins.iter().any(|origin| origin.is_empty()));
}

#[test]
fn a_conclusive_callback_shows_success_and_an_empty_one_asks_for_a_copy() {
    // Given: the provider returned a code, a token, or an error. All three mean
    // the flow concluded, and the initiator needs to hear about it.
    for query in [
        "?code=abc123&state=s1",
        "?token=tok_1",
        "?error=access_denied&error_description=nope",
    ] {
        let data = parse_callback(query, "https://router.example/callback");
        assert!(data.is_conclusive(), "{query}");
        assert_eq!(panel_for(&data), Panel::Success, "{query}");
    }

    // Nothing actionable: the user copies the URL by hand.
    let empty = parse_callback("", "https://router.example/callback");
    assert!(!empty.is_conclusive());
    assert_eq!(panel_for(&empty), Panel::ManualCopy);
}

#[test]
fn an_empty_parameter_is_not_treated_as_a_grant() {
    // Given: a provider that redirects with `?code=` has not sent a code.
    // Relaying the empty string would announce a grant of nothing and leave the
    // initiator waiting on a value that never arrives.
    let data = parse_callback("?code=&token=&error=", "https://router.example/callback");
    assert_eq!(data.code, None);
    assert_eq!(data.token, None);
    assert_eq!(data.error, None);
    assert!(!data.is_conclusive());
    assert_eq!(panel_for(&data), Panel::ManualCopy);
}

#[test]
fn percent_encoded_and_plus_encoded_values_are_decoded() {
    // Given: an authorization code is opaque and may be encoded either way.
    let data = parse_callback(
        "?code=a%2Fb%2Bc&error_description=not+allowed&state=%7Bx%7D",
        "https://router.example/callback",
    );
    assert_eq!(data.code.as_deref(), Some("a/b+c"));
    assert_eq!(data.error_description.as_deref(), Some("not allowed"));
    assert_eq!(data.state.as_deref(), Some("{x}"));
}

#[test]
fn a_malformed_escape_keeps_the_value_rather_than_dropping_it() {
    // Given: a truncated `%` escape. Dropping the parameter would silently lose a
    // grant; keeping it verbatim at least lets the relay or the manual copy work.
    let data = parse_callback("?code=abc%", "https://router.example/callback");
    assert_eq!(data.code.as_deref(), Some("abc%"));
    let short = parse_callback("?code=%2", "https://router.example/callback");
    assert_eq!(short.code.as_deref(), Some("%2"));
    let bad_hex = parse_callback("?code=%zz", "https://router.example/callback");
    assert_eq!(bad_hex.code.as_deref(), Some("%zz"));
}

#[test]
fn the_full_url_is_carried_for_the_manual_fallback() {
    // Given: the manual panel shows the URL to copy. Without it that panel is
    // useless.
    let url = "https://router.example/callback?state=s1";
    let data = parse_callback("?state=s1", url);
    assert_eq!(data.full_url, url);
}

#[test]
fn the_relay_payload_is_valid_json_with_a_timestamp() {
    // Given: a code containing a quote must not break out of the payload.
    let hostile = r#"co"de\1"#;
    let data = parse_callback(
        &format!("?code={}", hostile.replace('"', "%22").replace('\\', "%5C")),
        "https://router.example/callback",
    );
    assert_eq!(data.code.as_deref(), Some(hostile));

    let payload = relay_payload(&data, 1_700_000_000_000);
    let parsed: serde_json::Value =
        serde_json::from_str(&payload).expect("a relay payload must always be valid JSON");
    assert_eq!(
        parsed.get("code").and_then(serde_json::Value::as_str),
        Some(hostile)
    );
    assert_eq!(
        parsed.get("timestamp").and_then(serde_json::Value::as_i64),
        Some(1_700_000_000_000)
    );
}

#[test]
fn the_relay_payload_omits_absent_fields_rather_than_sending_null() {
    // Given: a listener checking `data.error` should not see an explicit null and
    // treat the flow as failed.
    let data = parse_callback("?code=abc", "https://router.example/callback");
    let payload = relay_payload(&data, 0);
    assert!(payload.contains("\"code\""));
    assert!(
        !payload.contains("\"error\""),
        "an absent error must not be serialised: {payload}"
    );
    assert!(!payload.contains("null"), "{payload}");
}

#[test]
fn the_opener_envelope_is_typed_so_a_listener_can_filter_it() {
    // Given: an opener may receive messages from several sources.
    let data = parse_callback("?code=abc", "https://router.example/callback");
    let envelope = relay_envelope(&data);
    let parsed: serde_json::Value = serde_json::from_str(&envelope).expect("valid JSON");
    assert_eq!(
        parsed.get("type").and_then(serde_json::Value::as_str),
        Some(RELAY_MESSAGE_TYPE)
    );
    assert_eq!(
        parsed
            .pointer("/data/code")
            .and_then(serde_json::Value::as_str),
        Some("abc")
    );
}

#[test]
fn the_channel_and_storage_key_agree() {
    // Given: a listener subscribes to one name and reads one key. Two different
    // names would mean the storage fallback never being found.
    assert_eq!(RELAY_CHANNEL, "oauth_callback");
    assert_eq!(RELAY_MESSAGE_TYPE, RELAY_CHANNEL);
}

#[test]
fn a_default_callback_is_inconclusive_rather_than_a_success() {
    // Given: a native build or a missing window yields the default. It must not
    // read as a completed authorization.
    let data = CallbackData::default();
    assert!(!data.is_conclusive());
    assert_eq!(panel_for(&data), Panel::ManualCopy);
    assert_eq!(Panel::default(), Panel::Processing);
}
