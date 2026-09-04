//! Import a legacy router installation into this store.
//!
//! The legacy product keeps its state in `SQLite` at `$DATA_DIR/db/data.sqlite`
//! (default `~/.9router/db/data.sqlite`), with a pre-`SQLite` JSON layout at
//! `$DATA_DIR/db.json` still present on older installs. Both are read here so a
//! user switching to nullrouter keeps their providers, keys, combos, proxy
//! pools, and settings instead of reconfiguring from scratch.
//!
//! Its row shape is a hybrid: each table keeps its queryable columns plus a
//! `data` TEXT column holding the remaining fields as JSON, and its own readers
//! splat that blob over the typed columns. The import mirrors that, with typed
//! columns winning over the JSON blob.
//!
//! The import is **additive and non-destructive**: existing records are left
//! alone and duplicates are skipped, so running it twice is safe.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::store::{ProviderConnectionInput, ProxyPoolInput, StateStore, StoreError};

/// What an import found and applied.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImportReport {
    /// Absolute path the data was read from.
    pub source: String,
    /// Either `sqlite` or `json`.
    pub format: String,
    pub connections_found: usize,
    pub connections_imported: usize,
    pub combos_found: usize,
    pub combos_imported: usize,
    pub proxy_pools_found: usize,
    pub proxy_pools_imported: usize,
    pub api_keys_found: usize,
    /// API keys are hashed in nullrouter and cannot be re-derived from the
    /// legacy plaintext storage, so they are reported but not imported.
    pub api_keys_imported: usize,
    pub settings_imported: bool,
    /// Per-record problems that did not abort the import.
    pub warnings: Vec<String>,
}

/// Why an import could not run.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ImportError {
    #[error("no legacy installation found at {searched}")]
    NotFound { searched: String },
    #[error("failed to read the legacy database: {0}")]
    Read(String),
    #[error("state write failed")]
    Store(#[from] StoreError),
}

/// The legacy product's data directory under `$HOME`. This is an external
/// program's address on disk: renaming it makes every import find nothing.
const LEGACY_DATA_DIR: &str = ".9router";

/// The same directory in the Windows `%APPDATA%` layout, and the same rule.
const LEGACY_DATA_DIR_WINDOWS: &str = "9router";

/// Candidate legacy data directories, most likely first.
///
/// `DATA_DIR` is the legacy product's own override, so a user who set it there
/// gets the same answer here.
fn candidate_dirs(explicit: Option<&str>) -> Vec<PathBuf> {
    if let Some(dir) = explicit.map(str::trim).filter(|dir| !dir.is_empty()) {
        return vec![PathBuf::from(dir)];
    }
    let mut dirs = Vec::new();
    if let Ok(configured) = std::env::var("DATA_DIR")
        && !configured.trim().is_empty()
    {
        dirs.push(PathBuf::from(configured));
    }
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(Path::new(&home).join(LEGACY_DATA_DIR));
    }
    // Windows layout, harmless to probe elsewhere.
    if let Some(appdata) = std::env::var_os("APPDATA") {
        dirs.push(Path::new(&appdata).join(LEGACY_DATA_DIR_WINDOWS));
    }
    dirs
}

/// Locate a legacy data source: `SQLite` first, then the older JSON file.
fn locate(explicit: Option<&str>) -> Result<(PathBuf, &'static str), ImportError> {
    let candidates = candidate_dirs(explicit);
    for dir in &candidates {
        let sqlite = dir.join("db").join("data.sqlite");
        if sqlite.is_file() {
            return Ok((sqlite, "sqlite"));
        }
        let legacy = dir.join("db.json");
        if legacy.is_file() {
            return Ok((legacy, "json"));
        }
    }
    Err(ImportError::NotFound {
        searched: candidates
            .iter()
            .map(|dir| dir.display().to_string())
            .collect::<Vec<_>>()
            .join(", "),
    })
}

/// One legacy record: typed columns merged with its `data` JSON blob.
#[derive(Debug, Clone, Default, Deserialize)]
struct Record {
    #[serde(flatten)]
    fields: BTreeMap<String, Value>,
}

impl Record {
    fn text(&self, key: &str) -> Option<String> {
        self.fields
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    }

    fn number(&self, key: &str) -> Option<u32> {
        self.fields
            .get(key)
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
    }

    /// Booleans arrive as `SQLite` integers, so `1`/`0` has to read as a flag.
    fn flag(&self, key: &str) -> Option<bool> {
        self.fields.get(key).and_then(|value| {
            value
                .as_bool()
                .or_else(|| value.as_u64().map(|number| number != 0))
        })
    }

