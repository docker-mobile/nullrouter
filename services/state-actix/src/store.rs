use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::{
    api_keys::{ApiKeyRecord, migrate_legacy_records},
    provider_nodes::ProviderNode,
};

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("state lock is poisoned")]
    Poisoned,
    #[error("state io failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("state json failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("secure random generation failed")]
    Random,
}

#[derive(Debug, Clone)]
pub struct StateStore {
    inner: Arc<StoreInner>,
}

#[derive(Debug)]
struct StoreInner {
    path: Option<PathBuf>,
    snapshot: RwLock<StateSnapshot>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StateSnapshot {
    pub(crate) api_keys: Vec<ApiKeyRecord>,
    pub(crate) provider_connections: Vec<ProviderConnection>,
    #[serde(default)]
    pub(crate) provider_nodes: Vec<ProviderNode>,
    combos: Vec<Combo>,
    proxy_pools: Vec<ProxyPool>,
    settings: Settings,
    #[serde(default)]
    pub(crate) usage: crate::usage::UsageLog,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConnection {
    pub id: String,
    pub provider: String,
    pub auth_type: String,
    pub name: String,
    pub priority: u32,
    pub is_active: bool,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub global_priority: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_specific_data: Option<BTreeMap<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    // ── runtime execution fields ──
    // Added for provider execution; all default so existing state files load
    // unchanged. Never returned by the public API surface.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    /// Last selection time, driving round-robin ordering.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<String>,
    /// Consecutive selections, for sticky round-robin.
    #[serde(default)]
    pub consecutive_use_count: u32,
    /// Exponential backoff level for quota errors.
    #[serde(default)]
    pub backoff_level: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<u16>,
    /// Per-model cooldown expiries (RFC3339). Upstream stores these as flat
    /// `modelLock_${model}` fields; a typed map is used here since this port
    /// owns its own state file. `__all` is the account-level lock.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub model_locks: BTreeMap<String, String>,
}

/// Account-level lock key (upstream `modelLock___all`).
pub(crate) const MODEL_LOCK_ALL: &str = "__all";

impl ProviderConnection {
    /// `true` when this connection is cooling down for `model`.
    ///
    /// The account-level lock applies to every model.
    fn is_locked(&self, model: Option<&str>, now_ms: u64) -> bool {
        let expiry = model
            .and_then(|model| self.model_locks.get(model))
            .or_else(|| self.model_locks.get(MODEL_LOCK_ALL));
        expiry
            .and_then(|expiry| iso_to_millis(expiry))
            .is_some_and(|expiry| expiry > now_ms)
    }

    /// Earliest still-active lock expiry, for retry hints.
    fn earliest_lock_ms(&self, now_ms: u64) -> Option<u64> {
        self.model_locks
            .values()
            .filter_map(|expiry| iso_to_millis(expiry))
            .filter(|expiry| *expiry > now_ms)
            .min()
    }
}

/// Account-selection strategy (upstream `settings.fallbackStrategy`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum FallbackStrategy {
    /// Lowest priority number first.
    #[default]
    FillFirst,
    /// Rotate, sticking with an account for a bounded number of calls.
    RoundRobin,
}

/// Inputs for [`StateStore::select_connection`].
#[derive(Debug)]
pub(crate) struct SelectConnectionRequest<'a> {
    pub provider: &'a str,
    /// Model being requested, for per-model cooldowns.
    pub model: Option<&'a str>,
    /// Connections already tried and failed in this request.
    pub exclude: &'a [String],
    /// Pin to this connection when it is available, skipping the strategy.
    ///
    /// Async video jobs are account-bound upstream: the account that created a job
    /// is the only one that can poll it, so the client echoes the creating
    /// connection back and selection must honour it.
    pub preferred: Option<&'a str>,
    pub strategy: FallbackStrategy,
    pub sticky_limit: u32,
}

/// Outcome of a selection attempt.
#[derive(Debug)]
pub(crate) enum ConnectionSelection {
    Selected(Box<ProviderConnection>),
    /// No active connection exists for this provider at all.
    NoCredentials,
    /// Every connection is cooling down; retry is possible later.
    AllRateLimited {
        retry_at_ms: u64,
        last_error: Option<String>,
        last_error_code: Option<u16>,
    },
    /// Every connection was already tried in this request.
    Exhausted,
}

/// Inputs for [`StateStore::mark_connection_unavailable`].
#[derive(Debug)]
pub(crate) struct MarkUnavailableRequest<'a> {
    pub connection_id: &'a str,
    pub model: Option<&'a str>,
    pub status: u16,
    pub reason: &'a str,
    pub cooldown_ms: u64,
    pub backoff_level: Option<u32>,
}

/// Refreshed OAuth credentials to persist.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CredentialUpdate {
    #[serde(default)]
    pub access_token: Option<String>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_at: Option<String>,
    /// Keys to merge into the connection's settings, not replace them with.
    ///
    /// A refresh carries `lastRefreshAt` and sometimes an `idToken`. Replacing the
    /// whole map would drop the connection's `baseUrl`, region, and proxy
    /// configuration on the first token rotation.
    #[serde(default)]
    pub provider_specific_data: Option<BTreeMap<String, Value>>,
}

/// Milliseconds since the Unix epoch.
fn now_millis() -> u64 {
    u64::try_from(current_millis()).unwrap_or(u64::MAX)
}

/// Current time in this store's timestamp format.
fn now_iso() -> String {
    timestamp()
}

/// Format epoch millis using this store's `unix-ms:` timestamp convention.
fn millis_to_iso(millis: u64) -> String {
    format!("unix-ms:{millis}")
}

