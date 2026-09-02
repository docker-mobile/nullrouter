//! The supervisor thread: one child, one runtime, one loop.
//!
//! The loop is a state machine rather than a sequence of awaits because every wait has to
//! stay interruptible. A start that is 80 seconds into a 90 second readiness deadline must
//! still answer a stop, and a child sitting in restart backoff must be cancellable without
//! waiting the backoff out. Each state selects only over the events that can occur in it,
//! turns one into an [`Event`], and then replaces the slot once the borrows have ended.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::process::Child;
use tokio::sync::{mpsc, oneshot};
use tokio::time::sleep_until;

use super::{ChildSpec, Command, ReadyRule, Shared, StartError, State};
use crate::secret::scrub;
use crate::signal::{StopOutcome, request_termination};

/// A spawned child and the spec that produced it.
struct Running {
    child: Child,
    /// Captured at spawn: `Child::id` returns `None` once the child is reaped.
    pid: Option<u32>,
    spec: Arc<ChildSpec>,
    /// Resolves once the watcher has drained both pipes to EOF.
    ///
    /// `child.wait()` and the watcher are separate tasks, so an exit can be observed while
    /// the child's last lines are still queued. Without waiting for this, the error for a
    /// child that explained itself and then exited reads "produced no output".
    drained: Option<oneshot::Receiver<()>>,
}

/// The caller waiting on a start, if the start came from one.
type Pending = Option<oneshot::Sender<Result<Option<String>, StartError>>>;

/// Where the supervisor is in its lifecycle.
enum Slot {
    /// Nothing running.
    Empty,
    /// Spawned, waiting for the readiness rule or the deadline.
    Starting {
        running: Running,
        deadline: Instant,
        /// When mere survival counts as ready, under [`ReadyRule::SurvivesOr`].
        ///
        /// Always earlier than `deadline`: it is a floor on how long the child must last, not a
        /// second way to give up.
        survives_at: Option<Instant>,
        /// `None` once the watcher has dropped its sender without signalling, which means
        /// readiness can no longer arrive. Kept distinct from a signal so that a closed
        /// channel cannot be mistaken for a met rule — the bug this shape prevents.
        ready: Option<oneshot::Receiver<Option<String>>>,
        reply: Pending,
        /// Consecutive restart attempt this start belongs to; `0` for a manual start.
        attempt: u32,
    },
    /// Up and past its readiness rule.
    Live(Running),
    /// Waiting out a backoff before restarting.
    Backoff {
        spec: Arc<ChildSpec>,
        at: Instant,
        attempt: u32,
    },
}

/// What the loop observed.
enum Event {
    /// An instruction, or `None` when every handle is gone.
    Command(Option<Command>),
    /// The child exited.
    Exited(Option<std::process::ExitStatus>),
    /// The readiness rule was met.
    Ready(Option<String>),
    /// The watcher ended without signalling readiness, so it never can.
    ///
    /// This is not a failure by itself: the child's own exit, arriving on the other branch,
    /// is what carries the reason. Treating it as readiness would report a start that
    /// failed as one that succeeded.
    ReadyImpossible,
    /// The startup deadline passed.
    Deadline,
    /// A backoff elapsed.
    RestartDue,
}

/// Start the supervisor thread.
///
/// A dedicated OS thread with its own `current_thread` runtime, so the child is created,
/// waited on and reaped by one runtime regardless of which actix worker asked.
pub(super) fn launch(
    program: &'static str,
    commands: mpsc::Receiver<Command>,
    shared: Arc<Mutex<Shared>>,
) {
    let build = std::thread::Builder::new()
        .name(format!("procctl-{program}"))
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    tracing::error!(
                        program,
                        %error,
                        "supervisor runtime could not be created; this program cannot be managed"
                    );
                    return;
                }
            };
            runtime.block_on(run(program, commands, shared));
        });

    if let Err(error) = build {
        tracing::error!(
            program,
            %error,
            "supervisor thread could not be started; this program cannot be managed"
        );
    }
}

