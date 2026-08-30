//! Loopback client for `nullrouter-state`'s internal endpoints.
//!
//! Credential selection, cooldown bookkeeping, and usage recording live in the
//! state service; the runtime reaches them over loopback HTTP so the two
//! services stay independently deployable.

use std::time::Duration;

use nullrouter_execute::Credentials;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Default loopback address of `nullrouter-state`.
const DEFAULT_STATE_ADDR: &str = "127.0.0.1:20134";
/// State calls are loopback and must never stall a provider request for long.
const STATE_TIMEOUT: Duration = Duration::from_secs(5);

/// Outcome of asking state for credentials.
#[derive(Debug)]
pub(crate) enum Selection {
    Selected(Box<Credentials>),
    /// No active connection is configured for the provider.
    NoCredentials {
        message: String,
    },
    /// Every connection is cooling down.
    AllRateLimited {
        retry_at_ms: u64,
        last_error: Option<String>,
        last_error_code: Option<u16>,
    },
    /// Every connection was already tried in this request.
    Exhausted,
    /// State itself is unreachable or failed.
    Unavailable {
        message: String,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SelectionResponse {
    status: String,
    #[serde(default)]
    credentials: Option<Credentials>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    retry_at_ms: Option<u64>,
    #[serde(default)]
    last_error: Option<String>,
    #[serde(default)]
    last_error_code: Option<u16>,
}

/// Routing inputs owned by state: combos, connections, and routing settings.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RoutingContext {
    #[serde(default)]
    pub combos: Vec<Combo>,
    /// Active connections, used to scope `/v1/models` to reachable providers.
    #[serde(default)]
    pub connections: Vec<ConnectionSummary>,
    #[serde(default)]
    pub settings: RoutingSettings,
}

/// A combo: one name fronting several models.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Combo {
    pub name: String,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub kind: Option<String>,
}

/// The non-secret shape of a connection, for model-list construction.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConnectionSummary {
    pub provider: String,
    #[serde(default)]
    pub prefix: Option<String>,
    #[serde(default)]
    pub enabled_models: Vec<String>,
}

/// Routing-relevant settings.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RoutingSettings {
    #[serde(default)]
    pub require_api_key: bool,
    /// How a combo picks among its models: `fallback`, `round-robin`, `fusion`.
    ///
    /// Absent or unrecognised reads as `fallback`, the upstream default, so a
    /// state service that predates this field does not change routing.
    #[serde(default)]
    pub combo_strategy: Option<String>,
    /// Requests to keep on one combo model before rotating.
    #[serde(default)]
    pub combo_sticky_round_robin_limit: Option<u32>,
    /// Whether the PXPIPE token saver is on.
    #[serde(default)]
    pub pxpipe_enabled: bool,
    /// Whether a missing PXPIPE package may be installed on demand.
    #[serde(default)]
    pub pxpipe_auto_install: bool,
    /// Body size below which PXPIPE compression is not attempted.
    #[serde(default)]
    pub pxpipe_min_chars: u64,
    /// Budget for one PXPIPE transform.
    #[serde(default)]
    pub pxpipe_timeout_ms: u64,
    /// Per-combo strategy overrides, keyed by combo name.
    ///
    /// A combo with an entry ignores [`Self::combo_strategy`] for itself alone.
    #[serde(default)]
    pub combo_strategies: std::collections::BTreeMap<String, ComboStrategyOverride>,
}

/// One combo's strategy override, as state reports it.
///
/// Every field optional: upstream persists only what the user changed, so an absent
/// tuning value means "use the default", not "reset to the default".
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ComboStrategyOverride {
    #[serde(default)]
    pub fallback_strategy: Option<String>,
    #[serde(default)]
    pub min_panel: Option<u32>,
    #[serde(default)]
    pub straggler_grace_ms: Option<u64>,
    #[serde(default)]
    pub panel_hard_timeout_ms: Option<u64>,
}

/// Usage report sent after a request finishes.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageReport {
    pub provider: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cached_tokens: u64,
    pub latency_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One account-cooldown report.
