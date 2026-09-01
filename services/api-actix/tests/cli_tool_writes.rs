//! `POST`/`DELETE /api/cli-tools/{tool}` against a real filesystem.
//!
//! Every assertion here reads a file back off disk. That is the point of the suite: the route it
//! covers replaced a handler that returned 501, and a mutation route that reports success without
//! having written anything is exactly the failure a mocked test cannot see.
//!
//! `$HOME` is redirected to a temporary directory for each case, under a process-wide lock. These
//! tests run as threads in one binary and the config paths are resolved from the environment, so
//! without the lock two cases would resolve each other's home directory.

#![allow(clippy::future_not_send)]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "free helpers here are not #[test] fns, so clippy.toml's allow-expect-in-tests does \
              not cover them, and assertions read clearer with expect than error plumbing"
)]

#![allow(
    clippy::indexing_slicing,
    reason = "indexing a serde_json::Value is the assertion: a shape that does not match \
              is a test failure, which is what the panic reports"
)]

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use actix_web::{
    App,
    body::to_bytes,
    http::{Method, StatusCode, header},
    test, web,
};
use serde_json::{Value, json};

use nullrouter_api::{AppConfig, RuntimeClient, StateClient, configure};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const UNREACHABLE_STATE_ADDR: &str = "127.0.0.1:1";

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// A temporary `$HOME` (and `$XDG_CONFIG_HOME`), restored on drop.
struct HomeGuard {
    _lock: MutexGuard<'static, ()>,
    directory: tempfile::TempDir,
    previous_home: Option<std::ffi::OsString>,
    previous_xdg: Option<std::ffi::OsString>,
}

impl HomeGuard {
    fn new() -> Self {
        let lock = env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let directory = tempfile::tempdir().expect("tempdir");
        let previous_home = std::env::var_os("HOME");
        let previous_xdg = std::env::var_os("XDG_CONFIG_HOME");
        // SAFETY: the lock is held, so no other test in this process reads or writes these here.
        unsafe { std::env::set_var("HOME", directory.path()) };
        // SAFETY: as above.
        unsafe { std::env::set_var("XDG_CONFIG_HOME", directory.path().join(".config")) };
        Self {
            _lock: lock,
            directory,
            previous_home,
            previous_xdg,
        }
    }

    fn path(&self, relative: &str) -> PathBuf {
        let mut path = self.directory.path().to_owned();
        for segment in relative.split('/') {
            path.push(segment);
        }
        path
    }

    /// Seed a file, creating its parents, so a merge has something to merge into.
    fn seed(&self, relative: &str, contents: &str) -> PathBuf {
        let path = self.path(relative);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("create parents");
        std::fs::write(&path, contents).expect("seed");
        path
    }

    fn read(&self, relative: &str) -> String {
        std::fs::read_to_string(self.path(relative))
            .unwrap_or_else(|error| panic!("reading {relative}: {error}"))
    }

    fn read_json(&self, relative: &str) -> Value {
        serde_json::from_str(&self.read(relative))
            .unwrap_or_else(|error| panic!("parsing {relative}: {error}"))
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        // Restored rather than cleared: a panicking test must not leave the rest of the binary
        // resolving config paths inside a directory that is about to be deleted.
        restore("HOME", self.previous_home.take());
        restore("XDG_CONFIG_HOME", self.previous_xdg.take());
    }
}

/// Put one variable back to what it was, or unset it.
///
/// A free function so each write is its own `unsafe` block: the lock the guard holds is what makes
/// them sound, and one block per operation keeps that claim attached to a single call.
fn restore(name: &str, previous: Option<std::ffi::OsString>) {
    match previous {
        // SAFETY: the caller is `HomeGuard::drop`, which still holds the env lock.
        Some(value) => unsafe { std::env::set_var(name, value) },
        // SAFETY: as above.
        None => unsafe { std::env::remove_var(name) },
    }
}

async fn call(method: Method, uri: &str, body: &Value) -> TestResult<(StatusCode, Value)> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(AppConfig::new("0.5.20")))
            .app_data(web::Data::new(StateClient::new(UNREACHABLE_STATE_ADDR)))
            .app_data(web::Data::new(RuntimeClient::new(UNREACHABLE_STATE_ADDR)))
            .configure(configure),
    )
    .await;
    let request = test::TestRequest::default()
        .method(method)
        .uri(uri)
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(serde_json::to_string(body)?)
        .to_request();

    let response = test::call_service(&app, request).await;
    let status = response.status();
    let bytes = to_bytes(response.into_body()).await?;
    Ok((status, serde_json::from_slice(&bytes)?))
}

async fn apply(tool: &str, body: &Value) -> TestResult<(StatusCode, Value)> {
    call(Method::POST, &format!("/api/cli-tools/{tool}"), body).await
}

/// A revoke, with no body at all — which is what upstream's `DELETE` takes.
async fn revoke(tool: &str) -> TestResult<(StatusCode, Value)> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(AppConfig::new("0.5.20")))
            .app_data(web::Data::new(StateClient::new(UNREACHABLE_STATE_ADDR)))
            .app_data(web::Data::new(RuntimeClient::new(UNREACHABLE_STATE_ADDR)))
            .configure(configure),
    )
    .await;
    let request = test::TestRequest::default()
        .method(Method::DELETE)
        .uri(&format!("/api/cli-tools/{tool}"))
        .to_request();
    let response = test::call_service(&app, request).await;
    let status = response.status();
    let bytes = to_bytes(response.into_body()).await?;
    Ok((status, serde_json::from_slice(&bytes)?))
}

fn backup_of(path: &Path) -> PathBuf {
    let mut name = path.file_name().expect("file name").to_os_string();
    name.push(".9router-backup");
    path.with_file_name(name)
}

const CLAUDE_SETTINGS: &str = ".claude/settings.json";
const CODEX_CONFIG: &str = ".codex/config.toml";
const CODEX_AUTH: &str = ".codex/auth.json";
const CLINE_STATE: &str = ".cline/data/globalState.json";
const CLINE_SECRETS: &str = ".cline/data/secrets.json";
const KILO_AUTH: &str = ".local/share/kilo/auth.json";
const VSCODE_SETTINGS: &str = ".config/Code/User/settings.json";
const COPILOT_MODELS: &str = ".config/Code/User/chatLanguageModels.json";
const OPENCODE_CONFIG: &str = ".config/opencode/opencode.json";
const DROID_SETTINGS: &str = ".factory/settings.json";
const HERMES_CONFIG: &str = ".hermes/config.yaml";
const HERMES_ENV: &str = ".hermes/.env";
const DEEPSEEK_CONFIG: &str = ".deepseek/config.toml";
const JCODE_CONFIG: &str = ".jcode/config.toml";
const JCODE_ENV: &str = ".config/jcode/provider-9router.env";
const OPENCLAW_SETTINGS: &str = ".openclaw/openclaw.json";
const GROK_CONFIG: &str = ".grok/config.toml";
const COWORK_META: &str = ".config/Claude/configLibrary/_meta.json";
const COWORK_CONFIG: &str = ".config/Claude/configLibrary/abc-123.json";

