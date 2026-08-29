//! Provider credentials and auth-header construction.
//!
//! Header construction ports the auth-descriptor handling in
//! `open-sse/executors/default.js` (`applyAuth`/`setAuth`) and the
//! `BaseExecutor.buildHeaders` fallback.

use std::collections::BTreeMap;

use nullrouter_providers::{ANTHROPIC_COMPAT_BASE, OPENAI_COMPAT_BASE, registry};
use nullrouter_translate::schema::ANTHROPIC_API_VERSION;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One provider connection's secrets and per-connection settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credentials {
    /// Connection id in persistent state (`"noauth"` for public providers).
    #[serde(default)]
    pub connection_id: String,
    /// Human-readable connection label, for logs.
    #[serde(default)]
    pub connection_name: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub access_token: Option<String>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_at: Option<String>,
    /// Free-form per-connection settings (`baseUrl`, proxy config, region, ...).
    #[serde(default)]
    pub provider_specific_data: BTreeMap<String, Value>,
}

impl Credentials {
    /// A string setting from `providerSpecificData`.
    pub fn setting(&self, key: &str) -> Option<&str> {
        self.provider_specific_data
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    /// A boolean setting from `providerSpecificData`.
    pub fn flag(&self, key: &str) -> bool {
        self.provider_specific_data
            .get(key)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    /// The user-configured base URL for a compatible-family provider.
    pub fn base_url(&self) -> Option<&str> {
        self.setting("baseUrl")
    }

    /// Outbound proxy URL, when this connection routes through one.
    pub fn proxy_url(&self) -> Option<&str> {
        if self.flag("connectionProxyEnabled") {
            self.setting("connectionProxyUrl")
        } else {
            None
        }
    }

    /// Hosts that bypass the proxy.
    pub fn no_proxy(&self) -> Option<&str> {
        self.setting("connectionNoProxy")
    }
}

/// Resolve the request URL for a provider (upstream `BaseExecutor.buildUrl`).
///
/// `url_index` selects among a provider's fallback `baseUrls`.
pub fn build_url(provider: &str, credentials: &Credentials, url_index: usize) -> Option<String> {
    // Dynamic compatible families derive their URL from the connection.
    if nullrouter_providers::is_openai_compatible(provider) {
        let base = credentials.base_url().unwrap_or(OPENAI_COMPAT_BASE);
        let normalized = base.trim_end_matches('/');
        let path = if provider.contains("responses") {
            "/responses"
        } else {
            "/chat/completions"
        };
        return Some(format!("{normalized}{path}"));
    }
    if nullrouter_providers::is_anthropic_compatible(provider) {
        let base = credentials.base_url().unwrap_or(ANTHROPIC_COMPAT_BASE);
        return Some(format!("{}/messages", base.trim_end_matches('/')));
    }

    let transport = registry::transport(provider)?;
    let urls = transport.urls();
    let base = urls
        .get(url_index)
        .or_else(|| urls.first())
        .copied()?
        .to_owned();

    // Region-scoped providers override the base URL from the connection.
    if let Some(regions) = transport.regions.as_ref() {
        let region = credentials
            .setting("region")
            .or(transport.default_region.as_deref());
        if let Some(url) = region.and_then(|region| regions.get(region)) {
            // A region entry is a bare base, while the default `baseUrl` carries
            // the full endpoint. Returning the region verbatim would POST to that
            // bare base — for `xiaomi-tokenplan`, to `/v1` instead of
            // `/v1/chat/completions`. The endpoint is recovered from the default
            // pair rather than hardcoded, so it stays right if the registry moves.
            let endpoint = region_endpoint_suffix(transport, regions);
            return Some(format!("{}{endpoint}", url.trim_end_matches('/')));
        }
    }

    Some(match transport.url_suffix.as_deref() {
        Some(suffix) => format!("{base}{suffix}"),
        None => base,
    })
}

/// The endpoint path a region-scoped provider's bare region base is missing.
///
/// Derived by asking what the default `baseUrl` carries beyond its own region
/// entry: for `xiaomi-tokenplan` the default is
/// `https://token-plan-sgp.xiaomimimo.com/v1/chat/completions` and its `sgp`
/// region is `https://token-plan-sgp.xiaomimimo.com/v1`, so the endpoint is
/// `/chat/completions`. Falls back to the transport's declared `url_suffix`, then
/// to nothing, so a provider whose regions already carry a full path is unchanged.
fn region_endpoint_suffix<'a>(
    transport: &'a registry::Transport,
    regions: &std::collections::BTreeMap<String, String>,
) -> &'a str {
    let default_url = transport.base_url.as_deref().unwrap_or_default();
    let derived = transport
        .default_region
        .as_deref()
        .and_then(|name| regions.get(name))
        .and_then(|base| default_url.strip_prefix(base.trim_end_matches('/')));
    match derived {
        // Only a path remainder counts. An empty remainder means the default was
        // already bare, and anything not starting with `/` is a different host.
        Some(suffix) if suffix.starts_with('/') => suffix,
        _ => transport.url_suffix.as_deref().unwrap_or_default(),
    }
}

