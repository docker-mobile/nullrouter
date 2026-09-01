//! Running a command to completion under a hard deadline.
//!
//! Every status probe here is a one-shot: `tailscale status --json`,
//! `tailscale funnel status --json`, `cloudflared --version`. Upstream runs these with
//! `execSync`, which blocks its event loop, and several call sites pass no timeout at all
//! — so a wedged daemon socket stalls the whole application rather than one probe.
//!
//! Three properties matter here, and each is a way one of these probes has hung in
//! practice:
//!
//! * the deadline is enforced by killing the child, not by abandoning the future. An
//!   abandoned future leaves the process running and its pipes open;
//! * output is captured with a size cap, so a child that decides to stream cannot exhaust
//!   memory;
//! * output is read *while* the child runs. Waiting first and reading after deadlocks the
//!   moment the child fills the 64KB pipe buffer.

use std::process::Stdio;
use std::time::Duration;

use thiserror::Error;
use tokio::io::AsyncReadExt as _;

use crate::binary::Executable;
use crate::secret::{Secret, scrub};

/// Why a one-shot did not produce a result.
#[derive(Debug, Error)]
pub enum RunError {
    /// The child could not be started at all.
    #[error("{program} could not be started: {source}")]
    Spawn {
        /// The program.
        program: String,
        /// The OS error.
        #[source]
        source: std::io::Error,
    },
    /// The deadline passed and the child was killed.
    #[error("{program} did not finish within {}ms and was killed. Output so far: {tail}", timeout.as_millis())]
    TimedOut {
        /// The program.
        program: String,
        /// The deadline.
        timeout: Duration,
        /// Whatever was captured before the kill.
        tail: String,
    },
    /// The pipes could not be read.
    #[error("{program}'s output could not be read: {source}")]
    Io {
        /// The program.
        program: String,
        /// The OS error.
        #[source]
        source: std::io::Error,
    },
}

/// What a finished one-shot produced.
#[derive(Debug, Clone)]
pub struct Output {
    /// Exit code, or `None` when a signal ended the child.
    pub code: Option<i32>,
    /// Captured stdout, scrubbed and lossily decoded.
    pub stdout: String,
    /// Captured stderr, scrubbed and lossily decoded.
    pub stderr: String,
    /// Whether either stream hit the capture cap.
    pub truncated: bool,
}

impl Output {
    /// Whether the child exited zero.
    #[must_use]
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }

    /// stderr if it has anything, otherwise stdout: the failure message, wherever the
    /// program chose to put it.
    #[must_use]
    pub fn failure_text(&self) -> &str {
        if self.stderr.trim().is_empty() {
            self.stdout.trim()
        } else {
            self.stderr.trim()
        }
    }
}

/// How to run one command.
#[derive(Debug)]
pub struct Run<'a> {
    /// The verified binary.
    pub program: &'a Executable,
    /// Its arguments, already validated by [`crate::argv`].
    pub args: Vec<String>,
    /// Hard deadline.
    pub timeout: Duration,
    /// Environment entries added to an otherwise cleared environment.
    ///
    /// Cleared rather than inherited: the service's own environment holds provider API
    /// keys and internal service URLs, none of which a tunnel binary has any use for, and
    /// a child that crashes can dump its environment into a log.
    pub env: Vec<(String, String)>,
    /// Secrets to scrub out of captured output.
    pub secrets: &'a [&'a Secret],
    /// Cap on captured bytes per stream.
    pub max_capture: usize,
}

impl Run<'_> {
    /// Default capture cap: enough for the largest `tailscale status --json` seen from a
    /// busy tailnet, small enough that a runaway stream cannot matter.
    pub const DEFAULT_CAPTURE: usize = 512 * 1024;

    /// Run the command, capturing both streams under the deadline.
    pub async fn call(self) -> Result<Output, RunError> {
        let program_name = self.program.name().to_owned();
        let mut command = tokio::process::Command::new(self.program.path());
        command
            .args(&self.args)
            .env_clear()
            .envs(self.env.iter().map(|(key, value)| (key, value)))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Without this, a probe that times out leaves a child behind on every call.
            .kill_on_drop(true);

        let mut child = command.spawn().map_err(|source| RunError::Spawn {
            program: program_name.clone(),
            source,
        })?;

        let mut stdout_pipe = child.stdout.take();
        let mut stderr_pipe = child.stderr.take();
        let cap = self.max_capture;

        // Read both streams and wait for exit concurrently. Doing the wait first would
        // deadlock as soon as the child fills a pipe buffer.
        let collect = async {
            let mut out = Vec::new();
            let mut err = Vec::new();
            let (status, out_full, err_full) = tokio::join!(
                child.wait(),
                read_capped(&mut stdout_pipe, &mut out, cap),
                read_capped(&mut stderr_pipe, &mut err, cap),
            );
            (status, out, err, out_full || err_full)
        };

        // Boxed because the future holds both 8KB read buffers, and a future this large
        // sitting inline in a caller's own future is a stack cost every probe pays.
        match tokio::time::timeout(self.timeout, Box::pin(collect)).await {
            Ok((status, out, err, truncated)) => {
                let status = status.map_err(|source| RunError::Io {
                    program: program_name,
                    source,
                })?;
                Ok(Output {
                    code: status.code(),
                    stdout: scrub(&String::from_utf8_lossy(&out), self.secrets),
                    stderr: scrub(&String::from_utf8_lossy(&err), self.secrets),
                    truncated,
                })
            }
            Err(_elapsed) => {
                // `kill_on_drop` covers the child, but killing here makes the reaping
                // immediate rather than deferred to the drop of a future.
                let _killed = child.kill().await;
                Err(RunError::TimedOut {
                    program: program_name,
                    timeout: self.timeout,
                    tail: "(deadline reached before the child said anything)".to_owned(),
                })
            }
        }
    }
}

/// Read a pipe into `into`, stopping at `cap` bytes. Returns whether the cap was hit.
async fn read_capped<R>(pipe: &mut Option<R>, into: &mut Vec<u8>, cap: usize) -> bool
where
    R: tokio::io::AsyncRead + Unpin,
{
    let Some(reader) = pipe.as_mut() else {
        return false;
    };
    let mut buffer = [0_u8; 8192];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => return false,
            Ok(read) => {
                let remaining = cap.saturating_sub(into.len());
                if remaining == 0 {
                    return true;
                }
                let take = read.min(remaining);
                into.extend_from_slice(buffer.get(..take).unwrap_or_default());
                if take < read {
                    return true;
                }
            }
        }
    }
}
