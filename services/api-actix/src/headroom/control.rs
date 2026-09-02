//! Starting, stopping and installing Headroom, under the same supervision as the tunnels.
//!
//! From `inspire/src/lib/headroom/process.js`. What it does is narrower than its reputation:
//!
//! * `spawn(binary, ["proxy", "--port", …])` — the `headroom` binary, with flags from a closed
//!   set of two booleans;
//! * `spawn(py, ["-m", "pip", "install", "--upgrade", spec])` — where `spec` is built from
//!   `HEADROOM_COMPRESSION_EXTRAS`, a fixed list, and never from the request;
//! * a pid file, then `SIGTERM`, then `SIGKILL` after about three seconds.
//!
//! No `sudo`, no root, no trust store. This was previously refused here on the grounds that the
//! Python interpreter is "not ours" — but owning the interpreter was never what the feature
//! needed. Invoking it safely was, and that is what [`nullrouter_procctl`] does.
//!
//! Two things are better than the original rather than equal to it. The pid is held by the
//! supervisor instead of a file, so the "is it running" question cannot be answered by a stale
//! file describing a pid the kernel has since given to something else. And `pip` gets a
//! deadline, where upstream waits on it indefinitely.

use std::sync::OnceLock;
use std::time::Duration;

use nullrouter_procctl::binary::{BinaryError, Executable};
use nullrouter_procctl::oneshot::{Output, Run, RunError};
use nullrouter_procctl::supervise::{
    ChildSpec, ReadyRule, RestartPolicy, Snapshot, StartError, Supervisor,
};
use nullrouter_procctl::{StopOutcome, argv::Argv};

/// Why a process operation did not happen.
#[derive(Debug, thiserror::Error)]
pub(super) enum ControlError {
    /// The `headroom` binary is not installed.
    #[error(
        "headroom is not installed. Install it with `pip install headroom-ai[proxy]`, or use \
         /api/headroom/extras to do that here."
    )]
    NotInstalled,
    /// No suitable Python was found.
    #[error(
        "no Python 3.10 or newer was found. Headroom needs one; install it and retry, or point \
         NULLROUTER_PYTHON at the interpreter to use."
    )]
    NoPython,
    /// A binary was found but is not one we will run.
    #[error(transparent)]
    Binary(#[from] BinaryError),
    /// The child could not be run.
    #[error(transparent)]
    Run(#[from] RunError),
    /// The daemon would not start.
    #[error(transparent)]
    Start(#[from] StartError),
    /// The interpreter refuses to install into itself.
    ///
    /// PEP 668: a distribution-managed Python marks itself `EXTERNALLY-MANAGED`, and `pip`
    /// refuses rather than fighting the system package manager. Reported as its own case
    /// because the fix is a virtualenv, not a retry.
    #[error(
        "this Python is externally managed (PEP 668), so pip will not install into it. Use a \
         virtual environment and point NULLROUTER_PYTHON at it, or install headroom-ai with \
         your system package manager. pip said: {detail}"
    )]
    ExternallyManaged {
        /// What pip actually printed, trimmed.
        detail: String,
    },
    /// `pip` ran and failed for some other reason.
    #[error("pip install failed ({code}): {detail}")]
    PipFailed {
        /// Rendered exit status.
        code: String,
        /// What pip printed.
        detail: String,
    },
}

impl ControlError {
    /// Which HTTP status fits.
    pub(super) const fn status(&self) -> actix_web::http::StatusCode {
        use actix_web::http::StatusCode;
        match self {
            // The capability exists; the dependency does not.
            Self::NotInstalled | Self::NoPython | Self::Binary(_) => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            // The environment is the way it is, and no retry changes it.
            Self::ExternallyManaged { .. } => StatusCode::CONFLICT,
            Self::Run(_) | Self::Start(_) | Self::PipFailed { .. } => StatusCode::BAD_GATEWAY,
        }
    }
}

/// The supervised Headroom daemon.
///
/// One per process, created on first use: the thread costs nothing while parked, but a
/// deployment that never touches Headroom should not start one at all.
fn daemon() -> &'static Supervisor {
    static DAEMON: OnceLock<Supervisor> = OnceLock::new();
    DAEMON.get_or_init(|| Supervisor::spawn("headroom", LOG_LINES))
}

