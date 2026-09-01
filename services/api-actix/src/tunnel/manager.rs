//! The one place that turns a catalog row into a running process.
//!
//! Holds exactly two supervisors — one for `cloudflared`, one for `tailscaled` — so "at most
//! one tunnel, and we know its pid" is a structural fact rather than a convention. Upstream
//! keeps module-level `let cloudflaredProcess = null` plus a pid file plus a
//! `pkill`-by-command-line fallback, which is three sources of truth for one process and is
//! why it needs the fallback at all.
//!
//! A one-shot operation runs and returns. A supervised operation replaces whatever that
//! tool's supervisor is running. Nothing else can start a process.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use nullrouter_procctl::binary::{BinaryError, Executable};
use nullrouter_procctl::oneshot::{Output, Run, RunError};
use nullrouter_procctl::secret::Secret;
use nullrouter_procctl::supervise::{Snapshot, StartError, Supervisor};
use nullrouter_procctl::{StopOutcome, argv::ArgError};

use super::catalog::{Args, Mode, Operation, Tool};
use super::{cloudflared, tailscale};

/// Why an operation did not run.
#[derive(Debug, thiserror::Error)]
pub(crate) enum OpError {
    /// The binary is not installed, or is not one we will execute.
    #[error(transparent)]
    Binary(#[from] BinaryError),
    /// A parameter was missing or unacceptable.
    #[error("{0}")]
    Argument(#[from] ArgError),
    /// The child could not be run to completion.
    #[error(transparent)]
    Run(#[from] RunError),
    /// A supervised start failed.
    #[error(transparent)]
    Start(#[from] StartError),
    /// No such operation.
    #[error("{0} is not an operation this service can run")]
    Unknown(String),
}

impl OpError {
    /// Which HTTP status fits.
    ///
    /// A missing binary is the operator's environment, not a bad request, and a panel needs
    /// to tell the two apart to say anything useful.
    pub(crate) const fn status(&self) -> actix_web::http::StatusCode {
        use actix_web::http::StatusCode;
        match self {
            Self::Argument(_) | Self::Unknown(_) => StatusCode::BAD_REQUEST,
            Self::Binary(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::Run(_) | Self::Start(_) => StatusCode::BAD_GATEWAY,
        }
    }
}

/// What running an operation produced.
#[derive(Debug)]
pub(crate) enum Outcome {
    /// A one-shot finished.
    Finished(Output),
    /// A supervised child is up, with whatever its readiness rule captured.
    Supervised(Option<String>),
}

/// Owns the supervisors and runs catalog operations.
#[derive(Debug, Clone)]
pub struct Manager {
    inner: Arc<Inner>,
}

/// The shared half, so cloning a `Manager` shares the same children.
///
/// Each supervisor is created on first use rather than up front. A supervisor owns an OS
/// thread, and a deployment that never opens a tunnel — or a test binary that builds an app
/// per case — should not pay for two threads per process to hold a facility nothing touches.
/// A status poll for a tool that was never started answers from [`Snapshot::idle`].
#[derive(Debug, Default)]
struct Inner {
    cloudflared: OnceLock<Supervisor>,
    tailscaled: OnceLock<Supervisor>,
}

/// The minimal environment every child gets.
///
/// `env_clear` then this. A tunnel binary needs no configuration from our environment, and
/// our environment holds provider API keys. `PATH` is here because both binaries shell out
/// occasionally — `tailscaled` for network helpers — and an empty `PATH` turns that into a
/// confusing failure rather than a clean one.
fn baseline_env() -> Vec<(String, String)> {
    vec![(
        "PATH".to_owned(),
        "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_owned(),
    )]
}

impl Manager {
    /// Build the manager and its supervisor threads.
    ///
    /// The threads park on a channel until something is started, so an installation that
    /// never opens a tunnel pays two idle threads and nothing else.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner::default()),
        }
    }

    /// The supervisor for one tool, starting its thread if this is the first use.
    fn supervisor(&self, tool: Tool) -> &Supervisor {
        match tool {
            Tool::Cloudflared => self
                .inner
                .cloudflared
                .get_or_init(|| Supervisor::spawn("cloudflared", cloudflared::LOG_LINES)),
            Tool::Tailscale => self
                .inner
                .tailscaled
                .get_or_init(|| Supervisor::spawn("tailscaled", tailscale::LOG_LINES)),
        }
    }

    /// The supervisor for one tool, only if it has already been used.
    fn existing(&self, tool: Tool) -> Option<&Supervisor> {
        match tool {
            Tool::Cloudflared => self.inner.cloudflared.get(),
            Tool::Tailscale => self.inner.tailscaled.get(),
        }
    }

    /// The current view of one tool's supervised child.
    ///
    /// A tool that has never been started reads as stopped without a thread being created for
    /// it, so polling status is free until something is actually run.
    pub(crate) fn snapshot(&self, tool: Tool) -> Snapshot {
        self.existing(tool)
            .map_or_else(Snapshot::idle, Supervisor::snapshot)
    }

    /// Resolve the binary an operation needs.
    fn resolve(operation: &Operation) -> Result<Executable, BinaryError> {
        match operation.tool {
            Tool::Cloudflared => cloudflared::CLOUDFLARED.resolve(None),
            Tool::Tailscale => tailscale::TAILSCALE.resolve(None),
        }
    }

    /// Run one operation by id.
    pub(crate) async fn run(&self, id: &str, args: &Args) -> Result<Outcome, OpError> {
        let operation = super::catalog::operation(id).ok_or_else(|| OpError::Unknown(id.to_owned()))?;
        self.run_operation(operation, args).await
    }

    /// Check that an operation's arguments are acceptable, without running anything.
    ///
    /// Called before `ensure_daemon`, so a rejected value cannot cause a daemon to be
    /// started as a side effect of validating it — and so the error a caller sees is about
    /// their value rather than about a dependency they were not asking for.
    pub(crate) fn validate(operation: &'static Operation, args: &Args) -> Result<(), OpError> {
        let _argv = (operation.build)(args)?;
        Ok(())
    }

    /// Run one operation.
    pub(crate) async fn run_operation(
        &self,
        operation: &'static Operation,
        args: &Args,
    ) -> Result<Outcome, OpError> {
        let program = Self::resolve(operation)?;
        let arguments = (operation.build)(args)?.into_vec();

        let mut env = baseline_env();
        let mut secrets = Vec::new();
        if let Some(build_env) = operation.env {
            for (key, value) in build_env(args) {
                secrets.push(Secret::new(value.clone()));
                env.push((key, value));
            }
        }

        match operation.mode {
            Mode::OneShot { timeout } => {
                let borrowed: Vec<&Secret> = secrets.iter().collect();
                let output = Run {
                    program: &program,
                    args: arguments,
                    timeout,
                    env,
                    secrets: &borrowed,
                    max_capture: Run::DEFAULT_CAPTURE,
                }
                .call()
                .await?;
                Ok(Outcome::Finished(output))
            }
            Mode::Supervised => {
                let spec = if operation.id == "cloudflared.tunnel.quick" {
                    cloudflared::quick_tunnel_child(program, arguments)
                } else {
                    cloudflared::named_tunnel_child(program, arguments, env, secrets)
                };
                let value = self.supervisor(operation.tool).start(spec).await?;
                Ok(Outcome::Supervised(value))
            }
        }
    }

    /// Stop one tool's supervised child. Idempotent, and free if nothing ever started.
    pub(crate) async fn stop(&self, tool: Tool) -> StopOutcome {
        match self.existing(tool) {
            Some(supervisor) => supervisor.stop().await,
            None => StopOutcome::NotRunning,
        }
    }

    /// Make sure `tailscaled` is up, and wait for its socket to answer.
    ///
    /// Two steps because they are two different questions: the supervisor knows the process
    /// exists, and only `status --json` knows the socket is serving. Upstream answers the
    /// second with `await new Promise(r => setTimeout(r, 3000))` — a fixed three-second sleep
    /// after the spawn — which is both slower than necessary when the daemon is quick and
    /// wrong when it is slow.
    pub(crate) async fn ensure_daemon(&self) -> Result<(), OpError> {
        if self.daemon_answers().await {
            return Ok(());
        }

        let program = tailscale::TAILSCALED.resolve(None)?;
        let arguments = tailscale::daemon_argv()?.into_vec();
        // The daemon writes its state here, so it has to exist before it starts.
        if let Ok(dir) = tailscale::state_dir() {
            let _created = std::fs::create_dir_all(dir.path());
        }
        let spec = tailscale::daemon_child(program, arguments);
        let _value = self.supervisor(Tool::Tailscale).start(spec).await?;

        // Poll rather than sleep: ready as soon as it is ready, and a bounded failure when
        // it never is.
        for _attempt in 0..40_u32 {
            if self.daemon_answers().await {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        Err(OpError::Start(StartError::NotReady {
            program: "tailscaled".to_owned(),
            timeout: Duration::from_secs(10),
            tail: self.snapshot(Tool::Tailscale).logs.join("\n"),
        }))
    }

    /// Whether our own `tailscaled` socket answers a status query.
    async fn daemon_answers(&self) -> bool {
        let Some(operation) = super::catalog::operation("tailscale.status") else {
            return false;
        };
        matches!(
            self.run_operation(operation, &Args::default()).await,
            Ok(Outcome::Finished(output)) if output.success()
        )
    }
}

impl Default for Manager {
    fn default() -> Self {
        Self::new()
    }
}
