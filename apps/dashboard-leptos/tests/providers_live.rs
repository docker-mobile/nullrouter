//! Boundary tests for the live Providers panel state.
//!
//! The panel these back replaced `provider_groups()` — fixtures that claimed
//! connections nobody had configured. Every test here is about the same
//! property: a card may only describe a row the router returned, and the absence
//! of rows must render as absence.

use nullrouter_dashboard_wasm::api::{ApiError, DetailedResponse};
use nullrouter_dashboard_wasm::dashboard::providers_live::{
    AuthKind, CONNECTIONS_PATH, Connection, ConnectionDraft, DeleteOutcome, DeleteSettlement,
    DraftError, TestOutcome, TestStatus, api_key_catalog, catalog, catalog_option, connection_path,
    connection_test_path, parse_connection, parse_connections, settle_delete, settle_test,
};

/// A body shaped like the state service's own output, including the fields it
/// omits via `skip_serializing_if` and the secrets it strips.
const REALISTIC_BODY: &str = r#"{
  "connections": [
    {
      "id": "connection_1717000000000_1",
      "provider": "anthropic",
      "authType": "apikey",
      "name": "Anthropic work key",
      "priority": 1,
      "isActive": true,
      "createdAt": "unix-ms:1717000000000",
      "updatedAt": "unix-ms:1717000500000",
      "testStatus": "active",
      "defaultModel": "claude-sonnet-4-20250514"
    },
    {
      "id": "connection_1717000000000_2",
      "provider": "openai",
      "authType": "apikey",
      "name": "OpenAI personal",
      "priority": 2,
      "isActive": false,
      "createdAt": "unix-ms:1717000000000",
      "updatedAt": "unix-ms:1717000000000",
      "testStatus": "error",
      "lastError": "401 invalid_api_key",
      "lastErrorAt": "unix-ms:1717000400000"
    },
    {
      "id": "connection_1717000000000_3",
      "provider": "claude",
      "authType": "oauth",
      "name": "Claude Code account",
      "priority": 1,
      "isActive": true,
      "createdAt": "unix-ms:1717000000000",
      "updatedAt": "unix-ms:1717000000000",
      "email": "dev@example.test"
    }
  ]
}"#;

#[test]
fn parses_a_realistic_connections_body_into_cards_that_match_the_rows() {
    // Given: the state service returns three configured connections.
    let list = parse_connections(REALISTIC_BODY).expect("realistic body parses");

    // Then: the panel holds exactly those rows, with the router's own values.
    assert_eq!(list.len(), 3);
    assert!(!list.is_empty());
    assert_eq!(list.active_count(), 2);
    assert_eq!(list.provider_count(), 3);

    let anthropic = list
        .connections()
        .iter()
        .find(|connection| connection.id == "connection_1717000000000_1")
        .expect("anthropic row present");
    assert_eq!(anthropic.name, "Anthropic work key");
    assert_eq!(anthropic.provider_label(), "Anthropic");
    assert_eq!(anthropic.auth_kind(), AuthKind::ApiKey);
    assert_eq!(anthropic.test_status(), TestStatus::Passing);
    assert_eq!(anthropic.priority_label(), "1");
    assert!(anthropic.is_active);
    // No email on an API-key row: named as absent, not filled in.
    assert_eq!(anthropic.account_label(), None);

    let openai = list
        .connections()
        .iter()
        .find(|connection| connection.id == "connection_1717000000000_2")
        .expect("openai row present");
    assert_eq!(openai.test_status(), TestStatus::Failing);
    assert_eq!(openai.last_error.as_deref(), Some("401 invalid_api_key"));
    assert!(!openai.is_active);

    let claude = list
        .connections()
        .iter()
        .find(|connection| connection.id == "connection_1717000000000_3")
        .expect("claude row present");
    assert_eq!(claude.auth_kind(), AuthKind::OAuth);
    assert_eq!(claude.account_label(), Some("dev@example.test"));
    // Never tested is its own state, distinct from a failure.
    assert_eq!(claude.test_status(), TestStatus::Untested);
}

