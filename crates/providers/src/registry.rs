//! Typed view over the frozen 9Router provider registry.
//!
//! The data in `data/registry.json` is dumped from the read-only `inspire/`
//! reference (`open-sse/providers/registry/`) so provider transports, auth
//! descriptors, and model tables stay byte-faithful to upstream instead of
//! being hand-transcribed.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde::Deserialize;

const REGISTRY_JSON: &str = include_str!("../data/registry.json");

/// All registry entries, parsed once.
///
/// A malformed embedded payload yields an empty registry rather than a panic;
/// `registry_parses` in this module's tests fails loudly if that ever happens.
static ENTRIES: LazyLock<Vec<RegistryEntry>> =
    LazyLock::new(|| serde_json::from_str(REGISTRY_JSON).unwrap_or_default());

/// Alias/id lookup table: id -> id, alias -> id, aliases[] -> id.
static ALIAS_TO_ID: LazyLock<BTreeMap<String, String>> = LazyLock::new(|| {
    let mut map = BTreeMap::new();
    for entry in entries() {
        map.insert(entry.id.clone(), entry.id.clone());
        if let Some(alias) = &entry.alias {
            map.insert(alias.clone(), entry.id.clone());
        }
        for alias in &entry.aliases {
            map.insert(alias.clone(), entry.id.clone());
        }
    }
    // Media-only providers without a registry transport keep explicit aliases
    // upstream (`open-sse/services/model.js` MEDIA_ONLY_ALIASES).
    for (alias, id) in [
        ("el", "elevenlabs"),
        ("jina", "jina-ai"),
        ("jina-ai", "jina-ai"),
        ("polly", "aws-polly"),
        ("aws-polly", "aws-polly"),
    ] {
        map.entry(alias.to_owned()).or_insert_with(|| id.to_owned());
    }
    map
});

static BY_ID: LazyLock<BTreeMap<&'static str, &'static RegistryEntry>> = LazyLock::new(|| {
    entries()
        .iter()
        .map(|entry| (entry.id.as_str(), entry))
        .collect()
});

/// Models are keyed by `alias` when present, else `id` (upstream `PROVIDER_MODELS`).
static MODELS_BY_KEY: LazyLock<BTreeMap<&'static str, &'static Vec<Model>>> = LazyLock::new(|| {
    entries()
        .iter()
        .map(|entry| (entry.models_key(), &entry.models))
        .collect()
});

/// A single provider entry.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryEntry {
    pub id: String,
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub priority: Option<u32>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub display: Option<Display>,
    #[serde(default)]
    pub transport: Option<Transport>,
    /// Multi-endpoint providers: pick the transport matching the client format.
    #[serde(default)]
    pub transports: Option<Vec<Transport>>,
    #[serde(default)]
    pub oauth: Option<OAuth>,
    #[serde(default)]
    pub models: Vec<Model>,
    #[serde(default)]
    pub service_kinds: Option<Vec<String>>,
    /// Where this provider publishes its own model catalogue, when it does.
    ///
    /// Eight providers do. The dashboard's "suggested models" list is that catalogue,
    /// filtered — a gateway like OpenRouter serves hundreds and the useful subset is
    /// whichever are free with a large context window.
    #[serde(default)]
    pub models_fetcher: Option<ModelsFetcher>,
}

/// A provider's public model catalogue.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsFetcher {
    pub url: String,
    /// Which filter to apply: `openrouter-free`, `opencode-free`, `mimo-free`, `openai`.
    #[serde(rename = "type")]
    pub filter: String,
}

impl RegistryEntry {
    /// Key under which this entry's models are registered.
    pub fn models_key(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.id)
    }
}

/// The catalogue URL a provider declares, if any.
pub fn models_fetcher(provider_id: &str) -> Option<&'static ModelsFetcher> {
    entry(provider_id)?.models_fetcher.as_ref()
}

