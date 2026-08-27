mod header;
mod sidebar;

pub(crate) use header::Header;
pub(crate) use sidebar::Sidebar;

use crate::ui::DashboardRoute;
use leptos::prelude::*;

const VISIBLE_CONTRACT: &[&str] = &[
    "nr-sidebar-desktop",
    "nr-sidebar-drawer",
    "nr-sidebar-overlay",
    "nr-sidebar-open",
    "aria-label=\"Open dashboard navigation\"",
    "aria-label=\"Close mobile dashboard navigation\"",
    "title=\"Open dashboard navigation\"",
    "title=\"Close mobile dashboard navigation\"",
    "aria-controls=\"nr-mobile-sidebar\"",
    "aria-expanded",
    "id=\"nr-mobile-sidebar\"",
    "data-state=\"open\"",
    "data-state=\"closed\"",
    "aria-hidden=\"true\"",
    "material-symbols-outlined",
    "menu",
    "9Router Proxy",
    "v0.5.20",
    "hub",
    "nr-media-navigation",
    "aria-label=\"Toggle media providers\"",
    "nr-media-nav-item",
    "id=\"nr-header-search\"",
    "id=\"nr-header-language\"",
    "id=\"nr-header-account\"",
    "aria-label=\"Search dashboard\"",
    "aria-label=\"Language\"",
    "aria-label=\"Open account menu\"",
    "aria-haspopup=\"dialog\"",
    "aria-haspopup=\"menu\"",
    "No destinations found",
    "nr-header-popover-dismiss",
    "data-header-panel",
];

pub const fn dashboard_shell_visible_contract() -> &'static [&'static str] {
    VISIBLE_CONTRACT
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum HeaderPanel {
    #[default]
    Closed,
    Search,
    Language,
    Account,
}

impl HeaderPanel {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Search => "search",
            Self::Language => "language",
            Self::Account => "account",
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct ShellSignals {
    pub(super) active: ReadSignal<DashboardRoute>,
    pub(super) set_active: WriteSignal<DashboardRoute>,
    pub(super) drawer_open: ReadSignal<bool>,
    pub(super) set_drawer_open: WriteSignal<bool>,
    pub(super) header_panel: ReadSignal<HeaderPanel>,
    pub(super) set_header_panel: WriteSignal<HeaderPanel>,
}

#[cfg(target_arch = "wasm32")]
pub(super) fn focus_sidebar_trigger() {
    use wasm_bindgen::JsCast;

    if let Some(trigger) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id("nr-sidebar-open"))
        .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
    {
        let _focus_restored = trigger.focus();
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) const fn focus_sidebar_trigger() {}
