//! Is this tool installed, and what does its config say?
//!
//! Replaces a hardcoded `installed: false`. That value was not a refusal but a false claim: a user
//! with Claude Code installed was told they did not have it, and the dashboard offered to install
//! something already present.
//!
//! # `PATH` lookup instead of spawning `which`
//!
//! Upstream runs `which <tool>` / `where <tool>` through a shell. Walking `PATH` here does the
//! same job without a process spawn per tool — fourteen spawns per `all-statuses` call is a lot of
//! forking for a dashboard poll — and without passing a name to a shell at all. The names come
//! from a fixed table, so this is not a difference in safety so much as one fewer thing to argue
//! about.
//!
//! The fallback matches upstream: a tool whose binary is not on `PATH` but whose config file
//! exists counts as installed, because that is the case where a user has it installed somewhere
//! unusual and still wants to configure it.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::spec::{Format, Marker, Tool};

/// What we could learn about one tool.
#[derive(Debug, Clone)]
pub(crate) struct Status {
    pub(crate) installed: bool,
    /// How we know: the binary, or the config file. Reported so a user can tell why a tool shows
    /// as installed when they think it is not.
    pub(crate) source: Option<String>,
    pub(crate) has_router: bool,
    /// The parsed config, when there is one and it parsed. `null` when the file is absent.
    pub(crate) settings: Value,
    /// The path inspected, even when it does not exist — a user fixing a problem needs to know
    /// which file was read.
    pub(crate) config_path: Option<PathBuf>,
    /// Set when the file exists but could not be parsed. Distinct from "absent": upstream treats
    /// an unparseable file as "no config" so the UI does not read a 500 as "not installed", and
    /// this reports the same but says so.
    pub(crate) parse_error: Option<String>,
}

impl Status {
    /// The answer for a tool we know nothing good about.
    fn absent() -> Self {
        Self {
            installed: false,
            source: None,
            has_router: false,
            settings: Value::Null,
            config_path: None,
            parse_error: None,
        }
    }
}

/// Look up a bare command name on `PATH`, returning where it was found.
///
/// Only entries that are files are accepted. The executable bit is not checked: it would need a
/// `unix`-gated import for a check that adds nothing here, since a non-executable file with a
/// tool's exact name in a `PATH` directory is not a case worth distinguishing.
fn on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|directory| !directory.as_os_str().is_empty())
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

/// Read a config file, tolerating the shapes upstream tolerates.
///
/// Returns `Ok(Value::Null)` when the file is absent, `Err` when it exists but will not parse.
fn read_config(path: &Path, format: Format) -> Result<Value, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Value::Null),
        Err(error) => return Err(error.to_string()),
    };
    parse_config(&text, format)
}

/// Parse config text according to its format.
pub(crate) fn parse_config(text: &str, format: Format) -> Result<Value, String> {
    match format {
        Format::Json => serde_json::from_str(text)
            .or_else(|_| serde_json::from_str(&strip_trailing_commas(text)))
            .map_err(|error| error.to_string()),
        Format::Toml => toml::from_str::<toml::Value>(text)
            .map_err(|error| error.to_string())
            .and_then(|value| serde_json::to_value(value).map_err(|error| error.to_string())),
        // Not parsed into a document: reported as text, which is what the dashboard shows for
        // these and what the marker checks.
        Format::DotEnv | Format::YamlBlock => Ok(Value::String(text.to_owned())),
    }
}

