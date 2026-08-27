#[path = "dashboard_data/media_providers.rs"]
mod media_providers;
#[path = "dashboard_data/pricing.rs"]
mod pricing;

use nullrouter_dashboard_wasm::dashboard::{
    basic_chat_state, cli_tool_detail_state, cli_tools, combo_summaries, console_log_state,
    media_providers_web_state, model_catalog, profile_state, provider_detail_state,
    provider_groups, provider_new_state, proxy_pools_state, quota_tracker_state, skill_summaries,
    token_saver_state, translator_state, usage_snapshot,
};
use nullrouter_dashboard_wasm::ui::{DashboardRoute, DashboardSection, dashboard_sections};

#[test]
fn provider_groups_include_reference_auth_surfaces_when_rendering_dashboard() {
    // Given: the local dashboard fixture catalog.
    let groups = provider_groups();

    // When: provider groups are exposed to the CSR view.
    let oauth = groups.iter().find(|group| group.title == "OAuth Providers");
    let api_key = groups
        .iter()
        .find(|group| group.title == "API Key Providers");

    // Then: the shell can render both 9Router provider sections with status tiles.
    assert!(oauth.is_some());
    assert!(api_key.is_some());
    assert!(
        groups
            .iter()
            .flat_map(|group| &group.providers)
            .any(|provider| { provider.id == "claude" && provider.status.connected == 0 })
    );
}

#[test]
fn model_catalog_serializes_openai_compatible_entries_when_served_to_wasm() {
    // Given: the local model catalog used by the Leptos dashboard.
    let models = model_catalog();

    // When: the catalog is serialized at the browser boundary.
    let json = serde_json::to_string(&models);

    // Then: it exposes OpenAI-compatible identifiers used by the dashboard tiles.
    assert!(json.is_ok());
    assert!(models.iter().any(|model| model.id == "openai/gpt-5"));
    assert!(models.iter().any(|model| model.provider == "opencode"));
}

#[test]
fn usage_snapshot_renders_reference_topology_without_claiming_live_execution() {
    // Given: the dashboard usage slice has no host telemetry stream yet.
    let snapshot = usage_snapshot();

    // When: the Leptos usage panel renders topology and recent requests.
    let provider_ids = snapshot
        .topology_providers
        .iter()
        .map(|provider| provider.id.as_str())
        .collect::<Vec<_>>();

    // Then: it exposes the 9Router topology shell while keeping live usage honest.
    assert!(!snapshot.stream_connected);
    assert_eq!(snapshot.active_requests, 0);
    assert_eq!(snapshot.requests_today, 0);
    assert!(snapshot.recent_requests.is_empty());
    assert!(provider_ids.contains(&"claude"));
    assert!(provider_ids.contains(&"codex"));
}

#[test]
fn dashboard_sections_include_active_parity_routes_when_hash_driven() {
    // Given: the Leptos dashboard section registry used by nav and hash state.
    let sections = dashboard_sections();

    // When: G002 and G003 parity routes are exposed to the shell.
    let hashes = sections
        .iter()
        .map(|section| section.hash())
        .collect::<Vec<_>>();

    // Then: the formerly inactive system routes are active and addressable.
    // 19 = the 18 upstream-parity sections plus `migrate`, which is
    // nullrouter-specific (upstream is the migration source, so it has no
    // equivalent page).
    assert_eq!(sections.len(), 19);
    assert!(
        hashes.contains(&"migrate"),
        "the 9Router import surface must be addressable"
    );
    assert!(hashes.contains(&"combos"));
    assert!(hashes.contains(&"quota"));
    assert!(hashes.contains(&"token-saver"));
    assert!(hashes.contains(&"cli-tools"));
    assert!(hashes.contains(&"skills"));
    assert!(hashes.contains(&"basic-chat"));
    assert!(hashes.contains(&"proxy-pools"));
    assert!(hashes.contains(&"translator"));
    assert!(hashes.contains(&"console-log"));
    assert!(hashes.contains(&"media-providers-web"));
    assert!(hashes.contains(&"profile"));
    assert!(hashes.contains(&"mitm"));
    assert!(hashes.contains(&"settings-pricing"));
    assert_eq!(
        DashboardSection::from_hash("#settings-pricing"),
        DashboardSection::SettingsPricing
    );
    assert_eq!(
        DashboardSection::from_hash("#token-saver"),
        DashboardSection::TokenSaver
    );
    assert_eq!(
        DashboardSection::from_hash("#unknown"),
        DashboardSection::Endpoint
    );
}

