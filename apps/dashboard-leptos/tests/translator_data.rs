use nullrouter_dashboard_wasm::dashboard::{TranslatorStepLanguage, translator_dashboard_state};

#[test]
fn translator_dashboard_state_exposes_log_replay_steps_when_unwired() {
    // Given: the WASM translator route has no host translator APIs mounted yet.
    let state = translator_dashboard_state();

    // When: the dashboard consumes its deterministic replay workspace state.
    let steps = state.steps;
    let files = steps.iter().map(|step| step.file).collect::<Vec<_>>();
    let labels = steps.iter().map(|step| step.label).collect::<Vec<_>>();
    let descriptions = steps
        .iter()
        .map(|step| step.description)
        .collect::<Vec<_>>();

    // Then: the upstream seven-step translator log flow is visible and exact.
    assert_eq!(state.route_path, "/dashboard/translator");
    assert_eq!(state.title, "Translator Debug");
    assert_eq!(state.subtitle, "Replay request flow — matches log files");
    assert_eq!(steps.len(), 7);
    assert_eq!(
        labels,
        [
            "Client Request",
            "Source Body",
            "OpenAI Intermediate",
            "Target Request",
            "Provider Response",
            "OpenAI Response",
            "Client Response",
        ]
    );
    assert_eq!(
        files,
        [
            "1_req_client.json",
            "2_req_source.json",
            "3_req_openai.json",
            "4_req_target.json",
            "5_res_provider.txt",
            "6_res_openai.txt",
            "7_res_client.txt",
        ]
    );
    assert_eq!(
        descriptions,
        [
            "Raw request from client",
            "After initial conversion",
            "source → openai",
            "openai → target + URL + headers",
            "Raw SSE from provider",
            "target → openai (response)",
            "Final response to client",
        ]
    );
    assert!(
        steps
            .iter()
            .any(|step| { step.id == 1 && step.language == TranslatorStepLanguage::Json })
    );
    assert!(
        steps
            .iter()
            .any(|step| { step.id == 5 && step.language == TranslatorStepLanguage::Text })
    );
    assert!(
        steps
            .iter()
            .any(|step| { step.id == 7 && step.api_default_file == Some("7_res_client.json") })
    );
}

#[test]
fn translator_dashboard_state_marks_all_execution_boundaries_disabled() {
    // Given: the translator workspace mirrors upstream controls without host wiring.
    let state = translator_dashboard_state();

    // When: controls and metadata are inspected at the browser boundary.
    let common_actions = state
        .common_actions
        .iter()
        .map(|action| action.label)
        .collect::<Vec<_>>();
    let primary_actions = state
        .steps
        .iter()
        .filter_map(|step| step.primary_action.map(|action| action.label))
        .collect::<Vec<_>>();
    let meta_labels = state
        .meta
        .iter()
        .map(|badge| badge.label)
        .collect::<Vec<_>>();

    // Then: affordances are present while filesystem, persistence, and provider execution stay honest.
    assert_eq!(common_actions, ["Load", "Copy", "Format"]);
    assert_eq!(primary_actions, ["→ OpenAI", "→ Target", "Send"]);
    assert_eq!(meta_labels, ["src", "dst", "provider", "model"]);
    assert!(state.common_actions.iter().all(|action| !action.enabled));
    assert!(
        state
            .steps
            .iter()
            .filter_map(|step| step.primary_action)
            .all(|action| !action.enabled)
    );
    assert!(
        state
            .capabilities
            .iter()
            .all(|capability| !capability.enabled)
    );
    assert!(
        state
            .capabilities
            .iter()
            .any(|capability| capability.label == "Filesystem")
    );
    assert!(
        state
            .capabilities
            .iter()
            .any(|capability| capability.label == "Persistence")
    );
}