/// Remove trailing commas before `}` or `]`, the way upstream's
/// `content.replace(/,(\s*[}\]])/g, "$1")` does.
///
/// Skips string literals, which the regex does not. Upstream's version corrupts
/// `{"a": "x,}"}`; matching that bug would mean failing to read a config this port could
/// otherwise read, so this is a deliberate improvement rather than a divergence in behaviour
/// anyone can observe — the outputs differ only for input upstream mangles.
fn strip_trailing_commas(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut in_string = false;
    let mut escaped = false;
    // Index into `output` of the last comma seen outside a string, if only whitespace has
    // followed it.
    let mut pending_comma: Option<usize> = None;

    for character in text.chars() {
        if in_string {
            output.push(character);
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        match character {
            '"' => {
                in_string = true;
                pending_comma = None;
                output.push(character);
            }
            ',' => {
                pending_comma = Some(output.len());
                output.push(character);
            }
            '}' | ']' => {
                if let Some(index) = pending_comma.take() {
                    // Drop the comma, keeping the whitespace between it and the brace.
                    output.remove(index);
                }
                output.push(character);
            }
            character if character.is_whitespace() => output.push(character),
            character => {
                pending_comma = None;
                output.push(character);
            }
        }
    }
    output
}

/// Everything we can say about one tool, from the filesystem.
pub(crate) fn status(tool: &Tool) -> Status {
    let binary = tool.binaries.iter().find_map(|name| on_path(name));

    let Some(config) = tool.config else {
        // Binary-only detection, as upstream does for devin.
        return match binary {
            Some(path) => Status {
                installed: true,
                source: Some(path.display().to_string()),
                ..Status::absent()
            },
            None => Status::absent(),
        };
    };

    let Some(path) = config.resolve() else {
        // No home directory. Report not-installed rather than guessing a path, and say why
        // through `parse_error` so it is not silent.
        return Status {
            parse_error: Some(
                "Cannot locate the home directory: $HOME is unset or empty, so no config path \
                 could be resolved."
                    .to_owned(),
            ),
            ..Status::absent()
        };
    };

    let (settings, parse_error) = match read_config(&path, config.format) {
        Ok(value) => (value, None),
        // Upstream treats an unparseable config as "no config" so the UI does not misread a 500
        // as "not installed". Same outcome, with the reason attached.
        Err(error) => (Value::Null, Some(error)),
    };

    let config_exists = path.exists();
    let source = binary
        .as_ref()
        .map(|found| found.display().to_string())
        .or_else(|| config_exists.then(|| path.display().to_string()));

    Status {
        installed: binary.is_some() || config_exists,
        source,
        // An absent or unparseable config cannot point anywhere, so the marker is not consulted.
        has_router: !settings.is_null() && marker_matches(tool, &settings),
        settings,
        config_path: Some(path),
        parse_error,
    }
}

/// Run a tool's marker against its parsed config.
fn marker_matches(tool: &Tool, settings: &Value) -> bool {
    match tool.marker {
        Marker::Json(check) => check(settings),
        // The text formats are carried as a `Value::String`, so the marker sees the file.
        Marker::Text(check) => settings.as_str().is_some_and(check),
        Marker::NoConfig => false,
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test assertions read clearer with expect than with error plumbing"
)]
mod tests {
    use super::{Format, parse_config, status, strip_trailing_commas};
    use crate::cli_tools::spec::Tool;
    use serde_json::json;

    #[test]
    fn trailing_commas_are_stripped_like_upstream() {
        assert_eq!(strip_trailing_commas("{\"a\": 1,}"), "{\"a\": 1}");
        assert_eq!(strip_trailing_commas("[1, 2,]"), "[1, 2]");
        // Two trailing commas here, nested: `,]` and then `,\n}`. Both go.
        assert_eq!(strip_trailing_commas("{\"a\": [1,],\n}"), "{\"a\": [1]\n}");
        // Whitespace between the comma and the brace is preserved, as upstream's `$1` does.
        assert_eq!(strip_trailing_commas("{\"a\": 1,\n  }"), "{\"a\": 1\n  }");
        // Nothing to do.
        assert_eq!(strip_trailing_commas("{\"a\": 1}"), "{\"a\": 1}");
    }

    #[test]
    fn a_comma_inside_a_string_is_left_alone() {
        // Upstream's regex corrupts this into `{"a": "x}"}`, losing a character from the user's
        // value. Reading the file correctly is strictly better and cannot show a user anything
        // upstream would have shown them, because upstream fails to parse it.
        let text = "{\"a\": \"x,}\"}";
        assert_eq!(strip_trailing_commas(text), text);
        let parsed = parse_config(text, Format::Json).expect("should parse");
        assert_eq!(parsed, json!({"a": "x,}"}));
    }

    #[test]
    fn an_escaped_quote_does_not_end_the_string() {
        let text = r#"{"a": "he said \"hi,\"", "b": 1,}"#;
        let parsed = parse_config(text, Format::Json).expect("should parse");
        assert_eq!(parsed, json!({"a": "he said \"hi,\"", "b": 1}));
    }

    #[test]
    fn json_with_a_trailing_comma_still_parses() {
        let parsed = parse_config("{\"env\": {\"A\": \"b\"},}", Format::Json).expect("parses");
        assert_eq!(parsed, json!({"env": {"A": "b"}}));
    }

    #[test]
    fn toml_becomes_json_so_one_marker_shape_works_for_both() {
        let parsed = parse_config(
            "model_provider = \"9router\"\n[model_providers.9router]\nbase_url = \"http://x\"\n",
            Format::Toml,
        )
        .expect("parses");
        assert_eq!(parsed["model_provider"], "9router");
        assert_eq!(parsed["model_providers"]["9router"]["base_url"], "http://x");
    }

    #[test]
    fn the_text_formats_are_carried_verbatim() {
        let text = "# comment\nJCODE_9ROUTER_API_KEY=abc\n";
        let parsed = parse_config(text, Format::DotEnv).expect("parses");
        assert_eq!(parsed, json!(text));
    }

    #[test]
    fn unparseable_json_is_an_error_not_a_silent_empty() {
        // The distinction that matters: absent means "not configured", broken means "your file
        // has a problem". Collapsing them hides a typo in the user's own config.
        assert!(parse_config("{not json", Format::Json).is_err());
        assert!(parse_config("= = =", Format::Toml).is_err());
    }

    #[test]
    fn a_tool_that_is_certainly_not_installed_reports_absent() {
        // `deepseek` is not on this machine's PATH and `~/.deepseek/config.toml` does not exist,
        // so this is the honest-negative case. If it ever *is* installed here the assertion below
        // would be wrong, so it checks the coupling instead of the bare value: `installed` must
        // agree with whether a source was found.
        let tool = Tool::parse("deepseek-tui").expect("in the table");
        let status = status(tool);
        assert_eq!(
            status.installed,
            status.source.is_some(),
            "installed must be reported on evidence, not asserted: {status:?}"
        );
        if !status.installed {
            assert!(!status.has_router);
            assert!(status.settings.is_null());
            // The path is still reported so a user knows where to look.
            assert!(status.config_path.is_some());
        }
    }

    #[test]
    fn every_tool_reports_a_status_without_panicking() {
        // The point is that this touches the real filesystem for all fourteen: a missing
        // directory, an unreadable file or an odd `$HOME` must not take the route down.
        for tool in crate::cli_tools::spec::TOOLS {
            let status = status(tool);
            assert_eq!(
                status.installed,
                status.source.is_some(),
                "{} reported installed={} with source={:?}",
                tool.id,
                status.installed,
                status.source
            );
            if status.settings.is_null() {
                assert!(
                    !status.has_router,
                    "{} claims a router with no config",
                    tool.id
                );
            }
        }
    }

    #[test]
    fn a_binary_on_path_is_found() {
        // `sh` is on PATH in every environment this runs in, so this exercises the lookup itself
        // rather than asserting something about which CLI tools happen to be installed.
        assert!(
            super::on_path("sh").is_some(),
            "PATH lookup failed for a binary that must exist"
        );
        assert!(super::on_path("definitely-not-a-real-binary-9router").is_none());
    }
}