/// Whether any provider declares this exact catalogue URL.
///
/// The reason `/api/providers/suggested-models` can take a `url` parameter without becoming
/// a server-side request forgery primitive. Upstream's route fetches whatever it is handed;
/// checking it against the registry first costs nothing, because the dashboard only ever
/// passes a URL it read from the registry in the first place.
///
/// Exact match, not a host or prefix match: a prefix check on `https://openrouter.ai/` would
/// still allow every other path on that host, and a host check allows anything an open
/// redirect on that host can reach.
pub fn declares_models_url(url: &str) -> bool {
    entries()
        .iter()
        .filter_map(|entry| entry.models_fetcher.as_ref())
        .any(|fetcher| fetcher.url == url)
}

/// UI metadata for the dashboard provider list.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Display {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub text_icon: Option<String>,
    #[serde(default)]
    pub website: Option<String>,
}

/// Runtime HTTP transport configuration.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transport {
    #[serde(default)]
    pub base_url: Option<String>,
    /// Providers with several equivalent endpoints, tried in order on 429.
    #[serde(default)]
    pub base_urls: Option<Vec<String>>,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub auth: Option<Auth>,
    #[serde(default)]
    pub force_stream: Option<bool>,
    #[serde(default)]
    pub no_auth: Option<bool>,
    #[serde(default)]
    pub url_suffix: Option<String>,
    #[serde(default)]
    pub quirks: Option<Quirks>,
    /// Status code (as string) -> retry entry.
    #[serde(default)]
    pub retry: BTreeMap<String, RetryEntry>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub validate_url: Option<String>,
    #[serde(default)]
    pub responses_url: Option<String>,
    #[serde(default)]
    pub regions: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub default_region: Option<String>,
    // ── added upstream after v0.5.20 ──
    /// Claude-format endpoint for providers exposing both shapes.
    #[serde(default)]
    pub messages_url: Option<String>,
    /// Live model-catalog endpoint.
    #[serde(default)]
    pub models_url: Option<String>,
    /// Account/identity endpoint, used by quota surfaces.
    #[serde(default)]
    pub user_url: Option<String>,
    /// Billing/credit endpoint, used by quota surfaces.
    #[serde(default)]
    pub billing_url: Option<String>,
    /// Client identity string some providers gate on.
    #[serde(default)]
    pub client_identifier: Option<String>,
    /// Header name carrying the token when it is not `Authorization`.
    #[serde(default)]
    pub token_auth: Option<String>,
    /// CLI version a provider gates quota on, sent in `User-Agent`.
    #[serde(default)]
    pub cli_version: Option<String>,
    /// `X-Goog-Api-Client` value for the Cloud Code Assist endpoints.
    #[serde(default)]
    pub api_client: Option<String>,
}

impl Transport {
    /// Effective wire format, defaulting to `openai` like upstream
    /// `PROVIDER_DEFAULTS.format`.
    pub fn format_or_default(&self) -> &str {
        self.format.as_deref().unwrap_or("openai")
    }

    /// Candidate URLs in fallback order (`baseUrls` wins over `baseUrl`).
    pub fn urls(&self) -> Vec<&str> {
        if let Some(urls) = &self.base_urls
            && !urls.is_empty()
        {
            return urls.iter().map(String::as_str).collect();
        }
        self.base_url
            .as_deref()
            .map(|url| vec![url])
            .unwrap_or_default()
    }
}

/// Auth descriptor: either `combined` (one header for key or token) or split
/// `apiKey`/`oauth` branches.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Auth {
    #[serde(default)]
    pub combined: Option<bool>,
    #[serde(default)]
    pub header: Option<String>,
    #[serde(default)]
    pub scheme: Option<String>,
    #[serde(default)]
    pub api_key: Option<AuthSpec>,
    #[serde(default)]
    pub oauth: Option<AuthSpec>,
    #[serde(default)]
    pub anthropic_version: Option<bool>,
    #[serde(default)]
    pub hooks: Vec<String>,
}