/// Parse a stored timestamp back to epoch millis.
///
/// Accepts this store's `unix-ms:` form and a bare integer, so timestamps
/// written by an older build still parse.
fn iso_to_millis(value: &str) -> Option<u64> {
    value
        .strip_prefix("unix-ms:")
        .unwrap_or(value)
        .trim()
        .parse::<u64>()
        .ok()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Combo {
    pub id: String,
    pub name: String,
    pub kind: Option<String>,
    pub models: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProxyPool {
    pub id: String,
    pub name: String,
    pub proxy_url: String,
    pub no_proxy: String,
    #[serde(rename = "type")]
    pub proxy_type: String,
    pub is_active: bool,
    pub strict_proxy: bool,
    pub test_status: String,
    pub last_tested_at: Option<String>,
    pub last_error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Settings {
    pub tunnel_dashboard_access: bool,
    pub tunnel_url: String,
    pub tailscale_url: String,
    #[serde(default)]
    pub outbound_proxy_enabled: bool,
    #[serde(default)]
    pub outbound_proxy_url: String,
    #[serde(default)]
    pub outbound_no_proxy: String,
    // ── OIDC dashboard login ──
    // All default so a state file written before these existed still loads.
    // Empty means "not configured"; normalisation of scopes and the login label
    // happens in `nullrouter-auth`, which owns the flow.
    #[serde(default)]
    pub oidc_issuer_url: String,
    #[serde(default)]
    pub oidc_client_id: String,
    /// Secret. Never leaves this service through the public API — `SettingsView`
    /// reports only `oidcClientSecretSet`.
    #[serde(default)]
    pub oidc_client_secret: String,
    #[serde(default)]
    pub oidc_scopes: String,
    #[serde(default)]
    pub oidc_login_label: String,
    // ── SAML dashboard login ──
    #[serde(default)]
    pub saml_entry_point: String,
    #[serde(default)]
    pub saml_issuer: String,
    /// The IdP's X.509 signing certificate. Held to the same discipline as
    /// `oidc_client_secret`: `SettingsView` reports only `samlCertSet`.
    #[serde(default)]
    pub saml_cert: String,
    #[serde(default)]
    pub saml_attribute_email: String,
    #[serde(default)]
    pub saml_attribute_name: String,
    // ── PXPIPE token saver ──
    // Read by the runtime through `/internal/v1/routing-context`, and reported on
    // the public `/api/settings` as upstream reports them: the dashboard's Token
    // Saver page is the only place they are set.
    /// Whether bulky Claude-format context is rendered to images before dispatch.
    ///
    /// Off by default, as upstream: it changes what the provider is sent, and a
    /// token saver that turns itself on is not one a user chose.
    #[serde(default)]
    pub pxpipe_enabled: bool,
    /// Whether a missing package may be installed on demand.
    #[serde(default = "default_true")]
    pub pxpipe_auto_install: bool,
    /// Body size below which compression is not attempted (0 = the default).
    #[serde(default = "default_pxpipe_min_chars")]
    pub pxpipe_min_chars: u64,
    /// Budget for one transform (0 = the default).
    #[serde(default = "default_pxpipe_timeout_ms")]
    pub pxpipe_timeout_ms: u64,
    // ── routing settings, read by the runtime ──
    // Persisted here, but omitted from the public `/api/settings` response,
    // whose shape is pinned by `dashboard_route_parity`. The runtime reads
    // these through `/internal/v1/routing-context`.
    /// `fill-first` (default) or `round-robin`.
    #[serde(default = "default_fallback_strategy")]
    pub fallback_strategy: String,
    /// Calls to keep on one account before rotating (upstream default 3).
    #[serde(default = "default_sticky_limit")]
    pub sticky_round_robin_limit: u32,
    /// How a combo picks among its models: `fallback` (default), `round-robin`,
    /// or `fusion`.
    ///
    /// Distinct from [`Self::fallback_strategy`], which chooses among *accounts*
    /// for one model. A combo chooses among *models*.
    #[serde(default = "default_combo_strategy")]
    pub combo_strategy: String,
    /// Requests to keep on one combo model before rotating (upstream default 1).
    #[serde(default = "default_combo_sticky_limit")]
    pub combo_sticky_round_robin_limit: u32,
    /// Require a managed API key on `/v1` calls.
    #[serde(default)]
    pub require_api_key: bool,
}

/// The public `/api/settings` projection.
///
/// Separate from [`Settings`] so persistence can carry routing fields without
/// widening the dashboard-facing contract.
///
/// Secrets are never projected. `oidc_client_secret` and `saml_cert` are
/// reported as the booleans `oidcClientSecretSet` / `samlCertSet`, the same
/// discipline `ProviderConnection::public` applies to provider API keys: the
/// dashboard can render "configured" without the value crossing the wire.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SettingsView {
    pub tunnel_dashboard_access: bool,
    pub tunnel_url: String,
    pub tailscale_url: String,
    pub outbound_proxy_enabled: bool,
    pub outbound_proxy_url: String,
    pub outbound_no_proxy: String,
    pub oidc_issuer_url: String,
    pub oidc_client_id: String,
    pub oidc_client_secret_set: bool,
    pub oidc_scopes: String,
    pub oidc_login_label: String,
    pub saml_entry_point: String,
    pub saml_issuer: String,
    pub saml_cert_set: bool,
    pub saml_attribute_email: String,
    pub saml_attribute_name: String,
    pub pxpipe_enabled: bool,
    pub pxpipe_auto_install: bool,
    pub pxpipe_min_chars: u64,
    pub pxpipe_timeout_ms: u64,
}

impl From<Settings> for SettingsView {
    fn from(settings: Settings) -> Self {
        Self {
            tunnel_dashboard_access: settings.tunnel_dashboard_access,
            tunnel_url: settings.tunnel_url,
            tailscale_url: settings.tailscale_url,
            outbound_proxy_enabled: settings.outbound_proxy_enabled,
            outbound_proxy_url: settings.outbound_proxy_url,
            outbound_no_proxy: settings.outbound_no_proxy,
            oidc_issuer_url: settings.oidc_issuer_url,
            oidc_client_id: settings.oidc_client_id,
            oidc_client_secret_set: !settings.oidc_client_secret.is_empty(),
            oidc_scopes: settings.oidc_scopes,
            oidc_login_label: settings.oidc_login_label,
            saml_entry_point: settings.saml_entry_point,
            saml_issuer: settings.saml_issuer,
            saml_cert_set: !settings.saml_cert.is_empty(),
            saml_attribute_email: settings.saml_attribute_email,
            saml_attribute_name: settings.saml_attribute_name,
            pxpipe_enabled: settings.pxpipe_enabled,
            pxpipe_auto_install: settings.pxpipe_auto_install,
            pxpipe_min_chars: settings.pxpipe_min_chars,
            pxpipe_timeout_ms: settings.pxpipe_timeout_ms,
        }
    }
}

const fn default_true() -> bool {
    true
}

/// Upstream's `pxpipeMinChars` default.
const fn default_pxpipe_min_chars() -> u64 {
    25_000
}

/// Upstream's `pxpipeTimeoutMs` default.
const fn default_pxpipe_timeout_ms() -> u64 {
    15_000
}

fn default_fallback_strategy() -> String {
    "fill-first".to_owned()
}

const fn default_sticky_limit() -> u32 {
    3
}

fn default_combo_strategy() -> String {
    "fallback".to_owned()
}

/// Upstream's `comboStickyRoundRobinLimit` default: switch model every request.
const fn default_combo_sticky_limit() -> u32 {
    1
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            tunnel_dashboard_access: false,
            tunnel_url: String::new(),
            tailscale_url: String::new(),
            outbound_proxy_enabled: false,
            outbound_proxy_url: String::new(),
            outbound_no_proxy: String::new(),
            oidc_issuer_url: String::new(),
            oidc_client_id: String::new(),
            oidc_client_secret: String::new(),
            oidc_scopes: String::new(),
            oidc_login_label: String::new(),
            saml_entry_point: String::new(),
            saml_issuer: String::new(),
            saml_cert: String::new(),
            saml_attribute_email: String::new(),
            saml_attribute_name: String::new(),
            pxpipe_enabled: false,
            pxpipe_auto_install: default_true(),
            pxpipe_min_chars: default_pxpipe_min_chars(),
            pxpipe_timeout_ms: default_pxpipe_timeout_ms(),
            fallback_strategy: default_fallback_strategy(),
            sticky_round_robin_limit: default_sticky_limit(),
            combo_strategy: default_combo_strategy(),
            combo_sticky_round_robin_limit: default_combo_sticky_limit(),
            require_api_key: false,
        }
    }
}

