//! The MITM control surface: the CA a client must trust, and the alias map the interceptor reads.
//!
//! Ports the control half of upstream's `antigravity-mitm` routes plus `src/lib/mitmAliasCache.js`. What
//! is *not* here is the interception proxy itself — no TLS listener, no per-SNI leaf issuance, no
//! forwarding. That is stated plainly in the README rather than implied by a route that half-works.
//!
//! What these routes do is the part a proxy cannot do for itself and an operator cannot do by hand:
//!
//! * **Generate the root CA and report where it is.** A client only trusts an interceptor whose CA it has
//!   installed, so the CA must exist and its path must be discoverable before anything else is useful.
//!   Generation is delegated to [`nullrouter_mitm_helper::ca`], which already writes the key `0600` inside
//!   a `0700` directory and never rotates an existing CA implicitly.
//! * **Write the alias map where the interceptor reads it.** Upstream keeps the map in SQLite and syncs a
//!   JSON read-replica to `$DATA_DIR/mitm/aliases.json`, because its standalone MITM server has no SQLite
//!   binding. The file *is* the interface, so the same path, the same `{tool: {alias: model}}` shape, and
//!   the same atomic tmp-then-rename are what make an interceptor built later able to read it.
//!
//! Two deliberate narrowings from upstream. Installing the CA into a system trust store is *reported*
//! rather than performed: it needs root, this service runs unprivileged, and the privileged step lives in
//! the separate `nullrouter-mitm-helper` binary that refuses to run unless already root. And the private
//! key's path is never returned through the API — the certificate's is, because a client needs it; the
//! key's would be a path traversal target with nothing to gain.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The tools whose traffic the interceptor knows how to alias.
///
/// Fixed rather than open: an alias map for an unknown tool is a file the interceptor will never read, and
/// accepting one would report success for work that cannot happen.
pub(crate) const TOOLS: [&str; 4] = ["antigravity", "copilot", "cursor", "kiro"];

/// Where the CA and the alias map live under a data directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Paths {
    /// `$DATA_DIR/mitm`.
    root: PathBuf,
}

impl Paths {
    /// Paths under an explicit data directory.
    pub(crate) fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join("mitm"),
        }
    }

    /// Paths under the data directory this deployment uses.
    ///
    /// `DATA_DIR` first, matching upstream's own override, then the per-user default. The same discovery
    /// the imports and pxpipe use, so a nullrouter sharing a directory with 9Router sees the same files —
    /// which is the point: an interceptor started from either reads one alias map.
    pub(crate) fn discover() -> Self {
        if let Ok(configured) = std::env::var("DATA_DIR")
            && !configured.trim().is_empty()
        {
            return Self::new(Path::new(configured.trim()));
        }
        if let Some(home) = std::env::var_os("HOME") {
            return Self::new(&Path::new(&home).join(".9router"));
        }
        if let Some(appdata) = std::env::var_os("APPDATA") {
            return Self::new(&Path::new(&appdata).join("9router"));
        }
        Self::new(Path::new("."))
    }

    /// The directory holding the CA and the alias map.
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// `$DATA_DIR/mitm/aliases.json`, the file the interceptor reads.
    pub(crate) fn alias_file(&self) -> PathBuf {
        self.root.join("aliases.json")
    }

    /// `$DATA_DIR/mitm/root-ca.crt`, the certificate a client must trust.
    pub(crate) fn certificate(&self) -> PathBuf {
        self.root.join("root-ca.crt")
    }
}

/// What went wrong, in terms an operator can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ControlError {
    /// The directory could not be written. Carries the path, because the fix is a permission change on
    /// that exact directory.
    NotWritable { path: String, detail: String },
    /// A CA is half-present: some of its three files exist and others do not.
    ///
    /// Refused rather than repaired. Generating a new key beside an existing certificate would leave
    /// clients trusting a certificate this router can no longer sign with, which fails at handshake time
    /// with nothing in any log to explain it.
    CorruptAuthority { path: String },
    /// The alias map on disk is not the shape the interceptor reads.
    MalformedAliasFile { path: String },
    /// A tool name outside [`TOOLS`].
    UnknownTool { tool: String },
}

