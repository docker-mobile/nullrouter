//! MCP stdio-to-SSE bridge.
//!
//! `plugins` is the spawn whitelist, `filter` shrinks oversized tool results, and `bridge` owns the
//! child processes and their SSE listeners.

pub(crate) mod bridge;
pub(crate) mod filter;
pub(crate) mod plugins;

/// A handle to the MCP children, for a `main` that must reap them at shutdown.
///
/// `bridge::Bridge` stays crate-private: it exposes `attach`, `send` and the listener plumbing,
/// none of which a binary should reach. This wrapper is the whole outside surface — hold one, pass
/// it to [`register`](Self::register), and call [`shutdown`](Self::shutdown) when the server stops.
///
/// It exists because `kill_all` is worthless if nobody calls it: without this, every `npx` child
/// spawned by a plugin outlives the service that started it.
#[derive(Debug, Clone, Default)]
pub struct McpBridge {
    inner: bridge::Bridge,
}

impl McpBridge {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register this bridge on an app so its routes use it.
    ///
    /// Must be called *before* `configure`, which registers a default bridge only if none is
    /// present. Registering after would leave `configure`'s own bridge in place and this handle
    /// reaping nothing.
    pub fn register(&self, config: &mut actix_web::web::ServiceConfig) {
        config.app_data(actix_web::web::Data::new(self.inner.clone()));
    }

    /// Kill every MCP child this bridge spawned.
    pub async fn shutdown(&self) {
        self.inner.kill_all().await;
    }
}
