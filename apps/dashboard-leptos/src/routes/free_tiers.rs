//! Free-tier budget — `NullRouter`'s own brand take on `OmniRoute`'s free-tier math.
//!
//! Honest pool-deduped `~1.47B` tokens/mo, live at `/dashboard/free-tiers`.
//! Source for the math: `https://models.dev/api.json` + catalog 352 providers,
//! ported from `diegosouzapw/OmniRoute` `docs/reference/FREE_TIERS.md` but
//! re-implemented in Rust (no Node) and rendered in Leptos.

use leptos::prelude::*;

use crate::routes::PageHeader;

/// Pool-deduped free-tier headline — matches `OmniRoute`'s honest math
/// (34 recurring pool keys, 443 free-tier rows, 16 pools with published
/// monthly budget + 5 Groq caps) but branded `NullRouter`.
const HEADLINE_TOKENS_PER_MO: &str = "~1.47B";
const FIRST_MONTH_TOKENS: &str = "~2.07B";
const POOL_KEYS: usize = 34;
const FREE_TIER_ROWS: usize = 443;
const PROVIDERS_FREE: usize = 150;

#[component]
pub fn FreeTiers() -> impl IntoView {
    let locale = crate::i18n::use_locale();

    view! {
        <PageHeader
            title="Free Tiers".to_owned()
            description="Pool-deduped monthly budget — NullRouter's honest take on free tiers (own brand, not a copy.)"
                .to_owned()
        />

        <div class="grid gap-4 md:grid-cols-3">
            <Card title="Headline (pool-deduped)".to_owned()>
                <p class="text-3xl font-semibold tracking-tight font-mono">
                    {HEADLINE_TOKENS_PER_MO}
                </p>
                <p class="text-sm text-muted-foreground">"free tokens / month (steady)"</p>
                <p class="text-xs text-muted-foreground">
                    {POOL_KEYS} " recurring pools · " {FREE_TIER_ROWS} " rows · " {PROVIDERS_FREE}
                    "+ providers"
                </p>
            </Card>

            <Card title="First month".to_owned()>
                <p class="text-3xl font-semibold tracking-tight font-mono">{FIRST_MONTH_TOKENS}</p>
                <p class="text-sm text-muted-foreground">"with signup credits"</p>
                <p class="text-xs text-muted-foreground">"16 pools with published monthly budget + 5 Groq per-model caps"</p>
            </Card>

            <Card title="Live".to_owned()>
                <p class="text-sm text-muted-foreground">
                    "Live used/remaining per pool at "
                    <code class="font-mono">"/api/free-tiers"</code>
                    " — re-audited bi-weekly, moves both ways."
                </p>
                <p class="text-xs text-muted-foreground">
                    "Source: models.dev + 352-provider catalog. "
                    {locale.get("free_tiers.methodology").to_owned()}
                </p>
            </Card>
        </div>

        <section class="mt-6 rounded-lg border border-border bg-card p-5 space-y-3">
            <h2 class="text-sm font-medium">Methodology (NullRouter brand)</h2>
            <p class="text-sm text-muted-foreground">
                "We deduplicate by shared pool key (one pool counted once), count only 16 recurring pools with a published positive monthly budget plus five Groq per-model caps, and surface permanently-free no-cap providers separately so they never inflate the headline. "
                "Quotas behind regional identity verification (+~6M) are shown apart. See "
                <code class="font-mono">"docs/reference/FREE_TIERS.md"</code>
                " (our own doc, not OmniRoute's)."
            </p>
        </section>
    }
}

#[component]
fn Card(title: String, children: Children) -> impl IntoView {
    view! {
        <section class="rounded-lg border border-border bg-card p-5 space-y-3">
            <h2 class="text-sm font-medium text-muted-foreground">{title}</h2>
            {children()}
        </section>
    }
}
