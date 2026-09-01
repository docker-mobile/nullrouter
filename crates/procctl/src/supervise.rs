//! One supervised child, owned by one thread, with a bounded lifecycle.
//!
//! # Why a dedicated thread
//!
//! `tokio::process::Child` is bound to the runtime that created it: its exit is delivered
//! through that runtime's signal driver. An actix service has one runtime per worker
//! thread, so a child spawned while handling a request on worker 1 cannot be waited on or
//! reaped from worker 3 — the handle is there, but the machinery that notices the exit is
//! not. A tunnel that is started by one request and stopped by another hits exactly that.
//!
//! So the child lives on a thread this module owns, running its own `current_thread`
//! runtime, and every worker talks to it over a channel. This is the same shape
//! `nullrouter-logship` uses for the same reason.
//!
//! # What "tightly controlled" means here
//!
//! * the child is addressed only by the pid the kernel gave us, never by a command-line
//!   pattern, so nothing outside this process can be signalled;
//! * one child at a time, per supervisor. A second start stops the first;
//! * a start either reaches a declared readiness condition inside a deadline or is torn
//!   down. There is no "probably came up";
//! * the environment is cleared, so nothing the service holds leaks into the child;
//! * restarts are counted and bounded, with backoff. A child that cannot come up stops
//!   being restarted rather than becoming a spawn loop;
//! * the log tail is bounded and scrubbed;
//! * dropping the supervisor stops the child.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

use crate::binary::Executable;
use crate::logring::LogRing;
use crate::secret::Secret;
use crate::signal::StopOutcome;

mod engine;

/// How the supervisor decides a child is up.
///
/// Both daemons announce readiness only in their logs, so most of these inspect output.
/// The two that do not are here because "up" is not the same event for every child: a
/// daemon is up when it is listening, while `tailscale funnel --bg` is a command that
/// finishes.
#[derive(Clone)]
pub enum ReadyRule {
    /// A successful spawn is enough, and the child is expected to keep running.
    ///
    /// For `tailscaled`, whose readiness is a socket answering rather than anything it
    /// prints. The caller does that probe itself, with its own deadline.
    Spawned,
    /// The child is a command, not a daemon: it is ready when it exits zero.
    ///
    /// `tailscale funnel --bg` and `tailscale funnel reset` both do their work and return.
    /// A non-zero exit is a failed start, not a child to restart.
    CompletesSuccessfully,
    /// A substring has to appear this many times.
    ///
    /// `cloudflared tunnel run` logs `Registered tunnel connection` once per edge
    /// connection, and four is a fully established tunnel.
    Occurrences {
        /// The substring.
        needle: &'static str,
        /// How many times it has to appear.
        times: usize,
    },
    /// A line yields a value, which becomes the start's result.
    ///
    /// A quick tunnel's hostname is only ever printed; this is how it is captured.
    Extract(fn(&str) -> Option<String>),
}

impl std::fmt::Debug for ReadyRule {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawned => formatter.write_str("Spawned"),
            Self::CompletesSuccessfully => formatter.write_str("CompletesSuccessfully"),
            Self::Occurrences { needle, times } => formatter
                .debug_struct("Occurrences")
                .field("needle", needle)
                .field("times", times)
                .finish(),
            Self::Extract(_) => formatter.write_str("Extract(fn)"),
        }
    }
}

/// Whether and how often a child that exits on its own is started again.
#[derive(Debug, Clone, Copy)]
pub struct RestartPolicy {
    /// Restarts allowed before the supervisor gives up. `0` never restarts.
    pub max_attempts: u32,
    /// Delay before the first restart. Doubles per consecutive attempt.
    pub backoff: Duration,
    /// Cap on the doubling.
    pub max_backoff: Duration,
    /// Uptime after which a child counts as healthy and the attempt counter resets.
    ///
    /// Without this a daemon that has been up for a week would still be on its last
    /// allowed restart after two bad minutes at the start.
    pub reset_after: Duration,
}

impl RestartPolicy {
    /// Never restart: the caller wants to see the failure.
    #[must_use]
    pub const fn never() -> Self {
        Self {
            max_attempts: 0,
            backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(1),
            reset_after: Duration::from_secs(60),
        }
    }

