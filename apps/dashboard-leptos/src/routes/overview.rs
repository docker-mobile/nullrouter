//! Dashboard overview: what this router is doing right now.
//!
//! # Why there is no hero banner
//!
//! The previous version opened with a full-width gradient panel, two blurred decorative orbs, an `h1`
//! in 40px type, and a tagline split into pill-shaped tags. It took a third of the first screen to say
//! the product's name, and the numbers an operator actually opens this page for were pushed below it.
//!
//! This version leads with the numbers. The page heading is the same size as every other page's, and the
//! four figures that matter are the first thing under it. The brand is in the sidebar, where it belongs.

use leptos::prelude::*;

use crate::t;
use nullrouter_contracts::VersionResponse;

use crate::api::{Hydrate, load};
use crate::routes::types::{AuthStatus, ModelsList, SettingsView, StateView, UsageStats};
use crate::routes::{
    Card, CardContent, CardHeader, CardTitle, Icon, IconName, PageBody, PageHeader, Panel,
    StatusDot, StatusTone, or_dash,
};

#[component]
pub fn Overview() -> impl IntoView {
    let (version, set_version) = signal(Hydrate::<VersionResponse>::Loading);
    let (settings, set_settings) = signal(Hydrate::<SettingsView>::Loading);
    let (state, set_state) = signal(Hydrate::<StateView>::Loading);
    let (models, set_models) = signal(Hydrate::<ModelsList>::Loading);
    let (usage, set_usage) = signal(Hydrate::<UsageStats>::Loading);
    let (auth, set_auth) = signal(Hydrate::<AuthStatus>::Loading);

    let reload = move || {
        set_version.set(Hydrate::Loading);
        set_settings.set(Hydrate::Loading);
        set_state.set(Hydrate::Loading);
        set_models.set(Hydrate::Loading);
        set_usage.set(Hydrate::Loading);
        set_auth.set(Hydrate::Loading);
        load("/api/version", set_version);
        load("/api/settings", set_settings);
        load("/api/state", set_state);
        load("/api/models", set_models);
        load("/api/usage/stats?period=7d", set_usage);
        load("/api/auth/status", set_auth);
    };
    reload();

    // Every label the tree below reads is resolved once, up front. `view!` moves whatever it
    // references into the closures it builds -- the `Children` prop of a container, each `move ||`
    // block -- so a `Locale` read inside one of those is consumed by it and unavailable to the next.
    // Reading the strings first is the same fix as cloning, without a clone per use.
    let heading = t!("nav.dashboard");
    let blurb = t!("overview.description");
    let chat_action = t!("overview.action_chat");
    let request_handling = t!("overview.request_handling");
    let version_title = t!("overview.version");
    let health_title = t!("overview.health");
    let quick_actions = t!("overview.quick_actions");

    view! {
        <PageHeader title=heading description=blurb>
            <a
                href="/dashboard/chat"
                class=crate::ui::button_class(
                    crate::ui::ButtonVariant::Default,
                    crate::ui::ButtonSize::Sm,
                )
            >
                <Icon name=IconName::MessageSquare class="size-4" />
                {chat_action}
            </a>
        </PageHeader>

        <PageBody>
            <StatGrid state=state models=models usage=usage version=version />

            <div class="grid gap-4 lg:grid-cols-3">
                <div class="lg:col-span-2">
                    <Card>
                        <CardHeader>
                            <CardTitle>{request_handling}</CardTitle>
                        </CardHeader>
                        <CardContent>
                            <Panel
                                state=settings
                                on_retry=Callback::new(move |()| reload())
                                children=|data: SettingsView| {
                                    view! { <SettingsSummary data=data /> }
                                }
                            />
                        </CardContent>
                    </Card>
                </div>

                <Card>
                    <CardHeader>
                        <CardTitle>{version_title}</CardTitle>
                    </CardHeader>
                    <CardContent>
                        <Panel
                            state=version
                            on_retry=Callback::new(move |()| reload())
                            children=|data: VersionResponse| view! { <VersionBody data=data /> }
                        />
                    </CardContent>
                </Card>
            </div>

            <div class="grid gap-4 lg:grid-cols-2">
                <Card>
                    <CardHeader>
                        <CardTitle>{health_title}</CardTitle>
                    </CardHeader>
                    <CardContent>
                        <Panel
                            state=auth
                            on_retry=Callback::new(move |()| reload())
                            children=|data: AuthStatus| view! { <AccessSummary data=data /> }
                        />
                    </CardContent>
                </Card>

                <Card>
                    <CardHeader>
                        <CardTitle>{quick_actions}</CardTitle>
                    </CardHeader>
                    <CardContent>
                        <QuickActions />
                    </CardContent>
                </Card>
            </div>
        </PageBody>
    }
}

