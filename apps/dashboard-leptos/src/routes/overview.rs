//! Service health, version, and the switches that decide how requests are handled.

use leptos::prelude::*;
use nullrouter_contracts::VersionResponse;

use crate::api::{Hydrate, load};
use crate::routes::types::SettingsView;
use crate::routes::{PageHeader, Panel};

#[component]
pub fn Overview() -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (version, set_version) = signal(Hydrate::<VersionResponse>::Loading);
    let (settings, set_settings) = signal(Hydrate::<SettingsView>::Loading);

    let reload = move || {
        set_version.set(Hydrate::Loading);
        set_settings.set(Hydrate::Loading);
        load("/api/version", set_version);
        load("/api/settings", set_settings);
    };
    reload();

    view! {
        <PageHeader
            title=locale.get("nav.dashboard").to_owned()
            description=locale.get("overview.description").to_owned()
        />

        <div class="grid gap-4 md:grid-cols-2">
            <Card title=locale.get("overview.version").to_owned()>
                <Panel
                    state=version
                    on_retry=Callback::new(move |()| reload())
                    children=|data: VersionResponse| view! { <VersionBody data=data /> }
                />
            </Card>

            <Card title=locale.get("overview.request_handling").to_owned()>
                <Panel
                    state=settings
                    on_retry=Callback::new(move |()| reload())
                    children=|data: SettingsView| view! { <SettingsSummary data=data /> }
                />
            </Card>
        </div>
    }
}

#[component]
fn Card(title: String, children: Children) -> impl IntoView {
    view! {
        <section class="rounded-lg border border-border bg-card p-5 space-y-4">
            <h2 class="text-sm font-medium text-muted-foreground">{title}</h2>
            {children()}
        </section>
    }
}

#[component]
fn VersionBody(data: VersionResponse) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let latest = data.latest_version.clone();

    view! {
        <div class="space-y-3">
            <p class="text-2xl font-semibold tracking-tight font-mono">{data.current_version}</p>
            {if data.has_update {
                let target = latest.unwrap_or_default();
                view! {
                    <div class="flex items-center gap-2 text-sm">
                        <span class="size-1.5 rounded-full bg-warning" />
                        <span class="text-foreground">
                            {format!("{} {target}", locale.get("overview.update_available"))}
                        </span>
                    </div>
                }
                    .into_any()
            } else if latest.is_some() {
                view! {
                    <div class="flex items-center gap-2 text-sm">
                        <span class="size-1.5 rounded-full bg-success" />
                        <span class="text-muted-foreground">
                            {locale.get("overview.up_to_date").to_owned()}
                        </span>
                    </div>
                }
                    .into_any()
            } else {
                view! {
                    <p class="text-sm text-muted-foreground">
                        {locale.get("overview.update_unknown").to_owned()}
                    </p>
                }
                    .into_any()
            }}
        </div>
    }
}

#[component]
fn SettingsSummary(data: SettingsView) -> impl IntoView {
    let locale = crate::i18n::use_locale();

    view! {
        <dl class="space-y-2.5">
            <Row
                label=locale.get("settings.require_api_key").to_owned()
                on=data.require_api_key
            />
            <Row
                label=locale.get("settings.tunnel_dashboard").to_owned()
                on=data.tunnel_dashboard_access
            />
            <Row
                label=locale.get("settings.outbound_proxy").to_owned()
                on=data.outbound_proxy_enabled
            />
            <Row label=locale.get("settings.pxpipe").to_owned() on=data.pxpipe_enabled />
            <Row
                label=locale.get("settings.oidc").to_owned()
                on=data.oidc_client_secret_set || !data.oidc_client_id.is_empty()
            />
        </dl>
    }
}

#[component]
fn Row(label: String, on: bool) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let state = if on {
        locale.get("state.enabled").to_owned()
    } else {
        locale.get("state.disabled").to_owned()
    };

    view! {
        <div class="flex items-center justify-between gap-4 text-sm">
            <dt class="text-muted-foreground truncate">{label}</dt>
            <dd class="flex items-center gap-2 shrink-0">
                <span class=if on {
                    "size-1.5 rounded-full bg-success"
                } else {
                    "size-1.5 rounded-full bg-muted-foreground/40"
                } />
                <span class="text-foreground">{state}</span>
            </dd>
        </div>
    }
}
