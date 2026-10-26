//! A fallback panel for dashboard paths the router does not have a page for yet.
//!
//! The wildcard route under `/dashboard/*rest` lands here. It reads the real settings and reports them,
//! rather than showing a placeholder, so a link to a section that has not been written yet still tells the
//! truth about the router's configuration instead of pretending to be a page.

use leptos::prelude::*;

use crate::t;
use leptos_router::hooks::use_location;

use crate::api::{Hydrate, load};
use crate::routes::or_dash;
use crate::routes::types::SettingsView;
use crate::ui::{
    Alert, AlertDescription, AlertTitle, AlertVariant, Card, CardContent, CardHeader, CardTitle,
    Icon, IconName, PageBody, PageHeader, Panel, StatusDot, StatusTone,
};

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

    let blurb = t!("status.description");
    let not_built_title = t!("status.not_built_title");
    let not_built_body = t!("status.not_built_body");
    let overview_title = t!("status.router_state");

    let path = Memo::new(move |_| location.pathname.get());

    view! {
        // `PageHeader` takes a plain `String`, so the title is derived inside the one reactive closure
        // that owns it rather than being passed as a signal the component cannot render.
        {move || {
            let title = section_title(&location.pathname.get(), &locale);
            view! { <PageHeader title=title description=blurb.clone() /> }
        }}

        <PageBody>
            <Alert variant=AlertVariant::Muted>
                <AlertTitle>{not_built_title}</AlertTitle>
                <AlertDescription>
                    <p>{not_built_body}</p>
                    <p class="mt-1 font-mono text-xs text-muted-foreground/80">
                        {move || path.get()}
                    </p>
                </AlertDescription>
            </Alert>

            <Card>
                <CardHeader>
                    <CardTitle>{overview_title}</CardTitle>
                </CardHeader>
                <CardContent>
                    <Panel
                        state=settings
                        on_retry=Callback::new(move |()| reload())
                        children=move |data: SettingsView| view! { <SettingsRows data=data /> }
                    />
                </CardContent>
            </Card>
        </PageBody>
    }
}

#[component]
fn SettingsRows(data: SettingsView) -> impl IntoView {
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
            <div class="flex items-center justify-between gap-4 py-2 last:pb-0">
                <dt class="text-sm text-muted-foreground">{t!("status.oidc")}</dt>
                <dd class="flex min-w-0 items-center gap-2">
                    <Icon name=IconName::ShieldCheck class="size-3.5 shrink-0 text-muted-foreground" />
                    <span class="truncate font-mono text-xs">
                        {or_dash(&data.oidc_client_id)}
                    </span>
                </dd>
            </div>
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

/// The last path segment, title-cased, as a best-effort page name.
///
/// The navigation table does not know this route -- that is the whole reason the fallback exists -- so
/// there is nothing to look the name up in. A readable form of the path is the honest answer; inventing a
/// friendly name would be a guess presented as a fact.
pub fn section_title(path: &str, locale: &crate::i18n::Locale) -> String {
    // Trailing slashes are stripped first, then the last segment is taken. `/dashboard/` therefore
    // reduces to `dashboard` rather than to an empty string -- the previous form consumed the fallback
    // and rendered a blank heading.
    let trimmed = path.trim_end_matches('/');
    let segment = trimmed.rsplit('/').next().unwrap_or_default();
    if segment.is_empty() || segment == "dashboard" {
        return locale.get("nav.dashboard").to_owned();
    }
    segment.replace('-', " ")
}

#[cfg(test)]
mod tests {
    use super::section_title;
    use crate::i18n::Locale;

    #[test]
    fn a_path_becomes_a_readable_name() {
        let locale = Locale::embedded();
        assert_eq!(section_title("/dashboard/webhooks", &locale), "webhooks");
        assert_eq!(
            section_title("/dashboard/free-tiers/", &locale),
            "free tiers"
        );
    }

    #[test]
    fn the_dashboard_root_falls_back_to_its_own_name() {
        let locale = Locale::embedded();
        // `/dashboard/` has no trailing segment; showing an empty heading would be worse than naming the
        // page the operator is effectively on.
        assert_eq!(section_title("/dashboard/", &locale), "Dashboard");
        assert_eq!(section_title("/dashboard", &locale), "Dashboard");
    }
}
