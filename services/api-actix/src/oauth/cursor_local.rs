//! Reading a Cursor credential off this machine's own disk.
//!
//! `GET /api/oauth/cursor/auto-import` saves the user the SQLite queries that `cursor/import`'s
//! instructions otherwise ask them to run by hand. It reads one file that belongs to the user running
//! this service, and returns what it found so the panel can offer it for import.
//!
//! # What this does not inherit
//!
//! Upstream reaches the same database two ways, and both are worth naming:
//!
//! | upstream | here |
//! |---|---|
//! | `require("better-sqlite3")` — a native module linked into the server | no in-process SQLite; the file is read by `sqlite3`, in its own process, under a deadline |
//! | its CLI fallback opens the database read-write | `-readonly`, so a live Cursor's journal is never recovered or rewritten by us |
//! | `execFileAsync("sqlite3", [dbPath, sql])` with `sqlite3` taken from `PATH` | the binary is resolved and its ownership and permissions checked before it runs ([`nullrouter_procctl::binary`]) |
//! | one spawn per key, up to five | one spawn: a single `key IN (...)` query, which is what upstream's own printed instructions tell the user to run |
//!
//! The read-write open is the one that can actually damage something. Opening a SQLite database
//! read-write replays a hot journal, so pointing a writable handle at the database of a *running*
//! Cursor can rewrite state that Cursor still believes it owns.
//!
//! # Why a subprocess rather than a SQLite crate
//!
//! Linking SQLite in would put a C library in the address space of the process that holds every
//! provider credential, to read two rows out of one file, once, when a user clicks a button. The
//! machinery to run a binary safely already exists here for the tunnel daemons.

use std::path::{Path, PathBuf};

use actix_web::{HttpResponse, http::StatusCode};
use nullrouter_procctl::{
    argv::Argv,
    binary::{BinarySpec, Executable, SYSTEM_BIN_DIRS},
    oneshot::Run,
};

use crate::responses;

/// The `sqlite3` command line, if the machine has one.
const SQLITE: BinarySpec = BinarySpec {
    name: "sqlite3",
    candidates: &[],
    env_override: "NULLROUTER_SQLITE_BIN",
    search_dirs: SYSTEM_BIN_DIRS,
};

/// The `cursor` command, used only to tell an installed editor from a leftover config directory.
const CURSOR: BinarySpec = BinarySpec {
    name: "cursor",
    candidates: &[],
    env_override: "NULLROUTER_CURSOR_BIN",
    search_dirs: SYSTEM_BIN_DIRS,
};

/// Overrides the home directory the candidate paths are built from, so the search can be tested.
///
/// Reading a real user's Cursor database in a test is not an option, and a search that has never been
/// exercised against a file that exists is a search that only proves it can fail.
const HOME_OVERRIDE_VAR: &str = "NULLROUTER_CURSOR_HOME";

/// Keys Cursor has stored the access token under, in the order upstream tries them.
const ACCESS_TOKEN_KEYS: [&str; 2] = ["cursorAuth/accessToken", "cursorAuth/token"];

/// Keys Cursor has stored the machine id under.
const MACHINE_ID_KEYS: [&str; 3] = [
    "storage.serviceMachineId",
    "storage.machineId",
    "telemetry.machineId",
];

/// Reading two rows out of a local file is not a slow operation. Ten seconds is upstream's own bound,
/// and it is generous: what it really guards against is a database locked by a busy Cursor.
const QUERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// A credential is a few kilobytes. This caps a pathological database, not a normal one.
const MAX_CAPTURE: usize = 64 * 1024;

/// The one query, built from compile-time constants only.
///
/// No caller-supplied text reaches it: the keys are the arrays above and the path is a candidate this
/// module constructed. `sqlite3` has no way to bind a parameter from argv, so the alternative to a
/// literal query would be interpolating a value — which is exactly what makes upstream's version worth
/// not copying, even though its keys happen to be constants too.
const QUERY: &str = "SELECT key, value FROM itemTable WHERE key IN \
                     ('cursorAuth/accessToken', 'cursorAuth/token', 'storage.serviceMachineId', \
                     'storage.machineId', 'telemetry.machineId')";