/// The loop.
async fn run(
    program: &'static str,
    mut commands: mpsc::Receiver<Command>,
    shared: Arc<Mutex<Shared>>,
) {
    let mut slot = Slot::Empty;

    loop {
        let event = next_event(&mut slot, &mut commands).await;
        let current = std::mem::replace(&mut slot, Slot::Empty);

        slot = match event {
            Event::Command(None) => {
                // Every handle was dropped. Take the child down rather than leave an
                // orphaned tunnel behind, then leave the loop.
                let _outcome = teardown(program, current, "the supervisor is shutting down").await;
                mark_stopped(&shared);
                return;
            }
            Event::Command(Some(Command::Start { spec, reply })) => {
                let _outcome = teardown(program, current, "superseded by a new start").await;
                // A manual start clears the restart history: the operator is asking for a
                // fresh attempt, not continuing a failed one.
                set(&shared, |state| {
                    state.restarts = 0;
                    state.last_error = None;
                    state.logs.clear();
                });
                begin(program, Arc::new(*spec), Some(reply), 0, &shared)
            }
            Event::Command(Some(Command::Stop { reply })) => {
                let outcome = teardown(program, current, "stopped on request").await;
                mark_stopped(&shared);
                let _sent = reply.send(outcome);
                Slot::Empty
            }
            Event::Ready(value) => on_ready(program, current, value, &shared),
            Event::ReadyImpossible => {
                // Retire the branch and keep waiting: the child's exit or the deadline is
                // what ends this start, and both are still selected on.
                match current {
                    Slot::Starting {
                        running,
                        deadline,
                        survives_at,
                        reply,
                        attempt,
                        ready: _closed,
                    } => Slot::Starting {
                        running,
                        deadline,
                        survives_at,
                        ready: None,
                        reply,
                        attempt,
                    },
                    other => other,
                }
            }
            Event::Deadline => on_deadline(program, current, &shared).await,
            Event::Exited(status) => on_exit(program, current, status, &shared).await,
            Event::RestartDue => {
                let Slot::Backoff { spec, attempt, .. } = current else {
                    continue;
                };
                tracing::info!(program, attempt, "restarting supervised child");
                begin(program, spec, None, attempt, &shared)
            }
        };
    }
}

/// Wait for whichever event the current state can produce.
///
/// Each arm borrows a disjoint part of the slot and yields a plain value, so the caller can
/// replace the slot as soon as this returns. Every future here is cancel-safe:
/// `Child::wait` is documented as such, a `oneshot::Receiver` resumes where it left off,
/// and `sleep_until` holds an absolute instant rather than a remaining duration — so
/// losing a `select!` race never shortens a deadline or drops an exit.
async fn next_event(slot: &mut Slot, commands: &mut mpsc::Receiver<Command>) -> Event {
    match slot {
        Slot::Empty => Event::Command(commands.recv().await),
        Slot::Starting {
            running,
            deadline,
            survives_at,
            ready,
            ..
        } => {
            tokio::select! {
                command = commands.recv() => Event::Command(command),
                status = running.child.wait() => Event::Exited(status.ok()),
                event = readiness(ready) => event,
                () = survival(survives_at.as_ref()) => Event::Ready(None),
                () = sleep_until((*deadline).into()) => Event::Deadline,
            }
        }
        Slot::Live(running) => {
            tokio::select! {
                command = commands.recv() => Event::Command(command),
                status = running.child.wait() => Event::Exited(status.ok()),
            }
        }
        Slot::Backoff { at, .. } => {
            tokio::select! {
                command = commands.recv() => Event::Command(command),
                () = sleep_until((*at).into()) => Event::RestartDue,
            }
        }
    }
}

/// Await the moment survival alone counts as ready, or never when no rule wants that.
async fn survival(at: Option<&Instant>) {
    match at {
        None => std::future::pending().await,
        Some(instant) => sleep_until((*instant).into()).await,
    }
}

