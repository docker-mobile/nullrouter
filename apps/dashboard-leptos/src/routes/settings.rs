//! Writable router settings. Only fields the state service actually stores.

use leptos::prelude::*;

use crate::api::{ApiError, Hydrate, Save, encode, load, post, put, submit};
use crate::routes::types::{SettingsPatch, SettingsView};
use crate::routes::{PageHeader, Panel};
use crate::theme::{Selection, use_theme};

#[component]
pub fn Settings() -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (settings, set_settings) = signal(Hydrate::<SettingsView>::Loading);
    let (save, set_save) = signal(Save::Idle);
    let reload = move || {
        set_settings.set(Hydrate::Loading);
        load("/api/settings", set_settings);
    };
    reload();

    view! {
        <PageHeader
            title=locale.get("nav.settings").to_owned()
            description=locale.get("settings.description").to_owned()
        />
        <section class="rounded-lg border border-border bg-card p-5 space-y-3 mb-4">
            <h2 class="text-sm font-medium text-muted-foreground">
                {locale.get("settings.appearance").to_owned()}
            </h2>
            <ThemePicker />
        </section>
        <Panel
            state=settings
            on_retry=Callback::new(move |()| reload())
            children=move |data: SettingsView| {
                view! { <SettingsForm data=data save=save set_save=set_save reload=reload /> }
            }
        />
        {move || {
            save.get().failure().map(|error| {
                view! { <p class="mt-3 text-sm text-destructive">{error.message()}</p> }
            })
        }}
        <MigratePanel />
    }
}

#[component]
fn ThemePicker() -> impl IntoView {
    let theme = use_theme();
    let locale = crate::i18n::use_locale();
    view! {
        <div class="flex flex-wrap gap-2">
            {Selection::ALL
                .into_iter()
                .map(|choice| {
                    let label = match choice {
                        Selection::System => locale.get("theme.system").to_owned(),
                        Selection::Light => locale.get("theme.light").to_owned(),
                        Selection::Dark => locale.get("theme.dark").to_owned(),
                    };
                    view! {
                        <button
                            type="button"
                            class=move || {
                                let active = theme.selection.get() == choice;
                                format!(
                                    "{} rounded-md border px-3 py-1.5 text-sm {}",
                                    if active {
                                        "border-primary bg-accent"
                                    } else {
                                        "border-border"
                                    },
                                    "transition-colors"
                                )
                            }
                            on:click=move |_| theme.set(choice)
                        >
                            {label}
                        </button>
                    }
                })
                .collect_view()}
        </div>
    }
}

#[component]
fn SettingsForm(
    data: SettingsView,
    save: ReadSignal<Save>,
    set_save: WriteSignal<Save>,
    reload: impl Fn() + Copy + 'static + Send + Sync,
) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (require_api_key, set_require_api_key) = signal(data.require_api_key);
    let (tunnel, set_tunnel) = signal(data.tunnel_dashboard_access);
    let (proxy, set_proxy) = signal(data.outbound_proxy_enabled);
    let (pxpipe, set_pxpipe) = signal(data.pxpipe_enabled);

    view! {
        <section class="rounded-lg border border-border bg-card p-5 space-y-4">
            <h2 class="text-sm font-medium text-muted-foreground">
                {locale.get("settings.router").to_owned()}
            </h2>
            <Toggle
                label=locale.get("settings.require_api_key").to_owned()
                on=require_api_key
                set=set_require_api_key
            />
            <Toggle
                label=locale.get("settings.tunnel_dashboard").to_owned()
                on=tunnel
                set=set_tunnel
            />
            <Toggle
                label=locale.get("settings.outbound_proxy").to_owned()
                on=proxy
                set=set_proxy
            />
            <Toggle label=locale.get("settings.pxpipe").to_owned() on=pxpipe set=set_pxpipe />
            <button
                type="button"
                class="rounded-md bg-primary px-3 py-2 text-sm font-medium text-primary-foreground disabled:opacity-50"
                disabled=move || save.get().is_saving()
                on:click=move |_| {
                    let Ok(body) = encode(&SettingsPatch {
                        require_api_key: Some(require_api_key.get()),
                        tunnel_dashboard_access: Some(tunnel.get()),
                        outbound_proxy_enabled: Some(proxy.get()),
                        pxpipe_enabled: Some(pxpipe.get()),
                    }) else {
                        return;
                    };
                    submit(
                        set_save,
                        move || async move { put("/api/settings", &body).await },
                        move |_| reload(),
                    );
                }
            >
                {locale.get("settings.save").to_owned()}
            </button>
        </section>
    }
}

