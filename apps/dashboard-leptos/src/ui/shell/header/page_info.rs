use crate::{
    dashboard::{
        cli_tool_detail_state, media_provider_combo_detail_state, media_provider_detail_state,
        media_provider_kind_state, provider_detail_state,
    },
    ui::{DashboardRoute, DashboardSection},
};

pub(super) struct PageInfo {
    pub(super) title: String,
    pub(super) description: String,
    pub(super) icon: &'static str,
}

impl PageInfo {
    fn section(section: DashboardSection) -> Self {
        Self {
            title: section.title().to_owned(),
            description: section.description().to_owned(),
            icon: section.icon(),
        }
    }
}

pub(super) fn page_info(route: &DashboardRoute) -> PageInfo {
    match route {
        DashboardRoute::Section { section } => PageInfo::section(*section),
        DashboardRoute::ProviderNew => PageInfo {
            title: "Add New Provider".to_owned(),
            description: "Configure a new AI provider to use with your applications.".to_owned(),
            icon: "dns",
        },
        DashboardRoute::ProviderDetail { provider_id } => provider_detail_state(provider_id)
            .map_or_else(
                || PageInfo {
                    title: provider_id.clone(),
                    description: String::new(),
                    icon: "dns",
                },
                |state| PageInfo {
                    title: state.provider.name,
                    description: String::new(),
                    icon: "dns",
                },
            ),
        DashboardRoute::MediaProviderKind { provider_kind } => {
            media_provider_kind_state(provider_kind).map_or_else(
                || PageInfo {
                    title: provider_kind.clone(),
                    description: String::new(),
                    icon: "perm_media",
                },
                |state| PageInfo {
                    title: state.kind.label.to_owned(),
                    description: format!("Manage your {} providers", state.kind.label),
                    icon: state.kind.icon,
                },
            )
        }
        DashboardRoute::MediaProviderDetail {
            provider_kind,
            provider_id,
        } => media_provider_detail_state(provider_kind, provider_id).map_or_else(
            || PageInfo {
                title: provider_id.clone(),
                description: String::new(),
                icon: "perm_media",
            },
            |state| PageInfo {
                title: state.provider.name,
                description: String::new(),
                icon: state.kind.icon,
            },
        ),
        DashboardRoute::MediaProviderCombo { combo_id } => {
            media_provider_combo_detail_state(combo_id).map_or_else(
                || PageInfo {
                    title: combo_id.clone(),
                    description: String::new(),
                    icon: "hub",
                },
                |state| PageInfo {
                    title: state.name,
                    description: String::new(),
                    icon: state.kind.icon,
                },
            )
        }
        DashboardRoute::CliToolDetail { tool_id } => cli_tool_detail_state(tool_id).map_or_else(
            || PageInfo {
                title: tool_id.clone(),
                description: "Configure CLI tools".to_owned(),
                icon: "terminal",
            },
            |state| PageInfo {
                title: state.tool.name.to_owned(),
                description: "Configure CLI tools".to_owned(),
                icon: "terminal",
            },
        ),
    }
}
