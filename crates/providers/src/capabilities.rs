//! Per-model capabilities: context window, output ceiling, modalities, and
//! thinking format.
//!
//! Upstream resolves these through a four-stage cascade in
//! `open-sse/providers/capabilities.js` (provider override → canonical exact →
//! ordered glob patterns → defaults). Rather than re-implementing that
//! pattern-matching — which would drift from upstream on every model added —
//! `data/capabilities.json` holds the *resolved* result for every registry
//! model, dumped by calling upstream's own `getCapabilitiesForModel`.
//!
//! Only deltas from [`Capabilities::DEFAULT`] are stored, so the table stays
//! small. Models absent from the table use the defaults, which is also what
//! upstream does for an unknown id.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde::Deserialize;

const CAPABILITIES_JSON: &str = include_str!("../data/capabilities.json");

#[derive(Debug, Deserialize)]
// The dump emits camelCase; without this the `byProviderModel` key would not
// bind and `serde(default)` would silently yield an empty table.
#[serde(rename_all = "camelCase")]
struct CapabilitiesTable {
    #[serde(default)]
    default: Capabilities,
    /// `provider/model` -> fields that differ from the default.
    #[serde(default)]
    by_provider_model: BTreeMap<String, CapabilitiesDelta>,
}

static TABLE: LazyLock<CapabilitiesTable> = LazyLock::new(|| {
    serde_json::from_str(CAPABILITIES_JSON).unwrap_or_else(|_| CapabilitiesTable {
        default: Capabilities::DEFAULT,
        by_provider_model: BTreeMap::new(),
    })
});

/// What a model can read, emit, and how much it can handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    // input modalities
    pub vision: bool,
    pub pdf: bool,
    pub audio_input: bool,
    pub video_input: bool,
    // output modalities
    pub image_output: bool,
    pub audio_output: bool,
    // features
    pub search: bool,
    pub tools: bool,
    pub reasoning: bool,
    /// Whether the model can turn thinking off entirely.
    pub thinking_can_disable: bool,
    // limits (tokens)
    pub context_window: u64,
    pub max_output: u64,
}

impl Capabilities {
    /// Upstream `DEFAULT_CAPABILITIES` — the safe floor every result merges over.
    pub const DEFAULT: Self = Self {
        vision: false,
        pdf: false,
        audio_input: false,
        video_input: false,
        image_output: false,
        audio_output: false,
        search: false,
        tools: true,
        reasoning: false,
        thinking_can_disable: true,
        context_window: 200_000,
        max_output: 64_000,
    };
}

impl Default for Capabilities {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// A sparse override applied over [`Capabilities::DEFAULT`].
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CapabilitiesDelta {
    #[serde(default)]
    vision: Option<bool>,
    #[serde(default)]
    pdf: Option<bool>,
    #[serde(default)]
    audio_input: Option<bool>,
    #[serde(default)]
    video_input: Option<bool>,
    #[serde(default)]
    image_output: Option<bool>,
    #[serde(default)]
    audio_output: Option<bool>,
    #[serde(default)]
    search: Option<bool>,
    #[serde(default)]
    tools: Option<bool>,
    #[serde(default)]
    reasoning: Option<bool>,
    #[serde(default)]
    thinking_can_disable: Option<bool>,
    #[serde(default)]
    context_window: Option<u64>,
    #[serde(default)]
    max_output: Option<u64>,
    /// Provider-native thinking wire format (`claude-adaptive`, `qwen`, ...).
    ///
    /// Parsed so the dumped table stays lossless. Not yet consumed: provider-
    /// native thinking normalization is not ported.
    #[allow(dead_code, reason = "retained for fidelity until thinking is ported")]
    #[serde(default)]
    thinking_format: Option<String>,
}

impl CapabilitiesDelta {
    #[allow(
        clippy::missing_const_for_fn,
        reason = "Option::is_some on a String field is not const-stable"
    )]
    fn apply(&self, mut base: Capabilities) -> Capabilities {
        if let Some(value) = self.vision {
            base.vision = value;
        }
        if let Some(value) = self.pdf {
            base.pdf = value;
        }
        if let Some(value) = self.audio_input {
            base.audio_input = value;
        }
        if let Some(value) = self.video_input {
            base.video_input = value;
        }
        if let Some(value) = self.image_output {
            base.image_output = value;
        }
        if let Some(value) = self.audio_output {
            base.audio_output = value;
        }
        if let Some(value) = self.search {
            base.search = value;
        }
        if let Some(value) = self.tools {
            base.tools = value;
        }
        if let Some(value) = self.reasoning {
            base.reasoning = value;
        }
        if let Some(value) = self.thinking_can_disable {
            base.thinking_can_disable = value;
        }
        if let Some(value) = self.context_window {
            base.context_window = value;
        }
        if let Some(value) = self.max_output {
            base.max_output = value;
        }
        base
    }
}

