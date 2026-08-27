//! OpenAI-compatible `/v1/models` list construction.
//!
//! Ports `buildModelsList` from `inspire/src/app/api/v1/models/route.js`. The
//! live per-provider catalog resolvers (kiro/qoder/kimchi/github/clinepass) and
//! remote `/models` probing for compatible providers are not ported; the static
//! registry catalog is used for those providers instead.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::registry;

/// Default service kind for models and combos without an explicit one.
pub const LLM_KIND: &str = "llm";

/// One row in the `OpenAI` models list.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelRow {
    pub id: String,
    pub object: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    pub owned_by: String,
}

impl ModelRow {
    fn new(id: String, owned_by: String) -> Self {
        Self {
            id,
            object: "model".to_owned(),
            kind: None,
            owned_by,
        }
    }
}

/// An active provider connection, as far as the models list cares.
#[derive(Debug, Clone, Default)]
pub struct ConnectionView {
    pub provider: String,
    /// Output prefix override (`providerSpecificData.prefix`).
    pub prefix: Option<String>,
    /// Explicit allow-list (`providerSpecificData.enabledModels`).
    pub enabled_models: Vec<String>,
}

/// A combo (multi-model fallback group).
#[derive(Debug, Clone, Default)]
pub struct ComboView {
    pub name: String,
    pub kind: Option<String>,
}

/// Everything the models list needs from persistent state.
#[derive(Debug, Clone, Default)]
pub struct ModelsListInput {
    pub connections: Vec<ConnectionView>,
    pub combos: Vec<ComboView>,
    /// alias -> `provider/model`
    pub model_aliases: BTreeMap<String, String>,
    /// models key -> disabled model ids
    pub disabled_models: BTreeMap<String, Vec<String>>,
}

/// Map a registry model kind onto a service kind (upstream `modelKind`);
/// unrecognized kinds fall back to LLM.
fn service_kind(model: &registry::Model) -> &'static str {
    match model.kind_or_default() {
        "image" => "image",
        "tts" => "tts",
        "embedding" => "embedding",
        "stt" => "stt",
        "imageToText" => "imageToText",
        _ => LLM_KIND,
    }
}

/// Heuristic kind for ids with no registry metadata
/// (upstream `inferKindFromUnknownModelId`).
fn infer_kind_from_id(model_id: &str) -> &'static str {
    let lower = model_id.to_lowercase();
    if lower.contains("embed") {
        return "embedding";
    }
    if ["tts", "speech", "audio", "voice"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        return "tts";
    }
    if [
        "image",
        "imagen",
        "dall-e",
        "dalle",
        "flux",
        "sdxl",
        "sd-",
        "stable-diffusion",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
    {
        return "image";
    }
    LLM_KIND
}

/// A provider matches when its `serviceKinds` intersect the filter; providers
/// without `serviceKinds` are LLM-only (upstream `providerMatchesKinds`).
fn provider_matches_kinds(provider_id: &str, kind_filter: &[&str]) -> bool {
    let kinds = registry::entry(provider_id)
        .and_then(|entry| entry.service_kinds.as_ref())
        .filter(|kinds| !kinds.is_empty());
    kinds.map_or_else(
        || kind_filter.contains(&LLM_KIND),
        |kinds| {
            kinds
                .iter()
                .any(|kind| kind_filter.contains(&kind.as_str()))
        },
    )
}

/// Build the models list for the requested service kinds.
pub fn build_models_list(input: &ModelsListInput, kind_filter: &[&str]) -> Vec<ModelRow> {
    let mut rows = Vec::new();

    // Combos first, matching upstream ordering.
    for combo in &input.combos {
        let kind = combo.kind.as_deref().unwrap_or(LLM_KIND);
        if !kind_filter.contains(&kind) {
            continue;
        }
        let mut row = ModelRow::new(combo.name.clone(), "combo".to_owned());
        if matches!(kind, "webSearch" | "webFetch") {
            row.kind = Some(kind.to_owned());
        }
        rows.push(row);
    }

    if input.connections.is_empty() {
        push_static_catalog(&mut rows, input, kind_filter);
    } else {
        push_connected_catalog(&mut rows, input, kind_filter);
    }

    dedupe_by_id(rows)
}

/// No active connections: expose the whole static registry catalog.
fn push_static_catalog(rows: &mut Vec<ModelRow>, input: &ModelsListInput, kind_filter: &[&str]) {
    for entry in registry::entries() {
        if entry.models.is_empty() || !provider_matches_kinds(&entry.id, kind_filter) {
            continue;
        }
        let key = entry.models_key();
        for model in &entry.models {
            if !kind_filter.contains(&service_kind(model)) || is_disabled(input, key, &model.id) {
                continue;
            }
            rows.push(ModelRow::new(format!("{key}/{}", model.id), key.to_owned()));
        }
    }
}