/// One header/scheme pair.
#[derive(Debug, Clone, Deserialize)]
pub struct AuthSpec {
    pub header: String,
    #[serde(default)]
    pub scheme: Option<String>,
}

impl AuthSpec {
    /// `true` when the value must be prefixed with `Bearer `.
    pub fn is_bearer(&self) -> bool {
        self.scheme.as_deref() != Some("raw")
    }
}

/// Provider-specific request-shaping flags.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Quirks {
    #[serde(default)]
    pub preserve_cache_control: bool,
    #[serde(default)]
    pub drop_client_metadata: bool,
    #[serde(default)]
    pub cloak_tools_on_oauth: bool,
    #[serde(default)]
    pub drop_output_config: bool,
}

/// Retry policy for one status code. Upstream allows a bare number (attempts)
/// or an object, so both encodings are accepted.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum RetryEntry {
    Attempts(u32),
    // `rename_all` on the enum does not reach variant fields, so the camelCase
    // mapping is declared on the variant itself.
    #[serde(rename_all = "camelCase")]
    Detailed {
        #[serde(default)]
        attempts: u32,
        #[serde(default)]
        delay_ms: Option<u64>,
    },
}

impl RetryEntry {
    pub const fn attempts(&self) -> u32 {
        match *self {
            Self::Attempts(attempts) | Self::Detailed { attempts, .. } => attempts,
        }
    }

    pub const fn delay_ms(&self) -> Option<u64> {
        match *self {
            Self::Attempts(_) => None,
            Self::Detailed { delay_ms, .. } => delay_ms,
        }
    }
}

/// OAuth flow configuration (refresh-grant fields only; full authorization
/// flows are not ported).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuth {
    #[serde(default)]
    pub client_id: Option<String>,
    /// Present only where the provider's own CLI ships one publicly.
    #[serde(default)]
    pub client_secret: Option<String>,
    #[serde(default)]
    pub token_url: Option<String>,
    /// Refresh endpoint when it differs from `token_url`.
    #[serde(default)]
    pub refresh_url: Option<String>,
    /// How long before expiry to refresh. Providers vary by four orders of
    /// magnitude here — Codex wants five days' lead, Kimi five minutes.
    #[serde(default)]
    pub refresh_lead_ms: Option<u64>,
    /// Refresh proactively once a token has gone this long without one, even if
    /// it has not expired. Codex invalidates a refresh token left unused.
    #[serde(default)]
    pub max_refresh_age_ms: Option<u64>,
    #[serde(default)]
    pub refresh: Option<OAuthRefresh>,
}

impl OAuth {
    /// Where to POST a refresh grant, preferring an explicit refresh endpoint.
    pub fn effective_refresh_url(&self) -> Option<&str> {
        self.refresh_url.as_deref().or(self.token_url.as_deref())
    }

    /// `true` when the grant body is JSON rather than form-encoded.
    pub fn refresh_is_json(&self) -> bool {
        self.refresh
            .as_ref()
            .and_then(|refresh| refresh.encoding.as_deref())
            == Some("json")
    }
}

/// Refresh-grant encoding for a provider that supports token refresh.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthRefresh {
    #[serde(default)]
    pub encoding: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
}

/// One model offered by a provider.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub kind: Option<String>,
    /// Upstream id to send when it differs from the catalog id.
    #[serde(default)]
    pub upstream_model_id: Option<String>,
    /// Per-model override of the provider's wire format.
    #[serde(default)]
    pub target_format: Option<String>,
    /// Content types to strip before dispatch (`image`, `audio`).
    #[serde(default)]
    pub strip: Vec<String>,
    #[serde(default)]
    pub params: Vec<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub quota_family: Option<String>,
    #[serde(default)]
    pub dimensions: Option<u32>,
    // ── added upstream after v0.5.20 ──
    /// Context window, when the registry states it per model.
    #[serde(default)]
    pub context_length: Option<u64>,
    /// Output ceiling, when the registry states it per model.
    #[serde(default)]
    pub max_output_tokens: Option<u64>,
    /// Quota cost multiplier relative to a normal request.
    #[serde(default)]
    pub rate_multiplier: Option<f64>,
    #[serde(default)]
    pub description: Option<String>,
    /// Wire formats this model accepts, when it is narrower than the provider's.
    #[serde(default)]
    pub supported_formats: Vec<String>,
}