/// Retained log lines from the daemon.
const LOG_LINES: usize = 200;

/// How long the proxy gets to start listening.
const STARTUP: Duration = Duration::from_secs(30);

/// How long it gets to shut down after `SIGTERM`.
///
/// Upstream's three seconds, which is what it allows before `SIGKILL`.
const SHUTDOWN: Duration = Duration::from_secs(3);

/// How long `pip install` may run.
///
/// Generous because it compiles wheels on some platforms, and `ml` pulls in large packages.
/// Bounded because upstream is not: a hung index leaves its install waiting forever.
const PIP_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// What the daemon prints once it is serving.
///
/// `headroom proxy` is a Python application that prints uvicorn's banner, so this wording belongs
/// to uvicorn rather than to headroom. It is used as an early signal, never as a requirement —
/// see [`SURVIVAL_GRACE`].
const READY_NEEDLE: &str = "Uvicorn running";

/// How long survival alone counts as a successful start.
///
/// Upstream's `STARTUP_TIMEOUT_MS`, exactly: it waits eight seconds and accepts a process that is
/// still alive, never reading the log at all. Requiring [`READY_NEEDLE`] outright would fail a
/// start that upstream would have accepted, the moment uvicorn rewords its banner — so the needle
/// ends the wait early when it appears and this is the floor when it does not. A child that dies
/// inside it still fails, which is the part of upstream's check that carries the weight.
const SURVIVAL_GRACE: Duration = Duration::from_secs(8);

/// Options the daemon accepts, mirroring upstream's `extrasProxyArgs`.
#[derive(Debug, Clone, Copy)]
pub(super) struct DaemonOptions {
    /// Port to listen on.
    pub(super) port: u16,
    /// `--code-aware`.
    pub(super) code_aware: bool,
    /// `--disable-kompress` when false.
    pub(super) kompress: bool,
}

/// Build the daemon's arguments.
///
/// Every flag is a literal and the only value is a `u16`, so there is nothing here a request
/// can shape into an argument.
fn daemon_argv(options: DaemonOptions) -> Vec<String> {
    let mut argv = Argv::new().word("proxy").flag("--port").port(options.port);
    if options.code_aware {
        argv = argv.flag("--code-aware");
    }
    if !options.kompress {
        argv = argv.flag("--disable-kompress");
    }
    argv.into_vec()
}

/// The environment the daemon runs with.
///
/// Cleared and rebuilt, as everywhere else in this port. `HOME` is included because Python
/// reads it for caches and a missing one turns into confusing import failures; `PATH` because
/// the proxy shells out to tokenizer helpers.
fn child_env() -> Vec<(String, String)> {
    let mut env = vec![("PATH".to_owned(), "/usr/local/bin:/usr/bin:/bin".to_owned())];
    if let Some(home) = std::env::var_os("HOME") {
        env.push(("HOME".to_owned(), home.to_string_lossy().into_owned()));
    }
    env
}

/// Start the daemon, replacing any current one.
pub(super) async fn start(options: DaemonOptions) -> Result<Snapshot, ControlError> {
    let binary = super::find_headroom_binary().ok_or(ControlError::NotInstalled)?;
    let program = Executable::verified(binary, "headroom")?;

    let _value = daemon()
        .start(ChildSpec {
            program,
            args: daemon_argv(options),
            env: child_env(),
            secrets: Vec::new(),
            ready: ReadyRule::SurvivesOr {
                needle: READY_NEEDLE,
                grace: SURVIVAL_GRACE,
            },
            startup_timeout: STARTUP,
            graceful_timeout: SHUTDOWN,
            restart: RestartPolicy::resilient(),
            log_capacity: LOG_LINES,
        })
        .await?;
    Ok(daemon().snapshot())
}

/// Stop the daemon. Idempotent.
pub(super) async fn stop() -> StopOutcome {
    daemon().stop().await
}

/// The daemon's current state.
pub(super) fn snapshot() -> Snapshot {
    daemon().snapshot()
}

