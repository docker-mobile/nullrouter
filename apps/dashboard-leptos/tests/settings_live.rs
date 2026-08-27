//! Settings panel state: what it reads, what it sends, and what it does when a
//! write fails.
//!
//! The regression these guard is a Settings panel whose toggles were local
//! `signal()`s: a click looked like a save and persisted nothing. So the
//! assertions here are mostly about refusing to show a value the server did not
//! send, and about a failed write leaving the ORIGINAL value on screen.

use nullrouter_dashboard_wasm::{
    api::ApiError,
    dashboard::{
        SETTINGS_FIELDS, SETTINGS_PATH, SettingsControl, SettingsField, SettingsSnapshot,
        SettingsValue, WriteOutcome, parse_settings, patch_body, resolve,
    },
};

const FULL_BODY: &str = r#"{
    "requireLogin": true,
    "tunnelDashboardAccess": false,
    "tunnelUrl": "https://tunnel.example.test",
    "tailscaleUrl": "https://router.tailnet.ts.net",
    "outboundProxyEnabled": true,
    "outboundProxyUrl": "http://127.0.0.1:8080",
    "outboundNoProxy": "localhost,127.0.0.1"
}"#;

/// What [`FULL_BODY`] must parse into, spelled out rather than derived from the
/// parser, so `parses_every_field_of_a_full_settings_body` is checking the
/// parser against a fixture instead of against itself.
fn full_snapshot() -> SettingsSnapshot {
    SettingsSnapshot {
        require_login: true,
        tunnel_dashboard_access: false,
        tunnel_url: "https://tunnel.example.test".to_owned(),
        tailscale_url: "https://router.tailnet.ts.net".to_owned(),
        outbound_proxy_enabled: true,
        outbound_proxy_url: "http://127.0.0.1:8080".to_owned(),
        outbound_no_proxy: "localhost,127.0.0.1".to_owned(),
    }
}

#[test]
fn parses_every_field_of_a_full_settings_body() {
    // Given: the exact SettingsView projection served by GET /api/settings.
    // When: the panel hydrates from it.
    let snapshot = parse_settings(FULL_BODY).expect("the full settings body should parse");

    // Then: every field is taken from the body, none is invented.
    assert_eq!(snapshot, full_snapshot());
    assert!(snapshot.require_login);
    assert!(!snapshot.tunnel_dashboard_access);
    assert_eq!(snapshot.tunnel_url, "https://tunnel.example.test");
    assert_eq!(snapshot.tailscale_url, "https://router.tailnet.ts.net");
    assert!(snapshot.outbound_proxy_enabled);
    assert_eq!(snapshot.outbound_proxy_url, "http://127.0.0.1:8080");
    assert_eq!(snapshot.outbound_no_proxy, "localhost,127.0.0.1");
}

#[test]
fn every_rendered_field_maps_to_a_settings_view_key() {
    // Given: the panel renders one row per field in SETTINGS_FIELDS.
    // When: each row's JSON key is read back out of a full server body.
    let parsed = serde_json::from_str::<serde_json::Value>(FULL_BODY)
        .expect("the fixture body should be valid JSON");

    // Then: no row exists that GET /api/settings does not report, and every key
    // the response carries has a row. A control whose state cannot be read is
    // exactly the bug this panel replaced.
    for field in SETTINGS_FIELDS {
        assert!(
            parsed.get(field.json_key()).is_some(),
            "{} has a control but is not in SettingsView",
            field.json_key()
        );
    }
    let rendered = SETTINGS_FIELDS
        .iter()
        .map(|field| field.json_key())
        .collect::<Vec<_>>();
    for key in parsed
        .as_object()
        .expect("the fixture body should be an object")
        .keys()
    {
        assert!(
            rendered.contains(&key.as_str()),
            "{key} is in SettingsView but has no control"
        );
    }
    assert_eq!(SETTINGS_PATH, "/api/settings");
}

#[test]
fn omitted_string_fields_read_as_unset_and_omitted_flags_fail() {
    // Given: a body where the optional URL/no-proxy strings are absent. The
    // service serialises an unconfigured tunnel or proxy as "", so absent and
    // empty carry the same meaning.
    let body = r#"{
        "requireLogin": false,
        "tunnelDashboardAccess": true,
        "outboundProxyEnabled": false
    }"#;

    // When: the panel parses it.
    let snapshot = parse_settings(body).expect("absent optional strings should parse");

    // Then: the strings read as unset and the booleans come from the body.
    assert!(!snapshot.require_login);
    assert!(snapshot.tunnel_dashboard_access);
    assert!(snapshot.tunnel_url.is_empty());
    assert!(snapshot.tailscale_url.is_empty());
    assert!(snapshot.outbound_proxy_url.is_empty());
    assert!(snapshot.outbound_no_proxy.is_empty());

    // And: a missing boolean is NOT defaulted. `requireLogin` absent would
    // otherwise render as "login is not required" — an access-control claim the
    // server never made — so it is a parse failure instead.
    assert_eq!(
        parse_settings(r#"{"tunnelDashboardAccess": false, "outboundProxyEnabled": false}"#),
        None,
        "a missing requireLogin must not default to false"
    );
    assert_eq!(
        parse_settings("{}"),
        None,
        "an empty object is not settings"
    );
}