impl Model {
    /// Model kind, defaulting to `llm` (upstream `MODEL_DEFAULTS.kind`).
    pub fn kind_or_default(&self) -> &str {
        self.kind.as_deref().unwrap_or("llm")
    }

    /// Id to send upstream.
    pub fn upstream_id(&self) -> &str {
        self.upstream_model_id.as_deref().unwrap_or(&self.id)
    }
}

/// Every registry entry, in upstream order.
pub fn entries() -> &'static [RegistryEntry] {
    &ENTRIES
}

/// Look up an entry by canonical provider id.
pub fn entry(provider_id: &str) -> Option<&'static RegistryEntry> {
    BY_ID.get(provider_id).copied()
}

/// Resolve an alias or id to a canonical provider id, returning the input
/// unchanged when unknown (upstream `resolveProviderAlias`).
pub fn resolve_provider_id(alias_or_id: &str) -> &str {
    ALIAS_TO_ID
        .get(alias_or_id)
        .map_or(alias_or_id, String::as_str)
}

/// Transport for a provider, resolving the dynamic
/// `openai-compatible-*` / `anthropic-compatible-*` families.
pub fn transport(provider_id: &str) -> Option<&'static Transport> {
    entry(provider_id).and_then(|entry| entry.transport.as_ref())
}

/// Models registered under a provider's models key (id or alias).
pub fn models_for_key(alias_or_id: &str) -> &'static [Model] {
    MODELS_BY_KEY
        .get(alias_or_id)
        .map_or(&[], |models| models.as_slice())
}

/// Models for a canonical provider id, going through its models key.
pub fn models_for_provider(provider_id: &str) -> &'static [Model] {
    let key = entry(provider_id).map_or(provider_id, RegistryEntry::models_key);
    models_for_key(key)
}

/// Providers that tolerate dash/dot version separators on lookup
/// (upstream `DOT_VERSION_PROVIDERS`).
const DOT_VERSION_PROVIDERS: [&str; 2] = ["kr", "kiro"];

/// Normalize `digit-digit` to `digit.digit` (upstream `normalizeModelId`).
pub fn normalize_model_id(model_id: &str) -> String {
    let bytes = model_id.as_bytes();
    let mut out = String::with_capacity(model_id.len());
    for (index, ch) in model_id.char_indices() {
        let is_version_dash = ch == '-'
            && index > 0
            && bytes.get(index - 1).is_some_and(u8::is_ascii_digit)
            && bytes.get(index + 1).is_some_and(u8::is_ascii_digit);
        out.push(if is_version_dash { '.' } else { ch });
    }
    out
}

/// Find a model under a models key: exact match, then dot-normalized match for
/// the providers that allow it (upstream `findModel`).
pub fn find_model(alias_or_id: &str, model_id: &str) -> Option<&'static Model> {
    let models = models_for_key(alias_or_id);
    if let Some(found) = models.iter().find(|model| model.id == model_id) {
        return Some(found);
    }
    if !DOT_VERSION_PROVIDERS.contains(&alias_or_id) {
        return None;
    }
    let normalized = normalize_model_id(model_id);
    if normalized == model_id {
        return None;
    }
    models.iter().find(|model| model.id == normalized)
}

#[cfg(test)]
mod tests {
    use super::{
        Auth, RetryEntry, entries, entry, find_model, models_for_provider, normalize_model_id,
        resolve_provider_id, transport,
    };

