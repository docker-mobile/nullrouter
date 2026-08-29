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

/// Provider-native thinking wire format.
///
/// Upstream keys `applyFormat`'s switch on these strings
/// (`open-sse/translator/concerns/thinkingUnified.js`). A value this build does
/// not know is kept as [`Self::Unrecognized`] rather than discarded: upstream's
/// switch falls through to a no-op for an unknown format, whereas dropping it to
/// `None` here would instead fall back to the *target format's* native default
/// and apply something the model never asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingFormat {
    OpenAi,
    ClaudeAdaptive,
    ClaudeBudget,
    GeminiLevel,
    GeminiBudget,
    Zai,
    Qwen,
    Kimi,
    DeepSeek,
    MiniMax,
    Hunyuan,
    Step,
    TokenRouter,
    Kiro,
    /// A format string this build does not recognise. Applied as a no-op.
    Unrecognized,
}

impl ThinkingFormat {
    /// Read an upstream `thinkingFormat` string. Never fails.
    pub fn from_wire(raw: &str) -> Self {
        match raw {
            "openai" => Self::OpenAi,
            "claude-adaptive" => Self::ClaudeAdaptive,
            "claude-budget" => Self::ClaudeBudget,
            "gemini-level" => Self::GeminiLevel,
            "gemini-budget" => Self::GeminiBudget,
            "zai" => Self::Zai,
            "qwen" => Self::Qwen,
            "kimi" => Self::Kimi,
            "deepseek" => Self::DeepSeek,
            "minimax" => Self::MiniMax,
            "hunyuan" => Self::Hunyuan,
            "step" => Self::Step,
            "tokenrouter" => Self::TokenRouter,
            "kiro" => Self::Kiro,
            _ => Self::Unrecognized,
        }
    }

    /// The upstream string identifier.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::ClaudeAdaptive => "claude-adaptive",
            Self::ClaudeBudget => "claude-budget",
            Self::GeminiLevel => "gemini-level",
            Self::GeminiBudget => "gemini-budget",
            Self::Zai => "zai",
            Self::Qwen => "qwen",
            Self::Kimi => "kimi",
            Self::DeepSeek => "deepseek",
            Self::MiniMax => "minimax",
            Self::Hunyuan => "hunyuan",
            Self::Step => "step",
            Self::TokenRouter => "tokenrouter",
            Self::Kiro => "kiro",
            Self::Unrecognized => "unrecognized",
        }
    }
}

impl<'de> Deserialize<'de> for ThinkingFormat {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Lenient by design: an unknown format must not fail the whole table
        // parse, which would degrade every model to defaults via the LazyLock
        // fallback.
        Ok(Self::from_wire(&String::deserialize(deserializer)?))
    }
}

/// Inclusive clamp applied to a resolved thinking budget.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub struct ThinkingRange {
    #[serde(default)]
    pub min: Option<u64>,
    #[serde(default)]
    pub max: Option<u64>,
}

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
    /// The wire shape this model's reasoning controls take, when it states one.
    ///
    /// `None` means the model does not name a format, and the target wire
    /// format's native default is used instead.
    #[serde(default)]
    pub thinking_format: Option<ThinkingFormat>,
    /// Clamp applied to a resolved thinking budget, when the model states one.
    #[serde(default)]
    pub thinking_range: Option<ThinkingRange>,
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
        thinking_format: None,
        thinking_range: None,
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
    #[serde(default)]
    thinking_format: Option<ThinkingFormat>,
    /// Clamp for a budget-shaped thinking format.
    #[serde(default)]
    thinking_range: Option<ThinkingRange>,
}

