use crate::dashboard::{
    MediaProviderAction, MediaProviderComboMember, MediaProviderConfigRow, MediaProviderPlaceholder,
};
use leptos::prelude::*;

const MEDIA_PROVIDER_STYLES: &str = r"
.nr-media-provider-grid,.nr-media-combo-list,.nr-media-action-list,.nr-media-config-list{display:grid;gap:10px}
.nr-media-provider-grid{grid-template-columns:repeat(3,minmax(0,1fr))}
.nr-media-detail-grid{display:grid;grid-template-columns:minmax(0,1fr) minmax(0,1fr);gap:16px}
.nr-media-tile,.nr-media-combo-row,.nr-media-action-row,.nr-media-config-row,.nr-media-member-row{min-width:0;border:1px solid var(--border-dark);border-radius:8px;background:var(--surface-dark-2);padding:12px}
.nr-media-tile{display:grid;gap:10px}
.nr-media-tile-head,.nr-media-combo-row,.nr-media-action-row,.nr-media-member-row{display:flex;align-items:center;justify-content:space-between;gap:10px}
.nr-media-provider-mark{width:36px;height:36px;display:grid;place-items:center;border-radius:8px;border:1px solid color-mix(in srgb,var(--provider-accent) 42%,var(--border-dark));background:color-mix(in srgb,var(--provider-accent) 16%,var(--surface-dark-2));color:var(--provider-accent);font-size:.75rem;font-weight:800}
.nr-media-tile h3,.nr-media-config-row strong,.nr-media-action-row strong,.nr-media-member-row strong{color:var(--text-main-dark)}
.nr-media-tile p,.nr-media-config-row span,.nr-media-action-row span,.nr-media-member-row span,.nr-media-combo-row span{color:var(--text-muted-dark);font-size:.82rem;line-height:1.45}
.nr-media-action-row>span,.nr-media-member-row>span,.nr-media-combo-row>span:first-child{min-width:0;display:grid;gap:3px}
.nr-media-member-row .nr-media-mono,.nr-media-combo-row .nr-media-mono{overflow-wrap:anywhere;word-break:break-word}
.nr-media-card-actions{display:flex;align-items:center;gap:8px;flex-wrap:wrap}
.nr-media-config-row{display:grid;gap:4px}
.nr-media-mono{font-family:monospace;overflow-wrap:anywhere;white-space:pre-wrap;max-width:100%}
.nr-media-placeholder{border-color:color-mix(in srgb,var(--warn) 55%,var(--border-dark));background:color-mix(in srgb,var(--warn) 8%,var(--surface-dark-2))}
@media (max-width:980px){.nr-media-provider-grid,.nr-media-detail-grid{grid-template-columns:1fr}}
@media (max-width:560px){.nr-media-action-row,.nr-media-member-row,.nr-media-combo-row{display:grid;grid-template-columns:1fr;align-items:start}.nr-media-action-row .nr-button,.nr-media-member-row .nr-button{width:100%}}
";

#[component]
pub(super) fn ActionList(actions: &'static [MediaProviderAction]) -> impl IntoView {
    view! {
        <div class="nr-media-action-list">
            <For
                each=move || actions.to_vec()
                key=|action| action.label
                children=|action| view! { <ActionRow action /> }
            />
        </div>
    }
}

#[component]
pub(super) fn ConfigRows(rows: Vec<MediaProviderConfigRow>) -> impl IntoView {
    view! {
        <div class="nr-media-config-list">
            <For
                each=move || rows.clone()
                key=|row| row.label
                children=|row| view! {
                    <div class="nr-media-config-row">
                        <strong>{row.label}</strong>
                        <span class="nr-media-mono">{row.value}</span>
                    </div>
                }
            />
        </div>
    }
}

#[component]
pub(super) fn MemberRows(members: Vec<MediaProviderComboMember>) -> impl IntoView {
    view! {
        <div class="nr-media-action-list">
            <For
                each=move || members.clone()
                key=|member| member.entry.clone()
                children=|member| view! { <MemberRow member /> }
            />
        </div>
    }
}

#[component]
pub(super) fn PlaceholderNotice(placeholder: MediaProviderPlaceholder) -> impl IntoView {
    view! {
        <article class="nr-card nr-media-placeholder">
            <div class="nr-empty-state">
                <strong>{placeholder.title}</strong>
                <span>{placeholder.detail}</span>
            </div>
        </article>
    }
}

#[component]
pub(super) fn MediaProviderStyles() -> impl IntoView {
    view! { <style>{MEDIA_PROVIDER_STYLES}</style> }
}

pub(super) fn invalid_media_route(message: &'static str) -> AnyView {
    view! {
        <MediaProviderStyles />
        <div class="nr-panel-stack">
            <article class="nr-card">
                <div class="nr-empty-state">
                    <strong>{message}</strong>
                    <span>"The route contains an unsupported nested segment."</span>
                    <a class="nr-button secondary small" href="/dashboard/media-providers/web">"Back to Web Media"</a>
                </div>
            </article>
        </div>
    }
    .into_any()
}

pub(super) fn endpoint_label(method: &'static str, path: &'static str) -> String {
    if path.is_empty() {
        "No upstream endpoint is registered for this kind.".to_owned()
    } else {
        format!("{method} {path}")
    }
}

pub(super) fn listing_href(kind_id: &str) -> String {
    match kind_id {
        "webSearch" | "webFetch" => "/dashboard/media-providers/web".to_owned(),
        _ => format!("/dashboard/media-providers/{kind_id}"),
    }
}

pub(super) fn state_icon(kind_id: &str) -> &'static str {
    match kind_id {
        "embedding" => "emb",
        "image" | "imageToText" => "img",
        "tts" | "stt" => "aud",
        "webSearch" | "webFetch" => "web",
        "video" => "vid",
        "music" => "mus",
        _ => "med",
    }
}

#[component]
fn ActionRow(action: MediaProviderAction) -> impl IntoView {
    view! {
        <div class="nr-media-action-row">
            <span>
                <strong>{action.label}</strong>
                <span>{action.status_label}</span>
            </span>
            <button type="button" class="nr-button secondary small" disabled=move || !action.enabled>"Preview"</button>
        </div>
    }
}

#[component]
fn MemberRow(member: MediaProviderComboMember) -> impl IntoView {
    let model = if member.model.is_empty() {
        "provider default".to_owned()
    } else {
        member.model
    };

    view! {
        <div class="nr-media-member-row">
            <span>
                <strong>{member.provider_name}</strong>
                <span class="nr-media-mono">{model}</span>
            </span>
            <button type="button" class="nr-button secondary small" disabled>"Remove"</button>
        </div>
    }
}
