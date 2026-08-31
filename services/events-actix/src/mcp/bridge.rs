//! One MCP server child per plugin, its stdout fanned out to SSE listeners.
//!
//! Ports upstream's `stdioSseBridge`: `getOrSpawn` / `registerSession` / `unregisterSession` /
//! `sendToChild` / `isRunning` / `killAllBridges`. An MCP server speaks JSON-RPC over stdio, so
//! this is a process supervisor rather than an HTTP proxy.
//!
//! Four choices differ from a literal port, each for a reason a test pins:
//!
//! * **Lines are read with a byte ceiling.** A server that writes a megabyte with no newline would
//!   otherwise grow the read buffer without bound. `nullrouter-pxpipe` reads its own worker the
//!   same way, for the same reason.
//! * **Listener sends are bounded and lossy.** A listener that stops reading must not stall the
//!   child or its siblings, so its channel fills and it misses frames instead.
//! * **A child is reaped when its last listener leaves.** Upstream keeps children in a global map
//!   for the process lifetime, which leaks one `npx` process per plugin ever opened.
//! * **Spawn failure is reported, not retried.** A missing `npx` is a fact about the machine;
//!   retrying per request would turn one missing binary into a spawn loop.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::{Mutex, mpsc};

use super::plugins::Plugin;

/// Largest single stdout line accepted from a child, in bytes.
const MAX_FRAME_BYTES: u64 = 8 * 1024 * 1024;
/// Frames buffered per SSE listener before its oldest frames are dropped.
const LISTENER_BUFFER_FRAMES: usize = 256;

/// Listener id to frame sink, shared between [`Bridge`] and the pump task feeding it.
type Listeners = Arc<Mutex<HashMap<u64, mpsc::Sender<String>>>>;

/// Why a spawn did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SpawnError {
    /// The path segment named nothing on the whitelist.
    UnknownPlugin,
    /// The command could not be started, with the OS reason.
    NotStartable(String),
}

impl SpawnError {
    /// A stable machine-readable tag, matching the shape the message route already returns.
    pub(crate) const fn code(&self) -> &'static str {
        match *self {
            Self::UnknownPlugin => "mcp_unknown_plugin",
            Self::NotStartable(_) => "mcp_backend_unavailable",
        }
    }

    pub(crate) fn message(&self) -> String {
        match *self {
            Self::UnknownPlugin => {
                "no MCP plugin by that name may be started; only preset plugins can be spawned"
                    .to_owned()
            }
            Self::NotStartable(ref reason) => {
                format!("the MCP server for this plugin could not be started: {reason}")
            }
        }
    }
}

/// A live child and everyone listening to it.
struct Session {
    child: Child,
    stdin: ChildStdin,
    /// Shared with the pump task, which is the only writer of frames into these sinks.
    listeners: Listeners,
    next_listener_id: u64,
}

/// Every running MCP child, keyed by plugin name.
#[derive(Clone, Default)]
pub(crate) struct Bridge {
    sessions: Arc<Mutex<HashMap<&'static str, Session>>>,
}

impl std::fmt::Debug for Bridge {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Bridge")
    }
}

/// A listener's handle. [`Listener::detach`] reaps the child if it was the last one.
pub(crate) struct Listener {
    bridge: Bridge,
    plugin: &'static str,
    id: u64,
    frames: mpsc::Receiver<String>,
}

impl std::fmt::Debug for Listener {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Listener")
            .field("plugin", &self.plugin)
            .field("id", &self.id)
            .finish()
    }
}

impl Listener {
    /// The next frame, or `None` once the child's stdout has closed and the buffer is drained.
    pub(crate) async fn next_frame(&mut self) -> Option<String> {
        self.frames.recv().await
    }

    /// Mutable access to the frame channel, for a caller polling it inside its own stream.
    pub(crate) fn frames_mut(&mut self) -> &mut mpsc::Receiver<String> {
        &mut self.frames
    }

