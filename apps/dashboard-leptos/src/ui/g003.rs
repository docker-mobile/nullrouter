use crate::dashboard::{
    DashboardPanelRow, DashboardPanelState, basic_chat_state, console_log_state,
    media_providers_web_state, profile_state, translator_state,
};
use leptos::prelude::*;

#[component]
pub(super) fn BasicChatPanel() -> impl IntoView {
    view! { <RoutePanel state=basic_chat_state() /> }
}

#[component]
pub(super) fn TranslatorPanel() -> impl IntoView {
    view! { <RoutePanel state=translator_state() /> }
}

#[component]
pub(super) fn ConsoleLogPanel() -> impl IntoView {
    view! { <RoutePanel state=console_log_state() /> }
}

#[component]
pub(super) fn MediaProvidersWebPanel() -> impl IntoView {
    view! { <RoutePanel state=media_providers_web_state() /> }
}

#[component]
pub(super) fn ProfilePanel() -> impl IntoView {
    view! { <RoutePanel state=profile_state() /> }
}

#[component]
fn RoutePanel(state: DashboardPanelState) -> impl IntoView {
    view! {
        <div class="nr-panel-stack">
            <article class="nr-card">
                <div class="nr-card-head between">
                    <div>
                        <h2><span class="nr-card-icon">"map"</span>{state.title}</h2>
                        <p>{state.route_path}</p>
                    </div>
                    <span class="nr-status-pill is-idle"><span></span>{state.api_status}</span>
                </div>
                <div class="nr-empty-state">
                    <strong>{state.empty_title}</strong>
                    <span>{state.empty_detail}</span>
                </div>
                <div class="nr-panel-stack">
                    <For
                        each=move || state.rows.to_vec()
                        key=|row| row.label
                        children=|row| view! { <RouteRow row /> }
                    />
                </div>
                <button type="button" class="nr-button secondary small" disabled=move || !state.controls_enabled>
                    {state.persistence_status}
                </button>
            </article>
        </div>
    }
}

#[component]
fn RouteRow(row: DashboardPanelRow) -> impl IntoView {
    view! {
        <button type="button" class="nr-setting-row" disabled>
            <span>
                <strong>{row.label}</strong>
                <small>{row.value}</small>
            </span>
            <span class="nr-toggle" aria-hidden="true"><span></span></span>
        </button>
    }
}
