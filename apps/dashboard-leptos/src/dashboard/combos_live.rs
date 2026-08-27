//! Pure state for the Combos panel: parse combos, order them, and settle one
//! write.
//!
//! The panel this backs rendered `combo_summaries()` — two hardcoded tiles,
//! "coding-fallback" and "web-research", captioned "Preview" and "Not persisted by
//! the WASM dashboard". Both were invented. Someone with a real combo saw neither
//! of theirs, and someone with none saw two that did not exist, listing member
//! models (`codex/gpt-5`, `9router-web-search`) chosen by whoever wrote the
//! fixture.
//!
//! `GET /api/combos` has existed the whole time. This module is the parse/merge
//! half of using it:
//!
//! * [`parse_combos`] is the only way a tile reaches the panel.
//! * [`ComboList::is_empty`] is rendered as itself — no combos means no tiles plus
//!   an invitation to make one.
//! * [`ComboDraft`] validates against the endpoint's own rules
//!   (`is_valid_combo_name` in `services/state-actix/src/routes.rs`) so a name the
//!   server would reject is explained before a request is spent on it.
//! * [`ComboList::take`] / [`ComboList::restore`] make an optimistic delete
//!   reversible.
//!
//! Model choices come from `GET /api/models` via [`parse_models`], so the picker
//! offers what this build actually advertises rather than a list typed out here.

use crate::api::ApiError;
use serde::{Deserialize, Serialize};

/// The endpoint that owns the combo list.
pub const COMBOS_PATH: &str = "/api/combos";

/// The endpoint that lists the models a combo can be built from.
pub const MODELS_PATH: &str = "/api/models";

/// `GET`/`PUT`/`DELETE` path for one combo.
pub fn combo_path(id: &str) -> String {
    format!(
        "{COMBOS_PATH}/{}",
        crate::dashboard::pools_live::encode_path_segment(id)
    )
}

/// The `{"combos":[...]}` envelope from `GET /api/combos`.
#[derive(Debug, Deserialize)]
struct CombosEnvelope {
    combos: Vec<Combo>,
}

/// One combo, as the API reports it.
///
/// Mirrors `Combo` in `services/state-actix/src/store.rs`. Note that `POST`, `GET
/// {id}`, and `PUT {id}` return the combo *bare* — the `combos` envelope is only on
/// the list — which is why [`parse_combo`] deserialises the object directly.
///
/// `id` and `name` are required: they are the row's identity, and a tile titled
/// with a defaulted empty name or a delete pointed at an empty id would be a
/// fabrication. `kind` is genuinely nullable upstream (`Option<String>`), and
/// `models` defaults to empty because a combo with no members is a real state the
/// store allows.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Combo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

impl Combo {
    /// The combo's kind, or a statement that it has none.
    ///
    /// `kind` is nullable and the store never fills it in, so this names the
    /// absence instead of showing "chat" — which is what the fixture claimed for a
    /// combo nobody had created.
    pub fn kind_label(&self) -> &str {
        self.kind
            .as_deref()
            .map(str::trim)
            .filter(|kind| !kind.is_empty())
            .unwrap_or("no kind set")
    }

    /// `true` when the router recorded a kind for this combo.
    pub fn has_kind(&self) -> bool {
        self.kind
            .as_deref()
            .is_some_and(|kind| !kind.trim().is_empty())
    }

    /// Member models, in the order the router stored them.
    ///
    /// Order is meaning here: a combo is tried in sequence, so re-sorting would
    /// misdescribe which model gets the request first.
    pub fn members(&self) -> &[String] {
        &self.models
    }

    /// "3 models", or the absence when the combo is empty.
    pub fn member_summary(&self) -> String {
        if self.models.is_empty() {
            String::from("no models")
        } else {
            plural(self.models.len(), "model")
        }
    }

    /// When this combo last changed, rendered as UTC.
    pub fn updated_label(&self) -> Option<String> {
        self.updated_at
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(crate::dashboard::pools_live::format_timestamp)
    }

    pub fn heading_id(&self) -> String {
        format!("nr-combo-heading-{}", dom_suffix(&self.id))
    }

    pub fn status_id(&self) -> String {
        format!("nr-combo-status-{}", dom_suffix(&self.id))
    }

    /// Accessible label for the delete control, naming the row it destroys.
    pub fn delete_label(&self) -> String {
        format!("Delete combo {}", self.name)
    }
}