#[test]
fn an_empty_connections_array_is_the_empty_state_and_never_a_fixture() {
    // Given: the router holds no connections. This is the bug being fixed: the
    // old panel could not tell this apart from "never asked", so it drew tiles.
    let list = parse_connections(r#"{"connections":[]}"#).expect("empty array is a valid answer");

    // Then: zero rows, zero groups, and nothing invented to fill the space.
    assert!(list.is_empty());
    assert_eq!(list.len(), 0);
    assert_eq!(list.active_count(), 0);
    assert_eq!(list.provider_count(), 0);
    assert!(list.connections().is_empty());
    assert!(list.groups().is_empty());
}

#[test]
fn the_registry_catalog_is_never_mistaken_for_configured_connections() {
    // Given: no connections, but a registry full of providers this build knows.
    let list = parse_connections(r#"{"connections":[]}"#).expect("empty array parses");
    let catalog = catalog();

    // Then: the catalog is populated and the connection list stays empty. The
    // two surfaces cannot substitute for one another.
    assert!(
        catalog.len() > 100,
        "expected the full registry, got {}",
        catalog.len()
    );
    assert!(list.is_empty());
    assert!(
        list.connections().is_empty(),
        "a catalog entry must never appear as a configured connection"
    );
}

#[test]
fn malformed_and_empty_bodies_fail_rather_than_read_as_no_providers() {
    // Given: bodies that are not a well-formed connections envelope.
    for body in [
        "",
        "   ",
        "null",
        "[]",
        "{}",
        "not json at all",
        r#"{"connections":{}}"#,
        r#"{"connections":[{"provider":"openai"}]}"#, // no id / name / authType / isActive
        r#"{"connections":[{"id":"c1","provider":"openai","authType":"apikey","name":"k"}]}"#, // no isActive
        r#"{"connections":[{"id":"c1","provider":"openai","name":"k","isActive":true}]}"#, // no authType
        r#"{"connections":[{"id":"c1","authType":"apikey","name":"k","isActive":true}]}"#, // no provider
        r#"{"connections":[{"id":"c1","provider":"openai","authType":"apikey","isActive":true}]}"#, // no name
        r#"{"connections":[{"id":"c1","provider":"openai","authType":"apikey","name":"k","isActive":"yes"}]}"#,
        r#"{"items":[]}"#, // right idea, wrong envelope
    ] {
        assert!(
            parse_connections(body).is_none(),
            "body should have failed the parse: {body}"
        );
    }
}

#[test]
fn a_connection_missing_every_optional_field_still_renders() {
    // Given: only the fields the API always sends.
    let body = r#"{"connections":[{
        "id":"connection_1",
        "provider":"ollama-local",
        "authType":"none",
        "name":"Local Ollama",
        "isActive":true
    }]}"#;
    let list = parse_connections(body).expect("minimal row parses");
    let connection = list.connections().first().expect("one row");

    // Then: the absent fields are named as absent rather than defaulted into a
    // claim. A missing priority is "unset", not the highest rank.
    assert_eq!(connection.priority, None);
    assert_eq!(connection.priority_label(), "unset");
    assert_eq!(connection.account_label(), None);
    assert_eq!(connection.default_model, None);
    assert_eq!(connection.last_error, None);
    assert_eq!(connection.created_at, None);
    assert_eq!(connection.test_status(), TestStatus::Untested);
    assert_eq!(connection.test_status().label(), "Never tested");
    assert_eq!(connection.auth_kind(), AuthKind::None);
    assert_eq!(
        connection.auth_kind().credential_note(),
        "This upstream needs no credential."
    );
}

#[test]
fn the_parsed_shape_has_no_room_for_a_secret() {
    // Given: a body that (contrary to the service, which strips them) carries
    // every secret field.
    let body = r#"{"connections":[{
        "id":"connection_1",
        "provider":"openai",
        "authType":"apikey",
        "name":"Leaky",
        "isActive":true,
        "apiKey":"sk-live-SHOULD-NOT-SURVIVE",
        "accessToken":"at-SHOULD-NOT-SURVIVE",
        "refreshToken":"rt-SHOULD-NOT-SURVIVE"
    }]}"#;
    let list = parse_connections(body).expect("unknown fields are ignored, not fatal");
    let connection = list.connections().first().expect("one row");

    // Then: nothing carried the secret through, so no rendering can leak it.
    let rendered = format!("{connection:?}");
    assert!(
        !rendered.contains("SHOULD-NOT-SURVIVE"),
        "a secret reached the parsed shape: {rendered}"
    );
    // And the card describes where the credential lives instead of faking one.
    assert_eq!(
        connection.auth_kind().credential_note(),
        "Key stored by the router. Never sent to this page."
    );
    assert!(
        !connection
            .auth_kind()
            .credential_note()
            .contains('\u{2022}'),
        "no invented masked value"
    );
}