/// The four figures the page exists to show.
///
/// A grid of tiles rather than a hero band's inline stats: each one is a card with its own label, so the
/// numbers line up in a column on a narrow screen and a row on a wide one without a bespoke layout.
#[component]
fn StatGrid(
    state: ReadSignal<Hydrate<StateView>>,
    models: ReadSignal<Hydrate<ModelsList>>,
    usage: ReadSignal<Hydrate<UsageStats>>,
    version: ReadSignal<Hydrate<VersionResponse>>,
) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    // One clone per closure below, because each `move ||` takes ownership of whatever it names.
    let hint_locale = locale;

    view! {
        <div class="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
            <Stat
                label=t!("overview.providers_connected")
                icon=IconName::Plug
                value=Signal::derive(move || {
                    state
                        .get()
                        .ready().map_or_else(|| "—".to_owned(), |state| state.connections.len().to_string())
                })
                hint=Signal::derive(move || {
                    let locale = hint_locale.clone();
                    state
                        .get()
                        .ready()
                        .map(|state| {
                            let active = state
                                .connections
                                .iter()
                                .filter(|connection| connection.is_active)
                                .count();
                            locale.fmt("overview.of_total_active", &[("count", &active.to_string())])
                        })
                        .unwrap_or_default()
                })
            />
            <Stat
                label=t!("overview.active_combos")
                icon=IconName::Combine
                value=Signal::derive(move || {
                    state
                        .get()
                        .ready().map_or_else(|| "—".to_owned(), |state| state.combos.len().to_string())
                })
                hint=Signal::derive(String::new)
            />
            <Stat
                label=t!("overview.models_available")
                icon=IconName::Box
                value=Signal::derive(move || {
                    models
                        .get()
                        .ready().map_or_else(|| "—".to_owned(), |models| models.models.len().to_string())
                })
                hint=Signal::derive(String::new)
            />
            <Stat
                label=t!("usage.requests")
                icon=IconName::Activity
                value=Signal::derive(move || {
                    usage
                        .get()
                        .ready().map_or_else(|| "—".to_owned(), |usage| format_count(usage.total_requests))
                })
                hint=Signal::derive(move || {
                    version
                        .get()
                        .ready()
                        .map(|version| format!("v{}", version.current_version))
                        .unwrap_or_default()
                })
            />
        </div>
    }
}

/// One figure.
///
/// The icon is decorative and muted, the label is a single line, and the value is the largest thing in
/// the tile. Deliberately no sparkline: this page has no history endpoint behind it, and a fake trend
/// line is worse than none.
#[component]
fn Stat(
    label: String,
    icon: IconName,
    value: Signal<String>,
    hint: Signal<String>,
) -> impl IntoView {
    view! {
        <div class="rounded-xl border border-border bg-card p-4 shadow-xs">
            <div class="flex items-center gap-1.5 text-muted-foreground">
                <Icon name=icon class="size-3.5" />
                <span class="truncate text-xs font-medium">{label}</span>
            </div>
            <p class="mt-2 font-mono text-2xl font-semibold tracking-tight tabular-nums">
                {move || value.get()}
            </p>
            {move || {
                let hint = hint.get();
                (!hint.is_empty())
                    .then(|| {
                        view! { <p class="mt-0.5 truncate text-xs text-muted-foreground">{hint}</p> }
                    })
            }}
        </div>
    }
}

/// Thousands, with a locale-independent separator.
///
/// `1_234_567` reads as one number; `1234567` needs counting. Not `toLocaleString`: the crate would have
/// to thread a locale tag through, and the dashboard's own locale is the interface language, not the
/// number format the operator expects.
fn format_count(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

#[component]
fn VersionBody(data: VersionResponse) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let current = data.current_version.clone();
    let latest = data.latest_version.clone();
    let has_update = data.has_update;

    view! {
        <div class="space-y-2">
            <p class="font-mono text-2xl font-semibold tracking-tight">{current}</p>
            {if has_update {
                let target = latest.unwrap_or_default();
                view! {
                    <p class="flex items-center gap-2 text-sm">
                        <span class="size-1.5 shrink-0 rounded-full bg-warning" aria-hidden="true" />
                        <span class="text-foreground">
                            {locale.fmt("overview.update_available", &[("version", &target)])}
                        </span>
                    </p>
                }
                    .into_any()
            } else if latest.is_some() {
                view! {
                    <p class="flex items-center gap-2 text-sm text-muted-foreground">
                        <span class="size-1.5 shrink-0 rounded-full bg-success" aria-hidden="true" />
                        {t!("overview.up_to_date")}
                    </p>
                }
                    .into_any()
            } else {
                view! {
                    <p class="text-sm text-muted-foreground">
                        {t!("overview.update_unknown")}
                    </p>
                }
                    .into_any()
            }}
        </div>
    }
}

