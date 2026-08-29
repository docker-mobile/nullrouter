//! The façade the routes and the request path both call.
//!
//! Ports `inspire/src/lib/pxpipe/service.js`, plus the wiring the upstream route
//! handlers do inline.
//!
//! Two callers, deliberately one type. `api-actix` serves the eight `/api/pxpipe/*`
//! routes from it; `runtime-actix` calls [`TokenSaver::compress`] on the way to the
//! provider. Sharing it is what keeps them from disagreeing — a status page claiming
//! the saver is running while the request path silently bypasses would be worse than
//! either being wrong alone.

use serde::Serialize;

use crate::bridge::{Bridge, LoadedInfo, SelfTest, StartError, TransformRequest};
use crate::compress::{Eligibility, Gate, Summary, budget, eligibility};
use crate::events::{self, Stats};
use crate::install::{InstallInfo, InstallOutcome, Paths, find_npm, install, install_info};

/// Aggregate status, for the Token Saver card and `GET /api/pxpipe/status`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub installed: bool,
    pub installing: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Whether the transform is loaded and ready.
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loaded_at: Option<u64>,
    pub uptime_ms: u64,
    /// The Node the loaded worker reports, when one is loaded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_version: Option<String>,
    /// The package's declared `engines.node`, when it declares one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires_node: Option<String>,
    pub npm_available: bool,
    /// Whether a `node` was found. Upstream has no equivalent field because it *is*
    /// Node; here the transform lives in a child process, so its absence is a
    /// distinct and reportable state.
    pub node_available: bool,
    /// How the transform is reached. `worker` here, against upstream's `library`:
    /// the difference is real, and naming it `library` would misdescribe the port.
    pub mode: &'static str,
}

/// One line of the health checklist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthStep {
    pub id: &'static str,
    pub label: &'static str,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// The health check result.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthCheck {
    pub healthy: bool,
    pub checks: Vec<HealthStep>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What one `compress` call did.
#[derive(Debug, Clone, PartialEq)]
pub struct Compressed {
    /// The replacement body, or `None` when nothing changed.
    pub body: Option<String>,
    pub summary: Summary,
}

/// The token saver.
#[derive(Debug, Clone)]
pub struct TokenSaver {
    bridge: Bridge,
}

impl TokenSaver {
    pub fn new(paths: Paths) -> Self {
        Self {
            bridge: Bridge::new(paths),
        }
    }

    /// A saver over the data directory this deployment uses.
    pub fn discover() -> Self {
        Self::new(Paths::discover())
    }

    pub const fn bridge(&self) -> &Bridge {
        &self.bridge
    }

    const fn paths(&self) -> &Paths {
        self.bridge.paths()
    }

    /// Where the package is and whether it is usable.
    pub fn install_info(&self) -> InstallInfo {
        install_info(self.paths())
    }

    /// The full status.
    pub async fn status(&self) -> Status {
        let install = self.install_info();
        let loaded = self.bridge.loaded().await;
        Status {
            installed: install.installed,
            // Nothing tracks an install in flight across processes here; the route
            // that installs holds the request open until npm returns, so a client
            // learns the outcome from that response rather than by polling.
            installing: false,
            version: install.version.clone(),
            path: install.path.clone(),
            running: loaded.loaded,
            loaded_at: loaded.loaded_at,
            uptime_ms: loaded.uptime_ms,
            node_version: loaded.node_version,
            requires_node: install.requires_node,
            npm_available: find_npm().is_some(),
            node_available: crate::install::find_node().is_some(),
            mode: "worker",
        }
    }

    /// Install or repair, then drop any loaded worker so the next load takes the
    /// fresh version.
    ///
    /// Blocking: `npm install` is a subprocess. Called from a blocking context by
    /// the route, which is why this is not `async`.
    pub fn install(&self) -> InstallOutcome {
        install(self.paths())
    }

    /// Start the worker.
    pub async fn start(&self) -> Result<LoadedInfo, StartError> {
        self.bridge.start().await
    }

    /// Stop the worker. Requests then bypass rather than fail.
    pub async fn stop(&self) -> bool {
        self.bridge.stop().await
    }

    /// Reload the worker, picking up an upgraded install.
    pub async fn restart(&self) -> Result<LoadedInfo, StartError> {
        self.bridge.restart().await
    }