#[test]
fn parity_panel_data_is_truthful_about_unwired_backend_execution() {
    // Given: compact data slices used by the new WASM route panels.
    let combos = combo_summaries();
    let quota = quota_tracker_state();
    let token_saver = token_saver_state();
    let tools = cli_tools();
    let skills = skill_summaries();

    // When: the panels render upstream-intent placeholders.
    let tool_names = tools.iter().map(|tool| tool.name).collect::<Vec<_>>();
    let skill_ids = skills.iter().map(|skill| skill.id).collect::<Vec<_>>();

    // Then: the UI has useful content without claiming missing persistence or execution.
    assert!(combos.iter().any(|combo| combo.name == "coding-fallback"));
    assert!(combos.iter().all(|combo| !combo.persisted));
    assert!(!quota.live_limits_connected);
    assert!(quota.rows.is_empty());
    assert!(!token_saver.rtk_wired);
    assert!(!token_saver.headroom_wired);
    assert!(tool_names.contains(&"Codex CLI"));
    assert!(tool_names.contains(&"MITM Bridge"));
    assert!(tools.iter().all(|tool| !tool.status_checked));
    assert!(skill_ids.contains(&"9router-chat"));
    assert!(skill_ids.contains(&"9router-web-fetch"));
}

#[test]
fn dashboard_sections_map_upstream_paths_to_g003_panels_when_path_driven() {
    // Given: upstream dashboard URLs that should land on concrete Leptos sections.
    let cases = [
        ("/dashboard", DashboardSection::Endpoint),
        ("/dashboard/endpoint", DashboardSection::Endpoint),
        ("/dashboard/basic-chat", DashboardSection::BasicChat),
        ("/dashboard/providers", DashboardSection::Providers),
        ("/dashboard/proxy-pools", DashboardSection::ProxyPools),
        ("/dashboard/translator", DashboardSection::Translator),
        ("/dashboard/usage", DashboardSection::Usage),
        ("/dashboard/status", DashboardSection::Status),
        ("/dashboard/settings", DashboardSection::Settings),
        ("/dashboard/console-log", DashboardSection::ConsoleLog),
        (
            "/dashboard/media-providers/web",
            DashboardSection::MediaProvidersWeb,
        ),
        ("/dashboard/combos", DashboardSection::Combos),
        ("/dashboard/quota", DashboardSection::QuotaTracker),
        ("/dashboard/token-saver", DashboardSection::TokenSaver),
        ("/dashboard/cli-tools", DashboardSection::CliTools),
        ("/dashboard/skills", DashboardSection::Skills),
        ("/dashboard/profile", DashboardSection::Profile),
        ("/dashboard/mitm", DashboardSection::Mitm),
        (
            "/dashboard/settings/pricing",
            DashboardSection::SettingsPricing,
        ),
    ];

    // When: the browser pathname is normalized without a hash fragment.
    // Then: each upstream page maps to the matching nonblank dashboard panel.
    for (path, section) in cases {
        assert_eq!(DashboardSection::from_path(path), section);
    }
    assert_eq!(
        DashboardSection::from_path("/dashboard/unknown"),
        DashboardSection::Endpoint
    );
}

