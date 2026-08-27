//! Behaviour tests for the 9Router import panel.
//!
//! Two properties are load-bearing and are asserted here rather than trusted to
//! review: an import cannot run before a preview succeeded, and no count reaches
//! the screen that the server did not send. Everything else in this file exists
//! to pin the shapes those two rules depend on.

#![allow(
    clippy::panic,
    clippy::missing_const_for_fn,
    clippy::items_after_statements,
    clippy::redundant_closure,
    clippy::case_sensitive_file_extension_comparisons,
    reason = "integration-test file: the workspace `allow-*-in-tests` settings only reach `#[cfg(test)]` modules"
)]
use nullrouter_dashboard_wasm::api::ApiError;
use nullrouter_dashboard_wasm::dashboard::migrate::{
    ImportGate, ImportReport, Outcome, Phase, import_gate, parse_response, request_body,
    status_line,
};

/// A body in the exact shape `services/state-actix` emits for a dry run.
const DRY_RUN_BODY: &str = r#"{
  "ok": true,
  "dryRun": true,
  "report": {
    "source": "/home/user/.9router/db/data.sqlite",
    "format": "sqlite",
    "connectionsFound": 7,
    "connectionsImported": 5,
    "combosFound": 3,
    "combosImported": 3,
    "proxyPoolsFound": 2,
    "proxyPoolsImported": 1,
    "apiKeysFound": 4,
    "apiKeysImported": 0,
    "settingsImported": true,
    "warnings": [
      "4 API key(s) found but not imported: nullrouter stores key digests, so existing keys cannot be re-derived. Re-issue them from the dashboard.",
      "skipped existing connection openai/work",
      "skipped a combo with no models"
    ]
  }
}"#;

/// The 404 envelope, including the searched-paths suffix.
const NOT_FOUND_BODY: &str = r#"{
  "ok": false,
  "error": "no_9router_installation",
  "message": "No 9Router installation found. Searched: /opt/9router, /home/user/.9router"
}"#;

fn dry_run_report() -> ImportReport {
    match parse_response(200, DRY_RUN_BODY) {
        Ok(Outcome::Completed { report, .. }) => report,
        other => panic!("expected a completed dry run, got {other:?}"),
    }
}

#[test]
fn realistic_dry_run_body_parses_into_every_count() {
    // Given: the server's own dry-run envelope.
    // When: the panel decodes it.
    let outcome = parse_response(200, DRY_RUN_BODY).expect("body must decode");

    // Then: it is recognised as a preview, not a completed write.
    assert!(outcome.is_dry_run(), "dryRun:true must be carried through");
    let report = outcome.report().expect("a completed run carries a report");

    // And: every field lands where the table reads it from.
    assert_eq!(report.source, "/home/user/.9router/db/data.sqlite");
    assert_eq!(report.format, "sqlite");
    assert_eq!(report.connections_found, 7);
    assert_eq!(report.connections_imported, 5);
    assert_eq!(report.combos_found, 3);
    assert_eq!(report.combos_imported, 3);
    assert_eq!(report.proxy_pools_found, 2);
    assert_eq!(report.proxy_pools_imported, 1);
    assert_eq!(report.api_keys_found, 4);
    assert_eq!(report.api_keys_imported, 0);
    assert!(report.settings_imported);
    assert_eq!(report.warnings.len(), 3);
}

#[test]
fn preview_rows_carry_found_importable_and_skipped_per_kind() {
    let rows = dry_run_report().rows();

    let labels: Vec<&str> = rows.iter().map(|row| row.label).collect();
    assert_eq!(
        labels,
        ["Provider connections", "Combos", "Proxy pools", "API keys"],
        "all four record kinds must appear in the preview table"
    );

    let connections = rows.first().expect("connections row");
    assert_eq!((connections.found, connections.importable), (7, 5));
    // 7 found, 5 importable: the user must be able to see the 2 that will not
    // come across, not just the two totals.
    assert_eq!(connections.skipped(), 2);

    let keys = rows.get(3).expect("api keys row");
    assert_eq!((keys.found, keys.importable), (4, 0));
    assert_eq!(keys.skipped(), 4);
    assert!(
        keys.note.is_some_and(
            |note| note.contains("re-issue") || note.to_lowercase().contains("re-issue")
        ),
        "the API-key row must say the keys have to be re-issued, got {:?}",
        keys.note
    );
    assert!(
        rows.iter().take(3).all(|row| row.note.is_none()),
        "only API keys carry a not-importable note"
    );
}

