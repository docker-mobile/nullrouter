//! Which CLI coding tools exist, where each keeps its config, and how to tell if we are in it.
//!
//! One table rather than fourteen handlers. Every field here describes a file some other program
//! owns, so none of it is guesswork: a wrong path means writing a config file into a directory
//! that belongs to someone else, and a wrong marker means reporting the wrong state for a tool
//! that is perfectly well configured.
//!
//! # Why the markers are function pointers
//!
//! The `hasRouter` flag drives a toggle in the dashboard, so a check that is merely plausible
//! shows the user something false. The fourteen checks do not share a shape: cline ANDs a provider
//! field with a URL test, copilot's config is a top-level array searched by name, openclaw nests
//! two levels deep, cowork compares an enum field, deepseek-tui has no mention of this router in
//! its config at all. Rather than bend those into one enum, each is a small function beside a
//! comment recording the field it reads and why that field and not the obvious one.
//!
//! # The `tool` path segment is not a path component
//!
//! `GET /api/cli-tools/{tool}` takes a caller-supplied string, and the route reads and writes
//! files in the user's home directory. So `{tool}` is resolved through [`Tool::parse`] against
//! this fixed table and never joined into a path. An unknown tool is a 404, not a filesystem
//! lookup — otherwise `../../../etc/passwd` would be a config path.

use std::path::PathBuf;

use serde_json::Value;

use super::mutations::{DISPLAY, PROVIDER};

/// The config file format, which decides how a merge is done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Format {
    /// Parsed, merged, re-serialised. Upstream tolerates trailing commas on read, so this does
    /// too — hand-edited editor configs often have them, and treating one as unreadable would
    /// report "not configured" for a tool that is configured.
    Json,
    /// Via `toml`, with `preserve_order` so a rewrite leaves the user's own keys where they were.
    Toml,
    /// TOML edited as text, by section, rather than parsed.
    ///
    /// Only Grok Build's config, and only because upstream records the user's previous default
    /// model in a comment that a parse and re-serialise would drop. Detection still reads that file
    /// as [`Self::Toml`]: the marker inspects a parsed value, and only the *write* has to preserve
    /// comments. See [`super::toml_text`].
    TomlText,
    /// Line-oriented `KEY=value`, merged by upserting keys so comments and unrelated lines
    /// survive.
    DotEnv,
    /// Hermes' `config.yaml`, edited at the text level by block rather than parsed.
    YamlBlock,
}

/// How to tell whether a config already points at this router.
#[derive(Clone, Copy)]
pub(crate) enum Marker {
    /// For the parseable formats: run against the parsed document.
    Json(fn(&Value) -> bool),
    /// For the text-edited formats, and for the cases where upstream itself greps the raw file.
    Text(fn(&str) -> bool),
    /// The tool has no config file to inspect — presence of the binary is the whole answer.
    NoConfig,
}

impl std::fmt::Debug for Marker {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Json(_) => "Marker::Json",
            Self::Text(_) => "Marker::Text",
            Self::NoConfig => "Marker::NoConfig",
        })
    }
}

/// One CLI tool.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Tool {
    /// The `{tool}` path segment, exactly as the dashboard sends it.
    pub(crate) id: &'static str,
    /// For messages a user reads.
    pub(crate) display_name: &'static str,
    /// Looked up on `PATH` to decide `installed`. Empty for tools with no CLI of their own
    /// (editor extensions), where the config file's existence is the only signal available.
    pub(crate) binaries: &'static [&'static str],
    /// The file reported as `settingsPath`/`configPath` and inspected for the marker. `None` for
    /// tools upstream detects by binary alone.
    pub(crate) config: Option<ConfigFile>,
    pub(crate) marker: Marker,
    /// Whether this port can apply and revoke the config, or only report it.
    pub(crate) writable: Writable,
}

/// Where a tool's config lives, resolved at call time because it depends on `$HOME`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ConfigFile {
    /// Directory roots to try in order. The first that exists wins; if none does, the first is
    /// used, matching upstream's `getCandidateRoots()[0]` fallback.
    pub(crate) roots: &'static [Root],
    /// Path segments below the chosen root. With an [`Indirection`] these name the directory; the
    /// filename comes from the meta file.
    pub(crate) segments: &'static [&'static str],
    pub(crate) format: Format,
    /// For tools whose config filename is recorded in another file rather than fixed.
    pub(crate) indirect: Option<Indirection>,
}

/// A config filename that has to be read out of a sibling file.
///
/// Cowork keeps a library of configs and records which one is applied, so the file to inspect is
/// `<dir>/<appliedId>.json`. Guessing a fixed name would report "not configured" for every Cowork
/// user, since no file is called `config.json`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Indirection {
    /// A JSON file inside the same directory.
    pub(crate) meta_file: &'static str,
    /// The key in that file holding the basename, to which `.json` is appended.
    pub(crate) key: &'static str,
}

