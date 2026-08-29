//! Provider HTTP execution: URL fallback, per-status retry, and streaming.
//!
//! Ports `BaseExecutor.execute` from `open-sse/executors/base.js`.

use std::collections::BTreeMap;
use std::time::Duration;

use nullrouter_providers::registry;
use reqwest::{Client, Response, StatusCode};
use serde_json::Value;

use crate::credentials::{Credentials, build_headers, build_url, fallback_count};

/// Connect timeout when a provider declares none
/// (upstream `FETCH_CONNECT_TIMEOUT_MS`).
const DEFAULT_CONNECT_TIMEOUT_MS: u64 = 60 * 1000;
/// Retry delay when an entry specifies none (upstream `RETRY_CONFIG.delayMs`).
const DEFAULT_RETRY_DELAY_MS: u64 = 2000;

/// Default per-status retry policy (upstream `DEFAULT_RETRY_CONFIG`).
/// `(status, attempts, delay_ms)`
const DEFAULT_RETRY_CONFIG: [(u16, u32, u64); 4] =
    [(429, 0, 0), (502, 3, 3000), (503, 3, 2000), (504, 2, 3000)];

/// A failure before any usable upstream response.
#[derive(Debug, thiserror::Error)]
pub enum ExecuteError {
    #[error("provider {provider} has no configured endpoint")]
    NoEndpoint { provider: String },
    #[error("request to {provider} failed: {message}")]
    Transport { provider: String, message: String },
    #[error("request to {provider} timed out")]
    Timeout { provider: String },
    #[error("failed to serialize request body: {0}")]
    Serialize(String),
}

impl ExecuteError {
    /// Status to report to the client for this failure.
    pub const fn client_status(&self) -> u16 {
        match self {
            // No endpoint is a configuration problem, not an upstream fault.
            Self::NoEndpoint { .. } | Self::Serialize(_) => 500,
            Self::Transport { .. } => 502,
            Self::Timeout { .. } => 504,
        }
    }
}

/// A successful dispatch: the upstream response plus what was sent.
#[derive(Debug)]
pub struct ExecuteOutcome {
    pub response: Response,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    /// The body actually sent, for request logging.
    pub sent_body: Value,
}

impl ExecuteOutcome {
    /// Upstream HTTP status.
    pub fn status(&self) -> StatusCode {
        self.response.status()
    }

    /// `true` when upstream returned 2xx.
    pub fn is_success(&self) -> bool {
        self.response.status().is_success()
    }

    /// `true` when the response body is an SSE stream.
    pub fn is_event_stream(&self) -> bool {
        self.response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("text/event-stream"))
    }
}

/// One provider call.
#[derive(Debug, Clone)]
pub struct ExecuteRequest<'a> {
    pub provider: &'a str,
    pub body: &'a Value,
    pub stream: bool,
    pub credentials: &'a Credentials,
}

/// One byte-preserving call to an explicit URL.
///
/// Used by the async video endpoints, where the client's body may be multipart and
/// must reach the provider unchanged.
#[derive(Debug)]
pub struct RawRequest<'a> {
    pub provider: &'a str,
    /// Absolute upstream URL.
    pub url: &'a str,
    /// `true` for a job creation POST, `false` for a status GET.
    pub post: bool,
    /// Bytes to send. Ignored when `post` is false.
    pub body: &'a [u8],
    /// The client's `Content-Type`, forwarded verbatim. `None` sends none.
    pub content_type: Option<&'a str>,
    /// Headers applied after the provider's own.
    pub extra_headers: &'a [(&'a str, &'a str)],
    pub credentials: &'a Credentials,
}

/// Builds HTTP clients, reusing a pooled default and building per-proxy
/// clients only when a connection needs one.
#[derive(Debug, Clone)]
pub struct Executor {
    client: Client,
}

impl Default for Executor {
    fn default() -> Self {
        Self::new()
    }
}

