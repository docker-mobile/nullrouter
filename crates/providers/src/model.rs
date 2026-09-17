//! Model-string parsing and provider resolution.
//!
//! Ports `open-sse/services/model.js`.

use std::collections::BTreeMap;

use crate::registry;

/// Outcome of parsing a client-supplied model string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedModel {
    /// Canonical provider id, or `None` when the string was a bare alias.
    pub provider: Option<String>,
    /// Model id (without the provider prefix).
    pub model: String,
    /// `true` when no `provider/` prefix was present.
    pub is_alias: bool,
    /// The provider token exactly as the client wrote it.
    pub provider_alias: Option<String>,
}

/// A fully resolved provider/model routing target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelTarget {
    pub provider: String,
    pub model: String,
}

/// Parse `provider/model`, `alias/model`, or a bare alias
/// (upstream `parseModel`).
pub fn parse_model(model_str: &str) -> ParsedModel {
    if model_str.is_empty() {
        return ParsedModel {
            provider: None,
            model: String::new(),
            is_alias: false,
            provider_alias: None,
        };
    }

    if let Some(slash) = model_str.find('/') {
        let (provider_or_alias, rest) = model_str.split_at(slash);
        let model = rest.get(1..).unwrap_or_default();
        return ParsedModel {
            provider: Some(registry::resolve_provider_id(provider_or_alias).to_owned()),
            model: model.to_owned(),
            is_alias: false,
            provider_alias: Some(provider_or_alias.to_owned()),
        };
    }

    ParsedModel {
        provider: None,
        model: model_str.to_owned(),
        is_alias: true,
        provider_alias: None,
    }
}

/// Resolve a model alias from a user-configured alias map
/// (upstream `resolveModelAliasFromMap`). Values may be `"provider/model"` or
/// `{ provider, model }`.
pub fn resolve_model_alias(
    alias: &str,
    aliases: &BTreeMap<String, serde_json::Value>,
) -> Option<ModelTarget> {
    let resolved = aliases.get(alias)?;

    if let Some(text) = resolved.as_str() {
        let slash = text.find('/')?;
        let (provider, rest) = text.split_at(slash);
        return Some(ModelTarget {
            provider: registry::resolve_provider_id(provider).to_owned(),
            model: rest.get(1..).unwrap_or_default().to_owned(),
        });
    }

    let provider = resolved.get("provider")?.as_str()?;
    let model = resolved.get("model")?.as_str()?;
    Some(ModelTarget {
        provider: registry::resolve_provider_id(provider).to_owned(),
        model: model.to_owned(),
    })
}

/// Prefix-based provider inference, used when a bare alias resolves to nothing
/// (upstream `inferProviderFromModelName`, first match wins).
pub fn infer_provider_from_model_name(model_name: &str) -> &'static str {
    if model_name.is_empty() {
        return "openai";
    }
    let lower = model_name.to_lowercase();
    if lower.starts_with("claude-") {
        return "anthropic";
    }
    if lower.starts_with("gemini-") {
        return "gemini";
    }
    if lower.starts_with("gpt-") {
        return "openai";
    }
    // Upstream regex /^o[134]/ — `o1`, `o3`, `o4` families.
    if let Some(rest) = lower.strip_prefix('o')
        && rest.starts_with(['1', '3', '4'])
    {
        return "openai";
    }
    if lower.starts_with("deepseek-") {
        return "openrouter";
    }
    "openai"
}

/// Resolve a client model string to a routing target
/// (upstream `getModelInfoCore`).
///
/// Returns `None` when the string is a bare alias that matches no configured
/// alias — the caller then checks whether it names a combo.
pub fn resolve_target(
    model_str: &str,
    aliases: &BTreeMap<String, serde_json::Value>,
) -> Option<ModelTarget> {
    let parsed = parse_model(model_str);

    if !parsed.is_alias {
        return parsed.provider.map(|provider| ModelTarget {
            provider,
            model: parsed.model,
        });
    }

    if let Some(resolved) = resolve_model_alias(&parsed.model, aliases) {
        return Some(resolved);
    }

    None
}

/// Fall back to prefix inference for an unresolved bare alias.
pub fn infer_target(model_str: &str) -> ModelTarget {
    ModelTarget {
        provider: infer_provider_from_model_name(model_str).to_owned(),
        model: model_str.to_owned(),
    }
}

