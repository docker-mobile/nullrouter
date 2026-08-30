//! The Node bridge: how Rust reaches `transformAnthropicMessages`.
//!
//! Ports `inspire/src/lib/pxpipe/loader.js` and the `transform` half of
//! `inspire/src/lib/pxpipe/service.js`.
//!
//! Upstream is itself JavaScript, so it `import()`s the installed package into the
//! server process and keeps the module in a cache; its start/stop/restart routes
//! govern that cache. There is no equivalent here — the compression is a JavaScript
//! library, and reimplementing PNG-packed context rendering in Rust would be a
//! different program with different output, not a port. So this keeps upstream's
//! shape and moves the module one process out: a long-lived `node` worker holding
//! the imported module, spoken to over a line-delimited pipe.
//!
//! A worker per router, not per request, for two reasons. Node's start-up plus the
//! package's own import cost tens to hundreds of milliseconds, which would be paid
//! on every large request and eat the savings. And a transform is uninterruptible
//! CPU work: upstream races it against a timer and abandons the result, leaving the
//! work running inside its own process, whereas a separate process can be killed
//! outright — the timeout here actually stops the work.
//!
//! Every failure is a bypass, never an error to the client. See [`TransformOutcome`].

use std::collections::VecDeque;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use crate::install::{Paths, find_node, install_info};

/// Refuse a reply larger than this rather than growing a buffer without bound.
///
/// A transformed body is legitimately large — base64 PNGs — but bounded by the
/// request that produced it. This is generous enough that no honest reply reaches it
/// and small enough that a broken worker cannot exhaust memory.
const MAX_FRAME_BYTES: u64 = 64 * 1024 * 1024;

/// Lines of worker stderr kept for diagnostics.
const MAX_STDERR_LINES: usize = 50;

/// How long the worker gets to report that the module imported.
const READY_TIMEOUT_MS: u64 = 30_000;

/// The worker program.
///
/// Passed with `node --input-type=module -e`, so there is no file to write, no
/// directory that has to be writable, and no stale copy on disk after an upgrade.
///
/// It answers every request with exactly one frame — including for a thrown error,
/// which is what keeps a transform failure a bypass rather than a hang.
const WORKER_SOURCE: &str = r#"
import { pathToFileURL } from "node:url";

// From the environment rather than argv: under `-e` there is no script name, so the
// first user argument lands at argv[1] rather than argv[2], and depending on that
// offset is a trap for anyone who later adds a node flag.
const entry = process.env.PXPIPE_ENTRY;
const send = (frame) => process.stdout.write(JSON.stringify(frame) + "\n");

let transform = null;
try {
  const mod = await import(pathToFileURL(entry).href);
  if (typeof mod.transformAnthropicMessages !== "function") {
    throw new Error("installed pxpipe package does not export transformAnthropicMessages");
  }
  transform = mod.transformAnthropicMessages;
  send({ type: "ready", node: process.versions.node });
} catch (error) {
  send({ type: "ready", node: process.versions.node, error: error?.message || String(error) });
  process.exit(1);
}

const encoder = new TextEncoder();
const decoder = new TextDecoder();

async function handle(request) {
  const reply = { type: "result", id: request.id };
  try {
    const result = await transform({
      body: encoder.encode(request.body),
      model: request.model,
      options: { minCompressChars: request.minChars },
    });
    if (!result || typeof result.applied !== "boolean") {
      throw new Error("transform returned an unexpected shape");
    }
    reply.applied = result.applied === true;
    reply.reason = result.reason || null;
    reply.detail = result.detail || null;
    // Only the fields Rust reads. The package's own `info` embeds the rendered PNG
    // bytes, every image's dimensions, and the imaged source text: measured at 393 KB
    // of JSON beside a 38 KB body, all of it discarded on arrival. Projecting here
    // keeps ten times the payload off the pipe on every applied request.
    const info = result.info || {};
    reply.info = {
      origChars: info.origChars,
      compressedChars: info.compressedChars,
      outgoingTextChars: info.outgoingTextChars,
      imageCount: info.imageCount,
      imageBytes: info.imageBytes,
      imagePixels: info.imagePixels,
      imageTokens: info.imageTokens,
      baselineTokens: info.baselineTokens,
    };
    reply.cacheOwnsControl = result.cache?.ownsCacheControl === true;
    if (reply.applied) {
      if (!(result.body instanceof Uint8Array)) {
        throw new Error("transform applied but returned no body");
      }
      reply.body = decoder.decode(result.body);
    }
    reply.ok = true;
  } catch (error) {
    reply.ok = false;
    reply.error = error?.message || String(error);
  }
  send(reply);
}

