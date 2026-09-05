//! Wire shapes the dashboard reads. Fields are the server's; unknown extras are ignored.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeyRow {
    pub id: String,
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub machine_id: String,
    #[serde(default)]
    pub is_active: bool,
    #[serde(default)]
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct KeysList {
    #[serde(default)]
    pub keys: Vec<ApiKeyRow>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CreatedKey {
    pub key: ApiKeyRow,
}

#[derive(Debug, Serialize)]
pub struct CreateKeyBody<'a> {
    pub name: &'a str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateKeyBody {
    pub is_active: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRow {
    pub id: String,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub auth_type: String,
    #[serde(default)]
    pub is_active: bool,
    #[serde(default)]
    pub test_status: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ProvidersList {
    #[serde(default)]
    pub connections: Vec<ProviderRow>,
}

#[derive(Clone, Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UsageStats {
    #[serde(default)]
    pub total_requests: u64,
    #[serde(default)]
    pub total_prompt_tokens: u64,
    #[serde(default)]
    pub total_completion_tokens: u64,
    #[serde(default)]
    pub total_cached_tokens: u64,
    #[serde(default)]
    pub total_cost: u64,
}

#[derive(Clone, Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UsageLive {
    #[serde(default)]
    pub live_telemetry: bool,
    #[serde(default)]
    pub active_requests: u64,
    #[serde(default)]
    pub requests_today: u64,
    #[serde(default)]
    pub tokens_today: u64,
    #[serde(default)]
    pub estimated_cost: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    #[serde(default)]
    pub require_api_key: bool,
    #[serde(default)]
    pub tunnel_dashboard_access: bool,
    #[serde(default)]
    pub tunnel_url: String,
    #[serde(default)]
    pub tailscale_url: String,
    #[serde(default)]
    pub outbound_proxy_enabled: bool,
    #[serde(default)]
    pub outbound_proxy_url: String,
    #[serde(default)]
    pub outbound_no_proxy: String,
    #[serde(default)]
    pub oidc_issuer_url: String,
    #[serde(default)]
    pub oidc_client_id: String,
    #[serde(default)]
    pub oidc_client_secret_set: bool,
    #[serde(default)]
    pub oidc_scopes: String,
    #[serde(default)]
    pub oidc_login_label: String,
    #[serde(default)]
    pub pxpipe_enabled: bool,
}

#[derive(Clone, Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    #[serde(default)]
    pub authenticated: bool,
    #[serde(default)]
    pub require_login: bool,
    #[serde(default)]
    pub has_password: bool,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub login_method: String,
}

#[derive(Clone, Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LoginSuccess {
    #[serde(default)]
    pub success: bool,
}

#[derive(Clone, Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LoginDenied {
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub remaining_before_lock: u32,
}

#[derive(Debug, Serialize)]
pub struct LoginBody<'a> {
    pub password: &'a str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub require_api_key: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_dashboard_access: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outbound_proxy_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pxpipe_enabled: Option<bool>,
}

/// What a model can do, as the catalogue reports it.
#[derive(Clone, Copy, Debug, Default, Deserialize)]
pub struct ModelCaps {
    #[serde(default)]
    pub vision: bool,
    #[serde(default)]
    pub search: bool,
    #[serde(default)]
    pub reasoning: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRow {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub model: String,
    /// `provider/model`, which is the name a request has to use.
    #[serde(default)]
    pub full_model: String,
    #[serde(default)]
    pub alias: String,
    #[serde(default)]
    pub caps: ModelCaps,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ModelsList {
    #[serde(default)]
    pub models: Vec<ModelRow>,
}

/// One model the router is holding back, and why.
///
/// Only `provider` and `model` are read. Those two are the pair `POST /api/models/availability`
/// takes to clear a cooldown, so they are the entry's identity; anything else the row carries is
/// left alone rather than guessed at, because this build always answers with an empty list and
/// there is no observed row to model the rest of the shape on.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct AvailabilityRow {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub model: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Availability {
    #[serde(default)]
    pub models: Vec<AvailabilityRow>,
    #[serde(default)]
    pub unavailable_count: u32,
}

/// A model added by hand rather than discovered from a provider.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomModel {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub provider_alias: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, rename = "type")]
    pub model_type: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct CustomModels {
    #[serde(default)]
    pub models: Vec<CustomModel>,
}

/// Models switched off, keyed by provider alias.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct DisabledModels {
    #[serde(default)]
    pub disabled: std::collections::BTreeMap<String, Vec<String>>,
}