#[test]
fn malformed_bodies_never_produce_a_snapshot() {
    // Given: bodies a panel could plausibly receive from a broken or wrong host.
    // When/Then: none of them yields renderable values. api::hydrate turns None
    // into Hydrate::Failed, so each of these surfaces as a visible error.
    for body in [
        "",
        "   ",
        "null",
        "[]",
        "not json",
        r#"{"requireLogin": true"#,
        r#"{"requireLogin": "yes", "tunnelDashboardAccess": false, "outboundProxyEnabled": false}"#,
        r#"{"requireLogin": null, "tunnelDashboardAccess": false, "outboundProxyEnabled": false}"#,
        "<!doctype html><html><body>login</body></html>",
    ] {
        assert_eq!(parse_settings(body), None, "{body:?} must not parse");
    }
}

#[test]
fn a_failed_write_restores_the_original_value() {
    // This is the core anti-regression test: the panel is allowed to flip a
    // control before the server answers only because a refusal puts the old
    // value back. If this passes while the write fails, the user is looking at a
    // setting the router does not have.

    // Given: a loaded panel with dashboard login required.
    let server = full_snapshot();
    assert!(server.require_login);
    let previous = server.value(SettingsField::RequireLogin);

    // When: the user turns it off (optimistic flip) and the PUT is refused.
    let optimistic = server.with(SettingsField::RequireLogin, SettingsValue::Flag(false));
    assert_eq!(
        optimistic.value(SettingsField::RequireLogin),
        SettingsValue::Flag(false),
        "the flip must be visible immediately"
    );
    let resolution = resolve(
        &optimistic,
        SettingsField::RequireLogin,
        &previous,
        WriteOutcome::Rejected(ApiError::Status(500)),
    );

    // Then: the original value is back, the row reports the failure, and nothing
    // claims the write landed.
    assert!(!resolution.committed);
    assert_eq!(resolution.error, Some(ApiError::Status(500)));
    assert_eq!(resolution.snapshot, server);
    assert!(
        resolution
            .snapshot
            .value(SettingsField::RequireLogin)
            .flag()
            .unwrap_or(false),
        "require_login must be true again after a refused write"
    );
}

#[test]
fn a_failed_text_write_restores_the_original_and_leaves_other_rows_alone() {
    // Given: a panel where another row has already been changed successfully, so
    // the optimistic snapshot differs from the last full server read.
    let server = full_snapshot();
    let previous = server.value(SettingsField::TunnelUrl);
    let with_other_row_saved = server.with(
        SettingsField::TunnelDashboardAccess,
        SettingsValue::Flag(true),
    );

    // When: a tunnel URL edit is typed, shown, and then rejected.
    let optimistic = with_other_row_saved.with(
        SettingsField::TunnelUrl,
        SettingsValue::Text("http://typo".to_owned()),
    );
    let resolution = resolve(
        &optimistic,
        SettingsField::TunnelUrl,
        &previous,
        WriteOutcome::Rejected(ApiError::Network),
    );

    // Then: only the failing field is rolled back. A rollback that restored the
    // whole snapshot would silently undo the row that did save.
    assert!(!resolution.committed);
    assert_eq!(
        resolution.snapshot.tunnel_url,
        "https://tunnel.example.test"
    );
    assert!(
        resolution.snapshot.tunnel_dashboard_access,
        "the other row's saved value must survive this rollback"
    );
    assert_eq!(resolution.error, Some(ApiError::Network));
}

