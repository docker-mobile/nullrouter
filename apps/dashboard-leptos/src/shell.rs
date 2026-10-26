//! The application frame: sidebar, header, and the outlet routes render into.
//!
//! # What this replaces
//!
//! The previous shell was a flat list of twenty-two links in one column, a header with a toggle button
//! and two unstyled controls, and no way to tell where you were except by which link was tinted. Adding
//! a section meant editing the list and hoping the label fitted.
//!
//! This version derives everything from one table. [`NAV_GROUPS`] is the single source of navigation
//! truth: the sidebar renders it, the breadcrumb resolves a path back through it to produce a section
//! name, and the page title comes from the same entry. A new section is one entry, not three edits that
//! can drift apart.
//!
//! # Layout
//!
//! Three breakpoints, each a different shape rather than the same shape at three widths:
//!
//! - **Under `md`**: the sidebar is a drawer over the content, opened from the header, closed by
//!   choosing a destination or pressing the scrim.
//! - **`md` to `xl`**: a fixed 15rem rail that collapses to icons, with the label carried by a tooltip.
//! - **`xl` and up**: the rail's labels are always visible, because there is room for them and an
//!   operator on a wide screen is looking at a great deal of tabular data and needs the map.

use leptos::prelude::*;
use leptos_router::components::{A, Outlet};
use leptos_router::hooks::use_location;

use crate::api::Hydrate;
use crate::theme::{Selection, use_theme};
use crate::ui::{Icon, IconName, Menu, MenuItem, MenuSeparator, MenuTriggerVariant};

/// One destination in the sidebar.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NavItem {
    /// Message key for the label.
    pub key: &'static str,
    /// Route path.
    pub path: &'static str,
    /// The glyph drawn beside the label.
    pub icon: IconName,
}

/// A named cluster of destinations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NavGroup {
    /// Message key for the group's heading.
    pub key: &'static str,
    /// Its destinations, in order.
    pub items: &'static [NavItem],
}

/// Every sidebar destination, grouped by what it is for.
///
/// The grouping is deliberate rather than alphabetical: someone opening this dashboard is usually
/// answering one of five questions -- is it healthy, what is it routing, who can use it, what is it
/// costing, what is exposed -- and the groups are those questions.
pub const NAV_GROUPS: &[NavGroup] = &[
    NavGroup {
        key: "nav.group_overview",
        items: &[
            NavItem {
                key: "nav.dashboard",
                path: "/dashboard",
                icon: IconName::LayoutDashboard,
            },
            NavItem {
                key: "nav.chat",
                path: "/dashboard/chat",
                icon: IconName::MessageSquare,
            },
            NavItem {
                key: "nav.usage",
                path: "/dashboard/usage",
                icon: IconName::ChartColumn,
            },
            NavItem {
                key: "nav.logs",
                path: "/dashboard/logs",
                icon: IconName::ScrollText,
            },
        ],
    },
    NavGroup {
        key: "nav.group_routing",
        items: &[
            NavItem {
                key: "nav.providers",
                path: "/dashboard/providers",
                icon: IconName::Plug,
            },
            NavItem {
                key: "nav.models",
                path: "/dashboard/models",
                icon: IconName::Box,
            },
            NavItem {
                key: "nav.combos",
                path: "/dashboard/combos",
                icon: IconName::Combine,
            },
            NavItem {
                key: "nav.nodes",
                path: "/dashboard/nodes",
                icon: IconName::Network,
            },
            NavItem {
                key: "nav.catalog",
                path: "/dashboard/catalog",
                icon: IconName::Route,
            },
            NavItem {
                key: "nav.import",
                path: "/dashboard/import",
                icon: IconName::Download,
            },
        ],
    },
    NavGroup {
        key: "nav.group_access",
        items: &[
            NavItem {
                key: "nav.keys",
                path: "/dashboard/keys",
                icon: IconName::Key,
            },
            NavItem {
                key: "nav.users",
                path: "/dashboard/users",
                icon: IconName::Users,
            },
            NavItem {
                key: "nav.sso",
                path: "/dashboard/sso",
                icon: IconName::ShieldCheck,
            },
        ],
    },
    NavGroup {
        key: "nav.group_operations",
        items: &[
            NavItem {
                key: "nav.tunnel",
                path: "/dashboard/tunnel",
                icon: IconName::Globe,
            },
            NavItem {
                key: "nav.proxy_pools",
                path: "/dashboard/proxy-pools",
                icon: IconName::Layers,
            },
            NavItem {
                key: "nav.pxpipe",
                path: "/dashboard/pxpipe",
                icon: IconName::Droplet,
            },
            NavItem {
                key: "nav.headroom",
                path: "/dashboard/headroom",
                icon: IconName::Gauge,
            },
            NavItem {
                key: "nav.cli_tools",
                path: "/dashboard/cli-tools",
                icon: IconName::Terminal,
            },
            NavItem {
                key: "nav.translator",
                path: "/dashboard/translator",
                icon: IconName::Sliders,
            },
        ],
    },
    NavGroup {
        key: "nav.group_billing",
        items: &[
            NavItem {
                key: "nav.pricing",
                path: "/dashboard/pricing",
                icon: IconName::Tag,
            },
            NavItem {
                key: "nav.free_tiers",
                path: "/dashboard/free-tiers",
                icon: IconName::Sparkles,
            },
            NavItem {
                key: "nav.settings",
                path: "/dashboard/settings",
                icon: IconName::Settings,
            },
        ],
    },
];

