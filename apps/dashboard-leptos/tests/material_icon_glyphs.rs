use nullrouter_dashboard_wasm::ui::{
    dashboard_account_actions, dashboard_header_controls, dashboard_icon_glyph,
    dashboard_media_navigation, dashboard_search, dashboard_sections,
};

const EXPECTED_GLYPHS: &[(&str, &str)] = &[
    ("api", "\u{f1b7}"),
    ("bar_chart", "\u{e26b}"),
    ("brush", "\u{e3ae}"),
    ("chat", "\u{e0b7}"),
    ("check", "\u{e5ca}"),
    ("close", "\u{e5cd}"),
    ("computer", "\u{e30a}"),
    ("content_copy", "\u{e14d}"),
    ("dark_mode", "\u{e51c}"),
    ("data_array", "\u{ead1}"),
    ("data_usage", "\u{e1af}"),
    ("dns", "\u{e875}"),
    ("expand_more", "\u{e5cf}"),
    ("extension", "\u{e87b}"),
    ("grid_view", "\u{e9b0}"),
    ("history", "\u{e889}"),
    ("hub", "\u{e9f4}"),
    ("lan", "\u{eb2f}"),
    ("language", "\u{e894}"),
    ("layers", "\u{e53b}"),
    ("logout", "\u{e9ba}"),
    ("menu", "\u{e5d2}"),
    ("mic", "\u{e029}"),
    ("monitor", "\u{ef5b}"),
    ("monitoring", "\u{f190}"),
    ("payments", "\u{ef63}"),
    ("perm_media", "\u{e8a7}"),
    ("power_settings_new", "\u{e8ac}"),
    ("record_voice_over", "\u{e91f}"),
    ("savings", "\u{e2eb}"),
    ("search", "\u{e8b6}"),
    ("security", "\u{e32a}"),
    ("settings", "\u{e8b8}"),
    ("terminal", "\u{eb8e}"),
    ("translate", "\u{e8e2}"),
    ("travel_explore", "\u{e2db}"),
];

#[test]
fn dashboard_material_symbols_use_exact_single_glyphs_when_rendered() {
    // Given: every semantic icon used by the shared dashboard shell.
    for &(name, expected) in EXPECTED_GLYPHS {
        // When: the render boundary resolves the semantic name.
        let glyph = dashboard_icon_glyph(name);

        // Then: it returns the exact upstream Material Symbols PUA glyph.
        assert_eq!(glyph, expected, "unexpected glyph for {name}");
    }
}

#[test]
fn dashboard_shell_icon_inventory_has_no_ligature_text_fallbacks() {
    // Given: every data-driven icon plus the shell's fixed controls.
    let data_icons = dashboard_sections()
        .iter()
        .map(|section| section.icon())
        .chain(dashboard_media_navigation().iter().map(|item| item.icon))
        .chain(dashboard_header_controls().iter().map(|item| item.icon))
        .chain(dashboard_account_actions().iter().map(|item| item.icon))
        .chain(dashboard_search("").into_iter().map(|item| item.icon))
        .chain([
            "hub",
            "menu",
            "close",
            "expand_more",
            "computer",
            "content_copy",
            "check",
        ]);

    for name in data_icons {
        // When: the icon reaches the text-rendering boundary.
        let glyph = dashboard_icon_glyph(name);
        let mut characters = glyph.chars();
        let codepoint = characters.next().map(u32::from);

        // Then: exactly one private-use glyph is rendered, never the ligature name.
        assert!(characters.next().is_none(), "{name} rendered as {glyph:?}");
        assert!(
            codepoint.is_some_and(|value| (0xe000..=0xf8ff).contains(&value)),
            "{name} did not resolve to a private-use glyph"
        );
    }
}