/// Active connections: expose only their providers' models.
fn push_connected_catalog(rows: &mut Vec<ModelRow>, input: &ModelsListInput, kind_filter: &[&str]) {
    let mut seen_providers = BTreeSet::new();
    for connection in &input.connections {
        // First connection per provider wins (upstream activeConnectionByProvider).
        if !seen_providers.insert(connection.provider.as_str())
            || !provider_matches_kinds(&connection.provider, kind_filter)
        {
            continue;
        }

        let static_alias = registry::entry(&connection.provider).map_or(
            connection.provider.as_str(),
            registry::RegistryEntry::models_key,
        );
        let output_alias = connection
            .prefix
            .as_deref()
            .map(str::trim)
            .filter(|prefix| !prefix.is_empty())
            .unwrap_or(static_alias);

        let provider_models = registry::models_for_key(static_alias);
        let model_ids = if connection.enabled_models.is_empty() {
            provider_models
                .iter()
                .map(|model| model.id.clone())
                .collect::<Vec<_>>()
        } else {
            connection
                .enabled_models
                .iter()
                .filter(|id| !id.trim().is_empty())
                .cloned()
                .collect()
        };

        let alias_model_ids =
            alias_models_for(input, output_alias, static_alias, &connection.provider);

        let mut merged = Vec::new();
        for id in model_ids.into_iter().chain(alias_model_ids) {
            let stripped =
                strip_known_prefix(&id, output_alias, static_alias, &connection.provider);
            if !stripped.trim().is_empty() && !merged.iter().any(|seen| seen == stripped) {
                merged.push(stripped.to_owned());
            }
        }

        for model_id in merged {
            let kind = registry::find_model(static_alias, &model_id)
                .map_or_else(|| infer_kind_from_id(&model_id), service_kind);
            // imageToText models stay visible in the LLM list (vision chat models).
            let allow_as_llm = kind == "imageToText" && kind_filter.contains(&LLM_KIND);
            if !kind_filter.contains(&kind) && !allow_as_llm {
                continue;
            }
            if is_disabled(input, output_alias, &model_id)
                || is_disabled(input, static_alias, &model_id)
            {
                continue;
            }
            rows.push(ModelRow::new(
                format!("{output_alias}/{model_id}"),
                output_alias.to_owned(),
            ));
        }
    }
}

/// Model ids contributed by the user's alias map for this provider.
fn alias_models_for(
    input: &ModelsListInput,
    output_alias: &str,
    static_alias: &str,
    provider_id: &str,
) -> Vec<String> {
    input
        .model_aliases
        .values()
        .filter(|full| full.contains('/'))
        .filter(|full| {
            [output_alias, static_alias, provider_id]
                .iter()
                .any(|prefix| full.starts_with(&format!("{prefix}/")))
        })
        .map(|full| strip_known_prefix(full, output_alias, static_alias, provider_id).to_owned())
        .filter(|id| !id.trim().is_empty())
        .collect()
}

/// Strip a leading `alias/` when it is one this provider answers to.
fn strip_known_prefix<'a>(
    model_id: &'a str,
    output_alias: &str,
    static_alias: &str,
    provider_id: &str,
) -> &'a str {
    for prefix in [output_alias, static_alias, provider_id] {
        if let Some(rest) = model_id.strip_prefix(prefix)
            && let Some(rest) = rest.strip_prefix('/')
        {
            return rest;
        }
    }
    model_id
}

fn is_disabled(input: &ModelsListInput, key: &str, model_id: &str) -> bool {
    input
        .disabled_models
        .get(key)
        .is_some_and(|disabled| disabled.iter().any(|entry| entry == model_id))
}