/// Capabilities for a provider/model pair.
///
/// A vendor-prefixed model id (`anthropic/claude-opus-4.7`) is reduced to its
/// last segment before lookup, matching upstream's `baseModel` handling. The
/// registry's own `maxOutputTokens` / `contextLength`, when present, take
/// precedence — it is the more specific statement.
pub fn for_model(provider: &str, model: &str) -> Capabilities {
    if model.is_empty() {
        return TABLE.default;
    }
    let base_model = model.rsplit('/').next().unwrap_or(model);

    let mut resolved = TABLE
        .by_provider_model
        .get(&format!("{provider}/{base_model}"))
        .or_else(|| TABLE.by_provider_model.get(&format!("{provider}/{model}")))
        .map_or(TABLE.default, |delta| delta.apply(TABLE.default));

    // Registry-stated limits win over the capability table.
    let key = crate::registry::entry(provider)
        .map_or(provider, crate::registry::RegistryEntry::models_key);
    if let Some(entry) = crate::registry::find_model(key, base_model) {
        if let Some(max_output) = entry.max_output_tokens {
            resolved.max_output = max_output;
        }
        if let Some(context) = entry.context_length {
            resolved.context_window = context;
        }
    }

    resolved
}

/// Output ceiling for a provider/model pair.
///
/// This is the value `max_tokens` is clamped to; using the default for a
/// high-output model would silently truncate long completions.
pub fn max_output(provider: &str, model: &str) -> u64 {
    for_model(provider, model).max_output
}

#[cfg(test)]
mod tests {
    use super::{Capabilities, for_model, max_output};

    #[test]
    fn table_parses_and_is_populated() {
        // Guards the LazyLock fallback: a malformed table would silently
        // degrade every model to defaults.
        assert!(
            super::TABLE.by_provider_model.len() > 400,
            "expected the dumped capability table, got {}",
            super::TABLE.by_provider_model.len()
        );
        assert_eq!(super::TABLE.default.max_output, 64_000);
        assert_eq!(super::TABLE.default.context_window, 200_000);
    }

    #[test]
    fn unknown_models_fall_back_to_defaults() {
        assert_eq!(for_model("nope", "nope"), Capabilities::DEFAULT);
        assert_eq!(for_model("openai", ""), Capabilities::DEFAULT);
    }

    #[test]
    fn defaults_allow_tools_but_not_vision() {
        let caps = Capabilities::DEFAULT;
        assert!(caps.tools, "tools are on by default upstream");
        assert!(!caps.vision);
        assert!(!caps.reasoning);
    }

    #[test]
    fn high_output_models_exceed_the_conservative_default() {
        // The whole point of this table: some models allow far more than 64000,
        // and clamping them to the default would truncate real completions.
        let ceilings: Vec<u64> = super::TABLE
            .by_provider_model
            .values()
            .filter_map(|delta| delta.max_output)
            .collect();
        assert!(
            ceilings.iter().any(|ceiling| *ceiling > 64_000),
            "expected at least one above-default ceiling"
        );
        assert!(
            ceilings.iter().any(|ceiling| *ceiling >= 128_000),
            "expected a 128k-class ceiling"
        );
    }

    #[test]
    fn vendor_prefixed_ids_resolve_to_the_base_model() {
        // Find any table entry, then confirm the prefixed form resolves too.
        let Some((key, _)) = super::TABLE.by_provider_model.iter().next() else {
            panic!("capability table is empty");
        };
        let Some((provider, model)) = key.split_once('/') else {
            panic!("malformed capability key: {key}");
        };
        let direct = for_model(provider, model);
        let prefixed = for_model(provider, &format!("{provider}/{model}"));
        assert_eq!(direct, prefixed, "prefixed id must resolve identically");
    }

    #[test]
    fn max_output_is_never_zero() {
        // A zero ceiling would clamp every request to nothing.
        for (key, _) in super::TABLE.by_provider_model.iter().take(200) {
            let Some((provider, model)) = key.split_once('/') else {
                continue;
            };
            assert!(
                max_output(provider, model) > 0,
                "{key} resolved to a zero ceiling"
            );
        }
    }
}
