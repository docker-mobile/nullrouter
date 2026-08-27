use nullrouter_dashboard_wasm::dashboard::{
    media_provider_combo_detail_state, media_provider_detail_state, media_provider_kind_state,
};
use nullrouter_dashboard_wasm::ui::{DashboardRoute, DashboardSection};

#[test]
fn media_provider_nested_routes_parse_and_serialize_upstream_paths() {
    // Given: upstream nested media provider URLs.
    let kind_route = DashboardRoute::from_path("/dashboard/media-providers/image/");
    let detail_route = DashboardRoute::from_path("/dashboard/media-providers/tts/openai");
    let combo_route = DashboardRoute::from_path("/dashboard/media-providers/combo/combo_1");
    let redirected_search = DashboardRoute::from_path("/dashboard/media-providers/webSearch/");
    let redirected_fetch = DashboardRoute::from_path("/dashboard/media-providers/webFetch");

    // When: the WASM shell normalizes path and hash-driven route state.
    let detail_json = serde_json::to_value(&detail_route);
    let combo_json = serde_json::to_value(&combo_route);

    // Then: concrete nested routes are preserved without regressing the active nav.
    assert_eq!(kind_route, DashboardRoute::media_provider_kind("image"));
    assert_eq!(
        detail_route,
        DashboardRoute::media_provider_detail("tts", "openai")
    );
    assert_eq!(combo_route, DashboardRoute::media_provider_combo("combo_1"));
    assert_eq!(
        redirected_search,
        DashboardRoute::for_section(DashboardSection::MediaProvidersWeb)
    );
    assert_eq!(
        redirected_fetch,
        DashboardRoute::for_section(DashboardSection::MediaProvidersWeb)
    );
    assert_eq!(kind_route.section(), DashboardSection::MediaProvidersWeb);
    assert_eq!(detail_route.section(), DashboardSection::MediaProvidersWeb);
    assert_eq!(combo_route.section(), DashboardSection::MediaProvidersWeb);
    assert_eq!(
        DashboardRoute::from_hash("#media-providers/image/openai"),
        DashboardRoute::media_provider_detail("image", "openai")
    );

    let detail_json = detail_json.expect("media provider detail serialization should succeed");
    let combo_json = combo_json.expect("media provider combo serialization should succeed");
    assert_eq!(
        detail_json.get("kind"),
        Some(&serde_json::json!("media-provider-detail"))
    );
    assert_eq!(
        detail_json.get("providerKind"),
        Some(&serde_json::json!("tts"))
    );
    assert_eq!(
        detail_json.get("providerId"),
        Some(&serde_json::json!("openai"))
    );
    assert_eq!(
        combo_json.get("kind"),
        Some(&serde_json::json!("media-provider-combo"))
    );
    assert_eq!(
        combo_json.get("comboId"),
        Some(&serde_json::json!("combo_1"))
    );
}

#[test]
fn media_provider_kind_data_matches_upstream_labels_with_preview_actions() {
    // Given: upstream media-provider kind pages for built-in and unknown kinds.
    let image = media_provider_kind_state("image");
    let unknown = media_provider_kind_state("unknown-kind");

    // When: the Leptos dashboard renders kind list preview data.
    let image = image.expect("image provider kind should exist");
    let json = serde_json::to_string(&image);

    // Then: labels and disabled actions mirror upstream intent without claiming persistence.
    assert!(json.is_ok());
    assert_eq!(image.route_path, "/dashboard/media-providers/image");
    assert_eq!(image.kind.id, "image");
    assert_eq!(image.kind.label, "Text to Image");
    assert_eq!(image.kind.endpoint_path, "/v1/images/generations");
    assert!(!image.provider_mutations_wired);
    assert!(!image.combo_mutations_wired);
    assert!(image.actions.iter().all(|action| !action.enabled));
    assert!(
        image
            .providers
            .iter()
            .any(|provider| provider.id == "openai" && provider.status.total == 0)
    );
    assert!(image.combos.iter().all(|combo| !combo.persisted));

    let unknown = unknown.expect("unknown kind should return a placeholder state");
    assert_eq!(unknown.kind.id, "unknown-kind");
    assert_eq!(unknown.kind.label, "Unknown Media Provider");
    assert!(unknown.providers.is_empty());
    assert!(unknown.placeholder.is_some());
}

#[test]
fn media_provider_detail_data_keeps_unknown_provider_as_placeholder() {
    // Given: upstream provider detail URLs for known and unknown provider ids.
    let detail = media_provider_detail_state("tts", "openai");
    let wrong_kind = media_provider_detail_state("music", "openai");
    let unknown = media_provider_detail_state("tts", "missing-provider");

    // When: the WASM data layer exposes direct detail state.
    let detail = detail.expect("openai tts detail should exist");
    let json = serde_json::to_string(&detail);

    // Then: known details are nonblank and every mutation/test action is preview-disabled.
    assert!(json.is_ok());
    assert_eq!(detail.route_path, "/dashboard/media-providers/tts/openai");
    assert_eq!(detail.kind.label, "Text To Speech");
    assert_eq!(detail.provider.id, "openai");
    assert!(
        detail
            .connection_actions
            .iter()
            .all(|action| !action.enabled)
    );
    assert!(detail.test_actions.iter().all(|action| !action.enabled));
    assert!(!detail.connection_writes_wired);
    assert!(!detail.test_execution_wired);
    assert!(!detail.model_settings_wired);

    let wrong_kind = wrong_kind.expect("wrong kind/provider pairing should return a placeholder");
    assert_eq!(wrong_kind.provider.id, "openai");
    assert!(wrong_kind.placeholder.is_some());

    let unknown = unknown.expect("unknown provider should return a placeholder");
    assert_eq!(unknown.provider.id, "missing-provider");
    assert_eq!(unknown.provider.name, "Unknown Provider");
    assert!(unknown.placeholder.is_some());
}

#[test]
fn media_provider_combo_detail_data_is_nonblank_preview_state() {
    // Given: upstream combo detail URLs for known and unknown combos.
    let combo = media_provider_combo_detail_state("combo_1");
    let unknown = media_provider_combo_detail_state("missing-combo");

    // When: the WASM data layer exposes combo editing/test preview state.
    let combo = combo.expect("combo_1 preview should exist");
    let json = serde_json::to_string(&combo);

    // Then: settings, provider order, test example, and logs are present but disabled.
    assert!(json.is_ok());
    assert_eq!(combo.route_path, "/dashboard/media-providers/combo/combo_1");
    assert_eq!(combo.name, "embedding_combo");
    assert_eq!(combo.kind.label, "Embedding");
    assert_eq!(combo.example_path, Some("/v1/embeddings"));
    assert!(
        combo
            .members
            .iter()
            .any(|member| member.provider_id == "openai")
    );
    assert!(combo.actions.iter().all(|action| !action.enabled));
    assert!(!combo.persistence_wired);
    assert!(!combo.test_execution_wired);
    assert_eq!(combo.usage_log_status, "No usage yet.");

    let unknown = unknown.expect("unknown combo should return a placeholder");
    assert_eq!(unknown.name, "missing-combo");
    assert!(unknown.placeholder.is_some());
    assert!(unknown.members.is_empty());
}
