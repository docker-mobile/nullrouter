//! Where PXPIPE lives, whether it is usable, and installing it.
//!
//! Ports `inspire/src/lib/pxpipe/install.js`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::PACKAGE;

/// An `npm install` on a cold cache legitimately takes minutes.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// The legacy product's data directory under `$HOME`. An external program's
/// address on disk: it must keep this spelling for a shared install to be found.
const LEGACY_DATA_DIR: &str = ".9router";

/// The same directory in the Windows `%APPDATA%` layout, and the same rule.
const LEGACY_DATA_DIR_WINDOWS: &str = "9router";

/// Lines of `install.log` returned to the dashboard.
const LOG_TAIL_LINES: usize = 200;

/// Directories added to `PATH` when looking for `npm` and `node`.
///
/// A service started by systemd or launchd often has a minimal `PATH` that omits
/// wherever Node was installed, so "npm not found" would be a lie about the machine
/// rather than a fact about it.
const EXTRA_BINS: [&str; 5] = [
    "/usr/local/bin",
    "/opt/homebrew/bin",
    "/usr/bin",
    "/bin",
    "/snap/bin",
];

/// Resolved filesystem locations for one data directory.
#[derive(Debug, Clone)]
pub struct Paths {
    /// `$DATA_DIR/pxpipe`.
    pub root: PathBuf,
}

impl Paths {
    /// Paths under an explicit data directory.
    pub fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join("pxpipe"),
        }
    }

    /// Paths under the data directory this deployment uses.
    ///
    /// `DATA_DIR` first, then the default per-user location. The same discovery
    /// `state-actix` does for imports, so a nullrouter sharing a directory with
    /// the legacy product sees the same install rather than reinstalling beside
    /// it.
    pub fn discover() -> Self {
        if let Ok(configured) = std::env::var("DATA_DIR")
            && !configured.trim().is_empty()
        {
            return Self::new(Path::new(configured.trim()));
        }
        if let Some(home) = std::env::var_os("HOME") {
            return Self::new(&Path::new(&home).join(LEGACY_DATA_DIR));
        }
        if let Some(appdata) = std::env::var_os("APPDATA") {
            return Self::new(&Path::new(&appdata).join(LEGACY_DATA_DIR_WINDOWS));
        }
        Self::new(Path::new("."))
    }

    /// The installed package's directory.
    pub fn package_root(&self) -> PathBuf {
        self.root.join("node_modules").join(PACKAGE)
    }

    /// The library entry point the transform is loaded from.
    pub fn library_entry(&self) -> PathBuf {
        self.package_root()
            .join("dist")
            .join("core")
            .join("library.js")
    }

    /// Where `npm install` output is appended.
    pub fn install_log(&self) -> PathBuf {
        self.root.join("install.log")
    }

    /// The per-request event log.
    pub fn events(&self) -> PathBuf {
        self.root.join("events.jsonl")
    }

    /// The rotated event log.
    pub fn rotated_events(&self) -> PathBuf {
        self.root.join("events.jsonl.1")
    }
}

/// What is on disk.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallInfo {
    /// `true` when both the manifest and the library entry are present.
    ///
    /// Both are checked because an interrupted install leaves a directory with a
    /// manifest and no code, which would otherwise report as installed and then
    /// fail on every transform.
    pub installed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The package's `engines.node`, when it declares one.
    ///
    /// Read because it matters: `pxpipe-proxy` 0.13 requires Node 20.19, and on an
    /// older Node it installs cleanly, imports cleanly, and then fails every
    /// transform with `crypto is not defined`. That message sends a user looking at
    /// their own request rather than at their Node, so the requirement is checked
    /// and reported instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires_node: Option<String>,
}

/// Read the install state.
pub fn install_info(paths: &Paths) -> InstallInfo {
    let manifest = paths.package_root().join("package.json");
    if !manifest.is_file() || !paths.library_entry().is_file() {
        return InstallInfo::default();
    }
    let manifest = std::fs::read_to_string(&manifest)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
    let field = |name: &str| {
        manifest
            .as_ref()
            .and_then(|manifest| manifest.get(name))
            .and_then(|value| value.as_str())
            .map(str::to_owned)
    };
    InstallInfo {
        installed: true,
        version: field("version"),
        path: Some(paths.package_root().to_string_lossy().into_owned()),
        requires_node: manifest
            .as_ref()
            .and_then(|manifest| manifest.pointer("/engines/node"))
            .and_then(|value| value.as_str())
            .map(str::to_owned),
    }
}

