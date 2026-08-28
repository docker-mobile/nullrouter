use nullrouter_dashboard_wasm::ui::console_log_visible_contract;

#[test]
fn console_log_visible_contract_contains_upstream_console_surface() {
    // Given: the concrete Leptos route exposes a visible contract for CSR-only tests.
    let contract = console_log_visible_contract();

    // When: tests assert route-level hooks and user-facing labels.
    // Then: the console-log route can be distinguished from the generic G003 placeholder.
    for expected in [
        "nr-console-log-panel",
        "nr-console-log-viewport",
        "nr-console-level-legend",
        "nr-console-endpoint-list",
        "Console Log",
        "Clear",
        "No console logs yet.",
        "Disconnected",
        "Connected",
        "No live capture",
        "/api/translator/console-logs",
        "/api/translator/console-logs/stream",
        "Newest 200 lines retained",
        "0 retained",
        "200 max",
        "LOG",
        "INFO",
        "WARN",
        "ERROR",
        "DEBUG",
        "nr-console-level-log",
        "nr-console-level-info",
        "nr-console-level-warn",
        "nr-console-level-error",
        "nr-console-level-debug",
    ] {
        assert!(
            contract.contains(&expected),
            "missing visible contract: {expected}"
        );
    }
}

#[test]
fn console_log_visible_contract_does_not_use_old_generic_placeholder_copy() {
    // Given: the prior G003 route rendered a generic dashboard placeholder.
    let contract = console_log_visible_contract();

    // When: the concrete route declares the strings it renders.
    // Then: the old placeholder copy is no longer the visible Console Log contract.
    for old_placeholder in [
        "Log stream not connected",
        "No console entries",
        "Not persisted",
    ] {
        assert!(
            !contract.contains(&old_placeholder),
            "old placeholder still visible: {old_placeholder}"
        );
    }
}

#[test]
fn console_log_contract_does_not_claim_the_stream_is_unwired() {
    // Given: the panel subscribes to the SSE feed and Clear performs a DELETE.
    let contract = console_log_visible_contract();

    // When/Then: the fixture-era copy must not come back. Rendering it while the
    // stream is actually wired would tell the user the feed is dead when it is
    // live — the inverse of the bug the wiring fixed.
    for stale in [
        "EventSource stream unwired in this WASM slice",
        "Clear endpoint unwired",
    ] {
        assert!(
            !contract.contains(&stale),
            "stale unwired copy still in the contract: {stale}"
        );
    }
}