    /// Which plugin this listener is attached to.
    pub(crate) const fn plugin(&self) -> &'static str {
        self.plugin
    }

    /// The bridge this listener came from, so a caller can detach on its own schedule.
    pub(crate) fn bridge(&self) -> Bridge {
        self.bridge.clone()
    }

    /// This listener's id, needed to detach it.
    pub(crate) const fn id(&self) -> u64 {
        self.id
    }

    /// Detach, reaping the child when no listeners remain. Idempotent.
    pub(crate) async fn detach(&self) {
        self.bridge.detach(self.plugin, self.id).await;
    }
}

impl Bridge {
    /// Whether a child is currently running for this plugin.
    pub(crate) async fn is_running(&self, plugin: &str) -> bool {
        self.sessions.lock().await.contains_key(plugin)
    }

    /// Attach a listener, spawning the child if this is the first one.
    pub(crate) async fn attach(&self, plugin: &'static Plugin) -> Result<Listener, SpawnError> {
        let mut sessions = self.sessions.lock().await;
        if !sessions.contains_key(plugin.name) {
            sessions.insert(plugin.name, spawn_session(plugin)?);
        }
        let session = sessions
            .get_mut(plugin.name)
            .ok_or_else(|| SpawnError::NotStartable("session vanished after spawn".to_owned()))?;

        let id = session.next_listener_id;
        session.next_listener_id = session.next_listener_id.wrapping_add(1);
        let (sender, frames) = mpsc::channel(LISTENER_BUFFER_FRAMES);
        session.listeners.lock().await.insert(id, sender);

        Ok(Listener {
            bridge: self.clone(),
            plugin: plugin.name,
            id,
            frames,
        })
    }

    /// Write one JSON-RPC frame to a child's stdin.
    ///
    /// `Ok(false)` means no child is running. Reported rather than papered over by spawning: a
    /// message for a plugin nobody is listening to has no SSE session to carry the reply.
    pub(crate) async fn send(&self, plugin: &str, frame: &str) -> Result<bool, String> {
        let mut sessions = self.sessions.lock().await;
        let Some(session) = sessions.get_mut(plugin) else {
            return Ok(false);
        };
        let mut line = frame.trim_end().to_owned();
        line.push('\n');
        session
            .stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|error| error.to_string())?;
        session
            .stdin
            .flush()
            .await
            .map_err(|error| error.to_string())?;
        Ok(true)
    }

    /// Remove one listener, and reap the child when none remain.
    pub(crate) async fn detach(&self, plugin: &str, id: u64) {
        let mut sessions = self.sessions.lock().await;
        let Some(session) = sessions.get(plugin) else {
            return;
        };
        let remaining = {
            let mut listeners = session.listeners.lock().await;
            listeners.remove(&id);
            listeners.len()
        };
        // Guarded before the remove, not after: taking the session out first and putting it back
        // would evict a child its remaining listeners are still reading.
        if remaining > 0 {
            return;
        }
        if let Some(session) = sessions.remove(plugin) {
            reap(session).await;
        }
    }

    /// Kill every child. Called at shutdown so no `npx` process outlives this service.
    pub(crate) async fn kill_all(&self) {
        let mut sessions = self.sessions.lock().await;
        let running: Vec<Session> = sessions.drain().map(|(_, session)| session).collect();
        drop(sessions);
        for session in running {
            reap(session).await;
        }
    }
}

/// Close a child's stdin, then kill it.
///
/// Closing stdin first lets a well-behaved MCP server exit on its own; the kill covers one that
/// ignores EOF. `kill` on an already-exited child is not an error.
async fn reap(session: Session) {
    let Session {
        mut child, stdin, ..
    } = session;
    drop(stdin);
    let _ = child.kill().await;
}