/// Every destination, flattened. Kept in step with [`NAV_GROUPS`] by construction rather than by hand.
pub fn all_nav_items() -> impl Iterator<Item = (Option<&'static NavGroup>, NavItem)> {
    NAV_GROUPS
        .iter()
        .flat_map(|group| group.items.iter().map(move |item| (Some(group), *item)))
}

/// The item whose path is the longest prefix of `path`, ignoring the query and trailing slash.
///
/// Longest rather than first: `/dashboard/providers` and `/dashboard/providers/openai` are both valid,
/// and the more specific one is what the breadcrumb should name. The root is special-cased because it
/// is a prefix of every other path and would otherwise always win.
pub fn item_for_path(path: &str) -> Option<(Option<&'static NavGroup>, NavItem)> {
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let path = path.trim_end_matches('/');
    let path = if path.is_empty() { "/" } else { path };

    all_nav_items()
        .filter(|(_, item)| {
            if item.path == "/dashboard" {
                path == "/dashboard"
            } else {
                path == item.path || path.starts_with(&format!("{}/", item.path))
            }
        })
        .max_by_key(|(_, item)| item.path.len())
}

/// The frame nested dashboard routes render inside.
#[component]
pub fn DashboardFrame() -> impl IntoView {
    view! {
        <Shell>
            <Outlet />
        </Shell>
    }
}

/// Whether the sidebar is collapsed to icons.
///
/// Module state rather than a signal owned by `Shell`, because the header's toggle and the rail's own
/// tooltips both need it and threading a `WriteSignal` through four components to reach a tooltip is
/// worse than one `RwSignal` provided at the frame.
#[derive(Clone, Copy, Debug)]
struct Sidebar {
    collapsed: RwSignal<bool>,
    /// Whether the narrow-viewport drawer is open. Independent of `collapsed`: a drawer is never
    /// collapsed, and a rail is never a drawer.
    drawer_open: RwSignal<bool>,
}

