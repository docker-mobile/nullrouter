#![allow(unreachable_pub)]

mod basic_chat;
mod cli_tools_live;
mod combos;
mod console_log;
mod endpoint;
mod g003;
mod headroom;
mod locales;
mod material_icons;
mod media_providers;
mod migrate;
mod mitm;
mod navigation;
mod parity;
mod pricing;
mod providers;
mod proxy_pools;
mod section;
mod settings;
mod shell;
mod translator;
mod usage;

use basic_chat::BasicChatPanel;
use cli_tools_live::{CliToolDetailPanel, CliToolsPanel};
use combos::CombosPanel;
use console_log::ConsoleLogPanel;
use endpoint::EndpointPanel;
use g003::{MediaProvidersWebPanel, ProfilePanel};
use leptos::prelude::*;
pub use locales::{LocaleOption, active_locale_label, dashboard_locales, install_locale_signal};
pub use material_icons::dashboard_icon_glyph;
use media_providers::{MediaProviderComboPanel, MediaProviderDetailPanel, MediaProviderKindPanel};
use migrate::MigratePanel;
use mitm::MitmPanel;
pub use navigation::{
    AccountAction, HeaderControl, MediaNavigationItem, SearchDestination,
    dashboard_account_actions, dashboard_header_controls, dashboard_media_navigation,
    dashboard_primary_navigation, dashboard_search, dashboard_section_path,
    dashboard_system_navigation,
};
use parity::{QuotaTrackerPanel, SkillsPanel, TokenSaverPanel};
use pricing::PricingSettingsPanel;
use providers::{ModelsPanel, ProviderDetailPanel, ProviderNewPanel, ProvidersPanel};
use proxy_pools::ProxyPoolsPanel;
pub use section::{DashboardRoute, DashboardSection, dashboard_sections};
use settings::SettingsPanel;
pub use shell::dashboard_shell_visible_contract;
use shell::{Header, HeaderPanel, ShellSignals, Sidebar};
use translator::TranslatorPanel;
use usage::UsagePanel;

pub use console_log::console_log_visible_contract;
pub use proxy_pools::proxy_pools_visible_contract;
pub use translator::translator_visible_contract;