#[test]
fn api_keys_never_count_toward_what_would_be_written() {
    // A store holding nothing but API keys has nothing importable, because the
    // server never writes them. If they counted, Import would open on a preview
    // that could not write a single record.
    let keys_only = parse_response(
        200,
        r#"{"ok":true,"dryRun":true,"report":{
            "source":"/x/db.json","format":"json",
            "connectionsFound":0,"connectionsImported":0,
            "combosFound":0,"combosImported":0,
            "proxyPoolsFound":0,"proxyPoolsImported":0,
            "apiKeysFound":9,"apiKeysImported":0,
            "settingsImported":false,"warnings":[]}}"#,
    )
    .expect("decodes");
    let report = keys_only.report().expect("report");

    assert_eq!(report.api_keys_found, 9);
    assert_eq!(report.pending_writes(), 0);
    assert!(!report.found_nothing(), "9 keys were found at the source");
    assert_eq!(
        import_gate(Some(&keys_only), false, false),
        ImportGate::NothingToImport
    );

    // Settings alone, by contrast, are a real write and must open the gate.
    let settings_only = parse_response(
        200,
        r#"{"ok":true,"dryRun":true,"report":{
            "source":"/x/db.json","format":"json",
            "connectionsFound":0,"connectionsImported":0,
            "combosFound":0,"combosImported":0,
            "proxyPoolsFound":0,"proxyPoolsImported":0,
            "apiKeysFound":0,"apiKeysImported":0,
            "settingsImported":true,"warnings":[]}}"#,
    )
    .expect("decodes");
    assert_eq!(
        settings_only.report().expect("report").pending_writes(),
        1,
        "imported settings are a write and must not be ignored"
    );
    assert!(import_gate(Some(&settings_only), false, false).allows_import());
}

#[test]
fn not_found_is_a_state_with_paths_not_an_error() {
    // Given: the 404 the server returns when discovery finds nothing.
    let outcome = parse_response(404, NOT_FOUND_BODY).expect("404 must be a state, not an error");

    // Then: it is the Missing state, carrying the message verbatim.
    let Outcome::Missing(missing) = outcome else {
        panic!("404 with no_9router_installation must decode as Missing, got {outcome:?}");
    };
    assert!(
        missing.message.contains("No 9Router installation found"),
        "the server's message must survive verbatim"
    );
    // And: each probed directory is listed separately so the user can see where
    // to point the override.
    assert_eq!(
        missing.searched,
        ["/opt/9router", "/home/user/.9router"],
        "both searched directories must be extracted"
    );
}

#[test]
fn not_found_without_a_searched_suffix_still_renders_its_message() {
    // Older or differently-worded messages must not be dropped just because no
    // path list could be split out of them.
    let outcome = parse_response(
        404,
        r#"{"ok":false,"error":"no_9router_installation","message":"Nothing here."}"#,
    )
    .expect("still a state");
    let Outcome::Missing(missing) = outcome else {
        panic!("expected Missing");
    };
    assert_eq!(missing.message, "Nothing here.");
    assert!(missing.searched.is_empty());
}

#[test]
fn a_refusal_is_reported_with_its_code_and_message() {
    // 503 from the api layer when the state service is down.
    let outcome = parse_response(
        503,
        r#"{"ok":false,"error":"state_unavailable","message":"The state service is unreachable, so no import was attempted."}"#,
    )
    .expect("a described refusal is renderable");

    let Outcome::Refused(rejected) = outcome else {
        panic!("expected Refused, got {outcome:?}");
    };
    assert_eq!(rejected.error, "state_unavailable");
    assert!(rejected.message.contains("no import was attempted"));
    assert!(
        rejected_never_looks_importable(
            &parse_response(
                503,
                r#"{"ok":false,"error":"state_unavailable","message":"down"}"#
            )
            .expect("decodes")
        ),
        "a refusal must never expose a report"
    );
}