/// The frame every route renders inside.
#[component]
pub fn Shell(children: Children) -> impl IntoView {
    let sidebar = Sidebar {
        collapsed: RwSignal::new(false),
        // Starts closed even on a narrow viewport: a drawer that opens itself covers the page the user
        // just asked for.
        drawer_open: RwSignal::new(false),
    };
    provide_context(sidebar);
    let location = use_location();

    // A destination change closes the drawer. Without this the drawer stays over the page the user
    // navigated to, which reads as a navigation that did not happen.
    Effect::new(move |_| {
        let _ = location.pathname.get();
        sidebar.drawer_open.set(false);
    });

    view! {
        <div class="min-h-dvh bg-background text-foreground">
            {move || {
                sidebar
                    .drawer_open
                    .get()
                    .then(|| {
                        view! {
                            <button
                                type="button"
                                class="fixed inset-0 z-30 bg-black/50 backdrop-blur-[1px] \
                                       animate-in fade-in duration-150 md:hidden"
                                aria-label="Close navigation"
                                on:click=move |_| sidebar.drawer_open.set(false)
                            />
                        }
                    })
            }}

            <SidebarRail />

            // The content column's inset tracks the rail's width, and both come from the same signal.
            // The rail is `fixed` rather than a flex sibling because a full-height sticky column
            // cannot be expressed as a flex item without the content column also becoming sticky.
            <div
                class="flex min-h-dvh flex-col transition-[padding] duration-200 ease-out md:pl-14"
                class=("md:pl-60", move || !sidebar.collapsed.get())
            >
                <Topbar />
                // min-w-0 on this element: without it a wide table inside a flex child refuses to
                // shrink and pushes the whole layout sideways instead of scrolling in its own box.
                <main class="mx-auto w-full min-w-0 max-w-[100rem] flex-1 px-4 pb-20 pt-5 \
                             sm:px-6 lg:px-8">
                    {children()}
                </main>
            </div>
        </div>
    }
}

/// The persistent navigation rail.
#[component]
fn SidebarRail() -> impl IntoView {
    let sidebar = expect_context::<Sidebar>();
    let location = use_location();
    let locale = crate::i18n::use_locale();

    let collapsed = move || sidebar.collapsed.get();

    view! {
        <aside
            class="fixed inset-y-0 left-0 z-40 flex w-60 flex-col border-r border-sidebar-border \
                   bg-sidebar text-sidebar-foreground \
                   transition-[width,transform] duration-200 ease-out \
                   max-md:-translate-x-full max-md:shadow-2xl \
                   md:z-40 md:translate-x-0"
            class=("max-md:translate-x-0", move || sidebar.drawer_open.get())
            class=("md:w-14", move || collapsed())
        >
            <div class="flex h-14 shrink-0 items-center gap-2.5 px-3">
                <A
                    href="/dashboard"
                    attr:class="flex min-w-0 items-center gap-2.5 rounded-md outline-none \
                                focus-visible:ring-[3px] focus-visible:ring-ring/50"
                >
                    <Logo />
                    <span
                        class="truncate text-[0.9375rem] font-semibold tracking-tight \
                               transition-opacity duration-150"
                        class=("md:opacity-0", move || collapsed())
                    >
                        "nullrouter"
                    </span>
                </A>
            </div>

            <nav
                class="flex-1 space-y-4 overflow-y-auto overflow-x-hidden px-2 pb-4 pt-1"
                aria-label=locale.get("nav.label").to_owned()
            >
                {NAV_GROUPS
                    .iter()
                    .map(|group| {
                        view! {
                            <NavSection
                                group=*group
                                collapsed=collapsed
                                location=location.pathname
                            />
                        }
                    })
                    .collect_view()}
            </nav>

            <SidebarFooter collapsed=collapsed />
        </aside>
    }
}

/// One group of links, with its heading.
#[component]
fn NavSection(
    group: NavGroup,
    collapsed: impl Fn() -> bool + Copy + Send + Sync + 'static,
    location: Memo<String>,
) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let heading = locale.get(group.key).to_owned();

    view! {
        <div class="space-y-0.5">
            // The heading is hidden rather than removed when collapsed: its vertical space is what
            // separates the groups, and losing that turns the rail into one undifferentiated column.
            <p
                class="px-2.5 pb-1 text-[0.6875rem] font-medium uppercase tracking-wider \
                       text-muted-foreground/80 transition-opacity duration-150"
                class=("opacity-0", move || collapsed())
                aria-hidden=move || collapsed().to_string()
            >
                {heading}
            </p>
            {group
                .items
                .iter()
                .map(|item| {
                    view! {
                        <NavLink item=*item collapsed=collapsed location=location />
                    }
                })
                .collect_view()}
        </div>
    }
}