/// Reduce an id to characters that are safe in a DOM id.
fn dom_suffix(id: &str) -> String {
    id.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

/// "1 model" / "3 models".
pub fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("{count} {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// The configured combos, ordered by name.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ComboList {
    combos: Vec<Combo>,
}

impl ComboList {
    pub fn new(mut combos: Vec<Combo>) -> Self {
        combos.sort_by(compare_combos);
        Self { combos }
    }

    pub fn combos(&self) -> &[Combo] {
        &self.combos
    }

    /// `true` when the router holds no combos.
    ///
    /// Rendered as the empty state. The old panel could not express this, so it
    /// always drew its two fixtures.
    pub const fn is_empty(&self) -> bool {
        self.combos.is_empty()
    }

    pub const fn len(&self) -> usize {
        self.combos.len()
    }

    /// How many distinct models are referenced across every combo.
    pub fn model_count(&self) -> usize {
        let mut models: Vec<&str> = self
            .combos
            .iter()
            .flat_map(|combo| combo.models.iter().map(String::as_str))
            .collect();
        models.sort_unstable();
        models.dedup();
        models.len()
    }

    /// Whether a name is already taken, ignoring case.
    ///
    /// `create_combo` rejects a duplicate with `400`, so the form can say so
    /// before spending a request. Case-insensitive is the stricter reading, and a
    /// near-duplicate is worth warning about either way.
    pub fn has_name(&self, name: &str) -> bool {
        let needle = name.trim().to_ascii_lowercase();
        self.combos
            .iter()
            .any(|combo| combo.name.to_ascii_lowercase() == needle)
    }

    /// Remove one combo, remembering where it was.
    pub fn take(&mut self, id: &str) -> Option<PendingDelete> {
        let index = self.combos.iter().position(|combo| combo.id == id)?;
        let combo = Box::new(self.combos.remove(index));
        Some(PendingDelete { index, combo })
    }

    /// Put a removed combo back at its original index.
    pub fn restore(&mut self, pending: PendingDelete) {
        let index = pending.index.min(self.combos.len());
        self.combos.insert(index, *pending.combo);
        self.combos.sort_by(compare_combos);
    }

    /// Add or replace a combo the server confirmed, keeping order.
    pub fn upsert(&mut self, combo: Combo) {
        self.combos.retain(|existing| existing.id != combo.id);
        self.combos.push(combo);
        self.combos.sort_by(compare_combos);
    }
}

/// Display order: name, then id, so the order is total and stable.
fn compare_combos(left: &Combo, right: &Combo) -> std::cmp::Ordering {
    left.name
        .to_ascii_lowercase()
        .cmp(&right.name.to_ascii_lowercase())
        .then_with(|| left.id.cmp(&right.id))
}

/// A combo removed optimistically, held until the `DELETE` settles.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingDelete {
    index: usize,
    combo: Box<Combo>,
}

impl PendingDelete {
    pub fn id(&self) -> &str {
        &self.combo.id
    }

    pub fn name(&self) -> &str {
        &self.combo.name
    }
}

/// Parse a `GET /api/combos` body.
///
/// `None` on anything that is not a `combos` array of well-formed rows, so a shape
/// change surfaces as a visible failure rather than an empty panel reading as "you
/// have no combos". An empty array is a success and reaches the empty state.
pub fn parse_combos(body: &str) -> Option<ComboList> {
    serde_json::from_str::<CombosEnvelope>(body)
        .ok()
        .map(|envelope| ComboList::new(envelope.combos))
}

/// Parse the bare combo object returned by `POST`, `GET {id}`, and `PUT {id}`.
///
/// Unlike providers and pools, these three responses carry no envelope
/// (`responses::json(StatusCode::CREATED, &combo)`), so this deserialises the
/// object itself.
pub fn parse_combo(body: &str) -> Option<Combo> {
    serde_json::from_str::<Combo>(body).ok()
}

/// How a `DELETE /api/combos/{id}` ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeleteOutcome {
    Confirmed,
    Rejected(ApiError),
}

/// What the panel should do with an optimistically removed tile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeleteSettlement {
    Removed,
    RolledBack {
        pending: PendingDelete,
        error: ApiError,
        message: String,
    },
}

/// Settle one optimistic delete.
///
/// A `404` settles as removed: the combo is gone, which is what the panel already
/// shows. Restoring the tile would put back a row that no longer exists.
pub fn settle_delete(pending: PendingDelete, outcome: DeleteOutcome) -> DeleteSettlement {
    match outcome {
        DeleteOutcome::Confirmed | DeleteOutcome::Rejected(ApiError::Status(404)) => {
            DeleteSettlement::Removed
        }
        DeleteOutcome::Rejected(error) => {
            let message = format!("{} was not deleted. {}", pending.name(), error.message());
            DeleteSettlement::RolledBack {
                pending,
                error,
                message,
            }
        }
    }
}

/// A combo the user is composing.
///
/// Only what `POST /api/combos` accepts: `name`, `kind`, `models`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ComboDraft {
    pub name: String,
    /// Optional upstream. Left blank rather than defaulted to "chat", because the
    /// store would then hold a kind the user never chose.
    pub kind: String,
    /// Full model ids, in the order they should be tried.
    pub models: Vec<String>,
}

/// The `POST /api/combos` request body.
#[derive(Debug, Serialize)]
struct ComboRequest<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<&'a str>,
    models: &'a [String],
}

