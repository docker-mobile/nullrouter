//! `GET /api/oauth/cursor/auto-import` against a synthetic home directory.
//!
//! The route reads a credential off this machine's own disk, which is the reason it is host-only at the
//! gateway. What this suite pins down is the part that decides *which* file gets opened, and the part
//! that decides what happens when it cannot be read — because both are reachable without a real Cursor
//! installation, and both are where a wrong answer would either miss a credential that is there or
//! attach one that came from a stale directory.
//!
//! The query itself needs `sqlite3` and a real database. What is asserted here instead is that a
//! missing `sqlite3` degrades to the manual instructions naming the file it found, rather than to a
//! failure — the fallback upstream also has, and the one a user actually hits.

#![allow(clippy::future_not_send)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "free helpers here are not #[test] fns, so clippy.toml's allow-expect-in-tests does \
              not cover them"
)]

use std::sync::Mutex;

use actix_web::{App, body::to_bytes, http::Method, http::StatusCode, test, web};
use serde_json::Value;

use nullrouter_api::{AppConfig, RuntimeClient, StateClient, configure};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn env_lock() -> &'static Mutex<()> {
    static LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// A home directory the route will search, plus the two binary overrides.
///
/// The guard is a field rather than a local binding: a `MutexGuard` held across an await is what
/// `clippy::await_holding_lock` exists to catch, and every case here awaits.
struct Home {
    _lock: std::sync::MutexGuard<'static, ()>,
    root: tempfile::TempDir,
    previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl Home {
    /// A home with no Cursor directory in it.
    fn empty() -> Self {
        let lock = env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let root = tempfile::tempdir().expect("tempdir");
        let mut previous = Vec::new();
        for name in [
            "NULLROUTER_CURSOR_HOME",
            "NULLROUTER_SQLITE_BIN",
            "NULLROUTER_CURSOR_BIN",
        ] {
            previous.push((name, std::env::var_os(name)));
        }
        // SAFETY: the lock above is held, so no other case in this binary reads or writes this while
        // it is being set.
        unsafe { std::env::set_var("NULLROUTER_CURSOR_HOME", root.path()) };
        // SAFETY: the lock above is still held. Pointed at nothing, so neither binary resolves unless
        // a case says otherwise. Without this the result would depend on the test machine's sqlite3.
        unsafe { std::env::set_var("NULLROUTER_SQLITE_BIN", root.path().join("no-sqlite3")) };
        // SAFETY: the lock above is still held.
        unsafe { std::env::set_var("NULLROUTER_CURSOR_BIN", root.path().join("no-cursor")) };
        Self {
            _lock: lock,
            root,
            previous,
        }
    }

    /// A home with a Cursor database file and a `cursor.desktop`, so the route gets past both checks.
    fn with_cursor() -> Self {
        let home = Self::empty();
        let database = home
            .root
            .path()
            .join(".config/Cursor/User/globalStorage/state.vscdb");
        std::fs::create_dir_all(database.parent().expect("a parent")).expect("create dirs");
        // Not a real SQLite file. Nothing in the paths under test parses it: what is asserted is which
        // path was chosen and what happens when it cannot be queried.
        std::fs::write(&database, b"not a database").expect("write database");

        let desktop = home
            .root
            .path()
            .join(".local/share/applications/cursor.desktop");
        std::fs::create_dir_all(desktop.parent().expect("a parent")).expect("create dirs");
        std::fs::write(&desktop, b"[Desktop Entry]\n").expect("write desktop entry");
        home
    }

    fn path(&self) -> &std::path::Path {
        self.root.path()
    }

    /// Install a tiny `sqlite3` stand-in that writes the rows the real command would print.
    ///
    /// It is a file owned by this test process and mode 0755, so it exercises the same verified-binary
    /// gate the real path does. Its shebang uses `/bin/sh` directly — `Run` clears `PATH`, deliberately.
    fn sqlite_that_prints(&self, stdout: &str) -> std::io::Result<()> {
        let binary = self.root.path().join("sqlite3");
        let script = format!("#!/bin/sh\nprintf '%s\\n' '{stdout}'\n");
        std::fs::write(&binary, script)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755))?;
        }
        // SAFETY: this fixture holds `env_lock` for its whole lifetime.
        unsafe { std::env::set_var("NULLROUTER_SQLITE_BIN", binary) };
        Ok(())
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        for (name, previous) in &self.previous {
            match previous {
                // SAFETY: the lock is still held until this guard finishes dropping.
                Some(value) => unsafe { std::env::set_var(name, value) },
                // SAFETY: as above.
                None => unsafe { std::env::remove_var(name) },
            }
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
        .uri("/api/oauth/cursor/auto-import")
        .to_request();
    let response = test::call_service(&app, request).await;
    let status = response.status();
    let body: Value = serde_json::from_slice(&to_bytes(response.into_body()).await?)?;
    Ok((status, body))
}

#[actix_rt::test]
async fn a_home_without_cursor_reports_every_path_it_checked() -> TestResult {
    // Given: no Cursor database anywhere. The answer names the paths searched, because "not found" with
    // no locations leaves a user who *does* have Cursor installed with nothing to compare against.
    let home = Home::empty();

    let (status, body) = auto_import().await?;

    // A question answered, not a request that failed — upstream's status too.
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.get("found"), Some(&Value::Bool(false)), "{body}");
    let error = body
        .get("error")
        .and_then(Value::as_str)
        .expect("an error string");
    assert!(error.contains(".config/Cursor"), "{error}");
    assert!(
        error.contains(&home.path().display().to_string()),
        "{error}"
    );
    Ok(())
}

