//! The dashboard's sections.
//!
//! Every panel that reads server data goes through [`crate::ui::Panel`], which renders the three states
//! of [`crate::api::Hydrate`] and nothing else. That is what keeps the guarantee the API layer exists to
//! make: there is no path through this module that renders data the server did not send, and no path
//! that renders a failure as though it were an empty result.
//!
//! The presentation primitives live in [`crate::ui`]; this module re-exports the ones every route uses
//! so a page's import list names what it draws rather than where each piece is defined.

use leptos::prelude::*;

// Re-exported rather than reached for directly, so a route's imports stay about the page.
pub use crate::ui::{
    Alert, AlertDescription, AlertTitle, AlertVariant, Badge, BadgeVariant, Button, ButtonSize,
    ButtonVariant, Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle, Checkbox,
    ConnectionDot, Dialog, DialogClose, DialogContent, DialogDescription, DialogDismiss,
    DialogFooter, DialogHeader, DialogTitle, EmptyState, ErrorNote, Field, FieldDescription, Icon,
    IconName, Input, Kbd, Menu, MenuItem, MenuSeparator, Mono, PageActions, PageBody, PageHeader,
    Panel, PanelError, PanelSkeleton, RelativeTime, Select, Separator, Skeleton, Spinner,
    StatusDot, StatusTone, Switch, Table, TableCell, TableEmpty, TableFrame, TableHead,
    TableHeaderCell, TableRow, Tabs, Textarea, button_class,
};

/// Send a write and await it, keeping the server's own explanation when it refuses.
///
/// The `await`-shaped counterpart to [`crate::api::submit_reporting`], for a panel that drives its own
/// task rather than a [`crate::api::Save`] signal. Both extract the reason the same way, through
/// [`crate::api::refusal_message`], so a refusal reads identically whichever one reported it.
///
/// The `Err` is always displayable: the server's sentence when the body carried one, and the status
/// message when it did not.
pub async fn write_reporting(
    method: crate::api::Method,
    path: &str,
    body: Option<&str>,
) -> Result<String, String> {
    let response = crate::api::request_detailed(method, path, body)
        .await
        .map_err(|error| error.message().to_owned())?;

    if response.ok {
        return Ok(response.body);
    }
    Err(crate::api::refusal_message(&response))
}

/// Render a boolean the server reported as a badge.
///
/// One function rather than a ternary per panel, because the wording has to match everywhere: a page
/// that says "Enabled" while the next says "On" for the same field makes the reader check whether they
/// are looking at the same setting.
pub fn enabled_badge(locale: &crate::i18n::Locale, on: bool) -> impl IntoView + use<> {
    let label = if on {
        locale.get("state.enabled").to_owned()
    } else {
        locale.get("state.disabled").to_owned()
    };
    let variant = if on {
        BadgeVariant::Success
    } else {
        BadgeVariant::Secondary
    };
    view! { <Badge variant=variant>{label}</Badge> }
}

/// An em dash for a value the server did not send.
///
/// A blank cell reads as "this column is empty"; an em dash reads as "there is no value here", which is
/// what an absent field actually means.
pub fn or_dash(value: &str) -> String {
    if value.trim().is_empty() {
        "—".to_owned()
    } else {
        value.to_owned()
    }
}

pub mod catalog;
pub mod chat;
pub mod cli_tools;
pub mod combos;
pub mod controls;
pub mod free_tiers;
pub mod headroom;
pub mod keys;
pub mod login;
pub mod logs;
pub mod models;
pub mod oauth_import;
pub mod overview;
pub mod pricing;
pub mod provider_nodes;
pub mod providers;
pub mod proxy_pools;
pub mod pxpipe;
pub mod settings;
pub mod sso;
pub mod status;
pub mod translator;
pub mod tunnel;
pub mod types;
pub mod usage;
pub mod users;

pub use chat::Chat;
pub use cli_tools::CliTools;
pub use combos::Combos;
pub use free_tiers::FreeTiers;
pub use headroom::Headroom;
pub use keys::Keys;
pub use login::Login;
pub use logs::Logs;
pub use models::Models;
pub use overview::Overview;
pub use pricing::Pricing;
pub use provider_nodes::ProviderNodes;
pub use providers::Providers;
pub use proxy_pools::ProxyPools;
pub use pxpipe::Pxpipe;
pub use settings::Settings;
pub use sso::Sso;
pub use status::StatusPage;
pub use tunnel::Tunnel;
pub use usage::Usage;
pub use users::Users;

/// Shown for any path the router does not recognise.
#[component]
pub fn NotFound() -> impl IntoView {
    let locale = crate::i18n::use_locale();
    view! {
        <div class="grid min-h-dvh place-items-center bg-background p-6">
            <div class="space-y-3 text-center">
                <p class="font-mono text-5xl font-semibold tracking-tight text-muted-foreground/40">
                    "404"
                </p>
                <h1 class="text-lg font-semibold tracking-tight">
                    {locale.get("not_found.title").to_owned()}
                </h1>
                <p class="max-w-sm text-sm text-muted-foreground">
                    {locale.get("not_found.copy").to_owned()}
                </p>
                <a
                    href="/dashboard"
                    class=button_class(ButtonVariant::Outline, ButtonSize::Sm)
                >
                    {locale.get("nav.dashboard").to_owned()}
                </a>
            </div>
        </div>
    }
}

/// OAuth callback mount: the grant stays in the address bar for the opener to read.
#[component]
pub fn Callback() -> impl IntoView {
    let locale = crate::i18n::use_locale();
    view! {
        <div class="grid min-h-dvh place-items-center bg-background p-6">
            <section class="w-full max-w-sm space-y-3 rounded-xl border border-border bg-card p-6 text-center shadow-xs">
                <div class="mx-auto grid size-10 place-items-center rounded-full border border-border bg-muted/50 text-success">
                    <Icon name=IconName::Check class="size-5" />
                </div>
                <h1 class="text-base font-semibold tracking-tight">
                    {locale.get("callback.title").to_owned()}
                </h1>
                <p class="text-sm leading-relaxed text-muted-foreground">
                    {locale.get("callback.copy").to_owned()}
                </p>
            </section>
        </div>
    }
}