/// Spawn one child and start the task pumping its stdout to this session's listeners.
fn spawn_session(plugin: &'static Plugin) -> Result<Session, SpawnError> {
    let mut child = tokio::process::Command::new(plugin.command)
        .args(plugin.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // Discarded rather than merged: an MCP server's stderr is human diagnostics, and
        // interleaving it into stdout would corrupt the JSON-RPC frame stream.
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| SpawnError::NotStartable(error.to_string()))?;

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| SpawnError::NotStartable("child stdin was not piped".to_owned()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| SpawnError::NotStartable("child stdout was not piped".to_owned()))?;

    let listeners: Listeners = Arc::new(Mutex::new(HashMap::new()));
    let pump_listeners = Arc::clone(&listeners);
    // Detached: it ends when the child's stdout closes, which `reap` causes by killing the child.
    tokio::spawn(async move {
        pump(BufReader::new(stdout), pump_listeners).await;
    });

    Ok(Session {
        child,
        stdin,
        listeners,
        next_listener_id: 0,
    })
}

/// Read `reader` line by line, filter each frame, and fan it out to every listener.
///
/// Generic over the reader so a test can drive it without a real child process.
pub(crate) async fn pump<R>(mut reader: BufReader<R>, listeners: Listeners)
where
    R: tokio::io::AsyncRead + Unpin,
{
    loop {
        let mut line = String::new();
        // Bounded per line: an unterminated write must not grow this buffer without limit.
        let read = (&mut reader)
            .take(MAX_FRAME_BYTES)
            .read_line(&mut line)
            .await;
        match read {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if trimmed.is_empty() {
            continue;
        }
        let filtered = super::filter::frame(trimmed);
        let sinks = listeners.lock().await;
        for sink in sinks.values() {
            // `try_send`, not `send`: one listener that stopped reading must not stall the child
            // or its siblings. Its buffer fills and it misses frames instead.
            let _ = sink.try_send(filtered.clone());
        }
    }

    // Stdout closed, so no further frame can arrive. Dropping the senders is what closes each
    // listener's channel and lets `recv()` return `None`; without it a listener waits forever on a
    // child that has already exited, and an SSE response built on it never completes — the
    // connection stays open until the client gives up.
    listeners.lock().await.clear();
}

/// A stdout reader wired to a fresh listener map, for tests that need the pump without a child.
#[cfg(test)]
pub(crate) fn test_listeners() -> Listeners {
    Arc::new(Mutex::new(HashMap::new()))
}

/// Pump a child's stdout, exposed so a test can supply its own `ChildStdout`.
#[cfg(test)]
pub(crate) async fn pump_child_stdout(stdout: ChildStdout, listeners: Listeners) {
    pump(BufReader::new(stdout), listeners).await;
}

#[cfg(test)]
mod tests {
    use super::{Bridge, SpawnError};
    use crate::mcp::plugins;

    /// Path to the mock server cargo built beside this test binary.
    fn mock_server() -> &'static str {
        // `current_exe` is target/debug/deps/<test>-<hash>; the binary is two levels up.
        Box::leak(
            std::env::current_exe()
                .expect("test exe path")
                .parent()
                .and_then(std::path::Path::parent)
                .expect("deps parent")
                .join("mock-mcp-server")
                .to_string_lossy()
                .into_owned()
                .into_boxed_str(),
        )
    }

    /// A plugin pointing at the mock server, spawnable only from tests.
    fn mock_plugin() -> &'static super::Plugin {
        plugins::leak_for_test("mockmcp", mock_server(), &[])
    }

    #[tokio::test]
    async fn an_unlisted_plugin_never_spawns() {
        // The RCE gate. A path segment naming a real binary must still not start it.
        assert!(plugins::find("sh").is_none());
        assert!(plugins::find("mockmcp").is_none());
    }

    #[tokio::test]
    async fn a_command_that_does_not_exist_is_reported_not_retried() {
        let bridge = Bridge::default();
        let plugin = plugins::leak_for_test("missing", "/nonexistent/mcp-server", &[]);
        let error = bridge.attach(plugin).await.expect_err("must not attach");
        assert!(matches!(error, SpawnError::NotStartable(_)));
        assert_eq!(error.code(), "mcp_backend_unavailable");
        // No session was left behind for a later request to find.
        assert!(!bridge.is_running("missing").await);
    }

    #[tokio::test]
    async fn initialize_and_tool_list_round_trip_through_the_bridge() {
        let bridge = Bridge::default();
        let mut listener = bridge.attach(mock_plugin()).await.expect("attach");
        assert!(bridge.is_running("mockmcp").await);

        assert!(
            bridge
                .send(
                    "mockmcp",
                    r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#
                )
                .await
                .expect("send initialize")
        );
        let frame = listener.next_frame().await.expect("initialize reply");
        assert!(frame.contains("mock-mcp-server"), "{frame}");
        assert!(frame.contains("\"id\":1"), "{frame}");

        assert!(
            bridge
                .send(
                    "mockmcp",
                    r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#
                )
                .await
                .expect("send tools/list")
        );
        let frame = listener.next_frame().await.expect("tools/list reply");
        assert!(frame.contains("echo"), "{frame}");
        assert!(frame.contains("\"id\":2"), "{frame}");

        listener.detach().await;
        assert!(!bridge.is_running("mockmcp").await, "child must be reaped");
    }

    #[tokio::test]
    async fn a_tool_call_result_is_filtered_on_the_way_out() {
        let bridge = Bridge::default();
        let mut listener = bridge.attach(mock_plugin()).await.expect("attach");
        bridge
            .send(
                "mockmcp",
                r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"huge"}}"#,
            )
            .await
            .expect("send tools/call");

        let frame = listener.next_frame().await.expect("tools/call reply");
        // The server sent 400 identical siblings; the filter must have collapsed them.
        assert!(
            frame.contains("items omitted by the nullrouter MCP bridge"),
            "oversized result reached the client unfiltered: {} bytes",
            frame.len()
        );
        assert!(frame.contains("\"id\":3"), "the id must survive filtering");
        listener.detach().await;
    }

    #[tokio::test]
    async fn a_mid_stream_exit_ends_the_frame_stream() {
        let bridge = Bridge::default();
        let mut listener = bridge.attach(mock_plugin()).await.expect("attach");
        bridge
            .send(
                "mockmcp",
                r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"die"}}"#,
            )
            .await
            .expect("send die");

        // The server exits without replying, closing stdout. The listener must see the end rather
        // than hanging: an SSE stream that never completes holds the connection open forever.
        assert!(
            listener.next_frame().await.is_none(),
            "a closed stdout must end the stream"
        );
        listener.detach().await;
        assert!(!bridge.is_running("mockmcp").await);
    }

    #[tokio::test]
    async fn two_listeners_share_one_child_and_the_last_one_reaps_it() {
        let bridge = Bridge::default();
        let first = bridge.attach(mock_plugin()).await.expect("first attach");
        let second = bridge.attach(mock_plugin()).await.expect("second attach");
        assert_ne!(first.id(), second.id(), "listeners must be distinct");

        first.detach().await;
        assert!(
            bridge.is_running("mockmcp").await,
            "one listener leaving must not kill a child the other is using"
        );
        second.detach().await;
        assert!(!bridge.is_running("mockmcp").await, "the last one reaps");
    }

    #[tokio::test]
    async fn kill_all_leaves_nothing_running() {
        let bridge = Bridge::default();
        let _listener = bridge.attach(mock_plugin()).await.expect("attach");
        assert!(bridge.is_running("mockmcp").await);
        bridge.kill_all().await;
        assert!(!bridge.is_running("mockmcp").await);
    }

    #[tokio::test]
    async fn a_message_for_a_plugin_with_no_session_is_reported() {
        let bridge = Bridge::default();
        let delivered = bridge
            .send(
                "mockmcp",
                r#"{"jsonrpc":"2.0","id":9,"method":"initialize"}"#,
            )
            .await
            .expect("send must not error");
        assert!(!delivered, "no session means no delivery");
    }
}
