use crate::dashboard::{EndpointRow, endpoint_rows, model_catalog};
use crate::ui::dashboard_icon_glyph;
use leptos::prelude::*;

#[component]
pub(super) fn EndpointPanel() -> impl IntoView {
    let (copied, set_copied) = signal(None::<&'static str>);
    let model_count = format!("{} models", model_catalog().len());

    view! {
        <div class="nr-dashboard-grid">
            <article class="nr-card nr-endpoint-card">
                <div class="nr-card-head">
                    <div>
                        <h2><span class="nr-card-icon">"api"</span>"API Endpoint"</h2>
                        <p>"Use these local and tunnel-compatible endpoint shapes with OpenAI-compatible clients."</p>
                    </div>
                </div>
                <div class="nr-endpoint-rows">
                    <For
                        each=endpoint_rows
                        key=|row| row.label
                        children=move |row| view! { <EndpointLine row=*row copied set_copied /> }
                    />
                </div>
            </article>
            <article class="nr-card nr-auth-card">
                <div class="nr-card-head between">
                    <div>
                        <h2><span class="nr-card-icon">"key"</span>"API Keys"</h2>
                        <p>"Persistent key management will attach to the host service contract."</p>
                    </div>
                    <button type="button" class="nr-button secondary small" disabled>"Create Key"</button>
                </div>
                <div class="nr-setting-summary">
                    <span>
                        <strong>"Require API key"</strong>
                        <small>"Bootstrap routes currently accept local development calls."</small>
                    </span>
                    <span class="nr-toggle" aria-hidden="true"><span></span></span>
                </div>
                <div class="nr-empty-state">
                    <strong>"No API keys yet"</strong>
                    <span>"The WASM dashboard is ready for the Actix host to hydrate real key state."</span>
                </div>
            </article>
            <MetricCard label="Health" value="200 OK".to_owned() detail="/api/health" tone="success" />
            <MetricCard label="Models" value=model_count detail="/v1/models" tone="info" />
            <MetricCard label="Execution" value="501 stub".to_owned() detail="/v1/chat/completions" tone="warn" />
        </div>
    }
}

#[component]
fn EndpointLine(
    row: EndpointRow,
    copied: ReadSignal<Option<&'static str>>,
    set_copied: WriteSignal<Option<&'static str>>,
) -> impl IntoView {
    let button_icon = move || {
        if copied.get() == Some(row.label) {
            dashboard_icon_glyph("check")
        } else {
            dashboard_icon_glyph("content_copy")
        }
    };

    view! {
        <div class="nr-endpoint-row">
            <span class=format!("nr-row-label {}", row.badge.class_name())>{row.label}</span>
            <code>{row.value}</code>
            <button
                type="button"
                class="nr-icon-button"
                aria-label=format!("Copy {} endpoint", row.label)
                on:click=move |_| copy_endpoint(row.value, row.label, set_copied)
            >
                <span class="material-symbols-outlined" aria-hidden="true">{button_icon}</span>
            </button>
        </div>
    }
}

#[cfg(target_arch = "wasm32")]
fn copy_endpoint(
    endpoint: &'static str,
    label: &'static str,
    set_copied: WriteSignal<Option<&'static str>>,
) {
    let clipboard = web_sys::window().map(|window| window.navigator().clipboard());

    if let Some(clipboard) = clipboard {
        wasm_bindgen_futures::spawn_local(async move {
            if wasm_bindgen_futures::JsFuture::from(clipboard.write_text(endpoint))
                .await
                .is_ok()
            {
                set_copied.set(Some(label));
            }
        });
    }
}

#[cfg(not(target_arch = "wasm32"))]
const fn copy_endpoint(
    _endpoint: &'static str,
    _label: &'static str,
    _set_copied: WriteSignal<Option<&'static str>>,
) {
}

#[component]
fn MetricCard(
    label: &'static str,
    value: String,
    detail: &'static str,
    tone: &'static str,
) -> impl IntoView {
    view! {
        <article class=format!("nr-card nr-metric-card {}", tone)>
            <span class="nr-metric-label">{label}</span>
            <strong>{value}</strong>
            <small>{detail}</small>
        </article>
    }
}
