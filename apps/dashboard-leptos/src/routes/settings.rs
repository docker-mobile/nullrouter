//! Writable router settings: only the fields the state service actually stores.
//!
//! # One card per subject, one save per card
//!
//! The previous version put five toggles and a Save button in a single card, so a change to one switch
//! meant re-sending all five, and a refusal on one field looked like a refusal of the form. This version
//! groups the switches by what they govern and saves each group on its own, so the error message lands next
//! to the thing that caused it.
//!
//! The appearance picker is not part of the settings payload: the theme is a cookie, not server state, and
//! saving it alongside the router's switches implied a persistence it does not have.

use leptos::prelude::*;

use crate::api::{Hydrate, Save, encode, load, post, put, submit};
use crate::routes::types::{SettingsPatch, SettingsView};
use crate::t;
use crate::theme::{Selection, use_theme};
use crate::ui::{
    Alert, AlertDescription, AlertTitle, AlertVariant, Button, ButtonSize, ButtonVariant, Card,
    CardContent, CardDescription, CardHeader, CardTitle, Checkbox, ErrorNote, Field, Icon,
    IconName, Input, PageBody, PageHeader, Panel, Separator, Switch,
};

#[component]
pub fn Settings() -> impl IntoView {
    let (settings, set_settings) = signal(Hydrate::<SettingsView>::Loading);
    let (save, set_save) = signal(Save::Idle);
    let reload = move || {
        set_settings.set(Hydrate::Loading);
        load("/api/settings", set_settings);
    };
    reload();

    view! {
        <PageHeader title=t!("nav.settings") description=t!("settings.description") />

        <PageBody>
            <AppearanceCard />

            <Panel
                state=settings
                on_retry=Callback::new(move |()| reload())
                children=move |data: SettingsView| {
                    view! { <SettingsForms data=data save=save set_save=set_save reload=reload /> }
                }
            />

            {move || {
                save.get()
                    .failure()
                    .map(|error| view! { <ErrorNote message=error.message() /> })
            }}

            <MigrateCard />
        </PageBody>
    }
}

/// The theme picker, as three mutually exclusive options rather than a cycle button.
#[component]
fn AppearanceCard() -> impl IntoView {
    let theme = use_theme();

    // Literal keys, not a table of `&str` looked up later: `t!` takes a literal so the key scanner can
    // find it, and a key built from a variable would be invisible to it and left untranslated.
    let options = [
        (Selection::System, IconName::Monitor, t!("theme.system")),
        (Selection::Light, IconName::Sun, t!("theme.light")),
        (Selection::Dark, IconName::Moon, t!("theme.dark")),
    ];

    view! {
        <Card>
            <CardHeader>
                <div class="space-y-1">
                    <CardTitle>{t!("settings.appearance")}</CardTitle>
                    <CardDescription>{t!("settings.appearance_hint")}</CardDescription>
                </div>
            </CardHeader>
            <CardContent>
                <div
                    role="radiogroup"
                    aria-label=t!("theme.label")
                    class="inline-flex items-center gap-0.5 rounded-lg border border-border \
                           bg-muted/50 p-0.5"
                >
                    {options
                        .into_iter()
                        .map(|(choice, icon, label)| {
                            view! {
                                <button
                                    type="button"
                                    role="radio"
                                    class="inline-flex items-center gap-1.5 rounded-[6px] px-3 py-1.5 \
                                           text-[0.8125rem] font-medium outline-none transition-colors \
                                           focus-visible:ring-[3px] focus-visible:ring-ring/50"
                                    class=("bg-background text-foreground shadow-xs", move || {
                                        theme.selection.get() == choice
                                    })
                                    class=("text-muted-foreground hover:text-foreground", move || {
                                        theme.selection.get() != choice
                                    })
                                    aria-checked=move || (theme.selection.get() == choice).to_string()
                                    on:click=move |_| theme.set(choice)
                                >
                                    <Icon name=icon class="size-3.5" />
                                    {label}
                                </button>
                            }
                        })
                        .collect_view()}
                </div>
            </CardContent>
        </Card>
    }
}