/// A directory root, relative to something only known at runtime.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Root {
    /// Below `$HOME`.
    Home(&'static [&'static str]),
    /// Below `$XDG_CONFIG_HOME`, defaulting to `~/.config` — where the editor-hosted tools keep
    /// theirs on Linux.
    XdgConfig(&'static [&'static str]),
}

impl Root {
    fn resolve(self) -> Option<PathBuf> {
        let (base, segments) = match self {
            Self::Home(segments) => (home_dir()?, segments),
            Self::XdgConfig(segments) => {
                let base = match std::env::var_os("XDG_CONFIG_HOME") {
                    Some(value) if !value.is_empty() => PathBuf::from(value),
                    _ => home_dir()?.join(".config"),
                };
                (base, segments)
            }
        };
        let mut path = base;
        for segment in segments {
            path.push(segment);
        }
        Some(path)
    }
}

impl ConfigFile {
    /// The absolute path, or `None` when the home directory cannot be determined.
    ///
    /// `None` rather than a guess: falling back to `/` or the process's cwd would write a config
    /// file somewhere nobody expects.
    pub(crate) fn resolve(self) -> Option<PathBuf> {
        let mut path = self.directory()?;
        match self.indirect {
            None => {
                for segment in self.segments {
                    path.push(segment);
                }
                Some(path)
            }
            Some(indirection) => {
                // `segments` named the directory; the filename is in the meta file.
                let meta = std::fs::read_to_string(path.join(indirection.meta_file)).ok()?;
                let parsed: Value = serde_json::from_str(&meta).ok()?;
                let name = parsed.get(indirection.key)?.as_str()?;
                path.push(Self::safe_basename(name)?);
                Some(path)
            }
        }
    }

    /// `<name>.json`, if `name` is a plain filename.
    ///
    /// A meta file is not user-authored, but it is a file on disk choosing a filename, so a
    /// separator or `..` in it must not become a path traversal. `None` rather than a sanitised
    /// name: a config id containing a slash is not something to guess the intent of.
    fn safe_basename(name: &str) -> Option<String> {
        let plain = !name.is_empty()
            && !name.contains('/')
            && !name.contains('\\')
            && !name.contains("..")
            && !name.contains('\0')
            && name != "."
            && !name.starts_with(std::path::MAIN_SEPARATOR);
        plain.then(|| format!("{name}.json"))
    }

    /// The directory the config lives in, for naming it in an error a user reads.
    ///
    /// Exposed separately because when [`Self::resolve`] returns `None` for an indirect config there
    /// is still a directory worth naming — telling someone "no config was found" without saying
    /// where it was looked for leaves them nothing to check.
    pub(crate) fn directory_for_report(self) -> Option<PathBuf> {
        self.directory()
    }

    /// The directory the config lives in: the chosen root plus, when indirect, `segments`.
    fn directory(self) -> Option<PathBuf> {
        let mut path = self
            .roots
            .iter()
            .filter_map(|root| root.resolve())
            .find(|path| path.exists())
            .or_else(|| self.roots.first().and_then(|root| root.resolve()))?;
        if self.indirect.is_some() {
            for segment in self.segments {
                path.push(segment);
            }
        }
        Some(path)
    }
}

/// The home directory, from `$HOME`.
///
/// Not `dirs::home_dir`: one env var read does not justify a dependency, and this port targets
/// platforms where `$HOME` is set. An empty value is treated as unset, since joining onto `""`
/// yields a relative path.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Can this port apply the config, or only report it?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Writable {
    /// Apply and revoke are implemented.
    Yes,
    /// Read-only because the tool exposes no way to apply a config that this port could drive —
    /// `devin` can only be reported on. A deliberate limit, not an unfinished writer.
    NoMutationAvailable,
}

/// Does a string look like it points at a local router?
///
/// Used by cline and kilo. The [`PROVIDER`] spelling counts as a match because it is a hostname a
/// user may have configured — a tool set up before switching has that string in its `baseUrl`, and
/// failing to recognise it would report a configured tool as unconfigured.
fn looks_like_router_url(url: &str) -> bool {
    url.contains("localhost") || url.contains("127.0.0.1") || url.contains(PROVIDER)
}

/// The other local-URL test, used by hermes and deepseek-tui.
///
/// Deliberately a different set from [`looks_like_router_url`]: this one accepts `0.0.0.0` and does
/// *not* accept the provider hostname. Kept as two functions rather than merged into their union,
/// because each tool decides for itself what counts as local, and a merged test would report a
/// state neither tool's own configuration implies.
fn is_local_base_url(url: &str) -> bool {
    url.contains("localhost") || url.contains("127.0.0.1") || url.contains("0.0.0.0")
}

/// A nested string field, or `""`.
fn string_at(value: &Value, path: &[&str]) -> String {
    let mut current = value;
    for key in path {
        match current.get(key) {
            Some(next) => current = next,
            None => return String::new(),
        }
    }
    current.as_str().unwrap_or_default().to_owned()
}