#[test]
fn nested_dashboard_routes_parse_upstream_detail_paths_into_serializable_state() {
    // Given: upstream nested dashboard paths that carry provider and CLI tool ids.
    let provider_new_route = DashboardRoute::from_path("/dashboard/providers/new");
    let provider_detail_route = DashboardRoute::from_path("/dashboard/providers/codex");
    let cli_tool_route = DashboardRoute::from_path("/dashboard/cli-tools/jcode");

    // When: the WASM shell parses those paths and serializes route state.
    let provider_detail_json = serde_json::to_value(&provider_detail_route);
    let cli_tool_json = serde_json::to_value(&cli_tool_route);

    // Then: each path lands on a concrete nested route without losing active nav context.
    assert_eq!(provider_new_route, DashboardRoute::ProviderNew);
    assert_eq!(
        provider_detail_route,
        DashboardRoute::provider_detail("codex")
    );
    assert_eq!(cli_tool_route, DashboardRoute::cli_tool_detail("jcode"));
    assert_eq!(provider_new_route.section(), DashboardSection::Providers);
    assert_eq!(provider_detail_route.section(), DashboardSection::Providers);
    assert_eq!(cli_tool_route.section(), DashboardSection::CliTools);
    assert_eq!(
        DashboardRoute::from_hash("#cli-tools"),
        DashboardRoute::for_section(DashboardSection::CliTools)
    );

    let provider_detail_json = provider_detail_json.expect("route serialization should succeed");
    let cli_tool_json = cli_tool_json.expect("route serialization should succeed");
    assert_eq!(
        provider_detail_json.get("kind"),
        Some(&serde_json::json!("provider-detail"))
    );
    assert_eq!(
        provider_detail_json.get("providerId"),
        Some(&serde_json::json!("codex"))
    );
    assert_eq!(
        cli_tool_json.get("kind"),
        Some(&serde_json::json!("cli-tool-detail"))
    );
    assert_eq!(
        cli_tool_json.get("toolId"),
        Some(&serde_json::json!("jcode"))
    );
}

#[test]
fn provider_nested_panel_data_is_serializable_preview_state() {
    // Given: upstream provider add/detail pages expose forms, connection lists, and model controls.
    let new_provider = provider_new_state();
    let detail = provider_detail_state("codex");

    // When: the WASM dashboard exposes those pages without persistence wiring.
    let new_provider_json = serde_json::to_string(&new_provider);
    let detail = detail.expect("codex provider detail should exist");
    let detail_json = serde_json::to_string(&detail);

    // Then: page data is useful and serializable while all unavailable actions stay disabled.
    assert!(new_provider_json.is_ok());
    assert_eq!(new_provider.title, "Add New Provider");
    assert_eq!(new_provider.default_auth_method, "api_key");
    assert!(new_provider.is_active_default);
    assert!(!new_provider.persistence_wired);
    assert!(
        new_provider
            .provider_options
            .iter()
            .any(|provider| provider.id == "codex")
    );
    assert!(
        new_provider
            .auth_methods
            .iter()
            .any(|method| method.id == "oauth2")
    );

    assert!(detail_json.is_ok());
    assert_eq!(detail.route_path, "/dashboard/providers/codex");
    assert_eq!(detail.provider.id, "codex");
    assert!(!detail.connections_wired);
    assert!(!detail.provider_settings_wired);
    assert!(detail.actions.iter().all(|action| !action.enabled));
    assert!(provider_detail_state("missing-provider").is_none());
}

#[test]
fn cli_tool_detail_panel_data_is_serializable_preview_state() {
    // Given: upstream CLI tool detail pages render per-tool setup and status cards.
    let detail = cli_tool_detail_state("jcode");

    // When: the WASM dashboard exposes a detail fixture for a known tool id.
    let detail = detail.expect("jcode CLI tool detail should exist");
    let json = serde_json::to_string(&detail);

    // Then: direct URLs have nonblank serializable setup state without host-side execution.
    assert!(json.is_ok());
    assert_eq!(detail.route_path, "/dashboard/cli-tools/jcode");
    assert_eq!(detail.tool.id, "jcode");
    assert_eq!(detail.base_url, "http://localhost:20128");
    assert!(!detail.install_detection_wired);
    assert!(!detail.api_keys_wired);
    assert!(detail.sections.iter().all(|section| !section.enabled));
    assert!(cli_tool_detail_state("unknown-tool").is_none());
}

#[test]
fn g003_panel_data_is_nonblank_without_claiming_live_integrations() {
    // Given: the remaining upstream dashboard pages rendered by G003.
    let states = [
        basic_chat_state(),
        proxy_pools_state(),
        translator_state(),
        console_log_state(),
        media_providers_web_state(),
        profile_state(),
    ];

    // When: the WASM panels consume route/API/persistence state.
    // Then: every page has truthful nonblank content and disabled actions.
    for state in states {
        assert!(state.route_path.starts_with("/dashboard/"));
        assert!(!state.title.is_empty());
        assert!(!state.api_status.is_empty());
        assert!(!state.persistence_status.is_empty());
        assert!(!state.controls_enabled);
        assert!(!state.empty_title.is_empty());
        assert!(!state.rows.is_empty());
    }
}
