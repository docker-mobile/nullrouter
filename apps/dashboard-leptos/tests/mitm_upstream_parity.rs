use nullrouter_dashboard_wasm::dashboard::mitm_dashboard_state;

const UPSTREAM_PAGE: &str =
    include_str!("../../../inspire/src/app/(dashboard)/dashboard/mitm/MitmPageClient.js");
const UPSTREAM_SERVER: &str = include_str!(
    "../../../inspire/src/app/(dashboard)/dashboard/cli-tools/components/MitmServerCard.js"
);
const UPSTREAM_TOOLS: &str = include_str!("../../../inspire/src/shared/constants/cliTools.js");
const UPSTREAM_HOSTS: &str = include_str!("../../../inspire/src/shared/constants/mitmToolHosts.js");

#[test]
fn mitm_server_state_is_stopped_disabled_and_truthful_when_unwired() {
    // Given: the Leptos dashboard has no live MITM host bridge.
    let state = mitm_dashboard_state();

    // When: the dedicated MITM route consumes its deterministic state.
    let checks = state
        .server
        .checks
        .iter()
        .map(|check| (check.label, check.ok))
        .collect::<Vec<_>>();

    // Then: the upstream server surface is visible without claiming live control.
    assert_eq!(state.route_path, "/dashboard/mitm");
    assert_eq!(state.title, "MITM Proxy");
    assert_eq!(
        state.risk_warning,
        "⚠️ MITM intercepts HTTPS traffic of IDE tools (Antigravity, GitHub Copilot, Kiro) via local CA to redirect requests to your providers. May violate ToS → account ban. Use at your own risk."
    );
    assert!(!state.live_control_wired);
    assert_eq!(
        state.unsupported_notice,
        "MITM control is unsupported in this Rust/WASM dashboard. Server, certificate, DNS, and model mapping controls are disabled."
    );
    assert_eq!(state.server.title, "MITM Server");
    assert_eq!(state.server.status_label, "Stopped");
    assert!(!state.server.running);
    assert_eq!(
        checks,
        [("Cert", false), ("Trusted", false), ("Server", false)]
    );
    assert_eq!(
        state.server.purpose,
        "Use Antigravity IDE & GitHub Copilot → with ANY provider/model from 9Router"
    );
    assert_eq!(
        state.server.how_it_works,
        "Antigravity/Copilot IDE request → DNS redirect to localhost:443 → MITM proxy intercepts → 9Router → response to Antigravity/Copilot"
    );
    assert_eq!(state.server.base_url.label, "9Router Base URL");
    assert_eq!(state.server.base_url.value, "http://localhost:20128");
    assert!(!state.server.base_url.enabled);
    assert_eq!(state.server.api_key.label, "API Key");
    assert_eq!(state.server.api_key.placeholder, "sk_9router (default)");
    assert!(!state.server.api_key.enabled);
    assert_eq!(state.server.action.label, "Start Server");
    assert!(!state.server.action.enabled);
    assert!(UPSTREAM_PAGE.contains(state.risk_warning));
    assert!(UPSTREAM_SERVER.contains(state.server.purpose));
    assert!(UPSTREAM_SERVER.contains(state.server.how_it_works));
    assert!(serde_json::to_string(&state).is_ok());
}

#[test]
fn mitm_tools_match_upstream_hosts_images_models_and_disabled_boundaries() {
    // Given: upstream MITM constants define three supported IDE tools.
    let state = mitm_dashboard_state();

    // When/Then: every rendered datum is anchored directly to the frozen constants.
    assert_eq!(
        state.tools.iter().map(|tool| tool.id).collect::<Vec<_>>(),
        ["antigravity", "copilot", "kiro"]
    );
    for tool in state.tools {
        for expected in [
            format!("{}: {{", tool.id),
            format!("name: \"{}\"", tool.name),
            format!("image: \"{}\"", tool.image),
        ] {
            assert!(
                UPSTREAM_TOOLS.contains(&expected),
                "missing upstream tool datum: {expected}"
            );
        }
        for host in tool.hosts {
            let expected = format!("\"{host}\"");
            assert!(
                UPSTREAM_HOSTS.contains(&expected),
                "missing upstream host: {host}"
            );
        }
        for model in tool.models {
            for expected in [
                format!("name: \"{}\"", model.name),
                format!("alias: \"{}\"", model.alias),
            ] {
                assert!(
                    UPSTREAM_TOOLS.contains(&expected),
                    "missing upstream model datum: {expected}"
                );
            }
        }
    }

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
