//! Importing a real 9Router `SQLite` database.
//!
//! The fixture is built with 9Router's actual schema from
//! `inspire/src/lib/db/schema.js`, including the `data` TEXT column that holds
//! everything outside the queryable columns. Reading a hand-shaped JSON blob
//! instead would not prove the importer works against a real installation.

#![allow(
    clippy::future_not_send,
    clippy::too_many_lines,
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "test setup: a failed fixture write should abort the test"
)]

use actix_web::{App, body::to_bytes, http::header, test, web};
use nullrouter_state::{StateStore, configure};
use serde_json::{Value, json};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A scratch directory laid out like a 9Router install.
struct Fixture {
    dir: std::path::PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "nr-9router-{tag}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(dir.join("db")).expect("fixture dirs");
        Self { dir }
    }

    fn path(&self) -> String {
        self.dir.display().to_string()
    }

    /// Write a `SQLite` database using 9Router's real schema.
    fn write_sqlite(&self) {
        let db = self.dir.join("db").join("data.sqlite");
        let connection = rusqlite::Connection::open(&db).expect("open fixture db");

        connection
            .execute_batch(
                "CREATE TABLE settings (id INTEGER PRIMARY KEY CHECK (id = 1), data TEXT NOT NULL);
                 CREATE TABLE providerConnections (
                    id TEXT PRIMARY KEY, provider TEXT NOT NULL, authType TEXT NOT NULL,
                    name TEXT, email TEXT, priority INTEGER, isActive INTEGER DEFAULT 1,
                    data TEXT NOT NULL, createdAt TEXT NOT NULL, updatedAt TEXT NOT NULL);
                 CREATE TABLE proxyPools (
                    id TEXT PRIMARY KEY, isActive INTEGER DEFAULT 1, testStatus TEXT,
                    data TEXT NOT NULL, createdAt TEXT NOT NULL, updatedAt TEXT NOT NULL);
                 CREATE TABLE apiKeys (
                    id TEXT PRIMARY KEY, key TEXT UNIQUE NOT NULL, name TEXT,
                    machineId TEXT, isActive INTEGER DEFAULT 1, createdAt TEXT NOT NULL);
                 CREATE TABLE combos (
                    id TEXT PRIMARY KEY, name TEXT UNIQUE NOT NULL, kind TEXT,
                    models TEXT NOT NULL, createdAt TEXT NOT NULL, updatedAt TEXT NOT NULL);",
            )
            .expect("create schema");

        // Secrets and per-connection config live in the `data` blob, exactly as
        // 9Router writes them.
        connection
            .execute(
                "INSERT INTO providerConnections
                 (id, provider, authType, name, email, priority, isActive, data, createdAt, updatedAt)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                rusqlite::params![
                    "conn_1",
                    "openai",
                    "apikey",
                    "my-openai",
                    "user@example.test",
                    1,
                    1,
                    json!({
                        "apiKey": "sk-imported-1",
                        "defaultModel": "gpt-5",
                        "providerSpecificData": { "baseUrl": "https://proxy.example.test/v1" },
                    })
                    .to_string(),
                    "2026-01-01T00:00:00Z",
                    "2026-01-01T00:00:00Z",
                ],
            )
            .expect("insert connection");
        connection
            .execute(
                "INSERT INTO providerConnections
                 (id, provider, authType, name, email, priority, isActive, data, createdAt, updatedAt)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                rusqlite::params![
                    "conn_2",
                    "anthropic",
                    "oauth",
                    "my-claude",
                    Option::<String>::None,
                    2,
                    0,
                    json!({ "accessToken": "tok-abc", "refreshToken": "ref-abc" }).to_string(),
                    "2026-01-02T00:00:00Z",
                    "2026-01-02T00:00:00Z",
                ],
            )
            .expect("insert oauth connection");

        connection
            .execute(
                "INSERT INTO proxyPools (id, isActive, testStatus, data, createdAt, updatedAt)
                 VALUES (?1,?2,?3,?4,?5,?6)",
                rusqlite::params![
                    "pool_1",
                    1,
                    "active",
                    json!({
                        "name": "office",
                        "proxyUrl": "http://127.0.0.1:7897",
                        "noProxy": "localhost",
                        "type": "http",
                        "strictProxy": true,
                    })
                    .to_string(),
                    "2026-01-01T00:00:00Z",
                    "2026-01-01T00:00:00Z",
                ],
            )
            .expect("insert pool");

        connection
            .execute(
                "INSERT INTO combos (id, name, kind, models, createdAt, updatedAt)
                 VALUES (?1,?2,?3,?4,?5,?6)",
                rusqlite::params![
                    "combo_1",
                    "my-combo",
                    Option::<String>::None,
                    // 9Router stores this as JSON text.
                    json!(["openai/gpt-5", "anthropic/claude-sonnet-4.5"]).to_string(),
                    "2026-01-01T00:00:00Z",
                    "2026-01-01T00:00:00Z",
                ],
            )
            .expect("insert combo");

        connection
            .execute(
                "INSERT INTO apiKeys (id, key, name, machineId, isActive, createdAt)
                 VALUES (?1,?2,?3,?4,?5,?6)",
                rusqlite::params![
                    "key_1",
                    "sk_9router_plain",
                    "cli",
                    "m1",
                    1,
                    "2026-01-01T00:00:00Z"
                ],
            )
            .expect("insert key");

        connection
            .execute(
                "INSERT INTO settings (id, data) VALUES (1, ?1)",
                rusqlite::params![
                    json!({
                        "requireLogin": false,
                        "requireApiKey": true,
                        "fallbackStrategy": "round-robin",
                        "stickyRoundRobinLimit": 7,
                        "comboStrategy": "round-robin",
                        "comboStickyRoundRobinLimit": 4,
                        "outboundProxyEnabled": true,
                        "outboundProxyUrl": "http://127.0.0.1:8888",
                    })
                    .to_string()
                ],
            )
            .expect("insert settings");
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

/// POST the import endpoint against a fresh in-memory store.
async fn run_import(store: StateStore, body: Value) -> TestResult<(u16, Value)> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(store))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::post()
        .uri("/internal/v1/migrate/9router")
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(body.to_string())
        .to_request();
    let res = test::call_service(&app, req).await;
    let status = res.status().as_u16();
    let bytes = to_bytes(res.into_body()).await?;
    Ok((status, serde_json::from_slice(&bytes)?))
}

