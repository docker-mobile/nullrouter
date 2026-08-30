//! Provider HTTP execution: URL fallback, per-status retry, and streaming.
//!
//! Ports `BaseExecutor.execute` from `open-sse/executors/base.js`.

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use nullrouter_providers::registry;
use reqwest::{Client, Response, StatusCode};
use serde_json::Value;

use crate::bespoke;
use crate::credentials::{Credentials, build_headers, build_url, fallback_count};
use crate::refresh;

/// Connect timeout when a provider declares none
/// (upstream `FETCH_CONNECT_TIMEOUT_MS`).
const DEFAULT_CONNECT_TIMEOUT_MS: u64 = 60 * 1000;
/// A token exchange is a short request; it must not inherit a provider's long
/// completion timeout, because a stalled refresh stalls the request behind it.
const REFRESH_TIMEOUT_MS: u64 = 30 * 1000;
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

/// Everything a dispatch needs beyond the URL and the client.
///
/// Exposed so a test can assert on what the executor *would* send to a provider
/// whose real endpoint is a fixed HTTPS host it cannot be pointed away from — the
/// envelope, the headers, and the URL suffix are the whole substance of those
/// providers, and asserting them any other way would mean asserting nothing.
#[derive(Debug, Clone)]
pub struct PreparedRequest {
    /// Headers to send, provider-specific ones applied last.
    pub headers: BTreeMap<String, String>,
    /// The body as it will go out, envelope included.
    pub body: Value,
    /// Appended to the resolved URL. Empty for most providers.
    pub url_suffix: &'static str,
}

/// Build the request a dispatch will send, without sending it.
pub fn prepare(request: &ExecuteRequest<'_>) -> PreparedRequest {
    let provider = request.provider;
    let mut headers = build_headers(provider, request.credentials, request.stream);
    // Provider-specific headers are applied last, so a provider that needs its own
    // `User-Agent` or `Accept` is not overridden by the generic one.
    let model = request
        .body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();
    for (key, value) in bespoke::extra_headers(provider, model, request.stream) {
        headers.insert(key, value);
    }
    PreparedRequest {
        headers,
        // A few providers wrap the body in an envelope the registry cannot describe.
        body: bespoke::envelope(provider, request.body, request.credentials)
            .unwrap_or_else(|| request.body.clone()),
        // Some providers select the method in the URL rather than the body.
        url_suffix: bespoke::url_suffix(provider, request.stream).unwrap_or_default(),
    }
}

