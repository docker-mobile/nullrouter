//! Settings panel state: what it reads, what it sends, and what it does when a
//! write fails.
//!
//! The regression these guard is a Settings panel whose toggles were local
//! `signal()`s: a click looked like a save and persisted nothing. So the
//! assertions here are mostly about refusing to show a value the server did not
//! send, and about a failed write leaving the ORIGINAL value on screen.
//!
//! A second regression is guarded alongside it: a secret rendered from a value
//! the panel does not have. `oidcClientSecret` and `samlCert` are write-only —
//! `SettingsView` reports only whether one is stored — so the panel must never
//! claim to hold one.

use nullrouter_dashboard_wasm::{
    api::ApiError,
    dashboard::{
        SETTINGS_FIELDS, SETTINGS_PATH, SettingsControl, SettingsField, SettingsSnapshot,
        SettingsValue, WriteOutcome, parse_settings, patch_body, resolve,
    },
};

/// The exact `SettingsView` projection served by `GET /api/settings`.
///
/// The two secrets appear only as their `…Set` booleans, which is the whole of
/// what the server discloses about them.
const FULL_BODY: &str = r#"{
    "tunnelDashboardAccess": false,
    "tunnelUrl": "https://tunnel.example.test",
    "tailscaleUrl": "https://router.tailnet.ts.net",
    "outboundProxyEnabled": true,
    "outboundProxyUrl": "http://127.0.0.1:8080",
    "outboundNoProxy": "localhost,127.0.0.1",
    "oidcIssuerUrl": "https://idp.example.test",
    "oidcClientId": "nullrouter-dashboard",
    "oidcClientSecretSet": true,
    "oidcScopes": "openid profile email",
    "oidcLoginLabel": "Sign in with SSO",
    "samlEntryPoint": "https://idp.example.test/sso",
    "samlIssuer": "urn:nullrouter:dashboard",
    "samlCertSet": false,
    "samlAttributeEmail": "email",
    "samlAttributeName": "displayName"
}"#;

/// What [`FULL_BODY`] must parse into, spelled out rather than derived from the
/// parser, so `parses_every_field_of_a_full_settings_body` is checking the
/// parser against a fixture instead of against itself.
fn full_snapshot() -> SettingsSnapshot {
    SettingsSnapshot {
        tunnel_dashboard_access: false,
        tunnel_url: "https://tunnel.example.test".to_owned(),
        tailscale_url: "https://router.tailnet.ts.net".to_owned(),
        outbound_proxy_enabled: true,
        outbound_proxy_url: "http://127.0.0.1:8080".to_owned(),
        outbound_no_proxy: "localhost,127.0.0.1".to_owned(),
        oidc_issuer_url: "https://idp.example.test".to_owned(),
        oidc_client_id: "nullrouter-dashboard".to_owned(),
        oidc_client_secret_set: true,
        oidc_scopes: "openid profile email".to_owned(),
        oidc_login_label: "Sign in with SSO".to_owned(),
        saml_entry_point: "https://idp.example.test/sso".to_owned(),
        saml_issuer: "urn:nullrouter:dashboard".to_owned(),
        saml_cert_set: false,
        saml_attribute_email: "email".to_owned(),
        saml_attribute_name: "displayName".to_owned(),
    }
}

