use nullrouter_dashboard_wasm::ui::proxy_pools_visible_contract;

#[test]
fn proxy_pools_visible_contract_contains_concrete_hooks_and_strings() {
    // Given: the concrete Leptos route exposes a visible contract for CSR-only tests.
    let contract = proxy_pools_visible_contract();

    // When: tests assert the route-level hooks and user-facing labels.
    // Then: the panel can be distinguished from the generic G003 placeholder.
    for expected in [
        "nr-proxy-pools-panel",
        "nr-proxy-relay-menu",
        "nr-proxy-bulk-bar",
        "nr-proxy-empty",
        "nr-proxy-row",
        "nr-proxy-modal-grid",
        "Proxy Pools",
        "Deploy Relay",
        "Cloudflare Relay",
        "Vercel Relay",
        "Deno Relay",
        "Batch Import",
        "Add Proxy Pool",
        "Total:",
        "Active:",
        "Select all",
        "0 selected",
        "Health Check",
        "Checking 0/0",
        "No proxy pool entries yet",
        "Create a proxy pool entry, then assign it to connections.",
        "cloudflare relay",
        "No proxy:",
        "Last tested:",
        "Test proxy",
        "Edit",
        "Delete",
        "Batch Import Proxies",
        "Add/Edit Proxy Pool",
        "Deploy Vercel Relay",
        "Deploy Cloudflare Relay",
        "Deploy Deno Relay",
    ] {
        assert!(
            contract.contains(&expected),
            "missing visible contract: {expected}"
        );
    }
}

#[test]
fn proxy_pools_visible_contract_does_not_use_old_generic_placeholder_copy() {
    // Given: the prior G003 panel used generic placeholder strings.
    let contract = proxy_pools_visible_contract();

    // When: the concrete route declares the strings it renders.
    // Then: those generic placeholders are no longer the visible Proxy Pools contract.
    for old_placeholder in [
        "Proxy API not connected",
        "No proxy pools configured",
        "Not persisted",
    ] {
        assert!(
            !contract.contains(&old_placeholder),
            "old placeholder still visible: {old_placeholder}"
        );
    }
}
