use std::{collections::BTreeMap, sync::Mutex};

type PricingFields = BTreeMap<String, serde_json::Value>;
type PricingModels = BTreeMap<String, PricingFields>;
pub(super) type PricingCatalog = BTreeMap<String, PricingModels>;

#[derive(Debug, Default)]
pub(super) struct PricingStore {
    user_pricing: Mutex<PricingCatalog>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct PricingStoreError;

impl PricingStore {
    pub(super) fn merged(&self) -> Result<PricingCatalog, PricingStoreError> {
        self.with_user_pricing(|user_pricing| merged_pricing(user_pricing))
    }

    pub(super) fn update(
        &self,
        update: PricingCatalog,
    ) -> Result<PricingCatalog, PricingStoreError> {
        self.with_user_pricing(|user_pricing| merge_user_pricing(user_pricing, update))
    }

    pub(super) fn reset(
        &self,
        provider: Option<&str>,
        model: Option<&str>,
    ) -> Result<PricingCatalog, PricingStoreError> {
        self.with_user_pricing(|user_pricing| {
            reset_user_pricing(user_pricing, provider, model);
            merged_pricing(user_pricing)
        })
    }

    fn with_user_pricing<T>(
        &self,
        action: impl FnOnce(&mut PricingCatalog) -> T,
    ) -> Result<T, PricingStoreError> {
        let Ok(mut user_pricing) = self.user_pricing.lock() else {
            return Err(PricingStoreError);
        };
        Ok(action(&mut user_pricing))
    }
}

pub(super) fn parse_pricing_update(value: serde_json::Value) -> Result<PricingCatalog, String> {
    let serde_json::Value::Object(providers) = value else {
        return Err("Invalid pricing data format".to_owned());
    };
    let mut catalog = PricingCatalog::new();

    for (provider, models_value) in providers {
        let serde_json::Value::Object(models) = models_value else {
            return Err(format!("Invalid pricing for provider: {provider}"));
        };
        let mut parsed_models = PricingModels::new();
        for (model, pricing_value) in models {
            let serde_json::Value::Object(fields) = pricing_value else {
                return Err(format!("Invalid pricing for model: {provider}/{model}"));
            };
            parsed_models.insert(
                model.clone(),
                parse_pricing_fields(&provider, &model, fields)?,
            );
        }
        catalog.insert(provider, parsed_models);
    }

    Ok(catalog)
}

fn parse_pricing_fields(
    provider: &str,
    model: &str,
    fields: serde_json::Map<String, serde_json::Value>,
) -> Result<PricingFields, String> {
    let mut parsed_fields = PricingFields::new();
    for (key, value) in fields {
        if !VALID_PRICING_FIELDS.contains(&key.as_str()) {
            return Err(format!(
                "Invalid pricing field: {key} for {provider}/{model}"
            ));
        }
        if !is_non_negative_number(&value) {
            return Err(format!(
                "Invalid pricing value for {key} in {provider}/{model}: must be non-negative number"
            ));
        }
        parsed_fields.insert(key, value);
    }
    Ok(parsed_fields)
}

fn is_non_negative_number(value: &serde_json::Value) -> bool {
    value
        .as_f64()
        .is_some_and(|number| number.is_finite() && number >= 0.0)
}

fn merge_user_pricing(user_pricing: &mut PricingCatalog, update: PricingCatalog) -> PricingCatalog {
    for (provider, models) in update {
        let provider_entry = user_pricing.entry(provider).or_default();
        for (model, fields) in models {
            provider_entry.insert(model, fields);
        }
    }
    user_pricing.clone()
}

fn reset_user_pricing(
    user_pricing: &mut PricingCatalog,
    provider: Option<&str>,
    model: Option<&str>,
) {
    let Some(provider) = provider else {
        user_pricing.clear();
        return;
    };
    let Some(model) = model else {
        user_pricing.remove(provider);
        return;
    };
    let should_remove_provider = user_pricing.get_mut(provider).is_some_and(|models| {
        models.remove(model);
        models.is_empty()
    });
    if should_remove_provider {
        user_pricing.remove(provider);
    }
}

fn merged_pricing(user_pricing: &PricingCatalog) -> PricingCatalog {
    let mut merged = default_pricing();
    for (provider, models) in user_pricing {
        let provider_entry = merged.entry(provider.clone()).or_default();
        for (model, fields) in models {
            match provider_entry.get_mut(model) {
                Some(existing_fields) => {
                    for (key, value) in fields {
                        existing_fields.insert(key.clone(), value.clone());
                    }
                }
                None => {
                    provider_entry.insert(model.clone(), fields.clone());
                }
            }
        }
    }
    merged
}

fn default_pricing() -> PricingCatalog {
    let mut providers = PricingCatalog::new();
    let mut gh = PricingModels::new();
    gh.insert(
        "gpt-5.3-codex".to_owned(),
        pricing_fields(&[
            ("input", 1.75),
            ("output", 14.0),
            ("cached", 0.175),
            ("reasoning", 14.0),
            ("cache_creation", 1.75),
        ]),
    );
    providers.insert("gh".to_owned(), gh);
    providers
}

fn pricing_fields(fields: &[(&str, f64)]) -> PricingFields {
    fields
        .iter()
        .map(|(key, value)| ((*key).to_owned(), serde_json::json!(value)))
        .collect()
}

const VALID_PRICING_FIELDS: &[&str] = &["input", "output", "cached", "reasoning", "cache_creation"];
