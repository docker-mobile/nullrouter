use nullrouter_dashboard_wasm::ui::dashboard_shell_visible_contract;

#[test]
fn dashboard_shell_visible_contract_contains_mobile_drawer_hooks_and_semantics() {
    // Given: the dashboard shell owns responsive navigation behavior.
    let contract = dashboard_shell_visible_contract();

    // When: tests assert the drawer hooks used by CSS and browser QA.
    // Then: the shell exposes upstream-style mobile drawer controls.
    for expected in [
        "nr-sidebar-desktop",
        "nr-sidebar-drawer",
        "nr-sidebar-overlay",
        "nr-sidebar-open",
        "aria-label=\"Open dashboard navigation\"",
        "aria-label=\"Close mobile dashboard navigation\"",
        "title=\"Open dashboard navigation\"",
        "title=\"Close mobile dashboard navigation\"",
        "aria-controls=\"nr-mobile-sidebar\"",
        "aria-expanded",
        "id=\"nr-mobile-sidebar\"",
        "data-state=\"open\"",
        "data-state=\"closed\"",
        "aria-hidden=\"true\"",
        "material-symbols-outlined",
        "menu",
        "9Router Proxy",
        "v0.5.20",
        "hub",
        "nr-media-navigation",
        "aria-label=\"Toggle media providers\"",
        "nr-media-nav-item",
        "id=\"nr-header-search\"",
        "id=\"nr-header-language\"",
        "id=\"nr-header-account\"",
        "aria-label=\"Search dashboard\"",
        "aria-label=\"Language\"",
        "aria-label=\"Open account menu\"",
        "aria-haspopup=\"dialog\"",
        "aria-haspopup=\"menu\"",
        "No destinations found",
        "nr-header-popover-dismiss",
        "data-header-panel",
    ] {
        assert!(
            contract.contains(&expected),
            "missing shell contract: {expected}"
        );
    }

    for removed_drawer_hook in [
        "nr-sidebar-close",
        "☰",
        "×",
        "Leptos WASM",
        "Health",
        "Models",
        "cht",
        "pxy",
        "trn",
        "bar",
        "lay",
        "quo",
        "sav",
        "cli",
        "ext",
        "usr",
    ] {
        assert!(
            !contract.contains(&removed_drawer_hook),
            "removed drawer hook still present: {removed_drawer_hook}"
        );
    }
}