/// The outcome of `POST /api/models/test`.
///
/// The route answers `200` even when the model did not work, with `ok` carrying the verdict, so
/// this is decoded on success rather than treated as a failed request. `error` is the provider's
/// own wording, which is the part worth showing.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelTestResult {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub latency_ms: u64,
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ModelTestBody<'a> {
    pub model: &'a str,
    pub kind: &'a str,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComboRow {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// Absent on combos stored without one, so not flattened to a string.
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct CombosList {
    #[serde(default)]
    pub combos: Vec<ComboRow>,
}

/// A combo write. `kind` is omitted when empty so a `PUT` does not clear it by accident: the state
/// service treats the key being *present* as "set this", including to null.
#[derive(Debug, Serialize)]
pub struct ComboBody {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    pub models: Vec<String>,
}

/// The per-model prices the router bills against.
///
/// Every field is optional because the catalogue only carries what a provider published: a model
/// priced for input and output but not for cached reads has three of these absent, and a zero there
/// would be a claim that reading cache is free.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct PriceFields {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation: Option<f64>,
}

/// One priced model, flattened out of the two-level map the endpoint returns.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PriceRow {
    pub provider: String,
    pub model: String,
    pub fields: PriceFields,
}

pub fn display_name(entry: &nullrouter_providers::RegistryEntry) -> String {
    entry
        .display
        .as_ref()
        .and_then(|display| display.name.clone())
        .unwrap_or_else(|| entry.id.clone())
}

/// Flatten `GET /api/pricing` into rows.
///
/// The endpoint answers a two-level map keyed by provider then model, with no envelope. Ordering
/// comes from `BTreeMap`, so the table is stable across reloads instead of reshuffling on every
/// fetch the way a `HashMap` would.
pub fn price_rows(body: &str) -> Result<Vec<PriceRow>, crate::api::ApiError> {
    type Catalog =
        std::collections::BTreeMap<String, std::collections::BTreeMap<String, PriceFields>>;

    let catalog: Catalog = crate::api::decode(body)?;
    Ok(catalog
        .into_iter()
        .flat_map(|(provider, models)| {
            models.into_iter().map(move |(model, fields)| PriceRow {
                provider: provider.clone(),
                model,
                fields,
            })
        })
        .collect())
}

/// A store timestamp, rendered for display.
///
/// The state service writes `unix-ms:1788527311412` rather than an ISO string, so showing the field
/// verbatim would put that prefix on screen. Anything that does not parse is passed through
/// unchanged: an older build's bare integer or a future ISO string is still more informative than a
/// blank cell.
pub fn timestamp_label(value: &str) -> String {
    value
        .strip_prefix("unix-ms:")
        .map_or_else(|| value.to_owned(), str::to_owned)
}

/// Whether the state service will accept this combo name.
///
/// Mirrors `is_valid_combo_name` in `services/state-actix`. Checked here so the common typo is
/// caught before a round trip, not to replace the server's check: the duplicate-name rule needs the
/// stored set, so a refusal still has to be surfaced.
pub fn is_valid_combo_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

#[cfg(test)]
mod tests {
    use super::{
        Availability, CombosList, CustomModels, DisabledModels, ModelTestResult, ModelsList,
        is_valid_combo_name, price_rows, timestamp_label,
    };

