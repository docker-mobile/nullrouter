use serde::Serialize;

mod catalog;

use catalog::ALL_SECTIONS;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DashboardSection {
    Endpoint,
    BasicChat,
    Providers,
    MediaProvidersWeb,
    ProxyPools,
    Translator,
    Usage,
    Status,
    Settings,
    SettingsPricing,
    Combos,
    QuotaTracker,
    TokenSaver,
    ConsoleLog,
    CliTools,
    Skills,
    Profile,
    Mitm,
    /// 9Router import surface. nullrouter-specific: upstream is the source.
    Migrate,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum DashboardRoute {
    Section {
        section: DashboardSection,
    },
    ProviderNew,
    ProviderDetail {
        #[serde(rename = "providerId")]
        provider_id: String,
    },
    MediaProviderKind {
        #[serde(rename = "providerKind")]
        provider_kind: String,
    },
    MediaProviderDetail {
        #[serde(rename = "providerKind")]
        provider_kind: String,
        #[serde(rename = "providerId")]
        provider_id: String,
    },
    MediaProviderCombo {
        #[serde(rename = "comboId")]
        combo_id: String,
    },
    CliToolDetail {
        #[serde(rename = "toolId")]
        tool_id: String,
    },
}

pub const fn dashboard_sections() -> &'static [DashboardSection] {
    &ALL_SECTIONS
}

impl DashboardRoute {
    pub const fn for_section(section: DashboardSection) -> Self {
        Self::Section { section }
    }

    pub fn provider_detail(provider_id: impl Into<String>) -> Self {
        Self::ProviderDetail {
            provider_id: provider_id.into(),
        }
    }

    pub fn cli_tool_detail(tool_id: impl Into<String>) -> Self {
        Self::CliToolDetail {
            tool_id: tool_id.into(),
        }
    }

    pub fn media_provider_kind(provider_kind: impl Into<String>) -> Self {
        Self::MediaProviderKind {
            provider_kind: provider_kind.into(),
        }
    }

    pub fn media_provider_detail(
        provider_kind: impl Into<String>,
        provider_id: impl Into<String>,
    ) -> Self {
        Self::MediaProviderDetail {
            provider_kind: provider_kind.into(),
            provider_id: provider_id.into(),
        }
    }

    pub fn media_provider_combo(combo_id: impl Into<String>) -> Self {
        Self::MediaProviderCombo {
            combo_id: combo_id.into(),
        }
    }

    pub fn from_hash(hash: &str) -> Self {
        let normalized = hash.trim_start_matches('#').trim_matches('/');
        if normalized == "providers/new" {
            return Self::ProviderNew;
        }
        if let Some(provider_id) = normalized.strip_prefix("providers/") {
            return Self::from_provider_segment(provider_id);
        }
        if let Some(media_path) = normalized.strip_prefix("media-providers/") {
            return Self::from_media_provider_segments(media_path);
        }
        if let Some(tool_id) = normalized.strip_prefix("cli-tools/") {
            return Self::from_cli_tool_segment(tool_id);
        }
        Self::for_section(DashboardSection::from_hash(normalized))
    }

    pub fn from_path(path: &str) -> Self {
        let normalized = path.trim_end_matches('/');
        if normalized == "/dashboard/providers/new" {
            return Self::ProviderNew;
        }
        if let Some(provider_id) = normalized.strip_prefix("/dashboard/providers/") {
            return Self::from_provider_segment(provider_id);
        }
        if let Some(media_path) = normalized.strip_prefix("/dashboard/media-providers/") {
            return Self::from_media_provider_segments(media_path);
        }
        if let Some(tool_id) = normalized.strip_prefix("/dashboard/cli-tools/") {
            return Self::from_cli_tool_segment(tool_id);
        }
        Self::for_section(DashboardSection::from_path(normalized))
    }

    pub const fn section(&self) -> DashboardSection {
        match self {
            Self::Section { section } => *section,
            Self::ProviderNew | Self::ProviderDetail { .. } => DashboardSection::Providers,
            Self::MediaProviderKind { .. }
            | Self::MediaProviderDetail { .. }
            | Self::MediaProviderCombo { .. } => DashboardSection::MediaProvidersWeb,
            Self::CliToolDetail { .. } => DashboardSection::CliTools,
        }
    }

    fn from_provider_segment(provider_id: &str) -> Self {
        single_segment(provider_id).map_or_else(
            || Self::for_section(DashboardSection::Providers),
            Self::provider_detail,
        )
    }

    fn from_cli_tool_segment(tool_id: &str) -> Self {
        single_segment(tool_id).map_or_else(
            || Self::for_section(DashboardSection::CliTools),
            Self::cli_tool_detail,
        )
    }

    fn from_media_provider_segments(media_path: &str) -> Self {
        let normalized = media_path.trim_matches('/');
        match normalized {
            "web" | "webSearch" | "webFetch" => {
                return Self::for_section(DashboardSection::MediaProvidersWeb);
            }
            "" => return Self::for_section(DashboardSection::MediaProvidersWeb),
            _ => {}
        }

        let mut parts = normalized.split('/');
        let first = parts.next();
        let second = parts.next();
        let third = parts.next();

        match (first, second, third) {
            (Some("combo"), Some(combo_id), None) => single_segment(combo_id).map_or_else(
                || Self::for_section(DashboardSection::MediaProvidersWeb),
                Self::media_provider_combo,
            ),
            (Some(kind), None, None) => single_segment(kind).map_or_else(
                || Self::for_section(DashboardSection::MediaProvidersWeb),
                Self::media_provider_kind,
            ),
            (Some(kind), Some(provider_id), None) => {
                match (single_segment(kind), single_segment(provider_id)) {
                    (Some(kind), Some(provider_id)) => {
                        Self::media_provider_detail(kind, provider_id)
                    }
                    _ => Self::for_section(DashboardSection::MediaProvidersWeb),
                }
            }
            _ => Self::for_section(DashboardSection::MediaProvidersWeb),
        }
    }
}

fn single_segment(value: &str) -> Option<&str> {
    if value.is_empty() || value.contains('/') {
        None
    } else {
        Some(value)
    }
}
