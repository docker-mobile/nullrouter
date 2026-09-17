//! The application frame: sidebar, header, and the outlet routes render into.
//!
//! The old dashboard's shell was the clearest symptom of what went wrong with it. Navigation,
//! layout, and per-panel styling were each maintained in a different place, so adding a section
//! meant touching three files that had drifted apart, and the account menu ended up shipping a
//! Theme entry wired to nothing. This version keeps the whole frame in one place and derives
//! navigation from a single list, so a new section is one entry rather than three edits.

use leptos::prelude::*;
use leptos_router::components::{A, Outlet};
use leptos_router::hooks::use_location;

use crate::theme::{Selection, use_theme};

/// One destination in the sidebar.
///
/// `icon` is an inline SVG path, not an icon-font ligature. The old shell pulled a Material Icons
/// webfont for this, which cost a render-blocking request and a flash of unstyled glyphs on every
/// cold load, to draw about fifteen shapes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NavItem {
    /// Message key for the label.
    pub key: &'static str,
    /// Route path.
    pub path: &'static str,
    /// SVG path data, drawn in a 24x24 viewBox.
    pub icon: &'static str,
}

/// Every sidebar destination, in order.
///
/// The single source of navigation truth. The sidebar renders it, and adding a section here is all
/// that is needed for it to appear.
pub const NAV_ITEMS: &[NavItem] = &[
    NavItem {
        key: "nav.dashboard",
        path: "/dashboard",
        icon: "M3 13h8V3H3v10zm0 8h8v-6H3v6zm10 0h8V11h-8v10zm0-18v6h8V3h-8z",
    },
    NavItem {
        key: "nav.chat",
        path: "/dashboard/chat",
        icon: "M20 2H4c-1.1 0-2 .9-2 2v18l4-4h14c1.1 0 2-.9 2-2V4c0-1.1-.9-2-2-2zm-2 12H6v-2h12v2zm0-3H6V9h12v2zm0-3H6V6h12v2z",
    },
    NavItem {
        key: "nav.providers",
        path: "/dashboard/providers",
        icon: "M4 6h16v2H4V6zm0 5h16v2H4v-2zm0 5h16v2H4v-2z",
    },
    NavItem {
        key: "nav.models",
        path: "/dashboard/models",
        icon: "M11.99 18.54l-7.37-5.73L3 14.07l9 7 9-7-1.63-1.27-7.38 5.74zM12 16l7.36-5.73L21 9l-9-7-9 7 1.63 1.27L12 16z",
    },
    NavItem {
        key: "nav.combos",
        path: "/dashboard/combos",
        icon: "M4 4h7v7H4V4zm9 0h7v7h-7V4zM4 13h7v7H4v-7zm9 0h7v7h-7v-7z",
    },
    NavItem {
        key: "nav.pricing",
        path: "/dashboard/pricing",
        icon: "M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm1.41 16.09V20h-2.67v-1.93c-1.71-.36-3.16-1.46-3.27-3.4h1.96c.1 1.05.82 1.87 2.65 1.87 1.96 0 2.4-.98 2.4-1.59 0-.83-.44-1.61-2.67-2.14-2.48-.6-4.18-1.62-4.18-3.67 0-1.72 1.39-2.84 3.11-3.21V4h2.67v1.95c1.86.45 2.79 1.86 2.85 3.39H14.3c-.05-1.11-.64-1.87-2.22-1.87-1.5 0-2.4.68-2.4 1.64 0 .84.65 1.39 2.67 1.91s4.18 1.39 4.18 3.91c-.01 1.83-1.38 2.83-3.12 3.16z",
    },
    NavItem {
        key: "nav.free_tiers",
        path: "/dashboard/free-tiers",
        icon: "M12 2l3.09 6.26L22 9.27l-5 4.87 1.18 6.88L12 17.77l-6.18 3.25L7 14.14 2 9.27l6.91-1.01L12 2z",
    },
    NavItem {
        key: "nav.keys",
        path: "/dashboard/keys",
        icon: "M12.65 10A5.99 5.99 0 0 0 7 6c-3.31 0-6 2.69-6 6s2.69 6 6 6a5.99 5.99 0 0 0 5.65-4H17v4h4v-4h2v-4H12.65zM7 14c-1.1 0-2-.9-2-2s.9-2 2-2 2 .9 2 2-.9 2-2 2z",
    },
    NavItem {
        key: "nav.users",
        path: "/dashboard/users",
        icon: "M16 11c1.66 0 3-1.34 3-3s-1.34-3-3-3-3 1.34-3 3 1.34 3 3 3zm-8 0c1.66 0 3-1.34 3-3S9.66 5 8 5 5 6.34 5 8s1.34 3 3 3zm0 2c-2.33 0-7 1.17-7 3.5V19h14v-2.5c0-2.33-4.67-3.5-7-3.5zm8 0c-.29 0-.62.02-.97.05 1.16.84 1.97 1.97 1.97 3.45V19h6v-2.5c0-2.33-4.67-3.5-7-3.5z",
    },
    NavItem {
        key: "nav.usage",
        path: "/dashboard/usage",
        icon: "M5 9.2h3V19H5zM10.6 5h2.8v14h-2.8zm5.6 8H19v6h-2.8z",
    },
    NavItem {
        key: "nav.logs",
        path: "/dashboard/logs",
        icon: "M20 2H4c-1.1 0-2 .9-2 2v16l4-4h14c1.1 0 2-.9 2-2V4c0-1.1-.9-2-2-2zM6 9h12v2H6V9zm8 5H6v-2h8v2zm4-6H6V6h12v2z",
    },
    NavItem {
        key: "nav.cli_tools",
        path: "/dashboard/cli-tools",
        icon: "M20 4H4c-1.1 0-2 .9-2 2v12c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V6c0-1.1-.9-2-2-2zm0 14H4V8h16v10zM6.5 9.5 5 11l2.5 2.5L5 16l1.5 1.5L10.5 13 6.5 9.5zM12 16h6v-1.5h-6V16z",
    },
    NavItem {
        key: "nav.pxpipe",
        path: "/dashboard/pxpipe",
        icon: "M17 4h-3V2h-4v2H7v6l3 3v5h4v-5l3-3V4zm-2 5.2-3 3-3-3V6h6v3.2zM11 20h2v2h-2v-2z",
    },
    NavItem {
        key: "nav.headroom",
        path: "/dashboard/headroom",
        icon: "M12 2 4 6v6c0 4.4 3.4 8.5 8 10 4.6-1.5 8-5.6 8-10V6l-8-4zm0 2.2 6 3v4.8c0 3.4-2.5 6.6-6 7.9-3.5-1.3-6-4.5-6-7.9V7.2l6-3zM9 11h6v2H9v-2zm0-3h6v2H9V8z",
    },
    NavItem {
        key: "nav.tunnel",
        path: "/dashboard/tunnel",
        icon: "M12 2C8.13 2 5 5.13 5 9c0 5.25 7 13 7 13s7-7.75 7-13c0-3.87-3.13-7-7-7zm0 9.5a2.5 2.5 0 010-5 2.5 2.5 0 010 5z",
    },
    NavItem {
        key: "nav.proxy_pools",
        path: "/dashboard/proxy-pools",
        icon: "M4 4h4v4H4V4zm6 0h4v4h-4V4zm6 0h4v4h-4V4zM4 10h4v4H4v-4zm6 0h4v4h-4v-4zm6 0h4v4h-4v-4zM4 16h4v4H4v-4zm6 0h4v4h-4v-4zm6 0h4v4h-4v-4z",
    },
    NavItem {
        key: "nav.nodes",
        path: "/dashboard/nodes",
        icon: "M12 2l4 4-4 4-4-4 4-4zm-8 8l4 4-4 4-4-4 4-4zm16 0l4 4-4 4-4-4 4-4zm-8 8l4 4-4 4-4-4 4-4z",
    },
    NavItem {
        key: "nav.import",
        path: "/dashboard/import",
        icon: "M19 9h-4V3H9v6H5l7 7 7-7zM5 18v2h14v-2H5z",
    },
    NavItem {
        key: "nav.translator",
        path: "/dashboard/translator",
        icon: "M12.87 15.07l-2.54-2.51.03-.03A17.52 17.52 0 0014.07 6H17V4h-7V2H8v2H1v2h11.17C11.5 7.92 10.44 9.75 9 11.35 8.07 10.32 7.3 9.19 6.69 8h-2c.73 1.63 1.73 3.17 2.98 4.56l-5.09 5.02L4 19l5-5 3.11 3.11.76-2.04zM18.5 10h-2L12 22h2l1.12-3h4.75L21 22h2l-4.5-12zm-2.62 7l1.62-4.33L19.12 17h-3.24z",
    },
    NavItem {
        key: "nav.catalog",
        path: "/dashboard/catalog",
        icon: "M3 5h18v2H3V5zm0 6h18v2H3v-2zm0 6h12v2H3v-2z",
    },
    NavItem {
        key: "nav.sso",
        path: "/dashboard/sso",
        icon: "M12 1 3 5v6c0 5.55 3.84 10.74 9 12 5.16-1.26 9-6.45 9-12V5l-9-4zm0 10.99h7c-.53 4.12-3.28 7.79-7 8.94V12H5V6.3l7-3.11v8.8z",
    },
    NavItem {
        key: "nav.settings",
        path: "/dashboard/settings",
        icon: "M19.14 12.94c.04-.3.06-.61.06-.94 0-.32-.02-.64-.07-.94l2.03-1.58a.49.49 0 0 0 .12-.61l-1.92-3.32a.488.488 0 0 0-.59-.22l-2.39.96c-.5-.38-1.03-.7-1.62-.94l-.36-2.54a.484.484 0 0 0-.48-.42h-3.84c-.24 0-.44.17-.47.41l-.36 2.54c-.59.24-1.13.57-1.62.94l-2.39-.96c-.22-.08-.47 0-.59.22L2.74 8.87c-.12.21-.08.47.12.61l2.03 1.58c-.05.3-.09.63-.09.94s.02.64.07.94l-2.03 1.58a.49.49 0 0 0-.12.61l1.92 3.32c.12.22.37.29.59.22l2.39-.96c.5.38 1.03.7 1.62.94l.36 2.54c.05.24.25.41.49.41h3.84c.24 0 .44-.17.47-.41l.36-2.54c.59-.24 1.13-.56 1.62-.94l2.39.96c.22.08.47 0 .59-.22l1.92-3.32c.12-.22.07-.47-.12-.61l-2.01-1.58zM12 15.6a3.6 3.6 0 1 1 0-7.2 3.6 3.6 0 0 1 0 7.2z",
    },
];

