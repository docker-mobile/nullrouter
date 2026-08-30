use std::collections::HashMap;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use reqwest::{Client, StatusCode, Url};
use thiserror::Error;

use crate::{
    AuthConfigError,
    contracts::{ValidateApiKeyRequest, ValidateApiKeyResponse},
};

const MAX_API_KEY_BYTES: usize = 4_096;
const MAX_STATE_RESPONSE_BYTES: usize = 8_192;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKeyValidation {
    pub valid: bool,
    pub active: bool,
    pub key_id: Option<String>,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum StateValidationError {
    #[error("state validation service is unavailable")]
    Unavailable,
    #[error("state validation response is invalid")]
    InvalidResponse,
}

#[async_trait]
pub trait ApiKeyValidator: Send + Sync {
    async fn validate(&self, api_key: &str) -> Result<ApiKeyValidation, StateValidationError>;
}

pub(crate) struct HttpApiKeyValidator {
    client: Client,
    endpoint: Url,
    /// Recent validation results, keyed by a digest of the key.
    ///
    /// The gateway calls `/internal/v1/authorize` for every `/v1` request, and this validator then
    /// asks the state service — so one client request costs two hops before any provider work
    /// begins. The same key repeats on essentially every request, and the answer changes only when
    /// a key is created, revoked or deactivated.
    ///
    /// **This cache does not weaken enforcement.** The runtime validates the key against state
    /// independently, on every request, uncached. So a key revoked within the TTL still fails: the
    /// gateway forwards the request on a stale `authorized: true`, and the runtime rejects it. The
    /// cost of a stale hit is one wasted hop, not an accepted request. (And when
    /// `requireApiKey` is off the gateway treats `/v1` as public and never calls here at all, so
    /// there is no configuration in which this cache is the only check.)
    ///
    /// Keyed by SHA-256 rather than by the key itself so a memory dump or an accidental `Debug`
    /// does not spill live credentials out of a map.
    cache: ValidationCache,
}

/// A key digest to the verdict for it and when that verdict was read.
type ValidationCache =
    std::sync::Arc<std::sync::RwLock<HashMap<[u8; 32], (Instant, ApiKeyValidation)>>>;

/// How long a validation result stays usable.
///
/// Short enough that a revoked key stops costing a wasted hop almost immediately, long enough that
/// a burst of requests from one client collapses to a single state call.
const VALIDATION_TTL: Duration = Duration::from_millis(250);

/// Entries kept before the cache is cleared.
///
/// A bound rather than an eviction policy: the map is keyed by digest, so an attacker sending
/// distinct invalid keys would otherwise grow it without limit. Clearing wholesale is crude but
/// costs one repopulated round trip per live key, and keeps this to a few lines with no ordering
/// structure to get wrong.
const MAX_CACHE_ENTRIES: usize = 4096;

impl HttpApiKeyValidator {
    pub(crate) fn new(endpoint: Url, timeout: Duration) -> Result<Self, AuthConfigError> {
        let client = Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|_| AuthConfigError::StateClient)?;
        Ok(Self {
            client,
            endpoint,
            cache: std::sync::Arc::default(),
        })
    }

    /// The digest a key is cached under.
    fn digest(api_key: &str) -> [u8; 32] {
        use sha2::Digest as _;
        sha2::Sha256::digest(api_key.as_bytes()).into()
    }

    /// A cached result, if it is still within the TTL.
    fn cached(&self, digest: &[u8; 32]) -> Option<ApiKeyValidation> {
        // The clone happens under the read lock and the guard is dropped at the end of the
        // statement, so nothing else is done while holding it.
        self.cache
            .read()
            .ok()?
            .get(digest)
            .filter(|(checked_at, _)| checked_at.elapsed() < VALIDATION_TTL)
            .map(|(_, validation)| validation.clone())
    }

    fn remember(&self, digest: [u8; 32], validation: &ApiKeyValidation) {
        if let Ok(mut cache) = self.cache.write() {
            if cache.len() >= MAX_CACHE_ENTRIES {
                cache.clear();
            }
            cache.insert(digest, (Instant::now(), validation.clone()));
        }
    }
}

#[async_trait]
impl ApiKeyValidator for HttpApiKeyValidator {
    async fn validate(&self, api_key: &str) -> Result<ApiKeyValidation, StateValidationError> {
        if api_key.is_empty() || api_key.len() > MAX_API_KEY_BYTES {
            return Ok(ApiKeyValidation {
                valid: false,
                active: false,
                key_id: None,
            });
        }
        // See the `cache` field: this saves a hop, not a check. The runtime validates the same key
        // against state on every request regardless.
        let digest = Self::digest(api_key);
        if let Some(cached) = self.cached(&digest) {
            return Ok(cached);
        }

        let response = self
            .client
            .post(self.endpoint.clone())
            .json(&ValidateApiKeyRequest { api_key })
            .send()
            .await
            .map_err(|_| StateValidationError::Unavailable)?;
        if response.status() != StatusCode::OK {
            return Err(StateValidationError::Unavailable);
        }
        if response.content_length().is_some_and(|length| {
            length > u64::try_from(MAX_STATE_RESPONSE_BYTES).unwrap_or(u64::MAX)
        }) {
            return Err(StateValidationError::InvalidResponse);
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|_| StateValidationError::Unavailable)?;
        if bytes.len() > MAX_STATE_RESPONSE_BYTES {
            return Err(StateValidationError::InvalidResponse);
        }
        let response: ValidateApiKeyResponse =
            serde_json::from_slice(&bytes).map_err(|_| StateValidationError::InvalidResponse)?;
        let validation = ApiKeyValidation {
            valid: response.valid,
            active: response.active,
            key_id: response.key_id,
        };
        // Cached whatever the verdict: an invalid key repeated in a loop should not cost a state
        // round trip each time either. A transport failure is *not* cached — it returns early
        // above — so an unreachable state service does not pin a wrong answer for the TTL.
        self.remember(digest, &validation);
        Ok(validation)
    }
}

