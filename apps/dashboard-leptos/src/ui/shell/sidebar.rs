use super::ShellSignals;
use crate::ui::{
    DashboardRoute, DashboardSection, MediaNavigationItem, dashboard_icon_glyph,
    dashboard_media_navigation, dashboard_primary_navigation, dashboard_section_path,
    dashboard_system_navigation,
};
use leptos::prelude::*;

#[component]
pub(crate) fn Sidebar(shell: ShellSignals, drawer: bool) -> impl IntoView {
    let sidebar_class = if drawer {
        "nr-sidebar nr-sidebar-drawer"
    } else {
        "nr-sidebar nr-sidebar-desktop"
    };
    let sidebar_id = if drawer {
        "nr-mobile-sidebar"
    } else {
        "nr-desktop-sidebar"
    };

    view! {
        <aside
            id=sidebar_id
            class=sidebar_class
            data-state=move || {
                if drawer && !shell.drawer_open.get() { "closed" } else { "open" }
            }
            aria-label="Dashboard navigation"
            aria-hidden=move || (drawer && !shell.drawer_open.get()).to_string()
            inert=move || drawer && !shell.drawer_open.get()
        >
            <div class="nr-sidebar-head">
                <div class="nr-traffic-lights" aria-hidden="true">
                    <span></span><span></span><span></span>
                </div>
            </div>
            <a
                class="nr-brand"
                href="/dashboard"
                on:click=move |_| {
                    shell.set_active.set(DashboardRoute::for_section(DashboardSection::Endpoint));
                    close_drawer(shell, drawer);
                }
            >
                <span class="nr-brand-mark material-symbols-outlined" aria-hidden="true">
                    {dashboard_icon_glyph("hub")}
                </span>
                <span class="nr-brand-copy">
                    <strong>"9Router Proxy"</strong>
                    <small>"v0.5.20"</small>
                </span>
            </a>
            <nav class="nr-side-nav" aria-label="Dashboard sections">
                <NavGroup sections=dashboard_primary_navigation() shell drawer />
                <p class="nr-side-title">"System"</p>
                <For
                    each=move || dashboard_system_navigation().iter().copied()
                    key=|section| section.hash()
                    children=move |section| {
                        if section == DashboardSection::MediaProvidersWeb {
                            view! { <MediaProviderNav shell drawer /> }.into_any()
                        } else if section == DashboardSection::Profile {
                            view! {
                                <RemoteAction />
                                <NavItem section shell drawer />
                            }.into_any()
                        } else {
                            view! { <NavItem section shell drawer /> }.into_any()
                        }
                    }
                />
            </nav>
        </aside>
    }
}

#[component]
fn NavGroup(
    sections: &'static [DashboardSection],
    shell: ShellSignals,
    drawer: bool,
) -> impl IntoView {
    view! {
        <For
            each=move || sections.iter().copied()
            key=|section| section.hash()
            children=move |section| view! { <NavItem section shell drawer /> }
        />
    }
}

#[component]
fn NavItem(section: DashboardSection, shell: ShellSignals, drawer: bool) -> impl IntoView {
    view! {
        <a
            class="nr-nav-item"
            class:active=move || shell.active.get().section() == section
            aria-current=move || {
                (shell.active.get().section() == section).then_some("page")
            }
            data-route=section.hash()
            href=dashboard_section_path(section)
            on:click=move |_| {
                shell.set_active.set(DashboardRoute::for_section(section));
                close_drawer(shell, drawer);
            }
        >
            <span class="nr-nav-icon material-symbols-outlined" aria-hidden="true">
                {dashboard_icon_glyph(section.icon())}
            </span>
            <span>{section.nav_label()}</span>
        </a>
    }
}

#[component]
fn MediaProviderNav(shell: ShellSignals, drawer: bool) -> impl IntoView {
    let (open, set_open) = signal(false);
    let navigation_id = if drawer {
        "nr-media-navigation-mobile"
    } else {
        "nr-media-navigation-desktop"
    };

    view! {
        <button
            type="button"
            class="nr-nav-item nr-media-nav-trigger"
            class:active=move || shell.active.get().section() == DashboardSection::MediaProvidersWeb
            aria-label="Toggle media providers"
            aria-controls=navigation_id
            aria-expanded=move || open.get().to_string()
            on:click=move |_| set_open.update(|value| *value = !*value)
        >
            <span class="nr-nav-icon material-symbols-outlined" aria-hidden="true">
                {dashboard_icon_glyph("perm_media")}
            </span>
            <span>"Media Providers"</span>
            <span class="nr-media-chevron material-symbols-outlined" class:is-open=move || open.get() aria-hidden="true">
                {dashboard_icon_glyph("expand_more")}
            </span>
        </button>
        <div id=navigation_id class="nr-media-navigation" hidden=move || !open.get()>
            <For
                each=move || dashboard_media_navigation().iter().copied()
                key=|item| item.id
                children=move |item| view! { <MediaNavItem item shell drawer /> }
            />
        </div>
    }
}

#[component]
fn MediaNavItem(item: MediaNavigationItem, shell: ShellSignals, drawer: bool) -> impl IntoView {
    view! {
        <a
            class="nr-media-nav-item"
            class:active=move || media_item_is_active(&shell.active.get(), item.id)
            aria-current=move || media_item_is_active(&shell.active.get(), item.id).then_some("page")
            href=item.path
            on:click=move |_| {
                shell.set_active.set(DashboardRoute::from_path(item.path));
                close_drawer(shell, drawer);
            }
        >
            <span class="material-symbols-outlined" aria-hidden="true">
                {dashboard_icon_glyph(item.icon)}
            </span>
            <span>{item.label}</span>
        </a>
    }
}

#[component]
fn RemoteAction() -> impl IntoView {
    view! {
        <button
            type="button"
            class="nr-nav-item"
            disabled
            aria-disabled="true"
            title="Remote management is not connected to a Rust host service"
        >
            <span class="nr-nav-icon material-symbols-outlined" aria-hidden="true">
                {dashboard_icon_glyph("computer")}
            </span>
            <span>"Remote"</span>
        </button>
    }
}

fn close_drawer(shell: ShellSignals, drawer: bool) {
    if drawer {
        shell.set_drawer_open.set(false);
    }
}

fn media_item_is_active(route: &DashboardRoute, item_id: &str) -> bool {
    match route {
        DashboardRoute::Section { section } => {
            *section == DashboardSection::MediaProvidersWeb && item_id == "web"
        }
        DashboardRoute::MediaProviderKind { provider_kind }
        | DashboardRoute::MediaProviderDetail { provider_kind, .. } => provider_kind == item_id,
        DashboardRoute::MediaProviderCombo { .. }
        | DashboardRoute::ProviderNew
        | DashboardRoute::ProviderDetail { .. }
        | DashboardRoute::CliToolDetail { .. } => false,
    }
}
