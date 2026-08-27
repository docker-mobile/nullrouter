//! Pure state for the Pricing panel: parse the rate catalog and settle one reset.
//!
//! The panel this backs rendered `pricing_settings_state()`: "Total Models 0",
//! "Providers 0", a "No pricing data available" card, and a modal of five fields
//! all reading `0.00` above the line "Current pricing remains empty until the host
//! provides /api/pricing data". `GET /api/pricing` was already implemented and
//! already returned rates. The panel showed zeros for data it never asked for, and
//! `0.00` is not a neutral placeholder for a price — it reads as free.
//!
//! What this module guarantees:
//!
//! * [`parse_pricing`] is the only way a rate reaches the screen, and it keeps
//!   every value as the [`serde_json::Number`] the server sent, so `0.175` renders
//!   as `0.175` rather than as whatever a float round-trip produced.
//! * A rate the server did not send is [`Rate::is_unset`] and renders as "not set".
//!   There is no `unwrap_or(0.0)` anywhere in this file: a missing rate is unknown,
//!   not zero.
//! * [`PricingTable::is_empty`] is the empty state, rendered as itself.
//! * [`settle_reset`] replaces the table only with what `DELETE` returned. The
//!   default rates live on the server (`default_pricing`), so this page cannot
//!   predict them and does not try — a reset shows the server's answer or the
//!   previous table plus an error.

use crate::api::ApiError;
use serde::Serialize;
use std::collections::BTreeMap;

/// The endpoint that owns the rate catalog.
pub const PRICING_PATH: &str = "/api/pricing";

/// The five rate fields the endpoint accepts.
///
/// Pinned to `VALID_PRICING_FIELDS` in
/// `services/api-actix/src/catalog/pricing/state.rs`; a `PATCH` carrying anything
/// else is rejected with `400`, so the form must not offer a sixth.
pub const RATE_FIELDS: [RateField; 5] = [
    RateField::Input,
    RateField::Output,
    RateField::Cached,
    RateField::Reasoning,
    RateField::CacheCreation,
];

/// One priced token class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RateField {
    Input,
    Output,
    Cached,
    Reasoning,
    CacheCreation,
}

impl RateField {
    /// The JSON key, exactly as the endpoint spells it.
    ///
    /// Snake case, unlike the rest of this API: the pricing catalog is a free-form
    /// map and its field names are not `camelCase`.
    pub const fn wire_key(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
            Self::Cached => "cached",
            Self::Reasoning => "reasoning",
            Self::CacheCreation => "cache_creation",
        }
    }

    /// Column heading.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Input => "Input",
            Self::Output => "Output",
            Self::Cached => "Cached",
            Self::Reasoning => "Reasoning",
            Self::CacheCreation => "Cache creation",
        }
    }

    /// What the class means, for the column's title.
    pub const fn detail(self) -> &'static str {
        match self {
            Self::Input => "Prompt tokens",
            Self::Output => "Completion tokens",
            Self::Cached => "Cached input tokens",
            Self::Reasoning => "Thinking tokens",
            Self::CacheCreation => "Cache writes",
        }
    }
}

/// One rate, or the absence of one.
///
/// Holds the server's own [`serde_json::Number`] rather than an `f64` so the text
/// on screen is the text the server sent. `1.75` stays `1.75`; nothing is
/// reformatted through a float and nothing gains a trailing `.0`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Rate {
    value: Option<serde_json::Number>,
}

impl Rate {
    /// `true` when the server did not price this class for this model.
    ///
    /// The panel renders this as "not set". It is the whole point of the type: the
    /// old modal printed `0.00` for every field, which claims a price of zero.
    pub const fn is_unset(&self) -> bool {
        self.value.is_none()
    }

    /// The rate as the server wrote it, or `None`.
    pub fn text(&self) -> Option<String> {
        self.value.as_ref().map(serde_json::Number::to_string)
    }

    /// Display text: the rate, or a statement that there is none.
    pub fn display(&self) -> String {
        self.text().unwrap_or_else(|| String::from("not set"))
    }

    /// The numeric value, for totals and comparisons only.
    pub fn as_f64(&self) -> Option<f64> {
        self.value.as_ref().and_then(serde_json::Number::as_f64)
    }
}