#[actix_web::test]
async fn a_claude_apply_reaches_the_file_claude_code_reads() -> TestResult {
    // Given: a home with no Claude config yet.
    let home = HomeGuard::new();

    // When: the dashboard applies a base URL and token.
    let (status, body) = apply(
        "claude-settings",
        &json!({
            "env": {
                "ANTHROPIC_BASE_URL": "http://127.0.0.1:20128",
                "ANTHROPIC_AUTH_TOKEN": "sk-test",
            },
        }),
    )
    .await?;

    // Then: the real file exists and holds what Claude Code will read.
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["success"], true, "{body}");
    let settings = home.read_json(CLAUDE_SETTINGS);
    // `/v1` is appended, which upstream does and Claude Code needs.
    assert_eq!(
        settings["env"]["ANTHROPIC_BASE_URL"],
        "http://127.0.0.1:20128/v1"
    );
    assert_eq!(settings["env"]["ANTHROPIC_AUTH_TOKEN"], "sk-test");
    assert_eq!(settings["hasCompletedOnboarding"], true);
    // And the response names the file, so the dashboard can show it.
    assert_eq!(
        body["settingsPath"],
        home.path(CLAUDE_SETTINGS).display().to_string()
    );
    Ok(())
}

#[actix_web::test]
async fn an_apply_backs_up_the_users_file_before_the_first_write() -> TestResult {
    // Given: a config the user has their own settings in.
    let home = HomeGuard::new();
    let original = r#"{"permissions": {"allow": ["Bash"]}, "env": {"MY_OWN": "keep me"}}"#;
    let path = home.seed(CLAUDE_SETTINGS, original);

    // When: an apply runs, and then a second one.
    let (status, body) = apply(
        "claude-settings",
        &json!({"env": {"ANTHROPIC_BASE_URL": "http://127.0.0.1:20128/v1"}}),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Then: the original was copied aside before being modified.
    let backup = backup_of(&path);
    assert_eq!(
        std::fs::read_to_string(&backup)?,
        original,
        "the pre-write contents must be recoverable"
    );
    assert_eq!(
        body["backedUp"][0],
        backup.display().to_string(),
        "the backup is reported: {body}"
    );
    // And the user's own keys survived the merge, which is the reason a backup is enough.
    let settings = home.read_json(CLAUDE_SETTINGS);
    assert_eq!(settings["permissions"]["allow"][0], "Bash");
    assert_eq!(settings["env"]["MY_OWN"], "keep me");

    // A second apply must not overwrite the backup with our own previous output.
    apply(
        "claude-settings",
        &json!({"env": {"ANTHROPIC_BASE_URL": "http://127.0.0.1:20128/v1"}}),
    )
    .await?;
    assert_eq!(
        std::fs::read_to_string(&backup)?,
        original,
        "the backup must stay the user's original"
    );
    Ok(())
}

#[actix_web::test]
async fn a_claude_revoke_removes_only_the_keys_upstream_lists() -> TestResult {
    // Given: an applied config sitting next to the user's own env keys.
    let home = HomeGuard::new();
    home.seed(
        CLAUDE_SETTINGS,
        r#"{"env": {"ANTHROPIC_BASE_URL": "http://x/v1", "API_TIMEOUT_MS": "600000",
             "MY_OWN": "keep me"}, "hooks": {"mine": 1}}"#,
    );

    // When: it is revoked.
    let (status, body) = revoke("claude-settings").await?;

    // Then: our keys are gone and nothing else is.
    assert_eq!(status, StatusCode::OK, "{body}");
    let settings = home.read_json(CLAUDE_SETTINGS);
    assert!(settings["env"]["ANTHROPIC_BASE_URL"].is_null(), "{settings}");
    assert!(settings["env"]["API_TIMEOUT_MS"].is_null(), "{settings}");
    assert_eq!(settings["env"]["MY_OWN"], "keep me");
    assert_eq!(settings["hooks"]["mine"], 1);
    Ok(())
}

#[actix_web::test]
async fn a_revoke_reports_a_missing_config_rather_than_creating_one() -> TestResult {
    // Given: a home where the tool was never configured.
    let home = HomeGuard::new();

    // When: a revoke arrives anyway, which is what the dashboard sends when a user toggles a tool
    // off that they had never toggled on.
    let (status, body) = revoke("claude-settings").await?;

    // Then: it says so, and no file was created. A config file left behind for a tool the user
    // never set up is worse than nothing: `has9Router` reads it, and the tool may too.
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["message"], "No settings file to reset", "{body}");
    assert_eq!(body["written"].as_array().map(Vec::len), Some(0), "{body}");
    assert!(
        !home.path(CLAUDE_SETTINGS).exists(),
        "a revoke must not create the file it had nothing to remove from"
    );
    assert!(
        !home.path(".claude").exists(),
        "nor the directory: {:?}",
        home.path(".claude")
    );
    Ok(())
}