/// One sidebar link.
#[component]
fn NavLink(
    item: NavItem,
    collapsed: impl Fn() -> bool + Copy + Send + Sync + 'static,
    location: Memo<String>,
) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let label = locale.get(item.key).to_owned();
    let title = label.clone();

    // The dashboard root would otherwise match every child route as a prefix and light up alongside
    // whichever section is actually open.
    let active = Memo::new(move |_| {
        let path = location.get();
        let path = path.trim_end_matches('/');
        let path = if path.is_empty() { "/" } else { path };
        if item.path == "/dashboard" {
            path == "/dashboard"
        } else {
            path == item.path || path.starts_with(&format!("{}/", item.path))
        }
    });

    view! {
        <A
            href=item.path
            attr:class=move || {
                let base = "group relative flex h-8 items-center gap-2.5 rounded-md px-2.5 \
                            text-[0.8125rem] font-medium outline-none transition-colors \
                            focus-visible:ring-[3px] focus-visible:ring-ring/50";
                let state = if active.get() {
                    "bg-sidebar-accent text-sidebar-accent-foreground"
                } else {
                    "text-sidebar-foreground/70 hover:bg-sidebar-accent/60 \
                     hover:text-sidebar-accent-foreground"
                };
                format!("{base} {state}")
            }
            attr:title=title
            attr:aria-current=move || active.get().then_some("page")
        >
            // The active marker is a bar on the leading edge rather than a background alone: a tinted
            // row in a list of tinted hovers is easy to lose, and a 2px bar reads at a glance.
            <span
                class="absolute inset-y-1 left-0 w-0.5 rounded-full bg-primary transition-opacity \
                       duration-150"
                class=("opacity-0", move || !active.get())
                aria-hidden="true"
            />
            <Icon name=item.icon class="size-4 shrink-0" />
            <span
                class="truncate transition-opacity duration-150"
                class=("opacity-0", move || collapsed())
            >
                {label.clone()}
            </span>
        </A>
    }
}

/// What sits under the navigation: the version, and a link to the docs.
#[component]
fn SidebarFooter(collapsed: impl Fn() -> bool + Copy + Send + Sync + 'static) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (version, set_version) = signal(Hydrate::<nullrouter_contracts::VersionResponse>::Loading);
    crate::api::load("/api/version", set_version);

    let current = move || match version.get() {
        Hydrate::Ready(data) => data.current_version,
        Hydrate::Loading | Hydrate::Failed(_) => String::new(),
    };

    view! {
        <div class="shrink-0 border-t border-sidebar-border px-3 py-2.5">
            <div class="flex items-center gap-2 text-[0.6875rem] text-muted-foreground">
                <span class="size-1.5 shrink-0 rounded-full bg-success" aria-hidden="true" />
                <span class="truncate transition-opacity duration-150" class=("opacity-0", move || collapsed())>
                    {move || {
                        let version = current();
                        if version.is_empty() {
                            locale.get("nav.version_unknown").to_owned()
                        } else {
                            format!("v{version}")
                        }
                    }}
                </span>
            </div>
        </div>
    }
}

/// The brand mark: three lanes in, one decision node, one lane out, struck through.
///
/// The same geometry as `assets/favicon.svg`, so the tab icon and the sidebar agree. Drawn inline
/// rather than loaded as an `<img>` because it has to take the theme's colours -- the plate is
/// `bg-primary` and the lanes are `primary-foreground`, which no external SVG can read.
#[component]
pub fn Logo() -> impl IntoView {
    view! {
        <div class="grid size-7 shrink-0 place-items-center rounded-md bg-primary">
            <svg
                class="size-[18px] text-primary-foreground"
                viewBox="0 0 32 32"
                fill="none"
                aria-hidden="true"
            >
                <g stroke="currentColor" stroke-width="2.6" stroke-linecap="round">
                    <path d="M5 9h6l4 4" />
                    <path d="M5 16h6" />
                    <path d="M5 23h6l4-4" />
                    <path d="M20 16h7" />
                </g>
                <circle cx="16" cy="16" r="3.4" fill="currentColor" />
                <path
                    d="M12.4 19.6 19.6 12.4"
                    stroke="var(--color-primary, currentColor)"
                    stroke-width="2.6"
                    stroke-linecap="round"
                    class="text-primary"
                />
            </svg>
        </div>
    }
}