/// Await the readiness channel, distinguishing a signal from a closed channel.
///
/// A `oneshot::Receiver` resolves `Err` when its sender is dropped, and the watcher drops
/// the sender as soon as the child's streams close — which is exactly what happens when a
/// child exits without ever matching its rule. Collapsing that `Err` into "ready with no
/// value" would report every such failed start as a success.
///
/// Once the channel is gone this parks forever, leaving the exit and deadline branches to
/// decide the outcome. Without the park, re-awaiting an already-resolved receiver on the
/// next iteration would spin the loop until the deadline.
async fn readiness(ready: &mut Option<oneshot::Receiver<Option<String>>>) -> Event {
    match ready.as_mut() {
        None => std::future::pending().await,
        Some(receiver) => match receiver.await {
            Ok(value) => Event::Ready(value),
            Err(_dropped) => Event::ReadyImpossible,
        },
    }
}

/// Stop whatever the slot holds, answering a caller that was waiting on a start.
///
/// Taking the slot by value is what removes the need for any placeholder child: the state
/// is destructured, the child is stopped, and the caller builds a fresh state afterwards.
async fn teardown(program: &'static str, slot: Slot, reason: &str) -> StopOutcome {
    match slot {
        Slot::Empty | Slot::Backoff { .. } => StopOutcome::NotRunning,
        Slot::Live(mut running) => stop_child(&mut running).await,
        Slot::Starting {
            mut running, reply, ..
        } => {
            let outcome = stop_child(&mut running).await;
            if let Some(reply) = reply {
                let _sent = reply.send(Err(StartError::NotReady {
                    program: program.to_owned(),
                    timeout: Duration::ZERO,
                    tail: reason.to_owned(),
                }));
            }
            outcome
        }
    }
}

/// The readiness rule was met: promote the child to live and answer the caller.
fn on_ready(
    program: &'static str,
    slot: Slot,
    value: Option<String>,
    shared: &Arc<Mutex<Shared>>,
) -> Slot {
    let Slot::Starting {
        running,
        reply,
        attempt,
        ..
    } = slot
    else {
        return slot;
    };

    set(shared, |state| {
        state.state = State::Running;
        state.ready_value.clone_from(&value);
        state.started_at = Some(Instant::now());
        state.restarts = attempt;
    });
    if let Some(reply) = reply {
        let _sent = reply.send(Ok(value));
    }
    tracing::info!(program, attempt, "supervised child is ready");
    Slot::Live(running)
}

/// The startup deadline passed: tear the child down and report it.
///
/// A child that is up but silent is still torn down. "Probably fine" is the state this
/// crate exists to not have: a tunnel that never logged its connections may equally be one
/// that never established them, and leaving it running would report success for a tunnel
/// that carries no traffic.
async fn on_deadline(program: &'static str, slot: Slot, shared: &Arc<Mutex<Shared>>) -> Slot {
    let Slot::Starting {
        mut running, reply, ..
    } = slot
    else {
        return slot;
    };

    let timeout = running.spec.startup_timeout;
    let _outcome = stop_child(&mut running).await;
    await_drain(&mut running).await;
    let error = StartError::NotReady {
        program: program.to_owned(),
        timeout,
        tail: tail_of(shared),
    };
    record_failure(shared, &error);
    if let Some(reply) = reply {
        let _sent = reply.send(Err(error));
    } else {
        tracing::warn!(program, "restarted child did not become ready in time");
    }
    Slot::Empty
}