#[actix_web::test]
async fn sqlite_install_imports_connections_pools_combos_and_settings() -> TestResult {
    let fixture = Fixture::new("full");
    fixture.write_sqlite();
    let store = StateStore::memory();

    let (status, body) = run_import(
        store.clone(),
        json!({ "dataDir": fixture.path(), "dryRun": false }),
    )
    .await?;

    assert_eq!(status, 200, "body: {body}");
    assert_eq!(body.get("ok"), Some(&json!(true)));
    assert_eq!(body.pointer("/report/format"), Some(&json!("sqlite")));
    assert_eq!(body.pointer("/report/connectionsFound"), Some(&json!(2)));
    assert_eq!(body.pointer("/report/connectionsImported"), Some(&json!(2)));
    assert_eq!(body.pointer("/report/proxyPoolsImported"), Some(&json!(1)));
    assert_eq!(body.pointer("/report/combosImported"), Some(&json!(1)));
    assert_eq!(body.pointer("/report/settingsImported"), Some(&json!(true)));

    // API keys are found but deliberately not imported: nullrouter stores
    // digests, so a plaintext key cannot be turned into a usable record.
    assert_eq!(body.pointer("/report/apiKeysFound"), Some(&json!(1)));
    assert_eq!(body.pointer("/report/apiKeysImported"), Some(&json!(0)));
    let warnings = body
        .pointer("/report/warnings")
        .and_then(Value::as_array)
        .expect("warnings");
    assert!(
        warnings
            .iter()
            .filter_map(Value::as_str)
            .any(|warning| warning.contains("API key")),
        "the key limitation must be reported, got {warnings:?}"
    );
    Ok(())
}

