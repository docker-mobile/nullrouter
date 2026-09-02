//! Writing a config file back without destroying what the user had in it.
//!
//! Every apply here is a read-merge-write, never a replace. A user's `~/.claude/settings.json`
//! holds their own permissions, hooks and MCP servers next to the two keys this router cares
//! about; a tool that wrote a fresh file with only its own keys would silently delete the rest.
//!
//! # Atomicity
//!
//! Writes go to a temporary file in the same directory and are then renamed over the target. A
//! partial write is the failure that matters: a truncated `settings.json` stops the tool from
//! starting at all, and it happens exactly when the write is interrupted, which is exactly when
//! nobody is watching. `rename` within a directory is atomic on every platform this targets.
//!
//! # Backups
//!
//! The previous contents are copied to `<name>.9router-backup` before the first modification, so
//! there is something to go back to. Only once: a second apply must not overwrite the backup of
//! the user's original file with a backup of our own output.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::spec::Format;

/// What went wrong, in words a user can act on.
#[derive(Debug)]
pub(crate) enum WriteError {
    /// `$HOME` is unset, so there is no path to write to.
    NoHome,
    /// The config path could not be worked out, and guessing one would mean writing a file into a
    /// directory some other program owns. Cowork is the case: its filename comes out of a
    /// `_meta.json` that only exists once the app has been set up.
    NoConfigPath {
        detail: String,
    },
    /// The file exists but does not parse, so merging into it would mean discarding it.
    Unparseable {
        path: PathBuf,
        detail: String,
    },
    Io {
        path: PathBuf,
        detail: String,
    },
    Serialise(String),
}

impl WriteError {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::NoHome => "Cannot locate the home directory: $HOME is unset or empty.".to_owned(),
            Self::NoConfigPath { detail } => detail.clone(),
            Self::Unparseable { path, detail } => format!(
                "{} exists but could not be parsed ({detail}), so it was left untouched. Fix or \
                 move that file and try again — overwriting it would discard whatever is in it.",
                path.display()
            ),
            Self::Io { path, detail } => format!("Could not write {}: {detail}", path.display()),
            Self::Serialise(detail) => format!("Could not serialise the new config: {detail}"),
        }
    }
}

fn io_error(path: &Path, error: &std::io::Error) -> WriteError {
    WriteError::Io {
        path: path.to_owned(),
        detail: error.to_string(),
    }
}

/// Read a config for modification.
///
/// An absent file yields `default_for`, so a first-time apply creates one. An unparseable file is
/// an error, *not* an empty default — the difference matters: defaulting would mean a stray comma
/// in the user's config costs them the whole file.
pub(crate) fn read_for_merge(path: &Path, format: Format) -> Result<Value, WriteError> {
    match std::fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(default_for(format)),
        Ok(text) => {
            super::detect::parse_config(&text, format).map_err(|detail| WriteError::Unparseable {
                path: path.to_owned(),
                detail,
            })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(default_for(format)),
        Err(error) => Err(io_error(path, &error)),
    }
}

/// The empty document for a format.
fn default_for(format: Format) -> Value {
    match format {
        // Copilot's config is a top-level array, but it is the only one, and it tolerates being
        // handed an object it then replaces — so the object default is right for the rest and the
        // copilot writer starts from an array explicitly.
        Format::Json | Format::Toml => Value::Object(serde_json::Map::new()),
        Format::DotEnv | Format::YamlBlock | Format::TomlText => Value::String(String::new()),
    }
}

