//! Dashboard overview: hero banner, live health, quick actions, and version info.

use leptos::prelude::*;
use nullrouter_contracts::VersionResponse;

use crate::api::{Hydrate, load};
use crate::routes::types::{ModelsList, SettingsView, StateView};
use crate::routes::{PageHeader, Panel};

#[component]
pub fn Overview() -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (version, set_version) = signal(Hydrate::<VersionResponse>::Loading);
    let (settings, set_settings) = signal(Hydrate::<SettingsView>::Loading);
    let (state, set_state) = signal(Hydrate::<StateView>::Loading);
    let (models, set_models) = signal(Hydrate::<ModelsList>::Loading);

    let reload = move || {
        set_version.set(Hydrate::Loading);
        set_settings.set(Hydrate::Loading);
        set_state.set(Hydrate::Loading);
        set_models.set(Hydrate::Loading);
        load("/api/version", set_version);
        load("/api/settings", set_settings);
        load("/api/state", set_state);
        load("/api/models", set_models);
    };
    reload();

    view! {
        <HeroBanner locale=locale.clone() version=version state=state models=models />

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

/// Full-width hero banner with gradient background, title, tagline, and live stats.
#[component]
fn HeroBanner(
    locale: crate::i18n::Locale,
    version: ReadSignal<Hydrate<VersionResponse>>,
    state: ReadSignal<Hydrate<StateView>>,
    models: ReadSignal<Hydrate<ModelsList>>,
) -> impl IntoView {
    view! {
        <div class="relative overflow-hidden rounded-2xl border border-border mb-6
                     bg-gradient-to-br from-primary/10 via-card to-card
                     dark:from-primary/5 dark:via-card dark:to-card">
            // Decorative gradient orbs
            <div class="absolute -top-24 -right-24 size-64 rounded-full bg-primary/10 blur-3xl pointer-events-none" />
            <div class="absolute -bottom-32 -left-12 size-48 rounded-full bg-blue-500/5 blur-3xl pointer-events-none" />

            <div class="relative p-8 md:p-10 space-y-6">
                // Title + tagline
                <div class="space-y-2">
                    <h1 class="text-3xl md:text-4xl font-bold tracking-tight">
                        {locale.get("overview.hero_title").to_owned()}
                    </h1>
                    <p class="text-base md:text-lg text-muted-foreground max-w-2xl">
                        {locale.get("overview.hero_subtitle").to_owned()}
                    </p>
                    <div class="flex flex-wrap gap-2 pt-1">
                        {locale.get("overview.hero_tagline")
                            .split(" · ")
                            .map(|tag| {
                                view! {
                                    <span class="px-2.5 py-1 rounded-full text-xs font-medium
                                                 bg-primary/10 text-primary border border-primary/20">
                                        {tag.to_owned()}
                                    </span>
                                }
                            })
                            .collect::<Vec<_>>()}
                    </div>
                </div>

                // Live stats row
                <div class="grid grid-cols-2 md:grid-cols-4 gap-4 pt-2">
                    <StatCard
                        label=locale.get("overview.providers_connected").to_owned()
                        value=move || match state.get() {
                            Hydrate::Ready(s) => format!("{}", s.connections.len()),
                            _ => "—".to_owned(),
                        }

                    />
                    <StatCard
                        label=locale.get("overview.active_combos").to_owned()
                        value=move || match state.get() {
                            Hydrate::Ready(s) => format!("{}", s.combos.len()),
                            _ => "—".to_owned(),
                        }

                    />
                    <StatCard
                        label=locale.get("overview.models_available").to_owned()
                        value=move || match models.get() {
                            Hydrate::Ready(m) => format!("{}", m.models.len()),
                            _ => "—".to_owned(),
                        }

                    />
                    <StatCard
                        label=locale.get("overview.version").to_owned()
                        value=move || match version.get() {
                            Hydrate::Ready(v) => v.current_version.as_str().to_owned(),
                            _ => "—".to_owned(),
                        }

                    />
                </div>
            </div>
        </div>
    }
}

/// One stat in the hero banner.
#[component]
fn StatCard(label: String, value: impl Fn() -> String + 'static + Send) -> impl IntoView {
    view! {
        <div class="space-y-1">
            <p class="text-xs text-muted-foreground truncate">{label}</p>
            <p class="text-xl font-semibold font-mono tabular-nums">
                {move || value()}
            </p>
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
