use super::super::{HeaderPanel, ShellSignals};
use crate::ui::{SearchDestination, dashboard_icon_glyph, dashboard_search};
use leptos::prelude::*;

#[component]
pub(super) fn HeaderSearch(
    shell: ShellSignals,
    query: ReadSignal<String>,
    set_query: WriteSignal<String>,
) -> impl IntoView {
    let search_input = NodeRef::<leptos::html::Input>::new();

    #[cfg(target_arch = "wasm32")]
    Effect::new(move |_| {
        if shell.header_panel.get() == HeaderPanel::Search
            && let Some(input) = search_input.get()
            && input.focus().is_err()
        {
            shell.set_header_panel.set(HeaderPanel::Closed);
        }
    });

    view! {
        <Show when=move || shell.header_panel.get() == HeaderPanel::Search>
            <section
                id="nr-header-search"
                class="nr-header-popover nr-search-popover"
                role="dialog"
                aria-label="Search dashboard"
            >
                <label class="nr-search-field">
                    <span class="material-symbols-outlined" aria-hidden="true">
                        {dashboard_icon_glyph("search")}
                    </span>
                    <input
                        type="search"
                        aria-label="Search dashboard destinations"
                        placeholder="Search dashboard"
                        autocomplete="off"
                        autofocus
                        node_ref=search_input
                        prop:value=move || query.get()
                        on:input=move |event| set_query.set(event_target_value(&event))
                    />
                    <Show when=move || !query.get().is_empty()>
                        <button
                            type="button"
                            class="nr-search-clear"
                            aria-label="Clear search"
                            title="Clear search"
                            on:click=move |_| set_query.set(String::new())
                        >
                            <span class="material-symbols-outlined" aria-hidden="true">
                                {dashboard_icon_glyph("close")}
                            </span>
                        </button>
                    </Show>
                </label>
                <div class="nr-search-results" role="listbox" aria-label="Dashboard destinations">
                    <Show
                        when=move || !dashboard_search(&query.get()).is_empty()
                        fallback=move || view! {
                            <p class="nr-search-empty">"No destinations found"</p>
                        }
                    >
                        <For
                            each=move || dashboard_search(&query.get())
                            key=|destination| destination.path
                            children=move |destination| view! {
                                <SearchResult destination shell />
                            }
                        />
                    </Show>
                </div>
            </section>
        </Show>
    }
}

#[component]
fn SearchResult(destination: &'static SearchDestination, shell: ShellSignals) -> impl IntoView {
    view! {
        <a
            class="nr-search-result"
            role="option"
            href=destination.path
            on:click=move |_| {
                shell.set_active.set(crate::ui::DashboardRoute::from_path(destination.path));
                shell.set_header_panel.set(HeaderPanel::Closed);
            }
        >
            <span class="material-symbols-outlined" aria-hidden="true">
                {dashboard_icon_glyph(destination.icon)}
            </span>
            <span>{destination.label}</span>
        </a>
    }
}