/// Strip a trailing thinking suffix such as `"(high)"`, returning
/// `(base_id, suffix)` (upstream `getModelUpstreamId`).
pub fn split_thinking_suffix(model_id: &str) -> (&str, &str) {
    let trimmed = model_id.trim_end();
    if !trimmed.ends_with(')') {
        return (model_id, "");
    }
    let Some(open) = trimmed.rfind('(') else {
        return (model_id, "");
    };
    let inner = trimmed.get(open + 1..trimmed.len() - 1).unwrap_or_default();
    // Upstream pattern is /\([^()]+\)\s*$/ — non-empty, no nested parens.
    if inner.is_empty() || inner.contains(['(', ')']) {
        return (model_id, "");
    }
    let base = trimmed.get(..open).unwrap_or_default().trim_end();
    let suffix = trimmed.get(open..).unwrap_or_default();
    (base, suffix)
}

/// Id to send upstream for a provider/model pair, preserving any thinking
/// suffix (upstream `getModelUpstreamId`).
pub fn upstream_model_id(provider_id: &str, model_id: &str) -> String {
    let key = registry::entry(provider_id).map_or(provider_id, registry::RegistryEntry::models_key);
    let (base, suffix) = split_thinking_suffix(model_id);
    let resolved = registry::find_model(key, base).map_or(base, registry::Model::upstream_id);
    format!("{resolved}{suffix}")
}

/// Per-model wire-format override, if the registry declares one.
pub fn model_target_format(provider_id: &str, model_id: &str) -> Option<&'static str> {
    let key = registry::entry(provider_id).map_or(provider_id, registry::RegistryEntry::models_key);
    let (base, _) = split_thinking_suffix(model_id);
    registry::find_model(key, base)?.target_format.as_deref()
}

/// Content types to strip before dispatch for this model.
pub fn model_strip_list(provider_id: &str, model_id: &str) -> &'static [String] {
    let key = registry::entry(provider_id).map_or(provider_id, registry::RegistryEntry::models_key);
    let (base, _) = split_thinking_suffix(model_id);
    registry::find_model(key, base).map_or(&[], |model| model.strip.as_slice())
}

/// Derive a human-readable display name from a model id when the registry
/// entry omits `name`. Ports `open-sse/providers/models/namePatterns.js`.
///
/// Example: `"gpt-4o-mini"` → `"GPT 4o Mini"`,
/// `"claude-sonnet-4-6"` → `"Claude Sonnet 4 6"`.
#[must_use]
pub fn derive_model_name(id: &str) -> String {
    if id.is_empty() {
        return String::new();
    }

    // Kimi: kimi-k2.5 → "Kimi K2.5"
    if let Some(rest) = id.strip_prefix("kimi-k") {
        return format!("Kimi K{}", title_case(rest));
    }
    // GLM: glm-4.6v → "GLM 4.6 V (Vision)"
    if let Some(rest) = id.strip_prefix("glm-") {
        if let Some(base) = rest.strip_suffix('v') {
            return format!("GLM {} V (Vision)", title_case(base));
        }
        return format!("GLM {}", title_case(rest));
    }
    // MiniMax: minimax-m3 → "MiniMax M3"
    if let Some(rest) = id.strip_prefix("minimax-m") {
        return format!("MiniMax M{}", title_case(rest));
    }
    // Generic prefixes with title case
    let upper_prefixes = [
        ("gpt-", "GPT"),
        ("glm-", "GLM"),
        ("grok-", "Grok"),
        ("gemini-", "Gemini"),
        ("deepseek-", "DeepSeek"),
        ("claude-", "Claude"),
        ("qwen", "Qwen"),
    ];
    for (prefix, display) in &upper_prefixes {
        if let Some(rest) = id.strip_prefix(prefix) {
            return format!("{} {}", display, title_case(rest));
        }
    }

    // Fallback: just title-case the whole id.
    title_case(id)
}

/// Capitalize each hyphen/underscore separated token.
fn title_case(s: &str) -> String {
    s.split(['-', '_', ' '])
        .filter(|w| !w.is_empty())
        .map(|w| {
            if w.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                w.to_owned()
            } else {
                let mut chars = w.chars();
                chars.next().map_or_else(String::new, |first| {
                    first.to_uppercase().collect::<String>() + chars.as_str()
                })
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod name_tests {
    use super::derive_model_name;

    #[test]
    fn derives_gpt_name() {
        assert_eq!(derive_model_name("gpt-4o-mini"), "GPT 4o Mini");
    }
    #[test]
    fn derives_claude_name() {
        assert_eq!(derive_model_name("claude-sonnet-4-6"), "Claude Sonnet 4 6");
    }
    #[test]
    fn derives_kimi_name() {
        assert_eq!(derive_model_name("kimi-k2.5"), "Kimi K2.5");
    }
    #[test]
    fn derives_glm_vision_name() {
        assert_eq!(derive_model_name("glm-4.6v"), "GLM 4.6 V (Vision)");
    }
    #[test]
    fn derives_unknown_fallback() {
        assert_eq!(derive_model_name("some-model"), "Some Model");
    }
}