#[test]
fn parses_every_field_of_a_full_settings_body() {
    // Given: the exact SettingsView projection served by GET /api/settings.
    // When: the panel hydrates from it.
    let snapshot = parse_settings(FULL_BODY).expect("the full settings body should parse");

    // Then: every field is taken from the body, none is invented.
    assert_eq!(snapshot, full_snapshot());
    assert!(!snapshot.tunnel_dashboard_access);
    assert_eq!(snapshot.tunnel_url, "https://tunnel.example.test");
    assert_eq!(snapshot.tailscale_url, "https://router.tailnet.ts.net");
    assert!(snapshot.outbound_proxy_enabled);
    assert_eq!(snapshot.outbound_proxy_url, "http://127.0.0.1:8080");
    assert_eq!(snapshot.outbound_no_proxy, "localhost,127.0.0.1");
    assert_eq!(snapshot.oidc_issuer_url, "https://idp.example.test");
    assert_eq!(snapshot.oidc_client_id, "nullrouter-dashboard");
    assert_eq!(snapshot.oidc_scopes, "openid profile email");
    assert_eq!(snapshot.saml_entry_point, "https://idp.example.test/sso");
    assert_eq!(snapshot.saml_attribute_name, "displayName");

    // And: the secrets are known only as "stored" / "not stored".
    assert_eq!(snapshot.is_set(SettingsField::OidcClientSecret), Some(true));
    assert_eq!(snapshot.is_set(SettingsField::SamlCert), Some(false));
}

#[test]
fn a_secret_is_never_readable_from_a_snapshot() {
    // Given: a snapshot whose server body said a client secret IS stored.
    let snapshot = full_snapshot();
    assert_eq!(snapshot.is_set(SettingsField::OidcClientSecret), Some(true));

    // When/Then: reading the field yields nothing to render. The panel cannot
    // display a secret it was never sent, and `is_set` is the only claim it may
    // make about one.
    for field in [SettingsField::OidcClientSecret, SettingsField::SamlCert] {
        assert_eq!(
            snapshot.value(field),
            SettingsValue::Text(String::new()),
            "{field:?} must read as empty, never as a value"
        );
    }

    // And: a non-secret field cannot be mistaken for an unset secret.
    assert_eq!(snapshot.is_set(SettingsField::TunnelUrl), None);
    assert_eq!(snapshot.is_set(SettingsField::OutboundProxyEnabled), None);
}

#[test]
fn writing_a_secret_locally_changes_nothing() {
    // Given: a loaded panel. Whether a secret is stored is server state, and the
    // server never echoes the value back.
    let server = full_snapshot();

    // When: a secret is typed into the panel's own snapshot.
    let typed = server.with(
        SettingsField::SamlCert,
        SettingsValue::Text("-----BEGIN CERTIFICATE-----".to_owned()),
    );

    // Then: nothing is held locally. The `…Set` flag is only ever adopted from a
    // PUT reply, so the panel cannot predict it and cannot leak the value.
    assert_eq!(typed, server);
    assert_eq!(typed.is_set(SettingsField::SamlCert), Some(false));
    assert_eq!(
        typed.value(SettingsField::SamlCert),
        SettingsValue::Text(String::new())
    );
}

