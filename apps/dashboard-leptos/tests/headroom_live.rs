//! Boundary tests for the live Headroom panel state.
//!
//! Headroom is an external Python subsystem: `nullrouter-api` detects it for
//! real and refuses to mutate it. Every test here defends one property — the
//! panel may only claim what the router reported, and it must never render a
//! refused install or restart as a completed one. A user who believes
//! compression is active while it is not pays for full-size requests.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test assertions read clearer with direct unwrap than with error plumbing"
)]

use nullrouter_dashboard_wasm::api::ApiError;
use nullrouter_dashboard_wasm::dashboard::headroom_live::{
    ActionOutcome, ActionSupport, EXTRAS_LOG_PATH, EXTRAS_PATH, PythonStatus, RESTART_PATH,
    install_body, parse_log, parse_report, settle_action,
};

/// A body shaped like `nullrouter-api`'s own output on a machine that has
/// Python and `headroom-ai`, with one extra installed and one not.
const REALISTIC_REPORT: &str = r#"{
  "available": ["code", "ml"],
  "installed": true,
  "version": "0.9.3",
  "extras": { "code": true, "ml": false },
  "python": "/usr/bin/python3",
  "pythonVersion": "3.12",
  "pythonMinVersion": "3.10",
  "installSupported": false,
  "installMessage": "Installing headroom extras is not supported by nullrouter-api. Nothing was installed.",
  "restartSupported": false,
  "restartMessage": "Restarting the headroom proxy is not supported by nullrouter-api. Nothing was restarted."
}"#;

/// The same endpoint on a machine with no suitable interpreter.
const NO_PYTHON_REPORT: &str = r#"{
  "available": ["code", "ml"],
  "installed": false,
  "version": null,
  "extras": { "code": false, "ml": false },
  "python": null,
  "pythonVersion": null,
  "pythonMinVersion": "3.10",
  "installSupported": false,
  "installMessage": "Installing headroom extras is not supported by nullrouter-api. Nothing was installed.",
  "restartSupported": false,
  "restartMessage": "Restarting the headroom proxy is not supported by nullrouter-api. Nothing was restarted."
}"#;

#[test]
fn paths_match_the_endpoints_the_router_serves() {
    assert_eq!(EXTRAS_PATH, "/api/headroom/extras");
    assert_eq!(EXTRAS_LOG_PATH, "/api/headroom/extras?log=1");
    assert_eq!(RESTART_PATH, "/api/headroom/restart");
}

#[test]
fn parses_a_realistic_report_into_rows_that_match_the_host() {
    // Given: the router reports one extra installed and one not.
    let report = parse_report(REALISTIC_REPORT).expect("realistic report should parse");

    // Then: the rows follow the router's own order and carry its states.
    let rows = report.rows();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].name, "code");
    assert_eq!(rows[0].installed, Some(true));
    assert_eq!(rows[0].label(), "Code-aware compression");
    assert_eq!(rows[1].name, "ml");
    assert_eq!(rows[1].installed, Some(false));

    // The count is what the panel headlines, so it must match the rows.
    assert_eq!(report.installed_count(), (1, 2));
    assert_eq!(report.version_label(), "0.9.3");
}