/// The top bar: the drawer toggle, a breadcrumb, and the account controls.
#[component]
fn Topbar() -> impl IntoView {
    let sidebar = expect_context::<Sidebar>();

    view! {
        // sticky rather than fixed so it does not need the content below to carry a matching top
        // offset, and translucent so content scrolling under it stays legible.
        <header class="sticky top-0 z-20 flex h-14 shrink-0 items-center gap-2 border-b \
                       border-border bg-background/85 px-3 backdrop-blur-md sm:px-4">
            <button
                type="button"
                class="grid size-8 shrink-0 place-items-center rounded-md text-muted-foreground \
                       outline-none transition-colors hover:bg-accent hover:text-accent-foreground \
                       focus-visible:ring-[3px] focus-visible:ring-ring/50"
                on:click=move |_| {
                    // One control, two behaviours: under `md` it is a drawer, above it a collapse.
                    if viewport_is_narrow() {
                        sidebar.drawer_open.update(|open| *open = !*open);
                    } else {
                        sidebar.collapsed.update(|collapsed| *collapsed = !*collapsed);
                    }
                }
                aria-label="Toggle navigation"
            >
                <Icon name=IconName::PanelLeft class="size-4" />
            </button>

            <Breadcrumb />

            <div class="flex-1" />

            <div class="flex shrink-0 items-center gap-1">
                <ThemeMenu />
                <LanguageMenu />
                <AccountMenu />
            </div>
        </header>
    }
}

/// Where the current page sits, resolved from the navigation table.
#[component]
fn Breadcrumb() -> impl IntoView {
    let location = use_location();
    let locale = crate::i18n::use_locale();
    let home = locale.get("nav.dashboard").to_owned();

    // `(section, page)`, both already-localised. The page is `None` on the root, whose name is
    // already the first crumb.
    let trail = Memo::new(move |_| -> (Option<String>, Option<String>) {
        let path = location.pathname.get();
        let Some((group, item)) = item_for_path(&path) else {
            // A route the navigation does not list: the provider detail page, or the status fallback.
            return (None, None);
        };
        let section = group.map(|group| locale.get(group.key).to_owned());
        let page = (item.path != "/dashboard").then(|| locale.get(item.key).to_owned());
        (section, page)
    });

    view! {
        <nav aria-label="Breadcrumb" class="hidden min-w-0 items-center gap-1.5 text-sm md:flex">
            <A
                href="/dashboard"
                // `-mx-1.5 px-1.5 py-1` widens the hit area to a 28px row without moving the text: a
                // 20px-tall link is below the 24px minimum a touch target should have.
                attr:class="inline-flex h-7 shrink-0 items-center rounded px-1.5 \
                            text-muted-foreground transition-colors hover:bg-accent \
                            hover:text-foreground outline-none \
                            focus-visible:ring-[3px] focus-visible:ring-ring/50"
            >
                {home}
            </A>
            {move || {
                let (group, item) = trail.get();
                let section = group.map(|group| {
                    view! {
                        <>
                            <Icon name=IconName::ChevronRight class="size-3.5 shrink-0 text-muted-foreground/50" />
                            <span class="truncate text-muted-foreground">{group}</span>
                        </>
                    }
                });
                let leaf = item.map(|item| {
                    view! {
                        <>
                            <Icon name=IconName::ChevronRight class="size-3.5 shrink-0 text-muted-foreground/50" />
                            <span class="truncate font-medium text-foreground">{item}</span>
                        </>
                    }
                });
                view! { {section} {leaf} }
            }}
        </nav>
    }
}