impl StateStore {
    pub fn memory() -> Self {
        Self::from_snapshot(None, StateSnapshot::default())
    }

    pub fn file(path: &Path) -> Result<Self, StoreError> {
        let mut snapshot = if path.exists() {
            let bytes = std::fs::read(path)?;
            serde_json::from_slice(&bytes)?
        } else {
            StateSnapshot::default()
        };
        let migrated = migrate_legacy_records(&mut snapshot.api_keys);
        let store = Self::from_snapshot(Some(path.to_owned()), snapshot.clone());
        if migrated {
            store.persist(&snapshot)?;
        }
        Ok(store)
    }

    fn from_snapshot(path: Option<PathBuf>, snapshot: StateSnapshot) -> Self {
        Self {
            inner: Arc::new(StoreInner {
                path,
                snapshot: RwLock::new(snapshot),
            }),
        }
    }

    pub(crate) fn health(&self) -> Result<serde_json::Value, StoreError> {
        let snapshot = self.read_snapshot()?;
        Ok(serde_json::json!({
            "ok": true,
            "service": crate::SERVICE_NAME,
            "keys": snapshot.api_keys.len(),
            "connections": snapshot.provider_connections.len(),
            "providerNodes": snapshot.provider_nodes.len(),
            "combos": snapshot.combos.len(),
            "proxyPools": snapshot.proxy_pools.len(),
        }))
    }

    pub(crate) fn list_connections(&self) -> Result<Vec<ProviderConnection>, StoreError> {
        let mut connections = self.read_snapshot()?.provider_connections;
        connections.sort_by_key(|connection| connection.priority);
        Ok(connections
            .into_iter()
            .map(ProviderConnection::public)
            .collect())
    }

    pub(crate) fn get_connection(
        &self,
        id: &str,
    ) -> Result<Option<ProviderConnection>, StoreError> {
        Ok(self
            .read_snapshot()?
            .provider_connections
            .into_iter()
            .find(|connection| connection.id == id)
            .map(ProviderConnection::public))
    }

    pub(crate) fn create_connection(
        &self,
        mut request: ProviderConnectionInput,
    ) -> Result<ProviderConnection, StoreError> {
        self.write_snapshot(|snapshot| {
            let now = timestamp();
            let provider_count = snapshot
                .provider_connections
                .iter()
                .filter(|connection| connection.provider == request.provider)
                .count();
            let priority = request
                .priority
                .take()
                .unwrap_or_else(|| u32::try_from(provider_count + 1).unwrap_or(u32::MAX));
            let id = next_id("conn", snapshot.provider_connections.len());
            let connection = ProviderConnection {
                id,
                provider: request.provider,
                auth_type: request.auth_type.unwrap_or_else(|| "apikey".to_owned()),
                name: request.name,
                priority,
                is_active: request.is_active.unwrap_or(true),
                created_at: now.clone(),
                updated_at: now,
                email: request.email,
                global_priority: request.global_priority,
                default_model: request.default_model,
                test_status: request.test_status.or_else(|| Some("unknown".to_owned())),
                last_error: request.last_error,
                last_error_at: request.last_error_at,
                provider_specific_data: request.provider_specific_data,
                api_key: request.api_key,
                access_token: request.access_token,
                refresh_token: request.refresh_token,
                expires_at: request.expires_at,
                last_used_at: None,
                consecutive_use_count: 0,
                backoff_level: 0,
                error_code: None,
                model_locks: BTreeMap::new(),
            };
            if let Some(existing) = snapshot.provider_connections.iter_mut().find(|existing| {
                existing.provider == connection.provider
                    && existing.auth_type == "apikey"
                    && existing.name == connection.name
            }) {
                let updated = merge_connection(existing.clone(), connection);
                *existing = updated.clone();
                return updated.public();
            }
            snapshot.provider_connections.push(connection.clone());
            connection.public()
        })
    }

