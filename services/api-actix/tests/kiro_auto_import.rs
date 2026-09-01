//! `GET /api/oauth/kiro/auto-import` against a synthetic AWS SSO cache.
//!
//! The success answer carries a refresh token and, for an IDC login, client credentials — all read from
//! the host's own disk. The gateway makes this route host-only. These cases exercise the selection
//! rules that make that boundary worth having: only Kiro-marked tokens, Kiro's own filename first, a
//! safe sibling lookup for the client registration, and an ARN normalised for the runtime gateway.

#![allow(clippy::future_not_send)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "free helpers here are not #[test] fns, so clippy.toml's allow-expect-in-tests does \
              not cover them"
)]

use std::sync::Mutex;

use actix_web::{App, body::to_bytes, http::Method, http::StatusCode, test, web};
use serde_json::{Value, json};

use nullrouter_api::{AppConfig, RuntimeClient, StateClient, configure};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const TOKEN: &str = "aorAAAAAGkiro-refresh-token";

fn env_lock() -> &'static Mutex<()> {
    static LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// A synthetic home, held together with the environment lock for the whole async case.
struct Home {
    _lock: std::sync::MutexGuard<'static, ()>,
    root: tempfile::TempDir,
    previous: Option<std::ffi::OsString>,
}

impl Home {
    fn empty() -> Self {
        let lock = env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let root = tempfile::tempdir().expect("tempdir");
        let previous = std::env::var_os("NULLROUTER_KIRO_HOME");
        // SAFETY: the lock above is held, so no other case in this binary reads or writes it here.
        unsafe { std::env::set_var("NULLROUTER_KIRO_HOME", root.path()) };
        Self {
            _lock: lock,
            root,
            previous,
        }
    }

    fn cache(&self) -> std::path::PathBuf {
        self.root.path().join(".aws/sso/cache")
    }

    fn write_cache(&self, name: &str, document: &Value) -> std::io::Result<()> {
        std::fs::create_dir_all(self.cache())?;
        std::fs::write(self.cache().join(name), serde_json::to_vec(document)?)
    }

    fn write_profile(&self, arn: &str) -> std::io::Result<()> {
        let path = self
            .root
            .path()
            .join(".config/Kiro/User/globalStorage/kiro.kiroagent/profile.json");
        std::fs::create_dir_all(path.parent().expect("parent"))?;
        std::fs::write(path, serde_json::to_vec(&json!({ "arn": arn }))?)
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        match self.previous.take() {
            // SAFETY: the lock is still held until this guard finishes dropping.
            Some(value) => unsafe { std::env::set_var("NULLROUTER_KIRO_HOME", value) },
            // SAFETY: as above.
            None => unsafe { std::env::remove_var("NULLROUTER_KIRO_HOME") },
        }
    }
}

async fn auto_import() -> TestResult<(StatusCode, Value)> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(AppConfig::new("0.5.20")))
            .app_data(web::Data::new(StateClient::new("127.0.0.1:1")))
            .app_data(web::Data::new(RuntimeClient::new("127.0.0.1:1")))
            .app_data(web::Data::new(nullrouter_api::TunnelManager::new()))
            .configure(configure),
    )
    .await;
    let request = test::TestRequest::default()
        .method(Method::GET)
        .uri("/api/oauth/kiro/auto-import")
        .to_request();
    let response = test::call_service(&app, request).await;
    let status = response.status();
    let body: Value = serde_json::from_slice(&to_bytes(response.into_body()).await?)?;
    Ok((status, body))
}

#[actix_rt::test]
async fn a_missing_cache_is_an_actionable_not_found_answer() -> TestResult {
    let _home = Home::empty();

    let (status, body) = auto_import().await?;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.get("found"), Some(&Value::Bool(false)), "{body}");
    assert_eq!(
        body.get("error").and_then(Value::as_str),
        Some("AWS SSO cache not found. Please login to Kiro IDE first."),
        "{body}"
    );
    Ok(())
}

#[actix_rt::test]
async fn only_a_kiro_marked_token_is_imported() -> TestResult {
    // The cache belongs to the AWS CLI, not Kiro. An unrelated session in it must not become a Kiro
    // connection just because it happened to be found first.
    let home = Home::empty();
    home.write_cache("other.json", &json!({ "refreshToken": "aws-cli-token" }))?;

    let (status, body) = auto_import().await?;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.get("found"), Some(&Value::Bool(false)), "{body}");
    assert!(
        !body.to_string().contains("aws-cli-token"),
        "a foreign AWS token must never be returned: {body}"
    );
    Ok(())
}