/// The theme control: a menu, not a cycle button.
///
/// The previous version cycled System -> Light -> Dark on every press with the current state only in a
/// tooltip. Choosing from three named options is a decision the user makes once; a cycle button makes
/// them press it up to three times and remember where it started.
#[component]
fn ThemeMenu() -> impl IntoView {
    let theme = use_theme();
    let locale = crate::i18n::use_locale();

    let options = [
        (Selection::System, IconName::Monitor, "theme.system"),
        (Selection::Light, IconName::Sun, "theme.light"),
        (Selection::Dark, IconName::Moon, "theme.dark"),
    ];

    let current_icon = move || match theme.selection.get() {
        Selection::System => IconName::Monitor,
        Selection::Light => IconName::Sun,
        Selection::Dark => IconName::Moon,
    };
    let label = locale.get("theme.label").to_owned();

    view! {
        <Menu aria_label=label icon=current_icon() trigger_variant=MenuTriggerVariant::Ghost align_end=true width="w-40">
            {options
                .into_iter()
                .map(|(choice, icon, key)| {
                    let text = locale.get(key).to_owned();
                    view! {
                        <MenuItem
                            label=text
                            icon=icon
                            on_select=Callback::new(move |()| theme.set(choice))
                        />
                    }
                })
                .collect_view()}
        </Menu>
    }
}

/// The interface language.
///
/// Writes the preference through `POST /api/locale`, which sets the `locale` cookie, and then reloads.
/// The reload is the point rather than a shortcut: [`crate::i18n`] resolves the locale once at startup
/// and holds it for the session on purpose, because a language that changes under a half-finished form
/// moves every label and button the operator was looking at. A reload makes the swap deliberate.
#[component]
fn LanguageMenu() -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let current = locale.tag.clone();
    let label = locale.get("language.label").to_owned();
    // The list is 35 entries; a menu is scrollable and the search-free scroll is acceptable because
    // the entries are ordered by their own name.
    let names = crate::i18n::LANGUAGE_NAMES;

    view! {
        <Menu
            aria_label=label
            icon=IconName::Globe
            trigger_variant=MenuTriggerVariant::Ghost
            align_end=true
            width="w-56"
        >
            <div class="max-h-80 overflow-y-auto">
                {names
                    .iter()
                    .map(|(tag, name)| {
                        let tag = *tag;
                        let selected = tag == current;
                        view! {
                            <button
                                type="button"
                                role="menuitem"
                                class="flex w-full items-center justify-between gap-2 rounded-md px-2 \
                                       py-1.5 text-left text-[0.8125rem] text-foreground outline-none \
                                       transition-colors hover:bg-accent hover:text-accent-foreground \
                                       focus-visible:ring-[3px] focus-visible:ring-ring/50"
                                aria-current=selected.then_some("true")
                                on:click=move |_| {
                                    leptos::task::spawn_local(async move {
                                        apply_language(tag).await;
                                    });
                                }
                            >
                                <span class="truncate">{*name}</span>
                                {selected
                                    .then(|| {
                                        view! { <Icon name=IconName::Check class="size-3.5 shrink-0" /> }
                                    })}
                            </button>
                        }
                    })
                    .collect_view()}
            </div>
        </Menu>
    }
}

/// Store the choice, then reload so the new catalogue is the one the session starts with.
///
/// The reload happens even when the write failed. The alternative is worse: the control already shows
/// the new language, so leaving the page on the old one puts the control and the interface into
/// disagreement with no way for the operator to tell which is real. A reload re-reads the cookie, so
/// whatever actually persisted is what appears.
#[cfg(target_arch = "wasm32")]
async fn apply_language(tag: &str) {
    if let Ok(body) = crate::api::encode(&serde_json::json!({ "locale": tag })) {
        let _ = crate::api::post("/api/locale", &body).await;
    }
    if let Some(window) = web_sys::window() {
        let _ = window.location().reload();
    }
}

/// Native builds have no document to reload.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::unused_async, reason = "mirrors the wasm signature")]
async fn apply_language(_tag: &str) {}