/// Serialise a document back to its format's text.
pub(crate) fn serialise(value: &Value, format: Format) -> Result<String, WriteError> {
    match format {
        Format::Json => serde_json::to_string_pretty(value)
            .map(|text| text + "\n")
            .map_err(|error| WriteError::Serialise(error.to_string())),
        Format::Toml => {
            // Through `toml::Value` so the JSON document this port carries internally becomes a
            // TOML document. Nulls have no TOML representation, so they are dropped rather than
            // failing the whole write.
            let pruned = prune_nulls(value.clone());
            let as_toml: toml::Value = serde_json::from_value::<toml::Value>(pruned)
                .map_err(|error| WriteError::Serialise(error.to_string()))?;
            toml::to_string_pretty(&as_toml)
                .map_err(|error| WriteError::Serialise(error.to_string()))
        }
        // Carried as text throughout, so there is nothing to serialise.
        Format::DotEnv | Format::YamlBlock | Format::TomlText => {
            Ok(value.as_str().unwrap_or_default().to_owned())
        }
    }
}

/// Drop null-valued keys, which TOML cannot represent.
fn prune_nulls(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .filter(|(_, item)| !item.is_null())
                .map(|(key, item)| (key, prune_nulls(item)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .filter(|item| !item.is_null())
                .map(prune_nulls)
                .collect(),
        ),
        other => other,
    }
}

/// Write `text` to `path`, creating parents, backing up once, and replacing atomically.
pub(crate) fn write_atomically(path: &Path, text: &str) -> Result<(), WriteError> {
    let directory = path.parent().ok_or_else(|| WriteError::Io {
        path: path.to_owned(),
        detail: "path has no parent directory".to_owned(),
    })?;
    std::fs::create_dir_all(directory).map_err(|error| io_error(directory, &error))?;
    back_up_once(path)?;

    // A fixed sibling name rather than a random one: two concurrent applies to the same tool are
    // not a case worth supporting, and a predictable leftover is easier for a user to find and
    // delete than a random one.
    let temporary = path.with_extension(format!(
        "{}.9router-tmp",
        path.extension()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or("")
    ));
    {
        let mut file =
            std::fs::File::create(&temporary).map_err(|error| io_error(&temporary, &error))?;
        file.write_all(text.as_bytes())
            .map_err(|error| io_error(&temporary, &error))?;
        // Flushed and synced before the rename: the rename is atomic with respect to the
        // directory entry, but without the sync the *contents* can still be missing after a crash.
        file.sync_all()
            .map_err(|error| io_error(&temporary, &error))?;
    }
    std::fs::rename(&temporary, path).map_err(|error| {
        // Leave nothing behind on failure.
        let _ = std::fs::remove_file(&temporary);
        io_error(path, &error)
    })
}

/// The name of the backup kept beside a config file.
pub(crate) fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".9router-backup");
    path.with_file_name(name)
}

/// Copy the current contents aside, unless a backup already exists.
///
/// Once only. A second apply overwriting the backup would replace the user's original file with a
/// copy of our own previous output, which is the one moment the backup stops being useful.
fn back_up_once(path: &Path) -> Result<(), WriteError> {
    if !path.exists() {
        return Ok(());
    }
    let backup = backup_path(path);
    if backup.exists() {
        return Ok(());
    }
    std::fs::copy(path, &backup)
        .map(|_| ())
        .map_err(|error| io_error(&backup, &error))
}

/// Upsert `KEY=value` in a `.env` file, leaving comments and other lines alone.
pub(crate) fn upsert_env(text: &str, key: &str, value: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let assignment = format!("{key}={value}");
    let existing = lines.iter().position(|line| {
        line.split_once('=')
            .is_some_and(|(name, _)| name.trim() == key)
    });
    match existing {
        Some(index) => {
            if let Some(slot) = lines.get_mut(index) {
                *slot = assignment;
            }
        }
        None => lines.push(assignment),
    }
    let mut joined = lines.join("\n");
    if !joined.is_empty() {
        joined.push('\n');
    }
    joined
}

/// Remove `KEY=...` from a `.env` file.
pub(crate) fn remove_env(text: &str, key: &str) -> String {
    let kept: Vec<&str> = text
        .lines()
        .filter(|line| {
            !line
                .split_once('=')
                .is_some_and(|(name, _)| name.trim() == key)
        })
        .collect();
    let mut joined = kept.join("\n");
    if !joined.is_empty() {
        joined.push('\n');
    }
    joined
}