/// The frame nested dashboard routes render inside.
#[component]
pub fn DashboardFrame() -> impl IntoView {
    view! {
        <Shell>
            <Outlet />
        </Shell>
    }
}

/// The frame every route renders inside.
#[component]
pub fn Shell(children: Children) -> impl IntoView {
    // Collapse state is local rather than persisted: the sidebar is a per-session affordance, and
    // restoring a collapsed sidebar on a fresh visit hides navigation from someone who has no
    // reason to expect it.
    let (collapsed, set_collapsed) = signal(viewport_is_narrow());

    view! {
        <div class="min-h-dvh bg-background text-foreground flex">
            {move || {
                (!collapsed.get())
                    .then(|| {
                        view! {
                            <button
                                type="button"
                                class="fixed inset-0 z-20 bg-black/40 md:hidden"
                                aria-label="Close navigation"
                                on:click=move |_| set_collapsed.set(true)
                            />
                        }
                    })
            }}
            <Sidebar collapsed=collapsed />
            <div class="flex-1 flex flex-col min-w-0">
                <Header collapsed=collapsed set_collapsed=set_collapsed />
                // min-w-0 on both this and the wrapper above: without it a wide table inside a flex
                // child refuses to shrink and pushes the whole layout sideways instead of scrolling.
                <main class="flex-1 min-w-0 p-6 animate-in fade-in duration-200">
                    {children()}
                </main>
            </div>
        </div>
    }
}