/// Every tool the dashboard can ask about.
///
/// The order is the order `all-statuses` reports them in, which is what the dashboard renders.
pub(crate) const TOOLS: &[Tool] = &[
    Tool {
        id: "claude-settings",
        display_name: "Claude Code",
        binaries: &["claude"],
        config: Some(ConfigFile {
            roots: &[Root::Home(&[".claude"])],
            segments: &["settings.json"],
            format: Format::Json,
            indirect: None,
        }),
        // `!!(settings?.env?.ANTHROPIC_BASE_URL)`
        marker: Marker::Json(|settings| {
            !string_at(settings, &["env", "ANTHROPIC_BASE_URL"]).is_empty()
        }),
        writable: Writable::Yes,
    },
    Tool {
        id: "codex-settings",
        display_name: "Codex",
        binaries: &["codex"],
        config: Some(ConfigFile {
            roots: &[Root::Home(&[".codex"])],
            segments: &["config.toml"],
            format: Format::Toml,
            indirect: None,
        }),
        // Matched against the raw text rather than the parsed tree, so a file where the provider
        // block exists but is not the selected one still counts as configured. That is the more
        // useful answer: the block is there because an apply put it there, and the dashboard's
        // toggle is about whether this router is set up, not whether it is currently in use.
        marker: Marker::Text(|text| {
            text.contains("model_provider = \"9router\"")
                || text.contains("[model_providers.9router]")
        }),
        writable: Writable::Yes,
    },
    Tool {
        id: "opencode-settings",
        display_name: "opencode",
        binaries: &["opencode"],
        config: Some(ConfigFile {
            roots: &[Root::XdgConfig(&["opencode"])],
            segments: &["opencode.json"],
            format: Format::Json,
            indirect: None,
        }),
        // A provider entry keyed by name under the top-level `provider` map.
        marker: Marker::Json(|config| {
            config
                .get("provider")
                .and_then(|providers| providers.get(PROVIDER))
                .is_some()
        }),
        writable: Writable::Yes,
    },
    Tool {
        id: "droid-settings",
        display_name: "Factory Droid",
        binaries: &["droid"],
        config: Some(ConfigFile {
            roots: &[Root::Home(&[".factory"])],
            segments: &["settings.json"],
            format: Format::Json,
            indirect: None,
        }),
        // Droid numbers its custom model ids — `custom:9Router-0`, `-1` — so this is a prefix test
        // over `customModels`, never an equality one.
        marker: Marker::Json(|settings| {
            settings
                .get("customModels")
                .and_then(Value::as_array)
                .is_some_and(|models| {
                    models.iter().any(|model| {
                        model
                            .get("id")
                            .and_then(Value::as_str)
                            .is_some_and(|id| id.starts_with(super::mutations::DROID_ID_PREFIX))
                    })
                })
        }),
        writable: Writable::Yes,
    },
    Tool {
        id: "openclaw-settings",
        display_name: "OpenClaw",
        binaries: &["openclaw"],
        config: Some(ConfigFile {
            roots: &[Root::Home(&[".openclaw"])],
            segments: &["openclaw.json"],
            format: Format::Json,
            indirect: None,
        }),
        // Two levels deep — `models.providers.<name>` — not the top-level `providers` map that
        // several other tools use. Reading the shallow path finds nothing for a configured install.
        marker: Marker::Json(|settings| {
            settings
                .get("models")
                .and_then(|models| models.get("providers"))
                .and_then(|providers| providers.get(PROVIDER))
                .is_some()
        }),
        writable: Writable::Yes,
    },
    Tool {
        id: "hermes-settings",
        display_name: "Hermes",
        binaries: &["hermes"],
        config: Some(ConfigFile {
            roots: &[Root::Home(&[".hermes"])],
            segments: &["config.yaml"],
            format: Format::YamlBlock,
            indirect: None,
        }),
        // Hermes' config is edited as text by block, so its marker reads text too: anywhere the
        // provider name appears in the YAML means an apply has been through it.
        marker: Marker::Text(|text| text.contains(PROVIDER)),
        writable: Writable::Yes,
    },
    Tool {
        id: "cowork-settings",
        display_name: "Cowork",
        // Claude Desktop in Cowork mode: a desktop app, not a CLI, so there is no binary to find.
        // The presence of one of the candidate roots is the only installation signal there is.
        binaries: &[],
        config: Some(ConfigFile {
            // `Claude-3p` then `Claude`, taking the first that has a `configLibrary` directory and
            // falling back to the first candidate.
            roots: &[
                Root::XdgConfig(&["Claude-3p"]),
                Root::XdgConfig(&["Claude"]),
            ],
            segments: &["configLibrary"],
            format: Format::Json,
            // The applied config is `<appliedId>.json`, named in `_meta.json`.
            indirect: Some(Indirection {
                meta_file: "_meta.json",
                key: "appliedId",
            }),
        }),
        // The trap in this table: cowork's provider value is the literal `"gateway"`, *not*
        // [`PROVIDER`], and the URL field is `inferenceGatewayBaseUrl`, not `baseUrl`. Testing
        // either of the obvious names reports every cowork user as unconfigured.
        marker: Marker::Json(|config| {
            string_at(config, &["inferenceProvider"]) == "gateway"
                && !string_at(config, &["inferenceGatewayBaseUrl"]).is_empty()
        }),
        writable: Writable::Yes,
    },
    Tool {
        id: "copilot-settings",
        display_name: "GitHub Copilot",
        // A VS Code extension: there is no `copilot` binary to look for, so the config file is
        // the only evidence available.
        binaries: &[],
        config: Some(ConfigFile {
            roots: &[Root::XdgConfig(&["Code", "User"])],
            segments: &["chatLanguageModels.json"],
            format: Format::Json,
            indirect: None,
        }),
        // Copilot's config is a top-level *array* of model entries, not an object, so this searches
        // it for one named [`DISPLAY`] — the capitalised spelling, matched exactly.
        marker: Marker::Json(|config| {
            config.as_array().is_some_and(|entries| {
                entries
                    .iter()
                    .any(|entry| entry.get("name").and_then(Value::as_str) == Some(DISPLAY))
            })
        }),
        writable: Writable::Yes,
    },
    Tool {
        id: "cline-settings",
        display_name: "Cline",
        binaries: &["cline"],
        config: Some(ConfigFile {
            roots: &[Root::Home(&[".cline", "data"])],
            segments: &["globalState.json"],
            format: Format::Json,
            indirect: None,
        }),
        // Cline names no provider of ours: it selects `openai` in either of its two modes and
        // points the OpenAI base URL here, so both halves have to hold. The provider field alone
        // would match anyone using OpenAI directly.
        marker: Marker::Json(|state| {
            let openai = string_at(state, &["actModeApiProvider"]) == "openai"
                || string_at(state, &["planModeApiProvider"]) == "openai";
            openai && looks_like_router_url(&string_at(state, &["openAiBaseUrl"]))
        }),
        writable: Writable::Yes,
    },
    Tool {
        id: "kilo-settings",
        display_name: "Kilo Code",
        binaries: &["kilo"],
        config: Some(ConfigFile {
            roots: &[Root::Home(&[".local", "share", "kilo"])],
            segments: &["auth.json"],
            format: Format::Json,
            indirect: None,
        }),
        // Kilo files its entry under either `openai-compatible` or the provider name, depending on
        // which release wrote it, and spells the URL key `baseUrl` or `baseURL` for the same reason.
        // All four combinations are live in the wild, so all four are accepted.
        marker: Marker::Json(|auth| {
            let entry = auth.get("openai-compatible").or_else(|| auth.get(PROVIDER));
            entry.is_some_and(|entry| {
                let url = match entry.get("baseUrl").and_then(Value::as_str) {
                    Some(url) => url.to_owned(),
                    None => string_at(entry, &["baseURL"]),
                };
                looks_like_router_url(&url)
            })
        }),
        writable: Writable::Yes,
    },
    Tool {
        id: "deepseek-tui-settings",
        display_name: "DeepSeek TUI",
        binaries: &["deepseek"],
        config: Some(ConfigFile {
            roots: &[Root::Home(&[".deepseek"])],
            segments: &["config.toml"],
            format: Format::Toml,
            indirect: None,
        }),
        // `provider == "openai"` plus a local `providers.openai.base_url`. Note what this is *not*:
        // a DeepSeek TUI config configured by this router contains the provider name nowhere at all,
        // because the tool only speaks OpenAI. A text search for the provider would report "not
        // configured" immediately after a successful apply, which is why this reads two fields.
        //
        // The nested path is the real one. A TOML reader that flattens `[providers.openai]` into a
        // literal dotted key would need `config["providers.openai"]` instead; a real parser gives a
        // nested table, and that is what is walked here.
        marker: Marker::Json(|config| {
            string_at(config, &["provider"]) == "openai"
                && is_local_base_url(&string_at(config, &["providers", "openai", "base_url"]))
        }),
        writable: Writable::Yes,
    },
    Tool {
        id: "jcode-settings",
        display_name: "jcode",
        binaries: &["jcode"],
        config: Some(ConfigFile {
            roots: &[Root::Home(&[".jcode"])],
            segments: &["config.toml"],
            format: Format::Toml,
            indirect: None,
        }),
        // The provider entry by name, or *any* provider whose `base_url` names the default port.
        // The second half catches a user who added the router under a name of their own choosing;
        // the port is spelled out rather than generalised because a provider on some other port is
        // a different router, not this one, and claiming it would light the toggle for someone
        // else's install.
        //
        // jcode's primary config is `config.toml`. Its `.env` file holds only the key, so a marker
        // that read the `.env` would miss every jcode user who has one but no provider block.
        marker: Marker::Json(|config| {
            let Some(providers) = config.get("providers").and_then(Value::as_object) else {
                return false;
            };
            providers.contains_key(PROVIDER)
                || providers
                    .values()
                    .any(|provider| string_at(provider, &["base_url"]).contains("localhost:20128"))
        }),
        writable: Writable::Yes,
    },
    Tool {
        id: "grok-build-settings",
        display_name: "Grok Build",
        binaries: &["grok"],
        config: Some(ConfigFile {
            roots: &[Root::Home(&[".grok"])],
            segments: &["config.toml"],
            format: Format::Toml,
            indirect: None,
        }),
        // The router's slot is the `[model.9router]` *section*, so the path is
        // `model.<provider>.base_url` — three levels, not the `[model]` table. Reading `[model]`
        // would report every Grok Build user with any model configured as pointing here.
        //
        // Presence of the URL is the whole test, with no check on its value: the section exists
        // only because an apply wrote it, so its presence already carries the answer.
        marker: Marker::Json(|config| {
            !string_at(config, &["model", PROVIDER, "base_url"]).is_empty()
        }),
        writable: Writable::Yes,
    },
    Tool {
        id: "devin-settings",
        display_name: "Devin",
        binaries: &["devin"],
        // Devin keeps no config file this router can inspect: it is found on `PATH` and reports its
        // version, and that is the whole signal. Inventing a config path here would report "not
        // configured" on the strength of a file the tool never writes.
        config: None,
        marker: Marker::NoConfig,
        // And with no config file, there is nothing an apply could write.
        writable: Writable::NoMutationAvailable,
    },
];

