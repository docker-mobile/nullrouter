//! Filtering a provider's published model catalogue down to the useful subset.
//!
//! Ports `src/app/api/providers/suggested-models/filters.js`. A gateway like OpenRouter
//! serves hundreds of models; the dashboard's "suggested" list is the handful worth
//! offering, and each provider's definition of that differs.
//!
//! The filters are here rather than in the API service because they are registry knowledge:
//! which subset matters is a property of the provider, and the shapes are those of the
//! catalogues the registry points at.

use serde::Deserialize;

/// One model from a provider's catalogue.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestedModel {
    pub id: String,
    pub name: String,
    /// Present only where the catalogue reports it and the filter sorts on it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u64>,
}

/// Free OpenCode models that do not carry the `-free` id suffix (upstream
/// `KNOWN_FREE_OPENCODE_MODELS`).
const KNOWN_FREE_OPENCODE: [&str; 1] = ["big-pickle"];

/// An OpenRouter-shaped catalogue row.
#[derive(Debug, Deserialize)]
struct CatalogueRow {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    context_length: Option<u64>,
    #[serde(default)]
    pricing: Option<Pricing>,
}

/// Prices are strings in OpenRouter's catalogue, and `"0"` means free.
#[derive(Debug, Deserialize)]
struct Pricing {
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    completion: Option<String>,
}

impl Pricing {
    fn is_free(&self) -> bool {
        self.prompt.as_deref() == Some("0") && self.completion.as_deref() == Some("0")
    }
}

/// The minimum context window an OpenRouter free model must have to be suggested
/// (upstream `context_length >= 200000`).
const OPENROUTER_MIN_CONTEXT: u64 = 200_000;

