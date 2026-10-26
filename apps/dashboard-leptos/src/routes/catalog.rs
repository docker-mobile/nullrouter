//! The gateway's routing map, as the running router reports it.
//!
//! A diagnostics page, not a settings page: nothing here is writable. It answers the two questions
//! that are otherwise guesswork when a request lands somewhere unexpected -- which service actually
//! serves a path, and whether the gateway rewrites the prefix on the way there -- and it answers
//! them from the router's own catalogue rather than from a list maintained by hand beside it.

use leptos::prelude::*;

use crate::t;

use crate::api::{Hydrate, load};
use crate::ui::{
    Badge, BadgeVariant, Card, CardContent, CardHeader, CardTitle, EmptyState, IconName, PageBody,
    PageHeader, Panel, StatusDot, StatusTone, Tabs,
};

/// One upstream and every path the gateway routes to it.
#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RouteFamily {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    /// The service that answers, e.g. `nullrouter-api`.
    #[serde(default)]
    upstream: String,
    /// The prefix a client calls.
    #[serde(default)]
    gateway_prefix: String,
    /// What the upstream itself sees. Equal to `gateway_prefix` unless the gateway rewrites.
    #[serde(default)]
    source_prefix: String,
    #[serde(default)]
    routes: Vec<String>,
}

#[derive(Clone, Debug, Default, serde::Deserialize)]
struct RouteCatalog {
    #[serde(default)]
    families: Vec<RouteFamily>,
}

/// Connection counts for one provider, as the catalogue reports them.
#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderStatus {
    #[serde(default)]
    connected: u32,
    #[serde(default)]
    error: u32,
    #[serde(default)]
    total: u32,
    /// The router's own one-word verdict, e.g. `Idle`.
    #[serde(default)]
    health: String,
}

#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderCard {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    auth_label: String,
    /// A colour the catalogue publishes for the provider. Checked before it reaches an attribute.
    #[serde(default)]
    accent: String,
    #[serde(default)]
    status: ProviderStatus,
}

/// `GET /api/catalog/providers` also carries `models` and `openaiModels`. Both are read by the
/// models panel from `/api/models`, so they are deliberately not decoded here: two panels deriving
/// the same list from different endpoints is how they come to disagree.
#[derive(Clone, Debug, Default, serde::Deserialize)]
struct ProviderCatalog {
    #[serde(default)]
    providers: Vec<ProviderCard>,
}

#[component]
pub fn Catalog() -> impl IntoView {
    let (routes, set_routes) = signal(Hydrate::<RouteCatalog>::Loading);
    let (providers, set_providers) = signal(Hydrate::<ProviderCatalog>::Loading);

    let reload = move || {
        set_routes.set(Hydrate::Loading);
        set_providers.set(Hydrate::Loading);
        load("/api/catalog/routes", set_routes);
        load("/api/catalog/providers", set_providers);
    };
    reload();

    // One page, two views of the same question. Tabs rather than two stacked sections: both lists are
    // long, and stacking them means scrolling past 40 route families to reach a provider.
    let (tab, set_tab) = signal(0usize);
    // Resolved once, not inside a `move ||`: the `view!` that builds the tab strip moves its
    // labels, and a second read for the body closure below would consume them.
    let routes_label = t!("catalog.routes_title");
    let providers_label = t!("catalog.providers_title");
    let tab_labels = Signal::derive(move || vec![routes_label.clone(), providers_label.clone()]);
    let selected = Signal::derive(move || tab.get());

    let heading = t!("nav.catalog");
    let blurb = t!("catalog.description");

    view! {
        <PageHeader title=heading description=blurb />

        <PageBody>
            <Tabs
                labels=tab_labels
                selected=selected
                on_select=Callback::new(move |index| set_tab.set(index))
            />

            {move || {
                if tab.get() == 0 {
                    view! {
                        <Panel
                            state=routes
                            on_retry=Callback::new(move |()| reload())
                            children=move |data: RouteCatalog| view! { <Families rows=data.families /> }
                        />
                    }
                        .into_any()
                } else {
                    view! {
                        <Panel
                            state=providers
                            on_retry=Callback::new(move |()| reload())
                            children=move |data: ProviderCatalog| view! { <Providers rows=data.providers /> }
                        />
                    }
                        .into_any()
                }
            }}
        </PageBody>
    }
}