/// The two things this service asks `pip` to do.
///
/// An enum rather than a string list because the subcommand and its flags are fixed per
/// operation: there is no arrangement of caller input that produces a third pip invocation.
#[derive(Debug, Clone)]
enum PipJob {
    /// `install --upgrade <spec>`.
    Install {
        /// A requirement built from `HEADROOM_COMPRESSION_EXTRAS`, never from a request.
        spec: String,
    },
    /// `uninstall -y <packages…>`.
    Uninstall {
        /// Distribution names from this repository's own marker table.
        packages: Vec<String>,
    },
}

impl PipJob {
    /// Build the argument vector.
    ///
    /// Every flag is a literal here. Only the requirement and the package names are values, and
    /// both still pass through [`Argv`]'s charset — so a future edit that let request text into
    /// either is caught rather than executed. A requirement is not an identifier, though:
    /// `headroom-ai[proxy,code]` contains brackets and commas, so it is checked against its own
    /// shape below instead.
    fn argv(&self) -> Result<Vec<String>, ControlError> {
        let base = Argv::new().flag("-m").word("pip");
        match self {
            Self::Install { spec } => {
                if !is_plausible_requirement(spec) {
                    return Err(ControlError::PipFailed {
                        code: "refused".to_owned(),
                        detail: format!("{spec:?} is not a requirement this service will install"),
                    });
                }
                Ok(base
                    .word("install")
                    .flag("--upgrade")
                    .requirement("requirement", spec)
                    .map_err(|error| ControlError::PipFailed {
                        code: "refused".to_owned(),
                        detail: error.to_string(),
                    })?
                    .into_vec())
            }
            Self::Uninstall { packages } => {
                let mut argv = base.word("uninstall").flag("-y");
                for package in packages {
                    argv = argv.token("package name", package).map_err(|error| {
                        ControlError::PipFailed {
                            code: "refused".to_owned(),
                            detail: error.to_string(),
                        }
                    })?;
                }
                Ok(argv.into_vec())
            }
        }
    }
}

/// Whether a string is a pip requirement of the shape this service builds.
///
/// `install_spec` produces `headroom-ai[proxy,code,ml]` and nothing else, so this accepts an
/// identifier optionally followed by a bracketed comma-separated extras list — and rejects
/// everything a requirement must never be able to carry: a URL, a path, a shell metacharacter,
/// a second requirement, or a leading dash that would read as a flag.
fn is_plausible_requirement(spec: &str) -> bool {
    /// Cap well above `headroom-ai[proxy,code,ml]`.
    const MAX: usize = 128;

    if spec.is_empty() || spec.len() > MAX || spec.starts_with('-') {
        return false;
    }
    let (name, extras) = match spec.split_once('[') {
        Some((name, rest)) => match rest.strip_suffix(']') {
            Some(extras) => (name, Some(extras)),
            None => return false,
        },
        None => (spec, None),
    };
    let identifier = |text: &str| {
        !text.is_empty()
            && text.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
            })
    };
    identifier(name)
        && extras.is_none_or(|extras| !extras.is_empty() && extras.split(',').all(identifier))
}

/// Run `pip install --upgrade <spec>` in the discovered interpreter.
pub(super) async fn pip_install(spec: &str) -> Result<Output, ControlError> {
    run_pip(&PipJob::Install {
        spec: spec.to_owned(),
    })
    .await
}

/// Run `pip uninstall -y <packages>`.
pub(super) async fn pip_uninstall(packages: &[String]) -> Result<Output, ControlError> {
    run_pip(&PipJob::Uninstall {
        packages: packages.to_vec(),
    })
    .await
}

/// Invoke `python -m pip` with a deadline, and classify the failure.
async fn run_pip(job: &PipJob) -> Result<Output, ControlError> {
    let python = super::find_python_310().ok_or(ControlError::NoPython)?;
    let program = Executable::verified(python, "python")?;

    let output = Run {
        program: &program,
        args: job.argv()?,
        timeout: PIP_TIMEOUT,
        env: child_env(),
        secrets: &[],
        max_capture: Run::DEFAULT_CAPTURE,
    }
    .call()
    .await?;

    if output.success() {
        return Ok(output);
    }

    let detail = output.failure_text().to_owned();
    if is_externally_managed(&detail) {
        return Err(ControlError::ExternallyManaged {
            detail: first_lines(&detail, 3),
        });
    }
    Err(ControlError::PipFailed {
        code: output
            .code
            .map_or_else(|| "signalled".to_owned(), |code| code.to_string()),
        detail: first_lines(&detail, 6),
    })
}

