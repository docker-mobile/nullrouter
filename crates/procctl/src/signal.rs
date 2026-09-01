//! Asking a child to exit, before insisting.
//!
//! `tokio::process::Child::kill` sends `SIGKILL`. For these two daemons that is the wrong
//! first move: `cloudflared` uses its shutdown path to unregister its edge connections, and
//! a `SIGKILL`ed tunnel leaves Cloudflare routing to a connection that is already gone
//! until it times out. `tailscaled` writes its state file on the way out.
//!
//! So the sequence is `SIGTERM`, a bounded wait, then `SIGKILL`. Upstream instead runs
//! `pkill -f "cloudflared.*:20128"` and `pkill -9 -f "tailscaled.*sock"`, which is a
//! pattern match against every process on the machine: it kills daemons this application
//! never started, including another copy of the router, and a user process whose command
//! line merely mentions the port. Nothing here matches on command lines. The only process
//! ever signalled is the child this crate spawned, addressed by the pid the kernel gave us.

/// Outcome of asking a child to stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopOutcome {
    /// It was not running.
    NotRunning,
    /// It exited after `SIGTERM`.
    Graceful,
    /// It ignored `SIGTERM` and was killed.
    Forced,
}

/// Send `SIGTERM` to one pid.
///
/// Returns whether the signal was delivered. A `false` here is normal: the child may have
/// exited between the check and the signal.
#[cfg(unix)]
pub(crate) fn request_termination(pid: u32) -> bool {
    // A pid that does not fit in `pid_t` cannot be one of our children, and passing a
    // negative value to `kill` addresses a whole process group — which is exactly the
    // over-broad kill this module exists to avoid.
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // SAFETY: `kill` takes two integers and no pointers, so there is nothing to
    // invalidate. `pid` is this process's own direct child, checked positive just above,
    // so the call cannot address a process group. The child has not been reaped yet —
    // the caller holds the `Child` — so the pid cannot yet have been recycled onto an
    // unrelated process.
    let result = unsafe { libc::kill(pid, libc::SIGTERM) };
    result == 0
}

/// Send `SIGTERM` to one pid.
///
/// Windows has no `SIGTERM`; the caller's forced kill is the only stop available, so this
/// reports "not delivered" and lets the escalation happen immediately.
#[cfg(not(unix))]
pub(crate) fn request_termination(_pid: u32) -> bool {
    false
}

#[cfg(all(test, unix))]
mod tests {
    use super::request_termination;

    #[test]
    fn a_pid_that_cannot_be_a_child_is_refused() {
        // 0 addresses this process's whole process group, and a negative value addresses
        // an arbitrary group. Both would be catastrophic and neither can be a child.
        assert!(!request_termination(0));
        // Above `pid_t`: cannot be a real pid, must not be truncated into one.
        assert!(!request_termination(u32::MAX));
        assert!(!request_termination(2_147_483_648));
    }

    #[test]
    fn a_live_child_receives_the_signal_and_a_reaped_one_does_not() {
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "sleep 30"])
            .spawn()
            .unwrap_or_else(|error| panic!("spawn a child to signal: {error}"));

        assert!(
            request_termination(child.id()),
            "SIGTERM to a live child must be delivered"
        );

        let status = child
            .wait()
            .unwrap_or_else(|error| panic!("wait for the signalled child: {error}"));
        assert!(!status.success(), "a SIGTERMed shell does not exit zero");
    }
}