/// One priced model: its five rates, as far as the server named them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelRates {
    pub provider: String,
    pub model: String,
    pub input: Rate,
    pub output: Rate,
    pub cached: Rate,
    pub reasoning: Rate,
    pub cache_creation: Rate,
}

impl ModelRates {
    /// One field's rate.
    pub const fn rate(&self, field: RateField) -> &Rate {
        match field {
            RateField::Input => &self.input,
            RateField::Output => &self.output,
            RateField::Cached => &self.cached,
            RateField::Reasoning => &self.reasoning,
            RateField::CacheCreation => &self.cache_creation,
        }
    }

    /// `provider/model`, the id the rest of the router uses.
    pub fn full_model(&self) -> String {
        format!("{}/{}", self.provider, self.model)
    }

    /// How many of the five classes the server priced.
    pub fn priced_count(&self) -> usize {
        RATE_FIELDS
            .iter()
            .filter(|field| !self.rate(**field).is_unset())
            .count()
    }

    /// Which classes have no rate, named rather than shown as zero.
    pub fn unset_fields(&self) -> Vec<RateField> {
        RATE_FIELDS
            .iter()
            .copied()
            .filter(|field| self.rate(*field).is_unset())
            .collect()
    }

    /// A note about what is missing, when anything is.
    pub fn gap_note(&self) -> Option<String> {
        let unset = self.unset_fields();
        if unset.is_empty() {
            return None;
        }
        let names: Vec<&str> = unset.iter().map(|field| field.label()).collect();
        Some(format!(
            "No rate published for {}. Cost for those token classes is unknown, not zero.",
            names.join(", "),
        ))
    }

    pub fn row_id(&self) -> String {
        format!(
            "nr-pricing-row-{}-{}",
            dom_suffix(&self.provider),
            dom_suffix(&self.model),
        )
    }

    /// Accessible label for the reset control, naming the row it changes.
    pub fn reset_label(&self) -> String {
        format!(
            "Reset pricing for {} to this build's default",
            self.full_model()
        )
    }
}

/// Reduce a name to characters that are safe in a DOM id.
fn dom_suffix(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

/// The rate catalog, flattened into rows.
///
/// The endpoint returns a nested `{provider: {model: {field: number}}}` map. It is
/// flattened here so the panel renders one table rather than nesting two loops,
/// and ordered so the rows do not move between reloads. `BTreeMap` upstream means
/// the input is already sorted; the sort is kept anyway so the order is a property
/// of this type rather than an assumption about the server's map.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PricingTable {
    rows: Vec<ModelRates>,
}

impl PricingTable {
    pub fn new(mut rows: Vec<ModelRates>) -> Self {
        rows.sort_by(|left, right| {
            left.provider
                .cmp(&right.provider)
                .then_with(|| left.model.cmp(&right.model))
        });
        Self { rows }
    }

    pub fn rows(&self) -> &[ModelRates] {
        &self.rows
    }

    /// `true` when the catalog prices nothing.
    ///
    /// A real state: `default_pricing()` is small and a user can reset every
    /// override away. Rendered as itself, not as a table of zeros.
    pub const fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// How many models carry at least one rate.
    pub const fn model_count(&self) -> usize {
        self.rows.len()
    }

    /// How many providers appear in the catalog.
    pub fn provider_count(&self) -> usize {
        let mut providers: Vec<&str> = self.rows.iter().map(|row| row.provider.as_str()).collect();
        providers.sort_unstable();
        providers.dedup();
        providers.len()
    }

    /// How many of the `model_count() * 5` possible rates are actually published.
    ///
    /// Shown next to the table so a sparse catalog is visible as sparse rather than
    /// looking complete with zeros in the gaps.
    pub fn published_rate_count(&self) -> usize {
        self.rows.iter().map(ModelRates::priced_count).sum()
    }

    pub fn row(&self, provider: &str, model: &str) -> Option<&ModelRates> {
        self.rows
            .iter()
            .find(|row| row.provider == provider && row.model == model)
    }
}