/// The child exited by itself: answer a pending start, or consider a restart.
async fn on_exit(
    program: &'static str,
    slot: Slot,
    status: Option<std::process::ExitStatus>,
    shared: &Arc<Mutex<Shared>>,
) -> Slot {
    let rendered = render_status(status);

    match slot {
        Slot::Starting {
            mut running,
            reply,
            attempt,
            ..
        } => {
            // The child's last lines may still be in flight; a failure message without them
            // is useless to whoever has to act on it.
            await_drain(&mut running).await;
            // A child that exits during startup can still be a success: `tailscale funnel
            // --bg` does its work and returns. Only a non-zero exit is a failure.
            let succeeded = status.is_some_and(|status| status.success());
            if succeeded && matches!(running.spec.ready, ReadyRule::CompletesSuccessfully) {
                set(shared, |state| {
                    state.state = State::Stopped;
                    state.pid = None;
                    state.started_at = None;
                });
                if let Some(reply) = reply {
                    let _sent = reply.send(Ok(None));
                }
                tracing::info!(program, "supervised child completed its work and exited");
                return Slot::Empty;
            }

            let error = StartError::ExitedEarly {
                program: program.to_owned(),
                status: rendered,
                tail: tail_of(shared),
            };
            record_failure(shared, &error);
            match reply {
                Some(reply) => {
                    let _sent = reply.send(Err(error));
                    // The caller has the failure and will decide; do not also restart
                    // underneath them.
                    Slot::Empty
                }
                None => {
                    // A restarted child that died again: keep applying the policy.
                    schedule_restart(program, running.spec, attempt, shared)
                }
            }
        }
        Slot::Live(running) => {
            tracing::warn!(program, status = %rendered, "supervised child exited unexpectedly");
            set(shared, |state| {
                state.pid = None;
                state.ready_value = None;
                state.started_at = None;
                state.last_error = Some(format!("exited unexpectedly with {rendered}"));
            });
            // Uptime long enough to count as healthy resets the attempt counter, so a
            // daemon that has been up for hours gets the full allowance again.
            let healthy = healthy_uptime(shared, running.spec.restart.reset_after);
            let attempt = if healthy { 0 } else { restarts_of(shared) };
            schedule_restart(program, running.spec, attempt, shared)
        }
        other => other,
    }
}

/// Apply the restart policy: either queue a backoff or give up.
fn schedule_restart(
    program: &'static str,
    spec: Arc<ChildSpec>,
    attempt: u32,
    shared: &Arc<Mutex<Shared>>,
) -> Slot {
    let next = attempt.saturating_add(1);
    let policy = spec.restart;

    if next > policy.max_attempts {
        tracing::error!(
            program,
            attempts = policy.max_attempts,
            "supervised child exited too many times; not restarting again"
        );
        set(shared, |state| {
            state.state = State::Failed;
            state.pid = None;
            state.ready_value = None;
            state.started_at = None;
            state.restarts = attempt;
            if state.last_error.is_none() {
                state.last_error = Some("exited repeatedly and was not restarted".to_owned());
            }
        });
        return Slot::Empty;
    }

    let delay = policy.delay_for(next);
    tracing::info!(
        program,
        attempt = next,
        delay_ms = delay.as_millis(),
        "scheduling a restart of the supervised child"
    );
    set(shared, |state| {
        state.state = State::Backoff;
        state.pid = None;
        state.ready_value = None;
        state.started_at = None;
        state.restarts = attempt;
    });
    Slot::Backoff {
        spec,
        at: Instant::now() + delay,
        attempt: next,
    }
}

/// Spawn the child and start watching its output.
fn begin(
    program: &'static str,
    spec: Arc<ChildSpec>,
    reply: Pending,
    attempt: u32,
    shared: &Arc<Mutex<Shared>>,
) -> Slot {
    let mut command = tokio::process::Command::new(spec.program.path());
    command
        .args(&spec.args)
        // Nothing is inherited: the service's environment holds provider API keys and
        // internal service URLs, and a child that crashes can dump its environment.
        .env_clear()
        .envs(spec.env.iter().map(|(key, value)| (key, value)))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // The backstop for every path that does not run `stop_child`: a panic, or the
        // whole slot being dropped.
        .kill_on_drop(true);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(source) => {
            let error = StartError::Spawn {
                program: program.to_owned(),
                source,
            };
            record_failure(shared, &error);
            if let Some(reply) = reply {
                let _sent = reply.send(Err(error));
            }
            return Slot::Empty;
        }
    };

    let pid = child.id();
    let (ready_sender, ready) = oneshot::channel();
    let drained = watch_output(&mut child, &spec, ready_sender, Arc::clone(shared));

    set(shared, |state| {
        state.state = State::Starting;
        state.pid = pid;
        state.ready_value = None;
        state.started_at = None;
    });
    tracing::info!(program, ?pid, attempt, "supervised child spawned");

    let survives_at = match &spec.ready {
        // Clamped below the startup deadline, so a grace longer than the timeout cannot make
        // survival unreachable — the rule would then be strictly harsher than the one it mirrors.
        ReadyRule::SurvivesOr { grace, .. } => {
            // Clamped strictly *below* the startup timeout, not merely to it. A grace equal to the
            // deadline puts two timers on the same instant, and `select!` between two ready branches
            // picks arbitrarily — so a clamped grace would fail or succeed at random. Survival is a
            // floor on how long the child must last, so when the two would coincide the floor is what
            // should fire.
            let margin = Duration::from_millis(1);
            let clamped = (*grace).min(spec.startup_timeout.saturating_sub(margin));
            Some(Instant::now() + clamped)
        }
        _other => None,
    };

    Slot::Starting {
        deadline: Instant::now() + spec.startup_timeout,
        survives_at,
        running: Running {
            child,
            pid,
            spec,
            drained: Some(drained),
        },
        ready: Some(ready),
        reply,
        attempt,
    }
}