/// The signed-in account, and what may be done with it.
///
/// The role is shown rather than only enforced, because it is the answer to "why is that button
/// refusing me". A viewer who can see `Viewer` next to their name has an explanation; one who only
/// sees a 403 has a bug report.
///
/// Renders nothing until the status arrives. A placeholder name would be a guess, and this is the one
/// label on screen that must not be one.
#[component]
fn AccountMenu() -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let (status, set_status) = signal(Hydrate::<crate::routes::types::AuthStatus>::Loading);
    crate::api::load("/api/auth/status", set_status);

    // Built once, not inside the render closure below: `Menu`'s children is a `ChildrenFn`, so its
    // body runs again every time the menu opens, and a callback allocated per render would be a new
    // identity each time for no gain.
    let sign_out = Callback::new(move |()| {
        leptos::task::spawn_local(async move {
            // The session is a cookie, so the only thing that reliably ends it on this side is the
            // route that clears it. A client-side route change would leave the shell mounted with a
            // `/api/auth/status` that still says `authenticated: true`.
            let _ = crate::api::post("/api/auth/logout", "{}").await;
            if let Some(window) = web_sys::window() {
                let _ = window.location().set_href("/login");
            }
        });
    });

    view! {
        {move || {
            let Hydrate::Ready(status) = status.get() else {
                return None;
            };
            if !status.authenticated {
                // Signed out is not an error to report here; the router sends the visitor to
                // `/login` on the first request that refuses them.
                return None;
            }
            let display = if status.display_name.trim().is_empty() {
                locale.get("users.role_session").to_owned()
            } else {
                status.display_name.clone()
            };
            let role = match status.role.as_str() {
                "admin" => locale.get("users.role_admin_short").to_owned(),
                "operator" => locale.get("users.role_operator_short").to_owned(),
                _ => locale.get("users.role_viewer_short").to_owned(),
            };
            let initial = display
                .chars()
                .next().map_or_else(|| "?".to_owned(), |first| first.to_uppercase().to_string());
            let sign_out_label = locale.get("nav.sign_out").to_owned();
            let account_label = locale.get("nav.account").to_owned();

            Some(view! {
                <Menu
                    align_end=true
                    width="w-60"
                    trigger_variant=MenuTriggerVariant::Ghost
                    aria_label=account_label
                >
                    <div class="flex items-center gap-2.5 px-2 py-2">
                        <span class="grid size-8 shrink-0 place-items-center rounded-full bg-muted \
                                     text-xs font-semibold text-foreground">
                            {initial.clone()}
                        </span>
                        <span class="min-w-0">
                            <span class="block truncate text-[0.8125rem] font-medium text-foreground">
                                {display.clone()}
                            </span>
                            <span class="block truncate text-xs text-muted-foreground">
                                {role.clone()}
                            </span>
                        </span>
                    </div>
                    <MenuSeparator />
                    <MenuItem
                        label=sign_out_label.clone()
                        icon=IconName::LogOut
                        destructive=true
                        on_select=sign_out
                    />
                </Menu>
            })
        }}
    }
}