#[actix_web::test]
async fn imported_connection_carries_secrets_from_the_data_blob() -> TestResult {
    let fixture = Fixture::new("secrets");
    fixture.write_sqlite();
    let store = StateStore::memory();

    let (status, _) = run_import(
        store.clone(),
        json!({ "dataDir": fixture.path(), "dryRun": false }),
    )
    .await?;
    assert_eq!(status, 200);

    // Read back through the credential-selection path, which is what the
    // runtime uses — this proves the imported connection is actually usable.
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(store))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::post()
        .uri("/internal/v1/credentials/select")
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(json!({ "provider": "openai", "model": "gpt-5" }).to_string())
        .to_request();
    let res = test::call_service(&app, req).await;
    let body: Value = serde_json::from_slice(&to_bytes(res.into_body()).await?)?;

    assert_eq!(body.get("status"), Some(&json!("selected")), "body: {body}");
    // The API key came out of the `data` blob, not a typed column.
    assert_eq!(
        body.pointer("/credentials/apiKey"),
        Some(&json!("sk-imported-1"))
    );
    // So did the per-connection base URL.
    assert_eq!(
        body.pointer("/credentials/providerSpecificData/baseUrl"),
        Some(&json!("https://proxy.example.test/v1"))
    );
    Ok(())
}

#[actix_web::test]
async fn oauth_tokens_survive_the_import() -> TestResult {
    let fixture = Fixture::new("oauth");
    fixture.write_sqlite();
    let store = StateStore::memory();
    run_import(
        store.clone(),
        json!({ "dataDir": fixture.path(), "dryRun": false }),
    )
    .await?;

    // conn_2 is inactive in the fixture, so selection would skip it; read the
    // snapshot through the public list and confirm it landed.
    let connections = store.list_connections_for_test();
    let claude = connections
        .iter()
        .find(|connection| connection.provider == "anthropic")
        .expect("anthropic connection imported");
    assert_eq!(claude.access_token.as_deref(), Some("tok-abc"));
    assert_eq!(claude.refresh_token.as_deref(), Some("ref-abc"));
    // isActive=0 is preserved rather than silently enabling the account.
    assert!(!claude.is_active, "inactive state must carry over");
    Ok(())
}

#[actix_web::test]
async fn imported_settings_reach_the_routing_context() -> TestResult {
    let fixture = Fixture::new("settings");
    fixture.write_sqlite();
    let store = StateStore::memory();
    run_import(
        store.clone(),
        json!({ "dataDir": fixture.path(), "dryRun": false }),
    )
    .await?;

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(store))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::get()
        .uri("/internal/v1/routing-context")
        .to_request();
    let res = test::call_service(&app, req).await;
    let body: Value = serde_json::from_slice(&to_bytes(res.into_body()).await?)?;

    // requireApiKey and the fallback strategy are behavioral: importing them
    // wrong would silently change how the router runs.
    assert_eq!(
        body.pointer("/settings/requireApiKey"),
        Some(&json!(true)),
        "body: {body}"
    );
    assert_eq!(
        body.pointer("/settings/fallbackStrategy"),
        Some(&json!("round-robin"))
    );
    // Combo routing is behavioral in the same way: dropping it would leave an
    // imported round-robin combo answering from only its first model, with
    // nothing to indicate the strategy had been lost.
    assert_eq!(
        body.pointer("/settings/comboStrategy"),
        Some(&json!("round-robin")),
        "body: {body}"
    );
    assert_eq!(
        body.pointer("/settings/comboStickyRoundRobinLimit"),
        Some(&json!(4))
    );
    Ok(())
}

#[actix_web::test]
async fn dry_run_reports_without_writing() -> TestResult {
    let fixture = Fixture::new("dry");
    fixture.write_sqlite();
    let store = StateStore::memory();

    let (status, body) = run_import(
        store.clone(),
        json!({ "dataDir": fixture.path(), "dryRun": true }),
    )
    .await?;

    assert_eq!(status, 200);
    assert_eq!(body.get("dryRun"), Some(&json!(true)));
    // The report still counts what would land.
    assert_eq!(body.pointer("/report/connectionsImported"), Some(&json!(2)));
    // But nothing was actually written.
    assert!(
        store.list_connections_for_test().is_empty(),
        "dry run must not write"
    );
    Ok(())
}