// Requests are handled one at a time: the transform is CPU-bound, so overlapping
// them would only trade one slow answer for several.
let queue = Promise.resolve();
let buffered = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk) => {
  buffered += chunk;
  let cut;
  while ((cut = buffered.indexOf("\n")) >= 0) {
    const line = buffered.slice(0, cut);
    buffered = buffered.slice(cut + 1);
    if (!line.trim()) continue;
    let request;
    try {
      request = JSON.parse(line);
    } catch {
      continue;
    }
    queue = queue.then(() => handle(request));
  }
});
process.stdin.on("end", () => process.exit(0));
"#;

/// What to transform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransformRequest {
    /// The serialised Claude-format request body.
    pub body: String,
    /// The upstream model name, which the package uses to size images.
    pub model: String,
    /// Bodies smaller than this are not worth imaging.
    pub min_chars: u64,
}

/// Details the package reports about what it did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransformInfo {
    /// The compressible characters the package counted: the static slab and the tool
    /// documents, not the whole body. Its own threshold is measured against this,
    /// which is why a large body can still be refused as `below_min_chars`.
    #[serde(default)]
    pub orig_chars: u64,
    /// Characters the images replaced. **Not** bounded by [`Self::orig_chars`] or by
    /// the body length — tool-result prose counts here and not there — so it must
    /// never be subtracted from a character count without a floor.
    #[serde(default)]
    pub compressed_chars: u64,
    /// Text characters left on the wire after the transform.
    ///
    /// The package's own measure, and far better than inferring it: on a real applied
    /// request this read 203 against a `compressed_chars` of 46 407, where the
    /// subtraction upstream performs saturates to zero and loses the remaining text
    /// entirely. Absent on older versions, hence the fallback in
    /// [`crate::compress::Summary`].
    #[serde(default)]
    pub outgoing_text_chars: u64,
    #[serde(default)]
    pub image_count: u64,
    #[serde(default)]
    pub image_bytes: u64,
    #[serde(default)]
    pub image_pixels: u64,
    #[serde(default)]
    pub image_tokens: u64,
    #[serde(default)]
    pub baseline_tokens: u64,
}

/// The result of one transform attempt.
///
/// There is no error variant on purpose. A token saver that can fail a request is
/// worse than one that does nothing, so every failure arrives as [`Self::Bypassed`]
/// carrying why — which is also what the event log records and the dashboard shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransformOutcome {
    /// The body was replaced.
    Applied {
        body: String,
        info: TransformInfo,
        /// The package took over `cache_control`, so cache breakpoints must not be
        /// re-pinned over it.
        cache_owns_control: bool,
    },
    /// Nothing changed. `reason` is machine-readable; `detail` is for a human.
    Bypassed {
        reason: &'static str,
        detail: Option<String>,
    },
}

impl TransformOutcome {
    fn bypassed(reason: &'static str, detail: impl Into<Option<String>>) -> Self {
        Self::Bypassed {
            reason,
            detail: detail.into(),
        }
    }

    pub const fn applied(&self) -> bool {
        matches!(self, Self::Applied { .. })
    }
}

/// One reply frame from the worker.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Frame {
    #[serde(default)]
    r#type: String,
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    applied: bool,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    detail: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    info: TransformInfo,
    #[serde(default)]
    cache_owns_control: bool,
    /// The worker's own `process.versions.node`, on the ready frame.
    #[serde(default)]
    node: Option<String>,
}

