use nullrouter_dashboard_wasm::ui::{
    DashboardSection, dashboard_header_controls, dashboard_media_navigation,
    dashboard_primary_navigation, dashboard_search, dashboard_section_path,
    dashboard_system_navigation,
};

type SectionMetadata = (
    DashboardSection,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
);

#[test]
fn dashboard_primary_metadata_matches_frozen_header_and_sidebar() {
    // Given: the primary destinations visible in the frozen 9Router dashboard shell.
    let expected = [
        (
            DashboardSection::Endpoint,
            "Endpoint",
            "Endpoint & Key",
            "API endpoint configuration",
            "api",
        ),
        (
            DashboardSection::Providers,
            "Providers",
            "Providers",
            "Manage your AI provider connections",
            "dns",
        ),
        (
            DashboardSection::Combos,
            "Combos",
            "Combos",
            "Model combos with fallback",
            "layers",
        ),
        (
            DashboardSection::Usage,
            "Usage & Analytics",
            "Usage",
            "Monitor your API usage, token consumption, and request logs",
            "bar_chart",
        ),
        (
            DashboardSection::QuotaTracker,
            "Quota Tracker",
            "Quota Tracker",
            "Track and manage your API quota limits",
            "data_usage",
        ),
        (
            DashboardSection::TokenSaver,
            "Token Saver",
            "Token Saver",
            "Compress prompts and outputs to save tokens",
            "savings",
        ),
        (
            DashboardSection::CliTools,
            "CLI Tools",
            "CLI Tools",
            "Configure CLI tools",
            "terminal",
        ),
    ];

    // When: the shared metadata is consumed by header, sidebar, and route search.
    // Then: every primary destination uses the exact source-facing copy and icon name.
    assert_section_metadata(&expected);
}

#[test]
fn dashboard_system_metadata_matches_frozen_header_and_sidebar() {
    // Given: the System destinations visible in the frozen 9Router dashboard shell.
    let expected = [
        (
            DashboardSection::MediaProvidersWeb,
            "Web Fetch & Search",
            "Media Providers",
            "Manage your Web Fetch & Search providers",
            "perm_media",
        ),
        (
            DashboardSection::ProxyPools,
            "Proxy Pools",
            "Proxy Pools",
            "Manage your proxy pool configurations",
            "lan",
        ),
        (
            DashboardSection::Skills,
            "Agent Skills",
            "Skills",
            "Copy a link and paste to your AI to use 9Router — no install needed",
            "extension",
        ),
        (
            DashboardSection::Mitm,
            "MITM Proxy",
            "MITM",
            "Intercept CLI tool traffic and route through 9Router",
            "security",
        ),
        (
            DashboardSection::ConsoleLog,
            "Console Log",
            "Console Log",
            "Live server console output",
            "monitor",
        ),
        (
            DashboardSection::Translator,
            "Translator",
            "Translator",
            "Debug translation flow between formats",
            "translate",
        ),
        (
            DashboardSection::Profile,
            "Settings",
            "Settings",
            "Manage your preferences",
            "settings",
        ),
    ];

    // When: the shared metadata is consumed by header, sidebar, and route search.
    // Then: every System destination uses the exact source-facing copy and icon name.
    assert_section_metadata(&expected);
}

fn assert_section_metadata(expected: &[SectionMetadata]) {
    for &(section, title, label, description, icon) in expected {
        assert_eq!(section.title(), title, "title mismatch for {section:?}");
        assert_eq!(section.nav_label(), label, "label mismatch for {section:?}");
        assert_eq!(
            section.description(),
            description,
            "description mismatch for {section:?}"
        );
        assert_eq!(section.icon(), icon, "icon mismatch for {section:?}");
    }
}

#[test]
fn dashboard_navigation_matches_frozen_groups_and_media_children() {
    // Given: the source-defined primary, System, and media-provider navigation groups.
    let primary = dashboard_primary_navigation();
    let system = dashboard_system_navigation();
    let media = dashboard_media_navigation();

    // When: the Leptos sidebar consumes the shared navigation model.
    // Then: order and nested media entries match the frozen Sidebar source.
    assert_eq!(
        primary,
        &[
            DashboardSection::Endpoint,
            DashboardSection::Providers,
            DashboardSection::Combos,
            DashboardSection::Usage,
            DashboardSection::QuotaTracker,
            DashboardSection::TokenSaver,
            DashboardSection::CliTools,
        ]
    );
    assert_eq!(
        primary
            .iter()
            .map(|section| dashboard_section_path(*section))
            .collect::<Vec<_>>(),
        vec![
            "/dashboard/endpoint",
            "/dashboard/providers",
            "/dashboard/combos",
            "/dashboard/usage",
            "/dashboard/quota",
            "/dashboard/token-saver",
            "/dashboard/cli-tools",
        ]
    );
    assert_eq!(
        system,
        &[
            DashboardSection::MediaProvidersWeb,
            DashboardSection::ProxyPools,
            DashboardSection::Skills,
            DashboardSection::Mitm,
            DashboardSection::ConsoleLog,
            DashboardSection::Translator,
            // Migrate sits immediately before Settings: it is an admin action a
            // user reaches once, during setup, not a daily surface.
            DashboardSection::Migrate,
            DashboardSection::Profile,
        ]
    );
    assert_eq!(
        media
            .iter()
            .map(|item| (item.id, item.label, item.icon, item.path))
            .collect::<Vec<_>>(),
        vec![
            (
                "embedding",
                "Embedding",
                "data_array",
                "/dashboard/media-providers/embedding",
            ),
            (
                "image",
                "Text to Image",
                "brush",
                "/dashboard/media-providers/image",
            ),
            (
                "tts",
                "Text To Speech",
                "record_voice_over",
                "/dashboard/media-providers/tts",
            ),
            (
                "stt",
                "Speech To Text",
                "mic",
                "/dashboard/media-providers/stt",
            ),
            (
                "web",
                "Web Fetch & Search",
                "travel_explore",
                "/dashboard/media-providers/web",
            ),
        ]
    );
}

#[test]
fn dashboard_header_controls_and_search_cover_keyboard_destinations() {
    // Given: the three G016 header controls and the completed dashboard destinations.
    let controls = dashboard_header_controls();

    // When: search receives mixed-case text, a shared media term, or no matching term.
    let mitm = dashboard_search("MiTm");
    let speech = dashboard_search("speech");
    let missing = dashboard_search("no-such-dashboard-destination");

    // Then: controls have stable ids/icons and search returns only real navigable results.
    assert_eq!(
        controls
            .iter()
            .map(|control| (control.id, control.label, control.icon))
            .collect::<Vec<_>>(),
        vec![
            ("search", "Search", "search"),
            ("language", "Language", "language"),
            ("account", "Menu", "grid_view"),
        ]
    );
    assert_eq!(
        mitm.iter()
            .map(|destination| (destination.label, destination.path))
            .collect::<Vec<_>>(),
        vec![("MITM", "/dashboard/mitm")]
    );
    assert_eq!(speech.len(), 2);
    assert!(
        speech
            .iter()
            .all(|destination| destination.path.starts_with("/dashboard/media-providers/"))
    );
    assert!(missing.is_empty());
}