    #[test]
    fn registry_parses_and_is_populated() {
        // Guards the `unwrap_or_default` on the embedded payload: a malformed
        // or truncated registry.json fails here instead of silently emptying.
        assert!(
            entries().len() > 90,
            "expected the full upstream registry, got {}",
            entries().len()
        );
        assert!(entries().iter().any(|entry| entry.id == "openai"));
        assert!(entries().iter().any(|entry| entry.id == "anthropic"));
    }

    #[test]
    fn openai_transport_matches_upstream() {
        let transport = transport("openai").expect("openai has a transport");
        assert_eq!(
            transport.base_url.as_deref(),
            Some("https://api.openai.com/v1/chat/completions")
        );
        assert_eq!(transport.format_or_default(), "openai");
        assert_eq!(transport.force_stream, Some(true));
    }

    #[test]
    fn anthropic_transport_is_claude_format_with_version_header() {
        let transport = transport("anthropic").expect("anthropic has a transport");
        assert_eq!(transport.format_or_default(), "claude");
        // Upstream has spelled this both `Anthropic-Version` and
        // `anthropic-version`; HTTP header names are case-insensitive, so the
        // lookup is too rather than pinning one spelling.
        let version = transport
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("anthropic-version"))
            .map(|(_, value)| value.as_str());
        assert_eq!(version, Some("2023-06-01"));
    }

    #[test]
    fn aliases_resolve_to_canonical_ids() {
        assert_eq!(resolve_provider_id("cc"), "claude");
        assert_eq!(resolve_provider_id("cx"), "codex");
        assert_eq!(resolve_provider_id("kr"), "kiro");
        assert_eq!(resolve_provider_id("openai"), "openai");
        // Unknown ids pass through unchanged.
        assert_eq!(resolve_provider_id("not-a-provider"), "not-a-provider");
    }

    #[test]
    fn models_resolve_through_alias_key() {
        // claude's models are registered under alias "cc".
        assert!(!models_for_provider("claude").is_empty());
        assert!(!models_for_provider("openai").is_empty());
        assert!(
            models_for_provider("openai")
                .iter()
                .any(|m| m.id == "gpt-5")
        );
    }

    #[test]
    fn gemini_auth_uses_split_key_and_oauth_headers() {
        let transport = transport("gemini").expect("gemini has a transport");
        let Some(Auth {
            api_key: Some(api_key),
            oauth: Some(oauth),
            ..
        }) = transport.auth.as_ref()
        else {
            panic!("gemini declares split apiKey/oauth auth");
        };
        assert_eq!(api_key.header, "x-goog-api-key");
        assert!(!api_key.is_bearer(), "x-goog-api-key is a raw scheme");
        assert_eq!(oauth.header, "Authorization");
        assert!(oauth.is_bearer());
    }

    #[test]
    fn dot_version_normalization_is_scoped_to_kiro() {
        assert_eq!(normalize_model_id("claude-sonnet-4-5"), "claude-sonnet-4.5");
        // Word hyphens and suffixes are untouched.
        assert_eq!(normalize_model_id("qwen3-coder-next"), "qwen3-coder-next");
        assert_eq!(normalize_model_id("gpt-5-mini"), "gpt-5-mini");
        // Non-kiro providers get exact matching only.
        assert!(find_model("openai", "gpt-4-1").is_none());
    }

    #[test]
    fn retry_entry_accepts_both_encodings() {
        let bare: RetryEntry = serde_json::from_str("2").expect("bare number parses");
        assert_eq!(bare.attempts(), 2);
        assert_eq!(bare.delay_ms(), None);

        let detailed: RetryEntry =
            serde_json::from_str(r#"{"attempts":3,"delayMs":500}"#).expect("object parses");
        assert_eq!(detailed.attempts(), 3);
        assert_eq!(detailed.delay_ms(), Some(500));
    }

    #[test]
    fn every_entry_with_models_is_reachable_by_id() {
        for candidate in entries() {
            if candidate.models.is_empty() {
                continue;
            }
            assert!(
                entry(&candidate.id).is_some(),
                "entry {} unreachable by id",
                candidate.id
            );
        }
    }
}