    /// Run upstream's checklist: installed, then loads, then transforms.
    ///
    /// Ordered so the first failure is the informative one — "not installed" makes
    /// "cannot load" redundant, and a checklist that reports both invites fixing the
    /// wrong thing.
    pub async fn health(&self) -> HealthCheck {
        let mut checks = Vec::new();
        let install = self.install_info();
        checks.push(HealthStep {
            id: "installed",
            label: "PXPIPE installed",
            ok: install.installed,
            detail: install
                .version
                .as_ref()
                .map(|version| format!("v{version}")),
        });
        if !install.installed {
            return failed(checks, "pxpipe not installed");
        }

        match self.bridge.start().await {
            Ok(_) => checks.push(HealthStep {
                id: "module",
                label: "Transform module loads",
                ok: true,
                detail: install
                    .version
                    .as_ref()
                    .map(|version| format!("v{version}")),
            }),
            Err(error) => {
                let message = error.to_string();
                checks.push(HealthStep {
                    id: "module",
                    label: "Transform module loads",
                    ok: false,
                    detail: Some(message.clone()),
                });
                return failed(checks, &format!("Cannot load module: {message}"));
            }
        }

        match self.bridge.self_test().await {
            Ok(SelfTest {
                reason,
                duration_ms,
            }) => {
                checks.push(HealthStep {
                    id: "transform",
                    label: "Test request transforms",
                    ok: true,
                    detail: Some(format!("{duration_ms}ms ({reason})")),
                });
                HealthCheck {
                    healthy: true,
                    checks,
                    error: None,
                }
            }
            Err(message) => {
                checks.push(HealthStep {
                    id: "transform",
                    label: "Test request transforms",
                    ok: false,
                    detail: Some(message.clone()),
                });
                failed(checks, &format!("Self-test failed: {message}"))
            }
        }
    }

    /// The tail of the install log.
    ///
    /// Separate from [`Self::logs`] so a failed install can report why without also
    /// reading the event file.
    pub fn install_log_tail(&self) -> String {
        crate::install::install_log_tail(self.paths())
    }

    /// The install log and the recent events, for `GET /api/pxpipe/logs`.
    pub async fn logs(&self, limit: usize) -> Logs {
        let mut recent = events::read(self.paths(), None, Some(limit));
        recent.reverse();
        Logs {
            install_log: crate::install::install_log_tail(self.paths()),
            worker_log: self.bridge.stderr_tail().await,
            events: recent,
        }
    }

    /// The aggregates, for `GET /api/pxpipe/stats`.
    pub fn stats(&self, recent_limit: usize) -> Stats {
        events::stats(self.paths(), events::now_millis(), recent_limit)
    }

    /// Compress one request body, recording the attempt.
    ///
    /// Never fails: every path answers with the original body and a reason. The
    /// event is written for skips too — a saver that appears to do nothing is a
    /// support question, and the log is the answer to it.
    pub async fn compress(&self, body: &str, model: &str, gate: &Gate) -> Compressed {
        let original_chars = u64::try_from(body.chars().count()).unwrap_or(u64::MAX);
        let min_chars = match eligibility(gate, original_chars) {
            Eligibility::Skip { reason, detail } => {
                let summary = Summary::skipped(reason, detail, original_chars);
                self.record(&summary);
                return Compressed {
                    body: None,
                    summary,
                };
            }
            Eligibility::Eligible { min_chars } => min_chars,
        };

        let started = events::now_millis();
        let request = TransformRequest {
            body: body.to_owned(),
            model: model.to_owned(),
            min_chars,
        };
        let outcome = self
            .bridge
            .transform(&request, budget(gate.timeout_ms), true)
            .await;
        let duration_ms = events::now_millis().saturating_sub(started);
        let summary = Summary::from_outcome(&outcome, original_chars, duration_ms);
        self.record(&summary);
        Compressed {
            body: match outcome {
                crate::bridge::TransformOutcome::Applied { body, .. } => Some(body),
                crate::bridge::TransformOutcome::Bypassed { .. } => None,
            },
            summary,
        }
    }

    fn record(&self, summary: &Summary) {
        events::append(self.paths(), &summary.event(events::now_millis()));
    }
}

/// The logs payload.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Logs {
    pub install_log: String,
    /// Worker stderr. Upstream has no equivalent because it has no worker; a load
    /// failure there surfaces in the server's own log.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worker_log: Option<String>,
    pub events: Vec<crate::events::Event>,
}