/// Set a nested key, creating intermediate objects.
///
/// Any non-object encountered on the way is replaced, because there is no meaningful merge of
/// `{"env": 5}` with `env.KEY = "x"`.
pub(crate) fn set_path(document: &mut Value, path: &[&str], value: Value) {
    let Some((last, leading)) = path.split_last() else {
        *document = value;
        return;
    };
    let mut current = document;
    for key in leading {
        if !current.is_object() {
            *current = Value::Object(serde_json::Map::new());
        }
        // Get-or-insert and descend in one step. Two steps would hold a borrow of `current` while
        // reassigning it, and the `unwrap_or(current)` that shape invites is worse than it looks:
        // it silently stops descending and writes the key at the wrong level.
        let Some(map) = current.as_object_mut() else {
            return;
        };
        current = map
            .entry((*key).to_owned())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
    }
    if !current.is_object() {
        *current = Value::Object(serde_json::Map::new());
    }
    if let Some(map) = current.as_object_mut() {
        map.insert((*last).to_owned(), value);
    }
}

/// Remove a nested key, and any object it leaves empty above it.
pub(crate) fn remove_path(document: &mut Value, path: &[&str]) {
    let Some((last, leading)) = path.split_last() else {
        return;
    };
    let mut current = &mut *document;
    for key in leading {
        match current.get_mut(*key) {
            Some(next) => current = next,
            None => return,
        }
    }
    if let Some(map) = current.as_object_mut() {
        map.remove(*last);
    }
    // Upstream deletes an `env` object once it is empty; the same tidy-up is applied to whatever
    // level was touched, so a revoke leaves the file as it found it.
    prune_empty_objects(document, leading);
}

fn prune_empty_objects(document: &mut Value, path: &[&str]) {
    let Some((last, leading)) = path.split_last() else {
        return;
    };
    let mut current = &mut *document;
    for key in leading {
        match current.get_mut(*key) {
            Some(next) => current = next,
            None => return,
        }
    }
    let empty = current
        .get(*last)
        .and_then(Value::as_object)
        .is_some_and(serde_json::Map::is_empty);
    if empty {
        if let Some(map) = current.as_object_mut() {
            map.remove(*last);
        }
        prune_empty_objects(document, leading);
    }
}

/// Append `/v1` unless it is already there, the way every upstream writer does.
pub(crate) fn with_v1(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/v1") {
        trimmed.to_owned()
    } else {
        format!("{trimmed}/v1")
    }
}

/// Strip a trailing `/v1`, for the tools that want the bare origin.
pub(crate) fn without_v1(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    trimmed.strip_suffix("/v1").unwrap_or(trimmed).to_owned()
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::expect_used,
    clippy::panic,
    reason = "test assertions read clearer with expect than with error plumbing"
)]
mod tests {
    use super::{
        Format, backup_path, read_for_merge, remove_env, remove_path, serialise, set_path,
        upsert_env, with_v1, without_v1, write_atomically,
    };
    use serde_json::json;

    #[test]
    fn a_merge_keeps_the_users_own_keys() {
        // The property the whole module exists for.
        let mut document = json!({
            "permissions": {"allow": ["Bash"]},
            "env": {"MY_OWN": "keep me"},
        });
        set_path(
            &mut document,
            &["env", "ANTHROPIC_BASE_URL"],
            json!("http://x/v1"),
        );
        assert_eq!(document["permissions"]["allow"][0], "Bash");
        assert_eq!(document["env"]["MY_OWN"], "keep me");
        assert_eq!(document["env"]["ANTHROPIC_BASE_URL"], "http://x/v1");
    }

    #[test]
    fn set_path_creates_missing_levels() {
        let mut document = json!({});
        set_path(&mut document, &["a", "b", "c"], json!(1));
        assert_eq!(document, json!({"a": {"b": {"c": 1}}}));
    }

