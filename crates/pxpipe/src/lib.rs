//! PXPIPE: install management, event statistics, and the transform bridge.
//!
//! Ports `inspire/src/lib/pxpipe/` and `inspire/open-sse/rtk/pxpipe.js`.
//!
//! PXPIPE renders bulky Claude-format context as dense PNGs, which bill by pixel
//! rather than by token. The compression itself lives in the `pxpipe-proxy` npm
//! package and is only reachable as JavaScript, so this crate manages the package
//! and calls it through a short-lived Node process rather than reimplementing it —
//! see [`bridge`]. Everything else is ported directly: where the package is
//! installed, whether it is usable, the per-request event log, and the aggregates
//! the dashboard reads.
//!
//! Two properties hold throughout. **It fails open**: any failure — no Node, no
//! package, a timeout, a malformed reply — leaves the request untouched rather than
//! rejecting it, because a token saver that can break a request is worse than one
//! that does nothing. And **nothing is faked**: where Node or npm is absent, that is
//! reported as the reason rather than reported as "not installed" or, worse, as a
//! transform that silently did nothing.

pub mod bridge;
pub mod compress;
pub mod events;
pub mod install;
pub mod service;

pub use bridge::{Bridge, TransformOutcome, TransformRequest};
pub use compress::{Eligibility, Gate, Summary};
pub use events::{Event, Stats, Totals};
pub use install::{InstallInfo, InstallOutcome, Paths};
pub use service::{HealthCheck, Status, TokenSaver};

/// The npm package this wraps.
pub const PACKAGE: &str = "pxpipe-proxy";

/// Default minimum body size before compression is worth attempting
/// (upstream `DEFAULT_MIN_CHARS`).
pub const DEFAULT_MIN_CHARS: u64 = 25_000;

/// Default budget for one transform (upstream `DEFAULT_TIMEOUT_MS`).
pub const DEFAULT_TIMEOUT_MS: u64 = 15_000;
