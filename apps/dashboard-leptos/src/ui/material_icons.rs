pub(super) const ARROW_FORWARD: &str = "\u{e5c8}";
pub(super) const CANCEL: &str = "\u{e888}";
pub(super) const EXPAND_MORE: &str = "\u{e5cf}";
pub(super) const PLAY_CIRCLE: &str = "\u{e1c4}";
pub(super) const SECURITY: &str = "\u{e32a}";
pub(super) const WARNING: &str = "\u{f083}";

pub fn dashboard_icon_glyph(name: &'static str) -> &'static str {
    match name {
        "api" => "\u{f1b7}",
        "bar_chart" => "\u{e26b}",
        "brush" => "\u{e3ae}",
        "chat" => "\u{e0b7}",
        "check" => "\u{e5ca}",
        "close" => "\u{e5cd}",
        "computer" => "\u{e30a}",
        "content_copy" => "\u{e14d}",
        "dark_mode" => "\u{e51c}",
        "data_array" => "\u{ead1}",
        "data_usage" => "\u{e1af}",
        "dns" => "\u{e875}",
        "expand_more" => EXPAND_MORE,
        "extension" => "\u{e87b}",
        "grid_view" => "\u{e9b0}",
        "history" => "\u{e889}",
        "hub" => "\u{e9f4}",
        "lan" => "\u{eb2f}",
        "language" => "\u{e894}",
        "layers" => "\u{e53b}",
        "logout" => "\u{e9ba}",
        "menu" => "\u{e5d2}",
        "mic" => "\u{e029}",
        "monitor" => "\u{ef5b}",
        "monitoring" => "\u{f190}",
        "payments" => "\u{ef63}",
        "perm_media" => "\u{e8a7}",
        "power_settings_new" => "\u{e8ac}",
        "record_voice_over" => "\u{e91f}",
        "savings" => "\u{e2eb}",
        "search" => "\u{e8b6}",
        "security" => SECURITY,
        "settings" => "\u{e8b8}",
        "terminal" => "\u{eb8e}",
        "translate" => "\u{e8e2}",
        "travel_explore" => "\u{e2db}",
        _ => name,
    }
}
