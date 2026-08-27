use crate::dashboard::{
    MediaProviderComboPreview, MediaProviderKindState, MediaProviderTile, media_provider_kind_state,
};
use leptos::prelude::*;

use super::shared::{
    ActionList, MediaProviderStyles, PlaceholderNotice, endpoint_label, invalid_media_route,
    state_icon,
};

#[component]
pub(crate) fn MediaProviderKindPanel(provider_kind: String) -> impl IntoView {
    let provider_kind = provider_kind.into_boxed_str();

    media_provider_kind_state(&provider_kind).map_or_else(
        || invalid_media_route("Invalid media provider kind"),
        |state| {
            view! {
                <MediaProviderStyles />
                <MediaProviderKindView state />
            }
            .into_any()
        },
    )
}

#[component]
fn MediaProviderKindView(state: MediaProviderKindState) -> impl IntoView {
    let endpoint = endpoint_label(state.kind.endpoint_method, state.kind.endpoint_path);
    let providers = state.providers;
    let combos = state.combos;
    let actions = state.actions;
    let placeholder = state.placeholder;
    let kind_id = state.kind.id;
    let kind_label = state.kind.label;
    let preview_notice = state.preview_notice;

    view! {
        <div class="nr-panel-stack">
            <article class="nr-card nr-card-hero">
                <div>
                    <p class="nr-eyebrow">"Media Providers"</p>
                    <h2>{kind_label}</h2>
                    <p>{endpoint}</p>
                </div>
                <span class="nr-status-pill is-idle"><span></span>"Preview"</span>
            </article>
            {placeholder.map(|placeholder| view! { <PlaceholderNotice placeholder /> })}
            <article class="nr-card">
                <div class="nr-card-head between">
                    <div>
                        <h2><span class="nr-card-icon">{state_icon(kind_id.as_str())}</span>"Provider Actions"</h2>
                        <p>{preview_notice}</p>
                    </div>
                    <a class="nr-button secondary small" href="/dashboard/media-providers/web">"Web Providers"</a>
                </div>
                <ActionList actions />
            </article>
            <MediaCombos combos />
            <article class="nr-card">
                <div class="nr-card-head between">
                    <div>
                        <h2><span class="nr-card-icon">"prv"</span>"Providers"</h2>
                        <p>"Connection counts stay at default until /api/providers is wired into this WASM panel."</p>
                    </div>
                    <span class="nr-status-pill is-idle"><span></span>"No connections"</span>
                </div>
                <div class="nr-media-provider-grid">
                    <For
                        each=move || providers.clone()
                        key=|provider| provider.id.clone()
                        children=move |provider| {
                            view! { <MediaProviderTileView provider kind_id=kind_id.clone() /> }
                        }
                    />
                </div>
            </article>
        </div>
    }
}

#[component]
fn MediaProviderTileView(provider: MediaProviderTile, kind_id: String) -> impl IntoView {
    let href = format!("/dashboard/media-providers/{kind_id}/{}", provider.id);
    let health = provider.status.health;
    let accent_style = format!("--provider-accent: {}", provider.color);
    let status_label = if provider.no_auth {
        "Ready"
    } else {
        health.label()
    };

    view! {
        <article class="nr-media-tile" style=accent_style>
            <div class="nr-media-tile-head">
                <span class="nr-media-provider-mark">{provider.text_icon}</span>
                <span class=format!("nr-status-pill {}", health.class_name())>
                    <span></span>{status_label}
                </span>
            </div>
            <h3>{provider.name}</h3>
            <p>{provider.description}</p>
            <div class="nr-media-card-actions">
                <a class="nr-button secondary small" href=href>"Open Preview"</a>
                <button type="button" class="nr-button secondary small" disabled>"Toggle"</button>
            </div>
        </article>
    }
}

#[component]
fn MediaCombos(combos: Vec<MediaProviderComboPreview>) -> impl IntoView {
    view! {
        <article class="nr-card">
            <div class="nr-card-head between">
                <div>
                    <h2><span class="nr-card-icon">"lay"</span>"Combos"</h2>
                    <p>"Combo rows are preview fixtures and are not persisted from this dashboard."</p>
                </div>
                <span class="nr-status-pill is-idle"><span></span>"Preview"</span>
            </div>
            <div class="nr-media-combo-list">
                <For
                    each=move || combos.clone()
                    key=|combo| combo.id.clone()
                    children=|combo| view! { <MediaComboRow combo /> }
                />
            </div>
        </article>
    }
}

#[component]
fn MediaComboRow(combo: MediaProviderComboPreview) -> impl IntoView {
    let href = format!("/dashboard/media-providers/combo/{}", combo.id);
    let member_count = combo.members.len().to_string();

    view! {
        <a class="nr-media-combo-row" href=href>
            <span>
                <strong class="nr-media-mono">{combo.name}</strong>
                <span>{combo.routing}</span>
            </span>
            <span>{member_count} " providers"</span>
        </a>
    }
}