    #[test]
    fn set_path_replaces_a_non_object_on_the_way() {
        // No sensible merge exists, and panicking or silently doing nothing would both be worse.
        let mut document = json!({"env": 5});
        set_path(&mut document, &["env", "KEY"], json!("v"));
        assert_eq!(document, json!({"env": {"KEY": "v"}}));
    }

    #[test]
    fn remove_path_tidies_up_the_object_it_empties() {
        // Upstream deletes `env` when the last key goes, and the file should come back to the
        // shape it had before an apply.
        let mut document = json!({"env": {"ANTHROPIC_BASE_URL": "x"}, "other": 1});
        remove_path(&mut document, &["env", "ANTHROPIC_BASE_URL"]);
        assert_eq!(document, json!({"other": 1}));
    }

    #[test]
    fn remove_path_keeps_an_object_that_still_has_keys() {
        let mut document = json!({"env": {"ANTHROPIC_BASE_URL": "x", "MINE": "y"}});
        remove_path(&mut document, &["env", "ANTHROPIC_BASE_URL"]);
        assert_eq!(document, json!({"env": {"MINE": "y"}}));
    }

    #[test]
    fn remove_path_on_a_missing_key_is_a_no_op() {
        let mut document = json!({"other": 1});
        remove_path(&mut document, &["env", "NOPE"]);
        assert_eq!(document, json!({"other": 1}));
    }

    #[test]
    fn an_unparseable_file_is_refused_rather_than_replaced() {
        // The important failure. Defaulting to `{}` here would mean a stray character in the
        // user's config costs them everything else in it.
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("settings.json");
        std::fs::write(&path, "{ this is not json").expect("write");

        let error = read_for_merge(&path, Format::Json).expect_err("should refuse");
        let message = error.message();
        assert!(message.contains("left untouched"), "{message}");
        assert!(message.contains("settings.json"), "{message}");
    }

    #[test]
    fn an_absent_or_empty_file_starts_from_an_empty_document() {
        let directory = tempfile::tempdir().expect("tempdir");
        let missing = directory.path().join("nope.json");
        assert_eq!(
            read_for_merge(&missing, Format::Json).expect("absent is fine"),
            json!({})
        );

        let blank = directory.path().join("blank.json");
        std::fs::write(&blank, "   \n").expect("write");
        assert_eq!(
            read_for_merge(&blank, Format::Json).expect("empty is fine"),
            json!({})
        );
    }

