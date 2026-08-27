use nullrouter_dashboard_wasm::ui::translator_visible_contract;

#[test]
fn translator_visible_contract_contains_upstream_debug_workspace_surface() {
    // Given: the concrete Leptos route exposes a visible contract for CSR-only tests.
    let contract = translator_visible_contract();

    // When: tests assert route-level hooks and user-facing labels.
    // Then: the translator route can be distinguished from the generic G003 placeholder.
    for expected in [
        "nr-translator-panel",
        "nr-translator-meta",
        "nr-translator-step",
        "nr-translator-code",
        "Translator Debug",
        "Replay request flow — matches log files",
        "Client Request",
        "1_req_client.json",
        "Source Body",
        "2_req_source.json",
        "OpenAI Intermediate",
        "3_req_openai.json",
        "Target Request",
        "4_req_target.json",
        "Provider Response",
        "5_res_provider.txt",
        "OpenAI Response",
        "6_res_openai.txt",
        "Client Response",
        "7_res_client.txt",
        "7_res_client.json",
        "Raw request from client",
        "source → openai",
        "openai → target + URL + headers",
        "json",
        "text",
        "expand_more",
        "chevron_right",
        "Load",
        "Copy",
        "Format",
        "→ OpenAI",
        "→ Target",
        "Send",
        "src:",
        "dst:",
        "provider:",
        "model:",
        "Filesystem",
        "Save",
        "Provider execution",
        "Persistence",
    ] {
        assert!(
            contract.contains(&expected),
            "missing visible contract: {expected}"
        );
    }
}

#[test]
fn translator_visible_contract_does_not_use_old_generic_placeholder_copy() {
    // Given: the prior G003 translator panel used generic placeholder strings.
    let contract = translator_visible_contract();

    // When: the concrete route declares the strings it renders.
    // Then: those placeholders are no longer the visible Translator contract.
    for old_placeholder in [
        "Translation API not connected",
        "No translation loaded",
        "Not persisted",
    ] {
        assert!(
            !contract.contains(&old_placeholder),
            "old placeholder still visible: {old_placeholder}"
        );
    }
}