impl Tool {
    /// Resolve a `{tool}` path segment against the table.
    ///
    /// The dashboard sends the full route name (`claude-settings`); the `all-statuses` response
    /// keys it short (`claude`). Both are accepted so a caller using either spelling works.
    pub(crate) fn parse(segment: &str) -> Option<&'static Self> {
        TOOLS
            .iter()
            .find(|tool| tool.id == segment || tool.short_id() == segment)
    }

    /// The key this tool appears under in `all-statuses`: the id without `-settings`.
    pub(crate) fn short_id(&self) -> &'static str {
        self.id.strip_suffix("-settings").unwrap_or(self.id)
    }
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    reason = "indexing a serde_json::Value is the assertion here: a shape that does not \
              match is a test failure, which is what the panic reports"
)]
mod tests {
    use super::{Format, Marker, Root, TOOLS, Tool, Writable, looks_like_router_url, string_at};
    use serde_json::json;

    #[test]
    fn every_tool_upstream_exposes_is_in_the_table() {
        // The route directories under `src/app/api/cli-tools/` minus the four that are not
        // per-tool settings routes (`all-statuses`, `antigravity-mitm`, `cowork-mcp-registry`,
        // `cowork-mcp-tools`). Listed literally so adding a tool upstream fails here rather than
        // silently 404ing for whoever uses it.
        let expected = [
            "claude-settings",
            "cline-settings",
            "codex-settings",
            "copilot-settings",
            "cowork-settings",
            "deepseek-tui-settings",
            "devin-settings",
            "droid-settings",
            "grok-build-settings",
            "hermes-settings",
            "jcode-settings",
            "kilo-settings",
            "openclaw-settings",
            "opencode-settings",
        ];
        for id in expected {
            assert!(
                Tool::parse(id).is_some(),
                "{id} is served upstream but missing from TOOLS"
            );
        }
        assert_eq!(
            TOOLS.len(),
            expected.len(),
            "TOOLS has entries the upstream list does not"
        );
    }