#[component]
fn Families(rows: Vec<RouteFamily>) -> impl IntoView {
    if rows.is_empty() {
        return view! {
            <Card>
                <EmptyState
                    title=t!("catalog.no_routes")
                    icon=IconName::Route
                />
            </Card>
        }
        .into_any();
    }
    view! {
        <div class="grid gap-3 xl:grid-cols-2">
            {rows.into_iter().map(|family| view! { <Family family=family /> }).collect_view()}
        </div>
    }
    .into_any()
}

#[component]
fn Family(family: RouteFamily) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    // Worth calling out rather than leaving to be spotted: a rewritten prefix is the reason a path
    // that exists on the upstream 404s at the gateway, and the reverse.
    let rewritten = family.gateway_prefix != family.source_prefix;
    let count = family.routes.len().to_string();

    let title = family.title.clone();
    let id = family.id.clone();
    let route_count = locale.fmt("catalog.route_count", &[("count", &count)]);
    let upstream_label = t!("catalog.upstream");
    let prefix_label = if rewritten {
        t!("catalog.prefix_rewritten")
    } else {
        t!("catalog.prefix")
    };
    let upstream = family.upstream.clone();
    let prefix = if rewritten {
        format!(
            "{} \u{2192} {}",
            family.gateway_prefix, family.source_prefix
        )
    } else {
        family.gateway_prefix.clone()
    };

    view! {
        <Card>
            <CardHeader>
                <div class="min-w-0 space-y-1">
                    <CardTitle class="truncate">{title}</CardTitle>
                    <p class="truncate font-mono text-xs text-muted-foreground">{id}</p>
                </div>
                <Badge variant=BadgeVariant::Secondary>{route_count}</Badge>
            </CardHeader>
            <CardContent class="space-y-3">
                <dl class="grid gap-x-4 gap-y-1.5 text-sm sm:grid-cols-2">
                    <div class="min-w-0">
                        <dt class="text-xs text-muted-foreground">{upstream_label}</dt>
                        <dd class="truncate font-mono text-xs">{upstream}</dd>
                    </div>
                    <div class="min-w-0">
                        <dt class="text-xs text-muted-foreground">{prefix_label}</dt>
                        <dd class="truncate font-mono text-xs">{prefix}</dd>
                    </div>
                </dl>
                <ul class="flex flex-wrap gap-1">
                    {family
                        .routes
                        .into_iter()
                        .map(|route| {
                            view! {
                                <li class="rounded border border-border bg-muted/40 px-1.5 py-0.5 \
                                           font-mono text-[0.6875rem] text-muted-foreground">
                                    {route}
                                </li>
                            }
                        })
                        .collect_view()}
                </ul>
            </CardContent>
        </Card>
    }
}

#[component]
fn Providers(rows: Vec<ProviderCard>) -> impl IntoView {
    if rows.is_empty() {
        return view! {
            <Card>
                <EmptyState
                    title=t!("catalog.no_providers")
                    icon=IconName::Plug
                />
            </Card>
        }
        .into_any();
    }
    view! {
        <ul class="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
            {rows.into_iter().map(|card| view! { <Provider card=card /> }).collect_view()}
        </ul>
    }
    .into_any()
}

#[component]
fn Provider(card: ProviderCard) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let status = card.status;
    let counts = format!("{}/{}", status.connected, status.total);
    let name = card.name.clone();
    let id = card.id.clone();
    let description = card.description.clone();
    let auth = card.auth_label.clone();
    // The router's own wording, not a verdict re-derived here from the counts: if the two ever disagree,
    // showing the router's is what makes that visible.
    let health = status.health.clone();
    let connected = format!("{} {counts}", locale.get("catalog.connected"));
    let errored =
        (status.error > 0).then(|| format!("{} {}", locale.get("catalog.errored"), status.error));
    let tone = if status.error > 0 {
        StatusTone::Destructive
    } else if status.connected > 0 {
        StatusTone::Success
    } else {
        StatusTone::Muted
    };

    view! {
        <li class="flex flex-col gap-2 rounded-xl border border-border bg-card p-4 shadow-xs">
            <div class="flex min-w-0 items-center gap-2">
                <Swatch accent=card.accent />
                <span class="truncate text-sm font-medium">{name}</span>
                <code class="ml-auto shrink-0 font-mono text-[0.6875rem] text-muted-foreground">
                    {id}
                </code>
            </div>
            <p class="line-clamp-2 min-h-8 text-xs leading-relaxed text-muted-foreground">
                {description}
            </p>
            <div class="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs">
                <StatusDot label=health tone=tone />
                <span class="text-muted-foreground">{connected}</span>
                {errored.map(|text| view! { <span class="text-destructive">{text}</span> })}
                <Badge variant=BadgeVariant::Outline class="ml-auto">{auth}</Badge>
            </div>
        </li>
    }
}