fn rejected_never_looks_importable(outcome: &Outcome) -> bool {
    outcome.report().is_none() && !import_gate(Some(outcome), false, false).allows_import()
}

#[test]
fn malformed_and_empty_bodies_fail_instead_of_decoding_to_zeros() {
    // Every one of these must surface as a failure. An empty table of zeros
    // would tell a user they have nothing to migrate, which is a different and
    // wrong statement.
    for body in [
        "",
        "   ",
        "not json at all",
        "{",
        "[]",
        "null",
        "{}",
        r#"{"ok":true}"#,
        r#"{"unrelated":1}"#,
        r#"{"ok":false}"#,
    ] {
        assert_eq!(
            parse_response(200, body),
            Err(ApiError::Body),
            "a 200 with body {body:?} must be a decode failure"
        );
    }

    // A partial report is also a failure: the server always sends every count,
    // so a missing one means the shape changed.
    assert_eq!(
        parse_response(
            200,
            r#"{"ok":true,"dryRun":true,"report":{"source":"/x","format":"json","connectionsFound":1}}"#
        ),
        Err(ApiError::Body),
        "a report missing counts must not be rendered with the rest defaulted to 0"
    );
}

#[test]
fn an_unreadable_body_on_an_error_status_reports_the_status() {
    // An HTML error page or a proxy's plain-text 502 carries no envelope; the
    // status is the only actionable thing left.
    assert_eq!(
        parse_response(502, "<html>bad gateway</html>"),
        Err(ApiError::Status(502))
    );
    assert_eq!(parse_response(401, ""), Err(ApiError::Status(401)));
    // And the message for it must tell the user what to do.
    assert!(ApiError::Status(401).message().contains("sign in"));
}

#[test]
fn a_report_with_zero_findings_reads_as_empty_not_as_a_failure() {
    // An installation that exists but holds nothing is a real, distinct answer.
    let outcome = parse_response(
        200,
        r#"{"ok":true,"dryRun":true,"report":{
            "source":"/home/user/.9router/db.json","format":"json",
            "connectionsFound":0,"connectionsImported":0,
            "combosFound":0,"combosImported":0,
            "proxyPoolsFound":0,"proxyPoolsImported":0,
            "apiKeysFound":0,"apiKeysImported":0,
            "settingsImported":false,"warnings":[]}}"#,
    )
    .expect("an empty install still decodes");

    let report = outcome.report().expect("report");
    assert!(report.found_nothing());
    assert_eq!(report.pending_writes(), 0);
    assert!(report.warnings.is_empty());
    // The source is still shown, so the user can tell *which* install was empty.
    assert_eq!(report.source, "/home/user/.9router/db.json");
    assert_eq!(report.format, "json");
    // Every row is present with a zero, rather than the table being absent.
    assert_eq!(report.rows().len(), 4);
    assert!(report.rows().iter().all(|row| row.found == 0));

    // And Import stays shut: there is nothing to write.
    assert_eq!(
        import_gate(Some(&outcome), false, false),
        ImportGate::NothingToImport
    );
    assert!(
        import_gate(Some(&outcome), false, false)
            .blocked_reason()
            .is_some_and(|reason| reason.contains("nothing"))
    );
}

#[test]
fn every_warning_survives_parsing_including_duplicates() {
    // A big migration emits one warning per skipped record. None may be dropped
    // or merged: the warning text is the only place the record's name appears.
    let warnings: Vec<String> = (0..64)
        .map(|index| format!("skipped existing connection openai/acct-{index}"))
        .chain((0..3).map(|_| "skipped existing combo daily".to_owned()))
        .collect();
    let body = serde_json::json!({
        "ok": true,
        "dryRun": false,
        "report": {
            "source": "/home/user/.9router/db/data.sqlite",
            "format": "sqlite",
            "connectionsFound": 70, "connectionsImported": 6,
            "combosFound": 4, "combosImported": 1,
            "proxyPoolsFound": 0, "proxyPoolsImported": 0,
            "apiKeysFound": 0, "apiKeysImported": 0,
            "settingsImported": true,
            "warnings": warnings,
        }
    })
    .to_string();

    let outcome = parse_response(200, &body).expect("decodes");
    let report = outcome.report().expect("report");
    assert_eq!(report.warnings.len(), 67, "no warning may be dropped");
    // The three identical warnings must all be present, not de-duplicated.
    assert_eq!(
        report
            .warnings
            .iter()
            .filter(|warning| *warning == "skipped existing combo daily")
            .count(),
        3
    );
    assert!(!outcome.is_dry_run(), "dryRun:false is a real import");
}