    fn object(&self, key: &str) -> Option<BTreeMap<String, Value>> {
        match self.fields.get(key)? {
            Value::Object(map) => Some(map.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
            // Some columns hold JSON as text.
            Value::String(text) => serde_json::from_str(text).ok(),
            _ => None,
        }
    }

    fn string_list(&self, key: &str) -> Vec<String> {
        match self.fields.get(key) {
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            Some(Value::String(text)) => {
                serde_json::from_str::<Vec<String>>(text).unwrap_or_default()
            }
            _ => Vec::new(),
        }
    }

    /// Merge the `data` JSON blob under the typed columns.
    ///
    /// Typed columns win, so a stale copy of a field inside the blob cannot
    /// override the column the source treated as authoritative.
    fn splat_data_column(&mut self) {
        let Some(blob) = self.fields.get("data").cloned() else {
            return;
        };
        let parsed = match blob {
            Value::String(text) => serde_json::from_str::<Value>(&text).ok(),
            Value::Object(_) => Some(blob),
            _ => None,
        };
        let Some(Value::Object(extra)) = parsed else {
            return;
        };
        for (key, value) in extra {
            self.fields.entry(key).or_insert(value);
        }
        self.fields.remove("data");
    }
}

/// Everything read out of a legacy installation.
#[derive(Debug, Default)]
struct Extracted {
    connections: Vec<Record>,
    combos: Vec<Record>,
    proxy_pools: Vec<Record>,
    api_keys: Vec<Record>,
    settings: Option<BTreeMap<String, Value>>,
}

/// Read a legacy `SQLite` database.
///
/// Opened read-only so an import can never disturb an installation that is
/// still running. Missing tables are tolerated: schema versions differ across
/// the releases a user might be coming from.
fn read_sqlite(path: &Path) -> Result<Extracted, ImportError> {
    use rusqlite::{Connection, OpenFlags};

    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|error| ImportError::Read(error.to_string()))?;

    // settings is a single row whose `data` column holds the whole object.
    let settings = read_table(&connection, "settings")
        .into_iter()
        .next()
        .and_then(|row| row.object("data").or_else(|| Some(row.fields.clone())));

    Ok(Extracted {
        connections: read_table(&connection, "providerConnections"),
        combos: read_table(&connection, "combos"),
        proxy_pools: read_table(&connection, "proxyPools"),
        api_keys: read_table(&connection, "apiKeys"),
        settings,
    })
}

/// Read every row of a table as generic records.
///
/// Returns empty on any error, including a missing table.
fn read_table(connection: &rusqlite::Connection, table: &str) -> Vec<Record> {
    // Table names are internal constants, never user input.
    let Ok(mut statement) = connection.prepare(&format!("SELECT * FROM {table}")) else {
        return Vec::new();
    };
    let column_names: Vec<String> = statement
        .column_names()
        .into_iter()
        .map(str::to_owned)
        .collect();

    let Ok(rows) = statement.query_map([], |row| {
        let mut fields = BTreeMap::new();
        for (index, name) in column_names.iter().enumerate() {
            let value = row
                .get_ref(index)
                .ok()
                .map_or(Value::Null, |raw| match raw {
                    rusqlite::types::ValueRef::Null => Value::Null,
                    rusqlite::types::ValueRef::Integer(number) => Value::from(number),
                    rusqlite::types::ValueRef::Real(number) => Value::from(number),
                    rusqlite::types::ValueRef::Text(bytes)
                    | rusqlite::types::ValueRef::Blob(bytes) => {
                        Value::String(String::from_utf8_lossy(bytes).into_owned())
                    }
                });
            fields.insert(name.clone(), value);
        }
        Ok(Record { fields })
    }) else {
        return Vec::new();
    };

    rows.filter_map(Result::ok)
        .map(|mut record| {
            record.splat_data_column();
            record
        })
        .collect()
}

