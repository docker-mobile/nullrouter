//! Per-service (non-chat) provider endpoints.
//!
//! Embeddings, TTS, STT, image generation, search, and fetch each have their
//! own base URL and auth style, distinct from a provider's chat transport.
//! Dumped from the `*Config` blocks in `inspire/open-sse/providers/registry/`.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde::Deserialize;

const SERVICES_JSON: &str = include_str!("../data/services.json");

/// provider id -> service kind -> endpoint config.
static SERVICES: LazyLock<BTreeMap<String, BTreeMap<String, ServiceEndpoint>>> =
    LazyLock::new(|| serde_json::from_str(SERVICES_JSON).unwrap_or_default());

/// One non-chat service endpoint.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceEndpoint {
    #[serde(default)]
    pub base_url: Option<String>,
    /// `apikey` or `oauth`.
    #[serde(default)]
    pub auth_type: Option<String>,
    /// `bearer` or a literal header name.
    #[serde(default)]
    pub auth_header: Option<String>,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub default_model: Option<String>,
}

/// The service a request targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceKind {
    Embedding,
    TextToSpeech,
    SpeechToText,
    ImageGeneration,
    Search,
    Fetch,
}

impl ServiceKind {
    /// Registry key for this service.
    const fn config_key(self) -> &'static str {
        match self {
            Self::Embedding => "embeddingConfig",
            Self::TextToSpeech => "ttsConfig",
            Self::SpeechToText => "sttConfig",
            Self::ImageGeneration => "imageConfig",
            Self::Search => "searchConfig",
            Self::Fetch => "fetchConfig",
        }
    }

    /// Infer the service from a request path.
    pub fn from_path(path: &str) -> Option<Self> {
        if path.contains("/embeddings") {
            return Some(Self::Embedding);
        }
        if path.contains("/audio/speech") {
            return Some(Self::TextToSpeech);
        }
        if path.contains("/audio/transcriptions") {
            return Some(Self::SpeechToText);
        }
        if path.contains("/images/generations") {
            return Some(Self::ImageGeneration);
        }
        if path.contains("/search") {
            return Some(Self::Search);
        }
        if path.contains("/web/fetch") {
            return Some(Self::Fetch);
        }
        None
    }
}

/// Endpoint config for a provider's service, if it offers one.
pub fn service_endpoint(provider: &str, kind: ServiceKind) -> Option<&'static ServiceEndpoint> {
    SERVICES.get(provider)?.get(kind.config_key())
}

/// `true` when the provider exposes this service.
pub fn supports_service(provider: &str, kind: ServiceKind) -> bool {
    service_endpoint(provider, kind).is_some_and(|endpoint| endpoint.base_url.is_some())
}

/// Providers offering a given service, in registry order.
pub fn providers_for_service(kind: ServiceKind) -> Vec<&'static str> {
    SERVICES
        .iter()
        .filter(|(_, services)| services.contains_key(kind.config_key()))
        .map(|(provider, _)| provider.as_str())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{ServiceKind, providers_for_service, service_endpoint, supports_service};

    #[test]
    fn services_json_parses_and_is_populated() {
        // Guards the LazyLock's unwrap_or_default.
        assert!(
            providers_for_service(ServiceKind::Embedding).len() > 5,
            "expected embedding providers from the dump"
        );
    }

    #[test]
    fn openai_service_endpoints_match_upstream() {
        let embeddings = service_endpoint("openai", ServiceKind::Embedding).expect("embeddings");
        assert_eq!(
            embeddings.base_url.as_deref(),
            Some("https://api.openai.com/v1/embeddings")
        );
        assert_eq!(embeddings.auth_header.as_deref(), Some("bearer"));

        let tts = service_endpoint("openai", ServiceKind::TextToSpeech).expect("tts");
        assert_eq!(
            tts.base_url.as_deref(),
            Some("https://api.openai.com/v1/audio/speech")
        );
        assert_eq!(tts.default_model.as_deref(), Some("gpt-4o-mini-tts"));

        let stt = service_endpoint("openai", ServiceKind::SpeechToText).expect("stt");
        assert_eq!(
            stt.base_url.as_deref(),
            Some("https://api.openai.com/v1/audio/transcriptions")
        );

        let images = service_endpoint("openai", ServiceKind::ImageGeneration).expect("images");
        assert_eq!(
            images.base_url.as_deref(),
            Some("https://api.openai.com/v1/images/generations")
        );
    }

    #[test]
    fn service_support_is_reported_per_provider() {
        assert!(supports_service("openai", ServiceKind::Embedding));
        // anthropic offers no embedding endpoint.
        assert!(!supports_service("anthropic", ServiceKind::Embedding));
        assert!(!supports_service("not-a-provider", ServiceKind::Embedding));
    }

    #[test]
    fn services_are_inferred_from_request_paths() {
        assert_eq!(
            ServiceKind::from_path("/v1/embeddings"),
            Some(ServiceKind::Embedding)
        );
        assert_eq!(
            ServiceKind::from_path("/v1/audio/speech"),
            Some(ServiceKind::TextToSpeech)
        );
        assert_eq!(
            ServiceKind::from_path("/v1/audio/transcriptions"),
            Some(ServiceKind::SpeechToText)
        );
        assert_eq!(
            ServiceKind::from_path("/v1/images/generations"),
            Some(ServiceKind::ImageGeneration)
        );
        assert_eq!(
            ServiceKind::from_path("/v1/search"),
            Some(ServiceKind::Search)
        );
        assert_eq!(
            ServiceKind::from_path("/v1/web/fetch"),
            Some(ServiceKind::Fetch)
        );
        assert_eq!(ServiceKind::from_path("/v1/chat/completions"), None);
    }
}