/// How many URLs this provider can fall back across.
pub fn fallback_count(provider: &str) -> usize {
    registry::transport(provider)
        .map(|transport| transport.urls().len())
        .filter(|count| *count > 0)
        .unwrap_or(1)
}

/// Build the outbound request headers for a provider call.
pub fn build_headers(
    provider: &str,
    credentials: &Credentials,
    stream: bool,
) -> BTreeMap<String, String> {
    let mut headers = BTreeMap::new();
    headers.insert("Content-Type".to_owned(), "application/json".to_owned());

    if let Some(transport) = registry::transport(provider) {
        for (key, value) in &transport.headers {
            headers.insert(key.clone(), value.clone());
        }
    }

    apply_auth(provider, credentials, &mut headers);

    if stream {
        headers.insert("Accept".to_owned(), "text/event-stream".to_owned());
    }
    headers
}

/// Which auth shape applies when the registry declares no explicit descriptor
/// (upstream `DefaultExecutor.resolveAuthDescriptor`).
enum FallbackAuth {
    /// `anthropic-compatible-*`: split `x-api-key` / bearer, plus version.
    AnthropicCompatibleSplit,
    /// Any `claude`-format provider: combined raw `x-api-key`, plus version.
    ClaudeApiKey,
    /// Everything else: combined bearer.
    Bearer,
}

fn fallback_auth(provider: &str) -> FallbackAuth {
    if nullrouter_providers::is_anthropic_compatible(provider) {
        return FallbackAuth::AnthropicCompatibleSplit;
    }
    let is_claude_format = registry::transport(provider)
        .is_some_and(|transport| transport.format_or_default() == "claude");
    if is_claude_format {
        return FallbackAuth::ClaudeApiKey;
    }
    FallbackAuth::Bearer
}

/// Apply credentials to headers per the registry auth descriptor, falling back
/// to `DefaultExecutor.resolveAuthDescriptor` behavior when none is declared.
fn apply_auth(provider: &str, credentials: &Credentials, headers: &mut BTreeMap<String, String>) {
    let descriptor = registry::transport(provider).and_then(|transport| transport.auth.as_ref());
    let Some(auth) = descriptor else {
        match fallback_auth(provider) {
            FallbackAuth::AnthropicCompatibleSplit => {
                // Split: the key branch wins, else the OAuth branch.
                if let Some(key) = credentials.api_key.as_deref() {
                    headers.insert("x-api-key".to_owned(), key.to_owned());
                } else if let Some(token) = credentials.access_token.as_deref() {
                    headers.insert("Authorization".to_owned(), format!("Bearer {token}"));
                }
                insert_anthropic_version(headers);
            }
            FallbackAuth::ClaudeApiKey => {
                // Combined raw x-api-key: never Bearer-prefixed. Anthropic and
                // every other claude-format provider land here.
                if let Some(secret) = credentials
                    .api_key
                    .as_deref()
                    .or(credentials.access_token.as_deref())
                {
                    headers.insert("x-api-key".to_owned(), secret.to_owned());
                }
                insert_anthropic_version(headers);
            }
            FallbackAuth::Bearer => {
                // Combined bearer, preferring the static key.
                if let Some(secret) = credentials
                    .api_key
                    .as_deref()
                    .or(credentials.access_token.as_deref())
                {
                    headers.insert("Authorization".to_owned(), format!("Bearer {secret}"));
                }
            }
        }
        return;
    };

    if auth.combined == Some(true) {
        // Combined providers use one header for either secret; the key wins.
        let secret = credentials
            .api_key
            .as_deref()
            .or(credentials.access_token.as_deref());
        if let Some(secret) = secret {
            let header = auth.header.as_deref().unwrap_or("Authorization");
            let value = if auth.scheme.as_deref() == Some("raw") {
                secret.to_owned()
            } else {
                format!("Bearer {secret}")
            };
            headers.insert(header.to_owned(), value);
        }
        if auth.anthropic_version == Some(true) {
            insert_anthropic_version(headers);
        }
        return;
    }

    // Split descriptor: only the branch matching the held secret is applied.
    if let (Some(key), Some(spec)) = (credentials.api_key.as_deref(), auth.api_key.as_ref()) {
        let value = if spec.is_bearer() {
            format!("Bearer {key}")
        } else {
            key.to_owned()
        };
        headers.insert(spec.header.clone(), value);
    } else if let (Some(token), Some(spec)) =
        (credentials.access_token.as_deref(), auth.oauth.as_ref())
    {
        let value = if spec.is_bearer() {
            format!("Bearer {token}")
        } else {
            token.to_owned()
        };
        headers.insert(spec.header.clone(), value);
    }

    if auth.anthropic_version == Some(true) {
        insert_anthropic_version(headers);
    }
}