#[component]
fn Toggle(label: String, on: ReadSignal<bool>, set: WriteSignal<bool>) -> impl IntoView {
    view! {
        <label class="flex items-center justify-between gap-4 text-sm">
            <span>{label}</span>
            <input
                type="checkbox"
                class="size-4"
                prop:checked=move || on.get()
                on:change=move |ev| set.set(event_target_checked(&ev))
            />
        </label>
    }
}

#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct MigrateReport {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    error: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    connections_imported: u64,
    #[serde(default)]
    combos_imported: u64,
    #[serde(default)]
    proxy_pools_imported: u64,
    #[serde(default)]
    warnings: Vec<String>,
}

#[component]
fn MigratePanel() -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (data_dir, set_data_dir) = signal(String::new());
    let (dry_run, set_dry_run) = signal(true);
    let (save, set_save) = signal(Save::Idle);
    let (report, set_report) = signal(Option::<MigrateReport>::None);

    view! {
        <section class="rounded-lg border border-border bg-card p-5 space-y-4 mt-4">
            <h2 class="text-sm font-medium text-muted-foreground">
                {locale.get("settings.migrate").to_owned()}
            </h2>
            <p class="text-sm text-muted-foreground">
                {locale.get("settings.migrate_help").to_owned()}
            </p>
            <label class="block space-y-1 text-sm">
                <span class="text-muted-foreground">{locale.get("settings.migrate_dir").to_owned()}</span>
                <input
                    type="text"
                    class="w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
                    prop:value=move || data_dir.get()
                    on:input=move |ev| set_data_dir.set(event_target_value(&ev))
                    placeholder=locale.get("settings.migrate_dir_placeholder").to_owned()
                />
            </label>
            <label class="flex items-center gap-2 text-sm">
                <input
                    type="checkbox"
                    class="size-4"
                    prop:checked=move || dry_run.get()
                    on:change=move |ev| set_dry_run.set(event_target_checked(&ev))
                />
                <span>{locale.get("settings.migrate_dry_run").to_owned()}</span>
            </label>
            <button
                type="button"
                class="rounded-md bg-primary px-3 py-2 text-sm font-medium text-primary-foreground disabled:opacity-50"
                disabled=move || save.get().is_saving()
                on:click=move |_| {
                    #[derive(serde::Serialize)]
                    #[serde(rename_all = "camelCase")]
                    struct Body {
                        #[serde(skip_serializing_if = "Option::is_none")]
                        data_dir: Option<String>,
                        dry_run: bool,
                    }
                    let dir = data_dir.get();
                    let body = Body {
                        data_dir: if dir.trim().is_empty() { None } else { Some(dir) },
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
                            if let Ok(parsed) = serde_json::from_str::<MigrateReport>(&body) {
                                set_report.set(Some(parsed));
                            } else {
                                set_report.set(Some(MigrateReport {
                                    ok: false,
                                    message: body,
                                    ..MigrateReport::default()
                                }));
                            }
                        },
                    );
                }
            >
                {locale.get("settings.migrate_run").to_owned()}
            </button>
            {move || {
                save.get().failure().map(|error: ApiError| {
                    view! { <p class="text-sm text-destructive">{error.message()}</p> }
                })
            }}
            {move || {
                report.get().map(|report| {
                    let success = report.ok || (!report.source.is_empty() && report.error.is_empty());
                    let summary = if success {
                        let head = if report.message.is_empty() {
                            locale.get("settings.migrate_ok").to_owned()
                        } else {
                            report.message.clone()
                        };
                        let mut summary = format!(
                            "{head} · {} {}, {} {}, {} {}",
                            report.connections_imported,
                            locale.get("settings.migrate_connections"),
                            report.proxy_pools_imported,
                            locale.get("settings.migrate_pools"),
                            report.combos_imported,
                            locale.get("settings.migrate_combos"),
                        );
                        if !report.warnings.is_empty() {
                            summary.push_str(" · ");
                            summary.push_str(&report.warnings.join("; "));
                        }
                        summary
                    } else {
                        let detail = if !report.message.is_empty() {
                            report.message
                        } else if !report.error.is_empty() {
                            report.error
                        } else {
                            locale.get("settings.migrate_failed").to_owned()
                        };
                        format!("{}: {detail}", locale.get("settings.migrate_failed"))
                    };
                    let class = if success {
                        "text-sm text-foreground"
                    } else {
                        "text-sm text-destructive"
                    };
                    view! { <p class=class>{summary}</p> }
                })
            }}
        </section>
    }
}