    #[test]
    fn ids_and_short_ids_are_unique() {
        // A duplicate would make `parse` resolve to whichever came first, silently configuring
        // the wrong tool.
        let mut ids: Vec<&str> = TOOLS.iter().map(|tool| tool.id).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "duplicate tool id");

        let mut shorts: Vec<&str> = TOOLS.iter().map(|tool| tool.short_id()).collect();
        shorts.sort_unstable();
        let before = shorts.len();
        shorts.dedup();
        assert_eq!(before, shorts.len(), "duplicate short id");
    }

    #[test]
    fn a_path_traversal_segment_does_not_resolve() {
        // The whole reason `{tool}` goes through the table: these must not become file paths.
        for segment in [
            "../../../etc/passwd",
            "..",
            ".",
            "",
            "claude-settings/../../..",
            "/etc/shadow",
            "claude%2Fsettings",
            "CLAUDE-SETTINGS",
        ] {
            assert!(
                Tool::parse(segment).is_none(),
                "{segment:?} should not resolve to a tool"
            );
        }
    }

    #[test]
    fn config_paths_stay_under_the_resolved_base() {
        // Belt and braces: the table is a constant, but if a segment ever gained a `..` this
        // would catch it before it reached a write.
        for tool in TOOLS {
            let Some(config) = tool.config else { continue };
            assert!(!config.segments.is_empty(), "{} has no segments", tool.id);
            let roots = config.roots.iter().flat_map(|root| match root {
                Root::Home(segments) | Root::XdgConfig(segments) => segments.iter(),
            });
            for segment in config.segments.iter().chain(roots) {
                assert!(
                    !segment.contains('/')
                        && !segment.contains('\\')
                        && *segment != ".."
                        && !segment.is_empty(),
                    "{} has a suspicious path segment {segment:?}",
                    tool.id
                );
            }
        }
    }

    #[test]
    fn resolve_is_absolute_when_home_is_set() {
        // A relative path would mean reading or writing inside the process's cwd.
        for tool in TOOLS {
            let Some(config) = tool.config else { continue };
            if let Some(path) = config.resolve() {
                assert!(
                    path.is_absolute(),
                    "{} resolved to a relative path {path:?}",
                    tool.id
                );
            }
        }
    }

    #[test]
    fn the_writable_flag_and_the_write_descriptors_agree() {
        // Two tables have to say the same thing: this one drives the dashboard's `writable` field,
        // and `mutations::writer_for` decides whether a mutation runs. A tool marked writable with
        // no descriptor offers the user a toggle that 501s; one with a descriptor but marked
        // read-only hides a writer that works.
        for tool in TOOLS {
            let has_writer = crate::cli_tools::mutations::writer_for(tool.id).is_some();
            assert_eq!(
                has_writer,
                tool.writable == Writable::Yes,
                "{} is marked {:?} but {} a write descriptor",
                tool.id,
                tool.writable,
                if has_writer { "has" } else { "has no" }
            );
        }
    }

    #[test]
    fn devin_is_the_only_read_only_tool() {
        let read_only: Vec<&str> = TOOLS
            .iter()
            .filter(|tool| tool.writable == Writable::NoMutationAvailable)
            .map(|tool| tool.id)
            .collect();
        // Devin is read-only because it has no config file to write. Every other tool here does, so
        // a second name appearing in this list is a writer someone forgot to finish.
        assert_eq!(read_only, ["devin-settings"]);
    }

    #[test]
    fn the_declared_format_matches_the_file_extension() {
        // A mismatch would parse a TOML file as JSON and report "not configured" for a tool that
        // is configured.
        for tool in TOOLS {
            let Some(config) = tool.config else { continue };
            if config.indirect.is_some() {
                // `segments` names a directory here; the filename comes from the meta file and is
                // always `.json`, which the next assertion covers.
                assert_eq!(
                    config.format,
                    Format::Json,
                    "{} is indirect, so its file must be JSON",
                    tool.id
                );
                continue;
            }
            let name = config.segments.last().copied().unwrap_or_default();
            let expected = match config.format {
                Format::Json => ".json",
                Format::Toml | Format::TomlText => ".toml",
                Format::YamlBlock => ".yaml",
                Format::DotEnv => ".env",
            };
            assert!(
                name.ends_with(expected),
                "{} declares {:?} but its file is {name}",
                tool.id,
                config.format
            );
        }
    }

    #[test]
    fn an_indirect_config_name_cannot_escape_its_directory() {
        // Tested on the guard directly rather than by redirecting `$HOME`: these tests share a
        // process with others that read it, and mutating it here would race with them.
        for name in [
            "../../escape",
            "/etc/passwd",
            "..",
            ".",
            "a/b",
            "a\\b",
            "",
            "x/../../y",
        ] {
            assert!(
                super::ConfigFile::safe_basename(name).is_none(),
                "appliedId {name:?} should be refused"
            );
        }
        assert_eq!(
            super::ConfigFile::safe_basename("abc-123").as_deref(),
            Some("abc-123.json")
        );
        // A UUID is the realistic case: that is what cowork names its config files.
        assert_eq!(
            super::ConfigFile::safe_basename("2f1c9e64-0b7a-4d1e-9f3a-77c0d9e5a1b2").as_deref(),
            Some("2f1c9e64-0b7a-4d1e-9f3a-77c0d9e5a1b2.json")
        );
    }

    #[test]
    fn a_tool_with_no_config_has_no_json_marker() {
        // A `Marker::Json` on a tool with no file could never run, and would quietly report
        // `hasRouter: false` forever.
        for tool in TOOLS {
            match (tool.config, tool.marker) {
                (None, Marker::NoConfig) | (Some(_), Marker::Json(_) | Marker::Text(_)) => {}
                (config, marker) => {
                    panic!("{} pairs {config:?} with {marker:?}", tool.id)
                }
            }
        }
    }

    #[test]
    fn the_claude_marker_matches_upstreams_check() {
        let tool = Tool::parse("claude").expect("claude is in the table");
        let Marker::Json(check) = tool.marker else {
            panic!("claude should have a JSON marker")
        };
        assert!(check(
            &json!({"env": {"ANTHROPIC_BASE_URL": "http://127.0.0.1:20128/v1"}})
        ));
        // Present but empty is falsy in JS, and must be here too.
        assert!(!check(&json!({"env": {"ANTHROPIC_BASE_URL": ""}})));
        assert!(!check(&json!({"env": {}})));
        assert!(!check(&json!({})));
        // A *different* base URL still counts: the user pointed Claude Code somewhere, and
        // upstream's check is presence, not identity.
        assert!(check(
            &json!({"env": {"ANTHROPIC_BASE_URL": "https://api.anthropic.com"}})
        ));
    }

    #[test]
    fn the_cline_marker_needs_both_halves() {
        let tool = Tool::parse("cline").expect("cline is in the table");
        let Marker::Json(check) = tool.marker else {
            panic!("cline should have a JSON marker")
        };
        // Provider set and URL local: configured.
        assert!(check(&json!({
            "actModeApiProvider": "openai",
            "openAiBaseUrl": "http://127.0.0.1:20128",
        })));
        // Plan-mode alone also counts, per the `||`.
        assert!(check(&json!({
            "planModeApiProvider": "openai",
            "openAiBaseUrl": "http://localhost:20128",
        })));
        // Local URL but a different provider: not configured.
        assert!(!check(&json!({
            "actModeApiProvider": "anthropic",
            "openAiBaseUrl": "http://127.0.0.1:20128",
        })));
        // Right provider, remote URL: not us.
        assert!(!check(&json!({
            "actModeApiProvider": "openai",
            "openAiBaseUrl": "https://api.openai.com/v1",
        })));
    }

    #[test]
    fn the_copilot_marker_reads_a_top_level_array() {
        // The one config that is an array rather than an object. Treating it as an object would
        // report "not configured" for every Copilot user.
        let tool = Tool::parse("copilot").expect("copilot is in the table");
        let Marker::Json(check) = tool.marker else {
            panic!("copilot should have a JSON marker")
        };
        assert!(check(&json!([{"name": "9Router", "models": []}])));
        assert!(check(&json!([{"name": "other"}, {"name": "9Router"}])));
        assert!(!check(&json!([{"name": "other"}])));
        assert!(!check(&json!([])));
        // Case matters: copilot compares the name exactly, so the lowercase provider spelling is a
        // different entry from the capitalised display name.
        assert!(!check(&json!([{"name": "9router"}])));
        // An object where an array was expected must not panic.
        assert!(!check(&json!({"name": "9Router"})));
    }

    #[test]
    fn the_kilo_marker_accepts_both_spellings_of_the_url_key() {
        let tool = Tool::parse("kilo").expect("kilo is in the table");
        let Marker::Json(check) = tool.marker else {
            panic!("kilo should have a JSON marker")
        };
        assert!(check(
            &json!({"openai-compatible": {"baseUrl": "http://127.0.0.1:20128/v1"}})
        ));
        assert!(check(
            &json!({"openai-compatible": {"baseURL": "http://localhost:20128/v1"}})
        ));
        assert!(check(
            &json!({"9router": {"baseUrl": "http://9router.local/v1"}})
        ));
        assert!(!check(
            &json!({"openai-compatible": {"baseUrl": "https://api.openai.com/v1"}})
        ));
        assert!(!check(&json!({})));
    }

    #[test]
    fn the_openclaw_marker_reaches_two_levels_down() {
        let tool = Tool::parse("openclaw").expect("openclaw is in the table");
        let Marker::Json(check) = tool.marker else {
            panic!("openclaw should have a JSON marker")
        };
        assert!(check(&json!({"models": {"providers": {"9router": {}}}})));
        assert!(!check(&json!({"models": {"providers": {}}})));
        assert!(!check(&json!({"providers": {"9router": {}}})));
    }

    #[test]
    fn the_cowork_marker_reads_coworks_field_names_not_the_obvious_ones() {
        // Two traps here, either of which would report every Cowork user as unconfigured: the
        // provider value cowork stores is `"gateway"`, not the provider name, and the URL field is
        // `inferenceGatewayBaseUrl`, not `baseUrl`.
        let tool = Tool::parse("cowork").expect("cowork is in the table");
        let Marker::Json(check) = tool.marker else {
            panic!("cowork should have a JSON marker")
        };
        assert!(check(&json!({
            "inferenceProvider": "gateway",
            "inferenceGatewayBaseUrl": "http://127.0.0.1:20128",
        })));
        assert!(!check(&json!({
            "inferenceProvider": "gateway",
            "inferenceGatewayBaseUrl": "",
        })));
        assert!(!check(&json!({
            "inferenceProvider": "anthropic",
            "inferenceGatewayBaseUrl": "http://x",
        })));
        // The plausible-but-wrong shapes.
        assert!(!check(&json!({
            "inferenceProvider": "9router",
            "inferenceGatewayBaseUrl": "http://x",
        })));
        assert!(!check(
            &json!({"inferenceProvider": "gateway", "baseUrl": "http://x"})
        ));
    }

    #[test]
    fn the_jcode_marker_has_both_of_its_branches() {
        let tool = Tool::parse("jcode").expect("jcode is in the table");
        let Marker::Json(check) = tool.marker else {
            panic!("jcode should have a JSON marker")
        };
        // Named provider.
        assert!(check(&json!({"providers": {"9router": {}}})));
        // Or any provider pointing at the default port, under a name of the user's own choosing.
        assert!(check(&json!({
            "providers": {"mine": {"base_url": "http://localhost:20128/v1"}}
        })));
        assert!(!check(&json!({
            "providers": {"mine": {"base_url": "http://localhost:11434/v1"}}
        })));
        assert!(!check(&json!({"providers": {}})));
        assert!(!check(&json!({})));
    }

    #[test]
    fn the_codex_marker_matches_either_written_string() {
        let tool = Tool::parse("codex").expect("codex is in the table");
        let Marker::Text(check) = tool.marker else {
            panic!("codex should have a text marker")
        };
        assert!(check("model_provider = \"9router\"\n"));
        assert!(check("[model_providers.9router]\nbase_url = \"...\"\n"));
        assert!(!check("model_provider = \"openai\"\n"));
        // Single-quoted is valid TOML but is not what an apply writes, and this marker reads raw
        // text rather than a parse. Matching it would claim a hand-edited file was written here.
        assert!(!check("model_provider = '9router'\n"));
    }

    #[test]
    fn the_deepseek_marker_matches_the_config_an_apply_writes() {
        // The trap: a deepseek apply produces a config with no provider name in it at all, so a text
        // search would report "not configured" straight after one succeeded. This asserts the marker
        // against the exact TOML the writer emits.
        let tool = Tool::parse("deepseek-tui").expect("in the table");
        let Marker::Json(check) = tool.marker else {
            panic!("deepseek-tui should have a JSON marker")
        };
        let written = "provider = \"openai\"\n\n[providers.openai]\n\
                       base_url = \"http://127.0.0.1:20128/v1\"\n\
                       api_key = \"sk_9router\"\nmodel = \"m\"\n";
        assert!(
            !written.contains("9router") || written.contains("sk_9router"),
            "sanity: the only 9router text is inside the placeholder key"
        );
        let parsed = crate::cli_tools::detect::parse_config(written, Format::Toml).expect("parses");
        assert!(
            check(&parsed),
            "the config an apply writes must match: {parsed}"
        );

        // And a remote OpenAI config does not.
        let remote = "provider = \"openai\"\n\n[providers.openai]\n\
                      base_url = \"https://api.openai.com/v1\"\n";
        let parsed = crate::cli_tools::detect::parse_config(remote, Format::Toml).expect("parses");
        assert!(!check(&parsed));
    }

    #[test]
    fn the_grok_marker_reads_the_slot_section_not_the_model_table() {
        // The router's model lives in the `[model.9router]` section, not a `[model]` table. Reading
        // `[model]` would report every Grok Build user as unconfigured.
        let tool = Tool::parse("grok-build").expect("in the table");
        let Marker::Json(check) = tool.marker else {
            panic!("grok-build should have a JSON marker")
        };
        let written = "[model.9router]\nname = \"9Router\"\n\
                       base_url = \"http://127.0.0.1:20128/v1\"\n\n[models]\ndefault = \"9router\"\n";
        let parsed = crate::cli_tools::detect::parse_config(written, Format::Toml).expect("parses");
        assert!(check(&parsed), "{parsed}");

        // A `[model]` table with a base_url is a different section, and must not match.
        let wrong_shape = "[model]\nbase_url = \"http://127.0.0.1:20128/v1\"\n";
        let parsed =
            crate::cli_tools::detect::parse_config(wrong_shape, Format::Toml).expect("parses");
        assert!(!check(&parsed), "{parsed}");
    }

    #[test]
    fn the_two_local_url_tests_are_kept_distinct() {
        // They differ, and merging them would make a tool report a state its own config does not
        // support. `0.0.0.0` is local to hermes and deepseek but not to cline and kilo; a provider
        // hostname is the reverse.
        assert!(super::is_local_base_url("http://0.0.0.0:20128/v1"));
        assert!(!looks_like_router_url("http://0.0.0.0:20128/v1"));

        assert!(looks_like_router_url("https://9router.example.com/v1"));
        assert!(!super::is_local_base_url("https://9router.example.com/v1"));

        // And they agree on the two forms that matter most.
        for url in ["http://localhost:20128/v1", "http://127.0.0.1:20128/v1"] {
            assert!(looks_like_router_url(url));
            assert!(super::is_local_base_url(url));
        }
    }

    #[test]
    fn the_local_url_test_accepts_the_three_forms_a_config_can_hold() {
        assert!(looks_like_router_url("http://localhost:20128/v1"));
        assert!(looks_like_router_url("http://127.0.0.1:20128/v1"));
        assert!(looks_like_router_url("https://9router.example.com/v1"));
        assert!(!looks_like_router_url("https://api.openai.com/v1"));
        assert!(!looks_like_router_url(""));
        // Deliberately not added: `[::1]` and `0.0.0.0` would both be reasonable, but the tools
        // reading this field never write them, so accepting them would only widen what counts as
        // "configured" without any config to justify it.
        assert!(!looks_like_router_url("http://[::1]:20128/v1"));
    }

    #[test]
    fn string_at_does_not_panic_on_the_wrong_shape() {
        assert_eq!(string_at(&json!({"a": {"b": "c"}}), &["a", "b"]), "c");
        assert_eq!(string_at(&json!({"a": 5}), &["a"]), "");
        assert_eq!(string_at(&json!({"a": {"b": "c"}}), &["a", "b", "d"]), "");
        assert_eq!(string_at(&json!([1, 2]), &["a"]), "");
        assert_eq!(string_at(&json!(null), &["a"]), "");
    }
}
