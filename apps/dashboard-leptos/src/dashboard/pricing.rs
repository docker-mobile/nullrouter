use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct PricingSettingsState {
    pub title: &'static str,
    pub description: &'static str,
    pub total_models: u16,
    pub providers: u16,
    pub status_label: &'static str,
    pub current_pricing_title: &'static str,
    pub empty_title: &'static str,
    pub stat_labels: [&'static str; 3],
    pub token_type_labels: [&'static str; 5],
    pub modal_field_labels: [&'static str; 5],
    pub how_pricing_works: &'static [&'static str],
    pub modal_persistence_label: &'static str,
    pub modal_persistence_note: &'static str,
    pub persistence_wired: bool,
}

pub const fn pricing_settings_state() -> PricingSettingsState {
    PricingSettingsState {
        title: "Pricing Settings",
        description: "Configure pricing rates for cost tracking and calculations",
        total_models: 0,
        providers: 0,
        status_label: "Preview",
        current_pricing_title: "Current Pricing Overview",
        empty_title: "No pricing data available",
        stat_labels: ["Total Models", "Providers", "Status"],
        token_type_labels: ["Input", "Output", "Cached", "Reasoning", "Cache Creation"],
        modal_field_labels: ["Input", "Output", "Cached", "Reasoning", "Cache Creation"],
        how_pricing_works: &HOW_PRICING_WORKS,
        modal_persistence_label: "Persistence unsupported",
        modal_persistence_note: "Pricing edits are local preview only until /api/pricing PATCH and DELETE persistence are wired into the WASM host.",
        persistence_wired: false,
    }
}

const HOW_PRICING_WORKS: [&str; 5] = [
    "Cost Calculation: Costs are calculated from token usage and pricing rates: (input_tokens x input_rate) + (output_tokens x output_rate) + (cached_tokens x cached_rate).",
    "Pricing Format: All rates are in dollars per million tokens ($/1M tokens). Example: an input rate of 2.50 means $2.50 per 1,000,000 input tokens.",
    "Token Types: Input covers prompt tokens, Output covers completion tokens, Cached covers cached input tokens, Reasoning falls back to output rate, and Cache Creation falls back to input rate.",
    "Custom Pricing: Upstream can override default pricing for specific models and reset back to standard rates.",
    "WASM Status: This dashboard view renders the pricing controls as a preview shell and does not persist changes.",
];