/// Add `anthropic-version` unless a cased variant is already present.
fn insert_anthropic_version(headers: &mut BTreeMap<String, String>) {
    let present = headers
        .keys()
        .any(|key| key.eq_ignore_ascii_case("anthropic-version"));
    if !present {
        headers.insert(
            "anthropic-version".to_owned(),
            ANTHROPIC_API_VERSION.to_owned(),
        );
    }
}

/// `true` when the provider needs no credentials at all.
pub fn is_no_auth(provider: &str) -> bool {
    registry::transport(provider).and_then(|transport| transport.no_auth) == Some(true)
}

/// `true` when the provider always streams from upstream
/// (upstream `transport.forceStream`).
pub fn forces_stream(provider: &str) -> bool {
    registry::transport(provider).and_then(|transport| transport.force_stream) == Some(true)
}

#[cfg(test)]
mod tests {
    use super::{Credentials, build_headers, build_url, fallback_count, forces_stream, is_no_auth};
    use serde_json::json;

    fn with_key(key: &str) -> Credentials {
        Credentials {
            api_key: Some(key.to_owned()),
            ..Credentials::default()
        }
    }

    fn with_token(token: &str) -> Credentials {
        Credentials {
            access_token: Some(token.to_owned()),
            ..Credentials::default()
        }
    }

    #[test]
    fn openai_uses_bearer_by_default() {
        let headers = build_headers("openai", &with_key("sk-test"), false);
        assert_eq!(
            headers.get("Authorization").map(String::as_str),
            Some("Bearer sk-test")
        );
        assert_eq!(
            headers.get("Content-Type").map(String::as_str),
            Some("application/json")
        );
    }

    #[test]
    fn streaming_requests_ask_for_event_stream() {
        let headers = build_headers("openai", &with_key("sk-test"), true);
        assert_eq!(
            headers.get("Accept").map(String::as_str),
            Some("text/event-stream")
        );
        assert!(!build_headers("openai", &with_key("sk-test"), false).contains_key("Accept"));
    }