#[actix_rt::test]
async fn kiros_named_file_wins_over_another_matching_cache_entry() -> TestResult {
    // Defined preference, not directory iteration: a user who signs into Kiro twice must get the
    // current named entry, rather than whichever the filesystem happens to enumerate first.
    let home = Home::empty();
    home.write_cache("z-old.json", &json!({ "refreshToken": "aorAAAAAGold" }))?;
    home.write_cache(
        "kiro-auth-token.json",
        &json!({ "refreshToken": TOKEN, "region": "eu-west-1", "authMethod": "idc" }),
    )?;

    let (status, body) = auto_import().await?;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.get("found"), Some(&Value::Bool(true)), "{body}");
    assert_eq!(
        body.get("refreshToken").and_then(Value::as_str),
        Some(TOKEN)
    );
    assert_eq!(
        body.get("source").and_then(Value::as_str),
        Some("kiro-auth-token.json")
    );
    assert_eq!(
        body.get("region").and_then(Value::as_str),
        Some("eu-west-1")
    );
    assert_eq!(body.get("authMethod").and_then(Value::as_str), Some("idc"));
    Ok(())
}

#[actix_rt::test]
async fn idc_client_credentials_are_resolved_only_from_a_safe_sibling_file() -> TestResult {
    // An IDC token cannot be renewed without its client credentials. `clientIdHash` must name a simple
    // sibling basename, never a path: accepting `../` here would turn a cache lookup into a read of any
    // file the service can access.
    let home = Home::empty();
    home.write_cache(
        "kiro-auth-token.json",
        &json!({ "refreshToken": TOKEN, "clientIdHash": "registration123" }),
    )?;
    home.write_cache(
        "registration123.json",
        &json!({ "clientId": "client-abc", "clientSecret": "secret-xyz" }),
    )?;

    let (status, body) = auto_import().await?;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body.get("clientId").and_then(Value::as_str),
        Some("client-abc")
    );
    assert_eq!(
        body.get("clientSecret").and_then(Value::as_str),
        Some("secret-xyz")
    );

    // A second synthetic home proves a traversal-looking hash resolves neither credential.
    drop(home);
    let unsafe_home = Home::empty();
    unsafe_home.write_cache(
        "kiro-auth-token.json",
        &json!({ "refreshToken": TOKEN, "clientIdHash": "../../not-a-sibling" }),
    )?;
    let (_status, unsafe_body) = auto_import().await?;
    assert_eq!(
        unsafe_body.get("clientId"),
        Some(&Value::Null),
        "{unsafe_body}"
    );
    assert_eq!(
        unsafe_body.get("clientSecret"),
        Some(&Value::Null),
        "{unsafe_body}"
    );
    Ok(())
}

#[actix_rt::test]
async fn profile_arn_is_normalised_for_kiros_runtime_gateway() -> TestResult {
    // The login region stays as it is, but Kiro's runtime gateway requires us-east-1 in this *one ARN*.
    // This is upstream's surprising but necessary normalisation.
    let home = Home::empty();
    home.write_cache("kiro-auth-token.json", &json!({ "refreshToken": TOKEN }))?;
    home.write_profile("arn:aws:codewhisperer:eu-west-1:123456789012:profile/ABC")?;

    let (status, body) = auto_import().await?;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body.get("profileArn").and_then(Value::as_str),
        Some("arn:aws:codewhisperer:us-east-1:123456789012:profile/ABC"),
        "{body}"
    );
    Ok(())
}

#[actix_rt::test]
async fn a_large_cache_file_is_skipped_instead_of_read_whole() -> TestResult {
    // Cache files don't need to be unbounded input. A 256 KiB+ document is not a token record, and it
    // belongs to an external tool — skipping it makes a malformed cache unable to consume memory here.
    let home = Home::empty();
    std::fs::create_dir_all(home.cache())?;
    std::fs::write(
        home.cache().join("kiro-auth-token.json"),
        vec![b'x'; 256 * 1024 + 1],
    )?;

    let (status, body) = auto_import().await?;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.get("found"), Some(&Value::Bool(false)), "{body}");
    Ok(())
}