/// Primary navigation.
#[component]
fn Sidebar(collapsed: ReadSignal<bool>) -> impl IntoView {
    let location = use_location();

    view! {
        <aside class=move || {
            let collapsed = collapsed.get();
            let width = if collapsed { "w-16" } else { "w-60" };
            let hidden = if collapsed { "max-md:hidden" } else { "" };
            format!(
                "{width} shrink-0 border-r border-sidebar-border bg-sidebar \
                 transition-[width] duration-200 ease-out flex flex-col \
                 max-md:fixed max-md:inset-y-0 max-md:left-0 max-md:z-30 \
                 {hidden}",
            )
        }>
            <div class="h-14 flex items-center gap-2.5 px-4 shrink-0">
                <Logo />
                <span class=move || {
                    // Fade the wordmark rather than removing it, so the collapse reads as one motion
                    // instead of text vanishing a frame before the panel finishes narrowing.
                    let hidden = if collapsed.get() { "opacity-0 w-0" } else { "opacity-100" };
                    format!("{hidden} font-semibold tracking-tight overflow-hidden whitespace-nowrap transition-opacity duration-150")
                }>
                    "nullrouter"
                </span>
            </div>

            <nav class="flex-1 px-2 py-2 space-y-0.5 overflow-y-auto">
                {NAV_ITEMS
                    .iter()
                    .map(|item| {
                        view! { <NavLink item=*item collapsed=collapsed location=location.pathname /> }
                    })
                    .collect_view()}
            </nav>
        </aside>
    }
}