/// One request frame to the worker.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RequestFrame<'a> {
    id: u64,
    body: &'a str,
    model: &'a str,
    min_chars: u64,
}

/// Why a worker could not be started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartError {
    /// The package is not installed, so there is nothing to import.
    NotInstalled,
    /// No `node` on the path. Distinguished from a broken install because the fix
    /// is different: install Node, not repair the package.
    NodeMissing,
    /// The package imported, but this machine's Node is below what it requires.
    ///
    /// Its own variant because it is neither a missing install nor a broken one: the
    /// package is fine, the environment is too old, and the fix is to upgrade Node.
    /// Left as a generic failure it surfaces per request as `crypto is not defined`,
    /// which points a user at their request instead of at their runtime.
    UnsupportedNode(String),
    /// The worker started and then failed, or never reported ready.
    Failed(String),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotInstalled => formatter.write_str("PXPIPE is not installed"),
            Self::NodeMissing => {
                formatter.write_str("node was not found, so the transform cannot be loaded")
            }
            Self::UnsupportedNode(detail) => formatter.write_str(detail),
            Self::Failed(detail) => write!(formatter, "the transform worker failed: {detail}"),
        }
    }
}

impl StartError {
    /// The machine-readable code, matching upstream's `error.code`.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NotInstalled => "NOT_INSTALLED",
            Self::NodeMissing => "NODE_MISSING",
            Self::UnsupportedNode(_) => "NODE_UNSUPPORTED",
            Self::Failed(_) => "WORKER_FAILED",
        }
    }
}

/// A running worker.
struct Worker {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    stderr: Arc<Mutex<VecDeque<String>>>,
    /// The package version this worker imported, so a status can say what is loaded
    /// rather than what is installed — they differ after an upgrade.
    version: Option<String>,
    /// The Node this worker is running, as it reported itself.
    node_version: Option<String>,
    loaded_at: u64,
    next_id: u64,
}

impl Worker {
    /// Read one frame, bounded in both size and time.
    async fn read_frame(&mut self, budget_ms: u64) -> Result<Frame, String> {
        let mut line = Vec::new();
        let read = tokio::time::timeout(
            std::time::Duration::from_millis(budget_ms),
            read_bounded_line(&mut self.stdout, &mut line),
        )
        .await;
        match read {
            Err(_) => Err("timeout".to_owned()),
            Ok(Err(error)) => Err(error),
            Ok(Ok(())) => serde_json::from_slice::<Frame>(&line)
                .map_err(|error| format!("unreadable reply: {error}")),
        }
    }

    fn stderr_tail(&self) -> Option<String> {
        let lines = self.stderr.lock().ok()?;
        if lines.is_empty() {
            return None;
        }
        Some(
            lines
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }

    async fn shut_down(mut self) {
        // Closing stdin lets it exit on its own; the kill is for a worker stuck
        // inside a transform, which cannot observe the close until it finishes.
        drop(self.stdin);
        let _ = self.child.kill().await;
    }
}

/// Read one newline-terminated frame, refusing one larger than [`MAX_FRAME_BYTES`].
async fn read_bounded_line(
    stdout: &mut BufReader<ChildStdout>,
    into: &mut Vec<u8>,
) -> Result<(), String> {
    let mut limited = stdout.take(MAX_FRAME_BYTES);
    let read = limited
        .read_until(b'\n', into)
        .await
        .map_err(|error| format!("could not read from the worker: {error}"))?;
    if read == 0 {
        return Err("the worker exited".to_owned());
    }
    if into.last() != Some(&b'\n') {
        return Err("the worker sent an oversized reply".to_owned());
    }
    Ok(())
}

/// What the dashboard shows about the loaded module.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadedInfo {
    pub loaded: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The Node running the transform, as it reported itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loaded_at: Option<u64>,
    pub uptime_ms: u64,
}