#[test]
fn a_successful_put_response_replaces_local_state() {
    // Given: a panel showing the current server state, and a PUT that the server
    // answers with the updated SettingsView.
    let server = full_snapshot();
    let previous = server.value(SettingsField::OutboundProxyUrl);
    let optimistic = server.with(
        SettingsField::OutboundProxyUrl,
        SettingsValue::Text("http://127.0.0.1:9999".to_owned()),
    );
    // The server normalised the value and also reports a field the panel did not
    // touch, which is why the reply wins over the optimistic guess.
    let reply = r#"{
        "requireLogin": true,
        "tunnelDashboardAccess": false,
        "tunnelUrl": "https://tunnel.example.test",
        "tailscaleUrl": "https://router.tailnet.ts.net",
        "outboundProxyEnabled": true,
        "outboundProxyUrl": "http://127.0.0.1:9999/",
        "outboundNoProxy": "localhost"
    }"#;
    let confirmed = parse_settings(reply).expect("the PUT reply should parse");

    // When: the write is settled with that reply.
    let resolution = resolve(
        &optimistic,
        SettingsField::OutboundProxyUrl,
        &previous,
        WriteOutcome::Confirmed(confirmed.clone()),
    );

    // Then: local state is the server's state, not the optimistic one.
    assert!(resolution.committed);
    assert_eq!(resolution.error, None);
    assert_eq!(resolution.snapshot, confirmed);
    assert_eq!(
        resolution.snapshot.outbound_proxy_url,
        "http://127.0.0.1:9999/"
    );
    assert_eq!(
        resolution.snapshot.outbound_no_proxy, "localhost",
        "a field the panel did not send must still follow the server reply"
    );
}

#[test]
fn an_accepted_write_with_an_unreadable_reply_is_reported_not_celebrated() {
    // Given: the PUT succeeded but its body could not be parsed, so the panel
    // cannot read back what was stored.
    let server = full_snapshot();
    let previous = server.value(SettingsField::TunnelDashboardAccess);
    let optimistic = server.with(
        SettingsField::TunnelDashboardAccess,
        SettingsValue::Flag(true),
    );

    // When: the write is settled.
    let resolution = resolve(
        &optimistic,
        SettingsField::TunnelDashboardAccess,
        &previous,
        WriteOutcome::Unconfirmed,
    );

    // Then: the value stays (rolling back a write that landed would be its own
    // lie) but the row still reports an error rather than a clean "Saved".
    assert!(resolution.committed);
    assert!(resolution.snapshot.tunnel_dashboard_access);
    assert_eq!(resolution.error, Some(ApiError::Body));
}

#[test]
fn a_patch_body_sends_only_the_changed_field() {
    // Given: SettingsRequest takes every field as Option, so a single-key body
    // leaves the rest of the stored settings untouched.
    // When: each control builds its PUT body.
    let flag = patch_body(SettingsField::RequireLogin, &SettingsValue::Flag(false));
    let text = patch_body(
        SettingsField::TunnelUrl,
        &SettingsValue::Text("https://a.example.test".to_owned()),
    );

    // Then: one key, correctly typed and camelCase.
    assert_eq!(flag, r#"{"requireLogin":false}"#);
    assert_eq!(text, r#"{"tunnelUrl":"https://a.example.test"}"#);

    // And: the value is JSON-encoded, so a quote or backslash in a URL cannot
    // break out of the payload.
    let hostile = patch_body(
        SettingsField::OutboundProxyUrl,
        &SettingsValue::Text(r#"http://a"b\c"#.to_owned()),
    );
    assert_eq!(hostile, r#"{"outboundProxyUrl":"http://a\"b\\c"}"#);
    let reparsed = serde_json::from_str::<serde_json::Value>(&hostile)
        .expect("a patch body must always be valid JSON");
    assert_eq!(
        reparsed
            .get("outboundProxyUrl")
            .and_then(serde_json::Value::as_str),
        Some(r#"http://a"b\c"#)
    );
}

#[test]
fn every_field_has_a_control_a_label_and_a_round_trip() {
    // Given: the panel builds each row from the field's own metadata.
    for field in SETTINGS_FIELDS {
        // Then: it has text a user can act on, and a stable id to bind a label
        // and an aria-live status region to.
        assert!(!field.label().is_empty(), "{field:?} has no label");
        assert!(
            field.description().ends_with('.'),
            "{field:?} description should read as a sentence"
        );
        assert!(field.dom_id().contains(field.json_key()));
        assert_ne!(field.dom_id(), field.status_id());

        // And: reading a field back after writing it returns what was written,
        // in the kind its control produces.
        let mut snapshot = SettingsSnapshot::default();
        let written = match field.control() {
            SettingsControl::Toggle => SettingsValue::Flag(true),
            SettingsControl::Text => SettingsValue::Text("written".to_owned()),
        };
        snapshot.set(field, written.clone());
        assert_eq!(
            snapshot.value(field),
            written,
            "{field:?} did not round-trip"
        );
    }
}

#[test]
fn a_value_of_the_wrong_kind_changes_nothing() {
    // Given: a loaded snapshot. A kind mismatch cannot come from a control, so
    // it must be dropped rather than guessed at or panicked on.
    let server = full_snapshot();

    // When: a toggle field is handed text, and a text field a boolean.
    let a = server.with(
        SettingsField::RequireLogin,
        SettingsValue::Text("true".to_owned()),
    );
    let b = server.with(SettingsField::TunnelUrl, SettingsValue::Flag(true));

    // Then: nothing moves.
    assert_eq!(a, server);
    assert_eq!(b, server);
}