#[derive(Debug)]
pub(crate) struct Cooldown<'a> {
    pub connection_id: &'a str,
    /// Model that failed, for a per-model lock.
    pub model: Option<&'a str>,
    pub status: u16,
    pub reason: &'a str,
    pub duration_ms: u64,
    /// Present when the failure was quota-style.
    pub backoff_level: Option<u32>,
}

/// Client for the state service.
#[derive(Debug, Clone)]
pub(crate) struct StateClient {
    client: reqwest::Client,
    base: String,
}

impl Default for StateClient {
    fn default() -> Self {
        Self::from_env()
    }
}

impl StateClient {
    /// Client pointed at `NULLROUTER_STATE_ADDR`, or the default loopback port.
    pub(crate) fn from_env() -> Self {
        let addr = std::env::var("NULLROUTER_STATE_ADDR")
            .unwrap_or_else(|_| DEFAULT_STATE_ADDR.to_owned());
        Self::new(&addr)
    }

    /// Client for an explicit `host:port`.
    pub(crate) fn new(addr: &str) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(STATE_TIMEOUT)
                .build()
                .unwrap_or_default(),
            base: format!("http://{addr}"),
        }
    }

    /// Ask state for the next connection to try.
    pub(crate) async fn select_credentials(
        &self,
        provider: &str,
        model: Option<&str>,
        exclude: &[String],
    ) -> Selection {
        self.select_credentials_pinned(provider, model, exclude, None)
            .await
    }

    /// [`Self::select_credentials`], optionally pinned to one connection.
    ///
    /// The pin is honoured only when that connection is available; state falls back
    /// to its normal strategy otherwise. Used by the async video endpoints, where a
    /// job can only be polled by the account that created it.
    pub(crate) async fn select_credentials_pinned(
        &self,
        provider: &str,
        model: Option<&str>,
        exclude: &[String],
        preferred: Option<&str>,
    ) -> Selection {
        let url = format!("{}/internal/v1/credentials/select", self.base);
        let payload = json!({
            "provider": provider,
            "model": model,
            "exclude": exclude,
            "preferredConnectionId": preferred,
        });

        let response = match self.client.post(&url).json(&payload).send().await {
            Ok(response) => response,
            Err(error) => {
                return Selection::Unavailable {
                    message: format!("state service unreachable: {error}"),
                };
            }
        };

        let parsed = match response.json::<SelectionResponse>().await {
            Ok(parsed) => parsed,
            Err(error) => {
                return Selection::Unavailable {
                    message: format!("state service returned an unreadable response: {error}"),
                };
            }
        };

        match parsed.status.as_str() {
            "selected" => parsed.credentials.map_or_else(
                || Selection::Unavailable {
                    message: "state reported a selection without credentials".to_owned(),
                },
                |credentials| Selection::Selected(Box::new(credentials)),
            ),
            "no_credentials" => Selection::NoCredentials {
                message: parsed
                    .message
                    .unwrap_or_else(|| format!("No active credentials for provider: {provider}")),
            },
            "all_rate_limited" => Selection::AllRateLimited {
                retry_at_ms: parsed.retry_at_ms.unwrap_or_default(),
                last_error: parsed.last_error,
                last_error_code: parsed.last_error_code,
            },
            "exhausted" => Selection::Exhausted,
            other => Selection::Unavailable {
                message: format!("state returned an unknown selection status: {other}"),
            },
        }
    }

    /// Lock a connection after a failure. Best-effort: a failure here must not
    /// mask the provider error being reported.
    pub(crate) async fn mark_unavailable(&self, cooldown: &Cooldown<'_>) {
        let url = format!("{}/internal/v1/credentials/unavailable", self.base);
        let payload = json!({
            "connectionId": cooldown.connection_id,
            "model": cooldown.model,
            "status": cooldown.status,
            "reason": cooldown.reason,
            "cooldownMs": cooldown.duration_ms,
            "backoffLevel": cooldown.backoff_level,
        });
        if let Err(error) = self.client.post(&url).json(&payload).send().await {
            tracing::warn!(
                %error,
                connection_id = cooldown.connection_id,
                "failed to record account cooldown"
            );
        }
    }

    /// Clear a connection's error state after a success. Best-effort.
    pub(crate) async fn clear_error(&self, connection_id: &str, model: Option<&str>) {
        let url = format!("{}/internal/v1/credentials/clear-error", self.base);
        let payload = json!({ "connectionId": connection_id, "model": model });
        if let Err(error) = self.client.post(&url).json(&payload).send().await {
            tracing::warn!(%error, connection_id, "failed to clear account error");
        }
    }

    /// Persist a refreshed OAuth credential.
    ///
    /// `false` when the write did not land, so the caller can say the new token is
    /// in memory only — the next process to serve this connection will refresh
    /// again rather than reuse a token it never saw.
    pub(crate) async fn store_refreshed(&self, payload: &Value) -> bool {
        let url = format!("{}/internal/v1/credentials/refresh", self.base);
        match self.client.post(&url).json(payload).send().await {
            Ok(response) => response.status().is_success(),
            Err(error) => {
                tracing::warn!(%error, "failed to persist refreshed credentials");
                false
            }
        }
    }

    /// Record usage. Best-effort: telemetry must never fail a request.
    pub(crate) async fn record_usage(&self, report: &UsageReport) {
        let url = format!("{}/internal/v1/usage", self.base);
        if let Err(error) = self.client.post(&url).json(report).send().await {
            tracing::warn!(%error, "failed to record usage");
        }
    }

    /// Validate a client-supplied managed API key.
    ///
    /// Returns `false` when the key is unknown, inactive, or state cannot be
    /// reached — enforcement fails closed, since the caller only asks when
    /// `requireApiKey` is on.
    pub(crate) async fn validate_api_key(&self, api_key: &str) -> bool {
        let url = format!(
            "{}{}",
            self.base,
            nullrouter_contracts::INTERNAL_API_KEY_VALIDATE_PATH
        );
        let payload = json!({ "apiKey": api_key });
        match self.client.post(&url).json(&payload).send().await {
            Ok(response) => response
                .json::<nullrouter_contracts::ValidateApiKeyResponse>()
                .await
                .is_ok_and(|parsed| parsed.valid && parsed.active),
            Err(error) => {
                tracing::warn!(%error, "API key validation failed; denying request");
                false
            }
        }
    }

    /// Fetch combos and routing settings.
    ///
    /// Falls back to defaults when state is unreachable, so a state outage
    /// degrades routing rather than failing every request.
    pub(crate) async fn routing_context(&self) -> RoutingContext {
        let url = format!("{}/internal/v1/routing-context", self.base);
        match self.client.get(&url).send().await {
            Ok(response) => response.json::<RoutingContext>().await.unwrap_or_default(),
            Err(error) => {
                tracing::warn!(%error, "failed to read routing context; using defaults");
                RoutingContext::default()
            }
        }
    }

    /// Credentials for the connections whose model list must be asked for.
    ///
    /// One call for all of them, and read-only: `credentials/select` advances
    /// round-robin under a write lock, so probing through it would let listing models
    /// change which connection the next real request uses.
    ///
    /// An empty list on failure means `/v1/models` reports what it can from the
    /// registry instead of failing outright.
    pub(crate) async fn probe_targets(&self) -> Vec<ProbeTarget> {
        let url = format!("{}/internal/v1/probe-targets", self.base);
        match self.client.get(&url).send().await {
            Ok(response) => response
                .json::<ProbeTargetsResponse>()
                .await
                .map(|parsed| parsed.targets)
                .unwrap_or_default(),
            Err(error) => {
                tracing::warn!(%error, "failed to read probe targets; not probing");
                Vec::new()
            }
        }
    }
}

/// The probe targets state reported.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct ProbeTargetsResponse {
    #[serde(default)]
    pub targets: Vec<ProbeTarget>,
}

/// One connection whose provider can be asked for a model list.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProbeTarget {
    pub connection_id: String,
    pub provider: String,
    pub credentials: Credentials,
}
