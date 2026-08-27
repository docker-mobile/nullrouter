use crate::dashboard::{MediaProviderComboDetailState, media_provider_combo_detail_state};
use leptos::prelude::*;

use super::shared::{
    ActionList, MediaProviderStyles, MemberRows, PlaceholderNotice, invalid_media_route,
    listing_href,
};

#[component]
pub(crate) fn MediaProviderComboPanel(combo_id: String) -> impl IntoView {
    let combo_id = combo_id.into_boxed_str();

    media_provider_combo_detail_state(&combo_id).map_or_else(
        || invalid_media_route("Invalid media provider combo route"),
        |state| {
            view! {
                <MediaProviderStyles />
                <MediaProviderComboView state />
            }
            .into_any()
        },
    )
}

#[component]
fn MediaProviderComboView(state: MediaProviderComboDetailState) -> impl IntoView {
    let kind = state.kind;
    let members = state.members;
    let actions = state.actions;
    let placeholder = state.placeholder;
    let back_href = listing_href(kind.id.as_str());
    let curl_preview = state.curl_preview;
    let usage_log_status = state.usage_log_status;

    view! {
        <div class="nr-panel-stack">
            <article class="nr-card nr-card-hero">
                <div>
                    <p class="nr-eyebrow">{kind.label} " Combo"</p>
                    <h2 class="nr-media-mono">{state.name}</h2>
                    <p>"Provider order, round robin, test execution, and deletion are preview-only in this WASM slice."</p>
                </div>
                <div class="nr-media-card-actions">
                    <a class="nr-button secondary small" href=back_href>"Back"</a>
                    <button type="button" class="nr-button primary small" disabled>"Delete (Preview)"</button>
                </div>
            </article>
            {placeholder.map(|placeholder| view! { <PlaceholderNotice placeholder /> })}
            <div class="nr-media-detail-grid">
                <article class="nr-card">
                    <div class="nr-card-head between">
                        <div>
                            <h2><span class="nr-card-icon">"set"</span>"Settings"</h2>
                            <p>"Round Robin is visible but disabled until settings persistence is connected."</p>
                        </div>
                        <span class="nr-status-pill is-idle"><span></span>"Persistence off"</span>
                    </div>
                    <ActionList actions />
                </article>
                <article class="nr-card">
                    <div class="nr-card-head between">
                        <div>
                            <h2><span class="nr-card-icon">"lst"</span>"Providers"</h2>
                            <p>"Tried in order from top to bottom when this combo is persisted upstream."</p>
                        </div>
                        <button type="button" class="nr-button secondary small" disabled>"Add Provider"</button>
                    </div>
                    <MemberRows members />
                </article>
            </div>
            <article class="nr-card">
                <div class="nr-card-head between">
                    <div>
                        <h2><span class="nr-card-icon">"run"</span>"Test Example"</h2>
                        <p>"The request body is a preview of the upstream test action."</p>
                    </div>
                    <button type="button" class="nr-button secondary small" disabled>"Run"</button>
                </div>
                <pre class="nr-media-mono nr-empty-state">{curl_preview}</pre>
            </article>
            <article class="nr-card">
                <div class="nr-card-head">
                    <div>
                        <h2><span class="nr-card-icon">"log"</span>"Usage Logs"</h2>
                        <p>{usage_log_status}</p>
                    </div>
                </div>
            </article>
        </div>
    }
}
