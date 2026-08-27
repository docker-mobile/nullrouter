mod actions;
mod builders;
mod examples;
mod placeholders;
mod registry;
mod types;

use actions::{COMBO_ACTIONS, DETAIL_CONNECTION_ACTIONS, DETAIL_TEST_ACTIONS, KIND_ACTIONS};
use builders::{
    combo_members, combo_preview, detail_config_rows, known_media_kind, media_kind_config,
    media_provider_tile, providers_for_kind,
};
use examples::{curl_preview, example_body, example_path};
use placeholders::{combo_placeholder, detail_placeholder, unknown_kind_placeholder};
use registry::combo_definitions;
pub use types::{
    MediaProviderAction, MediaProviderComboDetailState, MediaProviderComboMember,
    MediaProviderComboPreview, MediaProviderConfigRow, MediaProviderDetailState,
    MediaProviderKindConfig, MediaProviderKindState, MediaProviderPlaceholder, MediaProviderTile,
};

pub fn media_provider_kind_state(kind_id: &str) -> Option<MediaProviderKindState> {
    let kind_id = valid_segment(kind_id)?;
    let kind = media_kind_config(kind_id);
    let known = known_media_kind(kind_id).is_some();
    let providers = if known {
        providers_for_kind(kind_id)
    } else {
        Vec::new()
    };
    let combos = combo_definitions()
        .iter()
        .filter(|combo| combo.kind_id == kind_id)
        .map(combo_preview)
        .collect::<Vec<_>>();

    Some(MediaProviderKindState {
        route_path: format!("/dashboard/media-providers/{kind_id}"),
        kind,
        providers,
        combos,
        actions: &KIND_ACTIONS,
        provider_mutations_wired: false,
        combo_mutations_wired: false,
        preview_notice: "Provider toggles, combo creation, and custom embedding mutation are disabled until host APIs are connected.",
        placeholder: unknown_kind_placeholder(kind_id, known),
    })
}

pub fn media_provider_detail_state(
    kind_id: &str,
    provider_id: &str,
) -> Option<MediaProviderDetailState> {
    let kind_id = valid_segment(kind_id)?;
    let provider_id = valid_segment(provider_id)?;
    let kind = media_kind_config(kind_id);
    let known_kind = known_media_kind(kind_id).is_some();
    let provider = media_provider_tile(provider_id);
    let supported = known_kind && provider.service_kinds.contains(&kind_id);

    Some(MediaProviderDetailState {
        route_path: format!("/dashboard/media-providers/{kind_id}/{provider_id}"),
        config_rows: detail_config_rows(&kind, &provider),
        kind,
        provider,
        connection_actions: &DETAIL_CONNECTION_ACTIONS,
        test_actions: &DETAIL_TEST_ACTIONS,
        connection_writes_wired: false,
        test_execution_wired: false,
        model_settings_wired: false,
        preview_notice: "Connections, model settings, provider toggles, and example execution are preview-only in the WASM dashboard.",
        placeholder: detail_placeholder(kind_id, provider_id, known_kind, supported),
    })
}

pub fn media_provider_combo_detail_state(combo_id: &str) -> Option<MediaProviderComboDetailState> {
    let combo_id = valid_segment(combo_id)?;
    let combo = combo_definitions()
        .iter()
        .find(|candidate| candidate.id == combo_id);
    let kind_id = combo.map_or("unknown", |definition| definition.kind_id);
    let name = combo.map_or_else(
        || combo_id.to_owned(),
        |definition| definition.name.to_owned(),
    );
    let members = combo.map_or_else(Vec::new, |definition| combo_members(definition.models));
    let example_path = example_path(kind_id);
    let example_body = example_body(kind_id);

    Some(MediaProviderComboDetailState {
        route_path: format!("/dashboard/media-providers/combo/{combo_id}"),
        combo_id: combo_id.to_owned(),
        name: name.clone(),
        kind: media_kind_config(kind_id),
        members,
        round_robin: false,
        actions: &COMBO_ACTIONS,
        persistence_wired: false,
        test_execution_wired: false,
        example_path,
        example_body,
        curl_preview: curl_preview(example_path, example_body, &name),
        usage_log_status: "No usage yet.",
        placeholder: combo_placeholder(combo_id, combo.is_some()),
    })
}

fn valid_segment(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.contains('/') {
        None
    } else {
        Some(trimmed)
    }
}
