use nullrouter_dashboard_wasm::dashboard::pricing_settings_state;
use nullrouter_dashboard_wasm::ui::{DashboardSection, dashboard_sections};

#[test]
fn pricing_settings_route_and_state_match_upstream_empty_defaults() {
    // Given: upstream exposes /dashboard/settings/pricing backed by /api/pricing.
    let state = pricing_settings_state();

    // When: the path-driven dashboard bootstraps directly on the pricing settings URL.
    // Then: it selects a concrete pricing page instead of falling back to Endpoint.
    assert_eq!(
        DashboardSection::from_path("/dashboard/settings/pricing"),
        DashboardSection::SettingsPricing
    );
    assert_eq!(
        DashboardSection::from_hash("#settings-pricing"),
        DashboardSection::SettingsPricing
    );
    assert!(dashboard_sections().contains(&DashboardSection::SettingsPricing));

    // And: the local default state mirrors upstream labels without pretending persistence is live.
    assert_eq!(state.title, "Pricing Settings");
    assert_eq!(
        state.description,
        "Configure pricing rates for cost tracking and calculations"
    );
    assert_eq!(state.total_models, 0);
    assert_eq!(state.providers, 0);
    assert_eq!(state.status_label, "Preview");
    assert_eq!(state.current_pricing_title, "Current Pricing Overview");
    assert_eq!(state.empty_title, "No pricing data available");
    assert_eq!(state.stat_labels, ["Total Models", "Providers", "Status"]);
    assert_eq!(
        state.token_type_labels,
        ["Input", "Output", "Cached", "Reasoning", "Cache Creation"]
    );
    assert_eq!(
        state.modal_field_labels,
        ["Input", "Output", "Cached", "Reasoning", "Cache Creation"]
    );
    assert!(
        state
            .how_pricing_works
            .iter()
            .any(|line| line.contains("dollars per million tokens"))
    );
    assert_eq!(state.modal_persistence_label, "Persistence unsupported");
    assert!(state.modal_persistence_note.contains("preview"));
    assert!(!state.persistence_wired);
}