/// The home directory the search starts from.
fn home() -> Option<PathBuf> {
    if let Some(overridden) = std::env::var_os(HOME_OVERRIDE_VAR)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    {
        return Some(overridden);
    }
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Where Cursor keeps its database on this platform, most likely first.
///
/// The Windows arm is kept even though this service is unlikely to run there: the paths are part of
/// what upstream knows about Cursor, and a list that silently omits a platform reads as though that
/// platform had been checked and ruled out.
fn candidates() -> Vec<PathBuf> {
    let Some(home) = home() else {
        return Vec::new();
    };
    let suffix = "User/globalStorage/state.vscdb";

    if cfg!(target_os = "macos") {
        return vec![
            home.join(format!("Library/Application Support/Cursor/{suffix}")),
            home.join(format!(
                "Library/Application Support/Cursor - Insiders/{suffix}"
            )),
        ];
    }
    if cfg!(target_os = "windows") {
        let app_data = std::env::var_os("APPDATA")
            .filter(|value| !value.is_empty())
            .map_or_else(|| home.join("AppData/Roaming"), PathBuf::from);
        let local_app_data = std::env::var_os("LOCALAPPDATA")
            .filter(|value| !value.is_empty())
            .map_or_else(|| home.join("AppData/Local"), PathBuf::from);
        return vec![
            app_data.join(format!("Cursor/{suffix}")),
            app_data.join(format!("Cursor - Insiders/{suffix}")),
            local_app_data.join(format!("Cursor/{suffix}")),
            local_app_data.join(format!("Programs/Cursor/{suffix}")),
        ];
    }
    vec![
        home.join(format!(".config/Cursor/{suffix}")),
        home.join(format!(".config/cursor/{suffix}")),
    ]
}

/// The first candidate that exists and can be read.
fn locate() -> Option<PathBuf> {
    candidates()
        .into_iter()
        .find(|candidate| std::fs::File::open(candidate).is_ok())
}

/// Whether Cursor itself appears to be installed.
///
/// A config directory outlives an uninstall, so upstream checks for the editor before trusting what is
/// in it. Worth keeping: importing a token out of a stale directory would attach a credential that
/// nothing on this machine can refresh, and the user would have no idea where it came from.
fn cursor_is_installed() -> bool {
    if CURSOR.resolve(None).is_ok() {
        return true;
    }
    home().is_some_and(|home| {
        home.join(".local/share/applications/cursor.desktop")
            .is_file()
    })
}

/// A value as Cursor wrote it, unwrapped if it was stored as a JSON string.
///
/// Cursor has written both `"token"` and `token` into the same column across versions. A quoted value
/// used verbatim would carry its quotes into an `Authorization` header.
fn normalise(raw: &str) -> String {
    let value = raw.trim();
    match serde_json::from_str::<serde_json::Value>(value) {
        Ok(serde_json::Value::String(unwrapped)) => unwrapped,
        // Not a JSON string: either not JSON at all, or JSON that is not a string. Both mean the text
        // is the value.
        Ok(_) | Err(_) => value.to_owned(),
    }
}

/// The first key present in what the query returned, in the order the keys were listed.
fn first_of<'a>(rows: &'a [(String, String)], keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| {
        rows.iter()
            .find(|(found, value)| found == key && !value.is_empty())
            .map(|(_key, value)| value.as_str())
    })
}

/// `sqlite3`'s default output: one row per line, columns separated by a pipe.
///
/// Split on the first pipe only. A key never contains one, and a value that did would otherwise be
/// silently truncated at it.
fn parse_rows(stdout: &str) -> Vec<(String, String)> {
    stdout
        .lines()
        .filter_map(|line| line.split_once('|'))
        .map(|(key, value)| (key.trim().to_owned(), normalise(value)))
        .collect()
}

/// Ask `sqlite3` for the rows.
async fn query(sqlite: &Executable, database: &Path) -> Result<Vec<(String, String)>, String> {
    // `-readonly` is the difference that matters: this database may belong to a running Cursor, and a
    // writable open would replay its journal.
    let args = Argv::new()
        .flag("-readonly")
        .abs_path("database", database)
        .map_err(|error| error.to_string())?
        .word(QUERY)
        .into_vec();

    let output = Run {
        program: sqlite,
        args,
        timeout: QUERY_TIMEOUT,
        env: Vec::new(),
        secrets: &[],
        max_capture: MAX_CAPTURE,
    }
    .call()
    .await
    .map_err(|error| error.to_string())?;

    if output.success() {
        Ok(parse_rows(&output.stdout))
    } else {
        Err(output.failure_text().to_owned())
    }
}

/// A `found: false` answer with a reason the user can act on.
///
/// Upstream's status too: 200 with `found: false`. This is a question — is there a credential here —
/// and "no" is an answer to it rather than a failure of the request.
fn not_found(reason: impl Into<String>) -> HttpResponse {
    responses::json(
        StatusCode::OK,
        &serde_json::json!({ "found": false, "error": reason.into() }),
    )
}