#[test]
fn an_absent_warnings_field_is_no_warnings_not_a_failure() {
    // `warnings` is the one field allowed to default: an omitted empty list is
    // unambiguous, unlike an omitted count.
    let report = parse_response(
        200,
        r#"{"ok":true,"dryRun":true,"report":{
            "source":"/x","format":"json",
            "connectionsFound":1,"connectionsImported":1,
            "combosFound":0,"combosImported":0,
            "proxyPoolsFound":0,"proxyPoolsImported":0,
            "apiKeysFound":0,"apiKeysImported":0,
            "settingsImported":false}}"#,
    )
    .expect("decodes without warnings")
    .report()
    .cloned()
    .expect("report");
    assert!(report.warnings.is_empty());
    assert_eq!(report.pending_writes(), 1);
}

#[test]
fn import_is_gated_until_a_preview_succeeds() {
    // No preview yet: the gate is shut and says why.
    assert_eq!(import_gate(None, false, false), ImportGate::NeedsPreview);
    assert!(!import_gate(None, false, false).allows_import());
    assert!(
        import_gate(None, false, false)
            .blocked_reason()
            .is_some_and(|reason| reason.contains("preview")),
        "the reason must name the missing preview"
    );

    // A preview that found nothing does not open it either.
    let missing = parse_response(404, NOT_FOUND_BODY).expect("state");
    assert_eq!(
        import_gate(Some(&missing), false, false),
        ImportGate::NeedsPreview
    );

    // A refused preview does not open it.
    let refused = parse_response(
        503,
        r#"{"ok":false,"error":"state_unavailable","message":"down"}"#,
    )
    .expect("state");
    assert_eq!(
        import_gate(Some(&refused), false, false),
        ImportGate::NeedsPreview
    );

    // A successful preview with records to write opens it.
    let previewed = parse_response(200, DRY_RUN_BODY).expect("decodes");
    assert_eq!(
        import_gate(Some(&previewed), false, false),
        ImportGate::Ready
    );
    assert!(import_gate(Some(&previewed), false, false).allows_import());
    assert!(
        import_gate(Some(&previewed), false, false)
            .blocked_reason()
            .is_none(),
        "an open gate has nothing to explain"
    );
}

#[test]
fn a_request_in_flight_shuts_the_gate_against_a_double_submit() {
    let previewed = parse_response(200, DRY_RUN_BODY).expect("decodes");
    // Even with a good preview, an open request blocks a second one.
    assert_eq!(import_gate(Some(&previewed), true, false), ImportGate::Busy);
    assert!(!import_gate(Some(&previewed), true, false).allows_import());
    // Busy wins over every other reason, including having no preview at all.
    assert_eq!(import_gate(None, true, false), ImportGate::Busy);
}

#[test]
fn a_consumed_preview_must_be_re_scanned_before_importing_again() {
    let previewed = parse_response(200, DRY_RUN_BODY).expect("decodes");

    // Once an import has run against this preview, the counts describe work
    // already done; pressing Import again would act on stale numbers.
    assert_eq!(
        import_gate(Some(&previewed), false, true),
        ImportGate::AlreadyImported
    );
    assert!(!import_gate(Some(&previewed), false, true).allows_import());
    assert!(
        import_gate(Some(&previewed), false, true)
            .blocked_reason()
            .is_some_and(|reason| reason.to_lowercase().contains("re-scan")),
        "the reason must tell the user to re-scan"
    );

    // A fresh scan clears the flag and reopens the gate.
    assert!(import_gate(Some(&previewed), false, false).allows_import());
}

