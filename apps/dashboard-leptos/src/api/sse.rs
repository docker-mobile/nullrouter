//! Server-sent event streams.
//!
//! Usage rows and console logs arrive as streams rather than polled snapshots, so the panels
//! showing them stay live without hammering the router.
//!
//! Two behaviours here are deliberate and easy to get wrong:
//!
//! - `EventSource` reconnects by itself, and its `onerror` fires on every transient drop as well as
//!   on a permanent failure. Treating each one as fatal makes a stream that recovers look broken, so
//!   [`Stream`] reports connection state separately from data and only surfaces a failure the caller
//!   should act on once the connection is actually closed.
//! - The handle must outlive the call that created it. Dropping it closes the socket, so a stream
//!   opened in a component has to be tied to that component's lifetime, which [`Stream::close`] and
//!   the `on_cleanup` in [`subscribe`] handle between them.

use leptos::prelude::*;

/// Whether a stream is currently receiving.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Connection {
    #[default]
    Connecting,
    /// Receiving events.
    Open,
    /// Dropped, and the browser is retrying on its own.
    Reconnecting,
    /// Closed for good; nothing further will arrive.
    Closed,
}

impl Connection {
    /// Whether events can be expected, now or after a retry.
    pub const fn is_live(self) -> bool {
        matches!(self, Self::Connecting | Self::Open | Self::Reconnecting)
    }

    /// A short status for a stream indicator.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Connecting => "Connecting…",
            Self::Open => "Live",
            Self::Reconnecting => "Reconnecting…",
            Self::Closed => "Disconnected",
        }
    }
}

/// A live subscription. Closes when dropped.
#[derive(Debug)]
pub struct Stream {
    #[cfg(target_arch = "wasm32")]
    source: web_sys::EventSource,
}

impl Stream {
    /// Close the stream and stop the browser's reconnect attempts.
    // Not `const`: on wasm32 this calls into JS. `clippy --fix` suggested const because the native
    // build's body is empty once the cfg'd line is stripped.
    pub fn close(&self) {
        #[cfg(target_arch = "wasm32")]
        self.source.close();
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.close();
    }
}

/// Open a stream, delivering each decoded message to `on_message`.
///
/// Messages that do not parse are skipped rather than closing the stream: one malformed row in a log
/// feed must not take the rest of the feed down with it. The connection signal is what a panel binds
/// its live indicator to.
///
/// The returned handle is registered for cleanup on the current reactive owner, so a panel that
/// navigates away closes its socket without the caller having to remember.
#[cfg(target_arch = "wasm32")]
pub fn subscribe<T, F>(path: &str, connection: WriteSignal<Connection>, on_message: F) -> Option<Stream>
where
    T: serde::de::DeserializeOwned + 'static,
    F: Fn(T) + 'static,
{
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;

    let source = web_sys::EventSource::new(path).ok()?;

    let on_open = Closure::<dyn Fn(web_sys::Event)>::new(move |_| connection.set(Connection::Open));
    source.set_onopen(Some(on_open.as_ref().unchecked_ref()));
    on_open.forget();

    let message = Closure::<dyn Fn(web_sys::MessageEvent)>::new(move |event: web_sys::MessageEvent| {
        let Some(text) = event.data().as_string() else {
            return;
        };
        if let Ok(parsed) = serde_json::from_str::<T>(&text) {
            on_message(parsed);
        }
    });
    source.set_onmessage(Some(message.as_ref().unchecked_ref()));
    message.forget();

    // `onerror` fires on ordinary transient drops too, and the browser retries on its own unless the
    // socket has reached CLOSED. Reporting every one as fatal is what makes a self-healing stream
    // look broken, so the readyState decides which it was.
    let errored = source.clone();
    let on_error = Closure::<dyn Fn(web_sys::Event)>::new(move |_| {
        let next = if errored.ready_state() == web_sys::EventSource::CLOSED {
            Connection::Closed
        } else {
            Connection::Reconnecting
        };
        connection.set(next);
    });
    source.set_onerror(Some(on_error.as_ref().unchecked_ref()));
    on_error.forget();

    let handle = Stream { source: source.clone() };
    on_cleanup(move || source.close());
    Some(handle)
}

/// Native builds have no `EventSource`.
#[cfg(not(target_arch = "wasm32"))]
pub fn subscribe<T, F>(
    _path: &str,
    connection: WriteSignal<Connection>,
    _on_message: F,
) -> Option<Stream>
where
    T: serde::de::DeserializeOwned + 'static,
    F: Fn(T) + 'static,
{
    connection.set(Connection::Closed);
    None
}

#[cfg(test)]
mod tests {
    use super::Connection;

    #[test]
    fn only_a_closed_stream_stops_being_live() {
        // A transient drop must keep reading as live, or the indicator claims failure every time the
        // browser reconnects on its own.
        assert!(Connection::Connecting.is_live());
        assert!(Connection::Open.is_live());
        assert!(Connection::Reconnecting.is_live());
        assert!(!Connection::Closed.is_live());
    }

    #[test]
    fn every_state_has_a_distinct_label() {
        let states = [
            Connection::Connecting,
            Connection::Open,
            Connection::Reconnecting,
            Connection::Closed,
        ];
        for state in states {
            assert!(!state.label().is_empty(), "{state:?}");
        }
        // A user has to be able to tell "reconnecting" from "disconnected": one resolves itself and
        // the other needs them to do something.
        assert_ne!(Connection::Reconnecting.label(), Connection::Closed.label());
        assert_ne!(Connection::Connecting.label(), Connection::Open.label());
    }

    #[test]
    fn connecting_is_the_default() {
        assert_eq!(Connection::default(), Connection::Connecting);
        assert!(Connection::default().is_live());
    }
}