    /// The policy both tunnels use: keep it up, but bounded.
    #[must_use]
    pub const fn resilient() -> Self {
        Self {
            max_attempts: 5,
            backoff: Duration::from_secs(2),
            max_backoff: Duration::from_secs(60),
            reset_after: Duration::from_secs(120),
        }
    }

    /// Backoff for the given consecutive attempt, saturating at [`Self::max_backoff`].
    #[must_use]
    pub fn delay_for(&self, attempt: u32) -> Duration {
        let shift = attempt.saturating_sub(1).min(16);
        let scaled = self
            .backoff
            .saturating_mul(2_u32.saturating_pow(shift));
        scaled.min(self.max_backoff)
    }
}

/// Everything needed to run one child.
#[derive(Debug, Clone)]
pub struct ChildSpec {
    /// The verified binary.
    pub program: Executable,
    /// Validated arguments.
    pub args: Vec<String>,
    /// The child's entire environment. Nothing is inherited.
    pub env: Vec<(String, String)>,
    /// Credentials to scrub from captured output. These are usually also in `env`.
    pub secrets: Vec<Secret>,
    /// How to decide the child is up.
    pub ready: ReadyRule,
    /// How long a start may take before it is torn down.
    pub startup_timeout: Duration,
    /// How long `SIGTERM` is given before `SIGKILL`.
    pub graceful_timeout: Duration,
    /// Restart behaviour after an unexpected exit.
    pub restart: RestartPolicy,
    /// Lines of output retained.
    pub log_capacity: usize,
}

/// What the supervisor is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// No child, and none wanted.
    Stopped,
    /// A child is spawned but has not met its readiness rule.
    Starting,
    /// A child is up.
    Running,
    /// Being stopped on request.
    Stopping,
    /// Waiting out a backoff before restarting.
    Backoff,
    /// The child could not be kept up. No further restarts.
    Failed,
}

impl State {
    /// A short lowercase name, for a status payload.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Backoff => "backoff",
            Self::Failed => "failed",
        }
    }
}

/// A point-in-time view, readable without waiting on the engine.
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// Current state.
    pub state: State,
    /// The child's pid while one exists.
    pub pid: Option<u32>,
    /// Whatever [`ReadyRule::Extract`] captured, for as long as the child runs.
    pub ready_value: Option<String>,
    /// How long the current child has been up.
    pub uptime: Option<Duration>,
    /// Restarts since the last successful manual start.
    pub restarts: u32,
    /// Why the last attempt ended, when it ended badly.
    pub last_error: Option<String>,
    /// Retained output, oldest first.
    pub logs: Vec<String>,
    /// Lines evicted from the ring.
    pub dropped_logs: u64,
}

impl Snapshot {
    /// Whether a child is currently up.
    #[must_use]
    pub const fn is_running(&self) -> bool {
        matches!(self.state, State::Running)
    }

    /// The view of a program that has never been started.
    ///
    /// Lets a caller hold its supervisors lazily and still answer a status poll, without
    /// spawning a thread for a facility that may never be used.
    #[must_use]
    pub const fn idle() -> Self {
        Self {
            state: State::Stopped,
            pid: None,
            ready_value: None,
            uptime: None,
            restarts: 0,
            last_error: None,
            logs: Vec::new(),
            dropped_logs: 0,
        }
    }
}

/// Why a start did not succeed.
#[derive(Debug, Error)]
pub enum StartError {
    /// The child could not be spawned.
    #[error("{program} could not be started: {source}")]
    Spawn {
        /// The program.
        program: String,
        /// The OS error.
        #[source]
        source: std::io::Error,
    },
    /// The child exited before meeting its readiness rule.
    #[error("{program} exited during startup with {status}. Last output:\n{tail}")]
    ExitedEarly {
        /// The program.
        program: String,
        /// Exit code or signal, rendered.
        status: String,
        /// Scrubbed log tail.
        tail: String,
    },
    /// The readiness rule was not met inside the deadline.
    #[error("{program} did not become ready within {}s and was stopped. Last output:\n{tail}", timeout.as_secs())]
    NotReady {
        /// The program.
        program: String,
        /// The deadline.
        timeout: Duration,
        /// Scrubbed log tail.
        tail: String,
    },
    /// The supervisor thread is gone.
    #[error("the supervisor for {program} is no longer running")]
    Gone {
        /// The program.
        program: String,
    },
}