impl ControlError {
    /// The message returned to the caller.
    pub(crate) fn message(&self) -> String {
        match self {
            Self::NotWritable { path, detail } => {
                format!(
                    "MITM data directory {path} is not writable ({detail}). The CA and alias map live \
                     there; fix its permissions or set DATA_DIR to a writable location."
                )
            }
            Self::CorruptAuthority { path } => {
                format!(
                    "The MITM certificate authority in {path} is incomplete: some of its key, \
                     certificate, and fingerprint files exist and others do not. Refusing to generate a \
                     new one over it, because a client trusting the old certificate would fail at \
                     handshake with nothing to explain it. Remove the directory's root-ca.* files to \
                     start over."
                )
            }
            Self::MalformedAliasFile { path } => {
                format!(
                    "The alias map at {path} is not an object of per-tool mappings, so the interceptor \
                     cannot read it. Delete it to start from an empty map."
                )
            }
            Self::UnknownTool { tool } => {
                format!(
                    "Unknown MITM tool '{tool}'. An alias map for a tool the interceptor does not know \
                     would never be read. Known tools: {}.",
                    TOOLS.join(", ")
                )
            }
        }
    }
}

/// The CA as reported to a caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AuthorityReport {
    /// Whether the certificate exists on disk.
    pub(crate) exists: bool,
    /// Absolute path to the PEM certificate a client must trust.
    pub(crate) certificate_path: String,
    /// Hex SHA-256 over the certificate DER, which is how a trust store identifies it.
    pub(crate) fingerprint: String,
    /// Whether this process could install it into a system trust store. Never true here: installation
    /// needs root and this service runs unprivileged.
    pub(crate) can_install: bool,
    /// The command that performs the privileged step.
    pub(crate) install_command: String,
}

/// Generate the CA if absent, then report it.
///
/// Idempotent by construction: [`nullrouter_mitm_helper::ca::ensure`] returns the existing authority's
/// recorded fingerprint rather than rotating, because rotating while the old CA is still trusted strands
/// every client that trusted it.
pub(crate) fn ensure_authority(paths: &Paths) -> Result<AuthorityReport, ControlError> {
    let root = paths.root();
    match nullrouter_mitm_helper::ca::ensure(root) {
        Ok(authority) => Ok(AuthorityReport {
            exists: true,
            certificate_path: authority.certificate.display().to_string(),
            fingerprint: authority.fingerprint,
            can_install: false,
            install_command: install_command(&authority.certificate),
        }),
        Err(nullrouter_mitm_helper::HelperError::CorruptMarkers) => {
            Err(ControlError::CorruptAuthority {
                path: root.display().to_string(),
            })
        }
        Err(error) => Err(ControlError::NotWritable {
            path: root.display().to_string(),
            detail: error.to_string(),
        }),
    }
}

/// Report the CA without creating one.
///
/// Used by the status route, which must not have a side effect: a GET that generated a key pair would mean
/// merely opening the dashboard created one.
pub(crate) fn describe_authority(paths: &Paths) -> AuthorityReport {
    let certificate = paths.certificate();
    let fingerprint = std::fs::read_to_string(paths.root().join("root-ca.sha256"))
        .map(|text| text.trim().to_owned())
        .unwrap_or_default();
    AuthorityReport {
        exists: certificate.is_file(),
        certificate_path: certificate.display().to_string(),
        fingerprint,
        can_install: false,
        install_command: install_command(&certificate),
    }
}

/// The privileged command that installs the CA.
///
/// Named rather than run. The helper refuses unless already root, and this service is unprivileged, so
/// reporting the command is the honest surface: the operator runs it, sees what it did, and can undo it.
fn install_command(certificate: &Path) -> String {
    format!(
        "sudo nullrouter-mitm-helper install-ca {}",
        certificate.display()
    )
}

/// The hosts file whose redirection markers say whether a tool is intercepted.
///
/// Reading it needs no privilege — only writing does — so this service can report the real state rather
/// than a hardcoded `false`, and can enforce upstream's rule that aliases are editable only for a tool
/// whose traffic is actually being redirected. The override exists so the tests do not read the machine's
/// own `/etc/hosts`.
pub(crate) fn hosts_path() -> PathBuf {
    if let Ok(configured) = std::env::var("NULLROUTER_HOSTS_PATH")
        && !configured.trim().is_empty()
    {
        return PathBuf::from(configured.trim());
    }
    if cfg!(windows) {
        PathBuf::from(r"C:\Windows\System32\drivers\etc\hosts")
    } else {
        PathBuf::from("/etc/hosts")
    }
}