/// The brand mark: three lanes in, one decision node, one lane out, struck through.
///
/// The same geometry as `assets/favicon.svg`, so the tab icon and the sidebar agree. Drawn inline
/// rather than loaded as an `<img>` because it has to take the theme's colours -- the plate is
/// `bg-primary` and the lanes are `primary-foreground`, which no external SVG can read.
///
/// This replaces a plate with the letters "nr" in it. That version resolved a different sans-serif on
/// every platform and, at the 28px it is drawn here, the two glyphs merged into a smudge.
#[component]
fn Logo() -> impl IntoView {
    view! {
        <div class="size-7 rounded-md bg-primary grid place-items-center shrink-0">
            <svg
                class="size-5 text-primary-foreground"
                viewBox="0 0 32 32"
                fill="none"
                aria-hidden="true"
            >
                <g
                    stroke="currentColor"
                    stroke-width="2.4"
                    stroke-linecap="round"
                >
                    <path d="M5 9h6l4 4" />
                    <path d="M5 16h6" />
                    <path d="M5 23h6l4-4" />
                    <path d="M20 16h7" />
                </g>
                <circle cx="16" cy="16" r="3.4" fill="currentColor" />
                <path
                    d="M12.2 19.8 19.8 12.2"
                    stroke="var(--color-primary, currentColor)"
                    stroke-width="2.4"
                    stroke-linecap="round"
                    class="text-primary"
                />
            </svg>
        </div>
    }
}

/// One sidebar link.
#[component]
fn NavLink(item: NavItem, collapsed: ReadSignal<bool>, location: Memo<String>) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let label = locale.get(item.key).to_owned();
    // The tooltip is what carries the label when the sidebar is collapsed, so it needs its own copy.
    let title = label.clone();

    // The dashboard root would otherwise match every child route as a prefix and light up alongside
    // whichever section is actually open.
    let active = Memo::new(move |_| {
        let path = location.get();
        if item.path == "/dashboard" {
            path == "/dashboard" || path == "/dashboard/"
        } else {
            path.starts_with(item.path)
        }
    });

    view! {
        <A
            href=item.path
            attr:class=move || {
                let state = if active.get() {
                    "bg-sidebar-accent text-sidebar-accent-foreground"
                } else {
                    "text-sidebar-foreground/70 hover:bg-sidebar-accent/60 hover:text-sidebar-accent-foreground"
                };
                format!(
                    "{state} group relative flex items-center gap-3 rounded-md px-3 py-2 \
                     text-sm font-medium transition-colors duration-150"
                )
            }
            attr:title=title
        >
            <svg
                class="size-5 shrink-0"
                viewBox="0 0 24 24"
                fill="currentColor"
                aria-hidden="true"
            >
                <path d=item.icon />
            </svg>
            <span class=move || {
                let hidden = if collapsed.get() { "opacity-0 w-0" } else { "opacity-100" };
                format!("{hidden} overflow-hidden whitespace-nowrap transition-opacity duration-150")
            }>
                {label.clone()}
            </span>
        </A>
    }
}

/// Top bar: sidebar toggle and theme control.
#[component]
fn Header(collapsed: ReadSignal<bool>, set_collapsed: WriteSignal<bool>) -> impl IntoView {
    view! {
        // sticky rather than fixed so it does not need the content below to carry a matching top
        // offset, and backdrop-blur so content scrolling under it stays legible.
        <header class="h-14 shrink-0 sticky top-0 z-30 flex items-center gap-2 px-4 \
                       border-b border-border bg-background/80 backdrop-blur-sm">
            <button
                type="button"
                class="grid size-9 place-items-center rounded-md text-muted-foreground \
                       transition-colors hover:bg-accent hover:text-accent-foreground"
                on:click=move |_| set_collapsed.update(|value| *value = !*value)
                aria-label="Toggle navigation"
                aria-expanded=move || (!collapsed.get()).to_string()
            >
                <svg class="size-5" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
                    <path d="M3 18h18v-2H3v2zm0-5h18v-2H3v2zm0-7v2h18V6H3z" />
                </svg>
            </button>

            <div class="flex-1" />

            <AccountBadge />
            <LanguagePicker />
            <ThemeToggle />
        </header>
    }
}