    pub(crate) fn update_connection(
        &self,
        id: &str,
        input: ProviderConnectionUpdate,
    ) -> Result<Option<ProviderConnection>, StoreError> {
        self.write_snapshot(|snapshot| {
            let connection = snapshot
                .provider_connections
                .iter_mut()
                .find(|connection| connection.id == id)?;
            if let Some(name) = input.name {
                connection.name = name;
            }
            if let Some(priority) = input.priority {
                connection.priority = priority;
            }
            if let Some(global_priority) = input.global_priority {
                connection.global_priority = Some(global_priority);
            }
            if let Some(default_model) = input.default_model {
                connection.default_model = Some(default_model);
            }
            if let Some(is_active) = input.is_active {
                connection.is_active = is_active;
            }
            if let Some(api_key) = input.api_key {
                connection.api_key = Some(api_key);
            }
            if let Some(test_status) = input.test_status {
                connection.test_status = Some(test_status);
            }
            if let Some(last_error) = input.last_error {
                connection.last_error = Some(last_error);
            }
            if let Some(last_error_at) = input.last_error_at {
                connection.last_error_at = Some(last_error_at);
            }
            if let Some(provider_specific_data) = input.provider_specific_data {
                connection.provider_specific_data = Some(provider_specific_data);
            }
            connection.updated_at = timestamp();
            Some(connection.clone().public())
        })
    }

    pub(crate) fn delete_connection(&self, id: &str) -> Result<bool, StoreError> {
        self.write_snapshot(|snapshot| {
            let original_len = snapshot.provider_connections.len();
            snapshot
                .provider_connections
                .retain(|connection| connection.id != id);
            snapshot.provider_connections.len() != original_len
        })
    }

    pub(crate) fn proxy_pool_exists(&self, id: &str) -> Result<bool, StoreError> {
        Ok(self
            .read_snapshot()?
            .proxy_pools
            .iter()
            .any(|pool| pool.id == id))
    }

    pub(crate) fn list_combos(&self) -> Result<Vec<Combo>, StoreError> {
        Ok(self.read_snapshot()?.combos)
    }

    pub(crate) fn get_combo(&self, id: &str) -> Result<Option<Combo>, StoreError> {
        Ok(self
            .read_snapshot()?
            .combos
            .into_iter()
            .find(|combo| combo.id == id))
    }

    pub(crate) fn combo_name_exists(
        &self,
        name: &str,
        except_id: Option<&str>,
    ) -> Result<bool, StoreError> {
        Ok(self.read_snapshot()?.combos.iter().any(|combo| {
            combo.name == name && except_id.is_none_or(|except_id| combo.id != except_id)
        }))
    }

    pub(crate) fn create_combo(
        &self,
        name: String,
        kind: Option<String>,
        models: Vec<String>,
    ) -> Result<Combo, StoreError> {
        self.write_snapshot(|snapshot| {
            let now = timestamp();
            let combo = Combo {
                id: next_combo_id(&snapshot.combos),
                name,
                kind,
                models,
                created_at: now.clone(),
                updated_at: now,
            };
            snapshot.combos.push(combo.clone());
            combo
        })
    }

    pub(crate) fn update_combo(
        &self,
        id: &str,
        update: ComboUpdate,
    ) -> Result<Option<Combo>, StoreError> {
        self.write_snapshot(|snapshot| {
            let combo = snapshot.combos.iter_mut().find(|combo| combo.id == id)?;
            if let Some(name) = update.name {
                combo.name = name;
            }
            if update.kind_set {
                combo.kind = update.kind;
            }
            if let Some(models) = update.models {
                combo.models = models;
            }
            combo.updated_at = timestamp();
            Some(combo.clone())
        })
    }

    pub(crate) fn delete_combo(&self, id: &str) -> Result<bool, StoreError> {
        self.write_snapshot(|snapshot| {
            let original_len = snapshot.combos.len();
            snapshot.combos.retain(|combo| combo.id != id);
            snapshot.combos.len() != original_len
        })
    }

    pub(crate) fn list_proxy_pools(
        &self,
        is_active: Option<bool>,
        include_usage: bool,
    ) -> Result<Vec<Value>, StoreError> {
        let snapshot = self.read_snapshot()?;
        let mut pools = snapshot.proxy_pools;
        pools.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
        let values = pools
            .into_iter()
            .filter(|pool| is_active.is_none_or(|is_active| pool.is_active == is_active))
            .map(|pool| {
                let mut value = serde_json::to_value(&pool).unwrap_or(Value::Null);
                if include_usage {
                    let count = bound_connection_count(&snapshot.provider_connections, &pool.id);
                    if let Value::Object(ref mut object) = value {
                        object.insert("boundConnectionCount".to_owned(), Value::from(count));
                    }
                }
                value
            })
            .collect();
        Ok(values)
    }

    pub(crate) fn get_proxy_pool(&self, id: &str) -> Result<Option<ProxyPool>, StoreError> {
        Ok(self
            .read_snapshot()?
            .proxy_pools
            .into_iter()
            .find(|pool| pool.id == id))
    }

    pub(crate) fn create_proxy_pool(&self, input: ProxyPoolInput) -> Result<ProxyPool, StoreError> {
        self.write_snapshot(|snapshot| {
            let now = timestamp();
            let pool = ProxyPool {
                id: next_id("proxy_pool", snapshot.proxy_pools.len()),
                name: input.name,
                proxy_url: input.proxy_url,
                no_proxy: input.no_proxy.unwrap_or_default(),
                proxy_type: input.proxy_type.unwrap_or_else(|| "http".to_owned()),
                is_active: input.is_active.unwrap_or(true),
                strict_proxy: input.strict_proxy.unwrap_or(false),
                test_status: input.test_status.unwrap_or_else(|| "unknown".to_owned()),
                last_tested_at: None,
                last_error: None,
                created_at: now.clone(),
                updated_at: now,
            };
            snapshot.proxy_pools.push(pool.clone());
            pool
        })
    }

    pub(crate) fn update_proxy_pool(
        &self,
        id: &str,
        input: ProxyPoolUpdate,
    ) -> Result<Option<ProxyPool>, StoreError> {
        self.write_snapshot(|snapshot| {
            let pool = snapshot.proxy_pools.iter_mut().find(|pool| pool.id == id)?;
            if let Some(name) = input.name {
                pool.name = name;
            }
            if let Some(proxy_url) = input.proxy_url {
                pool.proxy_url = proxy_url;
            }
            if let Some(no_proxy) = input.no_proxy {
                pool.no_proxy = no_proxy;
            }
            if let Some(is_active) = input.is_active {
                pool.is_active = is_active;
            }
            if let Some(strict_proxy) = input.strict_proxy {
                pool.strict_proxy = strict_proxy;
            }
            if let Some(proxy_type) = input.proxy_type {
                pool.proxy_type = proxy_type;
            }
            pool.updated_at = timestamp();
            Some(pool.clone())
        })
    }