impl CapabilitiesDelta {
    #[allow(
        clippy::missing_const_for_fn,
        reason = "const buys nothing here: called once per lookup behind a LazyLock"
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
        if let Some(value) = self.thinking_format {
            base.thinking_format = Some(value);
        }
        if let Some(value) = self.thinking_range {
            base.thinking_range = Some(value);
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

// Shared level sets, ported from `open-sse/providers/thinkingLevels.js`.
const LEVELS_BASE: &[&str] = &["none", "low", "medium", "high"];
const LEVELS_ON_OFF: &[&str] = &["none", "thinking"];
const LEVELS_OPENAI: &[&str] = &["none", "minimal", "low", "medium", "high", "xhigh"];
const LEVELS_LEVEL_MAX: &[&str] = &["none", "low", "medium", "high", "max"];
const LEVELS_BUDGET_X: &[&str] = &["none", "low", "medium", "high", "xhigh", "max"];
/// Gemini 3's `thinkingLevel` enum, which has no "off".
const LEVELS_GEMINI: &[&str] = &["minimal", "low", "medium", "high"];
const LEVELS_HI_MAX: &[&str] = &["none", "high", "max"];

const CODEX_GPT_5_6: &[&str] = &["none", "minimal", "low", "medium", "high", "xhigh", "max"];
const CODEX_GPT_5_6_ULTRA: &[&str] = &[
    "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
];
const CODEX_GENERIC: &[&str] = &["low", "medium", "high", "xhigh"];

/// Model-name overrides, first match wins. Mirrors `PATTERN_THINKING`.
///
/// A `None` scope matches every provider.
const PATTERN_THINKING: &[(Option<&str>, &str, &[&str])] = &[
    (Some("codex"), "*gpt-5.6-sol*", CODEX_GPT_5_6_ULTRA),
    (Some("codex"), "*gpt-5.6-terra*", CODEX_GPT_5_6_ULTRA),
    (Some("codex"), "*gpt-5.6-luna*", CODEX_GPT_5_6),
    // Codex cannot disable thinking at all.
    (None, "*codex*", CODEX_GENERIC),
];

/// The levels a thinking format accepts.
///
/// Formats absent from upstream's `FORMAT_LEVELS` table resolve to the base set,
/// which is what its `|| L.base` does.
const fn format_levels(format: ThinkingFormat) -> &'static [&'static str] {
    match format {
        ThinkingFormat::OpenAi => LEVELS_OPENAI,
        ThinkingFormat::ClaudeAdaptive | ThinkingFormat::Kimi => LEVELS_LEVEL_MAX,
        ThinkingFormat::ClaudeBudget => LEVELS_BUDGET_X,
        ThinkingFormat::GeminiLevel => LEVELS_GEMINI,
        ThinkingFormat::Zai | ThinkingFormat::MiniMax => LEVELS_ON_OFF,
        ThinkingFormat::DeepSeek => LEVELS_HI_MAX,
        ThinkingFormat::GeminiBudget
        | ThinkingFormat::Qwen
        | ThinkingFormat::Hunyuan
        | ThinkingFormat::Step
        | ThinkingFormat::TokenRouter
        | ThinkingFormat::Kiro
        | ThinkingFormat::Unrecognized => LEVELS_BASE,
    }
}

/// Whether `pattern` matches `value`, with `*` as the only wildcard.
///
/// Upstream `matchPattern` compiles a case-insensitive anchored regex, so both
/// sides are lowercased and the literal segments between wildcards are walked in
/// order.
fn glob_matches(pattern: &str, value: &str) -> bool {
    let pattern = pattern.to_ascii_lowercase();
    let value = value.to_ascii_lowercase();
    let segments: Vec<&str> = pattern.split('*').collect();
    let Some((first, tail_segments)) = segments.split_first() else {
        return false;
    };
    // No wildcard at all: an exact match.
    if tail_segments.is_empty() {
        return value == *first;
    }
    // The leading segment is anchored at the start of the value.
    let Some(mut cursor) = value.strip_prefix(first) else {
        return false;
    };
    let Some((last, middle)) = tail_segments.split_last() else {
        return false;
    };
    // Interior segments may sit anywhere, in order. Leftmost match each.
    for segment in middle {
        let Some(at) = cursor.find(segment) else {
            return false;
        };
        cursor = cursor.get(at + segment.len()..).unwrap_or_default();
    }
    // The trailing segment is anchored at the end.
    cursor.ends_with(last)
}

/// The thinking levels a model accepts, or `None` when it does not reason.
///
/// Ports `getThinkingLevels`. Only the OpenAI thinking format consumes this, to
/// decide whether a requested `max`/`ultra` survives or clamps down to `xhigh` —
/// sending a level the model rejects is a 400, not a downgrade.
pub fn thinking_levels(provider: &str, model: &str) -> Option<Vec<&'static str>> {
    let caps = for_model(provider, model);
    if !caps.reasoning {
        return None;
    }
    let base_model = model.rsplit('/').next().unwrap_or(model);
    let matched = PATTERN_THINKING
        .iter()
        .find(|entry| {
            entry.0.is_none_or(|owner| owner == provider) && glob_matches(entry.1, base_model)
        })
        .map(|entry| entry.2);
    let levels = matched.unwrap_or_else(|| caps.thinking_format.map_or(LEVELS_BASE, format_levels));
    if caps.thinking_can_disable {
        return Some(levels.to_vec());
    }
    // A model that cannot stop reasoning must not offer "none" as a choice.
    Some(
        levels
            .iter()
            .copied()
            .filter(|level| *level != "none")
            .collect(),
    )
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