/// Parse a `GET /api/pricing` body.
///
/// The response is a bare map, with no envelope. `None` when the body is not an
/// object of objects of numbers — a shape change must surface as a failure, not as
/// a table with holes in it. An empty object parses to an empty table, which is the
/// empty state.
///
/// A field whose value is not a number fails the whole parse rather than being
/// skipped: silently dropping `"input": "1.75"` would show a model as unpriced when
/// the server does price it.
pub fn parse_pricing(body: &str) -> Option<PricingTable> {
    let providers = serde_json::from_str::<serde_json::Value>(body)
        .ok()?
        .as_object()?
        .clone();
    let mut rows = Vec::new();
    for (provider, models) in providers {
        for (model, fields) in models.as_object()? {
            rows.push(model_rates(&provider, model, fields.as_object()?)?);
        }
    }
    Some(PricingTable::new(rows))
}

/// One row from the endpoint's `{field: number}` map.
///
/// Unknown keys are ignored rather than rejected: the endpoint refuses them on
/// write, but a newer service adding a sixth class must not blank this table.
fn model_rates(
    provider: &str,
    model: &str,
    fields: &serde_json::Map<String, serde_json::Value>,
) -> Option<ModelRates> {
    let rate = |field: RateField| -> Option<Rate> {
        match fields.get(field.wire_key()) {
            None | Some(serde_json::Value::Null) => Some(Rate { value: None }),
            Some(serde_json::Value::Number(number)) => Some(Rate {
                value: Some(number.clone()),
            }),
            // Present but not a number: a shape this panel cannot render honestly.
            Some(_other) => None,
        }
    };
    Some(ModelRates {
        provider: provider.to_owned(),
        model: model.to_owned(),
        input: rate(RateField::Input)?,
        output: rate(RateField::Output)?,
        cached: rate(RateField::Cached)?,
        reasoning: rate(RateField::Reasoning)?,
        cache_creation: rate(RateField::CacheCreation)?,
    })
}

/// `DELETE /api/pricing?provider=..&model=..`, which drops a user override.
///
/// Query values are percent-encoded through the same helper the path builders use.
/// A provider or model id is not guaranteed to be free of `&` or `=`, and a broken
/// query would reset the wrong row — or everything, since an absent `provider`
/// means "clear all".
pub fn reset_path(provider: &str, model: &str) -> String {
    let encode = crate::dashboard::pools_live::encode_path_segment;
    format!(
        "{PRICING_PATH}?provider={}&model={}",
        encode(provider),
        encode(model),
    )
}

/// What the panel should do after a reset.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResetSettlement {
    /// The router applied it and returned the whole merged catalog. Boxed so the
    /// large variant does not widen every settlement.
    Replaced(Box<PricingTable>),
    /// Nothing changed on screen, and the reason is shown.
    ///
    /// Deliberately not optimistic: the default rates live in `default_pricing()`
    /// on the server, so this page cannot know what a reset would reveal. Showing a
    /// guessed default and correcting it afterwards would put invented prices on
    /// screen, which is the failure mode this panel was rewritten to remove.
    Kept { error: ApiError, message: String },
}

/// Interpret a reset response.
///
/// The endpoint answers `200` with the full merged catalog, so a success replaces
/// the table outright — including any other row the reset happened to affect.
pub fn settle_reset(model: &str, response: Result<&str, ApiError>) -> ResetSettlement {
    match response {
        Ok(body) => parse_pricing(body).map_or_else(
            || ResetSettlement::Kept {
                error: ApiError::Body,
                message: format!(
                    "{model} may have been reset, but the new rates could not be read. {}",
                    ApiError::Body.message(),
                ),
            },
            |table| ResetSettlement::Replaced(Box::new(table)),
        ),
        Err(error) => ResetSettlement::Kept {
            error,
            message: format!("{model} was not reset. {}", error.message()),
        },
    }
}

/// One rate the user typed, ready to send.
type RateMap = BTreeMap<String, serde_json::Number>;

/// The `PATCH /api/pricing` payload: `{provider: {model: {field: number}}}`.
#[derive(Debug, Serialize)]
struct PatchBody(BTreeMap<String, BTreeMap<String, RateMap>>);

/// Rates the user is editing for one model.
///
/// Text fields rather than numbers because an empty box and a `0` are different
/// intentions, and only the user can tell them apart. A blank field is omitted from
/// the `PATCH`, which leaves the merged value alone (`merged_pricing` in
/// `services/api-actix/src/catalog/pricing/state.rs` merges field by field).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RateDraft {
    pub provider: String,
    pub model: String,
    pub input: String,
    pub output: String,
    pub cached: String,
    pub reasoning: String,
    pub cache_creation: String,
}