/// Pull the catalogue array out of whichever key the provider used.
///
/// Upstream: `json.data ?? json.models ?? json`. A bare top-level array is accepted because
/// `models.dev` returns one.
fn rows(body: &serde_json::Value) -> Vec<CatalogueRow> {
    let array = body
        .get("data")
        .or_else(|| body.get("models"))
        .unwrap_or(body);
    array
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|row| serde_json::from_value::<CatalogueRow>(row.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Apply the named filter to a catalogue body.
///
/// `None` for an unknown filter name, which the caller reports as a 400 rather than as an
/// empty list — an empty list would read as "this provider has no free models".
pub fn filter_catalogue(filter: &str, body: &serde_json::Value) -> Option<Vec<SuggestedModel>> {
    let rows = rows(body);
    match filter {
        "openrouter-free" => {
            let mut models: Vec<SuggestedModel> = rows
                .into_iter()
                .filter(|row| {
                    row.pricing.as_ref().is_some_and(Pricing::is_free)
                        && row.context_length.unwrap_or(0) >= OPENROUTER_MIN_CONTEXT
                })
                .map(|row| SuggestedModel {
                    name: if row.name.is_empty() {
                        row.id.clone()
                    } else {
                        row.name
                    },
                    id: row.id,
                    context_length: row.context_length,
                })
                .collect();
            // Largest context first, as upstream sorts.
            models.sort_by(|left, right| {
                right
                    .context_length
                    .unwrap_or(0)
                    .cmp(&left.context_length.unwrap_or(0))
            });
            Some(models)
        }
        "opencode-free" => Some(
            rows.into_iter()
                .filter(|row| {
                    row.id.ends_with("-free") || KNOWN_FREE_OPENCODE.contains(&row.id.as_str())
                })
                .map(|row| SuggestedModel {
                    // Upstream uses the id for both fields here, not the display name.
                    name: row.id.clone(),
                    id: row.id,
                    context_length: None,
                })
                .collect(),
        ),
        "mimo-free" => Some(
            rows.into_iter()
                .filter(|row| {
                    row.id.starts_with("mimo") || row.name.to_lowercase().contains("mimo")
                })
                .map(|row| SuggestedModel {
                    name: if row.name.is_empty() {
                        row.id.clone()
                    } else {
                        row.name
                    },
                    id: row.id,
                    context_length: None,
                })
                .collect(),
        ),
        // Not in upstream's FILTERS, which is an upstream bug rather than a decision: four
        // providers declare `type: "openai"` (perplexity-agent, venice, tokenrouter,
        // vercel-ai-gateway) and upstream's route answers 400 for all four, so their
        // suggested-model lists never populate. The plain OpenAI catalogue shape needs no
        // filtering — every model listed is one the provider serves.
        "openai" => Some(
            rows.into_iter()
                .filter(|row| !row.id.trim().is_empty())
                .map(|row| SuggestedModel {
                    name: if row.name.is_empty() {
                        row.id.clone()
                    } else {
                        row.name
                    },
                    id: row.id,
                    context_length: row.context_length,
                })
                .collect(),
        ),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{SuggestedModel, filter_catalogue};
    use serde_json::json;

    fn ids(models: &[SuggestedModel]) -> Vec<&str> {
        models.iter().map(|model| model.id.as_str()).collect()
    }

    fn names(models: &[SuggestedModel]) -> Vec<&str> {
        models.iter().map(|model| model.name.as_str()).collect()
    }

    #[test]
    fn openrouter_keeps_only_free_models_with_a_large_context() {
        let body = json!({"data": [
            {"id": "free-big", "name": "Free Big", "context_length": 400_000,
             "pricing": {"prompt": "0", "completion": "0"}},
            {"id": "free-small", "name": "Free Small", "context_length": 8_192,
             "pricing": {"prompt": "0", "completion": "0"}},
            {"id": "paid-big", "name": "Paid Big", "context_length": 1_000_000,
             "pricing": {"prompt": "0.001", "completion": "0.002"}},
            {"id": "half-free", "name": "Half", "context_length": 500_000,
             "pricing": {"prompt": "0", "completion": "0.002"}},
        ]});
        let models = filter_catalogue("openrouter-free", &body).expect("known filter");
        assert_eq!(ids(&models), vec!["free-big"]);
    }

    #[test]
    fn openrouter_sorts_by_context_descending() {
        let body = json!({"data": [
            {"id": "mid", "context_length": 300_000, "pricing": {"prompt": "0", "completion": "0"}},
            {"id": "big", "context_length": 900_000, "pricing": {"prompt": "0", "completion": "0"}},
            {"id": "small", "context_length": 200_000, "pricing": {"prompt": "0", "completion": "0"}},
        ]});
        let models = filter_catalogue("openrouter-free", &body).expect("known filter");
        assert_eq!(ids(&models), vec!["big", "mid", "small"]);
    }

    #[test]
    fn the_openrouter_context_floor_is_inclusive() {
        // Upstream is `>= 200000`. An exclusive check would silently drop a whole tier.
        let body = json!({"data": [
            {"id": "exactly", "context_length": 200_000,
             "pricing": {"prompt": "0", "completion": "0"}},
        ]});
        let models = filter_catalogue("openrouter-free", &body).expect("known filter");
        assert_eq!(ids(&models), vec!["exactly"]);
    }

    #[test]
    fn opencode_keeps_the_free_suffix_and_the_known_exceptions() {
        let body = json!({"data": [
            {"id": "model-free"},
            {"id": "big-pickle"},
            {"id": "paid-model"},
        ]});
        let models = filter_catalogue("opencode-free", &body).expect("known filter");
        assert_eq!(ids(&models), vec!["model-free", "big-pickle"]);
    }

    #[test]
    fn opencode_names_a_model_by_its_id() {
        // Upstream maps name to the id here rather than the display name.
        let body = json!({"data": [{"id": "model-free", "name": "Pretty Name"}]});
        let models = filter_catalogue("opencode-free", &body).expect("known filter");
        assert_eq!(names(&models), vec!["model-free"]);
    }

    #[test]
    fn mimo_matches_on_id_prefix_or_name() {
        let body = json!([
            {"id": "mimo-auto", "name": "Mimo Auto"},
            {"id": "other", "name": "Has MiMo inside"},
            {"id": "unrelated", "name": "Nothing"},
        ]);
        let models = filter_catalogue("mimo-free", &body).expect("known filter");
        assert_eq!(ids(&models), vec!["mimo-auto", "other"]);
    }

    #[test]
    fn a_bare_top_level_array_is_read() {
        // models.dev returns one, so `json.data ?? json.models ?? json` matters.
        let body = json!([{"id": "mimo-x", "name": "Mimo X"}]);
        let models = filter_catalogue("mimo-free", &body).expect("known filter");
        assert_eq!(ids(&models), vec!["mimo-x"]);
    }

    #[test]
    fn a_models_key_is_read_as_well_as_data() {
        let body = json!({"models": [{"id": "mimo-y"}]});
        let models = filter_catalogue("mimo-free", &body).expect("known filter");
        assert_eq!(ids(&models), vec!["mimo-y"]);
    }

    #[test]
    fn the_openai_filter_passes_every_listed_model() {
        // Upstream has no such filter and answers 400 for the four providers that declare
        // it, so their lists never populate. A plain catalogue needs no filtering.
        let body = json!({"data": [{"id": "sonar"}, {"id": "sonar-pro", "name": "Sonar Pro"}]});
        let models = filter_catalogue("openai", &body).expect("known filter");
        assert_eq!(ids(&models), vec!["sonar", "sonar-pro"]);
        assert_eq!(names(&models), vec!["sonar", "Sonar Pro"]);
    }

    #[test]
    fn a_model_with_no_name_falls_back_to_its_id() {
        let body = json!({"data": [{"id": "bare"}]});
        let models = filter_catalogue("openai", &body).expect("known filter");
        assert_eq!(names(&models), vec!["bare"]);
    }

    #[test]
    fn an_unknown_filter_is_none_not_an_empty_list() {
        // The caller reports 400. An empty list would read as "no free models here".
        assert!(filter_catalogue("no-such-filter", &json!({"data": []})).is_none());
    }

    #[test]
    fn a_body_of_the_wrong_shape_yields_an_empty_list() {
        // A provider that answers 200 with something unexpected is not an error to report
        // as a 500; upstream returns an empty list and so does this.
        let models = filter_catalogue("openai", &json!({"unexpected": true})).expect("known");
        assert!(models.is_empty());
    }

    #[test]
    fn every_registry_fetcher_names_a_filter_this_module_implements() {
        // The reason this test exists: adding a provider with a new filter name upstream
        // would otherwise show up as an empty suggested-models list in the dashboard, with
        // nothing in any log to say why.
        let mut checked = 0;
        for entry in crate::registry::entries() {
            if let Some(fetcher) = entry.models_fetcher.as_ref() {
                checked += 1;
                assert!(
                    filter_catalogue(&fetcher.filter, &json!({"data": []})).is_some(),
                    "provider {} declares filter {:?}, which is not implemented",
                    entry.id,
                    fetcher.filter
                );
            }
        }
        assert!(checked > 0, "no registry entry carries a modelsFetcher");
    }

    #[test]
    fn every_registry_fetcher_url_passes_the_route_allowlist() {
        // The other half of the pairing. `/api/providers/suggested-models` refuses any URL
        // the registry does not declare, so a declared URL the predicate does not recognise
        // would give the dashboard a permanently empty list with nothing to explain it.
        //
        // Here rather than in the API service's tests because it is registry logic and needs
        // no HTTP boundary — and because testing it there would have meant fetching a third
        // party's catalogue on every `cargo test`.
        for entry in crate::registry::entries() {
            if let Some(fetcher) = entry.models_fetcher.as_ref() {
                assert!(
                    crate::registry::declares_models_url(&fetcher.url),
                    "provider {} declares {}, which the allowlist does not recognise",
                    entry.id,
                    fetcher.url
                );
            }
        }
    }

    #[test]
    fn a_url_sharing_a_host_with_a_declared_one_is_not_allowed() {
        // The allowlist is an exact match. A host or prefix match would still permit every
        // other path on that host, including whatever an open redirect there can reach.
        assert!(crate::registry::declares_models_url(
            "https://openrouter.ai/api/v1/models"
        ));
        for near_miss in [
            "https://openrouter.ai/api/v1/keys",
            "https://openrouter.ai/api/v1/models/../keys",
            "https://openrouter.ai/api/v1/models?x=1",
            "http://openrouter.ai/api/v1/models",
            "https://openrouter.ai/api/v1/models/",
        ] {
            assert!(
                !crate::registry::declares_models_url(near_miss),
                "{near_miss} should not pass the allowlist"
            );
        }
    }

    #[test]
    fn a_loopback_url_is_not_allowed() {
        // The case that matters: this router's own internal services are on loopback, and
        // reaching them is exactly what an SSRF would attempt.
        for url in [
            "http://127.0.0.1:20134/internal/v1/probe-targets",
            "http://localhost:20128/api/settings",
            "http://169.254.169.254/latest/meta-data/",
        ] {
            assert!(!crate::registry::declares_models_url(url), "{url}");
        }
    }
}