#[component]
fn SettingsForms(
    data: SettingsView,
    save: ReadSignal<Save>,
    set_save: WriteSignal<Save>,
    reload: impl Fn() + Copy + 'static + Send + Sync,
) -> impl IntoView {
    let (require_api_key, set_require_api_key) = signal(data.require_api_key);
    let (tunnel, set_tunnel) = signal(data.tunnel_dashboard_access);
    let (proxy, set_proxy) = signal(data.outbound_proxy_enabled);
    let (pxpipe, set_pxpipe) = signal(data.pxpipe_enabled);

    let save_router = move || {
        // Only the toggles this panel owns. The SSO fields on the same endpoint are left out entirely,
        // because the store treats a present key as "set this" and sending them empty from here would
        // clear a working identity-provider configuration.
        let Ok(body) = encode(&SettingsPatch {
            require_api_key: Some(require_api_key.get()),
            tunnel_dashboard_access: Some(tunnel.get()),
            outbound_proxy_enabled: Some(proxy.get()),
            pxpipe_enabled: Some(pxpipe.get()),
            ..SettingsPatch::default()
        }) else {
            return;
        };
        submit(
            set_save,
            move || async move { put("/api/settings", &body).await },
            move |_| reload(),
        );
    };

    view! {
        <div class="grid gap-4 xl:grid-cols-2">
            <Card>
                <CardHeader>
                    <div class="space-y-1">
                        <CardTitle>{t!("settings.router")}</CardTitle>
                        <CardDescription>{t!("settings.router_hint")}</CardDescription>
                    </div>
                </CardHeader>
                <CardContent class="space-y-1">
                    <SwitchRow
                        label=t!("settings.require_api_key")
                        description=t!("settings.require_api_key_hint")
                        checked=require_api_key
                        set=set_require_api_key
                    />
                    <Separator />
                    <SwitchRow
                        label=t!("settings.tunnel_dashboard")
                        description=t!("settings.tunnel_dashboard_hint")
                        checked=tunnel
                        set=set_tunnel
                    />
                    <Separator />
                    <SwitchRow
                        label=t!("settings.outbound_proxy")
                        description=t!("settings.outbound_proxy_hint")
                        checked=proxy
                        set=set_proxy
                    />
                    <Separator />
                    <SwitchRow
                        label=t!("settings.pxpipe")
                        description=t!("settings.pxpipe_hint")
                        checked=pxpipe
                        set=set_pxpipe
                    />
                </CardContent>
                <div class="flex items-center justify-between gap-3 border-t border-border bg-muted/30 px-5 py-3">
                    <p class="text-xs text-muted-foreground">{t!("settings.save_hint")}</p>
                    <Button
                        label=t!("settings.save")
                        size=ButtonSize::Sm
                        disabled=Signal::derive(move || save.get().is_saving())
                        busy=Signal::derive(move || save.get().is_saving())
                        on_click=Callback::new(move |()| save_router())
                    />
                </div>
            </Card>

            <div class="space-y-4">
                <Alert variant=AlertVariant::Info>
                    <AlertTitle>{t!("settings.sso_elsewhere_title")}</AlertTitle>
                    <AlertDescription>
                        <a
                            href="/dashboard/sso"
                            class="inline-flex items-center gap-1 text-foreground underline-offset-4 hover:underline"
                        >
                            {t!("nav.sso")}
                            <Icon name=IconName::ChevronRight class="size-3.5" />
                        </a>
                        " · "
                        {t!("settings.sso_elsewhere_body")}
                    </AlertDescription>
                </Alert>

                <Card>
                    <CardHeader>
                        <CardTitle>{t!("settings.api_keys")}</CardTitle>
                    </CardHeader>
                    <CardContent class="space-y-2">
                        <p class="text-[0.8125rem] leading-relaxed text-muted-foreground">
                            {t!("settings.api_keys_body")}
                        </p>
                        <a
                            href="/dashboard/keys"
                            class=crate::ui::button_class(ButtonVariant::Outline, ButtonSize::Sm)
                        >
                            <Icon name=IconName::Key class="size-4" />
                            {t!("nav.keys")}
                        </a>
                    </CardContent>
                </Card>
            </div>
        </div>
    }
}

