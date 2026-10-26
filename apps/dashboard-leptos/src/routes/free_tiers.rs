//! Free-tier budget: what this router can spend before a provider starts charging.
//!
//! # Why the numbers are explained rather than just shown
//!
//! The headline is a sum over deduplicated pools, and a number like that is only useful if the reader can
//! see what went into it. A dashboard that prints "~1.47B free tokens" with no working is a claim; one that
//! prints the pool count, the row count, and which pools were excluded is a calculation the reader can
//! check. The previous version had the same figures in three cards and the methodology in a paragraph
//! below them, which is the wrong order.

use leptos::prelude::*;

use crate::t;
use crate::ui::{
    Alert, AlertDescription, AlertTitle, AlertVariant, Card, CardContent, CardDescription,
    CardHeader, CardTitle, Icon, IconName, PageBody, PageHeader, Separator,
};

/// Pool-deduped headline, matching the honest math in `docs/reference/FREE_TIERS.md`: 34 recurring pool
/// keys, 443 free-tier rows, 16 pools with a published monthly budget, plus five Groq per-model caps.
const HEADLINE_TOKENS_PER_MO: &str = "~1.47B";
const FIRST_MONTH_TOKENS: &str = "~2.07B";
const POOL_KEYS: usize = 34;
const FREE_TIER_ROWS: usize = 443;
const PROVIDERS_FREE: usize = 150;

#[component]
pub fn FreeTiers() -> impl IntoView {
    let composition = format!(
        "{POOL_KEYS} {} · {FREE_TIER_ROWS} {} · {PROVIDERS_FREE}+ {}",
        t!("free_tiers.pools"),
        t!("free_tiers.rows"),
        t!("free_tiers.providers"),
    );

    view! {
        <PageHeader title=t!("free_tiers.title") description=t!("free_tiers.description") />

        <PageBody>
            <div class="grid gap-3 lg:grid-cols-3">
                <Card class="lg:col-span-2">
                    <CardHeader>
                        <div class="space-y-1">
                            <CardTitle>{t!("free_tiers.steady")}</CardTitle>
                            <CardDescription>{composition.clone()}</CardDescription>
                        </div>
                        <Icon name=IconName::Sparkles class="size-4 text-muted-foreground" />
                    </CardHeader>
                    <CardContent>
                        <p class="font-mono text-4xl font-semibold tracking-tight">
                            {HEADLINE_TOKENS_PER_MO}
                        </p>
                        <p class="mt-1 text-sm text-muted-foreground">
                            {t!("free_tiers.tokens_per_month")}
                        </p>
                    </CardContent>
                </Card>

                <Card>
                    <CardHeader>
                        <CardTitle>{t!("free_tiers.first_month")}</CardTitle>
                    </CardHeader>
                    <CardContent>
                        <p class="font-mono text-4xl font-semibold tracking-tight">
                            {FIRST_MONTH_TOKENS}
                        </p>
                        <p class="mt-1 text-sm text-muted-foreground">{t!("free_tiers.with_credits")}</p>
                    </CardContent>
                </Card>
            </div>

            <div class="grid gap-3 lg:grid-cols-2">
                <Card>
                    <CardHeader>
                        <CardTitle>{t!("free_tiers.methodology_title")}</CardTitle>
                    </CardHeader>
                    <CardContent class="space-y-3">
                        <p class="text-[0.8125rem] leading-relaxed text-muted-foreground">
                            {t!("free_tiers.methodology_body")}
                        </p>
                        <Separator />
                        <p class="text-[0.8125rem] leading-relaxed text-muted-foreground">
                            {t!("free_tiers.methodology_exclusions")}
                        </p>
                        <p class="font-mono text-xs text-muted-foreground">
                            "docs/reference/FREE_TIERS.md"
                        </p>
                    </CardContent>
                </Card>

                <Card>
                    <CardHeader>
                        <CardTitle>{t!("free_tiers.live_title")}</CardTitle>
                    </CardHeader>
                    <CardContent class="space-y-3">
                        <p class="text-[0.8125rem] leading-relaxed text-muted-foreground">
                            {t!("free_tiers.live_body")}
                        </p>
                        <code class="block rounded-md border border-border bg-muted/40 px-2.5 py-1.5 \
                                     font-mono text-xs">
                            "GET /api/free-tiers"
                        </code>
                        <Alert variant=AlertVariant::Muted>
                            <AlertTitle>{t!("free_tiers.caveat_title")}</AlertTitle>
                            <AlertDescription>
                                {t!("free_tiers.caveat_body")}
                            </AlertDescription>
                        </Alert>
                    </CardContent>
                </Card>
            </div>
        </PageBody>
    }
}