/// Shared state, written by the engine and read by callers without a round-trip.
///
/// A status endpoint polls several times a second; making it wait behind a start that is
/// mid-deadline would make the panel appear to hang exactly when it matters most.
#[derive(Debug)]
struct Shared {
    state: State,
    pid: Option<u32>,
    ready_value: Option<String>,
    started_at: Option<Instant>,
    restarts: u32,
    last_error: Option<String>,
    logs: LogRing,
}

impl Shared {
    /// Initial state, before any child.
    fn new(log_capacity: usize) -> Self {
        Self {
            state: State::Stopped,
            pid: None,
            ready_value: None,
            started_at: None,
            restarts: 0,
            last_error: None,
            logs: LogRing::new(log_capacity, MAX_LOG_LINE),
        }
    }

    /// Render the current view.
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            state: self.state,
            pid: self.pid,
            ready_value: self.ready_value.clone(),
            uptime: self.started_at.map(|since| since.elapsed()),
            restarts: self.restarts,
            last_error: self.last_error.clone(),
            logs: self.logs.lines(),
            dropped_logs: self.logs.dropped(),
        }
    }
}

/// Cap on one retained log line. `cloudflared` prints long JSON error bodies.
const MAX_LOG_LINE: usize = 4 * 1024;

/// Instructions the engine accepts.
enum Command {
    /// Replace whatever is running with this child.
    Start {
        spec: Box<ChildSpec>,
        reply: oneshot::Sender<Result<Option<String>, StartError>>,
    },
    /// Stop the current child, if any.
    Stop {
        reply: oneshot::Sender<StopOutcome>,
    },
}

/// A handle to one supervised child.
///
/// Cloning shares the same child. Dropping the last clone stops it: the command channel
/// closes, the engine falls out of its loop, and the child is killed on the way out.
#[derive(Debug, Clone)]
pub struct Supervisor {
    program: &'static str,
    commands: mpsc::Sender<Command>,
    shared: Arc<Mutex<Shared>>,
}

/// Depth of the command queue. Starts and stops arrive from a panel; a handful in flight
/// is already more than a person can produce.
const COMMAND_QUEUE: usize = 8;

impl Supervisor {
    /// Start a supervisor thread for one program.
    ///
    /// `program` names it in messages. `log_capacity` bounds the retained tail.
    #[must_use]
    pub fn spawn(program: &'static str, log_capacity: usize) -> Self {
        let (commands, receiver) = mpsc::channel(COMMAND_QUEUE);
        let shared = Arc::new(Mutex::new(Shared::new(log_capacity)));
        engine::launch(program, receiver, Arc::clone(&shared));
        Self {
            program,
            commands,
            shared,
        }
    }

    /// Start a child, replacing any current one, and wait for its readiness rule.
    ///
    /// `Ok(Some(value))` carries whatever [`ReadyRule::Extract`] captured.
    pub async fn start(&self, spec: ChildSpec) -> Result<Option<String>, StartError> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(Command::Start {
                spec: Box::new(spec),
                reply,
            })
            .await
            .map_err(|_closed| StartError::Gone {
                program: self.program.to_owned(),
            })?;
        answer.await.map_err(|_dropped| StartError::Gone {
            program: self.program.to_owned(),
        })?
    }

    /// Stop the current child. Idempotent.
    pub async fn stop(&self) -> StopOutcome {
        let (reply, answer) = oneshot::channel();
        if self.commands.send(Command::Stop { reply }).await.is_err() {
            return StopOutcome::NotRunning;
        }
        answer.await.unwrap_or(StopOutcome::NotRunning)
    }

    /// The current view, without waiting on the engine.
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        match self.shared.lock() {
            Ok(shared) => shared.snapshot(),
            // A poisoned lock means the engine panicked mid-update. The state is still
            // readable and reporting it beats turning a status poll into a failure.
            Err(poisoned) => poisoned.into_inner().snapshot(),
        }
    }

    /// The program this supervises.
    #[must_use]
    pub const fn program(&self) -> &'static str {
        self.program
    }
}