#[test]
fn every_shut_gate_explains_itself_and_the_open_one_does_not() {
    for gate in [
        ImportGate::Busy,
        ImportGate::NeedsPreview,
        ImportGate::NothingToImport,
        ImportGate::AlreadyImported,
    ] {
        assert!(!gate.allows_import(), "{gate:?} must not allow an import");
        let reason = gate.blocked_reason().unwrap_or_default();
        assert!(!reason.is_empty(), "{gate:?} must explain itself");
        assert!(
            reason.ends_with('.'),
            "{gate:?} reason should read as a sentence: {reason}"
        );
    }
    assert!(ImportGate::Ready.allows_import());
    assert_eq!(ImportGate::Ready.blocked_reason(), None);
}

#[test]
fn the_request_body_sends_null_for_the_default_location() {
    // A blank field must not become an empty path, or the server would probe ""
    // instead of running its own discovery.
    for blank in ["", "   ", "\t\n"] {
        let body: serde_json::Value =
            serde_json::from_str(&request_body(blank, true)).expect("valid JSON");
        assert_eq!(body.get("dataDir"), Some(&serde_json::Value::Null));
        assert_eq!(body.get("dryRun"), Some(&serde_json::Value::Bool(true)));
    }

    // A supplied directory is trimmed and sent as-is.
    let body: serde_json::Value =
        serde_json::from_str(&request_body("  /srv/9router  ", false)).expect("valid JSON");
    assert_eq!(
        body.get("dataDir").and_then(serde_json::Value::as_str),
        Some("/srv/9router")
    );
    assert_eq!(body.get("dryRun"), Some(&serde_json::Value::Bool(false)));
}

#[test]
fn the_status_line_names_the_phase_and_then_the_result() {
    // In flight: the two actions are distinguishable, so a user knows whether a
    // write is happening.
    let scanning = status_line(Some(Phase::Scan), None);
    let importing = status_line(Some(Phase::Import), None);
    assert!(scanning.to_lowercase().contains("scanning"), "{scanning}");
    assert!(importing.to_lowercase().contains("import"), "{importing}");
    assert_ne!(scanning, importing);

    // A phase in flight wins over a stale preview, so the line never claims a
    // finished result while a request is open.
    let previewed = parse_response(200, DRY_RUN_BODY).expect("decodes");
    assert_eq!(status_line(Some(Phase::Scan), Some(&previewed)), scanning);

    // Settled: the line names the source and how much would be written.
    let settled = status_line(None, Some(&previewed));
    assert!(
        settled.contains("/home/user/.9router/db/data.sqlite"),
        "{settled}"
    );
    assert!(settled.contains("sqlite"), "{settled}");
    // 5 connections + 3 combos + 1 pool + settings = 10 writes.
    assert!(settled.contains("10"), "{settled}");

    // Not found reads as a state, with no numbers in it.
    let missing = parse_response(404, NOT_FOUND_BODY).expect("state");
    let missing_line = status_line(None, Some(&missing));
    assert!(
        missing_line.contains("No 9Router installation found"),
        "{missing_line}"
    );

    // Idle before the first scan resolves.
    assert_eq!(status_line(None, None), "Idle.");
}

#[test]
fn the_panel_states_the_api_key_caveat_outside_the_warnings_list() {
    // The caveat has to be visible to someone who never scrolls to the report,
    // so it lives in the panel's own markup rather than only in server warnings.
    const UI: &str = include_str!("../src/ui/migrate.rs");

    let caveat = UI
        .split("fn ApiKeyCaveat")
        .nth(1)
        .and_then(|tail| tail.split("fn ScanControls").next())
        .unwrap_or_default();
    assert!(!caveat.is_empty(), "the caveat component must exist");
    assert!(
        caveat.contains("will not be imported"),
        "the caveat must say plainly that keys do not transfer"
    );
    assert!(
        caveat.contains("re-issue") || caveat.contains("re-issued"),
        "the caveat must say what to do about it"
    );
    assert!(
        caveat.contains("digest"),
        "the caveat must say why, so it does not read as an arbitrary limitation"
    );
    // It is rendered unconditionally: no `Show` wrapping it away.
    assert!(
        !caveat.contains("<Show"),
        "the caveat must not be conditional on any state"
    );

    // The panel renders it before the controls, not after the report.
    let panel = UI
        .split("pub(super) fn MigratePanel")
        .nth(1)
        .and_then(|tail| tail.split("fn ApiKeyCaveat").next())
        .unwrap_or_default();
    let caveat_at = panel.find("<ApiKeyCaveat");
    let controls_at = panel.find("<ScanControls");
    assert!(
        caveat_at.is_some() && caveat_at < controls_at,
        "the caveat must be rendered above the import controls"
    );
}