/// Drain both output streams into the log ring, deciding readiness as lines arrive.
///
/// stdout and stderr are read by separate tasks feeding one channel, because both daemons
/// split their output across the two unpredictably: `cloudflared` prints its quick-tunnel
/// URL to stderr, and `tailscale` prints its auth URL to stdout on one platform and stderr
/// on another. A single consumer then sees one ordered stream.
fn watch_output(
    child: &mut Child,
    spec: &Arc<ChildSpec>,
    ready: oneshot::Sender<Option<String>>,
    shared: Arc<Mutex<Shared>>,
) -> oneshot::Receiver<()> {
    /// Bound on lines queued between the readers and the consumer.
    const LINE_QUEUE: usize = 256;

    let (lines, mut incoming) = mpsc::channel::<String>(LINE_QUEUE);
    let (finished, drained) = oneshot::channel();

    if let Some(stdout) = child.stdout.take() {
        forward(BufReader::new(stdout), lines.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        forward(BufReader::new(stderr), lines.clone());
    }
    drop(lines);

    let spec = Arc::clone(spec);
    // A spawn is enough for this rule, so answer before reading anything. The child's
    // real health is then the caller's out-of-band check.
    let mut ready = Some(ready);
    if matches!(spec.ready, ReadyRule::Spawned)
        && let Some(sender) = ready.take()
    {
        let _sent = sender.send(None);
    }

    tokio::spawn(async move {
        let secrets: Vec<&crate::secret::Secret> = spec.secrets.iter().collect();
        let mut seen = 0_usize;

        while let Some(line) = incoming.recv().await {
            let clean = scrub(&line, &secrets);
            set(&shared, |state| state.logs.push(&clean));

            // Draining continues after readiness: the ring is the log the panel shows, and
            // an unread pipe would eventually block the child.
            if ready.is_none() {
                continue;
            }

            match &spec.ready {
                // Decided elsewhere: at spawn, or at a zero exit.
                ReadyRule::Spawned | ReadyRule::CompletesSuccessfully => {}
                ReadyRule::SurvivesOr { needle, .. } => {
                    // The early exit. Survival is the engine's timer, not this task's.
                    if clean.contains(needle)
                        && let Some(sender) = ready.take()
                    {
                        let _sent = sender.send(None);
                    }
                }
                ReadyRule::Occurrences { needle, times } => {
                    // Counted per line rather than per chunk: upstream counts matches in
                    // whatever chunk the pipe happened to deliver, which double-counts a
                    // line split across two reads and undercounts two lines in one read.
                    seen = seen.saturating_add(clean.matches(needle).count());
                    if seen >= *times
                        && let Some(sender) = ready.take()
                    {
                        let _sent = sender.send(None);
                    }
                }
                ReadyRule::Extract(extract) => {
                    if let Some(value) = extract(&clean)
                        && let Some(sender) = ready.take()
                    {
                        let _sent = sender.send(Some(value));
                    }
                }
            }
        }

        // Both pipes are at EOF and the queue is empty: whatever the child said is now in
        // the ring, and a caller building an error message can read a complete tail.
        let _signalled = finished.send(());
    });

    drained
}

/// Read one stream line by line into the shared channel.
fn forward<R>(reader: BufReader<R>, into: mpsc::Sender<String>)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = reader.lines();
        // `next_line` returning `Err` means the stream is unusable, not that the next read
        // might succeed, so both `Err` and `Ok(None)` end the loop.
        while let Ok(Some(line)) = lines.next_line().await {
            if into.send(line).await.is_err() {
                // The consumer is gone, which means the child is being torn down.
                return;
            }
        }
    });
}