/// A labelled switch with a sentence explaining what flipping it does.
///
/// The whole row is the label, so the text is a click target. A switch whose only hit area is the 36px
/// track is a switch people miss.
#[component]
fn SwitchRow(
    label: String,
    description: String,
    checked: ReadSignal<bool>,
    set: WriteSignal<bool>,
) -> impl IntoView {
    // The switch's accessible name is a second copy: the visible label is moved into the tree below, and
    // `view!` cannot read the same `String` twice.
    let switch_name = label.clone();

    view! {
        <div class="flex items-start justify-between gap-4 py-2.5">
            <div class="min-w-0 space-y-0.5">
                <p class="text-[0.8125rem] font-medium text-foreground">{label}</p>
                <p class="text-xs leading-relaxed text-muted-foreground">{description}</p>
            </div>
            <Switch
                checked=Signal::derive(move || checked.get())
                on_change=Callback::new(move |next| set.set(next))
                aria_label=switch_name
            />
        </div>
    }
}

/// The import response.
///
/// The counts sit under `report`, not at the top level. Flattening them here would compile and deserialize
/// fine -- `serde(default)` fills every missing field -- and then report "0 connections" after an import that
/// actually moved two, which is the failure mode this dashboard's API layer exists to prevent.
#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct MigrateResponse {
    #[serde(default)]
    ok: bool,
    /// Present only on failure.
    #[serde(default)]
    error: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    report: MigrateReport,
}

#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct MigrateReport {
    #[serde(default)]
    source: String,
    #[serde(default)]
    connections_imported: u64,
    #[serde(default)]
    combos_imported: u64,
    #[serde(default)]
    proxy_pools_imported: u64,
    /// Per-record problems that did not abort the import, such as API keys that cannot be re-derived.
    /// Surfaced because an import that silently skipped credentials looks complete.
    #[serde(default)]
    warnings: Vec<String>,
}

