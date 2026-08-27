mod account;

pub(super) use account::HeaderAccount;

use super::super::{HeaderPanel, ShellSignals};
use crate::ui::{LocaleOption, dashboard_locales};
use leptos::prelude::*;

#[component]
pub(super) fn HeaderLanguage(
    shell: ShellSignals,
    locale: ReadSignal<&'static str>,
    set_locale: WriteSignal<&'static str>,
) -> impl IntoView {
    view! {
        <Show when=move || shell.header_panel.get() == HeaderPanel::Language>
            <div
                id="nr-header-language"
                class="nr-header-popover nr-language-popover"
                role="menu"
                aria-label="Language"
            >
                <div class="nr-popover-heading">
                    <strong>"Language"</strong>
                    <span>{move || locale.get()}</span>
                </div>
                <div class="nr-language-list">
                    <For
                        each=move || dashboard_locales().iter().copied()
                        key=|option| option.id
                        children=move |option| view! {
                            <LocaleMenuItem option shell locale set_locale />
                        }
                    />
                </div>
            </div>
        </Show>
    }
}

#[component]
fn LocaleMenuItem(
    option: LocaleOption,
    shell: ShellSignals,
    locale: ReadSignal<&'static str>,
    set_locale: WriteSignal<&'static str>,
) -> impl IntoView {
    view! {
        <button
            type="button"
            class="nr-language-item"
            class:selected=move || locale.get() == option.id
            role="menuitemradio"
            aria-checked=move || (locale.get() == option.id).to_string()
            on:click=move |_| {
                set_locale.set(option.id);
                shell.set_header_panel.set(HeaderPanel::Closed);
            }
        >
            <span>{option.name}</span>
            <small>{option.id}</small>
        </button>
    }
}