impl RateDraft {
    /// Prefill from a row, showing exactly what the server sent.
    ///
    /// An unset rate becomes an empty box, not `0`. Saving without touching it
    /// leaves it unset.
    pub fn for_row(row: &ModelRates) -> Self {
        Self {
            provider: row.provider.clone(),
            model: row.model.clone(),
            input: row.input.text().unwrap_or_default(),
            output: row.output.text().unwrap_or_default(),
            cached: row.cached.text().unwrap_or_default(),
            reasoning: row.reasoning.text().unwrap_or_default(),
            cache_creation: row.cache_creation.text().unwrap_or_default(),
        }
    }

    /// The text for one field.
    pub fn field(&self, field: RateField) -> &str {
        match field {
            RateField::Input => &self.input,
            RateField::Output => &self.output,
            RateField::Cached => &self.cached,
            RateField::Reasoning => &self.reasoning,
            RateField::CacheCreation => &self.cache_creation,
        }
    }

    /// Replace the text for one field.
    pub fn set_field(&mut self, field: RateField, value: String) {
        let slot = match field {
            RateField::Input => &mut self.input,
            RateField::Output => &mut self.output,
            RateField::Cached => &mut self.cached,
            RateField::Reasoning => &mut self.reasoning,
            RateField::CacheCreation => &mut self.cache_creation,
        };
        *slot = value;
    }

    /// Validate every field and render the `PATCH` body.
    ///
    /// Mirrors `parse_pricing_fields`: a rate must be a finite, non-negative
    /// number. Blank fields are skipped. An all-blank draft is
    /// [`RateDraftError::Empty`] rather than an empty `PATCH`, which would spend a
    /// request to change nothing.
    pub fn patch_body(&self) -> Result<String, RateDraftError> {
        let mut rates = RateMap::new();
        for field in RATE_FIELDS {
            let text = self.field(field).trim();
            if text.is_empty() {
                continue;
            }
            let value = text
                .parse::<f64>()
                .ok()
                .filter(|value| value.is_finite() && *value >= 0.0)
                .and_then(serde_json::Number::from_f64)
                .ok_or(RateDraftError::NotANumber(field))?;
            rates.insert(field.wire_key().to_owned(), value);
        }
        if rates.is_empty() {
            return Err(RateDraftError::Empty);
        }
        let mut models = BTreeMap::new();
        models.insert(self.model.clone(), rates);
        let mut providers = BTreeMap::new();
        providers.insert(self.provider.clone(), models);
        serde_json::to_string(&PatchBody(providers)).map_err(|_error| RateDraftError::Encode)
    }

    /// The blocking validation error, for disabling submit before a click.
    pub fn validation_error(&self) -> Option<RateDraftError> {
        self.patch_body().err()
    }
}

/// Why a rate draft cannot be sent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RateDraftError {
    /// One field is not a non-negative finite number.
    NotANumber(RateField),
    /// Every field is blank.
    Empty,
    Encode,
}

impl RateDraftError {
    pub fn message(self) -> String {
        match self {
            Self::NotANumber(field) => format!(
                "{} must be a number of dollars per million tokens, and cannot be negative.",
                field.label(),
            ),
            Self::Empty => String::from(
                "Enter at least one rate. Blank fields are left as the router has them.",
            ),
            Self::Encode => String::from("These rates could not be encoded as a request."),
        }
    }
}

// ── requests ────────────────────────────────────────────────────────────────

/// `GET /api/pricing`.
pub async fn load_pricing() -> Result<PricingTable, ApiError> {
    let body = crate::api::get(PRICING_PATH).await?;
    parse_pricing(&body).ok_or(ApiError::Body)
}

/// `DELETE /api/pricing?provider=..&model=..`.
pub async fn reset_pricing(provider: &str, model: &str) -> ResetSettlement {
    let response = crate::api::delete(&reset_path(provider, model)).await;
    let label = format!("{provider}/{model}");
    settle_reset(&label, response.as_deref().map_err(|error| *error))
}
