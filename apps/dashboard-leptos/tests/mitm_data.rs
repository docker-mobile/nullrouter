//! The dashboard's MITM state without a reference checkout.
//!
//! This is intentionally separate from `mitm_upstream_parity.rs`. The latter compares literals against the
//! read-only `inspire/` checkout and is feature-gated because that checkout is gitignored; this one proves
//! what a normal clone actually compiles and serves.

use nullrouter_dashboard_wasm::dashboard::mitm_dashboard_state;

#[test]
fn mitm_server_state_is_stopped_and_disabled_without_a_reference_checkout() {
    let state = mitm_dashboard_state();
    let checks = state
        .server
        .checks
        .iter()
        .map(|check| (check.label, check.ok))
        .collect::<Vec<_>>();

    assert_eq!(state.route_path, "/dashboard/mitm");
    assert_eq!(state.title, "MITM Proxy");
    assert!(!state.live_control_wired);
    assert_eq!(state.server.title, "MITM Server");
    assert_eq!(state.server.status_label, "Stopped");
    assert!(!state.server.running);
    assert_eq!(
        checks,
        [("Cert", false), ("Trusted", false), ("Server", false)]
    );
    assert_eq!(state.server.base_url.label, "9Router Base URL");
    assert!(!state.server.base_url.enabled);
    assert_eq!(state.server.action.label, "Start Server");
    assert!(!state.server.action.enabled);
    assert!(serde_json::to_string(&state).is_ok());
}

#[test]
fn mitm_tools_keep_the_declared_non_operational_boundary() {
    let state = mitm_dashboard_state();

    assert_eq!(
        state.tools.iter().map(|tool| tool.id).collect::<Vec<_>>(),
        ["antigravity", "copilot", "kiro"]
    );
    assert_eq!(
        state
            .tools
            .iter()
            .map(|tool| tool.models.len())
            .collect::<Vec<_>>(),
        [9, 5, 7]
    );
    assert_eq!(
        state
            .tools
            .iter()
            .map(|tool| tool.hosts.len())
            .collect::<Vec<_>>(),
        [2, 1, 3]
    );
    assert!(state.tools.iter().all(|tool| {
        !tool.server_running
            && !tool.dns_active
            && tool.server_status_label == "Server off"
            && tool.dns_status_label == "DNS off"
            && !tool.mapping_inputs_enabled
            && !tool.model_select_enabled
            && !tool.dns_action.enabled
            && tool.dns_action.label == "Start DNS"
    }));
}