/// Wait for the watcher to finish draining, briefly.
///
/// Bounded because the wait is only ever for lines already in a queue whose writers have
/// closed. If the watcher is somehow gone the tail is simply whatever arrived, which is
/// still better than blocking the supervisor.
async fn await_drain(running: &mut Running) {
    /// Long enough for a queue of already-read lines, short enough to be invisible.
    const DRAIN_GRACE: Duration = Duration::from_millis(500);

    if let Some(drained) = running.drained.take() {
        let _elapsed = tokio::time::timeout(DRAIN_GRACE, drained).await;
    }
}

/// Ask the child to exit, then insist.
async fn stop_child(running: &mut Running) -> StopOutcome {
    if matches!(running.child.try_wait(), Ok(Some(_status))) {
        return StopOutcome::NotRunning;
    }

    let Some(pid) = running.pid else {
        let _killed = running.child.kill().await;
        return StopOutcome::NotRunning;
    };

    if !request_termination(pid) {
        // The signal could not be delivered, which almost always means the child exited
        // between the check above and here. Reap it either way.
        let _killed = running.child.kill().await;
        return StopOutcome::NotRunning;
    }

    match tokio::time::timeout(running.spec.graceful_timeout, running.child.wait()).await {
        Ok(_status) => StopOutcome::Graceful,
        Err(_elapsed) => {
            let _killed = running.child.kill().await;
            StopOutcome::Forced
        }
    }
}

/// Render an exit status for a message.
fn render_status(status: Option<std::process::ExitStatus>) -> String {
    // A `None` code on unix means a signal ended the child, which is the normal shape
    // after this crate's own SIGTERM.
    status.map_or_else(
        || "an unreadable status".to_owned(),
        |status| {
            status
                .code()
                .map_or_else(|| "a signal".to_owned(), |code| format!("exit code {code}"))
        },
    )
}

/// Mutate the shared view.
fn set<F>(shared: &Arc<Mutex<Shared>>, apply: F)
where
    F: FnOnce(&mut Shared),
{
    match shared.lock() {
        Ok(mut state) => apply(&mut state),
        // Poisoning means a previous writer panicked. The data is structurally fine, and
        // refusing to update it would freeze the status a caller sees.
        Err(poisoned) => apply(&mut poisoned.into_inner()),
    }
}

/// Read the shared view.
fn get<T, F>(shared: &Arc<Mutex<Shared>>, read: F) -> T
where
    F: FnOnce(&Shared) -> T,
{
    match shared.lock() {
        Ok(state) => read(&state),
        Err(poisoned) => read(&poisoned.into_inner()),
    }
}

/// The retained log tail, for an error message.
fn tail_of(shared: &Arc<Mutex<Shared>>) -> String {
    let tail = get(shared, |state| state.logs.tail());
    if tail.trim().is_empty() {
        "(the child produced no output)".to_owned()
    } else {
        tail
    }
}

/// Record a failed attempt.
fn record_failure(shared: &Arc<Mutex<Shared>>, error: &StartError) {
    let message = error.to_string();
    set(shared, |state| {
        state.state = State::Failed;
        state.pid = None;
        state.ready_value = None;
        state.started_at = None;
        state.last_error = Some(message);
    });
}

/// Note that nothing is running and nothing is wanted.
fn mark_stopped(shared: &Arc<Mutex<Shared>>) {
    set(shared, |state| {
        state.state = State::Stopped;
        state.pid = None;
        state.ready_value = None;
        state.started_at = None;
    });
}

/// Whether the child that just died had been up long enough to count as healthy.
fn healthy_uptime(shared: &Arc<Mutex<Shared>>, reset_after: Duration) -> bool {
    get(shared, |state| {
        state
            .started_at
            .is_some_and(|since| since.elapsed() >= reset_after)
    })
}

/// The current restart count.
fn restarts_of(shared: &Arc<Mutex<Shared>>) -> u32 {
    get(shared, |state| state.restarts)
}