/// Every `nr-…` class the panel puts in its markup.
fn classes_used_by_the_panel() -> Vec<String> {
    const UI: &str = include_str!("../src/ui/migrate.rs");
    let mut classes = Vec::new();
    // `class="a b c"` and `class=format!("… {}", …)`
    for chunk in UI.split("class=\"").skip(1) {
        let Some(list) = chunk.split('"').next() else {
            continue;
        };
        classes.extend(
            list.split_whitespace()
                .filter(|name| name.starts_with("nr-"))
                .map(str::to_owned),
        );
    }
    // `class:nr-foo=…`
    for chunk in UI.split("class:").skip(1) {
        if let Some(name) = chunk.split('=').next()
            && name.starts_with("nr-")
        {
            classes.push(name.to_owned());
        }
    }
    classes.sort_unstable();
    classes.dedup();
    classes
}

/// Whether a stylesheet has a rule for exactly this class.
///
/// Matched on an identifier boundary, not as a substring: `.nr-migrate-mark`
/// must not be satisfied by a rule for `.nr-migrate-marker`, which is precisely
/// the rename this check exists to catch.
fn defines_class(css: &str, name: &str) -> bool {
    let needle = format!(".{name}");
    css.match_indices(&needle).any(|(index, _)| {
        css.get(index + needle.len()..)
            .and_then(|tail| tail.chars().next())
            .is_none_or(|next| !next.is_alphanumeric() && next != '-' && next != '_')
    })
}

#[test]
fn the_class_check_respects_identifier_boundaries() {
    // Guards the guard: without this, every class assertion below is a substring
    // match and a renamed class would pass unnoticed.
    assert!(defines_class(".nr-a { color: red }", "nr-a"));
    assert!(defines_class(".nr-a,.nr-b{}", "nr-a"));
    assert!(defines_class(".nr-a:hover{}", "nr-a"));
    assert!(defines_class(".x .nr-a{}", "nr-a"));
    assert!(!defines_class(".nr-ab { color: red }", "nr-a"));
    assert!(!defines_class(".nr-a-b{}", "nr-a"));
    assert!(!defines_class(".nr-a_b{}", "nr-a"));
    assert!(!defines_class("nothing here", "nr-a"));
}

#[test]
fn every_panel_specific_class_is_defined_in_the_stylesheet() {
    // `ui/migrate.rs` inlines this exact file with `include_str!`, so a class
    // renamed on one side and not the other loses its styling silently. This is
    // the check that makes the coupling visible.
    const CSS: &str =
        include_str!("../../../services/dashboard-actix/static/assets/dashboard/migrate.css");

    let missing: Vec<String> = classes_used_by_the_panel()
        .into_iter()
        .filter(|name| name.starts_with("nr-migrate-"))
        .filter(|name| !defines_class(CSS, name))
        .collect();
    assert!(
        missing.is_empty(),
        "these panel classes have no rule in migrate.css: {missing:?}"
    );
    assert!(
        classes_used_by_the_panel()
            .iter()
            .any(|name| name.starts_with("nr-migrate-")),
        "the extraction itself must find classes, or this test proves nothing"
    );
}