impl ComboDraft {
    /// Add a model, keeping the list free of duplicates.
    ///
    /// A repeated member would make the tile claim a fallback step that resolves to
    /// the same upstream twice.
    pub fn add_model(&mut self, model: &str) {
        let model = model.trim();
        if model.is_empty() || self.contains(model) {
            return;
        }
        self.models.push(model.to_owned());
    }

    pub fn remove_model(&mut self, model: &str) {
        self.models.retain(|existing| existing != model);
    }

    pub fn contains(&self, model: &str) -> bool {
        self.models.iter().any(|existing| existing == model)
    }

    /// Validate the draft and render the body to `POST`.
    ///
    /// `existing` is the loaded list, used only for the duplicate-name check.
    /// `kind` is omitted when blank so the stored combo has `kind: null` rather
    /// than an empty string, matching what the store writes for a combo created
    /// without one.
    pub fn body(&self, existing: &ComboList) -> Result<String, DraftError> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err(DraftError::NameMissing);
        }
        if !is_valid_combo_name(name) {
            return Err(DraftError::NameCharset);
        }
        if existing.has_name(name) {
            return Err(DraftError::NameTaken);
        }
        if self.models.is_empty() {
            return Err(DraftError::ModelsMissing);
        }
        let kind = self.kind.trim();
        let request = ComboRequest {
            name,
            kind: Some(kind).filter(|kind| !kind.is_empty()),
            models: &self.models,
        };
        serde_json::to_string(&request).map_err(|_error| DraftError::Encode)
    }

    /// The blocking validation error, for disabling submit before a click.
    pub fn validation_error(&self, existing: &ComboList) -> Option<DraftError> {
        self.body(existing).err()
    }
}

/// The name rule the endpoint enforces.
///
/// Copied deliberately from `is_valid_combo_name` in
/// `services/state-actix/src/routes.rs`: letters, digits, `-`, `_`, `.`. Kept in
/// step with the server so the form's message and the server's `400` agree.
pub fn is_valid_combo_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

/// Why a draft cannot be submitted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DraftError {
    NameMissing,
    NameCharset,
    NameTaken,
    ModelsMissing,
    Encode,
}

impl DraftError {
    pub const fn message(self) -> &'static str {
        match self {
            Self::NameMissing => "Give this combo a name.",
            Self::NameCharset => "Names can only use letters, numbers, -, _ and .",
            Self::NameTaken => "A combo with that name already exists.",
            Self::ModelsMissing => "Pick at least one model for this combo.",
            Self::Encode => "This combo could not be encoded as a request.",
        }
    }
}

/// The `{"models":[...]}` envelope from `GET /api/models`.
#[derive(Debug, Deserialize)]
struct ModelsEnvelope {
    models: Vec<ModelOption>,
}

/// One model this build advertises, as the combo picker offers it.
///
/// Mirrors the entries `GET /api/models` returns (`DashboardModelEntry` in
/// `services/api-actix/src/models.rs`). `fullModel` is what a combo stores, so it
/// is required; `provider` and `model` are the label. `alias` and `caps` are
/// optional because they are presentation, and a missing one should not cost the
/// user a pickable model.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelOption {
    pub provider: String,
    pub model: String,
    pub full_model: String,
    #[serde(default)]
    pub alias: Option<String>,
}

impl ModelOption {
    /// "openai · gpt-5", the picker's label.
    pub fn label(&self) -> String {
        format!("{} · {}", self.provider, self.model)
    }
}

/// Parse a `GET /api/models` body.
///
/// `None` on a shape change, so the picker reports that it could not read the
/// model list instead of offering an empty select that looks like "no models".
pub fn parse_models(body: &str) -> Option<Vec<ModelOption>> {
    serde_json::from_str::<ModelsEnvelope>(body)
        .ok()
        .map(|envelope| envelope.models)
}

// ── requests ────────────────────────────────────────────────────────────────

/// `GET /api/combos`.
pub async fn load_combos() -> Result<ComboList, ApiError> {
    let body = crate::api::get(COMBOS_PATH).await?;
    parse_combos(&body).ok_or(ApiError::Body)
}

/// `GET /api/models`.
pub async fn load_models() -> Result<Vec<ModelOption>, ApiError> {
    let body = crate::api::get(MODELS_PATH).await?;
    parse_models(&body).ok_or(ApiError::Body)
}

/// `POST /api/combos`, returning the combo the router created.
pub async fn create_combo(body: String) -> Result<Combo, ApiError> {
    let response = crate::api::post(COMBOS_PATH, &body).await?;
    parse_combo(&response).ok_or(ApiError::Body)
}

/// `DELETE /api/combos/{id}`.
pub async fn delete_combo(id: &str) -> DeleteOutcome {
    match crate::api::delete(&combo_path(id)).await {
        Ok(_body) => DeleteOutcome::Confirmed,
        Err(error) => DeleteOutcome::Rejected(error),
    }
}
