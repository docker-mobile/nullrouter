mod menus;
mod page_info;
mod search;

use self::{
    menus::{HeaderAccount, HeaderLanguage},
    page_info::page_info,
    search::HeaderSearch,
};
use super::{HeaderPanel, ShellSignals};
use crate::ui::{
    HeaderControl, dashboard_header_controls, dashboard_icon_glyph, install_locale_signal,
};
use leptos::prelude::*;

#[component]
pub(crate) fn Header(shell: ShellSignals) -> impl IntoView {
    let (query, set_query) = signal(String::new());
    let (locale, set_locale) = signal("en");
    // Binds the picker to the i18n runtime: a selection now writes the locale
    // cookie, loads that locale's literal map, and re-renders. Without this the
    // dropdown only moved a checkmark.
    let _i18n = install_locale_signal(locale, set_locale);

    view! {
        <header
            class="nr-topbar"
            data-header-panel=move || shell.header_panel.get().as_str()
            on:keydown=move |event: web_sys::KeyboardEvent| {
                if event.key() == "Escape" {
                    shell.set_header_panel.set(HeaderPanel::Closed);
                }
            }
        >
            <button
                id="nr-sidebar-open"
                type="button"
                class="nr-icon-button nr-sidebar-open"
                aria-label="Open dashboard navigation"
                title="Open dashboard navigation"
                aria-controls="nr-mobile-sidebar"
                aria-expanded=move || shell.drawer_open.get().to_string()
                on:click=move |_| {
                    shell.set_header_panel.set(HeaderPanel::Closed);
                    shell.set_drawer_open.set(true);
                }
            >
                <span class="material-symbols-outlined" aria-hidden="true">
                    {dashboard_icon_glyph("menu")}
                </span>
            </button>
            <div class="nr-page-title">
                <span class="nr-title-icon material-symbols-outlined" aria-hidden="true">
                    {move || dashboard_icon_glyph(page_info(&shell.active.get()).icon)}
                </span>
                <span class="nr-page-copy">
                    <h1>{move || page_info(&shell.active.get()).title}</h1>
                    <Show when=move || !page_info(&shell.active.get()).description.is_empty()>
                        <p>{move || page_info(&shell.active.get()).description}</p>
                    </Show>
                </span>
            </div>
            <div class="nr-header-actions">
                <For
                    each=move || dashboard_header_controls().iter().copied()
                    key=|control| control.id
                    children=move |control| view! { <HeaderControlButton control shell /> }
                />
            </div>
            <Show when=move || shell.header_panel.get() != HeaderPanel::Closed>
                <button
                    type="button"
                    class="nr-header-popover-dismiss"
                    aria-label="Close header panel"
                    on:click=move |_| shell.set_header_panel.set(HeaderPanel::Closed)
                ></button>
            </Show>
            <HeaderSearch shell query set_query />
            <HeaderLanguage shell locale set_locale />
            <HeaderAccount shell />
        </header>
    }
}

#[component]
fn HeaderControlButton(control: HeaderControl, shell: ShellSignals) -> impl IntoView {
    let panel = panel_for_control(control.id);
    let aria_label = control_aria_label(panel);

    view! {
        <button
            type="button"
            class="nr-header-control"
            class:active=move || shell.header_panel.get() == panel
            aria-label=aria_label
            title=aria_label
            aria-controls=panel_popup_id(panel)
            aria-haspopup=panel_popup_kind(panel)
            aria-expanded=move || (shell.header_panel.get() == panel).to_string()
            on:click=move |_| {
                shell.set_drawer_open.set(false);
                shell.set_header_panel.update(|current| {
                    *current = if *current == panel { HeaderPanel::Closed } else { panel };
                });
            }
        >
            <span class="material-symbols-outlined" aria-hidden="true">
                {dashboard_icon_glyph(control.icon)}
            </span>
        </button>
    }
}

const fn panel_for_control(id: &str) -> HeaderPanel {
    match id.as_bytes() {
        b"search" => HeaderPanel::Search,
        b"language" => HeaderPanel::Language,
        b"account" => HeaderPanel::Account,
        _ => HeaderPanel::Closed,
    }
}

const fn control_aria_label(panel: HeaderPanel) -> &'static str {
    match panel {
        HeaderPanel::Search => "Search dashboard",
        HeaderPanel::Language => "Language",
        HeaderPanel::Account => "Open account menu",
        HeaderPanel::Closed => "Header control",
    }
}

const fn panel_popup_id(panel: HeaderPanel) -> &'static str {
    match panel {
        HeaderPanel::Search => "nr-header-search",
        HeaderPanel::Language => "nr-header-language",
        HeaderPanel::Account => "nr-header-account",
        HeaderPanel::Closed => "nr-header-closed",
    }
}

const fn panel_popup_kind(panel: HeaderPanel) -> &'static str {
    match panel {
        HeaderPanel::Search => "dialog",
        HeaderPanel::Language | HeaderPanel::Account => "menu",
        HeaderPanel::Closed => "false",
    }
}
