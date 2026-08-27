use nullrouter_dashboard_wasm::ui::{dashboard_account_actions, dashboard_locales};

#[test]
fn dashboard_language_and_account_menus_are_complete_and_truthful() {
    // Given: the frozen language inventory and account menu commands.
    let locales = dashboard_locales();
    let account_actions = dashboard_account_actions();

    // When: the header menus render from their shared immutable data.
    // Then: every locale exists and unavailable host actions remain explicitly disabled.
    // 35 locales, matching upstream `i18n/config.js` exactly. Only 34 ship
    // literal files: `en` is the source language and needs none.
    assert_eq!(locales.len(), 35);
    assert_eq!(
        locales.iter().map(|locale| locale.id).collect::<Vec<_>>(),
        vec![
            "en", "vi", "zh-CN", "zh-TW", "ja", "pt-BR", "pt-PT", "ko", "es", "de", "fr", "he",
            "ar", "ru", "pl", "cs", "nl", "tr", "uk", "tl", "id", "km", "th", "hi", "bn", "ur",
            "ro", "sv", "it", "el", "hu", "fi", "da", "no", "fa",
        ]
    );
    assert_eq!(locales.first().map(|locale| locale.name), Some("English"));
    assert_eq!(locales.last().map(|locale| locale.name), Some("فارسی"));
    assert_eq!(
        account_actions
            .iter()
            .map(|action| (action.label, action.icon, action.enabled))
            .collect::<Vec<_>>(),
        vec![
            ("Change Log", "history", false),
            ("Theme", "dark_mode", false),
            ("Shutdown", "power_settings_new", false),
            ("Logout", "logout", true),
        ]
    );
}