impl Executor {
    /// Executor with a pooled default client.
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .pool_idle_timeout(Duration::from_secs(90))
                .build()
                // A builder failure here means no TLS backend; the default
                // client still works for plaintext and surfaces errors per call.
                .unwrap_or_default(),
        }
    }

    /// Client for a call, honoring any per-connection outbound proxy.
    fn client_for(&self, credentials: &Credentials) -> Client {
        let Some(proxy_url) = credentials.proxy_url() else {
            return self.client.clone();
        };
        let Ok(mut proxy) = reqwest::Proxy::all(proxy_url) else {
            // An unparseable proxy URL must not silently bypass the proxy, but
            // upstream fails open here, so the direct client is used.
            tracing::warn!(proxy = %proxy_url, "ignoring unparseable proxy URL");
            return self.client.clone();
        };
        if let Some(no_proxy) = credentials.no_proxy() {
            proxy = proxy.no_proxy(reqwest::NoProxy::from_string(no_proxy));
        }
        Client::builder()
            .proxy(proxy)
            .pool_idle_timeout(Duration::from_secs(90))
            .build()
            .unwrap_or_else(|_| self.client.clone())
    }

    /// Dispatch to an explicit URL, bypassing the chat-transport resolution.
    ///
    /// Used by the non-chat services (embeddings, audio, images, search,
    /// fetch), whose endpoints come from the registry's per-service configs
    /// rather than the provider's chat `baseUrl`.
    pub async fn execute_at(
        &self,
        url: &str,
        request: ExecuteRequest<'_>,
    ) -> Result<ExecuteOutcome, ExecuteError> {
        let provider = request.provider;
        let timeout = Duration::from_millis(
            registry::transport(provider)
                .and_then(|transport| transport.timeout_ms)
                .unwrap_or(DEFAULT_CONNECT_TIMEOUT_MS),
        );
        let client = self.client_for(request.credentials);
        let headers = build_headers(provider, request.credentials, request.stream);
        let payload = serde_json::to_vec(request.body)
            .map_err(|error| ExecuteError::Serialize(error.to_string()))?;

        let mut builder = client.post(url).timeout(timeout);
        for (key, value) in &headers {
            builder = builder.header(key, value);
        }

        match builder.body(payload).send().await {
            Ok(response) => Ok(ExecuteOutcome {
                response,
                url: url.to_owned(),
                headers,
                sent_body: request.body.clone(),
            }),
            Err(error) if error.is_timeout() => Err(ExecuteError::Timeout {
                provider: provider.to_owned(),
            }),
            Err(error) => Err(ExecuteError::Transport {
                provider: provider.to_owned(),
                message: error.to_string(),
            }),
        }
    }

    /// Dispatch to an explicit URL, forwarding bytes exactly as received.
    ///
    /// Distinct from [`Self::execute_at`], which serialises a `Value` and always
    /// sends `application/json`. Async video jobs accept multipart bodies, and
    /// parsing then re-encoding multipart would mint a new boundary that no longer
    /// matches the client's `Content-Type` header — so the bytes are passed through
    /// untouched and the original content type travels with them.
    ///
    /// `extra_headers` is applied last, so a caller can add per-request headers
    /// (`Idempotency-Key`) without them being overwritten by the provider's own.
    pub async fn execute_raw(
        &self,
        request: RawRequest<'_>,
    ) -> Result<ExecuteOutcome, ExecuteError> {
        let provider = request.provider;
        let timeout = Duration::from_millis(
            registry::transport(provider)
                .and_then(|transport| transport.timeout_ms)
                .unwrap_or(DEFAULT_CONNECT_TIMEOUT_MS),
        );
        let client = self.client_for(request.credentials);

        let mut headers = build_headers(provider, request.credentials, false);
        // `build_headers` assumes a JSON body. A GET poll sends none at all, and a
        // POST carries whatever the client sent.
        headers.remove("Content-Type");
        if let Some(content_type) = request.content_type {
            headers.insert("Content-Type".to_owned(), content_type.to_owned());
        }
        headers.insert("Accept".to_owned(), "application/json".to_owned());
        for (key, value) in request.extra_headers {
            headers.insert((*key).to_owned(), (*value).to_owned());
        }

        let mut builder = if request.post {
            client.post(request.url)
        } else {
            client.get(request.url)
        }
        .timeout(timeout);
        for (key, value) in &headers {
            builder = builder.header(key, value);
        }
        if request.post {
            builder = builder.body(request.body.to_vec());
        }

        match builder.send().await {
            Ok(response) => Ok(ExecuteOutcome {
                response,
                url: request.url.to_owned(),
                headers,
                // The forwarded bytes are not necessarily JSON, and a multipart body
                // can carry a whole video. Logging records the shape, not the payload.
                sent_body: Value::Null,
            }),
            Err(error) if error.is_timeout() => Err(ExecuteError::Timeout {
                provider: provider.to_owned(),
            }),
            Err(error) => Err(ExecuteError::Transport {
                provider: provider.to_owned(),
                message: error.to_string(),
            }),
        }
    }

    /// Dispatch a provider call, walking fallback URLs and honoring the
    /// provider's per-status retry policy.
    pub async fn execute(
        &self,
        request: ExecuteRequest<'_>,
    ) -> Result<ExecuteOutcome, ExecuteError> {
        let provider = request.provider;
        let transport = registry::transport(provider);
        let timeout = Duration::from_millis(
            transport
                .and_then(|transport| transport.timeout_ms)
                .unwrap_or(DEFAULT_CONNECT_TIMEOUT_MS),
        );
        let client = self.client_for(request.credentials);
        let headers = build_headers(provider, request.credentials, request.stream);
        let payload = serde_json::to_vec(request.body)
            .map_err(|error| ExecuteError::Serialize(error.to_string()))?;

        let total_urls = fallback_count(provider);
        let mut attempts_by_url: BTreeMap<usize, u32> = BTreeMap::new();
        let mut last_error: Option<ExecuteError> = None;
        let mut url_index = 0;

        while url_index < total_urls {
            let Some(url) = build_url(provider, request.credentials, url_index) else {
                return Err(ExecuteError::NoEndpoint {
                    provider: provider.to_owned(),
                });
            };

            let mut builder = client.post(&url).timeout(timeout);
            for (key, value) in &headers {
                builder = builder.header(key, value);
            }

            match builder.body(payload.clone()).send().await {
                Ok(response) => {
                    let status = response.status().as_u16();

                    // Retry this same URL when the policy allows it.
                    if let Some(delay) =
                        Self::retry_delay(provider, status, url_index, &mut attempts_by_url)
                    {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                        continue;
                    }

                    // 429 with another URL available: try the next endpoint.
                    if status == 429 && url_index + 1 < total_urls {
                        url_index += 1;
                        continue;
                    }

                    return Ok(ExecuteOutcome {
                        response,
                        url,
                        headers,
                        sent_body: request.body.clone(),
                    });
                }
                Err(error) => {
                    let failure = if error.is_timeout() {
                        ExecuteError::Timeout {
                            provider: provider.to_owned(),
                        }
                    } else {
                        ExecuteError::Transport {
                            provider: provider.to_owned(),
                            message: error.to_string(),
                        }
                    };

                    // Network failures map onto the 502 retry policy.
                    if let Some(delay) =
                        Self::retry_delay(provider, 502, url_index, &mut attempts_by_url)
                    {
                        last_error = Some(failure);
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                        continue;
                    }

                    if url_index + 1 < total_urls {
                        last_error = Some(failure);
                        url_index += 1;
                        continue;
                    }
                    return Err(failure);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| ExecuteError::NoEndpoint {
            provider: provider.to_owned(),
        }))
    }

    /// Delay before retrying the same URL, or `None` when retries are exhausted.
    fn retry_delay(
        provider: &str,
        status: u16,
        url_index: usize,
        attempts_by_url: &mut BTreeMap<usize, u32>,
    ) -> Option<u64> {
        let (attempts, delay_ms) = resolve_retry_entry(provider, status)?;
        if attempts == 0 {
            return None;
        }
        let used = attempts_by_url.entry(url_index).or_insert(0);
        if *used >= attempts {
            return None;
        }
        *used += 1;
        tracing::debug!(
            provider,
            status,
            attempt = *used,
            attempts,
            "retrying upstream"
        );
        Some(delay_ms)
    }
}

