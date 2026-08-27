use serde::Serialize;

use crate::{
    provider_inventory::{DASHBOARD_MODELS, OPENAI_MODELS, PROVIDERS},
    route_inventory::ROUTE_FAMILIES,
};

#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct RouteCatalog {
    pub(crate) families: &'static [RouteFamily],
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RouteFamily {
    pub(crate) id: &'static str,
    pub(crate) title: &'static str,
    pub(crate) upstream: &'static str,
    pub(crate) gateway_prefix: &'static str,
    pub(crate) source_prefix: &'static str,
    pub(crate) routes: &'static [&'static str],
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProviderCatalog {
    pub(crate) providers: &'static [Provider],
    pub(crate) models: &'static [DashboardModel],
    pub(crate) openai_models: OpenAiModels,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Provider {
    pub(crate) id: &'static str,
    pub(crate) name: &'static str,
    pub(crate) description: &'static str,
    pub(crate) auth_label: &'static str,
    pub(crate) accent: &'static str,
    pub(crate) status: ProviderStatus,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct ProviderStatus {
    pub(crate) connected: u8,
    pub(crate) error: u8,
    pub(crate) total: u8,
    pub(crate) health: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DashboardModel {
    pub(crate) id: &'static str,
    pub(crate) provider: &'static str,
    pub(crate) model: &'static str,
    pub(crate) full_model: &'static str,
    pub(crate) alias: &'static str,
    pub(crate) caps: ModelCapabilities,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct ModelCapabilities {
    pub(crate) vision: bool,
    pub(crate) search: bool,
    pub(crate) reasoning: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct OpenAiModels {
    pub(crate) object: &'static str,
    pub(crate) data: &'static [OpenAiModel],
}

#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct OpenAiModel {
    pub(crate) id: &'static str,
    pub(crate) object: &'static str,
    #[serde(rename = "owned_by")]
    pub(crate) owned_by: &'static str,
}

pub(crate) const fn route_catalog() -> RouteCatalog {
    RouteCatalog {
        families: &ROUTE_FAMILIES,
    }
}

pub(crate) const fn provider_catalog() -> ProviderCatalog {
    ProviderCatalog {
        providers: &PROVIDERS,
        models: &DASHBOARD_MODELS,
        openai_models: OpenAiModels {
            object: "list",
            data: &OPENAI_MODELS,
        },
    }
}