// `expect` needs no allow here: `clippy.toml` sets `allow-expect-in-tests`.
#[cfg(test)]
mod cache_tests {
    use super::{ApiKeyValidation, HttpApiKeyValidator, MAX_CACHE_ENTRIES, VALIDATION_TTL};
    use std::time::{Duration, Instant};

    fn validator() -> HttpApiKeyValidator {
        HttpApiKeyValidator::new(
            "http://127.0.0.1:1/internal/v1/keys/validate"
                .parse()
                .expect("a valid URL"),
            Duration::from_secs(1),
        )
        .expect("the validator should build")
    }

    fn valid() -> ApiKeyValidation {
        ApiKeyValidation {
            valid: true,
            active: true,
            key_id: Some("key_1".to_owned()),
        }
    }

    #[test]
    fn a_remembered_result_is_returned_without_a_round_trip() {
        // The endpoint above is 127.0.0.1:1, which refuses connections — so if this reached the
        // network the test would fail rather than quietly succeed.
        let validator = validator();
        let digest = HttpApiKeyValidator::digest("sk-test");
        validator.remember(digest, &valid());
        assert_eq!(validator.cached(&digest), Some(valid()));
    }

    #[test]
    fn an_expired_entry_is_not_returned() {
        let validator = validator();
        let digest = HttpApiKeyValidator::digest("sk-test");
        // Inserted with a timestamp already past the TTL.
        if let Ok(mut cache) = validator.cache.write() {
            let stale = Instant::now()
                .checked_sub(VALIDATION_TTL + Duration::from_millis(10))
                .expect("the process has been running longer than the TTL");
            cache.insert(digest, (stale, valid()));
        }
        assert_eq!(
            validator.cached(&digest),
            None,
            "an entry past the TTL must not be served"
        );
    }

    #[test]
    fn different_keys_do_not_share_an_entry() {
        // The failure this rules out is the worst one available here: one key's verdict answering
        // for another.
        let validator = validator();
        validator.remember(HttpApiKeyValidator::digest("sk-one"), &valid());
        assert_eq!(
            validator.cached(&HttpApiKeyValidator::digest("sk-two")),
            None
        );
    }

    #[test]
    fn the_cache_is_keyed_by_digest_not_by_the_key() {
        // A live credential must not sit in a map that a memory dump or a stray `Debug` would spill.
        let validator = validator();
        let secret = "sk-super-secret-value";
        validator.remember(HttpApiKeyValidator::digest(secret), &valid());
        let keys: Vec<[u8; 32]> = validator
            .cache
            .read()
            .map_or_else(|_| Vec::new(), |cache| cache.keys().copied().collect());
        assert_eq!(keys.len(), 1);
        for key in keys {
            assert_ne!(
                key.as_slice(),
                secret.as_bytes(),
                "the key itself appears in the cache key"
            );
            // And the digest does not start with the secret either.
            assert!(
                !secret
                    .as_bytes()
                    .starts_with(key.get(..8).unwrap_or_default()),
                "the cache key looks derived from the plaintext"
            );
        }
    }

    #[test]
    fn the_cache_is_bounded() {
        // Distinct invalid keys must not grow the map without limit.
        let validator = validator();
        for index in 0..=MAX_CACHE_ENTRIES {
            validator.remember(
                HttpApiKeyValidator::digest(&format!("sk-{index}")),
                &ApiKeyValidation {
                    valid: false,
                    active: false,
                    key_id: None,
                },
            );
        }
        let size = validator
            .cache
            .read()
            .map_or(usize::MAX, |cache| cache.len());
        assert!(
            size <= MAX_CACHE_ENTRIES,
            "cache grew to {size}, past the {MAX_CACHE_ENTRIES} bound"
        );
    }

    #[actix_web::test]
    async fn an_unreachable_state_service_is_not_cached_as_a_verdict() {
        // The important negative. A transport failure returns `Unavailable`, and caching that as if
        // it were a verdict would pin the wrong answer for the TTL — including pinning "invalid"
        // over a key that is fine, or the reverse once state came back.
        use super::ApiKeyValidator as _;
        let validator = validator();
        let outcome = validator.validate("sk-test").await;
        assert!(outcome.is_err(), "127.0.0.1:1 should refuse the connection");
        assert!(
            validator.cache.read().is_ok_and(|cache| cache.is_empty()),
            "a transport failure must leave the cache untouched"
        );
    }

    #[actix_web::test]
    async fn an_oversized_key_is_rejected_without_caching() {
        // The length guard short-circuits before the network, and before the cache: there is no
        // verdict to remember for a key that was never asked about.
        use super::ApiKeyValidator as _;
        let validator = validator();
        let outcome = validator.validate(&"x".repeat(100_000)).await;
        assert_eq!(
            outcome.expect("the length guard answers rather than erroring"),
            ApiKeyValidation {
                valid: false,
                active: false,
                key_id: None,
            }
        );
        assert!(
            validator.cache.read().is_ok_and(|cache| cache.is_empty()),
            "a key rejected on length should not be cached"
        );
    }
}