fn failed(checks: Vec<HealthStep>, error: &str) -> HealthCheck {
    HealthCheck {
        healthy: false,
        checks,
        error: Some(error.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::TokenSaver;
    use crate::compress::Gate;
    use crate::install::Paths;

    fn saver() -> (tempfile::TempDir, TokenSaver) {
        let dir = tempfile::tempdir().expect("tempdir");
        let saver = TokenSaver::new(Paths::new(dir.path()));
        (dir, saver)
    }

    fn gate() -> Gate {
        Gate {
            enabled: true,
            claude_format: true,
            format: "claude".to_owned(),
            min_chars: 0,
            timeout_ms: 0,
        }
    }

    #[tokio::test]
    async fn status_reports_a_missing_install_without_claiming_a_version() {
        let (_dir, saver) = saver();
        let status = saver.status().await;
        assert!(!status.installed);
        assert!(!status.running);
        assert_eq!(status.version, None);
        assert_eq!(status.uptime_ms, 0);
        // Named for what it is, not for upstream's in-process arrangement.
        assert_eq!(status.mode, "worker");
    }

    #[tokio::test]
    async fn health_stops_at_the_first_failure() {
        let (_dir, saver) = saver();
        let health = saver.health().await;
        assert!(!health.healthy);
        assert_eq!(health.error.as_deref(), Some("pxpipe not installed"));
        // One step, not three: "cannot load" and "cannot transform" would both be
        // true and neither would be the thing to fix.
        assert_eq!(health.checks.len(), 1);
        assert_eq!(health.checks.first().map(|step| step.id), Some("installed"));
    }

    #[tokio::test]
    async fn a_disabled_saver_leaves_the_body_alone_and_says_so() {
        let (_dir, saver) = saver();
        let gate = Gate {
            enabled: false,
            ..gate()
        };
        let result = saver
            .compress("{\"messages\":[]}", "claude-fable-5", &gate)
            .await;
        assert_eq!(result.body, None);
        assert_eq!(result.summary.reason, "disabled");
        assert!(!result.summary.applied);
    }

    #[tokio::test]
    async fn a_missing_install_is_a_bypass_not_a_failure() {
        let (_dir, saver) = saver();
        // Over the threshold and Claude-format, so the gate passes and the bridge is
        // actually asked — with nothing installed to ask.
        let body = format!("{{\"messages\":[\"{}\"]}}", "x".repeat(30_000));
        let result = saver.compress(&body, "claude-fable-5", &gate()).await;
        assert_eq!(result.body, None, "the caller sends the original");
        assert_eq!(result.summary.reason, "not_installed");
        assert!(
            result
                .summary
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("not installed")),
            "the reason a user can act on travels with it: {:?}",
            result.summary.detail
        );
    }

    #[tokio::test]
    async fn every_attempt_is_recorded_including_the_skips() {
        let (_dir, saver) = saver();
        saver.compress("{}", "claude-fable-5", &gate()).await;
        let disabled = Gate {
            enabled: false,
            ..gate()
        };
        saver.compress("{}", "claude-fable-5", &disabled).await;

        let stats = saver.stats(10);
        assert_eq!(stats.windows.all.requests, 2);
        assert_eq!(stats.windows.all.compressed, 0);
        // Neither is an error: one body was too small, one saver was off.
        assert_eq!(stats.windows.all.errors, 0);
        assert_eq!(stats.windows.all.bypassed, 2);
        let reasons: Vec<&str> = stats
            .recent
            .iter()
            .map(|event| event.reason.as_str())
            .collect();
        assert!(reasons.contains(&"disabled"), "{reasons:?}");
        assert!(reasons.contains(&"below_threshold"), "{reasons:?}");
    }

    #[tokio::test]
    async fn logs_answer_before_anything_has_been_installed() {
        let (_dir, saver) = saver();
        let logs = saver.logs(10).await;
        assert!(logs.install_log.is_empty());
        assert!(logs.worker_log.is_none());
        assert!(logs.events.is_empty());
    }

    #[tokio::test]
    async fn stopping_a_saver_that_was_never_started_is_not_an_error() {
        let (_dir, saver) = saver();
        assert!(!saver.stop().await, "nothing was running");
    }
}