/// Whether each known tool's hosts entries are currently in place.
///
/// A tool whose markers cannot be read — no hosts file, or a corrupt marker pair — reports `false`. That
/// is the safe direction: it means "not redirected", which gates alias edits rather than permitting them
/// on a guess.
pub(crate) fn dns_status(hosts: &Path) -> BTreeMap<String, bool> {
    TOOLS
        .iter()
        .map(|name| {
            let enabled = nullrouter_mitm_helper::Tool::parse(name)
                .and_then(|tool| nullrouter_mitm_helper::hosts_enabled(hosts, tool))
                .unwrap_or(false);
            ((*name).to_owned(), enabled)
        })
        .collect()
}

/// Whether one tool is redirected.
pub(crate) fn tool_redirected(hosts: &Path, tool: &str) -> bool {
    nullrouter_mitm_helper::Tool::parse(tool)
        .and_then(|parsed| nullrouter_mitm_helper::hosts_enabled(hosts, parsed))
        .unwrap_or(false)
}

/// The alias map, as the interceptor reads it.
///
/// `{tool: {alias: model}}` — upstream's own shape, because the file is the interface between this control
/// surface and any interceptor built against it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct AliasMap {
    entries: BTreeMap<String, BTreeMap<String, String>>,
}

impl AliasMap {
    /// This tool's mappings, or an empty map.
    ///
    /// Only the tests read one tool in isolation; the routes return the whole map, because the dashboard
    /// renders every tool's aliases at once.
    #[cfg(test)]
    pub(crate) fn for_tool(&self, tool: &str) -> BTreeMap<String, String> {
        self.entries.get(tool).cloned().unwrap_or_default()
    }

    /// Every tool's mappings.
    pub(crate) fn all(&self) -> &BTreeMap<String, BTreeMap<String, String>> {
        &self.entries
    }

    /// Replace one tool's mappings, leaving the others alone.
    ///
    /// Per-tool rather than whole-file, matching upstream's `writeAliasForTool`: a UI saving one tool's
    /// aliases must not blank another's.
    pub(crate) fn set_tool(&mut self, tool: &str, mappings: BTreeMap<String, String>) {
        self.entries.insert(tool.to_owned(), mappings);
    }
}

/// Read the alias map, treating an absent file as empty.
///
/// Absent is the normal first-run state, not an error. A file that exists but is the wrong shape *is* an
/// error: silently replacing it would discard an operator's mappings.
pub(crate) fn read_aliases(paths: &Paths) -> Result<AliasMap, ControlError> {
    let path = paths.alias_file();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(AliasMap::default());
    };
    if text.trim().is_empty() {
        return Ok(AliasMap::default());
    }
    serde_json::from_str(&text).map_err(|_error| ControlError::MalformedAliasFile {
        path: path.display().to_string(),
    })
}

/// Write one tool's mappings into the alias map.
pub(crate) fn write_aliases(
    paths: &Paths,
    tool: &str,
    mappings: BTreeMap<String, String>,
) -> Result<AliasMap, ControlError> {
    if !TOOLS.contains(&tool) {
        return Err(ControlError::UnknownTool {
            tool: tool.to_owned(),
        });
    }
    let mut map = read_aliases(paths)?;
    map.set_tool(tool, mappings);

    let root = paths.root();
    std::fs::create_dir_all(root).map_err(|error| ControlError::NotWritable {
        path: root.display().to_string(),
        detail: error.to_string(),
    })?;
    // Two spaces, as upstream writes it: the file is meant to be readable by whoever is debugging why an
    // interception did not alias what they expected.
    let body = serde_json::to_string_pretty(&map).unwrap_or_else(|_error| "{}".to_owned());
    atomic_write(&paths.alias_file(), &body)?;
    Ok(map)
}