    pub(crate) fn delete_proxy_pool(&self, id: &str) -> Result<DeleteProxyPoolResult, StoreError> {
        self.write_snapshot(|snapshot| {
            let bound_connection_count = bound_connection_count(&snapshot.provider_connections, id);
            if bound_connection_count > 0 {
                return DeleteProxyPoolResult::InUse {
                    bound_connection_count,
                };
            }
            let original_len = snapshot.proxy_pools.len();
            snapshot.proxy_pools.retain(|pool| pool.id != id);
            if snapshot.proxy_pools.len() == original_len {
                DeleteProxyPoolResult::NotFound
            } else {
                DeleteProxyPoolResult::Deleted
            }
        })
    }

    pub(crate) fn settings(&self) -> Result<Settings, StoreError> {
        Ok(self.read_snapshot()?.settings)
    }

    /// Apply a settings patch.
    ///
    /// Every field is `Option`, and `None` means "not in the request body", so a
    /// `PUT` carrying one key leaves the rest alone. That is what keeps a
    /// settings write from clearing a stored secret it never mentioned; see
    /// [`SettingsUpdate`].
    pub(crate) fn update_settings(&self, update: SettingsUpdate) -> Result<Settings, StoreError> {
        self.write_snapshot(|snapshot| {
            if let Some(tunnel_dashboard_access) = update.tunnel_dashboard_access {
                snapshot.settings.tunnel_dashboard_access = tunnel_dashboard_access;
            }
            if let Some(tunnel_url) = update.tunnel_url {
                snapshot.settings.tunnel_url = tunnel_url;
            }
            if let Some(tailscale_url) = update.tailscale_url {
                snapshot.settings.tailscale_url = tailscale_url;
            }
            if let Some(outbound_proxy_enabled) = update.outbound_proxy_enabled {
                snapshot.settings.outbound_proxy_enabled = outbound_proxy_enabled;
            }
            if let Some(outbound_proxy_url) = update.outbound_proxy_url {
                snapshot.settings.outbound_proxy_url = outbound_proxy_url;
            }
            if let Some(outbound_no_proxy) = update.outbound_no_proxy {
                snapshot.settings.outbound_no_proxy = outbound_no_proxy;
            }
            if let Some(oidc_issuer_url) = update.oidc_issuer_url {
                snapshot.settings.oidc_issuer_url = oidc_issuer_url;
            }
            if let Some(oidc_client_id) = update.oidc_client_id {
                snapshot.settings.oidc_client_id = oidc_client_id;
            }
            if let Some(oidc_client_secret) = update.oidc_client_secret {
                snapshot.settings.oidc_client_secret = oidc_client_secret;
            }
            if let Some(oidc_scopes) = update.oidc_scopes {
                snapshot.settings.oidc_scopes = oidc_scopes;
            }
            if let Some(oidc_login_label) = update.oidc_login_label {
                snapshot.settings.oidc_login_label = oidc_login_label;
            }
            if let Some(saml_entry_point) = update.saml_entry_point {
                snapshot.settings.saml_entry_point = saml_entry_point;
            }
            if let Some(saml_issuer) = update.saml_issuer {
                snapshot.settings.saml_issuer = saml_issuer;
            }
            if let Some(saml_cert) = update.saml_cert {
                snapshot.settings.saml_cert = saml_cert;
            }
            if let Some(saml_attribute_email) = update.saml_attribute_email {
                snapshot.settings.saml_attribute_email = saml_attribute_email;
            }
            if let Some(saml_attribute_name) = update.saml_attribute_name {
                snapshot.settings.saml_attribute_name = saml_attribute_name;
            }
            if let Some(pxpipe_enabled) = update.pxpipe_enabled {
                snapshot.settings.pxpipe_enabled = pxpipe_enabled;
            }
            if let Some(pxpipe_auto_install) = update.pxpipe_auto_install {
                snapshot.settings.pxpipe_auto_install = pxpipe_auto_install;
            }
            if let Some(pxpipe_min_chars) = update.pxpipe_min_chars {
                snapshot.settings.pxpipe_min_chars = pxpipe_min_chars;
            }
            if let Some(pxpipe_timeout_ms) = update.pxpipe_timeout_ms {
                snapshot.settings.pxpipe_timeout_ms = pxpipe_timeout_ms;
            }
            snapshot.settings.clone()
        })
    }

