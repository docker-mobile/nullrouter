use crate::dashboard::{ProviderHealth, ProviderStatus};

use super::registry::{
    MEDIA_PROVIDER_KINDS, MediaKindDefinition, MediaProviderComboDefinition,
    MediaProviderDefinition, provider_definitions,
};
use super::types::{
    MediaProviderComboMember, MediaProviderComboPreview, MediaProviderConfigRow,
    MediaProviderKindConfig, MediaProviderTile,
};

pub(super) fn providers_for_kind(kind_id: &str) -> Vec<MediaProviderTile> {
    provider_definitions()
        .iter()
        .filter(|provider| provider.service_kinds.contains(&kind_id))
        .map(provider_tile)
        .collect()
}

pub(super) fn media_provider_tile(provider_id: &str) -> MediaProviderTile {
    provider_definitions()
        .iter()
        .find(|provider| provider.id == provider_id)
        .map_or_else(|| unknown_provider_tile(provider_id), provider_tile)
}

pub(super) fn detail_config_rows(
    kind: &MediaProviderKindConfig,
    provider: &MediaProviderTile,
) -> Vec<MediaProviderConfigRow> {
    vec![
        MediaProviderConfigRow {
            label: "Endpoint",
            value: format!("{} {}", kind.endpoint_method, kind.endpoint_path),
        },
        MediaProviderConfigRow {
            label: "Provider",
            value: provider.name.clone(),
        },
        MediaProviderConfigRow {
            label: "Models",
            value: model_settings_label(kind.id.as_str()).to_owned(),
        },
    ]
}

pub(super) fn combo_preview(combo: &MediaProviderComboDefinition) -> MediaProviderComboPreview {
    MediaProviderComboPreview {
        id: combo.id.to_owned(),
        name: combo.name,
        kind_id: combo.kind_id,
        members: combo_members(combo.models),
        routing: combo.routing,
        persisted: false,
    }
}

pub(super) fn combo_members(models: &[&'static str]) -> Vec<MediaProviderComboMember> {
    models.iter().map(|entry| combo_member(entry)).collect()
}

pub(super) fn media_kind_config(kind_id: &str) -> MediaProviderKindConfig {
    known_media_kind(kind_id).map_or_else(
        || MediaProviderKindConfig {
            id: kind_id.to_owned(),
            label: "Unknown Media Provider",
            icon: "help",
            endpoint_method: "POST",
            endpoint_path: "",
        },
        |kind| MediaProviderKindConfig {
            id: kind.id.to_owned(),
            label: kind.label,
            icon: kind.icon,
            endpoint_method: kind.endpoint_method,
            endpoint_path: kind.endpoint_path,
        },
    )
}

pub(super) fn known_media_kind(kind_id: &str) -> Option<&'static MediaKindDefinition> {
    MEDIA_PROVIDER_KINDS
        .iter()
        .find(|candidate| candidate.id == kind_id)
}

fn provider_tile(provider: &MediaProviderDefinition) -> MediaProviderTile {
    MediaProviderTile {
        id: provider.id.to_owned(),
        name: provider.name.to_owned(),
        description: provider.description,
        color: provider.color,
        text_icon: provider.text_icon,
        service_kinds: provider.service_kinds,
        no_auth: provider.no_auth,
        custom: false,
        status: idle_status(),
    }
}

fn unknown_provider_tile(provider_id: &str) -> MediaProviderTile {
    MediaProviderTile {
        id: provider_id.to_owned(),
        name: "Unknown Provider".to_owned(),
        description: "This provider id is not present in the local media provider fixture.",
        color: "#9ca3af",
        text_icon: "??",
        service_kinds: &[],
        no_auth: false,
        custom: false,
        status: idle_status(),
    }
}

fn combo_member(entry: &str) -> MediaProviderComboMember {
    let (provider_id, model) = entry
        .split_once('/')
        .map_or((entry, ""), |(provider_id, model)| (provider_id, model));
    let provider = media_provider_tile(provider_id);

    MediaProviderComboMember {
        entry: entry.to_owned(),
        provider_id: provider_id.to_owned(),
        provider_name: provider.name,
        model: model.to_owned(),
    }
}

const fn idle_status() -> ProviderStatus {
    ProviderStatus {
        connected: 0,
        error: 0,
        total: 0,
        health: ProviderHealth::Idle,
    }
}

fn model_settings_label(kind_id: &str) -> &'static str {
    match kind_id {
        "tts" | "webSearch" | "webFetch" => "Provider is the model",
        "unknown" => "Unavailable",
        _ => "Preview catalog",
    }
}
