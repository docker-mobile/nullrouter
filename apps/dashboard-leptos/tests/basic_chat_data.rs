use nullrouter_dashboard_wasm::dashboard::{
    basic_chat_dashboard_state, basic_chat_no_provider_state,
};

#[test]
fn basic_chat_dashboard_state_exposes_upstream_chat_controls_when_catalog_has_models() {
    // Given: the local model catalog has provider/model defaults for the WASM dashboard.
    let state = basic_chat_dashboard_state();

    // When: Basic Chat consumes that state for the route surface.
    let openai_group = state
        .provider_groups
        .iter()
        .find(|group| group.provider_id == "openai");

    // Then: the route has the upstream-style selector, history, empty chat, and composer controls.
    assert_eq!(state.route_path, "/dashboard/basic-chat");
    assert_eq!(state.model_menu_title, "Models");
    assert_eq!(state.model_menu_subtitle, "Only from connected providers");
    assert_eq!(state.empty_title, "Start a conversation");
    assert_eq!(state.composer.placeholder, "Message AI");
    assert_eq!(state.composer.attachment_label, "Attach image");
    assert_eq!(state.composer.send_label, "Send message");
    assert_eq!(state.composer.stop_label, "Stop response");
    assert!(!state.composer.can_send);
    assert!(!state.composer.can_stop);
    assert!(!state.execution_wired);
    assert!(!state.persistence_wired);
    assert_eq!(state.history.title, "Recent chats");
    assert_eq!(state.history.clear_label, "Clear");
    let session = state
        .history
        .sessions
        .first()
        .expect("happy state should expose a local session label");
    assert_eq!(session.title, "New chat");
    assert_eq!(session.preview, "Empty chat");
    assert_eq!(state.active_model_label, "openai/gpt-5");
    assert_eq!(state.composer.model_label, "openai/gpt-5");
    assert!(state.transcript_hooks.contains(&"nr-chat-transcript"));
    assert!(
        state
            .transcript_hooks
            .contains(&"nr-chat-message assistant")
    );

    let openai_group = openai_group.expect("openai model group should be derived from catalog");
    assert!(openai_group.models.iter().any(|model| {
        model.request_model == "openai/gpt-5" && model.source_label == "Catalog default"
    }));
}

#[test]
fn basic_chat_no_provider_state_keeps_boundary_controls_stable_and_truthful() {
    // Given: provider/model hydration returns no usable providers.
    let state = basic_chat_no_provider_state();

    // When: the route renders the empty provider boundary.
    // Then: it keeps the upstream controls visible without claiming chat execution.
    assert!(state.provider_groups.is_empty());
    assert!(state.history.sessions.is_empty());
    assert_eq!(state.provider_boundary_title, "No providers connected yet");
    assert_eq!(state.history.empty_label, "No conversations yet");
    assert_eq!(state.active_model_label, "No model");
    assert_eq!(state.composer.model_label, "No model");
    assert_eq!(state.composer.placeholder, "Message AI");
    assert_eq!(state.composer.attachment_label, "Attach image");
    assert_eq!(state.composer.send_label, "Send message");
    assert_eq!(state.composer.stop_label, "Stop response");
    assert!(!state.composer.can_send);
    assert!(!state.composer.can_stop);
    assert!(!state.execution_wired);
    assert!(!state.persistence_wired);
}