#[actix_web::test]
async fn importing_twice_does_not_duplicate() -> TestResult {
    let fixture = Fixture::new("idempotent");
    fixture.write_sqlite();
    let store = StateStore::memory();

    run_import(
        store.clone(),
        json!({ "dataDir": fixture.path(), "dryRun": false }),
    )
    .await?;
    let after_first = store.list_connections_for_test().len();

    let (_, body) = run_import(
        store.clone(),
        json!({ "dataDir": fixture.path(), "dryRun": false }),
    )
    .await?;

    // A re-run must be safe: nothing new, and the skips are reported.
    assert_eq!(store.list_connections_for_test().len(), after_first);
    assert_eq!(body.pointer("/report/connectionsImported"), Some(&json!(0)));
    let warnings = body
        .pointer("/report/warnings")
        .and_then(Value::as_array)
        .expect("warnings");
    assert!(
        warnings
            .iter()
            .filter_map(Value::as_str)
            .any(|warning| warning.contains("existing connection")),
        "duplicates must be reported as skipped: {warnings:?}"
    );
    Ok(())
}

#[actix_web::test]
async fn legacy_json_install_is_supported() -> TestResult {
    let fixture = Fixture::new("legacy");
    // Pre-SQLite 9Router installs keep a flat db.json.
    std::fs::write(
        fixture.dir.join("db.json"),
        json!({
            "providerConnections": [{
                "id": "c1", "provider": "groq", "authType": "apikey",
                "name": "legacy-groq", "apiKey": "gsk-legacy", "isActive": 1,
            }],
            "combos": [{ "id": "k1", "name": "legacy-combo", "models": ["groq/llama"] }],
            "settings": { "requireApiKey": true },
        })
        .to_string(),
    )
    .expect("write legacy json");

    let store = StateStore::memory();
    let (status, body) = run_import(
        store.clone(),
        json!({ "dataDir": fixture.path(), "dryRun": false }),
    )
    .await?;

    assert_eq!(status, 200, "body: {body}");
    assert_eq!(body.pointer("/report/format"), Some(&json!("json")));
    assert_eq!(body.pointer("/report/connectionsImported"), Some(&json!(1)));
    assert_eq!(body.pointer("/report/combosImported"), Some(&json!(1)));

    let connections = store.list_connections_for_test();
    let groq = connections
        .iter()
        .find(|connection| connection.provider == "groq")
        .expect("groq imported");
    assert_eq!(groq.api_key.as_deref(), Some("gsk-legacy"));
    Ok(())
}

#[actix_web::test]
async fn a_missing_installation_reports_where_it_looked() -> TestResult {
    let store = StateStore::memory();
    let (status, body) = run_import(
        store,
        json!({ "dataDir": "/definitely/not/a/9router/install", "dryRun": false }),
    )
    .await?;

    assert_eq!(status, 404, "body: {body}");
    assert_eq!(body.get("error"), Some(&json!("no_9router_installation")));
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        message.contains("/definitely/not/a/9router/install"),
        "the searched path must be reported: {message}"
    );
    Ok(())
}

#[actix_web::test]
async fn the_source_9router_database_is_never_modified() -> TestResult {
    let fixture = Fixture::new("readonly");
    fixture.write_sqlite();
    let db = fixture.dir.join("db").join("data.sqlite");
    let before = std::fs::read(&db).expect("read fixture");

    let store = StateStore::memory();
    run_import(store, json!({ "dataDir": fixture.path(), "dryRun": false })).await?;

    // Opened read-only, so a live 9Router install cannot be disturbed.
    let after = std::fs::read(&db).expect("read fixture again");
    assert_eq!(before, after, "the source database must not be written to");
    Ok(())
}