    /// Select a connection to execute against, honoring the configured
    /// fallback strategy and per-model cooldowns.
    ///
    /// Ports `getProviderCredentials` from `src/sse/services/auth.js`. Secrets
    /// are returned unredacted, so this is loopback-internal only.
    pub(crate) fn select_connection(
        &self,
        request: &SelectConnectionRequest<'_>,
    ) -> Result<ConnectionSelection, StoreError> {
        let now_ms = now_millis();
        // Selection mutates lastUsedAt for round-robin, so it takes the write
        // lock: two concurrent requests must not pick the same sticky slot.
        self.write_snapshot(|snapshot| {
            let matching: Vec<usize> = snapshot
                .provider_connections
                .iter()
                .enumerate()
                .filter(|(_, connection)| {
                    connection.provider == request.provider && connection.is_active
                })
                .map(|(index, _)| index)
                .collect();

            if matching.is_empty() {
                return ConnectionSelection::NoCredentials;
            }

            let available: Vec<usize> = matching
                .iter()
                .copied()
                .filter(|index| {
                    snapshot
                        .provider_connections
                        .get(*index)
                        .is_some_and(|connection| {
                            !request.exclude.iter().any(|id| id == &connection.id)
                                && !connection.is_locked(request.model, now_ms)
                        })
                })
                .collect();

            if available.is_empty() {
                // Distinguish "all locked" (retryable later) from "all excluded".
                let earliest = matching
                    .iter()
                    .filter_map(|index| snapshot.provider_connections.get(*index))
                    .filter(|connection| connection.is_locked(request.model, now_ms))
                    .filter_map(|connection| connection.earliest_lock_ms(now_ms))
                    .min();
                return earliest.map_or(ConnectionSelection::Exhausted, |retry_at_ms| {
                    let last = matching
                        .iter()
                        .filter_map(|index| snapshot.provider_connections.get(*index))
                        .find(|connection| connection.is_locked(request.model, now_ms));
                    ConnectionSelection::AllRateLimited {
                        retry_at_ms,
                        last_error: last.and_then(|connection| connection.last_error.clone()),
                        last_error_code: last.and_then(|connection| connection.error_code),
                    }
                });
            }

            // A preferred connection wins over the strategy, but only if it is in
            // the available set: pinning to a locked or excluded account would
            // defeat the cooldown that locked it.
            let pinned = request.preferred.and_then(|wanted| {
                available.iter().copied().find(|index| {
                    snapshot
                        .provider_connections
                        .get(*index)
                        .is_some_and(|connection| connection.id == wanted)
                })
            });
            let chosen = if let Some(pinned) = pinned {
                pinned
            } else if request.strategy == FallbackStrategy::RoundRobin {
                Self::pick_round_robin(snapshot, &available, request.sticky_limit, &now_iso())
            } else {
                // fill-first: lowest priority number wins.
                available
                    .iter()
                    .copied()
                    .min_by_key(|index| {
                        snapshot
                            .provider_connections
                            .get(*index)
                            .map_or(u32::MAX, |connection| connection.priority)
                    })
                    .unwrap_or_default()
            };

            snapshot
                .provider_connections
                .get(chosen)
                .cloned()
                .map_or(ConnectionSelection::NoCredentials, |connection| {
                    ConnectionSelection::Selected(Box::new(connection))
                })
        })
    }

    /// Sticky round-robin: stay on the most recent account until it has been
    /// used `sticky_limit` times, then move to the least recently used.
    fn pick_round_robin(
        snapshot: &mut StateSnapshot,
        available: &[usize],
        sticky_limit: u32,
        now: &str,
    ) -> usize {
        let most_recent = available
            .iter()
            .copied()
            .filter(|index| {
                snapshot
                    .provider_connections
                    .get(*index)
                    .is_some_and(|connection| connection.last_used_at.is_some())
            })
            .max_by(|left, right| {
                let key = |index: &usize| {
                    snapshot
                        .provider_connections
                        .get(*index)
                        .and_then(|connection| connection.last_used_at.clone())
                        .unwrap_or_default()
                };
                key(left).cmp(&key(right))
            });

        let stay = most_recent.filter(|index| {
            snapshot
                .provider_connections
                .get(*index)
                .is_some_and(|connection| connection.consecutive_use_count < sticky_limit)
        });

        let chosen = stay.unwrap_or_else(|| {
            // Least recently used; never-used accounts sort first.
            available
                .iter()
                .copied()
                .min_by(|left, right| {
                    let key = |index: &usize| {
                        snapshot.provider_connections.get(*index).map_or(
                            (true, String::new(), u32::MAX),
                            |connection| {
                                (
                                    connection.last_used_at.is_some(),
                                    connection.last_used_at.clone().unwrap_or_default(),
                                    connection.priority,
                                )
                            },
                        )
                    };
                    key(left).cmp(&key(right))
                })
                .unwrap_or_default()
        });

        if let Some(connection) = snapshot.provider_connections.get_mut(chosen) {
            connection.consecutive_use_count = if stay.is_some() {
                connection.consecutive_use_count.saturating_add(1)
            } else {
                1
            };
            now.clone_into(connection.last_used_at.insert(String::new()));
        }
        chosen
    }

    /// Lock a connection after a failure (upstream `markAccountUnavailable`).
    pub(crate) fn mark_connection_unavailable(
        &self,
        update: &MarkUnavailableRequest<'_>,
    ) -> Result<bool, StoreError> {
        let now_ms = now_millis();
        self.write_snapshot(|snapshot| {
            let Some(connection) = snapshot
                .provider_connections
                .iter_mut()
                .find(|connection| connection.id == update.connection_id)
            else {
                return false;
            };

            let lock_key = update.model.unwrap_or(MODEL_LOCK_ALL);
            connection.model_locks.insert(
                lock_key.to_owned(),
                millis_to_iso(now_ms.saturating_add(update.cooldown_ms)),
            );
            connection.test_status = Some("unavailable".to_owned());
            connection.last_error = Some(update.reason.chars().take(100).collect());
            connection.error_code = Some(update.status);
            connection.last_error_at = Some(millis_to_iso(now_ms));
            if let Some(level) = update.backoff_level {
                connection.backoff_level = level;
            }
            true
        })
    }

    /// Clear locks after a success (upstream `clearAccountError`).
    ///
    /// Clears the succeeding model's lock plus any expired locks, and resets
    /// error state only when no active lock remains.
    pub(crate) fn clear_connection_error(
        &self,
        connection_id: &str,
        model: Option<&str>,
    ) -> Result<bool, StoreError> {
        let now_ms = now_millis();
        self.write_snapshot(|snapshot| {
            let Some(connection) = snapshot
                .provider_connections
                .iter_mut()
                .find(|connection| connection.id == connection_id)
            else {
                return false;
            };

            connection.model_locks.retain(|key, expiry| {
                if model.is_some_and(|model| key == model) || key == MODEL_LOCK_ALL {
                    return false;
                }
                // Drop locks that have already expired.
                iso_to_millis(expiry).is_some_and(|expiry| expiry > now_ms)
            });

            if connection.model_locks.is_empty() {
                connection.test_status = Some("active".to_owned());
                connection.last_error = None;
                connection.last_error_at = None;
                connection.error_code = None;
                connection.backoff_level = 0;
            }
            true
        })
    }