#[test]
fn rows_sort_by_provider_then_priority_then_name_and_group_in_that_order() {
    // Given: rows returned in an arbitrary order, two of them sharing a provider
    // and one of them carrying no priority.
    let body = r#"{"connections":[
        {"id":"c-openai-9","provider":"openai","authType":"apikey","name":"zeta","isActive":true,"priority":9},
        {"id":"c-openai-none","provider":"openai","authType":"apikey","name":"alpha","isActive":true},
        {"id":"c-openai-1","provider":"openai","authType":"apikey","name":"beta","isActive":true,"priority":1},
        {"id":"c-anthropic","provider":"anthropic","authType":"apikey","name":"anthropic key","isActive":true,"priority":5},
        {"id":"c-openai-1b","provider":"openai","authType":"apikey","name":"Beta second","isActive":false,"priority":1}
    ]}"#;
    let list = parse_connections(body).expect("body parses");

    // Then: providers order by display name (Anthropic before OpenAI), then
    // priority ascending with "unset" last, then name case-insensitively.
    let order: Vec<&str> = list
        .connections()
        .iter()
        .map(|connection| connection.id.as_str())
        .collect();
    assert_eq!(
        order,
        [
            "c-anthropic",
            "c-openai-1",
            "c-openai-1b",
            "c-openai-9",
            "c-openai-none",
        ]
    );

    // And grouping preserves that order without inventing an empty group.
    let groups = list.groups();
    assert_eq!(groups.len(), 2);
    let first = groups.first().expect("first group");
    assert_eq!(first.provider, "anthropic");
    assert_eq!(first.label, "Anthropic");
    assert_eq!(first.connections.len(), 1);
    assert_eq!(first.summary(), "1 of 1 active");

    let second = groups.get(1).expect("second group");
    assert_eq!(second.provider, "openai");
    assert_eq!(second.label, "OpenAI");
    assert_eq!(second.connections.len(), 4);
    assert_eq!(second.active_count(), 3);
    assert_eq!(second.summary(), "3 of 4 active");
    assert!(
        groups.iter().all(|group| !group.connections.is_empty()),
        "a group only exists because rows exist"
    );
}

#[test]
fn an_unknown_provider_id_is_shown_as_itself_rather_than_hidden() {
    // Given: a connection naming a provider this build's registry does not have.
    let body = r#"{"connections":[{
        "id":"c1","provider":"retired-upstream","authType":"apikey","name":"Old key","isActive":true
    }]}"#;
    let list = parse_connections(body).expect("body parses");
    let connection = list.connections().first().expect("one row");

    // Then: the row survives and reports the raw id. The connection is real even
    // when the provider is unrecognised.
    assert_eq!(connection.provider_label(), "retired-upstream");
    assert_eq!(list.groups().len(), 1);
}

#[test]
fn a_failed_delete_restores_the_row_at_its_original_position() {
    // Given: a list where the middle row is deleted optimistically.
    let mut list = parse_connections(REALISTIC_BODY).expect("body parses");
    let before: Vec<String> = list
        .connections()
        .iter()
        .map(|connection| connection.id.clone())
        .collect();
    let target = before.get(1).cloned().expect("a second row exists");

    let pending = list.take(&target).expect("row was present");
    assert_eq!(pending.id(), target);
    assert_eq!(list.len(), 2);
    assert!(
        !list
            .connections()
            .iter()
            .any(|connection| connection.id == target),
        "the row is gone from the panel immediately"
    );

    // When: the DELETE is refused.
    let settlement = settle_delete(pending, DeleteOutcome::Rejected(ApiError::Status(500)));

    // Then: the row comes back exactly where it was, and the error is reported.
    match settlement {
        DeleteSettlement::RolledBack { pending, error } => {
            assert_eq!(error, ApiError::Status(500));
            list.restore(pending);
        }
        DeleteSettlement::Removed => panic!("a refused delete must not stand"),
    }
    let after: Vec<String> = list
        .connections()
        .iter()
        .map(|connection| connection.id.clone())
        .collect();
    assert_eq!(after, before);
}