/// Whether `version` satisfies a `>=x.y.z` requirement.
///
/// Only that one form, which is what `pxpipe-proxy` declares. Anything else answers
/// `None` — "cannot tell" — rather than guessing: reporting a wrong Node version as
/// the cause of an unrelated failure would be worse than not reporting one.
pub fn node_satisfies(requirement: &str, version: &str) -> Option<bool> {
    let wanted = requirement.trim().strip_prefix(">=")?.trim();
    if wanted.is_empty() || !wanted.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return None;
    }
    Some(semver_parts(version) >= semver_parts(wanted))
}

/// `major.minor.patch` as numbers, missing or unparsable parts as 0.
fn semver_parts(version: &str) -> (u64, u64, u64) {
    let mut parts = version
        .trim()
        .trim_start_matches('v')
        .split(['.', '-', '+'])
        .map(|part| part.parse::<u64>().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

/// `PATH` with the usual Node install locations appended.
fn extended_path() -> String {
    // The inherited `PATH` first, and the fallbacks only after it. The other order
    // looks equivalent and is not: a user who installed a newer Node — through nvm,
    // fnm, asdf, or by unpacking a tarball — puts it on their `PATH` precisely to be
    // preferred, and searching `/usr/bin` first would silently run the distribution's
    // older one instead. pxpipe requires Node 20.19, so on a box whose system Node is
    // 18 that is the difference between the token saver working and the token saver
    // refusing to load.
    let mut parts: Vec<String> = Vec::with_capacity(EXTRA_BINS.len() + 1);
    if let Ok(existing) = std::env::var("PATH")
        && !existing.is_empty()
    {
        parts.push(existing);
    }
    parts.extend(EXTRA_BINS.iter().map(|dir| (*dir).to_owned()));
    parts.join(":")
}

/// The absolute path of an executable on the extended `PATH`.
///
/// Resolved by walking `PATH` rather than by running `which`, so no shell is
/// involved and nothing here can become a command.
pub fn find_executable(name: &str) -> Option<PathBuf> {
    for dir in extended_path().split(':').filter(|dir| !dir.is_empty()) {
        let candidate = Path::new(dir).join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// The `npm` binary, if this machine has one.
pub fn find_npm() -> Option<PathBuf> {
    find_executable("npm")
}

/// The `node` binary, if this machine has one.
pub fn find_node() -> Option<PathBuf> {
    find_executable("node")
}

/// The result of an install attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallOutcome {
    /// The package is present and usable.
    Installed(InstallInfo),
    /// This machine has no npm, so nothing can be installed here.
    ///
    /// Reported distinctly because it is the user's environment rather than a
    /// failure of the install, and the fix is different.
    NpmMissing,
    /// npm ran and failed. Carries the reason; the log has the detail.
    Failed { message: String },
}

/// Install (or reinstall, which is how "repair" works) the package.
///
/// Blocking: `npm install` is a subprocess with a five-minute budget, so callers on
/// an async runtime must move this to a blocking thread.
pub fn install(paths: &Paths) -> InstallOutcome {
    let Some(npm) = find_npm() else {
        return InstallOutcome::NpmMissing;
    };
    if let Err(message) = prepare_directory(paths) {
        return InstallOutcome::Failed { message };
    }
    match run_npm(paths, &npm) {
        Err(message) => InstallOutcome::Failed { message },
        Ok(()) => settle_install(paths),
    }
}

/// Make the directory npm will install into.
fn prepare_directory(paths: &Paths) -> Result<(), String> {
    std::fs::create_dir_all(&paths.root)
        .map_err(|error| format!("could not create {}: {error}", paths.root.display()))?;
    // npm refuses to install into a directory with no manifest, and would otherwise
    // walk upwards and install into whatever it found.
    let manifest = paths.root.join("package.json");
    if !manifest.is_file() {
        std::fs::write(
            &manifest,
            "{\n  \"name\": \"nullrouter-pxpipe-host\",\n  \"private\": true\n}\n",
        )
        .map_err(|error| format!("could not write {}: {error}", manifest.display()))?;
    }
    Ok(())
}

/// What the install left behind, checked rather than assumed.
fn settle_install(paths: &Paths) -> InstallOutcome {
    let info = install_info(paths);
    if info.installed {
        InstallOutcome::Installed(info)
    } else {
        // npm reported success but the package is not usable — a partial publish, or
        // a layout change in the package. Saying "installed" here would produce a
        // transform that fails on every request.
        InstallOutcome::Failed {
            message: "npm reported success but the package is missing — see install.log".to_owned(),
        }
    }
}

/// Run `npm install`, logging to `install.log` and killing a hung one.
fn run_npm(paths: &Paths, npm: &Path) -> Result<(), String> {
    append_log(
        paths,
        &format!(
            "\n[{}] npm install {PACKAGE}@latest\n",
            crate::events::now_iso()
        ),
    );
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths.install_log())
        .map_err(|error| format!("could not open the install log: {error}"))?;
    let errors = log
        .try_clone()
        .map_err(|error| format!("could not open the install log: {error}"))?;

    let spawned = Command::new(npm)
        .args([
            "install",
            &format!("{PACKAGE}@latest"),
            "--no-audit",
            "--no-fund",
            "--omit=dev",
        ])
        .current_dir(&paths.root)
        .env("PATH", extended_path())
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(errors))
        .spawn();

    let mut child = spawned.map_err(|error| format!("could not run npm: {error}"))?;

    let deadline = Instant::now() + INSTALL_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    // Killed rather than left running: an npm install that has hung
                    // holds the lock on the directory and blocks every later attempt.
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(format!("lost track of the npm process: {error}")),
        }
    };

    let Some(status) = status else {
        return Err("npm install timed out after 5 minutes — see install.log".to_owned());
    };
    if !status.success() {
        return Err(format!(
            "npm install exited with {} — see install.log",
            status.code().unwrap_or(-1)
        ));
    }
    Ok(())
}