/// The provider's own colour, when it published one this is willing to put in an attribute.
///
/// The value arrives from the server, so it is checked rather than trusted: a hex triple or nothing.
/// Leptos escapes attribute values, so the risk is not injection -- it is that an unchecked string
/// reaches `style` and silently styles nothing, leaving a swatch that reads as a colour the provider
/// does not have. Falling back to a theme token keeps the row honest.
#[component]
fn Swatch(accent: String) -> impl IntoView {
    hex_colour(&accent).map_or_else(
        || {
            view! { <span class="size-2.5 rounded-full shrink-0 bg-muted-foreground/40" /> }
                .into_any()
        },
        |colour| {
            view! {
                <span
                    class="size-2.5 rounded-full shrink-0"
                    style=format!("background-color:{colour}")
                />
            }
            .into_any()
        },
    )
}

/// `#rgb` or `#rrggbb`, and nothing else.
fn hex_colour(value: &str) -> Option<&str> {
    let digits = value.strip_prefix('#')?;
    let shaped =
        matches!(digits.len(), 3 | 6) && digits.chars().all(|digit| digit.is_ascii_hexdigit());
    shaped.then_some(value)
}

#[cfg(test)]
mod tests {
    use super::{ProviderCatalog, RouteCatalog, hex_colour};

    /// Captured from `GET /api/catalog/routes` on a running router.
    const LIVE_ROUTES: &str = r#"{"families":[{"id":"api","title":"Control API",
        "upstream":"nullrouter-api","gatewayPrefix":"/api","sourcePrefix":"/api",
        "routes":["/api/health","/api/settings"]}]}"#;

    /// Captured from `GET /api/catalog/providers`. Kept whole, including the two keys this panel
    /// does not read, so a decoder that started refusing unknown fields fails here.
    // `r##` rather than `r#`: the accent's `"#` would otherwise close the literal mid-string.
    const LIVE_PROVIDERS: &str = r##"{"providers":[{"id":"claude","name":"Claude",
        "description":"OAuth-ready reference","authLabel":"Contract","accent":"#d97757",
        "status":{"connected":0,"error":0,"total":0,"health":"Idle"}}],
        "models":[],"openaiModels":[]}"##;

    #[test]
    fn the_live_route_catalogue_decodes() {
        let parsed: RouteCatalog = serde_json::from_str(LIVE_ROUTES).expect("live body decodes");
        let family = parsed.families.first().expect("one family");
        assert_eq!(family.upstream, "nullrouter-api");
        assert_eq!(family.gateway_prefix, "/api");
        assert_eq!(family.routes.len(), 2);
    }

    #[test]
    fn the_live_provider_catalogue_decodes_past_the_keys_this_panel_ignores() {
        let parsed: ProviderCatalog =
            serde_json::from_str(LIVE_PROVIDERS).expect("live body decodes");
        let card = parsed.providers.first().expect("one provider");
        assert_eq!(card.name, "Claude");
        assert_eq!(card.auth_label, "Contract");
        assert_eq!(card.status.health, "Idle");
    }

    #[test]
    fn a_missing_families_key_is_an_empty_map_not_a_decode_failure() {
        // The panel renders "no routes" for this, which is the truthful reading of `{}`.
        let parsed: RouteCatalog = serde_json::from_str("{}").expect("decodes");
        assert!(parsed.families.is_empty());
    }

    #[test]
    fn only_a_hex_triple_reaches_a_style_attribute() {
        assert_eq!(hex_colour("#d97757"), Some("#d97757"));
        assert_eq!(hex_colour("#abc"), Some("#abc"));
        for rejected in [
            "red",
            "#12",
            "#1234",
            "",
            "#gggggg",
            "url(x)",
            "#d97757;x:y",
        ] {
            assert_eq!(hex_colour(rejected), None, "{rejected} should be refused");
        }
    }
}