/// Who is signed in, and what they may do.
///
/// The role is shown rather than only used, because it is the answer to "why is that button refusing
/// me". A viewer who can see `Viewer` next to their name has an explanation; one who only sees a 403
/// has a bug report.
///
/// Renders nothing until the status arrives. A placeholder name would be a guess, and this is the one
/// label on screen that must not be one.
#[component]
fn AccountBadge() -> impl IntoView {
    use crate::api::{Hydrate, load};
    use crate::routes::types::AuthStatus;

    let locale = crate::i18n::use_locale();
    let (status, set_status) = signal(Hydrate::<AuthStatus>::Loading);
    load("/api/auth/status", set_status);

    view! {
        {move || {
            let Hydrate::Ready(status) = status.get() else {
                return None;
            };
            if !status.authenticated {
                return None;
            }
            let name = if status.display_name.is_empty() {
                locale.get("users.never").to_owned()
            } else {
                status.display_name.clone()
            };
            // Literal keys, because `i18n-gen` finds keys by scanning for `get("`.
            let role = match status.role.as_str() {
                "admin" => locale.get("users.role_admin").to_owned(),
                "operator" => locale.get("users.role_operator").to_owned(),
                _ => locale.get("users.role_viewer").to_owned(),
            };
            // The long form names what the role means; the badge shows only the word, since the row is
            // narrow and the full sentence belongs in a tooltip.
            let short = status.role;
            Some(view! {
                <div class="hidden sm:flex flex-col items-end leading-tight" title=role>
                    <span class="text-xs font-medium">{name}</span>
                    <span class="text-[11px] uppercase tracking-wide text-muted-foreground">
                        {short}
                    </span>
                </div>
            })
        }}
    }
}

/// Picks the interface language.
///
/// Writes the preference through `POST /api/locale`, which sets the `locale` cookie, and then reloads.
/// The reload is the point rather than a shortcut: [`crate::i18n`] resolves the locale once at startup
/// and holds it for the session on purpose, because a language that changes under a half-finished form
/// moves every label and button the operator was looking at. Reloading makes the swap a deliberate,
/// visible transition instead of a surprise.
///
/// A native `<select>` rather than a custom menu: it gets keyboard behaviour, scrolling for 35 items,
/// and the platform's own type-ahead for free, and this list is long enough that type-ahead matters.
#[component]
fn LanguagePicker() -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let current = locale.tag.clone();
    let label = locale.get("language.change").to_owned();

    view! {
        <label class="relative">
            <span class="sr-only">{locale.get("language.label").to_owned()}</span>
            <select
                class="h-9 rounded-md border border-input bg-background px-2 text-sm \
                       text-muted-foreground transition-colors hover:text-foreground"
                title=label.clone()
                aria-label=label
                prop:value=current.clone()
                on:change=move |ev| {
                    let tag = event_target_value(&ev);
                    if tag.is_empty() {
                        return;
                    }
                    leptos::task::spawn_local(async move { apply_language(&tag).await });
                }
            >
                {crate::i18n::LANGUAGE_NAMES
                    .iter()
                    .map(|(tag, name)| {
                        let selected = *tag == current;
                        view! {
                            <option value=*tag selected=selected>
                                {*name}
                            </option>
                        }
                    })
                    .collect_view()}
            </select>
        </label>
    }
}

/// Store the choice, then reload so the new catalogue is the one the session starts with.
///
/// The reload happens even when the write failed. The alternative is worse: the select already shows
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
#[allow(clippy::unused_async)]
async fn apply_language(_tag: &str) {}