#[test]
fn installed_state_is_available_as_text_not_only_as_colour() {
    // Given: the three states a row can be in.
    let report = parse_report(REALISTIC_REPORT).unwrap();
    let rows = report.rows();

    // Then: each renders a word, so the state survives greyscale and a screen
    // reader. An unreported state says exactly that, and never reads as "off".
    assert_eq!(rows[0].installed_label(), "Installed");
    assert_eq!(rows[1].installed_label(), "Not installed");

    let unreported =
        parse_report(r#"{"available":["code"],"installed":true,"version":"1.0.0","extras":{}}"#)
            .unwrap();
    let unreported_rows = unreported.rows();
    assert_eq!(unreported_rows[0].installed, None);
    assert_eq!(unreported_rows[0].installed_label(), "State not reported");
}

#[test]
fn detects_python_and_names_the_interpreter_that_answered() {
    let report = parse_report(REALISTIC_REPORT).unwrap();

    match report.python() {
        PythonStatus::Detected { path, version } => {
            assert_eq!(path, "/usr/bin/python3");
            assert_eq!(version.as_deref(), Some("3.12"));
        }
        PythonStatus::Missing { .. } => panic!("a python path was reported"),
    }
    assert!(report.python().label().contains("3.12"));
    // The detail line points at the interpreter, so the claim is checkable.
    assert_eq!(report.python().detail(), "/usr/bin/python3");
}

#[test]
fn a_report_with_no_python_says_so_and_quotes_the_requirement() {
    // Given: no interpreter on this machine satisfies the minimum.
    let report = parse_report(NO_PYTHON_REPORT).expect("no-python report should parse");

    // Then: the panel states the absence and the version needed to fix it —
    // never an empty banner the user cannot act on.
    let status = report.python();
    assert!(!status.is_detected());
    assert_eq!(
        status,
        PythonStatus::Missing {
            minimum: String::from("3.10")
        }
    );
    assert!(status.label().contains("3.10"));
    assert!(status.detail().contains("3.10"));

    // And nothing claims an install: no version, no extras on.
    assert_eq!(report.version_label(), "not installed");
    assert_eq!(report.installed_count(), (0, 2));
}

#[test]
fn a_missing_minimum_version_falls_back_rather_than_rendering_blank() {
    // Given: an older router that does not report the minimum.
    let report =
        parse_report(r#"{"available":[],"installed":false,"extras":{},"python":null}"#).unwrap();

    // Then: the requirement is still quoted, because "install Python " is not
    // an instruction anyone can follow.
    assert_eq!(report.min_python(), "3.10");
    assert!(report.python().detail().contains("3.10"));
}

#[test]
fn an_extra_the_router_reported_but_did_not_list_is_still_shown() {
    // Given: a build that tracks an extra this dashboard does not know about.
    let report = parse_report(
        r#"{"available":["code"],"installed":true,"version":"1.0.0","extras":{"code":true,"voice":true}}"#,
    )
    .unwrap();

    // Then: it is appended rather than hidden — the router said it is on, so the
    // panel must not silently drop it.
    let rows = report.rows();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].name, "code");
    assert_eq!(rows[1].name, "voice");
    // With no description of its own, the row says so instead of borrowing one.
    assert!(rows[1].description().contains("no description"));
    assert_eq!(report.installed_count(), (2, 2));
}

#[test]
fn a_report_missing_its_identity_fields_is_a_failure_not_an_empty_panel() {
    // `installed` and `available` are the identity of the report. Defaulting
    // either would make the panel assert something the router never said.
    for body in [
        r#"{"available":["code"],"extras":{}}"#,
        r#"{"installed":true,"extras":{}}"#,
        r#"{"available":["code"],"installed":true}"#,
    ] {
        assert!(parse_report(body).is_none(), "{body} should not parse");
    }
}

#[test]
fn malformed_and_empty_bodies_never_parse_into_a_report() {
    for body in [
        "",
        "   ",
        "{",
        "null",
        "[]",
        "not json at all",
        r#""a string""#,
    ] {
        assert!(parse_report(body).is_none(), "{body:?} should not parse");
        assert!(
            parse_log(body).is_none(),
            "{body:?} should not parse as a log"
        );
    }
}

#[test]
fn parses_the_log_tail_into_lines() {
    // Given: the router returns a few lines of pip output.
    let log = parse_log(
        r#"{"log":"Collecting headroom-ai\n\nInstalling collected packages: torch\n","logPath":"/home/dev/.9router/headroom/install.log"}"#,
    )
    .expect("log body should parse");

    // Then: blank lines are dropped and the order is preserved.
    assert_eq!(
        log.lines(),
        vec![
            "Collecting headroom-ai",
            "Installing collected packages: torch"
        ]
    );
    assert!(!log.is_empty());
    assert_eq!(
        log.log_path.as_deref(),
        Some("/home/dev/.9router/headroom/install.log")
    );
}

