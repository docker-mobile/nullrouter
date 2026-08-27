use super::DashboardSection;

const PRIMARY_NAVIGATION: [DashboardSection; 7] = [
    DashboardSection::Endpoint,
    DashboardSection::Providers,
    DashboardSection::Combos,
    DashboardSection::Usage,
    DashboardSection::QuotaTracker,
    DashboardSection::TokenSaver,
    DashboardSection::CliTools,
];

const SYSTEM_NAVIGATION: [DashboardSection; 8] = [
    DashboardSection::MediaProvidersWeb,
    DashboardSection::ProxyPools,
    DashboardSection::Skills,
    DashboardSection::Mitm,
    DashboardSection::ConsoleLog,
    DashboardSection::Translator,
    DashboardSection::Migrate,
    DashboardSection::Profile,
];

const MEDIA_NAVIGATION: [MediaNavigationItem; 5] = [
    MediaNavigationItem {
        id: "embedding",
        label: "Embedding",
        icon: "data_array",
        path: "/dashboard/media-providers/embedding",
    },
    MediaNavigationItem {
        id: "image",
        label: "Text to Image",
        icon: "brush",
        path: "/dashboard/media-providers/image",
    },
    MediaNavigationItem {
        id: "tts",
        label: "Text To Speech",
        icon: "record_voice_over",
        path: "/dashboard/media-providers/tts",
    },
    MediaNavigationItem {
        id: "stt",
        label: "Speech To Text",
        icon: "mic",
        path: "/dashboard/media-providers/stt",
    },
    MediaNavigationItem {
        id: "web",
        label: "Web Fetch & Search",
        icon: "travel_explore",
        path: "/dashboard/media-providers/web",
    },
];

const HEADER_CONTROLS: [HeaderControl; 3] = [
    HeaderControl {
        id: "search",
        label: "Search",
        icon: "search",
    },
    HeaderControl {
        id: "language",
        label: "Language",
        icon: "language",
    },
    HeaderControl {
        id: "account",
        label: "Menu",
        icon: "grid_view",
    },
];

const ACCOUNT_ACTIONS: [AccountAction; 4] = [
    AccountAction {
        label: "Change Log",
        icon: "history",
        enabled: false,
        kind: AccountActionKind::Unsupported,
    },
    AccountAction {
        label: "Theme",
        icon: "dark_mode",
        enabled: false,
        kind: AccountActionKind::Unsupported,
    },
    AccountAction {
        label: "Shutdown",
        icon: "power_settings_new",
        enabled: false,
        kind: AccountActionKind::Unsupported,
    },
    AccountAction {
        label: "Logout",
        icon: "logout",
        enabled: true,
        kind: AccountActionKind::Logout,
    },
];

const SEARCH_DESTINATIONS: [SearchDestination; 19] = [
    SearchDestination::new("Endpoint & Key", "/dashboard/endpoint", "api"),
    SearchDestination::new("Providers", "/dashboard/providers", "dns"),
    SearchDestination::new("Combos", "/dashboard/combos", "layers"),
    SearchDestination::new("Usage", "/dashboard/usage", "bar_chart"),
    SearchDestination::new("Quota Tracker", "/dashboard/quota", "data_usage"),
    SearchDestination::new("Token Saver", "/dashboard/token-saver", "savings"),
    SearchDestination::new("CLI Tools", "/dashboard/cli-tools", "terminal"),
    SearchDestination::new(
        "Embedding",
        "/dashboard/media-providers/embedding",
        "data_array",
    ),
    SearchDestination::new("Text to Image", "/dashboard/media-providers/image", "brush"),
    SearchDestination::new(
        "Text To Speech",
        "/dashboard/media-providers/tts",
        "record_voice_over",
    ),
    SearchDestination::new("Speech To Text", "/dashboard/media-providers/stt", "mic"),
    SearchDestination::new(
        "Web Fetch & Search",
        "/dashboard/media-providers/web",
        "travel_explore",
    ),
    SearchDestination::new("Proxy Pools", "/dashboard/proxy-pools", "lan"),
    SearchDestination::new("Skills", "/dashboard/skills", "extension"),
    SearchDestination::new("MITM", "/dashboard/mitm", "security"),
    SearchDestination::new("Console Log", "/dashboard/console-log", "monitor"),
    SearchDestination::new("Translator", "/dashboard/translator", "translate"),
    SearchDestination::new("Settings", "/dashboard/profile", "settings"),
    SearchDestination::new(
        "Pricing Settings",
        "/dashboard/settings/pricing",
        "payments",
    ),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MediaNavigationItem {
    pub id: &'static str,
    pub label: &'static str,
    pub icon: &'static str,
    pub path: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeaderControl {
    pub id: &'static str,
    pub label: &'static str,
    pub icon: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccountAction {
    pub label: &'static str,
    pub icon: &'static str,
    pub enabled: bool,
    pub kind: AccountActionKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountActionKind {
    Unsupported,
    Logout,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SearchDestination {
    pub label: &'static str,
    pub path: &'static str,
    pub icon: &'static str,
}

impl SearchDestination {
    const fn new(label: &'static str, path: &'static str, icon: &'static str) -> Self {
        Self { label, path, icon }
    }
}

pub const fn dashboard_primary_navigation() -> &'static [DashboardSection] {
    &PRIMARY_NAVIGATION
}

pub const fn dashboard_system_navigation() -> &'static [DashboardSection] {
    &SYSTEM_NAVIGATION
}

pub const fn dashboard_media_navigation() -> &'static [MediaNavigationItem] {
    &MEDIA_NAVIGATION
}

pub const fn dashboard_header_controls() -> &'static [HeaderControl] {
    &HEADER_CONTROLS
}

pub const fn dashboard_account_actions() -> &'static [AccountAction] {
    &ACCOUNT_ACTIONS
}

pub const fn dashboard_section_path(section: DashboardSection) -> &'static str {
    match section {
        DashboardSection::Endpoint => "/dashboard/endpoint",
        DashboardSection::BasicChat => "/dashboard/basic-chat",
        DashboardSection::Providers => "/dashboard/providers",
        DashboardSection::MediaProvidersWeb => "/dashboard/media-providers/web",
        DashboardSection::ProxyPools => "/dashboard/proxy-pools",
        DashboardSection::Translator => "/dashboard/translator",
        DashboardSection::Usage => "/dashboard/usage",
        DashboardSection::Status => "/dashboard/status",
        DashboardSection::Settings => "/dashboard/settings",
        DashboardSection::SettingsPricing => "/dashboard/settings/pricing",
        DashboardSection::Combos => "/dashboard/combos",
        DashboardSection::QuotaTracker => "/dashboard/quota",
        DashboardSection::TokenSaver => "/dashboard/token-saver",
        DashboardSection::ConsoleLog => "/dashboard/console-log",
        DashboardSection::CliTools => "/dashboard/cli-tools",
        DashboardSection::Skills => "/dashboard/skills",
        DashboardSection::Profile => "/dashboard/profile",
        DashboardSection::Mitm => "/dashboard/mitm",
        DashboardSection::Migrate => "/dashboard/migrate",
    }
}

pub fn dashboard_search(query: &str) -> Vec<&'static SearchDestination> {
    let normalized = query.trim().to_ascii_lowercase();
    SEARCH_DESTINATIONS
        .iter()
        .filter(|destination| {
            normalized.is_empty() || destination.label.to_ascii_lowercase().contains(&normalized)
        })
        .collect()
}
