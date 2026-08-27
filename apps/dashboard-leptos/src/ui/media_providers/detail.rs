use crate::dashboard::{MediaProviderDetailState, media_provider_detail_state};
use leptos::prelude::*;

use super::shared::{
    ActionList, ConfigRows, MediaProviderStyles, PlaceholderNotice, invalid_media_route,
    listing_href,
};

#[component]
pub(crate) fn MediaProviderDetailPanel(
    provider_kind: String,
    provider_id: String,
) -> impl IntoView {
    let provider_kind = provider_kind.into_boxed_str();
    let provider_id = provider_id.into_boxed_str();

    media_provider_detail_state(&provider_kind, &provider_id).map_or_else(
        || invalid_media_route("Invalid media provider detail route"),
        |state| {
            view! {
                <MediaProviderStyles />
                <MediaProviderDetailView state />
            }
            .into_any()
        },
    )
}

#[component]
fn MediaProviderDetailView(state: MediaProviderDetailState) -> impl IntoView {
    let kind = state.kind;
    let provider = state.provider;
    let config_rows = state.config_rows;
    let connection_actions = state.connection_actions;
    let test_actions = state.test_actions;
    let placeholder = state.placeholder;
    let preview_notice = state.preview_notice;
    let provider_name = provider.name.clone();
    let back_href = listing_href(kind.id.as_str());
    let accent_style = format!("--provider-accent: {}", provider.color);

    view! {
        <div class="nr-panel-stack" style=accent_style>
            <article class="nr-card nr-card-hero">
                <div class="nr-provider-detail-head">
                    <span class="nr-media-provider-mark">{provider.text_icon}</span>
                    <span>
                        <p class="nr-eyebrow">{kind.label}</p>
                        <h2>{provider_name}</h2>
                        <p>{preview_notice}</p>
                    </span>
                </div>
                <div class="nr-media-card-actions">
                    <a class="nr-button secondary small" href=back_href>"Back"</a>
                    <button type="button" class="nr-button primary small" disabled>"Add Connection (Preview)"</button>
                </div>
            </article>
            {placeholder.map(|placeholder| view! { <PlaceholderNotice placeholder /> })}
            <div class="nr-media-detail-grid">
                <article class="nr-card">
                    <div class="nr-card-head between">
                        <div>
                            <h2><span class="nr-card-icon">"key"</span>"Connections"</h2>
                            <p>"Upstream connection controls are represented as disabled preview actions."</p>
                        </div>
                        <span class="nr-status-pill is-idle"><span></span>"No host feed"</span>
                    </div>
                    <ActionList actions=connection_actions />
                </article>
                <article class="nr-card">
                    <div class="nr-card-head between">
                        <div>
                            <h2><span class="nr-card-icon">"cfg"</span>"Provider Config"</h2>
                            <p>"Config rows mirror upstream detail content without saving browser-side state."</p>
                        </div>
                        <span class="nr-status-pill is-degraded"><span></span>"Preview only"</span>
                    </div>
                    <ConfigRows rows=config_rows />
                </article>
            </div>
            <article class="nr-card">
                <div class="nr-card-head between">
                    <div>
                        <h2><span class="nr-card-icon">"run"</span>"Test Example"</h2>
                        <p>"Example execution is disabled until provider test APIs are connected."</p>
                    </div>
                    <button type="button" class="nr-button secondary small" disabled>"Run"</button>
                </div>
                <ActionList actions=test_actions />
            </article>
        </div>
    }
}