    /// Create a combo during a 9Router import, preserving its original name.
    ///
    /// Separate from `create_combo` so an import can carry the upstream `kind`
    /// and model list without going through request validation.
    pub(crate) fn create_combo_from_import(
        &self,
        name: &str,
        kind: Option<String>,
        models: Vec<String>,
    ) -> Result<Combo, StoreError> {
        self.write_snapshot(|snapshot| {
            let now = timestamp();
            let combo = Combo {
                id: next_combo_id(&snapshot.combos),
                name: name.to_owned(),
                kind,
                models,
                created_at: now.clone(),
                updated_at: now,
            };
            snapshot.combos.push(combo.clone());
            combo
        })
    }

    /// Apply settings read from a 9Router install.
    ///
    /// Only keys with a nullrouter equivalent are taken; anything else is
    /// ignored rather than guessed at.
    pub(crate) fn apply_imported_settings(
        &self,
        imported: &BTreeMap<String, Value>,
    ) -> Result<(), StoreError> {
        let boolean = |key: &str| imported.get(key).and_then(Value::as_bool);
        let text = |key: &str| imported.get(key).and_then(Value::as_str).map(str::to_owned);
        // 9Router stores this as a JSON number, but a SQLite settings row can
        // carry it as text, so both spellings are read.
        let count = |key: &str| {
            imported
                .get(key)
                .and_then(|value| {
                    value
                        .as_u64()
                        .or_else(|| value.as_str().and_then(|raw| raw.parse().ok()))
                })
                .and_then(|value| u32::try_from(value).ok())
        };

        self.write_snapshot(|snapshot| {
            // `requireLogin` is deliberately not imported: nullrouter always
            // requires dashboard login, so there is no field to import it into.
            if let Some(value) = boolean("requireApiKey") {
                snapshot.settings.require_api_key = value;
            }
            if let Some(value) = boolean("tunnelDashboardAccess") {
                snapshot.settings.tunnel_dashboard_access = value;
            }
            if let Some(value) = text("tunnelUrl") {
                snapshot.settings.tunnel_url = value;
            }
            if let Some(value) = text("tailscaleUrl") {
                snapshot.settings.tailscale_url = value;
            }
            if let Some(value) = boolean("outboundProxyEnabled") {
                snapshot.settings.outbound_proxy_enabled = value;
            }
            if let Some(value) = text("outboundProxyUrl") {
                snapshot.settings.outbound_proxy_url = value;
            }
            if let Some(value) = text("outboundNoProxy") {
                snapshot.settings.outbound_no_proxy = value;
            }
            if let Some(value) = text("fallbackStrategy") {
                snapshot.settings.fallback_strategy = value;
            }
            // Combo routing. Dropping these would silently downgrade an imported
            // round-robin combo to fallback: it would still answer, so nothing
            // would look broken while only the first model was ever used.
            if let Some(value) = text("comboStrategy") {
                snapshot.settings.combo_strategy = value;
            }
            if let Some(value) = count("comboStickyRoundRobinLimit") {
                snapshot.settings.combo_sticky_round_robin_limit = value;
            }
            if let Some(value) = count("stickyRoundRobinLimit") {
                snapshot.settings.sticky_round_robin_limit = value;
            }
            // Dashboard SSO configuration, under the same keys 9Router uses.
            for (key, target) in [
                ("oidcIssuerUrl", &mut snapshot.settings.oidc_issuer_url),
                ("oidcClientId", &mut snapshot.settings.oidc_client_id),
                (
                    "oidcClientSecret",
                    &mut snapshot.settings.oidc_client_secret,
                ),
                ("oidcScopes", &mut snapshot.settings.oidc_scopes),
                ("oidcLoginLabel", &mut snapshot.settings.oidc_login_label),
                ("samlEntryPoint", &mut snapshot.settings.saml_entry_point),
                ("samlIssuer", &mut snapshot.settings.saml_issuer),
                ("samlCert", &mut snapshot.settings.saml_cert),
                (
                    "samlAttributeEmail",
                    &mut snapshot.settings.saml_attribute_email,
                ),
                (
                    "samlAttributeName",
                    &mut snapshot.settings.saml_attribute_name,
                ),
            ] {
                if let Some(value) = text(key) {
                    *target = value;
                }
            }
            if let Some(value) = imported
                .get("stickyRoundRobinLimit")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
            {
                snapshot.settings.sticky_round_robin_limit = value;
            }
        })
    }

    /// Record a completed request for the usage dashboard.
    pub(crate) fn record_usage(
        &self,
        input: crate::usage::UsageInput,
    ) -> Result<crate::usage::UsageRecord, StoreError> {
        let now_ms = now_millis();
        self.write_snapshot(|snapshot| snapshot.usage.record(input, now_ms))
    }

    /// Aggregate usage stats.
    pub(crate) fn usage_stats(&self) -> Result<Value, StoreError> {
        let now_ms = now_millis();
        Ok(self.read_snapshot()?.usage.stats(now_ms))
    }

    /// Recent request records, newest first.
    pub(crate) fn usage_records(
        &self,
        since_ms: u64,
        limit: usize,
    ) -> Result<Vec<crate::usage::UsageRecord>, StoreError> {
        Ok(self
            .read_snapshot()?
            .usage
            .recent(since_ms, limit)
            .into_iter()
            .cloned()
            .collect())
    }

    /// Persist refreshed OAuth credentials for a connection.
    pub(crate) fn update_connection_credentials(
        &self,
        connection_id: &str,
        update: &CredentialUpdate,
    ) -> Result<bool, StoreError> {
        self.write_snapshot(|snapshot| {
            let Some(connection) = snapshot
                .provider_connections
                .iter_mut()
                .find(|connection| connection.id == connection_id)
            else {
                return false;
            };
            if update.access_token.is_some() {
                connection.access_token.clone_from(&update.access_token);
            }
            if update.refresh_token.is_some() {
                connection.refresh_token.clone_from(&update.refresh_token);
            }
            if update.expires_at.is_some() {
                connection.expires_at.clone_from(&update.expires_at);
            }
            if let Some(incoming) = update.provider_specific_data.as_ref() {
                let settings = connection
                    .provider_specific_data
                    .get_or_insert_with(BTreeMap::new);
                for (key, value) in incoming {
                    settings.insert(key.clone(), value.clone());
                }
            }
            connection.updated_at = timestamp();
            true
        })
    }