    #[test]
    fn a_write_creates_parents_and_replaces_atomically() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("deep").join("nested").join("c.json");
        write_atomically(&path, "{\"a\":1}\n").expect("write");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "{\"a\":1}\n");
        // No temporary left behind.
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().expect("parent"))
            .expect("read_dir")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains("9router-tmp"))
            .collect();
        assert!(leftovers.is_empty(), "left a temp file: {leftovers:?}");
    }

    #[test]
    fn the_first_write_backs_up_and_later_writes_do_not_clobber_it() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("settings.json");
        std::fs::write(&path, "ORIGINAL").expect("write");

        write_atomically(&path, "FIRST").expect("first write");
        let backup = backup_path(&path);
        assert_eq!(
            std::fs::read_to_string(&backup).expect("backup exists"),
            "ORIGINAL"
        );

        write_atomically(&path, "SECOND").expect("second write");
        assert_eq!(
            std::fs::read_to_string(&backup).expect("backup still there"),
            "ORIGINAL",
            "the backup must stay the user's original, not our previous output"
        );
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "SECOND");
    }

    #[test]
    fn no_backup_is_made_for_a_file_that_did_not_exist() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("new.json");
        write_atomically(&path, "X").expect("write");
        assert!(!backup_path(&path).exists(), "there was nothing to back up");
    }

    #[test]
    fn env_upsert_replaces_in_place_and_leaves_comments() {
        let original = "# my notes\nOTHER=1\nJCODE_9ROUTER_API_KEY=old\nTRAILING=2\n";
        let updated = upsert_env(original, "JCODE_9ROUTER_API_KEY", "new");
        assert_eq!(
            updated,
            "# my notes\nOTHER=1\nJCODE_9ROUTER_API_KEY=new\nTRAILING=2\n"
        );
    }

    #[test]
    fn env_upsert_appends_when_absent() {
        assert_eq!(upsert_env("A=1\n", "B", "2"), "A=1\nB=2\n");
        assert_eq!(upsert_env("", "B", "2"), "B=2\n");
    }

    #[test]
    fn env_remove_takes_only_the_named_key() {
        let original = "# c\nA=1\nB=2\n";
        assert_eq!(remove_env(original, "A"), "# c\nB=2\n");
        assert_eq!(remove_env(original, "MISSING"), original);
    }

    #[test]
    fn env_keys_are_matched_whole_not_by_prefix() {
        // `A` must not match `AB`.
        let original = "A=1\nAB=2\n";
        assert_eq!(remove_env(original, "A"), "AB=2\n");
    }

    #[test]
    fn toml_round_trips_through_the_json_document() {
        let mut document = super::read_for_merge(
            std::path::Path::new("/nonexistent-for-this-test"),
            Format::Toml,
        )
        .expect("absent is fine");
        set_path(&mut document, &["model_provider"], json!("9router"));
        set_path(
            &mut document,
            &["model_providers", "9router", "base_url"],
            json!("http://127.0.0.1:20128/v1"),
        );
        let text = serialise(&document, Format::Toml).expect("serialise");
        assert!(text.contains("model_provider = \"9router\""), "{text}");
        assert!(text.contains("[model_providers.9router]"), "{text}");
        // And the marker upstream greps for matches what was written, which is the whole point of
        // writing it in this shape.
        let tool = crate::cli_tools::spec::Tool::parse("codex").expect("codex");
        let crate::cli_tools::spec::Marker::Text(check) = tool.marker else {
            panic!("codex has a text marker")
        };
        assert!(
            check(&text),
            "the written file must satisfy the marker: {text}"
        );
    }

    #[test]
    fn nulls_are_dropped_rather_than_failing_a_toml_write() {
        let document = json!({"kept": 1, "dropped": null, "nested": {"gone": null, "here": "x"}});
        let text = serialise(&document, Format::Toml).expect("serialise");
        assert!(text.contains("kept"), "{text}");
        assert!(!text.contains("dropped"), "{text}");
        assert!(text.contains("here"), "{text}");
        assert!(!text.contains("gone"), "{text}");
    }

    #[test]
    fn the_v1_suffix_helpers_are_idempotent() {
        assert_eq!(
            with_v1("http://127.0.0.1:20128"),
            "http://127.0.0.1:20128/v1"
        );
        assert_eq!(
            with_v1("http://127.0.0.1:20128/v1"),
            "http://127.0.0.1:20128/v1"
        );
        assert_eq!(
            with_v1("http://127.0.0.1:20128/"),
            "http://127.0.0.1:20128/v1"
        );
        assert_eq!(
            with_v1("http://127.0.0.1:20128/v1/"),
            "http://127.0.0.1:20128/v1"
        );

        assert_eq!(
            without_v1("http://127.0.0.1:20128/v1"),
            "http://127.0.0.1:20128"
        );
        assert_eq!(
            without_v1("http://127.0.0.1:20128"),
            "http://127.0.0.1:20128"
        );
        assert_eq!(
            without_v1("http://127.0.0.1:20128/v1/"),
            "http://127.0.0.1:20128"
        );
    }

    #[test]
    fn json_is_written_with_a_trailing_newline() {
        // Not cosmetic: these files sit in dotfile directories people keep in git, and a missing
        // newline shows up as a diff on every apply.
        let text = serialise(&json!({"a": 1}), Format::Json).expect("serialise");
        assert!(text.ends_with('\n'), "{text:?}");
    }
}