/// The router's switches, as a definition list.
#[component]
fn SettingsSummary(data: SettingsView) -> impl IntoView {
    view! {
        <dl class="divide-y divide-border/70">
            <Row
                label=t!("settings.require_api_key")
                on=data.require_api_key
            />
            <Row
                label=t!("settings.tunnel_dashboard")
                on=data.tunnel_dashboard_access
            />
            <Row
                label=t!("settings.outbound_proxy")
                on=data.outbound_proxy_enabled
            />
            <Row label=t!("settings.pxpipe") on=data.pxpipe_enabled />
            <Row
                label=t!("settings.oidc")
                on=data.oidc_client_secret_set || !data.oidc_client_id.is_empty()
            />
        </dl>
    }
}

#[component]
fn Row(label: String, on: bool) -> impl IntoView {
    let text = if on {
        t!("state.enabled")
    } else {
        t!("state.disabled")
    };

    view! {
        <div class="flex items-center justify-between gap-4 py-2 first:pt-0 last:pb-0">
            <dt class="truncate text-sm text-muted-foreground">{label}</dt>
            <dd class="shrink-0">
                <StatusDot
                    label=text
                    tone=if on { StatusTone::Success } else { StatusTone::Muted }
                />
            </dd>
        </div>
    }
}

/// Who can get in, and how.
///
/// The shared-password case is called out rather than left implicit: it is the state a fresh install
/// starts in, and the one an operator is most likely to leave behind by accident.
#[component]
fn AccessSummary(data: AuthStatus) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let method = if data.login_method.trim().is_empty() {
        t!("state.unknown")
    } else {
        data.login_method.clone()
    };
    let needs_attention = !data.users_configured && data.has_password;

    view! {
        <div class="space-y-3">
            <dl class="divide-y divide-border/70">
                <div class="flex items-center justify-between gap-4 pb-2">
                    <dt class="text-sm text-muted-foreground">
                        {t!("overview.signed_in_as")}
                    </dt>
                    <dd class="truncate text-sm font-medium">{or_dash(&data.display_name)}</dd>
                </div>
                <div class="flex items-center justify-between gap-4 py-2">
                    <dt class="text-sm text-muted-foreground">
                        {t!("overview.login_method")}
                    </dt>
                    <dd class="truncate text-sm">{method}</dd>
                </div>
                <div class="flex items-center justify-between gap-4 pt-2">
                    <dt class="text-sm text-muted-foreground">
                        {t!("overview.accounts_configured")}
                    </dt>
                    <dd class="shrink-0">
                        <StatusDot
                            label=locale.fmt("overview.account_count", &[
                                ("count", &data.users_configured.to_string()),
                            ])
                            tone=if data.users_configured {
                                StatusTone::Success
                            } else {
                                StatusTone::Muted
                            }
                        />
                    </dd>
                </div>
            </dl>
            {needs_attention
                .then(|| {
                    view! {
                        <a
                            href="/dashboard/users"
                            class="flex items-start gap-2 rounded-md border border-warning/35 \
                                   bg-warning/[0.07] px-3 py-2 text-[0.8125rem] leading-relaxed \
                                   text-foreground transition-colors hover:bg-warning/10"
                        >
                            <Icon name=IconName::Alert class="mt-0.5 size-3.5 shrink-0 text-warning" />
                            <span>{t!("users.shared_password_notice")}</span>
                        </a>
                    }
                })}
        </div>
    }
}

/// The four places an operator is most likely to go next.
#[component]
fn QuickActions() -> impl IntoView {
    let locale = crate::i18n::use_locale();

    let actions = [
        (
            "/dashboard/providers",
            IconName::Plug,
            "overview.action_providers",
        ),
        ("/dashboard/models", IconName::Box, "overview.action_models"),
        (
            "/dashboard/chat",
            IconName::MessageSquare,
            "overview.action_chat",
        ),
        (
            "/dashboard/settings",
            IconName::Settings,
            "overview.action_settings",
        ),
    ];

    view! {
        <div class="grid gap-2 sm:grid-cols-2">
            {actions
                .into_iter()
                .map(|(href, icon, key)| {
                    let label = locale.get(key).to_owned();
                    view! {
                        <a
                            href=href
                            class="group flex items-center gap-2.5 rounded-lg border border-border \
                                   bg-background px-3 py-2.5 text-sm font-medium transition-colors \
                                   hover:border-ring/40 hover:bg-accent \
                                   focus-visible:ring-[3px] focus-visible:ring-ring/50 outline-none"
                        >
                            <Icon
                                name=icon
                                class="size-4 text-muted-foreground group-hover:text-foreground"
                            />
                            <span class="truncate">{label}</span>
                            <Icon
                                name=IconName::ChevronRight
                                class="ml-auto size-3.5 text-muted-foreground/50 \
                                       transition-transform group-hover:translate-x-0.5"
                            />
                        </a>
                    }
                })
                .collect_view()}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::format_count;

    #[test]
    fn large_counts_are_grouped_in_threes() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1000), "1,000");
        assert_eq!(format_count(1234), "1,234");
        assert_eq!(format_count(1_234_567), "1,234,567");
        assert_eq!(format_count(1_000_000_000), "1,000,000,000");
    }
}