/// Append a line to the install log, best-effort.
fn append_log(paths: &Paths, line: &str) {
    use std::io::Write as _;
    let _ = std::fs::create_dir_all(&paths.root);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths.install_log())
    {
        let _ = file.write_all(line.as_bytes());
    }
}

/// The tail of the install log, or empty when there is none.
pub fn install_log_tail(paths: &Paths) -> String {
    let Ok(text) = std::fs::read_to_string(paths.install_log()) else {
        return String::new();
    };
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let start = lines.len().saturating_sub(LOG_TAIL_LINES);
    lines.get(start..).unwrap_or_default().join("\n")
}

#[cfg(test)]
mod tests {
    use super::{InstallInfo, Paths, install_info, install_log_tail, node_satisfies};
    use std::path::Path;

    /// A package tree as npm would leave it.
    fn install_package(paths: &Paths, version: &str, with_entry: bool) {
        let root = paths.package_root();
        std::fs::create_dir_all(&root).expect("create package root");
        std::fs::write(
            root.join("package.json"),
            format!("{{\"name\":\"pxpipe-proxy\",\"version\":\"{version}\"}}"),
        )
        .expect("write manifest");
        if with_entry {
            let entry = paths.library_entry();
            std::fs::create_dir_all(entry.parent().expect("parent")).expect("create dist");
            std::fs::write(entry, "export function transformAnthropicMessages() {}")
                .expect("write entry");
        }
    }

    #[test]
    fn paths_are_derived_from_the_data_directory() {
        let paths = Paths::new(Path::new("/data"));
        assert_eq!(paths.root, Path::new("/data/pxpipe"));
        assert_eq!(
            paths.package_root(),
            Path::new("/data/pxpipe/node_modules/pxpipe-proxy")
        );
        // The entry point npm's own layout puts the library at.
        assert_eq!(
            paths.library_entry(),
            Path::new("/data/pxpipe/node_modules/pxpipe-proxy/dist/core/library.js")
        );
        assert_eq!(paths.events(), Path::new("/data/pxpipe/events.jsonl"));
    }

