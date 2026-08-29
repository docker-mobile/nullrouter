//! Login panel derivations.
//!
//! This logic used to be 162 lines of inline JavaScript in the actix host's login
//! shell, where nothing type-checked it and only a browser could exercise it. Two
//! of these functions are security decisions over untrusted input — a redirect
//! sanitiser and an auth-skip check — which is the reason they are worth testing
//! directly rather than through the rendered page.

use nullrouter_dashboard_wasm::dashboard::login_live::{
    AuthStatus, DEFAULT_TARGET, Mode, Submitted, button_disabled, button_label, dashboard_target,
    login_error, parse_status, retry_after_seconds, settle_submit, skips_login, submit_body,
};

const ORIGIN: &str = "https://router.example";

#[test]
fn a_hostile_next_value_never_leaves_this_origin() {
    // Given: `?next=` is attacker-controllable — it arrives in a link. An open
    // redirect off the login page is the classic phishing primitive: the victim
    // sees the real router's domain, signs in, and lands somewhere else.
    for hostile in [
        "https://evil.example/dashboard",
        "http://evil.example/dashboard",
        // Scheme-relative: resolves to another host while looking like a path.
        "//evil.example/dashboard",
        "//evil.example",
        "javascript:alert(1)",
        "data:text/html,<script>alert(1)</script>",
        // A different origin that merely starts with the real one.
        "https://router.example.evil.test/dashboard",
        // Backslashes are path separators to some parsers and not others.
        "/dashboard\\@evil.example",
        "\\\\evil.example/dashboard",
    ] {
        assert_eq!(
            dashboard_target(Some(hostile), ORIGIN),
            DEFAULT_TARGET,
            "{hostile} must not be honoured"
        );
    }
}

#[test]
fn a_next_value_outside_the_dashboard_is_refused() {
    // Given: even same-origin, the target must be the dashboard. Anything else is
    // a navigation the login screen has no business performing.
    for outside in [
        "/",
        "/login",
        "/api/keys",
        "/internal/v1/credentials/select",
        // A prefix match alone would wrongly accept this.
        "/dashboardsomething",
        "/dashboard-other",
    ] {
        assert_eq!(
            dashboard_target(Some(outside), ORIGIN),
            DEFAULT_TARGET,
            "{outside} is not inside the dashboard"
        );
    }
}

#[test]
fn a_legitimate_dashboard_target_is_preserved_with_its_query() {
    // Given: the whole point of `?next=` is returning the user where they were.
    assert_eq!(dashboard_target(Some("/dashboard"), ORIGIN), "/dashboard");
    assert_eq!(
        dashboard_target(Some("/dashboard/endpoint"), ORIGIN),
        "/dashboard/endpoint"
    );
    assert_eq!(
        dashboard_target(Some("/dashboard/usage?range=7d#top"), ORIGIN),
        "/dashboard/usage?range=7d#top",
        "query and fragment must survive"
    );
    // An absolute URL naming this exact origin is acceptable, but the result is
    // still a path so it cannot carry an origin onward.
    assert_eq!(
        dashboard_target(Some("https://router.example/dashboard/keys"), ORIGIN),
        "/dashboard/keys"
    );
}

#[test]
fn an_absent_or_blank_next_uses_the_default() {
    assert_eq!(dashboard_target(None, ORIGIN), DEFAULT_TARGET);
    assert_eq!(dashboard_target(Some(""), ORIGIN), DEFAULT_TARGET);
    assert_eq!(dashboard_target(Some("   "), ORIGIN), DEFAULT_TARGET);
}