/// Write via a sibling temporary file, then rename.
///
/// The interceptor may read this file at any moment. A truncate-then-write leaves a window where it reads
/// half a document and starts with no aliases at all, which looks like a configuration that was never
/// saved. Upstream does the same, for the same reason.
fn atomic_write(path: &Path, body: &str) -> Result<(), ControlError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let failed = |error: std::io::Error| ControlError::NotWritable {
        path: parent.display().to_string(),
        detail: error.to_string(),
    };
    // Created here as well as by the caller. The caller's `create_dir_all` can be undone between the two
    // — by a cleanup, by an operator, or by a concurrent request — and a missing parent surfaces as a bare
    // `ENOENT` that reads like a bug rather than as the writable-directory problem it is.
    std::fs::create_dir_all(parent).map_err(failed)?;

    // A unique suffix per attempt, so two concurrent writers cannot rename each other's half-written
    // temporary into place. A single fixed name is the usual bug here: the second writer truncates the
    // first's file while the first is still writing it, and the rename publishes the mix.
    let temporary = parent.join(format!(
        ".aliases.json.nullrouter-new.{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos())
    ));
    std::fs::write(&temporary, body).map_err(failed)?;
    match std::fs::rename(&temporary, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            // Leaving a stray temporary behind would make the next run's directory listing confusing.
            let _ignored = std::fs::remove_file(&temporary);
            Err(failed(error))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        AliasMap, ControlError, Paths, TOOLS, describe_authority, ensure_authority, read_aliases,
        write_aliases,
    };

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let base = std::env::temp_dir().join(format!(
            "nullrouter-mitm-{name}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&base).expect("a temp directory");
        base
    }

    #[test]
    fn the_alias_file_is_the_path_and_shape_the_interceptor_reads() {
        // Upstream's standalone MITM server has no SQLite binding, so this file *is* the interface. The
        // path and the shape are the contract, not an implementation detail.
        let base = temp_dir("alias-shape");
        let paths = Paths::new(&base);
        assert!(
            paths.alias_file().ends_with("mitm/aliases.json"),
            "got {}",
            paths.alias_file().display()
        );

        let mut mappings = BTreeMap::new();
        mappings.insert("gemini-3-pro".to_owned(), "kr/claude-sonnet-4".to_owned());
        write_aliases(&paths, "antigravity", mappings).expect("a write");

        let text = std::fs::read_to_string(paths.alias_file()).expect("the file");
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        assert_eq!(
            parsed.pointer("/antigravity/gemini-3-pro"),
            Some(&serde_json::json!("kr/claude-sonnet-4"))
        );
        // Pretty-printed, because whoever debugs an alias that did not apply reads this by hand.
        assert!(text.contains('\n'), "{text}");
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn writing_one_tool_leaves_the_others_alone() {
        // A UI saving antigravity's aliases must not blank cursor's.
        let base = temp_dir("alias-isolation");
        let paths = Paths::new(&base);
        write_aliases(
            &paths,
            "antigravity",
            BTreeMap::from([("a".to_owned(), "kr/one".to_owned())]),
        )
        .expect("first write");
        write_aliases(
            &paths,
            "cursor",
            BTreeMap::from([("b".to_owned(), "kr/two".to_owned())]),
        )
        .expect("second write");

        let map = read_aliases(&paths).expect("a read");
        assert_eq!(
            map.for_tool("antigravity").get("a").map(String::as_str),
            Some("kr/one")
        );
        assert_eq!(
            map.for_tool("cursor").get("b").map(String::as_str),
            Some("kr/two")
        );
        assert_eq!(map.all().len(), 2);
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn an_absent_map_reads_as_empty_and_a_malformed_one_is_refused() {
        let base = temp_dir("alias-malformed");
        let paths = Paths::new(&base);
        // Absent is the normal first-run state.
        assert_eq!(read_aliases(&paths).expect("a read"), AliasMap::default());

        // A wrong-shaped file is an error rather than something to overwrite: replacing it silently would
        // discard an operator's mappings.
        std::fs::create_dir_all(paths.root()).expect("the directory");
        std::fs::write(paths.alias_file(), "[\"not\", \"an object\"]").expect("a write");
        let error = read_aliases(&paths).expect_err("a malformed file should be refused");
        assert!(matches!(error, ControlError::MalformedAliasFile { .. }));
        assert!(
            error
                .message()
                .contains("Delete it to start from an empty map"),
            "{}",
            error.message()
        );
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn an_unknown_tool_is_refused_rather_than_written() {
        // An alias map for a tool the interceptor does not know is a file nothing will ever read, and
        // accepting it would report success for work that cannot happen.
        let base = temp_dir("alias-unknown");
        let paths = Paths::new(&base);
        let error = write_aliases(&paths, "notatool", BTreeMap::new())
            .expect_err("an unknown tool should be refused");
        assert!(matches!(error, ControlError::UnknownTool { .. }));
        for tool in TOOLS {
            assert!(error.message().contains(tool), "{}", error.message());
        }
        assert!(
            !paths.alias_file().exists(),
            "nothing should have been written"
        );
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn the_ca_is_generated_once_and_reported_with_its_path_and_fingerprint() {
        let base = temp_dir("ca-generate");
        let paths = Paths::new(&base);

        // Before generation the status route reports absence without creating anything — a GET that
        // generated a key pair would mean opening the dashboard created one.
        let before = describe_authority(&paths);
        assert!(!before.exists);
        assert!(!paths.certificate().exists());

        let first = ensure_authority(&paths).expect("generation");
        assert!(first.exists);
        assert!(first.certificate_path.ends_with("root-ca.crt"), "{first:?}");
        assert_eq!(first.fingerprint.len(), 64, "a hex sha-256: {first:?}");
        // Installation is reported, not performed: it needs root, and this service is unprivileged.
        assert!(!first.can_install);
        assert!(
            first
                .install_command
                .contains("nullrouter-mitm-helper install-ca"),
            "{first:?}"
        );
        // The private key's path is never reported, though it exists on disk.
        assert!(paths.root().join("root-ca.key").is_file());
        assert!(!first.certificate_path.contains("root-ca.key"));

        // Idempotent: a re-run returns the same authority rather than rotating. Rotating while the old CA
        // is still trusted strands every client that trusted it.
        let second = ensure_authority(&paths).expect("a re-run");
        assert_eq!(first, second);
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn a_half_present_authority_is_refused_rather_than_completed() {
        // Generating a new key beside an existing certificate leaves clients trusting a certificate this
        // router can no longer sign with — a handshake failure with nothing in any log to explain it.
        let base = temp_dir("ca-corrupt");
        let paths = Paths::new(&base);
        std::fs::create_dir_all(paths.root()).expect("the directory");
        std::fs::write(paths.certificate(), "-----BEGIN CERTIFICATE-----").expect("a stray cert");

        let error = ensure_authority(&paths).expect_err("a half-present CA should be refused");
        assert!(matches!(error, ControlError::CorruptAuthority { .. }));
        assert!(
            error.message().contains("incomplete"),
            "{}",
            error.message()
        );
        // And no key was generated beside it.
        assert!(!paths.root().join("root-ca.key").exists());
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn an_unwritable_directory_is_reported_as_such_with_its_path() {
        // The failure an operator actually hits: `DATA_DIR` points somewhere the CA cannot be written. The
        // message has to name the directory, because that is where the fix is applied.
        //
        // Provoked with a *file* where the directory must go rather than with permission bits: mode 0o500
        // is not a barrier to a process running as root, and CI containers commonly do. `ENOTDIR` is
        // refused for everyone, so the assertion holds regardless of who runs it.
        let base = temp_dir("unwritable");
        let blocked = base.join("occupied");
        std::fs::write(&blocked, "not a directory").expect("a blocking file");

        let paths = Paths::new(&blocked);
        let error = ensure_authority(&paths).expect_err("an unwritable parent should fail");
        assert!(
            matches!(error, ControlError::NotWritable { .. }),
            "{error:?}"
        );
        assert!(
            error.message().contains(&blocked.display().to_string()),
            "the message must name the directory: {}",
            error.message()
        );
        assert!(
            error.message().contains("DATA_DIR"),
            "the message should say how to redirect it: {}",
            error.message()
        );

        // A write attempt fails the same way rather than panicking.
        let write_error = write_aliases(&paths, "cursor", BTreeMap::new())
            .expect_err("an unwritable parent should fail the write too");
        assert!(matches!(write_error, ControlError::NotWritable { .. }));

        std::fs::remove_dir_all(&base).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_permission_denied_directory_is_reported_the_same_way() {
        use std::os::unix::fs::PermissionsExt as _;

        // The permission-bit form of the same failure. Skipped when running as root, which bypasses the
        // bits entirely — asserting a refusal that cannot happen would make this test lie about what it
        // checked, and the `ENOTDIR` test above covers the message either way.
        // SAFETY: `geteuid` reads a process id and cannot fail or race.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let base = temp_dir("permission-denied");
        let mut permissions = std::fs::metadata(&base).expect("metadata").permissions();
        permissions.set_mode(0o500);
        std::fs::set_permissions(&base, permissions).expect("chmod");

        let paths = Paths::new(&base);
        let error = ensure_authority(&paths).expect_err("a read-only parent should fail");
        assert!(
            matches!(error, ControlError::NotWritable { .. }),
            "{error:?}"
        );

        let mut restore = std::fs::metadata(&base).expect("metadata").permissions();
        restore.set_mode(0o700);
        std::fs::set_permissions(&base, restore).ok();
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn the_data_directory_follows_the_same_discovery_as_the_rest_of_the_port() {
        // A nullrouter sharing a directory with 9Router must see the same alias map, or an interceptor
        // started from either reads a different one.
        let paths = Paths::new(Path::new("/tmp/explicit"));
        assert_eq!(paths.root(), Path::new("/tmp/explicit/mitm"));
        assert!(paths.certificate().ends_with("mitm/root-ca.crt"));
    }

    use std::path::Path;
}