    #[test]
    fn an_absent_package_is_not_installed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path());
        assert_eq!(install_info(&paths), InstallInfo::default());
        assert!(!install_info(&paths).installed);
    }

    #[test]
    fn a_complete_package_reports_its_version_and_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path());
        install_package(&paths, "1.4.2", true);

        let info = install_info(&paths);
        assert!(info.installed);
        assert_eq!(info.version.as_deref(), Some("1.4.2"));
        assert!(
            info.path
                .as_deref()
                .is_some_and(|path| path.ends_with("pxpipe-proxy"))
        );
    }

    #[test]
    fn an_interrupted_install_is_not_reported_as_installed() {
        // A manifest with no library entry is what an interrupted `npm install`
        // leaves behind. Calling that installed would mean every transform fails
        // with a module-not-found instead of the panel offering a repair.
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path());
        install_package(&paths, "1.4.2", false);
        assert!(!install_info(&paths).installed);
    }

    #[test]
    fn a_manifest_with_no_version_is_still_installed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path());
        install_package(&paths, "1.0.0", true);
        std::fs::write(
            paths.package_root().join("package.json"),
            r#"{"name":"pxpipe-proxy"}"#,
        )
        .expect("rewrite manifest");

        let info = install_info(&paths);
        // The code is there and loadable; only the version is unknown.
        assert!(info.installed);
        assert_eq!(info.version, None);
    }

    #[test]
    fn a_corrupt_manifest_is_not_installed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path());
        install_package(&paths, "1.0.0", true);
        std::fs::write(paths.package_root().join("package.json"), "{not json")
            .expect("corrupt manifest");
        // The entry exists, so this stays installed with an unknown version rather
        // than being reported absent — the module may well still load.
        let info = install_info(&paths);
        assert!(info.installed);
        assert_eq!(info.version, None);
    }

    #[test]
    fn the_log_tail_is_bounded_and_empty_when_there_is_no_log() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path());
        assert_eq!(install_log_tail(&paths), "");

        std::fs::create_dir_all(&paths.root).expect("create root");
        let lines: Vec<String> = (0..500).map(|index| format!("line {index}")).collect();
        std::fs::write(paths.install_log(), lines.join("\n")).expect("write log");

        let tail = install_log_tail(&paths);
        assert_eq!(tail.lines().count(), 200, "the tail must be bounded");
        // The *last* lines, which is what a failure is at the end of.
        assert!(tail.ends_with("line 499"), "got {tail}");
        assert!(!tail.contains("line 299"), "kept too much");
    }

    #[test]
    fn a_node_requirement_is_compared_by_version_not_by_string() {
        // The case that matters: pxpipe-proxy 0.13 wants 20.19, and 18 installs
        // cleanly and then fails every transform.
        assert_eq!(node_satisfies(">=20.19", "18.20.4"), Some(false));
        assert_eq!(node_satisfies(">=20.19", "20.19.0"), Some(true));
        assert_eq!(node_satisfies(">=20.19", "v22.3.1"), Some(true));
        // Not a lexical comparison: "9" is not below "20".
        assert_eq!(node_satisfies(">=20.19", "20.9.0"), Some(false));
        assert_eq!(node_satisfies(">=8", "10.0.0"), Some(true));
        // A pre-release still compares on its numbers.
        assert_eq!(node_satisfies(">=20.19", "21.0.0-nightly"), Some(true));
    }

    #[test]
    fn an_unreadable_requirement_is_not_guessed_at() {
        // "Cannot tell" rather than a verdict: naming the wrong cause for a failure
        // is worse than naming none, so these let the worker run and answer for
        // itself.
        for requirement in ["^20.19", "20.x", ">=20 <23", "", "latest", ">="] {
            assert_eq!(
                node_satisfies(requirement, "18.0.0"),
                None,
                "{requirement} is not a form this reads"
            );
        }
    }

    #[test]
    fn a_declared_engine_requirement_is_read_from_the_manifest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path());
        let root = paths.package_root();
        std::fs::create_dir_all(root.join("dist").join("core")).expect("create tree");
        std::fs::write(
            root.join("package.json"),
            "{\"name\":\"pxpipe-proxy\",\"version\":\"0.13.2\",\"engines\":{\"node\":\">=20.19\"}}",
        )
        .expect("write manifest");
        std::fs::write(paths.library_entry(), "export const x = 1;\n").expect("write entry");

        let info = install_info(&paths);
        assert!(info.installed);
        assert_eq!(info.version.as_deref(), Some("0.13.2"));
        assert_eq!(info.requires_node.as_deref(), Some(">=20.19"));
    }

    #[test]
    fn a_package_declaring_no_engine_reports_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path());
        let root = paths.package_root();
        std::fs::create_dir_all(root.join("dist").join("core")).expect("create tree");
        std::fs::write(root.join("package.json"), "{\"version\":\"1.0.0\"}").expect("manifest");
        std::fs::write(paths.library_entry(), "export const x = 1;\n").expect("entry");
        assert_eq!(install_info(&paths).requires_node, None);
    }
}