#[actix_rt::test]
async fn a_leftover_config_without_the_editor_is_not_imported_from() -> TestResult {
    // A config directory outlives an uninstall. Importing out of one would attach a credential that
    // nothing on this machine can refresh, from a source the user has no way to identify later.
    let home = Home::empty();
    let database = home
        .path()
        .join(".config/Cursor/User/globalStorage/state.vscdb");
    std::fs::create_dir_all(database.parent().expect("a parent"))?;
    std::fs::write(&database, b"not a database")?;
    // No `cursor.desktop` and no resolvable `cursor` binary.

    let (status, body) = auto_import().await?;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.get("found"), Some(&Value::Bool(false)), "{body}");
    let error = body
        .get("error")
        .and_then(Value::as_str)
        .expect("an error string");
    assert!(
        error.contains("does not appear to be installed"),
        "the reason has to distinguish this from a missing file: {error}"
    );
    Ok(())
}

#[actix_rt::test]
async fn a_database_that_cannot_be_queried_falls_back_to_the_manual_path() -> TestResult {
    // The database is there and Cursor is installed, but `sqlite3` is not. The manual instructions in
    // `cursor/import` still work, so this names the file rather than presenting a dead end.
    let home = Home::with_cursor();

    let (status, body) = auto_import().await?;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.get("found"), Some(&Value::Bool(false)), "{body}");
    assert_eq!(body.get("manual"), Some(&Value::Bool(true)), "{body}");
    let path = body
        .get("dbPath")
        .and_then(Value::as_str)
        .expect("the path it found");
    assert!(path.ends_with("state.vscdb"), "{path}");
    assert!(
        path.starts_with(&home.path().display().to_string()),
        "{path}"
    );
    Ok(())
}

#[actix_rt::test]
async fn a_readable_database_yields_the_token_and_the_machine_id() -> TestResult {
    // The success path, with a stand-in for `sqlite3` that prints what the real one would. What is
    // under test is everything around the query: the path chosen, the row parsing, and the pairing of
    // the two values the panel needs.
    let home = Home::with_cursor();
    home.sqlite_that_prints(
        "cursorAuth/accessToken|\"ey.header.payload\"\nstorage.serviceMachineId|abc123",
    )?;

    let (status, body) = auto_import().await?;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.get("found"), Some(&Value::Bool(true)), "{body}");
    // Unwrapped from its JSON quoting: Cursor stores both forms, and the quotes would travel into an
    // Authorization header.
    assert_eq!(
        body.get("accessToken").and_then(Value::as_str),
        Some("ey.header.payload"),
        "{body}"
    );
    assert_eq!(
        body.get("machineId").and_then(Value::as_str),
        Some("abc123"),
        "{body}"
    );
    Ok(())
}

#[actix_rt::test]
async fn a_signed_out_cursor_is_not_a_partial_success() -> TestResult {
    // The database is readable but holds only one of the two values. Both are required by the import
    // that follows, so half a pair is reported as nothing found rather than offered to the user.
    let home = Home::with_cursor();
    home.sqlite_that_prints("storage.serviceMachineId|abc123")?;

    let (status, body) = auto_import().await?;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.get("found"), Some(&Value::Bool(false)), "{body}");
    assert_eq!(body.get("manual"), Some(&Value::Bool(true)), "{body}");
    assert!(
        !body.to_string().contains("abc123"),
        "an unusable half-pair should not be handed back: {body}"
    );
    Ok(())
}

#[actix_rt::test]
async fn the_database_is_opened_read_only() -> TestResult {
    // The one flag that can prevent damage. This database may belong to a *running* Cursor, and opening
    // it read-write replays a hot journal — rewriting state Cursor still believes it owns. Upstream's
    // CLI fallback omits it.
    let home = Home::with_cursor();
    let recorder = home.path().join("argv.txt");
    let binary = home.path().join("sqlite3");
    std::fs::write(
        &binary,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf 'cursorAuth/accessToken|t\\n'\n",
            recorder.display()
        ),
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755))?;
    }
    // SAFETY: the `Home` fixture holds `env_lock` for the whole case.
    unsafe { std::env::set_var("NULLROUTER_SQLITE_BIN", &binary) };

    let (status, _body) = auto_import().await?;
    assert_eq!(status, StatusCode::OK);

    let argv = std::fs::read_to_string(&recorder)?;
    assert!(
        argv.lines().any(|line| line == "-readonly"),
        "the query must be read-only: {argv}"
    );
    // And the path it opened is the one it reported finding, not something assembled later.
    assert!(argv.contains("state.vscdb"), "{argv}");
    Ok(())
}

#[actix_rt::test]
async fn no_credential_is_ever_in_a_not_found_answer() -> TestResult {
    // The whole reason this route is host-only is that its success answer carries a credential. Its
    // failure answers must not: a `dbPath` is a filename, and an error is a reason.
    let _home = Home::with_cursor();

    let (_status, body) = auto_import().await?;
    let rendered = body.to_string();

    assert!(
        !rendered.contains("accessToken"),
        "a not-found answer must not carry a token field: {rendered}"
    );
    assert!(
        !rendered.contains("machineId"),
        "nor a machine id: {rendered}"
    );
    Ok(())
}
