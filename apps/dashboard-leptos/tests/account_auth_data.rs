use nullrouter_dashboard_wasm::account::{
    AccountState, AccountStatusError, logout_response_succeeded,
};
use nullrouter_dashboard_wasm::ui::dashboard_account_actions;

#[test]
fn authenticated_account_menu_enables_only_logout() {
    // Given: the frozen G016 account command inventory.
    let actions = dashboard_account_actions();

    // When: G017 promotes the authenticated logout command.
    let enabled = actions
        .iter()
        .filter(|action| action.enabled)
        .map(|action| (action.label, action.icon))
        .collect::<Vec<_>>();

    // Then: unsupported profile-adjacent commands remain disabled.
    assert_eq!(enabled, vec![("Logout", "logout")]);
}

#[test]
fn authenticated_status_enables_logout_and_exposes_identity() {
    // Given: a fresh account state and a minimal authenticated status response.
    let mut state = AccountState::checking();

    // When: status hydration succeeds.
    let result = state.apply_status_json(
        r#"{"authenticated":true,"displayName":"Arian","loginMethod":"Password"}"#,
    );

    // Then: identity is visible and Logout becomes actionable.
    assert!(result.is_ok());
    assert_eq!(state.display_name(), "Arian");
    assert_eq!(state.login_method(), "Password");
    assert!(state.can_logout());
}

#[test]
fn authenticated_status_bounds_identity_fields() {
    // Given: authenticated identity fields longer than the compact header can display.
    let mut state = AccountState::checking();
    let json = format!(
        r#"{{"authenticated":true,"displayName":"{}","loginMethod":"{}"}}"#,
        "n".repeat(40),
        "m".repeat(24),
    );

    // When: the typed status boundary accepts the response.
    let result = state.apply_status_json(&json);

    // Then: both dynamic fields are bounded before entering the component.
    assert!(result.is_ok());
    assert_eq!(state.display_name().chars().count(), 32);
    assert_eq!(state.login_method().chars().count(), 16);
}

#[test]
fn malformed_status_is_rejected_without_enabling_logout() {
    // Given: a fresh account state and malformed response JSON.
    let mut state = AccountState::checking();

    // When: the status boundary parses the response.
    let result = state.apply_status_json("{");

    // Then: parsing fails locally and Logout remains unavailable.
    assert_eq!(result, Err(AccountStatusError::InvalidPayload));
    assert!(!state.can_logout());
}

#[test]
fn logout_pending_disables_repeat_submission() {
    // Given: an authenticated account.
    let mut state = authenticated_state();

    // When: Logout starts and is activated again before completion.
    let first = state.begin_logout();
    let repeated = state.begin_logout();

    // Then: only the first activation is accepted and pending remains named.
    assert!(first);
    assert!(!repeated);
    assert!(!state.can_logout());
    assert_eq!(state.logout_status(), "Signing out");
    assert_eq!(state.logout_announcement(), "Signing out");
}

#[test]
fn logout_error_is_accessible_and_retryable() {
    // Given: an authenticated account with a pending logout.
    let mut state = authenticated_state();
    assert!(state.begin_logout());

    // When: the request fails.
    state.logout_failed();

    // Then: bounded local copy is exposed and retry is enabled without losing identity.
    assert_eq!(state.logout_status(), "Logout failed");
    assert_eq!(state.logout_announcement(), "Logout failed");
    assert!(state.can_logout());
    assert_eq!(state.display_name(), "Arian");
}

#[test]
fn logout_response_requires_explicit_success_true() {
    // Given: successful, denied, error-shaped, and malformed logout payloads.
    // When: the typed logout boundary evaluates each response.
    // Then: only an explicit successful response permits navigation.
    assert!(logout_response_succeeded(r#"{"success":true}"#));
    assert!(!logout_response_succeeded(r#"{"success":false}"#));
    assert!(!logout_response_succeeded(r#"{"error":"denied"}"#));
    assert!(!logout_response_succeeded("{"));
}

#[test]
fn unavailable_status_never_enables_logout() {
    // Given: initial non-blocking shell state.
    let mut state = AccountState::checking();

    // When: status hydration fails.
    state.status_failed();

    // Then: the shell keeps a bounded local status and no privileged action is enabled.
    assert_eq!(state.account_status(), "Session unavailable");
    assert!(!state.can_logout());
}

fn authenticated_state() -> AccountState {
    let mut state = AccountState::checking();
    let result = state.apply_status_json(
        r#"{"authenticated":true,"displayName":"Arian","loginMethod":"Password"}"#,
    );
    assert!(result.is_ok());
    state
}
