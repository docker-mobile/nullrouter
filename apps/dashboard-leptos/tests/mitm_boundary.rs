use nullrouter_dashboard_wasm::{
    dashboard::mitm_dashboard_state,
    ui::{DashboardRoute, DashboardSection},
};

const UI_SOURCE: &str = include_str!("../src/ui/mitm.rs");

#[test]
fn mitm_route_parses_upstream_path_and_hash_to_the_dedicated_section() {
    // Given: the upstream dashboard exposes MITM at the canonical route.
    let path_route = DashboardRoute::from_path("/dashboard/mitm");
    let hash_route = DashboardRoute::from_hash("#/mitm");

    // When: both CSR entry paths are resolved.
    // Then: they select the dedicated MITM section rather than a fallback route.
    assert_eq!(
        path_route,
        DashboardRoute::for_section(DashboardSection::Mitm)
    );
    assert_eq!(
        hash_route,
        DashboardRoute::for_section(DashboardSection::Mitm)
    );
    assert_eq!(DashboardSection::Mitm.title(), "MITM Proxy");
    assert_eq!(
        DashboardSection::Mitm.description(),
        "Intercept CLI tool traffic and route through 9Router"
    );
    assert_eq!(DashboardSection::Mitm.icon(), "security");
}

#[test]
fn mitm_render_source_contains_the_upstream_server_and_current_tool_surface() {
    // Given: the actual Leptos render source and typed state are available.
    let state = mitm_dashboard_state();

    // When: route-level hooks and component calls are inspected.
    // Then: the real render path contains the responsive server and tool surfaces.
    for expected in [
        "nr-mitm-panel",
        "nr-mitm-risk",
        "nr-mitm-server",
        "nr-mitm-server-icon",
        "nr-mitm-badge",
        "nr-mitm-field-arrow",
        "nr-mitm-tool-list",
        "nr-mitm-tool",
        "nr-mitm-model-arrow",
        "nr-mitm-action-icon",
        "nr-mitm-chevron",
        "nr-mitm-model-row",
        "material-symbols-outlined",
        "data-icon=\"security\"",
        "data-icon=\"arrow_forward\"",
        "data-icon=\"play_circle\"",
        "data-icon=\"expand_more\"",
        "data-icon=\"warning\"",
        "data-icon=\"cancel\"",
        "alt=\"\"",
        "aria-expanded",
        "<ServerCard state />",
        "<ToolCard tool state expanded_tool />",
        "<ModelRow model tool state />",
    ] {
        assert!(
            UI_SOURCE.contains(expected),
            "missing render hook: {expected}"
        );
    }
    assert_eq!(state.server.title, "MITM Server");
    assert_eq!(state.server.status_label, "Stopped");
    assert_eq!(
        state.tools.iter().map(|tool| tool.name).collect::<Vec<_>>(),
        ["Antigravity", "GitHub Copilot", "Kiro"]
    );
    assert!(state.tools.iter().all(|tool| !tool.server_running));
}

#[test]
fn mitm_styles_use_the_upstream_compact_primitives_and_sm_breakpoint() {
    let styles = include_str!("../src/ui/mitm/styles.rs");

    for expected in [
        "--mitm-badge-font",
        "--mitm-control-radius",
        "--mitm-copy-host",
        "@media (max-width:639px)",
    ] {
        assert!(
            styles.contains(expected),
            "missing MITM style contract: {expected}"
        );
    }
    assert!(!styles.contains("max-width:860px"));
    assert!(!styles.contains(".nr-mitm-tool-head:hover"));
}

#[test]
fn mitm_render_source_replaces_the_generic_g003_placeholder() {
    // Given: the old route rendered the generic G003 MITM state.
    for component in ["MitmPanel", "ServerCard", "ToolCard", "ModelRow"] {
        assert!(
            UI_SOURCE.contains(component),
            "missing dedicated component: {component}"
        );
    }

    // When: the dedicated render source is inspected.
    // Then: concrete components replace all generic placeholder copy.
    for old_placeholder in [
        "MITM API not connected",
        "Not persisted",
        "MITM bridge inactive",
        "Root CA",
        "Proxy listener",
        "Capture",
    ] {
        assert!(
            !UI_SOURCE.contains(old_placeholder),
            "old placeholder still rendered: {old_placeholder}"
        );
    }
}