#[test]
fn motion_is_composed_from_the_shared_vocabulary() {
    const CSS: &str =
        include_str!("../../../services/dashboard-actix/static/assets/dashboard/migrate.css");
    const MOTION: &str =
        include_str!("../../../services/dashboard-actix/static/assets/dashboard/motion.css");

    // No panel-local animation: motion.css owns timings and the reduced-motion
    // suppression, and a private keyframe would escape both.
    assert!(
        !CSS.contains("@keyframes"),
        "migrate.css must not define its own keyframes"
    );
    assert!(
        !CSS.contains("animation:"),
        "migrate.css must not declare animations of its own"
    );

    // The motion classes the panel does use all come from that file.
    let motion_classes: Vec<String> = classes_used_by_the_panel()
        .into_iter()
        .filter(|name| {
            name.starts_with("nr-anim-")
                || name.starts_with("nr-skeleton")
                || matches!(
                    name.as_str(),
                    "nr-stagger" | "nr-spinner" | "nr-progress-indeterminate" | "nr-tick"
                )
        })
        .collect();
    assert!(
        !motion_classes.is_empty(),
        "the panel is expected to use the motion vocabulary"
    );
    for name in &motion_classes {
        assert!(
            defines_class(MOTION, name),
            "{name} is not defined in motion.css"
        );
    }

    // The states the brief calls for are all present.
    for required in [
        "nr-skeleton",
        "nr-stagger",
        "nr-progress-indeterminate",
        "nr-anim-rise",
    ] {
        assert!(
            motion_classes.iter().any(|name| name == required),
            "the panel must use {required}"
        );
    }
}

#[test]
fn the_browser_only_request_stays_behind_a_cfg_gate() {
    // The native target exists so this logic is testable; it must not try to
    // fetch there, and it must report the absence rather than fake a result.
    const DATA: &str = include_str!("../src/dashboard/migrate.rs");
    assert_eq!(
        DATA.matches(r#"#[cfg(target_arch = "wasm32")]"#).count(),
        1,
        "exactly one wasm-only arm: the fetch"
    );
    assert_eq!(
        DATA.matches(r#"#[cfg(not(target_arch = "wasm32"))]"#)
            .count(),
        1,
        "and exactly one native arm to match it"
    );
    assert!(
        DATA.contains("web_sys") || DATA.contains("web-sys"),
        "the wasm arm is the only place web_sys appears"
    );

    // Proof rather than inspection: on this (native) target the request fails
    // with Environment instead of returning a fabricated report.
    let result = futures_lite_block_on(nullrouter_dashboard_wasm::dashboard::migrate::run_migrate(
        String::new(),
        true,
    ));
    assert_eq!(result, Err(ApiError::Environment));
}

/// Drive a future to completion without pulling in an async runtime.
///
/// The futures here are either immediately ready (native) or never constructed
/// (wasm), so a single poll is enough.
fn futures_lite_block_on<F: std::future::Future>(future: F) -> F::Output {
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

    const VTABLE: RawWakerVTable = RawWakerVTable::new(|_| RAW, |_| {}, |_| {}, |_| {});
    const RAW: RawWaker = RawWaker::new(std::ptr::null(), &VTABLE);
    // SAFETY: the vtable's clone returns the same no-op waker and its wake
    // functions do nothing, so no invalid state can be observed.
    let waker = unsafe { Waker::from_raw(RAW) };
    let mut context = Context::from_waker(&waker);
    let mut future = Box::pin(future);
    match future.as_mut().poll(&mut context) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("the native arm must resolve immediately"),
    }
}

#[test]
fn the_panel_has_no_branch_that_invents_a_count() {
    // The guarantee is structural: `ImportReport` has no `Default`, so the only
    // way a count exists is that the server sent it. If a `Default` were added,
    // `ImportReport::default()` would compile and a zeroed table could reach the
    // screen looking like real data.
    const DATA: &str = include_str!("../src/dashboard/migrate.rs");
    assert!(
        !DATA.contains("Default, Deserialize") && !DATA.contains("Deserialize, Default"),
        "ImportReport must not derive Default"
    );

    const UI: &str = include_str!("../src/ui/migrate.rs");
    // The loading state renders skeletons, and failures render notices; neither
    // path may reach a table.
    let preview = UI
        .split("fn PreviewCard")
        .nth(1)
        .and_then(|tail| tail.split("fn ImportResultCard").next())
        .unwrap_or_default();
    assert!(preview.contains("Hydrate::Loading => view! { <SkeletonRows /> }"));
    assert!(preview.contains("Hydrate::Failed(error) => view! { <FailureNotice error /> }"));
    // Exactly one place builds the table, and it needs a report to do it.
    assert!(preview.contains("Hydrate::Ready(Outcome::Completed { report, .. })"));
    assert_eq!(
        UI.matches("<ReportBody report").count(),
        2,
        "the table is built only from a server report, in the preview and the import result"
    );
}