/// Whether the viewport is narrow enough that the rail becomes a drawer.
///
/// Read once at mount rather than subscribed to: the value is used to pick an initial state, and
/// re-reading it on every resize would fight the user's own collapse choice. The CSS breakpoints do the
/// responsive work; this only decides where things start.
#[allow(clippy::missing_const_for_fn)]
fn viewport_is_narrow() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|window| window.inner_width().ok())
            .and_then(|width| width.as_f64())
            .is_some_and(|width| width < 768.0)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{NAV_GROUPS, all_nav_items, item_for_path};

    #[test]
    fn every_nav_item_is_complete() {
        for (_, item) in all_nav_items() {
            assert!(!item.key.is_empty(), "{item:?} has no message key");
            assert!(
                item.path.starts_with("/dashboard"),
                "{item:?} escapes the dashboard"
            );
            assert!(
                item.key.starts_with("nav."),
                "{item:?} key should be namespaced"
            );
        }
    }

    #[test]
    fn nav_paths_are_unique() {
        // A duplicate path would light up two sidebar entries at once.
        for (index, (_, item)) in all_nav_items().enumerate() {
            let duplicate = all_nav_items()
                .skip(index + 1)
                .find(|(_, other)| other.path == item.path);
            assert!(duplicate.is_none(), "{:?} is listed twice", item.path);
        }
    }

    #[test]
    fn nav_keys_are_unique() {
        for (index, (_, item)) in all_nav_items().enumerate() {
            let duplicate = all_nav_items()
                .skip(index + 1)
                .find(|(_, other)| other.key == item.key);
            assert!(duplicate.is_none(), "{:?} is listed twice", item.key);
        }
    }

    #[test]
    fn every_group_has_items_and_a_namespaced_key() {
        for group in NAV_GROUPS {
            assert!(!group.items.is_empty(), "{} is empty", group.key);
            assert!(
                group.key.starts_with("nav."),
                "{} is not namespaced",
                group.key
            );
        }
    }

    #[test]
    fn group_keys_are_unique() {
        // Two groups sharing a key would render one heading twice.
        for (index, group) in NAV_GROUPS.iter().enumerate() {
            let duplicate = NAV_GROUPS
                .iter()
                .skip(index + 1)
                .find(|other| other.key == group.key);
            assert!(duplicate.is_none(), "{} is listed twice", group.key);
        }
    }

    #[test]
    fn the_root_is_listed_exactly_once() {
        // Active-state matching special-cases the root because it prefixes every other path. That only
        // works while there is exactly one root entry.
        let roots = all_nav_items()
            .filter(|(_, item)| item.path == "/dashboard")
            .count();
        assert_eq!(roots, 1);
    }

    #[test]
    fn every_destination_is_reachable_from_the_navigation() {
        // The breadcrumb and the page title both resolve through this table, so a destination missing
        // from it silently loses its heading and its section name.
        for path in [
            "/dashboard",
            "/dashboard/chat",
            "/dashboard/providers",
            "/dashboard/models",
            "/dashboard/combos",
            "/dashboard/pricing",
            "/dashboard/free-tiers",
            "/dashboard/keys",
            "/dashboard/users",
            "/dashboard/sso",
            "/dashboard/usage",
            "/dashboard/logs",
            "/dashboard/cli-tools",
            "/dashboard/pxpipe",
            "/dashboard/headroom",
            "/dashboard/tunnel",
            "/dashboard/proxy-pools",
            "/dashboard/nodes",
            "/dashboard/import",
            "/dashboard/translator",
            "/dashboard/catalog",
            "/dashboard/settings",
        ] {
            assert!(item_for_path(path).is_some(), "{path} has no nav entry");
        }
    }

    #[test]
    fn the_root_does_not_swallow_child_routes() {
        let (_, item) = item_for_path("/dashboard/keys").expect("keys is listed");
        assert_eq!(item.path, "/dashboard/keys");
        let (_, root) = item_for_path("/dashboard").expect("the root is listed");
        assert_eq!(root.path, "/dashboard");
        // A trailing slash and a query string are the same destination.
        let (_, trailing) = item_for_path("/dashboard/keys/").expect("a trailing slash resolves");
        assert_eq!(trailing.path, "/dashboard/keys");
        let (_, query) = item_for_path("/dashboard/keys?tab=1").expect("a query resolves");
        assert_eq!(query.path, "/dashboard/keys");
    }

    #[test]
    fn the_most_specific_entry_wins() {
        // `/dashboard/providers/openai` is the provider detail page: it belongs under Providers, not
        // under the dashboard root, even though both are prefixes of it.
        let (group, item) = item_for_path("/dashboard/providers/openai").expect("resolves");
        assert_eq!(item.path, "/dashboard/providers");
        assert_eq!(group.map(|group| group.key), Some("nav.group_routing"));
    }

    #[test]
    fn every_navigation_key_resolves_to_a_message() {
        // The sidebar reads its labels through a `key` field rather than a literal at the call site, so a
        // key missing from the catalogue does not fail to compile and is not caught by the `t!` scanner's
        // usual path. It renders as the raw key instead -- `nav.chat` in the sidebar, which is what
        // shipped before this test existed.
        let locale = crate::i18n::Locale::embedded();
        let mut keys: Vec<&str> = NAV_GROUPS.iter().map(|group| group.key).collect();
        keys.extend(all_nav_items().map(|(_, item)| item.key));

        for key in keys {
            assert_ne!(
                locale.get(key),
                key,
                "{key} is used by the navigation but missing from en-US.json"
            );
        }
    }

    #[test]
    fn an_unknown_path_resolves_to_nothing() {
        // The status fallback route renders for these, and a breadcrumb that invented a name for it
        // would be a guess.
        assert!(item_for_path("/nonsense").is_none());
    }
}
