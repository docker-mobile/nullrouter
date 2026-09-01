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
        unsafe {
            std::env::set_var("HOME", directory.path());
            std::env::set_var("XDG_CONFIG_HOME", directory.path().join(".config"));
        }
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
        // SAFETY: the lock is still held until this guard finishes dropping.
        unsafe {
            match self.previous_home.take() {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
            match self.previous_xdg.take() {
                Some(value) => std::env::set_var("XDG_CONFIG_HOME", value),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
        }
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