#[component]
fn MigrateCard() -> impl IntoView {
    let (data_dir, set_data_dir) = signal(String::new());
    let (dry_run, set_dry_run) = signal(true);
    let (save, set_save) = signal(Save::Idle);
    let (report, set_report) = signal(None::<MigrateResponse>);

    let run = move || {
        #[derive(serde::Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Body {
            #[serde(skip_serializing_if = "Option::is_none")]
            data_dir: Option<String>,
            dry_run: bool,
        }
        let dir = data_dir.get();
        let body = Body {
            data_dir: if dir.trim().is_empty() {
                None
            } else {
                Some(dir)
            },
            dry_run: dry_run.get(),
        };
        let Ok(encoded) = encode(&body) else {
            return;
        };
        set_report.set(None);
        submit(
            set_save,
            move || async move { post("/api/migrate/legacy", &encoded).await },
            move |body: String| {
                if let Ok(parsed) = serde_json::from_str::<MigrateResponse>(&body) {
                    set_report.set(Some(parsed));
                } else {
                    set_report.set(Some(MigrateResponse {
                        ok: false,
                        message: body,
                        ..MigrateResponse::default()
                    }));
                }
            },
        );
    };

    view! {
        <Card>
            <CardHeader>
                <div class="space-y-1">
                    <CardTitle>{t!("settings.migrate")}</CardTitle>
                    <CardDescription>{t!("settings.migrate_help")}</CardDescription>
                </div>
            </CardHeader>
            <CardContent class="space-y-3">
                <Field
                    label=t!("settings.migrate_dir")
                    description=t!("settings.migrate_dir_hint")
                >
                    <Input
                        value=Signal::derive(move || data_dir.get())
                        on_input=Callback::new(move |value| set_data_dir.set(value))
                        placeholder=t!("settings.migrate_dir_placeholder")
                    />
                </Field>

                <Checkbox
                    checked=Signal::derive(move || dry_run.get())
                    on_change=Callback::new(move |next| set_dry_run.set(next))
                    label=t!("settings.migrate_dry_run")
                />

                {move || {
                    save.get()
                        .failure()
                        .map(|error| view! { <ErrorNote message=error.message() /> })
                }}

                {move || {
                    report.get().map(|report| {
                        let success = report.ok && report.error.is_empty();
                        let summary = if success {
                            let head = if report.message.is_empty() {
                                t!("settings.migrate_ok")
                            } else {
                                report.message.clone()
                            };
                            let mut summary = format!(
                                "{head} · {} {} · {} {} · {} {}",
                                report.report.connections_imported,
                                t!("settings.migrate_connections"),
                                report.report.proxy_pools_imported,
                                t!("settings.migrate_pools"),
                                report.report.combos_imported,
                                t!("settings.migrate_combos"),
                            );
                            // Where the data came from. Worth showing: discovery searches several
                            // locations, so "which install did it read" is not obvious from the counts.
                            if !report.report.source.is_empty() {
                                summary.push_str(" · ");
                                summary.push_str(&report.report.source);
                            }
                            if !report.report.warnings.is_empty() {
                                summary.push_str(" · ");
                                summary.push_str(&report.report.warnings.join("; "));
                            }
                            summary
                        } else {
                            let detail = if !report.message.is_empty() {
                                report.message
                            } else if !report.error.is_empty() {
                                report.error
                            } else {
                                String::new()
                            };
                            format!("{}: {detail}", t!("settings.migrate_failed"))
                        };
                        view! {
                            <Alert variant=if success {
                                AlertVariant::Info
                            } else {
                                AlertVariant::Destructive
                            }>
                                <AlertDescription>
                                    <p class="whitespace-pre-wrap break-words">{summary}</p>
                                </AlertDescription>
                            </Alert>
                        }
                    })
                }}
            </CardContent>
            <div class="flex items-center justify-between gap-3 border-t border-border bg-muted/30 px-5 py-3">
                <p class="text-xs text-muted-foreground">{t!("settings.migrate_warning")}</p>
                <Button
                    label=t!("settings.migrate_run")
                    variant=ButtonVariant::Outline
                    size=ButtonSize::Sm
                    disabled=Signal::derive(move || save.get().is_saving())
                    busy=Signal::derive(move || save.get().is_saving())
                    on_click=Callback::new(move |()| run())
                />
            </div>
        </Card>
    }
}

#[cfg(test)]
mod tests {
    use super::MigrateResponse;

    /// The exact body `POST /api/migrate/legacy` returns, captured from a running router.
    ///
    /// Pinned as a literal because the counts are nested under `report`: a flat struct would deserialize this
    /// without error and render "0 connections" after importing two.
    const LIVE_RESPONSE: &str = r#"{
        "dryRun": true,
        "ok": true,
        "report": {
            "apiKeysFound": 1,
            "apiKeysImported": 0,
            "combosFound": 0,
            "combosImported": 0,
            "connectionsFound": 2,
            "connectionsImported": 2,
            "format": "sqlite",
            "proxyPoolsFound": 0,
            "proxyPoolsImported": 0,
            "settingsImported": false,
            "source": "/root/.9router/db/data.sqlite",
            "warnings": ["1 API key(s) found but not imported."]
        }
    }"#;

    #[test]
    fn the_live_response_decodes_with_its_counts() {
        let parsed: MigrateResponse =
            serde_json::from_str(LIVE_RESPONSE).expect("the live body must decode");
        assert!(parsed.ok);
        assert_eq!(parsed.report.connections_imported, 2);
        assert_eq!(parsed.report.source, "/root/.9router/db/data.sqlite");
        assert_eq!(parsed.report.warnings.len(), 1);
    }

    #[test]
    fn a_failure_body_is_not_read_as_success() {
        let body =
            r#"{"ok": false, "error": "no_legacy_installation", "message": "nothing found"}"#;
        let parsed: MigrateResponse = serde_json::from_str(body).expect("decodes");
        assert!(!parsed.ok);
        assert_eq!(parsed.error, "no_legacy_installation");
        assert_eq!(parsed.report.connections_imported, 0);
    }
}