    /// `GET /api/models`, captured from the running router.
    const MODELS_BODY: &str = r#"{"models":[
        {"provider":"openai","model":"gpt-5","fullModel":"openai/gpt-5","alias":"gpt-5",
         "caps":{"vision":false,"search":false,"reasoning":false}},
        {"provider":"anthropic","model":"claude-sonnet-4.5",
         "fullModel":"anthropic/claude-sonnet-4.5","alias":"claude-sonnet-4.5",
         "caps":{"vision":true,"search":false,"reasoning":true}}
    ]}"#;

    #[test]
    fn the_live_model_list_decodes_with_its_capabilities() {
        let parsed: ModelsList = serde_json::from_str(MODELS_BODY).expect("decodes");
        assert_eq!(parsed.models.len(), 2);
        let second = parsed.models.get(1).expect("two rows");
        // `fullModel` is the name a request has to use, so a rename upstream must not go unnoticed.
        assert_eq!(second.full_model, "anthropic/claude-sonnet-4.5");
        assert!(second.caps.vision);
        assert!(second.caps.reasoning);
        assert!(!second.caps.search);
    }

    #[test]
    fn an_empty_availability_response_is_not_read_as_a_cooldown() {
        let parsed: Availability =
            serde_json::from_str(r#"{"models":[],"unavailableCount":0}"#).expect("decodes");
        assert!(parsed.models.is_empty());
        assert_eq!(parsed.unavailable_count, 0);
    }

    #[test]
    fn an_availability_row_keeps_the_pair_that_identifies_it() {
        let parsed: Availability = serde_json::from_str(
            r#"{"models":[{"provider":"openai","model":"gpt-5","until":"soon"}],
                "unavailableCount":1}"#,
        )
        .expect("decodes");
        assert_eq!(parsed.unavailable_count, 1);
        let row = parsed.models.first().expect("one row");
        assert_eq!(row.provider, "openai");
        assert_eq!(row.model, "gpt-5");
    }

    #[test]
    fn the_empty_custom_and_disabled_responses_decode() {
        let custom: CustomModels = serde_json::from_str(r#"{"models":[]}"#).expect("decodes");
        assert!(custom.models.is_empty());
        let disabled: DisabledModels = serde_json::from_str(r#"{"disabled":{}}"#).expect("decodes");
        assert!(disabled.disabled.is_empty());
    }

    #[test]
    fn a_populated_disabled_map_keeps_its_provider_keys() {
        let parsed: DisabledModels =
            serde_json::from_str(r#"{"disabled":{"openai":["gpt-4","gpt-4o"]}}"#).expect("decodes");
        assert_eq!(
            parsed.disabled.get("openai").map(Vec::len),
            Some(2),
            "the ids must stay grouped under their provider"
        );
    }

    #[test]
    fn a_failed_model_test_is_not_read_as_a_pass() {
        // The route answers 200 with `ok: false`, so treating a 200 as success would report a
        // model with no credentials as working.
        let body = r#"{"error":"No active credentials for provider: openai","kind":"llm",
                       "latencyMs":4,"model":"openai/gpt-5","ok":false,"status":404}"#;
        let parsed: ModelTestResult = serde_json::from_str(body).expect("decodes");
        assert!(!parsed.ok);
        assert_eq!(parsed.error, "No active credentials for provider: openai");
        assert_eq!(parsed.latency_ms, 4);
    }

    #[test]
    fn a_passing_model_test_carries_its_finish_reason() {
        let body = r#"{"ok":true,"model":"openai/gpt-5","kind":"llm","latencyMs":812,
                       "finishReason":"length","usage":{"total_tokens":9}}"#;
        let parsed: ModelTestResult = serde_json::from_str(body).expect("decodes");
        assert!(parsed.ok);
        assert_eq!(parsed.finish_reason.as_deref(), Some("length"));
        assert!(parsed.error.is_empty());
    }

    /// `GET /api/combos` after creating one, captured from the running router.
    const COMBOS_BODY: &str = r#"{"combos":[{"id":"combo_1","name":"probe-combo","kind":"llm",
        "models":["openai/gpt-5"],"createdAt":"unix-ms:1788527311412",
        "updatedAt":"unix-ms:1788527311412"}]}"#;

    #[test]
    fn the_live_combo_list_decodes_with_its_models() {
        let parsed: CombosList = serde_json::from_str(COMBOS_BODY).expect("decodes");
        let combo = parsed.combos.first().expect("one combo");
        assert_eq!(combo.id, "combo_1");
        assert_eq!(combo.models, vec!["openai/gpt-5".to_owned()]);
        assert_eq!(combo.kind.as_deref(), Some("llm"));
    }

    #[test]
    fn a_combo_without_a_kind_stays_absent_rather_than_empty() {
        // A combo stored with `kind: null` must not read as one whose kind is the empty string.
        let parsed: CombosList = serde_json::from_str(
            r#"{"combos":[{"id":"combo_2","name":"x","kind":null,"models":[]}]}"#,
        )
        .expect("decodes");
        assert_eq!(parsed.combos.first().and_then(|c| c.kind.clone()), None);
    }

    /// `GET /api/pricing`, captured from the running router. No envelope, two levels of map.
    const PRICING_BODY: &str = r#"{
        "gh":{"gpt-5.3-codex":{"cache_creation":1.75,"cached":0.175,"input":1.75,
                               "output":14.0,"reasoning":14.0}},
        "openai":{"gpt-5":{"input":2.5,"output":10}}
    }"#;

    #[test]
    fn the_live_pricing_body_flattens_into_ordered_rows() {
        let rows = price_rows(PRICING_BODY).expect("decodes");
        assert_eq!(rows.len(), 2);
        // BTreeMap ordering: `gh` before `openai`, so the table does not reshuffle per fetch.
        let first = rows.first().expect("two rows");
        assert_eq!(first.provider, "gh");
        assert_eq!(first.model, "gpt-5.3-codex");
        assert_eq!(first.fields.input, Some(1.75));
        assert_eq!(first.fields.cache_creation, Some(1.75));
    }

    #[test]
    fn a_partially_priced_model_leaves_the_absent_fields_absent() {
        // Defaulting these to 0.0 would claim cached reads are free.
        let rows =
            price_rows(r#"{"openai":{"gpt-5":{"input":2.5,"output":10}}}"#).expect("decodes");
        let row = rows.first().expect("one row");
        assert_eq!(row.fields.input, Some(2.5));
        assert_eq!(row.fields.cached, None);
        assert_eq!(row.fields.reasoning, None);
    }

    #[test]
    fn an_empty_pricing_catalogue_is_no_rows_rather_than_an_error() {
        assert!(price_rows("{}").expect("decodes").is_empty());
    }

    #[test]
    fn a_pricing_shape_change_becomes_a_visible_failure() {
        // Not an empty table, which would read as "nothing is priced".
        assert!(price_rows(r#"{"openai":["gpt-5"]}"#).is_err());
        assert!(price_rows("truncated").is_err());
    }

    #[test]
    fn a_store_timestamp_loses_its_prefix_for_display() {
        assert_eq!(timestamp_label("unix-ms:1788527311412"), "1788527311412");
    }

    #[test]
    fn an_unprefixed_timestamp_is_shown_as_it_arrived() {
        // An older build's bare integer, or an ISO string, is still worth showing.
        assert_eq!(timestamp_label("1788527311412"), "1788527311412");
        assert_eq!(
            timestamp_label("2026-09-04T00:00:00Z"),
            "2026-09-04T00:00:00Z"
        );
        assert_eq!(timestamp_label(""), "");
    }

    #[test]
    fn combo_names_follow_the_state_services_rule() {
        for name in ["combo", "fast-3", "a_b.c", "ABC123"] {
            assert!(is_valid_combo_name(name), "{name} should be accepted");
        }
        for name in ["", "has space", "slash/es", "bang!", "über"] {
            assert!(!is_valid_combo_name(name), "{name} should be refused");
        }
    }
}