    /// Unredacted connections, for tests that must prove secrets round-tripped.
    ///
    /// Not part of the service surface: the public API always redacts.
    #[doc(hidden)]
    pub fn list_connections_for_test(&self) -> Vec<ProviderConnection> {
        self.read_snapshot()
            .map(|snapshot| snapshot.provider_connections)
            .unwrap_or_default()
    }

    pub(crate) fn read_snapshot(&self) -> Result<StateSnapshot, StoreError> {
        self.inner
            .snapshot
            .read()
            .map_err(|_| StoreError::Poisoned)
            .map(|snapshot| snapshot.clone())
    }

    pub(crate) fn write_snapshot<T>(
        &self,
        mutate: impl FnOnce(&mut StateSnapshot) -> T,
    ) -> Result<T, StoreError> {
        let (result, snapshot) = {
            let mut snapshot = self
                .inner
                .snapshot
                .write()
                .map_err(|_| StoreError::Poisoned)?;
            let result = mutate(&mut snapshot);
            (result, snapshot.clone())
        };
        self.persist(&snapshot)?;
        Ok(result)
    }

    fn persist(&self, snapshot: &StateSnapshot) -> Result<(), StoreError> {
        let Some(path) = self.inner.path.as_ref() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let payload = serde_json::to_vec_pretty(snapshot)?;
        std::fs::write(path, payload)?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ProviderConnectionInput {
    pub provider: String,
    pub auth_type: Option<String>,
    pub name: String,
    pub api_key: Option<String>,
    pub priority: Option<u32>,
    pub global_priority: Option<u32>,
    pub default_model: Option<String>,
    pub is_active: Option<bool>,
    pub test_status: Option<String>,
    pub email: Option<String>,
    pub last_error: Option<String>,
    pub last_error_at: Option<String>,
    pub provider_specific_data: Option<BTreeMap<String, Value>>,
    /// OAuth secrets, set when a connection is created from a token import.
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ProviderConnectionUpdate {
    pub name: Option<String>,
    pub api_key: Option<String>,
    pub priority: Option<u32>,
    pub global_priority: Option<u32>,
    pub default_model: Option<String>,
    pub is_active: Option<bool>,
    pub test_status: Option<String>,
    pub last_error: Option<String>,
    pub last_error_at: Option<String>,
    pub provider_specific_data: Option<BTreeMap<String, Value>>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ComboUpdate {
    pub name: Option<String>,
    pub kind: Option<String>,
    pub kind_set: bool,
    pub models: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub(crate) struct ProxyPoolInput {
    pub name: String,
    pub proxy_url: String,
    pub no_proxy: Option<String>,
    pub proxy_type: Option<String>,
    pub is_active: Option<bool>,
    pub strict_proxy: Option<bool>,
    pub test_status: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ProxyPoolUpdate {
    pub name: Option<String>,
    pub proxy_url: Option<String>,
    pub no_proxy: Option<String>,
    pub proxy_type: Option<String>,
    pub is_active: Option<bool>,
    pub strict_proxy: Option<bool>,
}

/// A settings patch: `None` means the request did not mention the field.
///
/// The distinction matters most for the two secrets. A `PUT` that omits
/// `oidcClientSecret` must leave the stored secret intact, because the dashboard
/// never reads a secret back and so can never echo one — if omission cleared it,
/// saving any other row would silently destroy the credential. An explicit `""`
/// is a different request and does clear it.
#[derive(Debug, Clone, Default)]
pub(crate) struct SettingsUpdate {
    pub tunnel_dashboard_access: Option<bool>,
    pub tunnel_url: Option<String>,
    pub tailscale_url: Option<String>,
    pub outbound_proxy_enabled: Option<bool>,
    pub outbound_proxy_url: Option<String>,
    pub outbound_no_proxy: Option<String>,
    pub oidc_issuer_url: Option<String>,
    pub oidc_client_id: Option<String>,
    pub oidc_client_secret: Option<String>,
    pub oidc_scopes: Option<String>,
    pub oidc_login_label: Option<String>,
    pub saml_entry_point: Option<String>,
    pub saml_issuer: Option<String>,
    pub saml_cert: Option<String>,
    pub saml_attribute_email: Option<String>,
    pub saml_attribute_name: Option<String>,
    pub pxpipe_enabled: Option<bool>,
    pub pxpipe_auto_install: Option<bool>,
    pub pxpipe_min_chars: Option<u64>,
    pub pxpipe_timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeleteProxyPoolResult {
    Deleted,
    NotFound,
    InUse { bound_connection_count: usize },
}

impl ProviderConnection {
    /// Strip every secret before the record crosses the public API surface.
    fn public(mut self) -> Self {
        self.api_key = None;
        self.access_token = None;
        self.refresh_token = None;
        self
    }
}

fn merge_connection(
    mut existing: ProviderConnection,
    incoming: ProviderConnection,
) -> ProviderConnection {
    existing.priority = incoming.priority;
    existing.is_active = incoming.is_active;
    existing.updated_at = incoming.updated_at;
    existing.global_priority = incoming.global_priority;
    existing.default_model = incoming.default_model;
    existing.test_status = incoming.test_status;
    existing.last_error = incoming.last_error;
    existing.last_error_at = incoming.last_error_at;
    existing.provider_specific_data = incoming.provider_specific_data;
    existing.api_key = incoming.api_key;
    existing
}

fn bound_connection_count(connections: &[ProviderConnection], proxy_pool_id: &str) -> usize {
    connections
        .iter()
        .filter(|connection| {
            connection
                .provider_specific_data
                .as_ref()
                .and_then(|data| data.get("proxyPoolId"))
                .and_then(Value::as_str)
                == Some(proxy_pool_id)
        })
        .count()
}

pub(crate) fn next_id(prefix: &str, index: usize) -> String {
    format!("{prefix}_{}_{}", current_millis(), index + 1)
}

pub(crate) fn timestamp() -> String {
    format!("unix-ms:{}", current_millis())
}

fn next_combo_id(combos: &[Combo]) -> String {
    let mut index = combos.len() + 1;
    loop {
        let candidate = format!("combo_{index}");
        if !combos.iter().any(|combo| combo.id == candidate) {
            return candidate;
        }
        index += 1;
    }
}

fn current_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis())
}