/// A machine-readable code for a failure, so a panel can branch without parsing prose.
pub(super) const fn code_for(error: &ControlError) -> &'static str {
    match error {
        ControlError::NotInstalled => "NOT_INSTALLED",
        ControlError::NoPython => "NO_PYTHON",
        ControlError::Binary(_) => "BINARY_REFUSED",
        ControlError::ExternallyManaged { .. } => "EXTERNALLY_MANAGED",
        ControlError::PipFailed { .. } => "PIP_FAILED",
        ControlError::Run(_) => "RUN_FAILED",
        ControlError::Start(_) => "START_FAILED",
    }
}

/// The useful tail of a successful run's output.
///
/// `pip` is verbose in success as well as failure, and the whole transcript in a JSON response
/// is noise. The last lines are where "Successfully installed …" appears.
pub(super) fn tail_of(output: &Output) -> String {
    let text = if output.stdout.trim().is_empty() {
        &output.stderr
    } else {
        &output.stdout
    };
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let start = lines.len().saturating_sub(8);
    lines.get(start..).unwrap_or_default().join("\n")
}

/// Whether pip refused because the interpreter is distribution-managed.
///
/// Matched on pip's own wording rather than on an exit code, because the code is the same `1`
/// it uses for a missing package or a network failure — and those three need different advice.
fn is_externally_managed(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    lowered.contains("externally-managed-environment")
        || lowered.contains("externally managed environment")
}

/// The first `count` non-empty lines, so a response carries the reason without pip's full trace.
fn first_lines(text: &str, count: usize) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(count)
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::{DaemonOptions, daemon_argv, first_lines, is_externally_managed};

    #[test]
    fn the_daemon_argv_matches_upstreams_flag_set() {
        let argv = daemon_argv(DaemonOptions {
            port: 8787,
            code_aware: true,
            kompress: true,
        });

        assert_eq!(argv, ["proxy", "--port", "8787", "--code-aware"]);
    }

    #[test]
    fn kompress_is_disabled_by_a_flag_rather_than_enabled_by_one() {
        // Upstream pushes `--disable-kompress` when the setting is false, so the default is on.
        // Inverting this would silently turn compression off for everyone.
        let on = daemon_argv(DaemonOptions {
            port: 8787,
            code_aware: false,
            kompress: true,
        });
        let off = daemon_argv(DaemonOptions {
            port: 8787,
            code_aware: false,
            kompress: false,
        });

        assert_eq!(on, ["proxy", "--port", "8787"]);
        assert_eq!(off, ["proxy", "--port", "8787", "--disable-kompress"]);
    }

    #[test]
    fn both_flags_can_appear_together() {
        let argv = daemon_argv(DaemonOptions {
            port: 9000,
            code_aware: true,
            kompress: false,
        });

        assert_eq!(
            argv,
            [
                "proxy",
                "--port",
                "9000",
                "--code-aware",
                "--disable-kompress"
            ]
        );
    }

    #[test]
    fn pep_668_is_recognised_from_pips_own_wording() {
        // The exit code is the same 1 pip uses for a missing package and for a network
        // failure, and those three need different advice, so the text is what decides.
        for refusal in [
            "error: externally-managed-environment",
            "ERROR: Externally-Managed-Environment\n× This environment is externally managed",
            "This environment is an externally managed environment",
        ] {
            assert!(is_externally_managed(refusal), "{refusal:?}");
        }

        for other in [
            "ERROR: Could not find a version that satisfies the requirement headroom-ai",
            "ERROR: Could not install packages due to an OSError: [Errno 28] No space left",
            "WARNING: Retrying after connection broken",
            "",
        ] {
            assert!(!is_externally_managed(other), "{other:?}");
        }
    }

    #[test]
    fn the_reported_detail_is_trimmed_to_the_useful_lines() {
        let noisy = "\n  ERROR: first line  \n\n  second line\n  third line\n  fourth line\n";

        assert_eq!(
            first_lines(noisy, 3),
            "ERROR: first line second line third line"
        );
        assert_eq!(first_lines("", 3), "");
        assert_eq!(first_lines("only", 3), "only");
    }
}