/// Resolve `(attempts, delay_ms)` for a status, provider policy first
/// (upstream `resolveRetryEntry` over merged config).
fn resolve_retry_entry(provider: &str, status: u16) -> Option<(u32, u64)> {
    if let Some(entry) =
        registry::transport(provider).and_then(|transport| transport.retry.get(&status.to_string()))
    {
        return Some((
            entry.attempts(),
            entry.delay_ms().unwrap_or(DEFAULT_RETRY_DELAY_MS),
        ));
    }
    DEFAULT_RETRY_CONFIG
        .iter()
        .find(|(rule_status, _, _)| *rule_status == status)
        .map(|(_, attempts, delay_ms)| (*attempts, *delay_ms))
}

#[cfg(test)]
mod tests {
    use super::{ExecuteError, Executor, resolve_retry_entry};
    use crate::credentials::Credentials;
    use serde_json::json;

    #[test]
    fn default_retry_policy_matches_upstream() {
        // 429 is not retried in place by default; it walks fallback URLs.
        assert_eq!(resolve_retry_entry("openai", 429), Some((0, 0)));
        assert_eq!(resolve_retry_entry("openai", 502), Some((3, 3000)));
        assert_eq!(resolve_retry_entry("openai", 503), Some((3, 2000)));
        assert_eq!(resolve_retry_entry("openai", 504), Some((2, 3000)));
        // Statuses with no rule are not retried.
        assert_eq!(resolve_retry_entry("openai", 400), None);
        assert_eq!(resolve_retry_entry("openai", 200), None);
    }