#[test]
fn an_empty_log_explains_itself_differently_when_no_file_exists() {
    // Given: a log that exists but holds nothing, and no log at all.
    let existing = parse_log(r#"{"log":"","logPath":"/data/headroom/install.log"}"#).unwrap();
    let absent = parse_log(r#"{"log":"","logPath":null}"#).unwrap();

    // Then: both are empty, but only one can point at a file — the user needs to
    // know which, because it decides where to look next.
    assert!(existing.is_empty());
    assert!(absent.is_empty());
    assert!(
        existing
            .placeholder()
            .contains("/data/headroom/install.log")
    );
    assert!(absent.placeholder().contains("never writes one"));
    assert_ne!(existing.placeholder(), absent.placeholder());
}

#[test]
fn a_log_body_without_its_log_field_is_a_failure() {
    // Defaulting `log` to "" would render as "the log is empty", which the
    // router did not say.
    assert!(parse_log(r#"{"logPath":"/x"}"#).is_none());
}

#[test]
fn support_flags_are_read_from_the_report_before_any_control_is_drawn() {
    // Given: this build refuses both mutations and says why.
    let report = parse_report(REALISTIC_REPORT).unwrap();

    // Then: the panel knows to render an explanation instead of a button,
    // without having to POST first and discover the refusal.
    let install = report.install_support();
    assert!(!install.is_supported());
    assert_eq!(
        install.reason(),
        Some(
            "Installing headroom extras is not supported by nullrouter-api. Nothing was installed."
        )
    );
    assert!(!report.restart_support().is_supported());
    assert!(
        report
            .restart_support()
            .reason()
            .unwrap()
            .contains("restarted")
    );
}

#[test]
fn an_absent_support_flag_is_read_as_unsupported() {
    // Given: a report with no capability flags at all.
    let report = parse_report(
        r#"{"available":["ml"],"installed":true,"version":"1.0.0","extras":{"ml":false}}"#,
    )
    .unwrap();

    // Then: neither action is offered. A missing flag must never enable a
    // control that mutates a host, and the fallback still explains itself.
    assert!(!report.install_support().is_supported());
    assert!(!report.restart_support().is_supported());
    assert!(!report.install_support().reason().unwrap().is_empty());
    assert_eq!(
        report.install_support(),
        ActionSupport::Unsupported {
            reason: String::from(
                "This build does not install headroom extras. Nothing was installed."
            )
        }
    );
}

#[test]
fn a_refused_install_is_a_refusal_and_never_a_success() {
    // Given: the 501 body `nullrouter-api` returns for an install request.
    let body = r#"{"success":false,"unsupported":true,"code":"UNSUPPORTED","error":"Installing headroom extras is not supported by nullrouter-api. Nothing was installed.","requested":["ml"],"ignored":["image"],"available":["code","ml"],"spec":"headroom-ai[proxy,ml]"}"#;

    // When: the panel settles it.
    let outcome = settle_action(501, body);

    // Then: it is a refusal carrying the router's reason and the requirement to
    // run by hand — and it did not change the host. That last assertion is the
    // one that keeps a user from believing compression is now active.
    match outcome {
        ActionOutcome::Refused(refusal) => {
            assert_eq!(refusal.code.as_deref(), Some("UNSUPPORTED"));
            assert_eq!(refusal.spec.as_deref(), Some("headroom-ai[proxy,ml]"));
            assert_eq!(refusal.ignored, vec![String::from("image")]);
            assert!(refusal.message.contains("Nothing was installed"));
        }
        other => panic!("a 501 install must settle as a refusal, got {other:?}"),
    }
    assert!(!settle_action(501, body).changed_the_host());
}

#[test]
fn a_refused_restart_reports_the_external_proxy_case_distinctly() {
    // Given: upstream's 400 for a proxy on another host.
    let outcome = settle_action(
        400,
        r#"{"success":false,"error":"External Headroom proxies must be started outside 9Router","code":"EXTERNAL_PROXY","url":"http://headroom.internal:8787"}"#,
    );

    // Then: it is a refusal with the code, so the panel can say the proxy is not
    // this machine's to restart rather than showing a generic error.
    match outcome {
        ActionOutcome::Refused(refusal) => {
            assert_eq!(refusal.code.as_deref(), Some("EXTERNAL_PROXY"));
            assert!(refusal.message.contains("outside 9Router"));
            assert!(refusal.spec.is_none());
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn success_requires_both_a_2xx_and_an_explicit_success_flag() {
    // A 2xx that says `success:false` is still a refusal: the status alone must
    // not be enough to claim the host changed.
    let outcome = settle_action(200, r#"{"success":false,"error":"pip exited with code 1"}"#);
    assert!(!outcome.changed_the_host());
    assert!(matches!(outcome, ActionOutcome::Refused(_)));

    // Only both together is a completion.
    let completed = settle_action(200, r#"{"success":true,"message":"installed"}"#);
    assert!(completed.changed_the_host());
    assert_eq!(completed.message(), "installed");
}

#[test]
fn an_unreadable_or_silent_body_is_a_failure_not_an_outcome() {
    // Given: bodies that say nothing about what happened.
    for (status, body) in [(501, "not json"), (200, "{"), (200, "{}"), (500, "{}")] {
        let outcome = settle_action(status, body);
        assert!(
            matches!(outcome, ActionOutcome::Failed(_)),
            "({status}, {body:?}) should be a failure, got {outcome:?}"
        );
        // Above all: it must not be read as having changed anything.
        assert!(!outcome.changed_the_host());
    }

    // A failure carries the status so the panel can explain it.
    assert_eq!(
        settle_action(503, "{}"),
        ActionOutcome::Failed(ApiError::Status(503))
    );
    assert_eq!(
        settle_action(200, "{}"),
        ActionOutcome::Failed(ApiError::Body)
    );
}

#[test]
fn the_refusal_carries_a_command_the_user_can_actually_run() {
    // Given: the install action is refused, so the panel's only way forward is
    // telling the user what to run.
    let report = parse_report(REALISTIC_REPORT).unwrap();

    // Then: the command targets the interpreter the router reported — the same
    // environment the extras were read from — and asks only for what is missing.
    let command = report.manual_install_command();
    assert_eq!(
        command,
        "/usr/bin/python3 -m pip install --upgrade 'headroom-ai[proxy,ml]'"
    );
    // Quoted, because `[` and `]` are glob characters in a shell.
    assert!(command.contains("'headroom-ai[proxy,ml]'"));
    // `code` is already installed, so it is not re-requested.
    assert!(!command.contains("code"));
}

#[test]
fn the_command_still_names_an_interpreter_when_none_was_detected() {
    // Given: no Python on this machine.
    let report = parse_report(NO_PYTHON_REPORT).unwrap();

    // Then: the command is still complete and asks for both extras. `python3` is
    // a placeholder the user will have after installing Python, not a claim that
    // one was found.
    let command = report.manual_install_command();
    assert_eq!(
        command,
        "python3 -m pip install --upgrade 'headroom-ai[proxy,code,ml]'"
    );
}

#[test]
fn the_install_body_sends_exactly_the_requested_extras() {
    assert_eq!(install_body(&[]), r#"{"extras":[]}"#);
    assert_eq!(
        install_body(&[String::from("code"), String::from("ml")]),
        r#"{"extras":["code","ml"]}"#
    );
}
