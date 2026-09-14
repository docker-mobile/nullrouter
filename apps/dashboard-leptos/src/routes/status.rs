//! Configuration status for routes that have no dedicated panel yet.

use leptos::prelude::*;
use leptos_router::hooks::use_location;

use crate::api::{Hydrate, load};
use crate::routes::types::SettingsView;
use crate::routes::{PageHeader, Panel};

#[component]
pub fn StatusPage() -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let location = use_location();
    let (settings, set_settings) = signal(Hydrate::<SettingsView>::Loading);
    let reload = move || {
        set_settings.set(Hydrate::Loading);
        load("/api/settings", set_settings);
    };
    reload();

    let description = locale.get("status.description").to_owned();

    view! {
        {move || {
            view! {
                <PageHeader
                    title=section_title(&location.pathname.get())
                    description=description.clone()
                />
            }
        }}
        <Panel
            state=settings
            on_retry=Callback::new(move |()| reload())
            children=move |data: SettingsView| {
                view! {
                    <dl class="rounded-lg border border-border bg-card p-5 space-y-2 text-sm">
                        <Row
                            label=locale.get("settings.require_api_key").to_owned()
                            value=on_off(data.require_api_key, &locale)
                        />
                        <Row
                            label=locale.get("settings.tunnel_dashboard").to_owned()
                            value=on_off(data.tunnel_dashboard_access, &locale)
                        />
                        <Row
                            label=locale.get("settings.outbound_proxy").to_owned()
                            value=on_off(data.outbound_proxy_enabled, &locale)
                        />
                        <Row
                            label=locale.get("settings.pxpipe").to_owned()
                            value=on_off(data.pxpipe_enabled, &locale)
                        />
                        <Row
                            label=locale.get("status.oidc").to_owned()
                            value=if data.oidc_client_id.is_empty() {
                                locale.get("state.disabled").to_owned()
                            } else {
                                data.oidc_client_id
                            }
                        />
                    </dl>
                }
            }
        />
    }
}

fn on_off(on: bool, locale: &crate::i18n::Locale) -> String {
    if on {
        locale.get("state.enabled").to_owned()
    } else {
        locale.get("state.disabled").to_owned()
    }
}

fn section_title(path: &str) -> String {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("status")
        .replace('-', " ")
}

#[component]
fn Row(label: String, value: String) -> impl IntoView {
    view! {
        <div class="flex items-center justify-between gap-4">
            <dt class="text-muted-foreground">{label}</dt>
            <dd>{value}</dd>
        </div>
    }
}