#[test]
fn a_confirmed_delete_stands_and_a_404_is_treated_as_already_gone() {
    let mut list = parse_connections(REALISTIC_BODY).expect("body parses");
    let first = list
        .connections()
        .first()
        .map(|connection| connection.id.clone())
        .expect("a row exists");

    let pending = list.take(&first).expect("row was present");
    assert_eq!(
        settle_delete(pending, DeleteOutcome::Confirmed),
        DeleteSettlement::Removed
    );
    assert_eq!(list.len(), 2);

    // A 404 means the connection is not there, which is what the panel shows.
    let second = list
        .connections()
        .first()
        .map(|connection| connection.id.clone())
        .expect("a row exists");
    let pending = list.take(&second).expect("row was present");
    assert_eq!(
        settle_delete(pending, DeleteOutcome::Rejected(ApiError::Status(404))),
        DeleteSettlement::Removed
    );
    assert_eq!(list.len(), 1);

    // Taking a row that is not there yields nothing to roll back.
    assert!(list.take("connection_does_not_exist").is_none());
}

#[test]
fn a_created_connection_is_inserted_in_order_without_duplicating_it() {
    let mut list = parse_connections(REALISTIC_BODY).expect("body parses");
    let created = parse_connection(
        r#"{"connection":{
            "id":"connection_new","provider":"anthropic","authType":"apikey",
            "name":"AAA first","isActive":true,"priority":0
        }}"#,
    )
    .expect("create response parses");

    list.insert(created.clone());
    assert_eq!(list.len(), 4);
    assert_eq!(
        list.connections()
            .first()
            .map(|connection| connection.id.as_str()),
        Some("connection_new"),
        "priority 0 on the first provider sorts to the top"
    );

    // A refresh that re-delivers the same row must not double it.
    list.insert(created);
    assert_eq!(list.len(), 4);
}

/// A response as `api::request_detailed` reports it.
fn reply(status: u16, body: &str) -> DetailedResponse {
    DetailedResponse {
        status,
        ok: (200..300).contains(&status),
        retry_after: None,
        body: body.to_owned(),
    }
}