    #[test]
    fn anthropic_sends_x_api_key_not_bearer() {
        // anthropic declares no auth descriptor, so it resolves through the
        // claude-format fallback: raw x-api-key. Sending Bearer here would make
        // every Claude-format provider fail authentication.
        let headers = build_headers("anthropic", &with_key("sk-ant"), false);
        assert_eq!(headers_get(&headers, "x-api-key"), Some("sk-ant"));
        assert!(
            !headers.contains_key("Authorization"),
            "anthropic must not send Authorization: {headers:?}"
        );
        // Upstream has spelled this both ways and HTTP header names are
        // case-insensitive, so the assertion is too.
        let version = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("anthropic-version"))
            .map(|(_, value)| value.as_str());
        assert_eq!(
            version,
            Some("2023-06-01"),
            "anthropic must carry a version header: {headers:?}"
        );
    }

    #[test]
    fn claude_format_providers_all_use_raw_x_api_key() {
        // Every claude-format provider without an explicit descriptor must take
        // the x-api-key path, never bearer.
        let claude_providers: Vec<&str> = nullrouter_providers::entries()
            .iter()
            .filter(|entry| {
                entry.transport.as_ref().is_some_and(|transport| {
                    transport.format_or_default() == "claude" && transport.auth.is_none()
                })
            })
            .map(|entry| entry.id.as_str())
            .collect();

        assert!(
            !claude_providers.is_empty(),
            "expected at least one descriptor-less claude provider"
        );
        for provider in claude_providers {
            let headers = build_headers(provider, &with_key("secret"), false);
            assert_eq!(
                headers_get(&headers, "x-api-key"),
                Some("secret"),
                "{provider} must send a raw x-api-key"
            );
        }
    }

    #[test]
    fn openai_format_providers_keep_bearer_auth() {
        let headers = build_headers("groq", &with_key("gsk"), false);
        assert_eq!(headers_get(&headers, "Authorization"), Some("Bearer gsk"));
        assert!(!headers.contains_key("x-api-key"));
    }

    #[test]
    fn gemini_picks_the_branch_matching_the_held_secret() {
        let key_headers = build_headers("gemini", &with_key("AIza"), false);
        assert_eq!(headers_get(&key_headers, "x-goog-api-key"), Some("AIza"));
        assert!(!key_headers.contains_key("Authorization"));

        let token_headers = build_headers("gemini", &with_token("ya29"), false);
        assert_eq!(
            token_headers.get("Authorization").map(String::as_str),
            Some("Bearer ya29")
        );
        assert!(!token_headers.contains_key("x-goog-api-key"));
    }

    fn headers_get<'a>(
        headers: &'a std::collections::BTreeMap<String, String>,
        key: &str,
    ) -> Option<&'a str> {
        headers.get(key).map(String::as_str)
    }

    #[test]
    fn anthropic_compatible_prefers_key_over_token() {
        let both = Credentials {
            api_key: Some("sk-key".to_owned()),
            access_token: Some("tok".to_owned()),
            ..Credentials::default()
        };
        let headers = build_headers("anthropic-compatible-x", &both, false);
        assert_eq!(headers_get(&headers, "x-api-key"), Some("sk-key"));
        assert_eq!(
            headers_get(&headers, "anthropic-version"),
            Some("2023-06-01")
        );
        assert!(!headers.contains_key("Authorization"));

        // Token-only falls back to bearer.
        let headers = build_headers("anthropic-compatible-x", &with_token("tok"), false);
        assert_eq!(headers_get(&headers, "Authorization"), Some("Bearer tok"));
    }

    #[test]
    fn compatible_families_derive_urls_from_the_connection() {
        let mut credentials = Credentials::default();
        credentials
            .provider_specific_data
            .insert("baseUrl".to_owned(), json!("https://host.test/v1/"));

        assert_eq!(
            build_url("openai-compatible-abc", &credentials, 0).as_deref(),
            Some("https://host.test/v1/chat/completions")
        );
        assert_eq!(
            build_url("openai-compatible-responses-abc", &credentials, 0).as_deref(),
            Some("https://host.test/v1/responses")
        );
        assert_eq!(
            build_url("anthropic-compatible-abc", &credentials, 0).as_deref(),
            Some("https://host.test/v1/messages")
        );
    }

    #[test]
    fn compatible_families_fall_back_to_vendor_defaults() {
        let credentials = Credentials::default();
        assert_eq!(
            build_url("openai-compatible-abc", &credentials, 0).as_deref(),
            Some("https://api.openai.com/v1/chat/completions")
        );
        assert_eq!(
            build_url("anthropic-compatible-abc", &credentials, 0).as_deref(),
            Some("https://api.anthropic.com/v1/messages")
        );
    }

    #[test]
    fn registry_urls_resolve_and_index_out_of_range_clamps() {
        let credentials = Credentials::default();
        assert_eq!(
            build_url("openai", &credentials, 0).as_deref(),
            Some("https://api.openai.com/v1/chat/completions")
        );
        // Out-of-range index falls back to the first URL rather than failing.
        assert_eq!(
            build_url("openai", &credentials, 99).as_deref(),
            Some("https://api.openai.com/v1/chat/completions")
        );
        assert_eq!(build_url("not-a-provider", &credentials, 0), None);
    }

    #[test]
    fn a_region_selected_url_keeps_its_endpoint_path() {
        // The bug: a region entry is a bare base, and it was returned verbatim. So
        // a region-selected connection POSTed to `/v1` instead of
        // `/v1/chat/completions` — a 404 or a silent wrong endpoint, depending on
        // the host.
        let mut credentials = Credentials::default();
        credentials
            .provider_specific_data
            .insert("region".to_owned(), serde_json::json!("cn"));
        assert_eq!(
            build_url("xiaomi-tokenplan", &credentials, 0).as_deref(),
            Some("https://token-plan-cn.xiaomimimo.com/v1/chat/completions"),
            "the endpoint path must survive the region override"
        );

        credentials
            .provider_specific_data
            .insert("region".to_owned(), serde_json::json!("ams"));
        assert_eq!(
            build_url("xiaomi-tokenplan", &credentials, 0).as_deref(),
            Some("https://token-plan-ams.xiaomimimo.com/v1/chat/completions")
        );
    }

    #[test]
    fn the_default_region_resolves_to_the_same_url_as_no_region() {
        // With no region set, the transport's `defaultRegion` applies. It must
        // agree with the declared `baseUrl`, or the default path and the
        // explicitly-selected default path would differ.
        let credentials = Credentials::default();
        let implicit = build_url("xiaomi-tokenplan", &credentials, 0);

        let mut explicit_credentials = Credentials::default();
        explicit_credentials
            .provider_specific_data
            .insert("region".to_owned(), serde_json::json!("sgp"));
        let explicit = build_url("xiaomi-tokenplan", &explicit_credentials, 0);

        assert_eq!(implicit, explicit);
        assert_eq!(
            implicit.as_deref(),
            Some("https://token-plan-sgp.xiaomimimo.com/v1/chat/completions")
        );
    }

    #[test]
    fn an_unknown_region_falls_back_to_the_declared_base_url() {
        // A hand-edited or stale region name must not produce a bare base or an
        // empty URL; the declared `baseUrl` is the safe answer.
        let mut credentials = Credentials::default();
        credentials
            .provider_specific_data
            .insert("region".to_owned(), serde_json::json!("nowhere"));
        assert_eq!(
            build_url("xiaomi-tokenplan", &credentials, 0).as_deref(),
            Some("https://token-plan-sgp.xiaomimimo.com/v1/chat/completions")
        );
    }

    #[test]
    fn a_non_region_provider_is_unaffected_by_the_region_path() {
        // The endpoint-recovery logic must not touch providers without regions.
        let mut credentials = Credentials::default();
        credentials
            .provider_specific_data
            .insert("region".to_owned(), serde_json::json!("cn"));
        assert_eq!(
            build_url("openai", &credentials, 0).as_deref(),
            Some("https://api.openai.com/v1/chat/completions"),
            "a region setting on a provider with no regions changes nothing"
        );
    }

    #[test]
    fn transport_flags_are_read_from_the_registry() {
        assert!(forces_stream("openai"));
        assert!(!forces_stream("anthropic"));
        assert!(!is_no_auth("openai"));
        assert!(fallback_count("openai") >= 1);
        // Unknown providers still report one attempt.
        assert_eq!(fallback_count("not-a-provider"), 1);
    }

    #[test]
    fn proxy_settings_require_the_enable_flag() {
        let mut credentials = Credentials::default();
        credentials.provider_specific_data.insert(
            "connectionProxyUrl".to_owned(),
            json!("http://127.0.0.1:7897"),
        );
        // URL alone is ignored until the flag is set.
        assert_eq!(credentials.proxy_url(), None);

        credentials
            .provider_specific_data
            .insert("connectionProxyEnabled".to_owned(), json!(true));
        assert_eq!(credentials.proxy_url(), Some("http://127.0.0.1:7897"));
    }
}