#[component]
pub fn App() -> impl IntoView {
    let (route, set_route) = signal(initial_route());
    let (drawer_open, set_drawer_open) = signal(false);
    let (header_panel, set_header_panel) = signal(HeaderPanel::Closed);
    let shell = ShellSignals {
        active: route,
        set_active: set_route,
        drawer_open,
        set_drawer_open,
        header_panel,
        set_header_panel,
    };

    view! {
        <div
            class="nr-app-shell"
            data-leptos="mounted"
            on:keydown=move |event: web_sys::KeyboardEvent| {
                if event.key() == "Escape" {
                    set_drawer_open.set(false);
                    set_header_panel.set(HeaderPanel::Closed);
                }
            }
        >
            <Sidebar shell drawer=false />
            <button
                type="button"
                class="nr-sidebar-overlay"
                class:nr-sidebar-overlay-open=move || drawer_open.get()
                data-state=move || if drawer_open.get() { "open" } else { "closed" }
                aria-label="Close mobile dashboard navigation"
                title="Close mobile dashboard navigation"
                aria-controls="nr-mobile-sidebar"
                disabled=move || !drawer_open.get()
                on:click=move |_| {
                    set_drawer_open.set(false);
                    set_header_panel.set(HeaderPanel::Closed);
                    shell::focus_sidebar_trigger();
                }
            ></button>
            <Sidebar shell drawer=true />
            <main class="nr-workspace">
                <div class="nr-grid-bg" aria-hidden="true"></div>
                <Header shell />
                <section class="nr-content" aria-label="9Router dashboard">
                    <Show when=move || route.get().section() != DashboardSection::Mitm>
                        <StatusAlert />
                    </Show>
                    {move || match route.get() {
                        DashboardRoute::Section { section } => match section {
                            DashboardSection::Endpoint => view! { <EndpointPanel /> }.into_any(),
                            DashboardSection::BasicChat => view! { <BasicChatPanel /> }.into_any(),
                            DashboardSection::Providers => view! { <ProvidersPanel /> }.into_any(),
                            DashboardSection::MediaProvidersWeb => view! { <MediaProvidersWebPanel /> }.into_any(),
                            DashboardSection::ProxyPools => view! { <ProxyPoolsPanel /> }.into_any(),
                            DashboardSection::Translator => view! { <TranslatorPanel /> }.into_any(),
                            DashboardSection::Usage => view! { <UsagePanel /> }.into_any(),
                            DashboardSection::Status => view! { <StatusPanel /> }.into_any(),
                            DashboardSection::Settings => view! { <SettingsPanel /> }.into_any(),
                            DashboardSection::SettingsPricing => view! { <PricingSettingsPanel /> }.into_any(),
                            DashboardSection::Combos => view! { <CombosPanel /> }.into_any(),
                            DashboardSection::QuotaTracker => view! { <QuotaTrackerPanel /> }.into_any(),
                            DashboardSection::TokenSaver => view! { <TokenSaverPanel /> }.into_any(),
                            DashboardSection::ConsoleLog => view! { <ConsoleLogPanel /> }.into_any(),
                            DashboardSection::CliTools => view! { <CliToolsPanel /> }.into_any(),
                            DashboardSection::Skills => view! { <SkillsPanel /> }.into_any(),
                            DashboardSection::Profile => view! { <ProfilePanel /> }.into_any(),
                            DashboardSection::Migrate => view! { <MigratePanel /> }.into_any(),
                            DashboardSection::Mitm => view! { <MitmPanel /> }.into_any(),
                        },
                        DashboardRoute::ProviderNew => view! { <ProviderNewPanel /> }.into_any(),
                        DashboardRoute::ProviderDetail { provider_id } => {
                            view! { <ProviderDetailPanel provider_id /> }.into_any()
                        }
                        DashboardRoute::MediaProviderKind { provider_kind } => {
                            view! { <MediaProviderKindPanel provider_kind /> }.into_any()
                        }
                        DashboardRoute::MediaProviderDetail { provider_kind, provider_id } => {
                            view! { <MediaProviderDetailPanel provider_kind provider_id /> }.into_any()
                        }
                        DashboardRoute::MediaProviderCombo { combo_id } => {
                            view! { <MediaProviderComboPanel combo_id /> }.into_any()
                        }
                        DashboardRoute::CliToolDetail { tool_id } => {
                            view! { <CliToolDetailPanel tool_id /> }.into_any()
                        }
                    }}
                </section>
            </main>
        </div>
    }
}

#[cfg(target_arch = "wasm32")]
fn initial_route() -> DashboardRoute {
    web_sys::window()
        .map(|window| {
            let location = window.location();
            match location.hash() {
                Ok(hash) if !hash.is_empty() => DashboardRoute::from_hash(&hash),
                _ => location
                    .pathname()
                    .map(|path| DashboardRoute::from_path(&path))
                    .unwrap_or(DashboardRoute::for_section(DashboardSection::Endpoint)),
            }
        })
        .unwrap_or(DashboardRoute::for_section(DashboardSection::Endpoint))
}

#[cfg(not(target_arch = "wasm32"))]
const fn initial_route() -> DashboardRoute {
    DashboardRoute::for_section(DashboardSection::Endpoint)
}

#[component]
fn StatusAlert() -> impl IntoView {
    view! {
        <div class="nr-status-alert">
            <span class="nr-status-icon">"!"</span>
            <span>
                <strong>"Provider execution"</strong>
                <span>"Chat, responses, and messages currently surface explicit gateway status while routing is ported."</span>
            </span>
        </div>
    }
}

#[component]
fn StatusPanel() -> impl IntoView {
    view! {
        <div class="nr-panel-stack">
            <div class="nr-card nr-card-hero">
                <div>
                    <p class="nr-eyebrow">"Gateway"</p>
                    <h2>"Local router status"</h2>
                    <p>"The dashboard shell is ready for live Pingora/Actix wiring and keeps API state visible without claiming unfinished provider execution."</p>
                </div>
                <div class="nr-health-orb" aria-label="Gateway status 200 OK">
                    <strong>"200"</strong>
                    <span>"OK"</span>
                </div>
            </div>
            <ModelsPanel />
        </div>
    }
}

// The Settings panel used to be defined here, as three toggles over local
// `signal()`s — including a "Require dashboard login" toggle that moved a knob
// and wrote nothing. The real panel lives in `ui/settings.rs`, reads
// `GET /api/settings`, and saves each row with `PUT /api/settings`.
