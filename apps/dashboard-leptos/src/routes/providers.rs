//! Configured provider connections, plus the registry catalogue they can be created from.

use leptos::prelude::*;
use nullrouter_providers::registry;

use crate::api::{Hydrate, load};
use crate::routes::types::{ProviderRow, ProvidersList, display_name};
use crate::routes::{PageHeader, Panel};

#[component]
pub fn Providers() -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (list, set_list) = signal(Hydrate::<ProvidersList>::Loading);
    let reload = move || {
        set_list.set(Hydrate::Loading);
        load("/api/providers", set_list);
    };
    reload();

    view! {
        <PageHeader
            title=locale.get("nav.providers").to_owned()
            description=locale.get("providers.description").to_owned()
        />
        <Panel
            state=list
            on_retry=Callback::new(move |()| reload())
            children=move |data: ProvidersList| view! { <Connections rows=data.connections /> }
        />
        <section class="mt-6 rounded-lg border border-border bg-card p-5 space-y-3">
            <h2 class="text-sm font-medium text-muted-foreground">
                {locale.get("providers.catalogue").to_owned()}
            </h2>
            <p class="text-sm text-muted-foreground">
                {locale.get("providers.catalogue_hint").to_owned()}
            </p>
            <ul class="grid gap-2 sm:grid-cols-2 lg:grid-cols-3">
                {registry::entries()
                    .iter()
                    .map(|entry| {
                        let name = display_name(entry);
                        view! {
                            <li class="rounded-md border border-border px-3 py-2 text-sm flex items-center justify-between gap-2">
                                <span class="truncate">{name}</span>
                                <code class="text-xs text-muted-foreground shrink-0">
                                    {entry.id.clone()}
                                </code>
                            </li>
                        }
                    })
                    .collect_view()}
            </ul>
        </section>
    }
}

#[component]
fn Connections(rows: Vec<ProviderRow>) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    if rows.is_empty() {
        return view! {
            <p class="text-sm text-muted-foreground">{locale.get("providers.empty").to_owned()}</p>
        }
        .into_any();
    }
    view! {
        <div class="rounded-lg border border-border overflow-x-auto">
            <table class="w-full text-sm">
                <thead class="bg-muted/50 text-muted-foreground">
                    <tr>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("providers.col_name").to_owned()}
                        </th>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("providers.col_provider").to_owned()}
                        </th>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("providers.col_auth").to_owned()}
                        </th>
                        <th class="text-left font-medium px-3 py-2">
                            {locale.get("providers.col_status").to_owned()}
                        </th>
                    </tr>
                </thead>
                <tbody>
                    {rows
                        .into_iter()
                        .map(|row| {
                            view! {
                                <tr class="border-t border-border">
                                    <td class="px-3 py-2">{row.name}</td>
                                    <td class="px-3 py-2 font-mono text-xs">{row.provider}</td>
                                    <td class="px-3 py-2 text-muted-foreground">{row.auth_type}</td>
                                    <td class="px-3 py-2">
                                        {if row.is_active {
                                            locale.get("state.enabled").to_owned()
                                        } else {
                                            locale.get("state.disabled").to_owned()
                                        }}
                                    </td>
                                </tr>
                            }
                        })
                        .collect_view()}
                </tbody>
            </table>
        </div>
    }
    .into_any()
}