/// Builds HTTP clients, reusing a pooled default and building per-proxy
/// clients only when a connection needs one.
#[derive(Debug, Clone)]
pub struct Executor {
    client: Client,
    /// Clients for connections that dispatch through an outbound proxy, keyed by the proxy
    /// specification.
    ///
    /// `reqwest::Client` owns a connection pool, so building one per call — which is what
    /// `client_for` used to do whenever a proxy was configured — means a fresh TCP connection to
    /// the proxy on every request, and a fresh TLS handshake with it. That is the one case where
    /// pooling matters most, since a proxy hop is usually the slowest link in the path.
    ///
    /// Keyed by `(proxy_url, no_proxy)` because both change which hosts the client bypasses, and a
    /// client built for one must not serve a connection configured with the other.
    proxied: std::sync::Arc<std::sync::RwLock<HashMap<(String, String), Client>>>,
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
            proxied: std::sync::Arc::default(),
        }
    }

    /// Ask a provider which models it serves.
    ///
    /// On the executor rather than on the caller so it goes through `client_for`: a
    /// connection that dispatches through an outbound proxy must probe through the same
    /// one, or the probe tests a network path the real requests never take — and would
    /// report a model list that then fails to dispatch, or fail while dispatch works.
    pub async fn probe_models(
        &self,
        provider: &str,
        credentials: &Credentials,
        timeout: Duration,
    ) -> Result<Vec<crate::probe::ProbedModel>, crate::probe::ProbeError> {
        crate::probe::probe_models(
            &self.client_for(credentials),
            provider,
            credentials,
            timeout,
        )
        .await
    }

    /// Client for a call, honoring any per-connection outbound proxy.
    ///
    /// Proxied clients are cached, because each one owns a connection pool and rebuilding it per
    /// request throws that pool away — see the `proxied` field.
    fn client_for(&self, credentials: &Credentials) -> Client {
        let Some(proxy_url) = credentials.proxy_url() else {
            return self.client.clone();
        };
        let key = (
            proxy_url.to_owned(),
            credentials.no_proxy().unwrap_or_default().to_owned(),
        );
        if let Ok(cache) = self.proxied.read()
            && let Some(client) = cache.get(&key)
        {
            return client.clone();
        }

        let Ok(mut proxy) = reqwest::Proxy::all(proxy_url) else {
            // An unparseable proxy URL must not silently bypass the proxy, but
            // upstream fails open here, so the direct client is used.
            tracing::warn!(proxy = %proxy_url, "ignoring unparseable proxy URL");
            return self.client.clone();
        };
        if let Some(no_proxy) = credentials.no_proxy() {
            proxy = proxy.no_proxy(reqwest::NoProxy::from_string(no_proxy));
        }
        let Ok(client) = Client::builder()
            .proxy(proxy)
            .pool_idle_timeout(Duration::from_secs(90))
            .build()
        else {
            return self.client.clone();
        };

        // A lost race just means two clients were built and one is dropped; `insert` keeps the
        // later one so every subsequent call shares a single pool either way.
        if let Ok(mut cache) = self.proxied.write() {
            cache.insert(key, client.clone());
        }
        client
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

    /// Exchange a refresh token for a new access token.
    ///
    /// De-duplicated through `cache`: two concurrent requests on one expiring
    /// connection must not both spend the token, because a provider that
    /// invalidates a reused refresh token would lock the account out.
    ///
    /// The connection's own proxy is honoured — a deployment that can only reach
    /// the internet through a proxy cannot reach the token endpoint either.
    pub async fn refresh_credentials(
        &self,
        provider: &str,
        credentials: &Credentials,
        cache: &refresh::RefreshCache,
    ) -> Result<refresh::Refreshed, refresh::RefreshError> {
        let Some(token) = credentials.refresh_token.as_deref() else {
            return Err(refresh::RefreshError::NotConfigured);
        };
        if !refresh::supports_refresh(provider) {
            return Err(refresh::RefreshError::Unsupported);
        }
        if let Some(cached) = cache.get(provider, token) {
            return cached;
        }

        let result = self.perform_refresh(provider, credentials, token).await;
        cache.put(provider, token, &result);
        result
    }

    /// The uncached exchange, against the provider's registered endpoint.
    async fn perform_refresh(
        &self,
        provider: &str,
        credentials: &Credentials,
        token: &str,
    ) -> Result<refresh::Refreshed, refresh::RefreshError> {
        let Some(url) = registry::entry(provider)
            .and_then(|entry| entry.oauth.as_ref())
            .and_then(|oauth| oauth.effective_refresh_url())
        else {
            return Err(refresh::RefreshError::NotConfigured);
        };
        self.refresh_at(url, provider, credentials, token).await
    }

    /// Exchange a refresh token at an explicit endpoint.
    ///
    /// The same split as [`Self::execute`] and [`Self::execute_at`]: the grant body
    /// and headers are the provider's, but the URL is supplied. Uncached and
    /// unconditional — callers serving a request should use
    /// [`Self::refresh_credentials`], which resolves the registered endpoint and
    /// de-duplicates.
    pub async fn refresh_at(
        &self,
        url: &str,
        provider: &str,
        credentials: &Credentials,
        token: &str,
    ) -> Result<refresh::Refreshed, refresh::RefreshError> {
        let Some(grant) = refresh::grant_body(provider, token) else {
            return Err(refresh::RefreshError::NotConfigured);
        };

        let mut builder = self
            .client_for(credentials)
            .post(url)
            .timeout(Duration::from_millis(REFRESH_TIMEOUT_MS))
            .header(reqwest::header::CONTENT_TYPE, grant.content_type);
        for (key, value) in refresh::grant_headers(provider, credentials) {
            builder = builder.header(key, value);
        }

        let response = match builder.body(grant.body).send().await {
            Ok(response) => response,
            Err(error) => {
                // A network failure says nothing about the token's validity, so it
                // must not be reported as a revoked credential.
                return Err(refresh::RefreshError::Transient {
                    message: format!("token endpoint unreachable: {error}"),
                });
            }
        };
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        refresh::settle(status, &body, token, refresh::now_millis())
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
        let prepared = prepare(&request);
        let PreparedRequest {
            headers,
            body: outgoing,
            url_suffix: suffix,
        } = &prepared;
        let payload = serde_json::to_vec(outgoing)
            .map_err(|error| ExecuteError::Serialize(error.to_string()))?;

        let total_urls = fallback_count(provider);
        let mut attempts_by_url: BTreeMap<usize, u32> = BTreeMap::new();
        let mut last_error: Option<ExecuteError> = None;
        let mut url_index = 0;

        while url_index < total_urls {
            let Some(url) = build_url(provider, request.credentials, url_index)
                .map(|url| format!("{url}{suffix}"))
            else {
                return Err(ExecuteError::NoEndpoint {
                    provider: provider.to_owned(),
                });
            };

            let mut builder = client.post(&url).timeout(timeout);
            for (key, value) in headers {
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
                        headers: headers.clone(),
                        // What actually went out, envelope included, so request
                        // logging shows the body the provider received.
                        sent_body: outgoing.clone(),
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

    /// How many proxied clients the executor has cached.
    ///
    /// A poisoned lock reads as zero, which fails the assertions rather than hiding behind them.
    fn cached_clients(executor: &Executor) -> usize {
        executor.proxied.read().map_or(0, |cache| cache.len())
    }

    /// Credentials that dispatch through `proxy_url`.
    fn proxied_credentials(proxy_url: &str, no_proxy: Option<&str>) -> Credentials {
        let mut settings = serde_json::Map::new();
        settings.insert("connectionProxyEnabled".to_owned(), json!(true));
        settings.insert("connectionProxyUrl".to_owned(), json!(proxy_url));
        if let Some(no_proxy) = no_proxy {
            settings.insert("connectionNoProxy".to_owned(), json!(no_proxy));
        }
        serde_json::from_value(json!({
            "connectionId": "conn_proxy",
            "connectionName": "proxied",
            "providerSpecificData": settings,
        }))
        .expect("credentials should deserialise")
    }

    #[test]
    fn a_proxied_client_is_built_once_and_reused() {
        // Each `reqwest::Client` owns a connection pool, so rebuilding one per request means a new
        // TCP connection — and TLS handshake — to the proxy every time. Asserted on the cache,
        // because pool reuse itself is not observable from here.
        let executor = Executor::new();
        let credentials = proxied_credentials("http://127.0.0.1:3128", None);

        let _first = executor.client_for(&credentials);
        assert_eq!(
            cached_clients(&executor),
            1,
            "the first call should populate the cache"
        );
        let _second = executor.client_for(&credentials);
        assert_eq!(
            cached_clients(&executor),
            1,
            "a second call with the same proxy must not build another client"
        );
    }

    #[test]
    fn different_proxies_do_not_share_a_client() {
        // Sharing would send a connection's traffic through another connection's proxy, which is
        // both wrong and a credential leak: the two proxies may belong to different tenants.
        let executor = Executor::new();
        let _one = executor.client_for(&proxied_credentials("http://127.0.0.1:3128", None));
        let _two = executor.client_for(&proxied_credentials("http://127.0.0.1:8080", None));
        assert_eq!(cached_clients(&executor), 2);
    }

    #[test]
    fn the_no_proxy_list_is_part_of_the_cache_key() {
        // Same proxy, different bypass lists: a client built for one would send traffic direct that
        // the other requires to be proxied, or the reverse.
        let executor = Executor::new();
        let _one = executor.client_for(&proxied_credentials("http://127.0.0.1:3128", None));
        let _two = executor.client_for(&proxied_credentials(
            "http://127.0.0.1:3128",
            Some("example.com"),
        ));
        assert_eq!(cached_clients(&executor), 2);
    }

    #[test]
    fn an_unproxied_connection_does_not_touch_the_cache() {
        // The common path stays a clone of the shared pooled client.
        let executor = Executor::new();
        let plain: Credentials = serde_json::from_value(json!({
            "connectionId": "conn_plain",
            "connectionName": "plain",
        }))
        .expect("credentials should deserialise");
        let _client = executor.client_for(&plain);
        assert_eq!(
            cached_clients(&executor),
            0,
            "an unproxied call should not populate the proxied cache"
        );
    }

    #[test]
    fn an_unparseable_proxy_url_is_not_cached() {
        // It falls back to the direct client; caching that under the bad URL would make the
        // fallback permanent even after the URL was corrected.
        let executor = Executor::new();
        let _client = executor.client_for(&proxied_credentials("not a url", None));
        assert_eq!(
            cached_clients(&executor),
            0,
            "a rejected proxy URL should not be cached"
        );
    }

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
