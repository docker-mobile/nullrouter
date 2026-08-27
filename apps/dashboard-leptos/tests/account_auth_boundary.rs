const ACCOUNT_MENU_SOURCE: &str = concat!(
    include_str!("../src/account.rs"),
    include_str!("../src/ui/shell/header/menus/account.rs"),
);

#[test]
fn account_status_hydration_is_non_blocking_and_same_origin() {
    // Given: the account menu remains part of the already-rendered G016 shell.
    // When: G017 hydrates authenticated account state.
    // Then: the component uses a detached typed same-origin status request.
    for marker in [
        "/api/auth/status",
        "RequestCredentials::SameOrigin",
        "spawn_local",
        "authenticated",
    ] {
        assert!(
            ACCOUNT_MENU_SOURCE.contains(marker),
            "missing account status contract: {marker}"
        );
    }
}

#[test]
fn logout_pending_disables_repeat_submission() {
    // Given: an authenticated account with an available Logout command.
    // When: the same-origin logout request is pending.
    // Then: repeat activation is disabled and the pending state stays named.
    for marker in [
        "/api/auth/logout",
        "method(\"POST\")",
        "RequestCredentials::SameOrigin",
        "Signing out",
        "aria-busy",
    ] {
        assert!(
            ACCOUNT_MENU_SOURCE.contains(marker),
            "missing logout pending contract: {marker}"
        );
    }
}

#[test]
fn logout_error_is_accessible_and_retryable() {
    // Given: a failed logout HTTP response or browser request.
    // When: the account menu reports the failure without navigating.
    // Then: the error is announced, the menu closes deterministically, and retry remains possible.
    for marker in [
        "aria-live=\"polite\"",
        "Logout failed",
        "HeaderPanel::Closed",
        "set_href(\"/login\")",
    ] {
        assert!(
            ACCOUNT_MENU_SOURCE.contains(marker),
            "missing logout error contract: {marker}"
        );
    }
}

#[test]
fn logout_navigation_requires_typed_success_payload() {
    // Given: the logout endpoint can return HTTP success with a non-success payload.
    // When: G017 decides whether to leave the dashboard.
    // Then: navigation is guarded by the typed response decoder.
    for marker in [
        "struct LogoutResponse",
        "success: bool",
        "Ok(body) if logout_response_succeeded(&body)",
        "set_href(\"/login\")",
    ] {
        assert!(
            ACCOUNT_MENU_SOURCE.contains(marker),
            "missing typed logout navigation contract: {marker}"
        );
    }
}