/// The transform, and the worker that serves it.
///
/// Cloneable and shared: the worker is behind a mutex because a transform is
/// CPU-bound, so serialising requests costs nothing that overlapping them would win
/// back, and one pipe cannot carry two conversations at once anyway.
#[derive(Debug, Clone)]
pub struct Bridge {
    paths: Paths,
    worker: Arc<tokio::sync::Mutex<Option<Worker>>>,
}

impl std::fmt::Debug for Worker {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Worker")
            .field("version", &self.version)
            .field("node_version", &self.node_version)
            .field("loaded_at", &self.loaded_at)
            .finish_non_exhaustive()
    }
}

impl Bridge {
    pub fn new(paths: Paths) -> Self {
        Self {
            paths,
            worker: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    pub const fn paths(&self) -> &Paths {
        &self.paths
    }

    /// Start the worker if it is not already running.
    pub async fn start(&self) -> Result<LoadedInfo, StartError> {
        let mut slot = self.worker.lock().await;
        if let Some(worker) = slot.as_ref() {
            return Ok(loaded_info(worker));
        }
        let worker = spawn_worker(&self.paths).await?;
        let info = loaded_info(&worker);
        *slot = Some(worker);
        drop(slot);
        Ok(info)
    }

    /// Stop the worker. Answers whether one was running.
    pub async fn stop(&self) -> bool {
        let taken = self.worker.lock().await.take();
        match taken {
            Some(worker) => {
                worker.shut_down().await;
                true
            }
            None => false,
        }
    }

    /// Stop and start, so an upgraded install takes effect without a restart.
    pub async fn restart(&self) -> Result<LoadedInfo, StartError> {
        self.stop().await;
        self.start().await
    }

    /// What is loaded right now.
    pub async fn loaded(&self) -> LoadedInfo {
        self.worker
            .lock()
            .await
            .as_ref()
            .map(loaded_info)
            .unwrap_or_default()
    }

    /// Recent worker stderr, for the logs view.
    pub async fn stderr_tail(&self) -> Option<String> {
        self.worker
            .lock()
            .await
            .as_ref()
            .and_then(Worker::stderr_tail)
    }

    /// Transform one body.
    ///
    /// `auto_load` mirrors upstream's `getTransform({ autoLoad })`: with it, the
    /// first request warms a cold worker; without it, a cold worker means a bypass.
    ///
    /// Never returns an error. A dead worker, a timeout, a malformed reply — each is
    /// a [`TransformOutcome::Bypassed`] and the caller sends the original body.
    pub async fn transform(
        &self,
        request: &TransformRequest,
        timeout_ms: u64,
        auto_load: bool,
    ) -> TransformOutcome {
        let mut slot = self.worker.lock().await;
        if slot.is_none() {
            if !auto_load {
                return TransformOutcome::bypassed("not_loaded", None);
            }
            match spawn_worker(&self.paths).await {
                Ok(worker) => *slot = Some(worker),
                Err(error) => {
                    let reason = match error {
                        StartError::NotInstalled => "not_installed",
                        StartError::NodeMissing => "node_missing",
                        StartError::UnsupportedNode(_) => "node_unsupported",
                        StartError::Failed(_) => "load_error",
                    };
                    return TransformOutcome::bypassed(reason, error.to_string());
                }
            }
        }
        let Some(worker) = slot.as_mut() else {
            return TransformOutcome::bypassed("not_loaded", None);
        };

        let id = worker.next_id;
        worker.next_id += 1;
        let frame = RequestFrame {
            id,
            body: &request.body,
            model: &request.model,
            min_chars: request.min_chars,
        };
        let mut line = match serde_json::to_vec(&frame) {
            Ok(line) => line,
            Err(error) => {
                return TransformOutcome::bypassed("encode_error", error.to_string());
            }
        };
        line.push(b'\n');

        if let Err(error) = worker.stdin.write_all(&line).await {
            let detail = worker.stderr_tail();
            let taken = slot.take();
            drop(slot);
            if let Some(worker) = taken {
                worker.shut_down().await;
            }
            return TransformOutcome::bypassed(
                "worker_gone",
                detail.unwrap_or_else(|| error.to_string()),
            );
        }
        if let Err(error) = worker.stdin.flush().await {
            return TransformOutcome::bypassed("worker_gone", error.to_string());
        }

        match worker.read_frame(timeout_ms).await {
            Ok(reply) => settle(reply),
            Err(error) => {
                // The worker is not reusable: a late answer would be read as the
                // reply to whichever request comes next. Killing it is also what
                // makes the timeout real — the transform stops with the process.
                let timed_out = error == "timeout";
                let detail = worker.stderr_tail().unwrap_or_else(|| {
                    if timed_out {
                        format!("no reply within {timeout_ms}ms")
                    } else {
                        error
                    }
                });
                let taken = slot.take();
                drop(slot);
                if let Some(worker) = taken {
                    worker.shut_down().await;
                }
                let reason = if timed_out {
                    "timeout"
                } else {
                    "transform_error"
                };
                TransformOutcome::bypassed(reason, detail)
            }
        }
    }

    /// Run a synthetic request through the worker, as upstream's `selfTest` does.
    ///
    /// A worker that imports but cannot transform is the failure this catches, and
    /// it is not visible from the install state alone.
    pub async fn self_test(&self) -> Result<SelfTest, String> {
        let started = crate::events::now_millis();
        let body = serde_json::json!({
            "model": "claude-fable-5",
            "max_tokens": 16,
            "messages": [{ "role": "user", "content": "ping" }],
        })
        .to_string();
        let request = TransformRequest {
            body,
            model: "claude-fable-5".to_owned(),
            // 0 lets the package's own gate decide, so a tiny body still exercises
            // the whole path instead of being refused before it starts.
            min_chars: 0,
        };
        let outcome = self
            .transform(&request, crate::DEFAULT_TIMEOUT_MS, true)
            .await;
        let duration_ms = crate::events::now_millis().saturating_sub(started);
        match outcome {
            TransformOutcome::Applied { .. } => Ok(SelfTest {
                reason: "applied".to_owned(),
                duration_ms,
            }),
            // A refusal is a healthy answer: the module parsed the request and
            // decided. Only the failure reasons are failures.
            TransformOutcome::Bypassed { reason, detail }
                if !matches!(
                    reason,
                    "timeout"
                        | "transform_error"
                        | "worker_gone"
                        | "not_installed"
                        | "node_missing"
                        | "node_unsupported"
                        | "load_error"
                        | "not_loaded"
                        | "encode_error"
                ) =>
            {
                let _ = detail;
                Ok(SelfTest {
                    reason: reason.to_owned(),
                    duration_ms,
                })
            }
            TransformOutcome::Bypassed { reason, detail } => {
                Err(detail.unwrap_or_else(|| reason.to_owned()))
            }
        }
    }
}

/// A passing self-test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfTest {
    pub reason: String,
    pub duration_ms: u64,
}

fn loaded_info(worker: &Worker) -> LoadedInfo {
    LoadedInfo {
        loaded: true,
        version: worker.version.clone(),
        node_version: worker.node_version.clone(),
        loaded_at: Some(worker.loaded_at),
        uptime_ms: crate::events::now_millis().saturating_sub(worker.loaded_at),
    }
}

/// The sentence to report when the running Node is below the package's requirement.
///
/// `None` when it satisfies it, when the package declares nothing, or when the
/// requirement is in a form this does not read — a wrong diagnosis is worse than no
/// diagnosis, so an unparsable range lets the worker run and fail on its own terms.
fn engine_mismatch(requires: Option<&str>, running: Option<&str>) -> Option<String> {
    let requires = requires?;
    let running = running?;
    if crate::install::node_satisfies(requires, running)? {
        return None;
    }
    Some(format!(
        "the installed pxpipe package requires node {requires}, but this router is running node \
         {running}; every transform would fail on a missing runtime global"
    ))
}

/// Turn a reply frame into an outcome.
fn settle(reply: Frame) -> TransformOutcome {
    if !reply.ok {
        return TransformOutcome::bypassed("transform_error", reply.error);
    }
    if !reply.applied {
        // The package's own reason, held to the set its `classifyReason` actually
        // emits. Bounded so the stats buckets stay finite, but the real names are
        // kept rather than folded together: `unsupported_model` is by far the most
        // likely reason a user sees nothing happen — the package images only three
        // model families unless `PXPIPE_MODELS` says otherwise — and reporting that
        // as a generic passthrough would send them looking for a fault that is a
        // setting. Anything outside the set becomes `passthrough` with the original
        // in `detail`, so a future release's new reason is visible rather than lost.
        let reason = match reply.reason.as_deref() {
            Some("unsupported_model") => "unsupported_model",
            Some("below_min_chars") => "below_min_chars",
            Some("below_min_tokens") => "below_min_tokens",
            Some("not_profitable") => "not_profitable",
            Some("compress_disabled") => "compress_disabled",
            Some("image_limit") => "image_limit",
            Some("parse_error") => "parse_error",
            Some("transform_error") => "transform_error",
            _ => "passthrough",
        };
        let detail = reply.detail.or(reply.reason);
        return TransformOutcome::bypassed(reason, detail);
    }
    match reply.body {
        Some(body) => TransformOutcome::Applied {
            body,
            info: reply.info,
            cache_owns_control: reply.cache_owns_control,
        },
        // Claimed applied with nothing to apply. Reported rather than trusted.
        None => TransformOutcome::bypassed(
            "transform_error",
            "the transform reported a change but sent no body".to_owned(),
        ),
    }
}

/// Spawn a worker and wait for it to report that the module imported.
async fn spawn_worker(paths: &Paths) -> Result<Worker, StartError> {
    let info = install_info(paths);
    if !info.installed {
        return Err(StartError::NotInstalled);
    }
    let node = find_node().ok_or(StartError::NodeMissing)?;
    let entry = paths.library_entry();

    let mut child = Command::new(node)
        .arg("--input-type=module")
        .arg("-e")
        .arg(WORKER_SOURCE)
        .env("PXPIPE_ENTRY", &entry)
        .current_dir(&paths.root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Without this a killed worker is reaped only when the router exits.
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| StartError::Failed(format!("could not start node: {error}")))?;

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| StartError::Failed("node gave no stdin".to_owned()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| StartError::Failed("node gave no stdout".to_owned()))?;
    let stderr_lines = Arc::new(Mutex::new(VecDeque::new()));
    if let Some(stderr) = child.stderr.take() {
        drain_stderr(stderr, Arc::clone(&stderr_lines));
    }

    let mut worker = Worker {
        child,
        stdin,
        stdout: BufReader::new(stdout),
        stderr: stderr_lines,
        version: info.version,
        node_version: None,
        loaded_at: crate::events::now_millis(),
        next_id: 1,
    };
    let requires_node = info.requires_node;

    match worker.read_frame(READY_TIMEOUT_MS).await {
        Ok(frame) if frame.r#type == "ready" && frame.error.is_none() => {
            worker.node_version = frame.node;
            // The module imported, which is not the same as being able to run. An
            // under-version Node imports the package and then fails every transform
            // on a missing global; refused here, where the reason is legible.
            if let Some(problem) =
                engine_mismatch(requires_node.as_deref(), worker.node_version.as_deref())
            {
                worker.shut_down().await;
                return Err(StartError::UnsupportedNode(problem));
            }
            Ok(worker)
        }
        Ok(frame) => {
            let detail = frame
                .error
                .or_else(|| worker.stderr_tail())
                .unwrap_or_else(|| "the worker did not report ready".to_owned());
            worker.shut_down().await;
            Err(StartError::Failed(detail))
        }
        Err(error) => {
            let detail = worker.stderr_tail().unwrap_or(error);
            worker.shut_down().await;
            Err(StartError::Failed(detail))
        }
    }
}