/// Read the pre-`SQLite` `db.json` layout.
fn read_legacy_json(path: &Path) -> Result<Extracted, ImportError> {
    let bytes = std::fs::read(path).map_err(|error| ImportError::Read(error.to_string()))?;
    let root: Value =
        serde_json::from_slice(&bytes).map_err(|error| ImportError::Read(error.to_string()))?;

    // Key names varied across releases, so several spellings are accepted.
    let list = |keys: &[&str]| -> Vec<Record> {
        for key in keys {
            if let Some(Value::Array(items)) = root.get(*key) {
                return items
                    .iter()
                    .filter_map(|item| serde_json::from_value::<Record>(item.clone()).ok())
                    .map(|mut record| {
                        record.splat_data_column();
                        record
                    })
                    .collect();
            }
        }
        Vec::new()
    };

    Ok(Extracted {
        connections: list(&["providerConnections", "connections"]),
        combos: list(&["combos"]),
        proxy_pools: list(&["proxyPools", "proxy_pools"]),
        api_keys: list(&["apiKeys", "api_keys"]),
        settings: root.get("settings").and_then(|settings| match settings {
            Value::Object(map) => Some(map.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
            _ => None,
        }),
    })
}

/// Import a legacy installation into this store.
///
/// `explicit_dir` overrides discovery. `dry_run` reports what would be imported
/// without writing, so a user can preview before committing.
#[allow(
    clippy::too_many_lines,
    reason = "one linear import over four record kinds plus settings; splitting it would scatter the shared report"
)]
pub(crate) fn import(
    store: &StateStore,
    explicit_dir: Option<&str>,
    dry_run: bool,
) -> Result<ImportReport, ImportError> {
    let (path, format) = locate(explicit_dir)?;
    let extracted = if format == "sqlite" {
        read_sqlite(&path)?
    } else {
        read_legacy_json(&path)?
    };

    let mut report = ImportReport {
        source: path.display().to_string(),
        format: format.to_owned(),
        connections_found: extracted.connections.len(),
        combos_found: extracted.combos.len(),
        proxy_pools_found: extracted.proxy_pools.len(),
        api_keys_found: extracted.api_keys.len(),
        ..ImportReport::default()
    };

    if !extracted.api_keys.is_empty() {
        // nullrouter stores only a digest of each key, so a plaintext key
        // cannot be turned into a usable record without re-issuing it.
        report.warnings.push(format!(
            "{} API key(s) found but not imported: nullrouter stores key digests, \
             so existing keys cannot be re-derived. Re-issue them from the dashboard.",
            extracted.api_keys.len()
        ));
    }

    // Existing records are never overwritten; a name collision is skipped.
    let existing_connections = store.list_connections().unwrap_or_default();
    let existing_pools = store.list_proxy_pools(None, false).unwrap_or_default();
    let existing_combos = store.list_combos().unwrap_or_default();

    for record in &extracted.connections {
        let Some(provider) = record.text("provider") else {
            report
                .warnings
                .push("skipped a connection with no provider".to_owned());
            continue;
        };
        let name = record
            .text("name")
            .or_else(|| record.text("displayName"))
            .or_else(|| record.text("email"))
            .unwrap_or_else(|| provider.clone());

        let already_present = existing_connections
            .iter()
            .any(|existing| existing.provider == provider && existing.name == name);
        if already_present {
            report
                .warnings
                .push(format!("skipped existing connection {provider}/{name}"));
            continue;
        }

        if dry_run {
            report.connections_imported += 1;
            continue;
        }

        let input = ProviderConnectionInput {
            provider: provider.clone(),
            auth_type: record.text("authType"),
            name,
            api_key: record.text("apiKey"),
            priority: record.number("priority"),
            global_priority: record.number("globalPriority"),
            default_model: record.text("defaultModel"),
            is_active: record.flag("isActive"),
            test_status: record.text("testStatus"),
            email: record.text("email"),
            last_error: None,
            last_error_at: None,
            provider_specific_data: record.object("providerSpecificData"),
            access_token: record.text("accessToken"),
            refresh_token: record.text("refreshToken"),
            expires_at: record.text("expiresAt"),
        };
        match store.create_connection(input) {
            Ok(_) => report.connections_imported += 1,
            Err(_) => report
                .warnings
                .push(format!("failed to write connection for {provider}")),
        }
    }

    for record in &extracted.proxy_pools {
        let Some(name) = record.text("name") else {
            report
                .warnings
                .push("skipped a proxy pool with no name".to_owned());
            continue;
        };
        let pool_exists = existing_pools
            .iter()
            .any(|existing| existing.get("name").and_then(Value::as_str) == Some(name.as_str()));
        if pool_exists {
            report
                .warnings
                .push(format!("skipped existing proxy pool {name}"));
            continue;
        }
        if dry_run {
            report.proxy_pools_imported += 1;
            continue;
        }
        let input = ProxyPoolInput {
            name: name.clone(),
            proxy_url: record.text("proxyUrl").unwrap_or_default(),
            no_proxy: record.text("noProxy"),
            proxy_type: record.text("type"),
            is_active: record.flag("isActive"),
            strict_proxy: record.flag("strictProxy"),
            test_status: record.text("testStatus"),
        };
        match store.create_proxy_pool(input) {
            Ok(_) => report.proxy_pools_imported += 1,
            Err(_) => report
                .warnings
                .push(format!("failed to write proxy pool {name}")),
        }
    }

    for record in &extracted.combos {
        let Some(name) = record.text("name") else {
            report
                .warnings
                .push("skipped a combo with no name".to_owned());
            continue;
        };
        if existing_combos.iter().any(|existing| existing.name == name) {
            report
                .warnings
                .push(format!("skipped existing combo {name}"));
            continue;
        }
        let models = record.string_list("models");
        if models.is_empty() {
            report
                .warnings
                .push(format!("skipped combo {name} with no models"));
            continue;
        }
        if dry_run {
            report.combos_imported += 1;
            continue;
        }
        match store.create_combo_from_import(&name, record.text("kind"), models) {
            Ok(_) => report.combos_imported += 1,
            Err(_) => report
                .warnings
                .push(format!("failed to write combo {name}")),
        }
    }

    if let Some(settings) = &extracted.settings {
        if dry_run {
            report.settings_imported = true;
        } else {
            match store.apply_imported_settings(settings) {
                Ok(()) => report.settings_imported = true,
                Err(_) => report
                    .warnings
                    .push("failed to write imported settings".to_owned()),
            }
        }
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::{Record, candidate_dirs, locate, read_legacy_json};
    use serde_json::json;
    use std::collections::BTreeMap;

    fn record(fields: serde_json::Value) -> Record {
        let map: BTreeMap<String, serde_json::Value> =
            serde_json::from_value(fields).expect("object");
        Record { fields: map }
    }

    #[test]
    fn data_column_splats_under_typed_columns() {
        // A typed column wins over the same key inside the JSON blob, because
        // the column is the one the source kept queryable and current.
        let mut row = record(json!({
            "id": "conn_1",
            "provider": "openai",
            "data": "{\"apiKey\":\"sk-x\",\"provider\":\"SHOULD-NOT-WIN\"}",
        }));
        row.splat_data_column();

        assert_eq!(row.text("provider").as_deref(), Some("openai"));
        assert_eq!(row.text("apiKey").as_deref(), Some("sk-x"));
        // The blob itself is consumed.
        assert!(!row.fields.contains_key("data"));
    }

    #[test]
    fn sqlite_integer_booleans_are_understood() {
        let row = record(json!({ "isActive": 1, "strictProxy": 0, "native": true }));
        assert_eq!(row.flag("isActive"), Some(true));
        assert_eq!(row.flag("strictProxy"), Some(false));
        assert_eq!(row.flag("native"), Some(true));
        assert_eq!(row.flag("absent"), None);
    }

    #[test]
    fn models_parse_from_array_or_json_text() {
        let as_array = record(json!({ "models": ["a/b", "c/d"] }));
        assert_eq!(as_array.string_list("models"), vec!["a/b", "c/d"]);

        // SQLite stores it as TEXT.
        let as_text = record(json!({ "models": "[\"a/b\",\"c/d\"]" }));
        assert_eq!(as_text.string_list("models"), vec!["a/b", "c/d"]);

        assert!(record(json!({})).string_list("models").is_empty());
    }

    #[test]
    fn blank_strings_are_treated_as_absent() {
        let row = record(json!({ "name": "   ", "email": "a@b.test" }));
        assert_eq!(row.text("name"), None);
        assert_eq!(row.text("email").as_deref(), Some("a@b.test"));
    }

    #[test]
    fn discovery_prefers_an_explicit_directory() {
        let dirs = candidate_dirs(Some("/custom/path"));
        assert_eq!(dirs.len(), 1);
        assert_eq!(
            dirs.first().map(|dir| dir.display().to_string()),
            Some("/custom/path".to_owned())
        );
    }

    #[test]
    fn discovery_falls_back_to_the_home_layout() {
        let dirs = candidate_dirs(None);
        assert!(
            dirs.iter().any(|dir| dir.ends_with(super::LEGACY_DATA_DIR)),
            "expected the default ~/{} layout in {dirs:?}",
            super::LEGACY_DATA_DIR
        );
    }

    #[test]
    fn missing_installation_reports_what_was_searched() {
        let error = locate(Some("/nonexistent/import/path")).expect_err("must not find");
        let message = error.to_string();
        assert!(
            message.contains("/nonexistent/import/path"),
            "got {message}"
        );
    }

    #[test]
    fn legacy_json_reads_connections_and_combos() {
        let dir = std::env::temp_dir().join(format!("nr-import-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("db.json");
        std::fs::write(
            &path,
            json!({
                "providerConnections": [
                    { "id": "c1", "provider": "openai", "name": "mine", "apiKey": "sk-1", "isActive": 1 },
                ],
                "combos": [{ "id": "k1", "name": "combo", "models": ["openai/gpt-5"] }],
                "settings": { "requireLogin": false },
            })
            .to_string(),
        )
        .expect("write");

        let extracted = read_legacy_json(&path).expect("reads");
        assert_eq!(extracted.connections.len(), 1);
        assert_eq!(
            extracted
                .connections
                .first()
                .and_then(|row| row.text("provider"))
                .as_deref(),
            Some("openai")
        );
        assert_eq!(extracted.combos.len(), 1);
        assert!(extracted.settings.is_some());

        std::fs::remove_dir_all(&dir).ok();
    }
}