#[test]
fn only_an_existing_session_skips_the_login_screen() {
    // Given: the inline version checked `authenticated === true` alone, and that
    // is load-bearing. A `requireLogin: false` in the body must not skip auth:
    // login is unconditional here, so such a body is stale or spoofed, and
    // honouring it would be an auth bypass driven by a JSON field.
    let authenticated = parse_status(r#"{"authenticated":true}"#).expect("parses");
    assert!(skips_login(&authenticated));

    for body in [
        r#"{"authenticated":false}"#,
        r#"{"authenticated":false,"requireLogin":false}"#,
        r#"{"requireLogin":false}"#,
        r#"{"authenticated":"true"}"#,
        "{}",
    ] {
        let status = parse_status(body).unwrap_or_default();
        assert!(
            !skips_login(&status),
            "{body} must not skip the login screen"
        );
    }
}

#[test]
fn an_unreadable_status_body_still_shows_the_password_form() {
    // Given: this is the only screen the user can recover from, so a broken or
    // unexpected status body must degrade to "ask for the password".
    for body in ["", "not json", "[]", "null", "<!doctype html>"] {
        let status = parse_status(body).unwrap_or_default();
        assert!(!skips_login(&status), "{body:?}");
        assert!(!status.oidc_ready());
        assert!(!status.password_hidden(), "the form must stay reachable");
    }
}

#[test]
fn oidc_is_offered_only_when_configured_and_selected() {
    // Given: a configured provider the mode does not select would send the user
    // into a flow this router will not complete.
    let both = parse_status(r#"{"oidcConfigured":true,"authMode":"both"}"#).expect("parses");
    assert!(both.oidc_ready());
    assert!(!both.password_hidden(), "both keeps the password form");

    let oidc_only = parse_status(r#"{"oidcConfigured":true,"authMode":"oidc"}"#).expect("parses");
    assert!(oidc_only.oidc_ready());
    assert!(oidc_only.password_hidden());

    for body in [
        r#"{"oidcConfigured":false,"authMode":"oidc"}"#,
        r#"{"oidcConfigured":true,"authMode":"password"}"#,
        r#"{"oidcConfigured":true}"#,
    ] {
        let status = parse_status(body).expect("parses");
        assert!(!status.oidc_ready(), "{body}");
        assert!(
            !status.password_hidden(),
            "{body} must not hide the only usable form"
        );
    }
}

#[test]
fn a_blank_oidc_label_falls_back_rather_than_rendering_empty() {
    for body in [
        r#"{"oidcLoginLabel":""}"#,
        r#"{"oidcLoginLabel":"   "}"#,
        "{}",
    ] {
        let status = parse_status(body).expect("parses");
        assert_eq!(status.oidc_label(), "Sign in with OIDC", "{body}");
    }
    let named = parse_status(r#"{"oidcLoginLabel":"Corp SSO"}"#).expect("parses");
    assert_eq!(named.oidc_label(), "Corp SSO");
}

#[test]
fn a_remaining_attempt_count_is_bounded_before_it_is_shown() {
    // Given: the count comes from the server and is rendered into the page. An
    // absurd value is an odd claim rather than a useful one.
    assert_eq!(
        login_error(401, Some(2)),
        "Invalid password. 2 attempt(s) left before lockout."
    );
    assert_eq!(
        login_error(401, Some(0)),
        "Invalid password. 0 attempt(s) left before lockout."
    );
    for unbounded in [Some(-1), Some(101), Some(i64::MAX), None] {
        assert_eq!(
            login_error(401, unbounded),
            "Invalid password.",
            "{unbounded:?} should not be rendered"
        );
    }
}

#[test]
fn each_refusal_status_gets_its_own_message() {
    assert_eq!(
        login_error(429, None),
        "Too many failed attempts. Try again later."
    );
    assert_eq!(login_error(403, None), "Password login is unavailable.");
    assert_eq!(login_error(400, None), "Enter a valid password.");
    // An unmapped status must still say something actionable.
    assert_eq!(
        login_error(500, None),
        "Unable to sign in. Please try again."
    );
    assert_eq!(login_error(0, None), "Unable to sign in. Please try again.");
}

#[test]
fn a_retry_countdown_is_clamped_and_never_negative() {
    // Given: `Retry-After` is server-controlled. A hostile or broken value must
    // not park the button for a week, and must not underflow.
    assert_eq!(retry_after_seconds(Some("30"), None), 30);
    assert_eq!(retry_after_seconds(Some("0.2"), None), 1, "rounds up");
    assert_eq!(retry_after_seconds(Some("99999"), None), 3600, "clamped");
    assert_eq!(retry_after_seconds(Some("-5"), None), 0);
    assert_eq!(retry_after_seconds(Some("0"), None), 0);
    assert_eq!(retry_after_seconds(Some("not a number"), None), 0);
    assert_eq!(retry_after_seconds(Some("NaN"), None), 0);
    // Rust's parser accepts "inf"/"infinity" where JS's `Number()` gives NaN.
    // Both must end at 0: a non-finite countdown would park the button forever.
    assert_eq!(retry_after_seconds(Some("inf"), None), 0);
    assert_eq!(retry_after_seconds(Some("infinity"), None), 0);
    assert_eq!(retry_after_seconds(Some("-inf"), None), 0);
    assert_eq!(retry_after_seconds(None, Some(f64::INFINITY)), 0);
    assert_eq!(retry_after_seconds(None, Some(f64::NAN)), 0);
    assert_eq!(retry_after_seconds(None, None), 0);
    // The header wins over the body, as upstream reads it.
    assert_eq!(retry_after_seconds(Some("10"), Some(90.0)), 10);
    assert_eq!(retry_after_seconds(None, Some(45.0)), 45);
}

#[test]
fn a_submit_body_is_json_encoded_so_a_password_cannot_break_out() {
    // Given: a password may contain quotes and backslashes.
    let hostile = r#"pa"ss\word"#;
    let body = submit_body(Mode::SignIn, hostile, "");
    let reparsed: serde_json::Value =
        serde_json::from_str(&body).expect("a submit body must always be valid JSON");
    assert_eq!(
        reparsed.get("password").and_then(serde_json::Value::as_str),
        Some(hostile)
    );

    let change = submit_body(Mode::ChangePassword, "old", hostile);
    let reparsed: serde_json::Value = serde_json::from_str(&change).expect("valid JSON");
    assert_eq!(
        reparsed
            .get("newPassword")
            .and_then(serde_json::Value::as_str),
        Some(hostile)
    );
    assert_eq!(
        reparsed
            .get("currentPassword")
            .and_then(serde_json::Value::as_str),
        Some("old")
    );
    // Sign-in must not send a newPassword key at all.
    assert!(
        !body.contains("newPassword"),
        "sign-in body should carry only the password: {body}"
    );
}

#[test]
fn each_mode_targets_its_own_endpoint_and_method() {
    assert_eq!(Mode::SignIn.path(), "/api/auth/login");
    assert_eq!(Mode::SignIn.method(), "POST");
    assert_eq!(Mode::ChangePassword.path(), "/api/settings");
    assert_eq!(Mode::ChangePassword.method(), "PATCH");
    // Posting a change to the login endpoint would silently not change anything.
    assert_ne!(Mode::SignIn.path(), Mode::ChangePassword.path());
}

#[test]
fn a_required_password_change_wins_over_navigating() {
    // Given: a 2xx that asks for a change. Navigating to the dashboard would
    // strand the user on a password they were just told to replace.
    assert_eq!(
        settle_submit(true, 200, true, None, 0, "/dashboard"),
        Submitted::RequireChange
    );
    assert_eq!(
        settle_submit(true, 200, false, None, 0, "/dashboard/usage"),
        Submitted::Navigate(String::from("/dashboard/usage"))
    );
}

#[test]
fn a_refusal_carries_its_message_and_only_429_carries_a_countdown() {
    assert_eq!(
        settle_submit(false, 401, false, Some(3), 0, "/dashboard"),
        Submitted::Refused {
            message: String::from("Invalid password. 3 attempt(s) left before lockout."),
            retry_after: 0,
        }
    );
    assert_eq!(
        settle_submit(false, 429, false, None, 60, "/dashboard"),
        Submitted::Refused {
            message: String::from("Too many failed attempts. Try again later."),
            retry_after: 60,
        }
    );
    // A countdown on a non-lockout refusal would disable the button for no reason.
    assert_eq!(
        settle_submit(false, 401, false, None, 60, "/dashboard"),
        Submitted::Refused {
            message: String::from("Invalid password."),
            retry_after: 0,
        }
    );
}

#[test]
fn the_button_reports_what_it_is_doing() {
    assert_eq!(button_label(Mode::SignIn, false, 0), "Login");
    assert_eq!(button_label(Mode::SignIn, true, 0), "Logging in...");
    assert_eq!(button_label(Mode::ChangePassword, false, 0), "Set password");
    assert_eq!(button_label(Mode::ChangePassword, true, 0), "Saving...");
    // A countdown outranks both: the button cannot be pressed anyway.
    assert_eq!(button_label(Mode::SignIn, true, 12), "Wait 12s");

    assert!(!button_disabled(false, 0));
    assert!(button_disabled(true, 0), "no double submit");
    assert!(button_disabled(false, 5), "locked out");
}

#[test]
fn the_change_password_mode_says_why_it_is_asking() {
    // The copy is what tells the user this is not the screen they expected.
    assert_ne!(Mode::SignIn.copy(), Mode::ChangePassword.copy());
    assert!(Mode::ChangePassword.copy().contains("new password"));
    assert!(!AuthStatus::default().authenticated);
}