/// Keep the last [`MAX_STDERR_LINES`] of worker stderr.
///
/// Drained rather than ignored: an unread pipe fills and blocks the worker inside
/// its own write, which would look exactly like a hung transform.
fn drain_stderr(stderr: tokio::process::ChildStderr, into: Arc<Mutex<VecDeque<String>>>) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Ok(mut buffer) = into.lock() {
                if buffer.len() == MAX_STDERR_LINES {
                    buffer.pop_front();
                }
                buffer.push_back(line);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{Frame, StartError, TransformOutcome, settle};

    fn frame(json: serde_json::Value) -> Frame {
        serde_json::from_value(json).expect("frame")
    }

    #[test]
    fn an_applied_reply_carries_the_new_body() {
        let outcome = settle(frame(serde_json::json!({
            "type": "result", "ok": true, "applied": true,
            "body": "{\"messages\":[]}",
            "info": { "imageCount": 3, "compressedChars": 40_000, "baselineTokens": 12_000 },
            "cacheOwnsControl": true,
        })));
        match outcome {
            TransformOutcome::Applied {
                body,
                info,
                cache_owns_control,
            } => {
                assert_eq!(body, "{\"messages\":[]}");
                assert_eq!(info.image_count, 3);
                assert_eq!(info.compressed_chars, 40_000);
                assert_eq!(info.baseline_tokens, 12_000);
                assert!(cache_owns_control, "the package took over cache_control");
            }
            TransformOutcome::Bypassed { reason, .. } => panic!("bypassed as {reason}"),
        }
    }

    #[test]
    fn a_thrown_transform_is_a_bypass_carrying_the_message() {
        let outcome = settle(frame(serde_json::json!({
            "type": "result", "ok": false, "error": "boom",
        })));
        assert_eq!(
            outcome,
            TransformOutcome::Bypassed {
                reason: "transform_error",
                detail: Some("boom".to_owned()),
            }
        );
    }

    #[test]
    fn a_reply_claiming_a_change_with_no_body_is_not_trusted() {
        let outcome = settle(frame(serde_json::json!({
            "type": "result", "ok": true, "applied": true,
        })));
        // Sending the original is right; sending nothing would be a broken request.
        assert!(matches!(
            outcome,
            TransformOutcome::Bypassed {
                reason: "transform_error",
                ..
            }
        ));
    }

    /// The exact set `pxpipe-proxy`'s own `classifyReason` emits, taken from the
    /// installed package rather than guessed — an earlier draft of this mapping
    /// invented three names the package never sends, which would have filed every
    /// real refusal under `passthrough`.
    #[test]
    fn the_packages_own_refusals_survive_as_reasons() {
        for (given, expected) in [
            ("unsupported_model", "unsupported_model"),
            ("below_min_chars", "below_min_chars"),
            ("below_min_tokens", "below_min_tokens"),
            ("not_profitable", "not_profitable"),
            ("compress_disabled", "compress_disabled"),
            ("image_limit", "image_limit"),
            ("parse_error", "parse_error"),
            ("something-new", "passthrough"),
        ] {
            let outcome = settle(frame(serde_json::json!({
                "type": "result", "ok": true, "applied": false, "reason": given,
            })));
            match outcome {
                TransformOutcome::Bypassed { reason, detail } => {
                    assert_eq!(reason, expected);
                    // An unrecognised reason is still reported, not discarded.
                    assert_eq!(detail.as_deref(), Some(given));
                }
                TransformOutcome::Applied { .. } => panic!("{given} must not apply"),
            }
        }
    }

    #[test]
    fn start_errors_report_a_code_and_a_sentence() {
        assert_eq!(StartError::NotInstalled.code(), "NOT_INSTALLED");
        assert_eq!(StartError::NodeMissing.code(), "NODE_MISSING");
        assert_eq!(StartError::Failed(String::new()).code(), "WORKER_FAILED");
        assert!(
            StartError::NodeMissing.to_string().contains("node"),
            "the message names what is missing"
        );
    }
}