#[test]
fn every_rendered_field_maps_to_a_settings_view_key() {
    // Given: the panel renders one row per field in SETTINGS_FIELDS.
    // When: each row's readback key is read back out of a full server body.
    let parsed = serde_json::from_str::<serde_json::Value>(FULL_BODY)
        .expect("the fixture body should be valid JSON");

    // Then: no row exists whose state GET /api/settings does not report, and
    // every key the response carries has a row. A control whose state cannot be
    // read is exactly the bug this panel replaced. Secrets are matched on their
    // `…Set` readback key, since their value is deliberately absent.
    for field in SETTINGS_FIELDS {
        assert!(
            parsed.get(field.readback_key()).is_some(),
            "{} has a control but is not in SettingsView",
            field.readback_key()
        );
    }
    let rendered = SETTINGS_FIELDS
        .iter()
        .map(|field| field.readback_key())
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
fn a_secret_is_written_under_a_different_key_than_it_is_read() {
    // Given: a secret is sent as its own key but reported as a boolean. Reusing
    // one key for both would either echo the secret back or make the stored
    // state unreadable.
    // When/Then: the write key and the readback key differ, and only for secrets.
    for field in SETTINGS_FIELDS {
        if field.control() == SettingsControl::Secret {
            assert_ne!(
                field.json_key(),
                field.readback_key(),
                "{field:?} must not be read back under the key it is written with"
            );
            assert!(field.readback_key().ends_with("Set"), "{field:?}");
        } else {
            assert_eq!(field.json_key(), field.readback_key(), "{field:?}");
        }
    }
}

#[test]
fn omitted_string_fields_read_as_unset_and_omitted_flags_fail() {
    // Given: a body where the optional URL/no-proxy/SSO strings are absent. The
    // service serialises an unconfigured tunnel, proxy, or SSO field as "", so
    // absent and empty carry the same meaning.
    let body = r#"{
        "tunnelDashboardAccess": true,
        "outboundProxyEnabled": false,
        "oidcClientSecretSet": false,
        "samlCertSet": false
    }"#;

    // When: the panel parses it.
    let snapshot = parse_settings(body).expect("absent optional strings should parse");

    // Then: the strings read as unset and the booleans come from the body.
    assert!(snapshot.tunnel_dashboard_access);
    assert!(!snapshot.outbound_proxy_enabled);
    assert!(snapshot.tunnel_url.is_empty());
    assert!(snapshot.tailscale_url.is_empty());
    assert!(snapshot.outbound_proxy_url.is_empty());
    assert!(snapshot.outbound_no_proxy.is_empty());
    assert!(snapshot.oidc_issuer_url.is_empty());
    assert!(snapshot.saml_entry_point.is_empty());

    // And: a missing boolean is NOT defaulted. `outboundProxyEnabled` absent
    // would otherwise render as "no proxy is in use", and `oidcClientSecretSet`
    // absent as "no secret is stored" — claims the server never made — so each
    // is a parse failure instead.
    for incomplete in [
        r#"{"outboundProxyEnabled": false, "oidcClientSecretSet": false, "samlCertSet": false}"#,
        r#"{"tunnelDashboardAccess": false, "oidcClientSecretSet": false, "samlCertSet": false}"#,
        r#"{"tunnelDashboardAccess": false, "outboundProxyEnabled": false, "samlCertSet": false}"#,
        r#"{"tunnelDashboardAccess": false, "outboundProxyEnabled": false, "oidcClientSecretSet": false}"#,
    ] {
        assert_eq!(
            parse_settings(incomplete),
            None,
            "a missing boolean must not default: {incomplete}"
        );
    }
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
        r#"{"tunnelDashboardAccess": true"#,
        r#"{"tunnelDashboardAccess": "yes", "outboundProxyEnabled": false, "oidcClientSecretSet": false, "samlCertSet": false}"#,
        r#"{"tunnelDashboardAccess": null, "outboundProxyEnabled": false, "oidcClientSecretSet": false, "samlCertSet": false}"#,
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

    // Given: a loaded panel with proxied outbound traffic.
    let server = full_snapshot();
    assert!(server.outbound_proxy_enabled);
    let previous = server.value(SettingsField::OutboundProxyEnabled);

    // When: the user turns it off (optimistic flip) and the PUT is refused.
    let optimistic = server.with(
        SettingsField::OutboundProxyEnabled,
        SettingsValue::Flag(false),
    );
    assert_eq!(
        optimistic.value(SettingsField::OutboundProxyEnabled),
        SettingsValue::Flag(false),
        "the flip must be visible immediately"
    );
    let resolution = resolve(
        &optimistic,
        SettingsField::OutboundProxyEnabled,
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
            .value(SettingsField::OutboundProxyEnabled)
            .flag()
            .unwrap_or(false),
        "outbound_proxy_enabled must be true again after a refused write"
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
        "tunnelDashboardAccess": false,
        "tunnelUrl": "https://tunnel.example.test",
        "tailscaleUrl": "https://router.tailnet.ts.net",
        "outboundProxyEnabled": true,
        "outboundProxyUrl": "http://127.0.0.1:9999/",
        "outboundNoProxy": "localhost",
        "oidcIssuerUrl": "https://idp.example.test",
        "oidcClientId": "nullrouter-dashboard",
        "oidcClientSecretSet": true,
        "oidcScopes": "openid profile email",
        "oidcLoginLabel": "Sign in with SSO",
        "samlEntryPoint": "https://idp.example.test/sso",
        "samlIssuer": "urn:nullrouter:dashboard",
        "samlCertSet": false,
        "samlAttributeEmail": "email",
        "samlAttributeName": "displayName"
    }"#;
    let confirmed = parse_settings(reply).expect("the PUT reply should parse");

    // When: the write is settled with that reply.
    let resolution = resolve(
        &optimistic,
        SettingsField::OutboundProxyUrl,
        &previous,
        WriteOutcome::Confirmed(Box::new(confirmed.clone())),
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
fn a_stored_secret_is_adopted_from_the_put_reply_not_predicted() {
    // Given: a panel that has just saved a client secret. Locally it holds
    // nothing, so the only way it can report "configured" is the server's reply.
    let server = full_snapshot();
    let previous = server.value(SettingsField::SamlCert);
    let optimistic = server.with(
        SettingsField::SamlCert,
        SettingsValue::Text("-----BEGIN CERTIFICATE-----".to_owned()),
    );
    assert_eq!(
        optimistic.is_set(SettingsField::SamlCert),
        Some(false),
        "the panel must not predict that the write landed"
    );

    // When: the PUT is confirmed with a body reporting the certificate as stored.
    let reply = FULL_BODY.replace(r#""samlCertSet": false"#, r#""samlCertSet": true"#);
    let confirmed = parse_settings(&reply).expect("the PUT reply should parse");
    let resolution = resolve(
        &optimistic,
        SettingsField::SamlCert,
        &previous,
        WriteOutcome::Confirmed(Box::new(confirmed)),
    );

    // Then: the row now reports a stored certificate, still without holding it.
    assert!(resolution.committed);
    assert_eq!(
        resolution.snapshot.is_set(SettingsField::SamlCert),
        Some(true)
    );
    assert_eq!(
        resolution.snapshot.value(SettingsField::SamlCert),
        SettingsValue::Text(String::new())
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
    let flag = patch_body(
        SettingsField::OutboundProxyEnabled,
        &SettingsValue::Flag(false),
    );
    let text = patch_body(
        SettingsField::TunnelUrl,
        &SettingsValue::Text("https://a.example.test".to_owned()),
    );

    // Then: one key, correctly typed and camelCase.
    assert_eq!(flag, r#"{"outboundProxyEnabled":false}"#);
    assert_eq!(text, r#"{"tunnelUrl":"https://a.example.test"}"#);

    // And: a secret is sent under its write key, not its `…Set` readback key —
    // otherwise the save would post a boolean and store nothing.
    let secret = patch_body(
        SettingsField::OidcClientSecret,
        &SettingsValue::Text("s3cr3t".to_owned()),
    );
    assert_eq!(secret, r#"{"oidcClientSecret":"s3cr3t"}"#);

    // And: the value is JSON-encoded, so a quote or backslash in a URL or a
    // certificate cannot break out of the payload.
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
        // in the kind its control produces — except a secret, which has nowhere
        // local to be stored and so always reads back empty.
        let mut snapshot = SettingsSnapshot::default();
        let written = match field.control() {
            SettingsControl::Toggle => SettingsValue::Flag(true),
            SettingsControl::Text | SettingsControl::Secret => {
                SettingsValue::Text("written".to_owned())
            }
        };
        snapshot.set(field, written.clone());
        let expected = if field.control() == SettingsControl::Secret {
            SettingsValue::Text(String::new())
        } else {
            written
        };
        assert_eq!(
            snapshot.value(field),
            expected,
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
        SettingsField::OutboundProxyEnabled,
        SettingsValue::Text("true".to_owned()),
    );
    let b = server.with(SettingsField::TunnelUrl, SettingsValue::Flag(true));

    // Then: nothing moves.
    assert_eq!(a, server);
    assert_eq!(b, server);
}