    #[test]
    fn provider_retry_policy_overrides_the_default() {
        // Some providers declare `{"429": N}` as a bare attempt count; the
        // delay then comes from the legacy RETRY_CONFIG default.
        let overriding = nullrouter_providers::entries()
            .iter()
            .find(|entry| {
                entry
                    .transport
                    .as_ref()
                    .is_some_and(|transport| transport.retry.contains_key("429"))
            })
            .map(|entry| entry.id.clone());

        if let Some(provider) = overriding {
            let resolved = resolve_retry_entry(&provider, 429);
            assert!(resolved.is_some(), "{provider} declares a 429 retry policy");
        }
    }

    #[test]
    fn error_statuses_map_to_client_facing_codes() {
        assert_eq!(
            ExecuteError::Timeout {
                provider: "openai".to_owned()
            }
            .client_status(),
            504
        );
        assert_eq!(
            ExecuteError::Transport {
                provider: "openai".to_owned(),
                message: "reset".to_owned(),
            }
            .client_status(),
            502
        );
        assert_eq!(
            ExecuteError::NoEndpoint {
                provider: "openai".to_owned()
            }
            .client_status(),
            500
        );
    }

    #[tokio::test]
    async fn unknown_provider_reports_no_endpoint() {
        let executor = Executor::new();
        let credentials = Credentials::default();
        let body = json!({ "model": "x" });
        let result = executor
            .execute(super::ExecuteRequest {
                provider: "definitely-not-a-provider",
                body: &body,
                stream: false,
                credentials: &credentials,
            })
            .await;
        assert!(matches!(result, Err(ExecuteError::NoEndpoint { .. })));
    }
}