/// `GET /api/oauth/cursor/auto-import`.
///
/// Host-only at the gateway, because it answers with a credential read off local disk: a session
/// cookie stolen from a browser somewhere else must not be able to ask this host for the token sitting
/// in its Cursor database.
pub(super) async fn cursor_auto_import() -> HttpResponse {
    let Some(database) = locate() else {
        let checked = candidates()
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        return not_found(format!(
            "Cursor database not found. Checked locations:\n{checked}\n\nMake sure Cursor IDE is \
             installed and opened at least once."
        ));
    };

    if !cursor_is_installed() {
        return not_found(
            "Cursor config files found but Cursor IDE does not appear to be installed. Skipping \
             auto-import.",
        );
    }

    let sqlite = match SQLITE.resolve(None) {
        Ok(sqlite) => sqlite,
        Err(error) => {
            // The manual path still works, so this names the file and lets the panel fall back to the
            // instructions rather than presenting a dead end.
            return responses::json(
                StatusCode::OK,
                &serde_json::json!({
                    "found": false,
                    "manual": true,
                    "dbPath": database.display().to_string(),
                    "error": format!(
                        "sqlite3 is not available, so the database could not be read here: {error}. \
                         The values can still be copied out by hand."
                    ),
                }),
            );
        }
    };

    let rows = match query(&sqlite, &database).await {
        Ok(rows) => rows,
        Err(error) => {
            return responses::json(
                StatusCode::OK,
                &serde_json::json!({
                    "found": false,
                    "manual": true,
                    "dbPath": database.display().to_string(),
                    "error": format!("The Cursor database could not be read: {error}"),
                }),
            );
        }
    };

    let token = first_of(&rows, &ACCESS_TOKEN_KEYS);
    let machine = first_of(&rows, &MACHINE_ID_KEYS);
    match (token, machine) {
        (Some(token), Some(machine)) => responses::json(
            StatusCode::OK,
            &serde_json::json!({
                "found": true,
                "accessToken": token,
                "machineId": machine,
            }),
        ),
        // The file is there and readable, but one of the two values is not in it — which is what a
        // signed-out Cursor looks like. Both are required, so half of a pair is not a partial success.
        (_missing_one, _or_the_other) => responses::json(
            StatusCode::OK,
            &serde_json::json!({
                "found": false,
                "manual": true,
                "dbPath": database.display().to_string(),
                "error": "The Cursor database has no stored credential. Sign in to Cursor and try \
                          again.",
            }),
        ),
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        reason = "a failed unwrap is the assertion in these"
    )]

    use super::{ACCESS_TOKEN_KEYS, MACHINE_ID_KEYS, first_of, normalise, parse_rows};

    #[test]
    fn a_json_quoted_value_is_unwrapped() {
        // Cursor has written both forms into the same column. Quotes carried into an Authorization
        // header would make every request fail with nothing to point at.
        assert_eq!(normalise("\"ey.token.sig\""), "ey.token.sig");
        assert_eq!(normalise("ey.token.sig"), "ey.token.sig");
        assert_eq!(normalise("  \"spaced\"  "), "spaced");
    }

    #[test]
    fn a_non_string_json_value_is_left_alone() {
        // A number or an object is not a credential. Keeping the text as written means the caller sees
        // what is actually in the database rather than a coerced version of it.
        assert_eq!(normalise("12345"), "12345");
        assert_eq!(normalise("{\"a\":1}"), "{\"a\":1}");
    }

    #[test]
    fn a_value_containing_a_pipe_survives_parsing() {
        // Split on the first separator only: sqlite3 does not quote its output, so a value with a pipe
        // in it would otherwise come back truncated at that pipe.
        let rows = parse_rows("storage.machineId|abc|def\ncursorAuth/token|tok");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows.first().map(|row| row.1.as_str()), Some("abc|def"));
        assert_eq!(rows.get(1).map(|row| row.1.as_str()), Some("tok"));
    }

    #[test]
    fn keys_are_preferred_in_the_order_they_are_listed() {
        // Both keys present: the first listed wins, because it is the one current Cursor writes.
        let rows = vec![
            ("cursorAuth/token".to_owned(), "older".to_owned()),
            ("cursorAuth/accessToken".to_owned(), "current".to_owned()),
        ];
        assert_eq!(first_of(&rows, &ACCESS_TOKEN_KEYS), Some("current"));
    }

    #[test]
    fn an_empty_value_is_not_a_value() {
        // A row that exists with an empty string is a signed-out Cursor, not a credential.
        let rows = vec![
            ("storage.serviceMachineId".to_owned(), String::new()),
            ("storage.machineId".to_owned(), "fallback".to_owned()),
        ];
        assert_eq!(first_of(&rows, &MACHINE_ID_KEYS), Some("fallback"));
    }

    #[test]
    fn a_row_without_a_separator_is_ignored() {
        // sqlite3 prints nothing but rows, but a warning on stdout would otherwise become a key.
        assert!(parse_rows("some notice with no pipe\n").is_empty());
    }
}