#[test]
fn a_test_result_separates_a_refused_key_from_a_test_that_never_ran() {
    // Given: `/api/providers/{id}/test` now makes a real one-token upstream call.
    // It answers 200 on a pass, 502 when the provider refuses, and 503/400/404 when
    // nothing could be tested. The old `501 unsupported` stub is gone.

    // A pass.
    assert_eq!(
        settle_test(Ok(reply(
            200,
            r#"{"success":true,"valid":true,"status":200}"#
        ))),
        TestOutcome::Passed
    );
    assert_eq!(TestOutcome::Passed.recorded_status(), Some("active"));

    // A refusal relays the provider's own message, because "invalid key" and "model
    // not found" send the user to different places.
    assert_eq!(
        settle_test(Ok(reply(
            502,
            r#"{"success":false,"status":401,"error":"Incorrect API key provided"}"#
        ))),
        TestOutcome::Failed(String::from("Incorrect API key provided"))
    );
    // A refusal is a verdict on the credential, so it is recorded.
    assert_eq!(
        TestOutcome::Failed(String::new()).recorded_status(),
        Some("error")
    );

    // A down state service tested nothing. Recording "error" here would tell the user
    // to replace a key that may be perfectly good.
    let unavailable = settle_test(Ok(reply(
        503,
        r#"{"success":false,"error":"The state service is unreachable, so this connection could not be read"}"#,
    )));
    assert_eq!(
        unavailable,
        TestOutcome::NotTested(String::from(
            "The state service is unreachable, so this connection could not be read"
        ))
    );
    assert_eq!(unavailable.recorded_status(), None);
    assert!(
        unavailable.message().contains("Nothing was tested"),
        "got {}",
        unavailable.message()
    );

    // Same for a connection with no model, and for one that does not exist.
    assert_eq!(
        settle_test(Ok(reply(
            400,
            r#"{"success":false,"error":"This connection names no model to test: set a default model first"}"#
        )))
        .recorded_status(),
        None
    );
    assert_eq!(
        settle_test(Ok(reply(
            404,
            r#"{"success":false,"error":"No such provider connection"}"#
        ))),
        TestOutcome::NotTested(String::from("No such provider connection"))
    );

    // A refusal with no message still names the status rather than inventing a cause.
    assert_eq!(
        settle_test(Ok(reply(502, "{}"))),
        TestOutcome::Failed(String::from("the router answered 502"))
    );
    // A non-2xx whose body is not JSON at all (an HTML error page, say) tested nothing.
    assert_eq!(
        settle_test(Ok(reply(503, "<html>gateway</html>"))),
        TestOutcome::NotTested(String::from("the router answered 503"))
    );

    // A 2xx that reports failure in the body is believed over its status.
    assert_eq!(
        settle_test(Ok(reply(200, r#"{"valid":false}"#))),
        TestOutcome::Failed(String::from("the upstream rejected the credential"))
    );

    // Transport failures are neither a pass nor a verdict on the credential.
    assert_eq!(
        settle_test(Err(ApiError::Network)),
        TestOutcome::Rejected(ApiError::Network)
    );
    // A 2xx that cannot be parsed cannot be called a pass.
    assert_eq!(
        settle_test(Ok(reply(200, "not json"))),
        TestOutcome::Rejected(ApiError::Body)
    );
    assert_eq!(
        TestOutcome::Rejected(ApiError::Network).recorded_status(),
        None
    );
}

#[test]
fn a_draft_is_validated_against_the_endpoints_own_rules_before_a_request() {
    // Nothing chosen yet.
    assert_eq!(
        ConnectionDraft::default().validation_error(),
        Some(DraftError::ProviderMissing)
    );
    // A provider the registry does not carry.
    assert_eq!(
        ConnectionDraft {
            provider: String::from("not-a-provider"),
            name: String::new(),
            api_key: String::from("sk-test"),
        }
        .validation_error(),
        Some(DraftError::ProviderUnknown)
    );
    // A key-requiring provider with no key: the state service rejects this, so
    // the form says so first.
    assert_eq!(
        ConnectionDraft {
            provider: String::from("openai"),
            name: String::from("Work"),
            api_key: String::from("   "),
        }
        .validation_error(),
        Some(DraftError::ApiKeyMissing)
    );
    // `ollama-local` is the one provider the service creates without a key.
    let ollama = ConnectionDraft {
        provider: String::from("ollama-local"),
        name: String::new(),
        api_key: String::new(),
    };
    assert_eq!(ollama.validation_error(), None);
    assert_eq!(
        ollama.create_body().expect("body builds"),
        r#"{"provider":"ollama-local"}"#,
        "a blank name is omitted so the service applies its own default"
    );
}

#[test]
fn the_create_body_escapes_user_text_instead_of_interpolating_it() {
    let draft = ConnectionDraft {
        provider: String::from("openai"),
        name: String::from(r#"He said "hi" \ then left"#),
        api_key: String::from(r#"sk-"quote"-\slash"#),
    };
    let body = draft.create_body().expect("body builds");
    // Valid JSON with the text intact, not a broken payload.
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("body is valid JSON");
    assert_eq!(
        parsed.get("provider").and_then(|value| value.as_str()),
        Some("openai")
    );
    assert_eq!(
        parsed.get("name").and_then(|value| value.as_str()),
        Some(r#"He said "hi" \ then left"#)
    );
    assert_eq!(
        parsed.get("apiKey").and_then(|value| value.as_str()),
        Some(r#"sk-"quote"-\slash"#)
    );
}

#[test]
fn the_create_form_only_offers_providers_it_can_actually_create() {
    let options = api_key_catalog();
    assert!(!options.is_empty());
    // OAuth and cookie providers are excluded: the create endpoint is API-key
    // only, and offering them would promise a flow this build does not run.
    assert!(
        options.iter().all(|option| option.api_key_capable),
        "an OAuth-only provider reached the create form"
    );
    assert!(options.iter().all(|option| option.id != "claude"));
    assert!(options.iter().all(|option| option.id != "grok-web"));
    assert!(options.iter().any(|option| option.id == "openai"));
    assert!(options.iter().any(|option| option.id == "ollama-local"));

    // Sorted by display name so a 100-plus list is navigable.
    let names: Vec<String> = options
        .iter()
        .map(|option| option.name.to_ascii_lowercase())
        .collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);

    // The full catalog still lists the excluded providers, with the reason.
    let claude = catalog_option("claude").expect("claude is in the registry");
    assert!(!claude.api_key_capable);
    assert_eq!(claude.auth_label(), "OAuth or browser sign-in");
    assert!(claude.unavailable_note().is_some());
    let openai = catalog_option("openai").expect("openai is in the registry");
    assert_eq!(openai.auth_label(), "API key");
    assert!(openai.unavailable_note().is_none());
    assert!(openai.model_count > 0);
    let ollama = catalog_option("ollama-local").expect("ollama-local is in the registry");
    assert!(!ollama.requires_api_key);
    assert_eq!(ollama.auth_label(), "No credential");
}

#[test]
fn connection_paths_target_the_documented_endpoints_and_encode_the_id() {
    assert_eq!(CONNECTIONS_PATH, "/api/providers");
    assert_eq!(
        connection_path("connection_1717000000000_1"),
        "/api/providers/connection_1717000000000_1"
    );
    assert_eq!(
        connection_test_path("connection_1717000000000_1"),
        "/api/providers/connection_1717000000000_1/test"
    );
    // An id containing path or query characters cannot escape its segment.
    assert_eq!(
        connection_path("../settings?x=1"),
        "/api/providers/..%2Fsettings%3Fx%3D1"
    );
}

#[test]
fn every_status_and_auth_kind_reads_as_a_sentence_the_user_can_act_on() {
    for status in [
        TestStatus::Passing,
        TestStatus::Failing,
        TestStatus::Unavailable,
        TestStatus::Expired,
        TestStatus::Testing,
        TestStatus::Untested,
    ] {
        assert!(!status.label().is_empty(), "{status:?} has no label");
        assert!(
            status.class_name().starts_with("is-"),
            "{status:?} must reuse the shared pill vocabulary"
        );
    }
    for kind in [
        AuthKind::ApiKey,
        AuthKind::OAuth,
        AuthKind::Cookie,
        AuthKind::None,
        AuthKind::Other(String::from("custom")),
    ] {
        assert!(!kind.label().is_empty(), "{kind:?} has no label");
        assert!(
            kind.credential_note().ends_with('.'),
            "{kind:?} note should read as a sentence"
        );
    }
    for error in [
        DraftError::ProviderMissing,
        DraftError::ApiKeyMissing,
        DraftError::ProviderUnknown,
    ] {
        assert!(
            error.message().ends_with('.'),
            "{error:?} is not a sentence"
        );
    }
}

#[test]
fn cards_carry_labels_that_name_the_row_they_act_on() {
    let list = parse_connections(REALISTIC_BODY).expect("body parses");
    let connection = list.connections().first().expect("a row exists");

    // A bare "Delete" is ambiguous with several cards on screen, and this action
    // is irreversible.
    assert_eq!(
        connection.delete_label(),
        "Delete connection Anthropic work key (Anthropic)"
    );
    assert_eq!(
        connection.test_label(),
        "Test connection Anthropic work key"
    );
    // DOM ids tie a card to its heading and its live status region.
    assert!(
        connection
            .heading_id()
            .starts_with("nr-connection-heading-")
    );
    assert!(connection.status_id().starts_with("nr-connection-status-"));
    assert_ne!(connection.heading_id(), connection.status_id());

    // Ids with characters that are illegal in a DOM id are reduced, and stay
    // distinct per row.
    let odd = parse_connections(
        r#"{"connections":[
            {"id":"a b/c","provider":"openai","authType":"apikey","name":"one","isActive":true},
            {"id":"a-b-c","provider":"openai","authType":"apikey","name":"two","isActive":true}
        ]}"#,
    )
    .expect("body parses");
    let ids: Vec<String> = odd
        .connections()
        .iter()
        .map(Connection::heading_id)
        .collect();
    assert!(ids.iter().all(|id| !id.contains(' ') && !id.contains('/')));
}

#[test]
fn a_test_result_updates_only_the_row_it_belongs_to() {
    let mut list = parse_connections(REALISTIC_BODY).expect("body parses");
    let target = list
        .connections()
        .first()
        .map(|connection| connection.id.clone())
        .expect("a row exists");
    let others_before: Vec<Option<String>> = list
        .connections()
        .iter()
        .skip(1)
        .map(|connection| connection.test_status.clone())
        .collect();

    list.set_test_status(&target, Some(String::from("error")));

    let updated = list
        .connections()
        .iter()
        .find(|connection| connection.id == target)
        .expect("the row is still there");
    assert_eq!(updated.test_status(), TestStatus::Failing);

    let others_after: Vec<Option<String>> = list
        .connections()
        .iter()
        .filter(|connection| connection.id != target)
        .map(|connection| connection.test_status.clone())
        .collect();
    assert_eq!(
        others_after, others_before,
        "testing one connection must not restate another one's result"
    );

    // An unknown id changes nothing, so a stale in-flight test cannot write to a
    // row that was deleted while it ran.
    let snapshot = list.clone();
    list.set_test_status("connection_does_not_exist", Some(String::from("active")));
    assert_eq!(list, snapshot);
}