fn dedupe_by_id(rows: Vec<ModelRow>) -> Vec<ModelRow> {
    let mut seen = BTreeSet::new();
    rows.into_iter()
        .filter(|row| !row.id.is_empty() && seen.insert(row.id.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        ComboView, ConnectionView, LLM_KIND, ModelsListInput, build_models_list, infer_kind_from_id,
    };

    #[test]
    fn static_catalog_exposes_full_registry_when_no_connections() {
        let rows = build_models_list(&ModelsListInput::default(), &[LLM_KIND]);
        // Upstream exposes the whole static catalog; the previous port shipped 6 rows.
        assert!(
            rows.len() > 300,
            "expected the full LLM catalog, got {}",
            rows.len()
        );
        assert!(rows.iter().any(|row| row.id == "openai/gpt-5"));
        assert!(rows.iter().all(|row| row.object == "model"));
        // Non-LLM kinds are filtered out of the default list.
        assert!(!rows.iter().any(|row| row.id == "openai/tts-1"));
        assert!(!rows.iter().any(|row| row.id == "openai/dall-e-3"));
    }

    #[test]
    fn kind_filter_selects_non_llm_catalogs() {
        let tts = build_models_list(&ModelsListInput::default(), &["tts"]);
        assert!(tts.iter().any(|row| row.id == "openai/tts-1"));
        assert!(!tts.iter().any(|row| row.id == "openai/gpt-5"));

        let embeddings = build_models_list(&ModelsListInput::default(), &["embedding"]);
        assert!(
            embeddings
                .iter()
                .any(|row| row.id == "openai/text-embedding-3-large")
        );
    }

    #[test]
    fn connections_restrict_catalog_to_their_providers() {
        let input = ModelsListInput {
            connections: vec![ConnectionView {
                provider: "openai".to_owned(),
                ..ConnectionView::default()
            }],
            ..ModelsListInput::default()
        };
        let rows = build_models_list(&input, &[LLM_KIND]);
        assert!(rows.iter().any(|row| row.id == "openai/gpt-5"));
        assert!(rows.iter().all(|row| row.owned_by == "openai"));
    }

    #[test]
    fn enabled_models_allow_list_wins() {
        let input = ModelsListInput {
            connections: vec![ConnectionView {
                provider: "openai".to_owned(),
                enabled_models: vec!["gpt-5".to_owned()],
                ..ConnectionView::default()
            }],
            ..ModelsListInput::default()
        };
        let rows = build_models_list(&input, &[LLM_KIND]);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows.first().map(|row| row.id.as_str()),
            Some("openai/gpt-5")
        );
    }

    #[test]
    fn prefix_override_renames_output_alias() {
        let input = ModelsListInput {
            connections: vec![ConnectionView {
                provider: "openai".to_owned(),
                prefix: Some("mine".to_owned()),
                enabled_models: vec!["gpt-5".to_owned()],
            }],
            ..ModelsListInput::default()
        };
        let rows = build_models_list(&input, &[LLM_KIND]);
        assert_eq!(rows.first().map(|row| row.id.as_str()), Some("mine/gpt-5"));
        assert_eq!(rows.first().map(|row| row.owned_by.as_str()), Some("mine"));
    }

    #[test]
    fn disabled_models_are_hidden() {
        let mut input = ModelsListInput {
            connections: vec![ConnectionView {
                provider: "openai".to_owned(),
                enabled_models: vec!["gpt-5".to_owned(), "gpt-4o".to_owned()],
                ..ConnectionView::default()
            }],
            ..ModelsListInput::default()
        };
        input
            .disabled_models
            .insert("openai".to_owned(), vec!["gpt-5".to_owned()]);
        let rows = build_models_list(&input, &[LLM_KIND]);
        assert_eq!(
            rows.first().map(|row| row.id.as_str()),
            Some("openai/gpt-4o")
        );
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn combos_come_first_and_carry_web_kinds() {
        let input = ModelsListInput {
            connections: vec![ConnectionView {
                provider: "openai".to_owned(),
                enabled_models: vec!["gpt-5".to_owned()],
                ..ConnectionView::default()
            }],
            combos: vec![
                ComboView {
                    name: "my-combo".to_owned(),
                    kind: None,
                },
                ComboView {
                    name: "my-search".to_owned(),
                    kind: Some("webSearch".to_owned()),
                },
            ],
            ..ModelsListInput::default()
        };
        let rows = build_models_list(&input, &[LLM_KIND]);
        assert_eq!(rows.first().map(|row| row.id.as_str()), Some("my-combo"));
        assert_eq!(rows.first().map(|row| row.owned_by.as_str()), Some("combo"));
        // webSearch combo is excluded from the LLM filter.
        assert!(!rows.iter().any(|row| row.id == "my-search"));

        let search = build_models_list(&input, &["webSearch"]);
        assert_eq!(
            search.first().and_then(|row| row.kind.as_deref()),
            Some("webSearch")
        );
    }

    #[test]
    fn unknown_ids_get_heuristic_kinds() {
        assert_eq!(infer_kind_from_id("text-embedding-foo"), "embedding");
        assert_eq!(infer_kind_from_id("some-tts-model"), "tts");
        assert_eq!(infer_kind_from_id("flux-pro"), "image");
        assert_eq!(infer_kind_from_id("my-chat-model"), LLM_KIND);
    }
}
