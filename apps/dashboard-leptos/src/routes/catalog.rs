//! The gateway's routing map, as the running router reports it.
//!
//! A diagnostics page, not a settings page: nothing here is writable. It answers the two questions
//! that are otherwise guesswork when a request lands somewhere unexpected -- which service actually
//! serves a path, and whether the gateway rewrites the prefix on the way there -- and it answers
//! them from the router's own catalogue rather than from a list maintained by hand beside it.

use leptos::prelude::*;

use crate::api::{Hydrate, load};
use crate::routes::{PageHeader, Panel};

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
    let locale = crate::i18n::use_locale();
    let (routes, set_routes) = signal(Hydrate::<RouteCatalog>::Loading);
    let (providers, set_providers) = signal(Hydrate::<ProviderCatalog>::Loading);

    let reload = move || {
        set_routes.set(Hydrate::Loading);
        set_providers.set(Hydrate::Loading);
        load("/api/catalog/routes", set_routes);
        load("/api/catalog/providers", set_providers);
    };
    reload();

    view! {
        <PageHeader
            title=locale.get("nav.catalog").to_owned()
            description=locale.get("catalog.description").to_owned()
        />

        <section class="space-y-3">
            <h2 class="text-sm font-medium text-muted-foreground">
                {locale.get("catalog.routes_title").to_owned()}
            </h2>
            <Panel
                state=routes
                on_retry=Callback::new(move |()| reload())
                children=move |data: RouteCatalog| view! { <Families rows=data.families /> }
            />
        </section>

        <section class="mt-6 space-y-3">
            <h2 class="text-sm font-medium text-muted-foreground">
                {locale.get("catalog.providers_title").to_owned()}
            </h2>
            <Panel
                state=providers
                on_retry=Callback::new(move |()| reload())
                children=move |data: ProviderCatalog| view! { <Providers rows=data.providers /> }
            />
        </section>
    }
}

#[component]
fn Families(rows: Vec<RouteFamily>) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    if rows.is_empty() {
        return view! {
            <p class="text-sm text-muted-foreground">
                {locale.get("catalog.no_routes").to_owned()}
            </p>
        }
        .into_any();
    }
    view! {
        <div class="space-y-4">
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

    view! {
        <section class="rounded-lg border border-border bg-card p-5 space-y-3">
            <div class="flex flex-wrap items-baseline justify-between gap-2">
                <div class="space-y-1 min-w-0">
                    <h3 class="text-sm font-semibold tracking-tight truncate">{family.title}</h3>
                    <code class="text-xs text-muted-foreground">{family.id}</code>
                </div>
                <p class="text-xs text-muted-foreground shrink-0">
                    {locale.fmt("catalog.route_count", &[("count", &count)])}
                </p>
            </div>

            <dl class="grid gap-2 sm:grid-cols-2 text-sm">
                <Field label=locale.get("catalog.upstream").to_owned() value=family.upstream />
                <Field
                    label=if rewritten {
                        locale.get("catalog.prefix_rewritten").to_owned()
                    } else {
                        locale.get("catalog.prefix").to_owned()
                    }
                    value=if rewritten {
                        format!("{} \u{2192} {}", family.gateway_prefix, family.source_prefix)
                    } else {
                        family.gateway_prefix
                    }
                />
            </dl>

            <ul class="flex flex-wrap gap-1.5">
                {family
                    .routes
                    .into_iter()
                    .map(|route| {
                        view! {
                            <li class="rounded border border-border px-2 py-1 font-mono text-xs text-muted-foreground">
                                {route}
                            </li>
                        }
                    })
                    .collect_view()}
            </ul>
        </section>
    }
}

#[component]
fn Field(label: String, value: String) -> impl IntoView {
    view! {
        <div class="min-w-0">
            <dt class="text-xs text-muted-foreground">{label}</dt>
            <dd class="font-mono text-xs truncate">{value}</dd>
        </div>
    }
}

#[component]
fn Providers(rows: Vec<ProviderCard>) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    if rows.is_empty() {
        return view! {
            <p class="text-sm text-muted-foreground">
                {locale.get("catalog.no_providers").to_owned()}
            </p>
        }
        .into_any();
    }
    view! {
        <ul class="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
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

    view! {
        <li class="rounded-lg border border-border bg-card p-4 space-y-2">
            <div class="flex items-center gap-2 min-w-0">
                <Swatch accent=card.accent />
                <span class="text-sm font-medium truncate">{card.name}</span>
                <code class="ml-auto text-xs text-muted-foreground shrink-0">{card.id}</code>
            </div>
            <p class="text-xs text-muted-foreground line-clamp-2">{card.description}</p>
            <div class="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs">
                <span class="flex items-center gap-1.5">
                    <span class=health_dot(&status) />
                    // The router's own wording, not a verdict re-derived here from the counts: if the
                    // two ever disagree, showing the router's is what makes that visible.
                    <span class="text-foreground">{status.health}</span>
                </span>
                <span class="text-muted-foreground">
                    {format!("{} {counts}", locale.get("catalog.connected"))}
                </span>
                {(status.error > 0)
                    .then(|| {
                        view! {
                            <span class="text-destructive">
                                {format!("{} {}", locale.get("catalog.errored"), status.error)}
                            </span>
                        }
                    })}
                <span class="ml-auto text-muted-foreground truncate">{card.auth_label}</span>
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

/// Health as a colour: errors first, because a provider with both connections and errors is a
/// provider that is failing, and colouring it by the connections would hide that.
const fn health_dot(status: &ProviderStatus) -> &'static str {
    if status.error > 0 {
        "size-1.5 rounded-full bg-destructive"
    } else if status.connected > 0 {
        "size-1.5 rounded-full bg-success"
    } else {
        "size-1.5 rounded-full bg-muted-foreground/40"
    }
}

#[cfg(test)]
mod tests {
    use super::{ProviderCatalog, ProviderStatus, RouteCatalog, health_dot, hex_colour};

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

    #[test]
    fn an_erroring_provider_is_coloured_as_failing_even_when_some_connections_work() {
        let failing = ProviderStatus {
            connected: 2,
            error: 1,
            total: 3,
            health: "Degraded".to_owned(),
        };
        assert!(health_dot(&failing).contains("destructive"));
        let healthy = ProviderStatus {
            connected: 2,
            error: 0,
            total: 2,
            health: "Ready".to_owned(),
        };
        assert!(health_dot(&healthy).contains("success"));
        assert!(health_dot(&ProviderStatus::default()).contains("muted"));
    }
}
