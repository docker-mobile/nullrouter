use super::DashboardSection;

impl DashboardSection {
    pub const fn hash(self) -> &'static str {
        match self {
            Self::Endpoint => "endpoint",
            Self::BasicChat => "basic-chat",
            Self::Providers => "providers",
            Self::MediaProvidersWeb => "media-providers-web",
            Self::ProxyPools => "proxy-pools",
            Self::Translator => "translator",
            Self::Usage => "usage",
            Self::Status => "status",
            Self::Settings => "settings",
            Self::SettingsPricing => "settings-pricing",
            Self::Combos => "combos",
            Self::QuotaTracker => "quota",
            Self::TokenSaver => "token-saver",
            Self::ConsoleLog => "console-log",
            Self::CliTools => "cli-tools",
            Self::Skills => "skills",
            Self::Profile => "profile",
            Self::Mitm => "mitm",
            Self::Migrate => "migrate",
        }
    }

    pub fn from_hash(hash: &str) -> Self {
        match hash.trim_start_matches('#') {
            "basic-chat" => Self::BasicChat,
            "providers" => Self::Providers,
            "media-providers-web" => Self::MediaProvidersWeb,
            "proxy-pools" => Self::ProxyPools,
            "translator" => Self::Translator,
            "usage" => Self::Usage,
            "status" => Self::Status,
            "settings" => Self::Settings,
            "settings-pricing" => Self::SettingsPricing,
            "combos" => Self::Combos,
            "quota" => Self::QuotaTracker,
            "token-saver" => Self::TokenSaver,
            "console-log" => Self::ConsoleLog,
            "cli-tools" => Self::CliTools,
            "skills" => Self::Skills,
            "profile" => Self::Profile,
            "mitm" => Self::Mitm,
            "migrate" => Self::Migrate,
            _ => Self::Endpoint,
        }
    }

    pub fn from_path(path: &str) -> Self {
        match path.trim_end_matches('/') {
            "/dashboard/basic-chat" => Self::BasicChat,
            "/dashboard/providers" => Self::Providers,
            "/dashboard/proxy-pools" => Self::ProxyPools,
            "/dashboard/translator" => Self::Translator,
            "/dashboard/usage" => Self::Usage,
            "/dashboard/status" => Self::Status,
            "/dashboard/settings" => Self::Settings,
            "/dashboard/console-log" => Self::ConsoleLog,
            "/dashboard/media-providers/web" => Self::MediaProvidersWeb,
            "/dashboard/settings/pricing" => Self::SettingsPricing,
            "/dashboard/combos" => Self::Combos,
            "/dashboard/quota" => Self::QuotaTracker,
            "/dashboard/token-saver" => Self::TokenSaver,
            "/dashboard/cli-tools" => Self::CliTools,
            "/dashboard/skills" => Self::Skills,
            "/dashboard/profile" => Self::Profile,
            "/dashboard/mitm" => Self::Mitm,
            "/dashboard/migrate" => Self::Migrate,
            _ => Self::Endpoint,
        }
    }

    pub const fn title(self) -> &'static str {
        match self {
            Self::Endpoint => "Endpoint",
            Self::BasicChat => "Basic Chat",
            Self::Providers => "Providers",
            Self::MediaProvidersWeb => "Web Fetch & Search",
            Self::ProxyPools => "Proxy Pools",
            Self::Translator => "Translator",
            Self::Usage => "Usage & Analytics",
            Self::Status => "Status",
            Self::Settings | Self::Profile => "Settings",
            Self::SettingsPricing => "Pricing Settings",
            Self::Combos => "Combos",
            Self::QuotaTracker => "Quota Tracker",
            Self::TokenSaver => "Token Saver",
            Self::ConsoleLog => "Console Log",
            Self::CliTools => "CLI Tools",
            Self::Skills => "Agent Skills",
            Self::Mitm => "MITM Proxy",
            Self::Migrate => "Migrate from 9Router",
        }
    }

    pub const fn nav_label(self) -> &'static str {
        match self {
            Self::Endpoint => "Endpoint & Key",
            Self::BasicChat => "Basic Chat",
            Self::Providers => "Providers",
            Self::MediaProvidersWeb => "Media Providers",
            Self::ProxyPools => "Proxy Pools",
            Self::Translator => "Translator",
            Self::Usage => "Usage",
            Self::Status => "Status",
            Self::Settings | Self::Profile => "Settings",
            Self::SettingsPricing => "Pricing",
            Self::Combos => "Combos",
            Self::QuotaTracker => "Quota Tracker",
            Self::TokenSaver => "Token Saver",
            Self::ConsoleLog => "Console Log",
            Self::CliTools => "CLI Tools",
            Self::Skills => "Skills",
            Self::Mitm => "MITM",
            Self::Migrate => "Migrate",
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::Endpoint => "API endpoint configuration",
            Self::BasicChat => "Chat with configured models",
            Self::Providers => "Manage your AI provider connections",
            Self::MediaProvidersWeb => "Manage your Web Fetch & Search providers",
            Self::ProxyPools => "Manage your proxy pool configurations",
            Self::Translator => "Debug translation flow between formats",
            Self::Usage => "Monitor your API usage, token consumption, and request logs",
            Self::Status => "Gateway health and model availability",
            Self::Settings => "Security and dashboard preferences",
            Self::SettingsPricing => "Configure pricing rates for cost tracking",
            Self::Combos => "Model combos with fallback",
            Self::QuotaTracker => "Track and manage your API quota limits",
            Self::TokenSaver => "Compress prompts and outputs to save tokens",
            Self::ConsoleLog => "Live server console output",
            Self::CliTools => "Configure CLI tools",
            Self::Skills => "Copy a link and paste to your AI to use 9Router — no install needed",
            Self::Profile => "Manage your preferences",
            Self::Mitm => "Intercept CLI tool traffic and route through 9Router",
            Self::Migrate => {
                "Import providers, combos, and settings from an existing 9Router install"
            }
        }
    }

    pub const fn icon(self) -> &'static str {
        match self {
            Self::Endpoint => "api",
            Self::BasicChat => "chat",
            Self::Providers => "dns",
            Self::MediaProvidersWeb => "perm_media",
            Self::ProxyPools => "lan",
            Self::Translator => "translate",
            Self::Usage => "bar_chart",
            Self::Status => "monitoring",
            Self::Settings | Self::Profile => "settings",
            Self::SettingsPricing => "payments",
            Self::Combos => "layers",
            Self::QuotaTracker => "data_usage",
            Self::TokenSaver => "savings",
            Self::ConsoleLog => "monitor",
            Self::CliTools => "terminal",
            Self::Skills => "extension",
            Self::Mitm => "security",
            // `content_copy` = "copy in from another install". Not the ideal
            // glyph (`move_down`/`download` would read better) but the icon font
            // is a pre-built binary subset with no regeneration script, so only
            // glyphs already in `material_icons.rs` can render — anything else
            // falls back to literal ligature text.
            Self::Migrate => "content_copy",
        }
    }
}

pub(super) const ALL_SECTIONS: [DashboardSection; 19] = [
    DashboardSection::Endpoint,
    DashboardSection::BasicChat,
    DashboardSection::Providers,
    DashboardSection::MediaProvidersWeb,
    DashboardSection::ProxyPools,
    DashboardSection::Translator,
    DashboardSection::Usage,
    DashboardSection::Status,
    DashboardSection::Settings,
    DashboardSection::SettingsPricing,
    DashboardSection::Combos,
    DashboardSection::QuotaTracker,
    DashboardSection::TokenSaver,
    DashboardSection::ConsoleLog,
    DashboardSection::CliTools,
    DashboardSection::Skills,
    DashboardSection::Profile,
    DashboardSection::Mitm,
    DashboardSection::Migrate,
];