#[actix_web::test]
async fn a_codex_apply_writes_both_the_config_and_the_credential() -> TestResult {
    // Given: a home with no Codex config.
    let home = HomeGuard::new();

    // When: an apply runs.
    let (status, body) = apply(
        "codex-settings",
        &json!({
            "baseUrl": "http://127.0.0.1:20128",
            "apiKey": "sk-codex",
            "model": "cc/opus",
        }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Then: the TOML holds the provider, and the key is in `auth.json` where Codex looks first.
    // A config written without the second file leaves Codex pointing here with nothing to
    // authenticate with, which is why both are `Required::Yes`.
    let config = home.read(CODEX_CONFIG);
    assert!(config.contains(r#"model_provider = "9router""#), "{config}");
    assert!(config.contains("[model_providers.9router]"), "{config}");
    assert!(
        config.contains(r#"base_url = "http://127.0.0.1:20128/v1""#),
        "the /v1 suffix codex needs: {config}"
    );
    assert!(
        config.contains(r#"wire_api = "responses""#),
        "codex speaks the Responses API to this provider: {config}"
    );
    // The subagent model defaults to the main model rather than being left unset.
    assert!(config.contains("[agents.subagent]"), "{config}");
    assert!(config.contains(r#"model = "cc/opus""#), "{config}");
    // And the key never lands in config.toml.
    assert!(!config.contains("sk-codex"), "the key must stay out of the config: {config}");

    let auth = home.read_json(CODEX_AUTH);
    assert_eq!(auth["OPENAI_API_KEY"], "sk-codex");
    assert_eq!(auth["auth_mode"], "apikey");
    Ok(())
}

#[actix_web::test]
async fn a_codex_revoke_keeps_a_model_the_user_repointed_elsewhere() -> TestResult {
    // Given: a config where the user has since moved Codex to another provider, leaving our
    // provider section behind.
    let home = HomeGuard::new();
    home.seed(
        CODEX_CONFIG,
        "model = \"gpt-5\"\nmodel_provider = \"openai\"\n\n\
         [model_providers.9router]\nbase_url = \"http://127.0.0.1:20128/v1\"\n",
    );

    // When: the revoke runs.
    let (status, body) = revoke("codex-settings").await?;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Then: our section is gone but their model selection is untouched. Deleting `model` here
    // would break a Codex the user had working.
    let config = home.read(CODEX_CONFIG);
    assert!(!config.contains("model_providers.9router"), "{config}");
    assert!(config.contains(r#"model = "gpt-5""#), "{config}");
    assert!(config.contains(r#"model_provider = "openai""#), "{config}");
    Ok(())
}

#[actix_web::test]
async fn a_codex_revoke_unlinks_an_auth_file_it_emptied() -> TestResult {
    // Given: an `auth.json` holding nothing but what an apply put there.
    let home = HomeGuard::new();
    home.seed(CODEX_CONFIG, "model_provider = \"9router\"\n");
    home.seed(
        CODEX_AUTH,
        r#"{"OPENAI_API_KEY": "sk-codex", "auth_mode": "apikey"}"#,
    );

    // When: it is revoked.
    let (status, body) = revoke("codex-settings").await?;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Then: the file is removed, not left as `{}`. An empty `auth.json` reads to Codex as api-key
    // mode with a blank key, which stops it falling back to a ChatGPT login.
    assert!(
        !home.path(CODEX_AUTH).exists(),
        "an emptied auth.json must be unlinked"
    );
    Ok(())
}

#[actix_web::test]
async fn a_codex_revoke_keeps_an_auth_file_holding_a_chatgpt_login() -> TestResult {
    // Given: an `auth.json` that also holds tokens from a ChatGPT login.
    let home = HomeGuard::new();
    home.seed(CODEX_CONFIG, "model_provider = \"9router\"\n");
    home.seed(
        CODEX_AUTH,
        r#"{"OPENAI_API_KEY": "sk-codex", "auth_mode": "apikey", "tokens": {"id": "keep"}}"#,
    );

    // When: it is revoked.
    revoke("codex-settings").await?;

    // Then: only our two keys went. Unlinking here would log the user out of ChatGPT.
    let auth = home.read_json(CODEX_AUTH);
    assert!(auth["OPENAI_API_KEY"].is_null(), "{auth}");
    assert!(auth["auth_mode"].is_null(), "{auth}");
    assert_eq!(auth["tokens"]["id"], "keep");
    Ok(())
}

#[actix_web::test]
async fn cline_gets_the_origin_without_v1_and_both_modes_set() -> TestResult {
    // Given: a fresh home.
    let home = HomeGuard::new();

    // When: an apply runs with a `/v1` base URL.
    let (status, body) = apply(
        "cline-settings",
        &json!({
            "baseUrl": "http://127.0.0.1:20128/v1",
            "apiKey": "sk-cline",
            "model": "cc/sonnet",
        }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Then: cline gets the bare origin — it appends its own path, so a `/v1` here becomes
    // `/v1/chat/completions` twice over.
    let state = home.read_json(CLINE_STATE);
    assert_eq!(state["openAiBaseUrl"], "http://127.0.0.1:20128");
    // Both modes, because cline routes act and plan independently.
    assert_eq!(state["actModeApiProvider"], "openai");
    assert_eq!(state["planModeApiProvider"], "openai");
    assert_eq!(state["openAiModelId"], "cc/sonnet");
    assert_eq!(state["planModeOpenAiModelId"], "cc/sonnet");
    // And the key is in the secrets file, not the state file.
    assert_eq!(home.read_json(CLINE_SECRETS)["openAiApiKey"], "sk-cline");
    assert!(
        !home.read(CLINE_STATE).contains("sk-cline"),
        "the key must not be in globalState.json"
    );
    Ok(())
}

#[actix_web::test]
async fn a_cline_revoke_restores_its_own_default_provider() -> TestResult {
    // Given: an applied cline config.
    let home = HomeGuard::new();
    apply(
        "cline-settings",
        &json!({"baseUrl": "http://127.0.0.1:20128", "apiKey": "sk", "model": "m"}),
    )
    .await?;

    // When: it is revoked.
    revoke("cline-settings").await?;

    // Then: the provider is put back to `cline`, not deleted. Cline reads a missing provider as
    // unconfigured and shows a setup prompt; `cline` is its own default.
    let state = home.read_json(CLINE_STATE);
    assert_eq!(state["actModeApiProvider"], "cline");
    assert_eq!(state["planModeApiProvider"], "cline");
    assert!(state["openAiBaseUrl"].is_null(), "{state}");
    assert!(state["openAiModelId"].is_null(), "{state}");
    assert!(
        home.read_json(CLINE_SECRETS)["openAiApiKey"].is_null(),
        "the key must be gone"
    );
    Ok(())
}

#[actix_web::test]
async fn kilo_writes_its_own_auth_and_vs_codes_settings() -> TestResult {
    // Given: a home with a VS Code user settings file the user has their own keys in.
    let home = HomeGuard::new();
    home.seed(VSCODE_SETTINGS, r#"{"editor.fontSize": 13}"#);

    // When: an apply runs.
    let (status, body) = apply(
        "kilo-settings",
        &json!({
            "baseUrl": "http://127.0.0.1:20128",
            "apiKey": "sk-kilo",
            "model": "cc/haiku",
        }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{body}");

    let auth = home.read_json(KILO_AUTH);
    assert_eq!(auth["openai-compatible"]["type"], "api-key");
    assert_eq!(auth["openai-compatible"]["baseUrl"], "http://127.0.0.1:20128/v1");
    assert_eq!(auth["openai-compatible"]["apiKey"], "sk-kilo");
    assert_eq!(auth["openai-compatible"]["model"], "cc/haiku");

    // Then: VS Code's dotted keys are written as single keys, not as a nested object. Nesting
    // would produce `{"kilocode": {"customProvider": ...}}`, which the extension does not read.
    let vscode = home.read_json(VSCODE_SETTINGS);
    assert_eq!(vscode["kilocode.customProvider"]["name"], "9Router");
    // `baseURL` here, against `baseUrl` in auth.json — the extension's own spelling.
    assert_eq!(
        vscode["kilocode.customProvider"]["baseURL"],
        "http://127.0.0.1:20128/v1"
    );
    assert_eq!(vscode["kilocode.defaultModel"], "cc/haiku");
    assert!(
        vscode.get("kilocode").is_none(),
        "the dotted key must not have been nested: {vscode}"
    );
    // And the user's own settings survived.
    assert_eq!(vscode["editor.fontSize"], 13);
    Ok(())
}

#[actix_web::test]
async fn copilot_is_written_as_a_top_level_array_with_the_azure_fragment() -> TestResult {
    // Given: a Copilot config the user already has another provider in.
    let home = HomeGuard::new();
    home.seed(COPILOT_MODELS, r#"[{"name": "mine", "models": []}]"#);

    // When: an apply runs with two models and no key.
    let (status, body) = apply(
        "copilot-settings",
        &json!({
            "baseUrl": "http://127.0.0.1:20128",
            "models": ["cc/opus", "cc/sonnet"],
        }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Then: the document stays an array, and the user's entry is still in it.
    let config = home.read_json(COPILOT_MODELS);
    let entries = config.as_array().expect("a top-level array");
    assert_eq!(entries.len(), 2, "{config}");
    assert_eq!(entries[0]["name"], "mine");

    let ours = &entries[1];
    assert_eq!(ours["name"], "9Router", "the capitalisation copilot matches on");
    assert_eq!(ours["vendor"], "azure");
    // The key is defaulted, not required.
    assert_eq!(ours["apiKey"], "sk_9router");
    // The endpoint carries the Azure dialect fragment and takes **no** `/v1`. Normalising this
    // URL would break the one copilot builds from it.
    assert_eq!(
        ours["models"][0]["url"],
        "http://127.0.0.1:20128/chat/completions#models.ai.azure.com"
    );
    assert_eq!(ours["models"][0]["id"], "cc/opus");
    assert_eq!(ours["models"][1]["id"], "cc/sonnet");
    assert_eq!(ours["models"][0]["toolCalling"], true);
    Ok(())
}

#[actix_web::test]
async fn a_second_copilot_apply_replaces_its_entry_rather_than_appending() -> TestResult {
    // Given: an applied config.
    let home = HomeGuard::new();
    apply(
        "copilot-settings",
        &json!({"baseUrl": "http://127.0.0.1:20128", "models": ["a"]}),
    )
    .await?;

    // When: a second apply changes the model list.
    apply(
        "copilot-settings",
        &json!({"baseUrl": "http://127.0.0.1:20128", "models": ["b"]}),
    )
    .await?;

    // Then: there is still one entry. Appending would leave copilot with two providers of the
    // same name and no way to tell which is current.
    let config = home.read_json(COPILOT_MODELS);
    let entries = config.as_array().expect("array");
    assert_eq!(entries.len(), 1, "{config}");
    assert_eq!(entries[0]["models"][0]["id"], "b");

    // And a revoke takes only ours, leaving a non-empty file as an array.
    home.seed(COPILOT_MODELS, r#"[{"name": "mine"}, {"name": "9Router"}]"#);
    revoke("copilot-settings").await?;
    let config = home.read_json(COPILOT_MODELS);
    assert_eq!(config.as_array().map(Vec::len), Some(1), "{config}");
    assert_eq!(config[0]["name"], "mine");
    Ok(())
}

#[actix_web::test]
async fn opencode_keeps_the_npm_client_and_models_a_previous_apply_wrote() -> TestResult {
    // Given: an existing provider entry with a model this apply does not mention.
    let home = HomeGuard::new();
    home.seed(
        OPENCODE_CONFIG,
        r#"{"provider": {"9router": {"npm": "@ai-sdk/openai-compatible",
             "options": {"custom": "keep"}, "models": {"old/model": {"name": "old/model"}}}},
             "theme": "mine"}"#,
    );

    // When: an apply runs with a different model.
    let (status, body) = apply(
        "opencode-settings",
        &json!({"baseUrl": "http://127.0.0.1:20128", "models": ["new/model"]}),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Then: the npm client survives — opencode loads its client from that key, so losing it
    // leaves a provider it cannot instantiate.
    let config = home.read_json(OPENCODE_CONFIG);
    let provider = &config["provider"]["9router"];
    assert_eq!(provider["npm"], "@ai-sdk/openai-compatible");
    // And so does the earlier model, alongside the new one.
    assert_eq!(provider["models"]["old/model"]["name"], "old/model");
    assert_eq!(provider["models"]["new/model"]["name"], "new/model");
    // Unrelated options are merged, not replaced.
    assert_eq!(provider["options"]["custom"], "keep");
    assert_eq!(provider["options"]["baseURL"], "http://127.0.0.1:20128/v1");
    assert_eq!(provider["options"]["apiKey"], "sk_9router");
    // The selection is qualified with the provider name.
    assert_eq!(config["model"], "9router/new/model");
    assert_eq!(config["agent"]["explorer"]["mode"], "subagent");
    assert_eq!(config["theme"], "mine");
    Ok(())
}

#[actix_web::test]
async fn an_empty_active_model_clears_the_opencode_selection() -> TestResult {
    // Given: a fresh home. An explicitly empty `activeModel` is how the dashboard sends
    // "no default", against a missing one meaning "pick the first".
    let home = HomeGuard::new();

    // When: an apply names models but no active one.
    apply(
        "opencode-settings",
        &json!({"baseUrl": "http://x", "models": ["a", "b"], "activeModel": ""}),
    )
    .await?;

    // Then: the selection is empty while the models stay listed.
    let config = home.read_json(OPENCODE_CONFIG);
    assert_eq!(config["model"], "", "{config}");
    assert!(config["provider"]["9router"]["models"]["a"].is_object(), "{config}");

    // And a missing `activeModel` picks the first instead.
    apply(
        "opencode-settings",
        &json!({"baseUrl": "http://x", "models": ["a", "b"]}),
    )
    .await?;
    assert_eq!(home.read_json(OPENCODE_CONFIG)["model"], "9router/a");
    Ok(())
}

#[actix_web::test]
async fn an_opencode_revoke_leaves_a_selection_pointing_elsewhere() -> TestResult {
    // Given: a config where the user has since selected another provider's model.
    let home = HomeGuard::new();
    home.seed(
        OPENCODE_CONFIG,
        r#"{"provider": {"9router": {"models": {}}, "other": {}}, "model": "anthropic/opus"}"#,
    );

    // When: the revoke runs.
    revoke("opencode-settings").await?;

    // Then: our provider is gone and their selection is not. Clearing `model` here would leave
    // opencode with no model chosen because of a revoke of a provider it was not using.
    let config = home.read_json(OPENCODE_CONFIG);
    assert!(config["provider"]["9router"].is_null(), "{config}");
    assert_eq!(config["model"], "anthropic/opus");
    assert!(config["provider"]["other"].is_object(), "{config}");
    Ok(())
}

#[actix_web::test]
async fn droid_ids_are_indexed_and_the_default_is_moved_to_the_front() -> TestResult {
    // Given: a settings file holding the user's own custom model.
    let home = HomeGuard::new();
    home.seed(
        DROID_SETTINGS,
        r#"{"customModels": [{"id": "custom:mine", "model": "mine"}], "theme": "dark"}"#,
    );

    // When: an apply names three models and picks the third as default.
    let (status, body) = apply(
        "droid-settings",
        &json!({
            "baseUrl": "http://127.0.0.1:20128",
            "models": ["a", "b", "c"],
            "activeModel": "c",
        }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{body}");

    let settings = home.read_json(DROID_SETTINGS);
    let models = settings["customModels"].as_array().expect("array");
    // Droid takes the first entry as the default, so the reorder *is* the setting.
    assert_eq!(models[0]["model"], "c", "the default must be first: {settings}");
    // And `index` is renumbered to match the new order.
    for (position, model) in models.iter().enumerate() {
        assert_eq!(model["index"], position, "{settings}");
    }
    // Ids are prefixed and numbered. `spec`'s marker matches this by prefix; an equality test
    // against `custom:9Router` would match none of them.
    let ours: Vec<&str> = models
        .iter()
        .filter_map(|model| model["id"].as_str())
        .filter(|id| id.starts_with("custom:9Router"))
        .collect();
    assert_eq!(ours.len(), 3, "{settings}");
    // The user's own model and other settings survived.
    assert!(
        models.iter().any(|model| model["id"] == "custom:mine"),
        "{settings}"
    );
    assert_eq!(settings["theme"], "dark");
    // Droid's placeholder is its own, not the one the other tools use.
    let placeholder = models
        .iter()
        .find(|model| model["id"].as_str().is_some_and(|id| id.starts_with("custom:9Router")))
        .and_then(|model| model["apiKey"].as_str());
    assert_eq!(placeholder, Some("your_api_key"), "{settings}");
    Ok(())
}

#[actix_web::test]
async fn a_droid_reapply_replaces_its_entries_rather_than_stacking_them() -> TestResult {
    // Given: an applied config with three models.
    let home = HomeGuard::new();
    apply(
        "droid-settings",
        &json!({"baseUrl": "http://x", "models": ["a", "b", "c"]}),
    )
    .await?;

    // When: a second apply names one.
    apply("droid-settings", &json!({"baseUrl": "http://x", "models": ["a"]})).await?;

    // Then: the two dropped models are gone. Matching by prefix is what makes this work; an
    // equality match would leave `custom:9Router-1` and `-2` behind forever.
    let settings = home.read_json(DROID_SETTINGS);
    assert_eq!(settings["customModels"].as_array().map(Vec::len), Some(1), "{settings}");

    // And a revoke removes the array entirely rather than leaving `[]`.
    revoke("droid-settings").await?;
    let settings = home.read_json(DROID_SETTINGS);
    assert!(
        settings["customModels"].is_null(),
        "an emptied array must be dropped: {settings}"
    );
    Ok(())
}

#[actix_web::test]
async fn hermes_gets_a_yaml_block_and_keeps_the_rest_byte_identical() -> TestResult {
    // Given: a hand-written config with comments and the user's own sections.
    let home = HomeGuard::new();
    let original = "# my notes\nagent:\n  name: mine\n  tools:\n    - shell\n\nlogging:\n  level: debug\n";
    home.seed(HERMES_CONFIG, original);

    // When: an apply runs.
    let (status, body) = apply(
        "hermes-settings",
        &json!({
            "baseUrl": "http://127.0.0.1:20128",
            "apiKey": "sk-hermes",
            "model": "cc/opus",
        }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Then: the user's YAML is present verbatim — comments, key order and all. This is why the
    // file is edited by block rather than parsed and re-serialised.
    let config = home.read(HERMES_CONFIG);
    assert!(config.contains(original), "the rest must survive: {config}");
    assert!(config.contains("model:\n  default: \"cc/opus\""), "{config}");
    assert!(config.contains("base_url: \"http://127.0.0.1:20128/v1\""), "{config}");
    // The key is a literal `${OPENAI_API_KEY}` reference, and the secret is in `.env`.
    assert!(config.contains("api_key: ${OPENAI_API_KEY}"), "{config}");
    assert!(
        !config.contains("sk-hermes"),
        "the key must not reach the YAML, which users keep in dotfile repos: {config}"
    );
    assert!(home.read(HERMES_ENV).contains("OPENAI_API_KEY=sk-hermes"));
    Ok(())
}

#[actix_web::test]
async fn a_hermes_apply_without_a_key_does_not_create_an_env_file() -> TestResult {
    // Given: a fresh home. Upstream writes `.env` only `if (apiKey)`.
    let home = HomeGuard::new();

    // When: an apply omits the key.
    apply(
        "hermes-settings",
        &json!({"baseUrl": "http://x", "model": "m"}),
    )
    .await?;

    // Then: the YAML is written and no empty `.env` is left behind.
    assert!(home.path(HERMES_CONFIG).exists());
    assert!(
        !home.path(HERMES_ENV).exists(),
        "an apply with no key must not create a blank .env"
    );

    // And a revoke removes the block, leaving `.env` alone — the variable name is generic enough
    // that the user may own it for real OpenAI.
    home.seed(HERMES_ENV, "OPENAI_API_KEY=theirs\n");
    revoke("hermes-settings").await?;
    assert!(!home.read(HERMES_CONFIG).contains("model:"), "block removed");
    assert_eq!(home.read(HERMES_ENV), "OPENAI_API_KEY=theirs\n");
    Ok(())
}

#[actix_web::test]
async fn deepseek_is_merged_rather_than_overwritten() -> TestResult {
    // Given: a config holding another provider the user set up.
    let home = HomeGuard::new();
    home.seed(
        DEEPSEEK_CONFIG,
        "provider = \"deepseek\"\n\n[providers.anthropic]\napi_key = \"keep-me\"\n",
    );

    // When: an apply runs.
    let (status, body) = apply(
        "deepseek-tui-settings",
        &json!({"baseUrl": "http://127.0.0.1:20128", "model": "cc/opus"}),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Then: the provider points here, and the user's other section survived. Upstream replaces the
    // whole file here, which would have deleted it.
    let config = home.read(DEEPSEEK_CONFIG);
    assert!(config.contains(r#"provider = "openai""#), "{config}");
    assert!(config.contains("[providers.openai]"), "{config}");
    assert!(config.contains(r#"base_url = "http://127.0.0.1:20128/v1""#), "{config}");
    assert!(config.contains("keep-me"), "the user's provider must survive: {config}");
    // No `9router` string appears in a deepseek config, which is why `spec`'s marker tests the
    // provider and URL rather than grepping for a name.
    assert_eq!(config.matches("9router").count(), 1, "only the placeholder key: {config}");
    Ok(())
}

#[actix_web::test]
async fn a_deepseek_revoke_keeps_a_real_openai_section() -> TestResult {
    // Given: a config where the user has since put their real OpenAI key in that section.
    let home = HomeGuard::new();
    home.seed(
        DEEPSEEK_CONFIG,
        "provider = \"openai\"\n\n[providers.openai]\n\
         base_url = \"https://api.openai.com/v1\"\napi_key = \"sk-real\"\n",
    );

    // When: the revoke runs.
    revoke("deepseek-tui-settings").await?;

    // Then: the section is left alone, because its URL is not local and so is not ours. Upstream
    // cannot make this distinction: it writes a two-line default over the whole file.
    let config = home.read(DEEPSEEK_CONFIG);
    assert!(config.contains("sk-real"), "the user's key must survive: {config}");
    assert!(config.contains("api.openai.com"), "{config}");
    Ok(())
}

#[actix_web::test]
async fn jcode_points_at_an_env_file_it_also_writes() -> TestResult {
    // Given: a fresh home.
    let home = HomeGuard::new();

    // When: an apply runs.
    let (status, body) = apply(
        "jcode-settings",
        &json!({
            "baseUrl": "http://127.0.0.1:20128",
            "apiKey": "sk-jcode",
            "models": ["cc/opus", "cc/sonnet"],
        }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Then: the config names where the key lives, and the key is there. The two are coupled by
    // those strings: a config written without the env file leaves jcode looking up a variable
    // nothing sets, and `requires_api_key` makes it fail rather than send an unauthenticated call.
    let config = home.read(JCODE_CONFIG);
    assert!(config.contains(r#"api_key_env = "JCODE_9ROUTER_API_KEY""#), "{config}");
    assert!(config.contains(r#"env_file = "provider-9router.env""#), "{config}");
    assert!(config.contains(r#"default_model = "cc/opus""#), "the first model: {config}");
    assert!(config.contains("requires_api_key = true"), "{config}");
    assert!(!config.contains("sk-jcode"), "the key stays out of the config: {config}");
    assert!(home.read(JCODE_ENV).contains("JCODE_9ROUTER_API_KEY=sk-jcode"));

    // And a revoke takes both, since this variable name is ours and nothing else reads it.
    revoke("jcode-settings").await?;
    assert!(!home.read(JCODE_CONFIG).contains("9router"), "{}", home.read(JCODE_CONFIG));
    assert!(!home.read(JCODE_ENV).contains("JCODE_9ROUTER_API_KEY"));
    Ok(())
}

#[actix_web::test]
async fn openclaw_writes_the_provider_the_selection_and_the_allowlist() -> TestResult {
    // Given: a fresh home. All three have to agree or the model is configured and unusable:
    // the provider supplies the endpoint, `model.primary` selects it, and `defaults.models` is an
    // allowlist OpenClaw checks the selection against.
    let home = HomeGuard::new();

    // When: an apply runs.
    let (status, body) = apply(
        "openclaw-settings",
        &json!({
            "baseUrl": "http://127.0.0.1:20128",
            "apiKey": "sk-claw",
            "model": "cc/opus",
        }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{body}");

    let settings = home.read_json(OPENCLAW_SETTINGS);
    let provider = &settings["models"]["providers"]["9router"];
    assert_eq!(provider["baseUrl"], "http://127.0.0.1:20128/v1");
    assert_eq!(provider["api"], "openai-completions");
    assert_eq!(provider["models"][0]["id"], "cc/opus");
    // The display name is the last path segment, per upstream's `split("/").pop()`.
    assert_eq!(provider["models"][0]["name"], "opus");
    // The selection and the allowlist are both qualified with the provider name.
    assert_eq!(settings["agents"]["defaults"]["model"]["primary"], "9router/cc/opus");
    assert!(
        settings["agents"]["defaults"]["models"]["9router/cc/opus"].is_object(),
        "the allowlist gates the selection: {settings}"
    );
    Ok(())
}

#[actix_web::test]
async fn a_reapply_clears_stale_openclaw_allowlist_entries() -> TestResult {
    // Given: a config whose allowlist holds a model from an earlier apply, next to one the user
    // added for another provider.
    let home = HomeGuard::new();
    home.seed(
        OPENCLAW_SETTINGS,
        r#"{"agents": {"defaults": {"models": {"9router/old": {}, "anthropic/opus": {}}}}}"#,
    );

    // When: a new apply names a different model.
    apply(
        "openclaw-settings",
        &json!({"baseUrl": "http://x", "apiKey": "k", "model": "new"}),
    )
    .await?;

    // Then: the stale entry is gone and the unrelated one is not. Leaving `9router/old` behind
    // would keep allowing a model the provider no longer serves.
    let models = &home.read_json(OPENCLAW_SETTINGS)["agents"]["defaults"]["models"];
    assert!(models["9router/old"].is_null(), "{models}");
    assert!(models["9router/new"].is_object(), "{models}");
    assert!(models["anthropic/opus"].is_object(), "the user's entry: {models}");
    Ok(())
}

#[actix_web::test]
async fn an_openclaw_revoke_leaves_an_agent_pointed_at_another_provider() -> TestResult {
    // Given: two agents, one on us and one not — and the one on us in the object form of the
    // field, which is the form a naive string test would miss.
    let home = HomeGuard::new();
    home.seed(
        OPENCLAW_SETTINGS,
        r#"{"models": {"providers": {"9router": {}}},
            "agents": {"defaults": {"model": {"primary": "9router/a"}},
            "list": [{"id": "one", "model": {"primary": "9router/a"}},
                     {"id": "two", "model": "anthropic/opus"}]}}"#,
    );

    // When: the revoke runs.
    revoke("openclaw-settings").await?;

    // Then: ours is cleared in both forms of the field, and theirs is untouched.
    let settings = home.read_json(OPENCLAW_SETTINGS);
    assert!(settings["models"]["providers"]["9router"].is_null(), "{settings}");
    assert!(settings["agents"]["defaults"]["model"]["primary"].is_null(), "{settings}");
    let agents = settings["agents"]["list"].as_array().expect("list");
    assert!(agents[0]["model"].is_null(), "the object form must be cleared: {settings}");
    assert_eq!(agents[1]["model"], "anthropic/opus");
    Ok(())
}

#[actix_web::test]
async fn a_per_agent_models_file_is_written_only_into_a_directory_that_exists() -> TestResult {
    // Given: two agents, one whose directory exists and one whose does not.
    let home = HomeGuard::new();
    let real = home.path("agents/real");
    std::fs::create_dir_all(&real)?;
    let missing = home.path("agents/missing");
    home.seed(
        OPENCLAW_SETTINGS,
        &format!(
            r#"{{"agents": {{"list": [
                 {{"id": "one", "agentDir": {}}},
                 {{"id": "two", "agentDir": {}}}]}}}}"#,
            json!(real.display().to_string()),
            json!(missing.display().to_string()),
        ),
    );

    // When: an apply runs with a per-agent override.
    let (status, body) = apply(
        "openclaw-settings",
        &json!({
            "baseUrl": "http://127.0.0.1:20128",
            "apiKey": "sk-claw",
            "model": "cc/opus",
            "agentModels": {"one": "cc/sonnet"},
        }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Then: the existing directory got its file, with the agent's own override rather than the
    // default.
    let models: Value = serde_json::from_str(&std::fs::read_to_string(real.join("models.json"))?)?;
    assert_eq!(models["providers"]["9router"]["models"][0]["id"], "cc/sonnet");
    assert_eq!(models["providers"]["9router"]["baseUrl"], "http://127.0.0.1:20128/v1");

    // And the missing one was reported rather than created. `agentDir` is a path out of a config
    // file being used as a destination, so creating it means a settings file naming `../../.ssh`
    // gets a directory tree.
    assert!(!missing.exists(), "the directory must not have been created");
    let warnings = body["warnings"].as_array().expect("warnings reported");
    assert!(
        warnings
            .iter()
            .any(|warning| warning.as_str().is_some_and(|text| text.contains("does not exist"))),
        "{body}"
    );
    Ok(())
}

#[actix_web::test]
async fn grok_remembers_the_users_previous_default_across_a_revoke() -> TestResult {
    // Given: a config where the user has chosen their own default model.
    let home = HomeGuard::new();
    home.seed(
        GROK_CONFIG,
        "# my notes\ntheme = \"dark\"\n\n[models]\ndefault = \"grok-4\"\n",
    );

    // When: an apply runs, and then a revoke.
    let (status, body) = apply(
        "grok-build-settings",
        &json!({
            "baseUrl": "http://127.0.0.1:20128",
            "apiKey": "sk-grok",
            "model": "cc/opus",
            "contextWindow": 200_000,
        }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{body}");

    let applied = home.read(GROK_CONFIG);
    assert!(applied.contains("[model.9router]"), "{applied}");
    assert!(applied.contains(r#"base_url = "http://127.0.0.1:20128/v1""#), "{applied}");
    assert!(applied.contains(r#"api_backend = "chat_completions""#), "{applied}");
    assert!(applied.contains("context_window = 200000"), "{applied}");
    assert!(applied.contains(r#"default = "9router""#), "{applied}");
    // The previous choice is recorded in a comment, which is why this file is edited as text.
    assert!(applied.contains(r#"# 9router-prev-default = "grok-4""#), "{applied}");
    // The user's own content is still there.
    assert!(applied.contains("# my notes\ntheme = \"dark\""), "{applied}");

    // Then: the revoke puts their choice back rather than the built-in default.
    revoke("grok-build-settings").await?;
    let reverted = home.read(GROK_CONFIG);
    assert!(
        reverted.contains(r#"default = "grok-4""#),
        "the user's own default must come back, not grok-build's: {reverted}"
    );
    assert!(!reverted.contains("[model.9router]"), "{reverted}");
    assert!(!reverted.contains("9router-prev-default"), "the marker is consumed: {reverted}");
    assert!(reverted.contains("theme = \"dark\""), "{reverted}");
    Ok(())
}

#[actix_web::test]
async fn a_second_grok_apply_does_not_overwrite_the_remembered_default() -> TestResult {
    // Given: a config with the user's default, already applied once.
    let home = HomeGuard::new();
    home.seed(GROK_CONFIG, "[models]\ndefault = \"grok-4\"\n");
    let payload = json!({"baseUrl": "http://x", "apiKey": "k", "model": "m"});
    apply("grok-build-settings", &payload).await?;

    // When: a second apply runs. At this point the current default is `9router`, so a naive
    // remember would record that and lose the original.
    apply("grok-build-settings", &payload).await?;

    // Then: the marker still holds their model, and a revoke restores it.
    assert!(
        home.read(GROK_CONFIG).contains(r#"# 9router-prev-default = "grok-4""#),
        "{}",
        home.read(GROK_CONFIG)
    );
    revoke("grok-build-settings").await?;
    assert!(home.read(GROK_CONFIG).contains(r#"default = "grok-4""#));
    Ok(())
}

#[actix_web::test]
async fn a_grok_revoke_with_nothing_remembered_uses_the_builtin_default() -> TestResult {
    // Given: a config that was applied when the user had made no choice of their own.
    let home = HomeGuard::new();
    apply(
        "grok-build-settings",
        &json!({"baseUrl": "http://x", "apiKey": "k", "model": "m"}),
    )
    .await?;
    // No marker, because there was no previous value to remember.
    assert!(!home.read(GROK_CONFIG).contains("9router-prev-default"));

    // When: the revoke runs.
    revoke("grok-build-settings").await?;

    // Then: Grok Build's own built-in default is written, since leaving `9router` selected would
    // point it at a slot that no longer exists.
    assert!(
        home.read(GROK_CONFIG).contains(r#"default = "grok-build""#),
        "{}",
        home.read(GROK_CONFIG)
    );
    Ok(())
}

#[actix_web::test]
async fn grok_subagent_slots_restore_an_unset_key_by_deleting_it() -> TestResult {
    // Given: a fresh config, so the subagent keys start unset. The distinction being tested is
    // "was unset" against "was set to X": restoring the first means deleting the key, and writing
    // an empty string instead would leave Grok Build resolving "" as a model name.
    let home = HomeGuard::new();

    // When: an apply sets a subagent model, and then a revoke runs.
    apply(
        "grok-build-settings",
        &json!({
            "baseUrl": "http://x",
            "apiKey": "k",
            "model": "m",
            "subagentModels": {"explore": {"model": "cc/haiku"}},
        }),
    )
    .await?;
    let applied = home.read(GROK_CONFIG);
    assert!(applied.contains("[model.9router-explore]"), "{applied}");
    assert!(applied.contains(r#"explore = "9router-explore""#), "{applied}");
    // The sentinel records that there was nothing there before.
    assert!(applied.contains("__9router_unset__"), "{applied}");

    revoke("grok-build-settings").await?;

    // Then: the key is gone, not blank, and its section with it.
    let reverted = home.read(GROK_CONFIG);
    assert!(!reverted.contains("explore ="), "the key must be deleted: {reverted}");
    assert!(!reverted.contains("[model.9router-explore]"), "{reverted}");
    assert!(!reverted.contains("__9router_unset__"), "{reverted}");
    Ok(())
}

#[actix_web::test]
async fn an_absent_subagent_block_leaves_existing_subagent_config_alone() -> TestResult {
    // Given: a config with a subagent the user set up themselves.
    let home = HomeGuard::new();
    home.seed(
        GROK_CONFIG,
        "[subagents.models]\nplan = \"grok-4-fast\"\n",
    );

    // When: an apply arrives with no `subagentModels` at all — which is what a dashboard pane that
    // does not know about subagents sends.
    apply(
        "grok-build-settings",
        &json!({"baseUrl": "http://x", "apiKey": "k", "model": "m"}),
    )
    .await?;

    // Then: their subagent is untouched. Clearing it here would mean an unrelated pane silently
    // resets a setting it never showed.
    assert!(
        home.read(GROK_CONFIG).contains(r#"plan = "grok-4-fast""#),
        "{}",
        home.read(GROK_CONFIG)
    );
    Ok(())
}

#[actix_web::test]
async fn a_zero_context_window_is_omitted_rather_than_written() -> TestResult {
    // A zero or negative window would make Grok Build reject every request as over budget, so
    // upstream writes the key only for a finite positive number.
    let home = HomeGuard::new();
    for window in [json!(0), json!(-1), json!("nonsense"), Value::Null] {
        apply(
            "grok-build-settings",
            &json!({
                "baseUrl": "http://x",
                "apiKey": "k",
                "model": "m",
                "contextWindow": window,
            }),
        )
        .await?;
        assert!(
            !home.read(GROK_CONFIG).contains("context_window"),
            "{window} produced a context_window: {}",
            home.read(GROK_CONFIG)
        );
    }
    // And a fractional value is floored rather than refused.
    apply(
        "grok-build-settings",
        &json!({"baseUrl": "http://x", "apiKey": "k", "model": "m", "contextWindow": 1024.7}),
    )
    .await?;
    assert!(home.read(GROK_CONFIG).contains("context_window = 1024"));
    Ok(())
}

#[actix_web::test]
async fn cowork_bridges_a_local_plugin_through_this_routers_sse_endpoint() -> TestResult {
    // Given: a Cowork install with an applied config, and the user's own setting in it.
    let home = HomeGuard::new();
    home.seed(COWORK_META, r#"{"appliedId": "abc-123"}"#);
    home.seed(COWORK_CONFIG, r#"{"windowState": "keep me"}"#);

    // When: an apply names a remote plugin, a local one, and a custom URL.
    let (status, body) = apply(
        "cowork-settings",
        &json!({
            "baseUrl": "http://127.0.0.1:20128",
            "apiKey": "sk-cowork",
            "models": ["cc/opus"],
            "plugins": ["exa"],
            "localPlugins": ["browsermcp"],
            "customPlugins": [{"name": "mine", "url": "https://mcp.example.com/mcp"}],
        }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{body}");

    let config = home.read_json(COWORK_CONFIG);
    // Upstream's provider value is `gateway`, not `9router`.
    assert_eq!(config["inferenceProvider"], "gateway");
    assert_eq!(config["inferenceGatewayBaseUrl"], "http://127.0.0.1:20128");
    assert_eq!(config["inferenceModels"][0]["name"], "cc/opus");

    let servers = config["managedMcpServers"].as_array().expect("servers");
    assert_eq!(servers.len(), 3, "{config}");
    // The remote plugin keeps its own URL and gets an allow-policy under both the bare and the
    // prefixed tool name, since either is what arrives at call time.
    assert_eq!(servers[0]["url"], "https://mcp.exa.ai/mcp");
    assert_eq!(servers[0]["toolPolicy"]["web_search_exa"], "allow");
    assert_eq!(servers[0]["toolPolicy"]["exa-web_search_exa"], "allow");
    // The local stdio plugin is not spawned by Cowork: it is bridged through this router.
    assert_eq!(
        servers[1]["url"],
        "http://localhost:20128/api/mcp/browsermcp/sse"
    );
    assert_eq!(servers[1]["transport"], "sse");
    // And that bridge entry is what `isLocalDevMcpEnabled` exists for.
    assert_eq!(config["isLocalDevMcpEnabled"], true);
    assert_eq!(servers[2]["custom"], true);

    // The user's unrelated setting survived, where upstream replaces the whole config object.
    assert_eq!(config["windowState"], "keep me");
    // And none of upstream's other relax-security keys were written.
    for key in [
        "coworkEgressAllowedHosts",
        "isDesktopExtensionSignatureRequired",
        "disableNonessentialTelemetry",
    ] {
        assert!(config[key].is_null(), "{key} should not be written: {config}");
    }
    Ok(())
}

#[actix_web::test]
async fn an_unlisted_local_plugin_name_is_skipped_rather_than_spawned() -> TestResult {
    // The whitelist is the point: a name that is not on it must not become a bridge entry, because
    // the events service would refuse to spawn it and the config would name a dead endpoint.
    let home = HomeGuard::new();
    home.seed(COWORK_META, r#"{"appliedId": "abc-123"}"#);

    apply(
        "cowork-settings",
        &json!({
            "baseUrl": "http://x",
            "apiKey": "k",
            "models": ["m"],
            "plugins": [],
            "localPlugins": ["sh", "browsermcp; id", "../../bin/sh", "BrowserMCP"],
        }),
    )
    .await?;

    let config = home.read_json(COWORK_CONFIG);
    assert!(
        config["managedMcpServers"].is_null(),
        "no server should have been written: {config}"
    );
    Ok(())
}

#[actix_web::test]
async fn an_empty_plugins_array_is_not_replaced_by_the_defaults() -> TestResult {
    // An absent `plugins` means "the defaults"; an empty array means the user turned them all off.
    // Treating the two the same would re-enable servers the user just switched off.
    let home = HomeGuard::new();
    home.seed(COWORK_META, r#"{"appliedId": "abc-123"}"#);

    apply(
        "cowork-settings",
        &json!({"baseUrl": "http://x", "apiKey": "k", "models": ["m"], "plugins": []}),
    )
    .await?;
    assert!(home.read_json(COWORK_CONFIG)["managedMcpServers"].is_null());

    // Absent, by contrast, writes both defaults.
    apply(
        "cowork-settings",
        &json!({"baseUrl": "http://x", "apiKey": "k", "models": ["m"]}),
    )
    .await?;
    let servers = home.read_json(COWORK_CONFIG);
    assert_eq!(servers["managedMcpServers"].as_array().map(Vec::len), Some(2), "{servers}");
    Ok(())
}

#[actix_web::test]
async fn a_cowork_apply_without_an_applied_config_reports_it() -> TestResult {
    // Given: a Cowork whose config library has no `_meta.json`, which is the state of an install
    // that has never had a configuration applied.
    let home = HomeGuard::new();
    std::fs::create_dir_all(home.path(".config/Claude/configLibrary"))?;

    // When: an apply runs.
    let (status, body) = apply(
        "cowork-settings",
        &json!({"baseUrl": "http://x", "apiKey": "k", "models": ["m"]}),
    )
    .await?;

    // Then: it says what was missing and where it looked, and invents nothing. Upstream generates a
    // UUID and writes `configLibrary/<uuid>.json`, putting a file into an app's own data directory
    // that the app has no reason to ever read.
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    let message = body["error"].as_str().unwrap_or_default();
    assert!(message.contains("_meta.json"), "{message}");
    assert!(message.contains("configLibrary"), "{message}");
    let written: Vec<_> = std::fs::read_dir(home.path(".config/Claude/configLibrary"))?
        .filter_map(Result::ok)
        .collect();
    assert!(written.is_empty(), "nothing should have been created");
    Ok(())
}

#[actix_web::test]
async fn a_cowork_revoke_keeps_the_rest_of_the_config() -> TestResult {
    // Given: an applied config alongside settings that are nothing to do with routing.
    let home = HomeGuard::new();
    home.seed(COWORK_META, r#"{"appliedId": "abc-123"}"#);
    home.seed(
        COWORK_CONFIG,
        r#"{"inferenceProvider": "gateway", "inferenceGatewayApiKey": "sk",
            "managedMcpServers": [{"name": "exa"}], "windowState": "keep me",
            "workspacePrefs": {"theme": "dark"}}"#,
    );

    // When: the revoke runs.
    revoke("cowork-settings").await?;

    // Then: ours are gone and theirs are not. Upstream writes `{}` here, discarding every Cowork
    // setting the user has rather than the ones an apply wrote.
    let config = home.read_json(COWORK_CONFIG);
    assert!(config["inferenceProvider"].is_null(), "{config}");
    assert!(config["inferenceGatewayApiKey"].is_null(), "{config}");
    assert!(config["managedMcpServers"].is_null(), "{config}");
    assert_eq!(config["windowState"], "keep me");
    assert_eq!(config["workspacePrefs"]["theme"], "dark");
    Ok(())
}

#[actix_web::test]
async fn the_mcp_tool_probe_refuses_a_target_only_this_service_can_reach() -> TestResult {
    // Given: the probe route, which has this service fetch a URL the caller supplies.
    let _home = HomeGuard::new();

    // When: the URL names a loopback or private address — including this router's own services,
    // which a caller cannot reach directly.
    for url in [
        "http://127.0.0.1:20134/internal/v1/keys",
        "https://localhost/mcp",
        "https://169.254.169.254/latest/meta-data/",
        "https://10.0.0.5/mcp",
        "https://[::1]/mcp",
        "http://mcp.example.com/mcp",
        "file:///etc/passwd",
    ] {
        let (status, body) = call(
            Method::POST,
            "/api/cli-tools/cowork-mcp-tools",
            &json!({"url": url}),
        )
        .await?;

        // Then: it is refused with a reason, and no request is made. Upstream fetches whatever it
        // is handed, which makes the route a server-side request forgery pivot.
        assert_eq!(status, StatusCode::BAD_REQUEST, "{url} was not refused: {body}");
        assert!(
            body["error"].as_str().is_some_and(|error| !error.is_empty()),
            "{url}: {body}"
        );
        assert_eq!(body["tools"].as_array().map(Vec::len), Some(0), "{url}");
    }
    Ok(())
}

#[actix_web::test]
async fn the_mcp_tool_probe_still_requires_a_url() -> TestResult {
    let _home = HomeGuard::new();
    for body in [json!({}), json!({"url": ""}), json!({"url": "   "})] {
        let (status, response) =
            call(Method::POST, "/api/cli-tools/cowork-mcp-tools", &body).await?;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body} -> {response}");
    }
    Ok(())
}

#[actix_web::test]
async fn a_missing_required_field_is_refused_before_any_file_is_touched() -> TestResult {
    // Given: a fresh home.
    let home = HomeGuard::new();

    // When: an apply arrives without the model upstream requires.
    let (status, body) = apply(
        "codex-settings",
        &json!({"baseUrl": "http://127.0.0.1:20128", "apiKey": "sk"}),
    )
    .await?;

    // Then: it is a 400 and the disk is untouched. Validating after the first write would leave a
    // half-configured tool behind.
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"], "baseUrl, apiKey and model are required");
    assert!(!home.path(CODEX_CONFIG).exists(), "no config was written");
    assert!(!home.path(CODEX_AUTH).exists(), "no auth was written");
    Ok(())
}

#[actix_web::test]
async fn an_unparseable_config_is_refused_rather_than_replaced() -> TestResult {
    // Given: a config with a syntax error in it, which a hand-edited dotfile often has.
    let home = HomeGuard::new();
    let original = "{ not json at all";
    home.seed(CLAUDE_SETTINGS, original);

    // When: an apply runs.
    let (status, body) = apply(
        "claude-settings",
        &json!({"env": {"ANTHROPIC_BASE_URL": "http://127.0.0.1:20128"}}),
    )
    .await?;

    // Then: it fails and the file is exactly as it was. Defaulting to `{}` here would cost the
    // user everything else in their config because of one stray character.
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    let message = body["error"].as_str().unwrap_or_default();
    assert!(message.contains("left untouched"), "{message}");
    assert_eq!(home.read(CLAUDE_SETTINGS), original);
    Ok(())
}

#[actix_web::test]
async fn devin_stays_a_501_because_upstream_has_no_writer_either() -> TestResult {
    // Given: the one tool upstream exposes only a `GET` for.
    let _home = HomeGuard::new();

    // When: a mutation is attempted.
    let (status, body) = apply("devin-settings", &json!({"baseUrl": "http://x"})).await?;

    // Then: 501 with `unsupported`, which is parity rather than a gap. Inventing a config path for
    // devin would write a file that tool never reads.
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{body}");
    assert_eq!(body["unsupported"], true, "{body}");
    Ok(())
}

#[actix_web::test]
async fn an_unknown_tool_is_a_404_not_a_path() -> TestResult {
    // The `{tool}` segment reaches a filesystem write, so it must resolve through the table.
    let _home = HomeGuard::new();
    for segment in ["../../../etc/passwd", "nope-settings", ".."] {
        let (status, _body) = call(
            Method::POST,
            &format!("/api/cli-tools/{segment}"),
            &json!({"baseUrl": "http://x"}),
        )
        .await?;
        assert_eq!(status, StatusCode::NOT_FOUND, "{segment}");
    }
    Ok(())
}