/// Cycles System → Light → Dark.
///
/// The control the old dashboard shipped as `enabled: false`. Showing the *resolved* icon rather
/// than the selection means System is not a third mystery glyph: it shows whichever scheme it
/// currently resolves to, and the tooltip names the selection.
#[component]
fn ThemeToggle() -> impl IntoView {
    let theme = use_theme();
    let locale = crate::i18n::use_locale();

    let title = {
        let system = locale.get("theme.system").to_owned();
        let light = locale.get("theme.light").to_owned();
        let dark = locale.get("theme.dark").to_owned();
        move || match theme.selection.get() {
            Selection::System => system.clone(),
            Selection::Light => light.clone(),
            Selection::Dark => dark.clone(),
        }
    };

    view! {
        <button
            type="button"
            class="grid size-9 place-items-center rounded-md text-muted-foreground \
                   transition-colors hover:bg-accent hover:text-accent-foreground"
            on:click=move |_| theme.cycle()
            title=title
            aria-label="Change theme"
        >
            <svg
                class="size-5 transition-transform duration-300"
                class:rotate-180=move || theme.resolved.get().is_dark()
                viewBox="0 0 24 24"
                fill="currentColor"
                aria-hidden="true"
            >
                {move || {
                    if theme.resolved.get().is_dark() {
                        view! {
                            <path d="M12 3a9 9 0 1 0 9 9c0-.46-.04-.92-.1-1.36a5.389 5.389 0 0 1-4.4 2.26 5.403 5.403 0 0 1-3.14-9.8c-.44-.06-.9-.1-1.36-.1z" />
                        }
                            .into_any()
                    } else {
                        view! {
                            <path d="M12 7c-2.76 0-5 2.24-5 5s2.24 5 5 5 5-2.24 5-5-2.24-5-5-5zM2 13h2c.55 0 1-.45 1-1s-.45-1-1-1H2c-.55 0-1 .45-1 1s.45 1 1 1zm18 0h2c.55 0 1-.45 1-1s-.45-1-1-1h-2c-.55 0-1 .45-1 1s.45 1 1 1zM11 2v2c0 .55.45 1 1 1s1-.45 1-1V2c0-.55-.45-1-1-1s-1 .45-1 1zm0 18v2c0 .55.45 1 1 1s1-.45 1-1v-2c0-.55-.45-1-1-1s-1 .45-1 1zM5.99 4.58a.996.996 0 0 0-1.41 0 .996.996 0 0 0 0 1.41l1.06 1.06c.39.39 1.03.39 1.41 0s.39-1.03 0-1.41L5.99 4.58zm12.37 12.37a.996.996 0 0 0-1.41 0 .996.996 0 0 0 0 1.41l1.06 1.06c.39.39 1.03.39 1.41 0a.996.996 0 0 0 0-1.41l-1.06-1.06zm1.06-10.96a.996.996 0 0 0 0-1.41.996.996 0 0 0-1.41 0l-1.06 1.06c-.39.39-.39 1.03 0 1.41s1.03.39 1.41 0l1.06-1.06zM7.05 18.36a.996.996 0 0 0 0-1.41.996.996 0 0 0-1.41 0l-1.06 1.06c-.39.39-.39 1.03 0 1.41s1.03.39 1.41 0l1.06-1.06z" />
                        }
                            .into_any()
                    }
                }}
            </svg>
        </button>
    }
}

#[allow(clippy::missing_const_for_fn)]
fn viewport_is_narrow() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|window| window.inner_width().ok())
            .and_then(|width| width.as_f64())
            .map_or(false, |width| width < 768.0)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::NAV_ITEMS;

    #[test]
    fn every_nav_item_is_complete() {
        for item in NAV_ITEMS {
            assert!(!item.key.is_empty(), "{item:?} has no message key");
            assert!(
                item.path.starts_with("/dashboard"),
                "{item:?} escapes the dashboard"
            );
            assert!(!item.icon.is_empty(), "{item:?} has no icon");
            assert!(
                item.key.starts_with("nav."),
                "{item:?} key should be namespaced"
            );
        }
    }

    #[test]
    fn nav_paths_are_unique() {
        // A duplicate path would light up two sidebar entries at once.
        for (index, item) in NAV_ITEMS.iter().enumerate() {
            let duplicate = NAV_ITEMS
                .iter()
                .skip(index + 1)
                .find(|other| other.path == item.path);
            assert!(duplicate.is_none(), "{:?} is listed twice", item.path);
        }
    }

    #[test]
    fn nav_keys_are_unique() {
        for (index, item) in NAV_ITEMS.iter().enumerate() {
            let duplicate = NAV_ITEMS
                .iter()
                .skip(index + 1)
                .find(|other| other.key == item.key);
            assert!(duplicate.is_none(), "{:?} is listed twice", item.key);
        }
    }

    #[test]
    fn the_root_is_listed_exactly_once() {
        // Active-state matching special-cases the root because it prefixes every other path. That
        // only works while there is exactly one root entry.
        let roots = NAV_ITEMS
            .iter()
            .filter(|item| item.path == "/dashboard")
            .count();
        assert_eq!(roots, 1);
    }
}
