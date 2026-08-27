//! Shared HTTP client and hydration state for the dashboard.
//!
//! Before this existed the app had exactly one `fetch` call site (auth status /
//! logout) and every panel rendered compile-time fixtures. That made the UI
//! assert things that were not true — a Settings toggle that discarded writes,
//! a Providers page listing accounts that did not exist.
//!
//! [`Hydrate`] is the contract that prevents that: a panel is either loading,
//! holding real data, or explaining a failure. There is no state in which
//! fabricated data can be presented as live.

use leptos::prelude::*;

/// Why a request did not produce usable data.
///
/// Kept coarse on purpose: the UI needs to tell a user what to do, not surface
/// transport internals.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApiError {
    /// The browser environment was unavailable (no `window`).
    Environment,
    /// The request could not be constructed.
    RequestBuild,
    /// The request never reached the server.
    Network,
    /// The server answered with a non-2xx status.
    Status(u16),
    /// The response body could not be read or decoded.
    Body,
}

impl ApiError {
    /// A short, user-facing explanation.
    pub const fn message(self) -> &'static str {
        match self {
            Self::Environment => "Browser environment unavailable.",
            Self::RequestBuild => "Could not build the request.",
            Self::Network => "Could not reach the local router. Is the service running?",
            Self::Status(401) => "Not signed in. Reload and sign in again.",
            Self::Status(403) => "This action is not permitted.",
            Self::Status(404) => "That endpoint is not available on this build.",
            Self::Status(503) => "The service is temporarily unavailable.",
            Self::Status(_) => "The local router returned an error.",
            Self::Body => "The response could not be read.",
        }
    }
}

/// HTTP verb for a dashboard request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Method {
    Get,
    Post,
    Put,
    Delete,
}

impl Method {
    /// Only read by the wasm request path; the native stub never sends.
    #[cfg_attr(
        not(target_arch = "wasm32"),
        allow(dead_code, reason = "used only by the wasm fetch path")
    )]
    const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Delete => "DELETE",
        }
    }
}

/// Loading state for one piece of server-owned data.
///
/// Panels match on this exhaustively, so a failure or an empty result is always
/// rendered as itself rather than silently replaced by a placeholder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Hydrate<T> {
    /// The first request is still in flight.
    #[default]
    Loading,
    Ready(T),
    Failed(ApiError),
}

impl<T> Hydrate<T> {
    /// The data, when present.
    pub const fn ready(&self) -> Option<&T> {
        match self {
            Self::Ready(value) => Some(value),
            Self::Loading | Self::Failed(_) => None,
        }
    }

    /// `true` while the first request is still in flight.
    pub const fn is_loading(&self) -> bool {
        matches!(self, Self::Loading)
    }

    /// The failure, when the request did not succeed.
    pub const fn failure(&self) -> Option<ApiError> {
        match self {
            Self::Failed(error) => Some(*error),
            Self::Loading | Self::Ready(_) => None,
        }
    }
}

/// Whether a write is in flight, and how the last one ended.
///
/// Separate from [`Hydrate`] because a write does not replace the panel's data:
/// the row stays visible while saving, and a failure must be recoverable.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Save {
    #[default]
    Idle,
    Saving,
    Saved,
    Failed(ApiError),
}

impl Save {
    pub const fn is_saving(&self) -> bool {
        matches!(self, Self::Saving)
    }

    /// Status text for the row, or `None` when idle.
    pub const fn status(&self) -> Option<&'static str> {
        match self {
            Self::Idle => None,
            Self::Saving => Some("Saving…"),
            Self::Saved => Some("Saved"),
            Self::Failed(error) => Some(error.message()),
        }
    }
}

/// Perform a JSON request and return the raw body.
///
/// `body` is sent only for the verbs that carry one. Credentials are included
/// so the dashboard session cookie reaches session-gated routes, and caching is
/// disabled so a panel never renders a stale snapshot.
#[cfg(target_arch = "wasm32")]
pub async fn request(method: Method, path: &str, body: Option<&str>) -> Result<String, ApiError> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{Request, RequestCache, RequestCredentials, RequestInit, Response};

    let init = RequestInit::new();
    init.set_method(method.as_str());
    init.set_credentials(RequestCredentials::SameOrigin);
    init.set_cache(RequestCache::NoStore);
    if let Some(payload) = body {
        init.set_body(&wasm_bindgen::JsValue::from_str(payload));
    }

    let request =
        Request::new_with_str_and_init(path, &init).map_err(|_| ApiError::RequestBuild)?;
    if body.is_some() {
        request
            .headers()
            .set("content-type", "application/json")
            .map_err(|_| ApiError::RequestBuild)?;
    }

    let window = web_sys::window().ok_or(ApiError::Environment)?;
    let response = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|_| ApiError::Network)?
        .dyn_into::<Response>()
        .map_err(|_| ApiError::Body)?;

    if !response.ok() {
        return Err(ApiError::Status(response.status()));
    }
    let text = JsFuture::from(response.text().map_err(|_| ApiError::Body)?)
        .await
        .map_err(|_| ApiError::Body)?;
    text.as_string().ok_or(ApiError::Body)
}

/// Native builds have no browser to fetch from.
///
/// The native target exists so panel logic and parsing stay unit-testable; any
/// request there is a programming error, reported rather than faked.
#[cfg(not(target_arch = "wasm32"))]
#[allow(
    clippy::unused_async,
    reason = "mirrors the wasm signature so callers are target-agnostic"
)]
pub async fn request(
    _method: Method,
    _path: &str,
    _body: Option<&str>,
) -> Result<String, ApiError> {
    Err(ApiError::Environment)
}

/// `GET` a path.
pub async fn get(path: &str) -> Result<String, ApiError> {
    request(Method::Get, path, None).await
}

/// `POST` a JSON body.
pub async fn post(path: &str, body: &str) -> Result<String, ApiError> {
    request(Method::Post, path, Some(body)).await
}

/// `PUT` a JSON body.
pub async fn put(path: &str, body: &str) -> Result<String, ApiError> {
    request(Method::Put, path, Some(body)).await
}

/// `DELETE` a path.
pub async fn delete(path: &str) -> Result<String, ApiError> {
    request(Method::Delete, path, None).await
}

/// Fetch a path, parse it, and drive a [`Hydrate`] signal.
///
/// `parse` converts the raw body; returning `None` is treated as a body error,
/// so a shape change upstream surfaces as a visible failure rather than an
/// empty panel that looks like "no data".
#[cfg(target_arch = "wasm32")]
pub fn hydrate<T, F>(path: &'static str, setter: WriteSignal<Hydrate<T>>, parse: F)
where
    T: Send + Sync + 'static,
    F: Fn(&str) -> Option<T> + 'static,
{
    wasm_bindgen_futures::spawn_local(async move {
        let next = match get(path).await {
            Ok(body) => parse(&body).map_or(Hydrate::Failed(ApiError::Body), Hydrate::Ready),
            Err(error) => Hydrate::Failed(error),
        };
        setter.set(next);
    });
}

#[cfg(not(target_arch = "wasm32"))]
pub fn hydrate<T, F>(_path: &'static str, setter: WriteSignal<Hydrate<T>>, _parse: F)
where
    T: Send + Sync + 'static,
    F: Fn(&str) -> Option<T> + 'static,
{
    setter.set(Hydrate::Failed(ApiError::Environment));
}

#[cfg(test)]
mod tests {
    use super::{ApiError, Hydrate, Method, Save};

    #[test]
    fn hydrate_starts_loading() {
        let state: Hydrate<u8> = Hydrate::default();
        assert!(state.is_loading());
        assert!(state.ready().is_none());
        assert!(state.failure().is_none());
    }

    #[test]
    fn hydrate_exposes_data_only_when_ready() {
        let ready = Hydrate::Ready(7_u8);
        assert_eq!(ready.ready(), Some(&7));
        assert!(!ready.is_loading());
        assert!(ready.failure().is_none());

        let failed: Hydrate<u8> = Hydrate::Failed(ApiError::Network);
        assert!(failed.ready().is_none());
        assert_eq!(failed.failure(), Some(ApiError::Network));
        assert!(!failed.is_loading());
    }

    #[test]
    fn every_error_has_actionable_text() {
        // A user must always be told something useful, never an empty string.
        for error in [
            ApiError::Environment,
            ApiError::RequestBuild,
            ApiError::Network,
            ApiError::Body,
            ApiError::Status(401),
            ApiError::Status(403),
            ApiError::Status(404),
            ApiError::Status(500),
            ApiError::Status(503),
        ] {
            let message = error.message();
            assert!(!message.is_empty(), "{error:?} has no message");
            assert!(
                message.ends_with('.') || message.ends_with('?'),
                "{error:?} message should read as a sentence: {message}"
            );
        }
    }

    #[test]
    fn unauthorized_tells_the_user_to_sign_in() {
        // 401 is the one status where the remedy is specific and worth naming.
        assert!(ApiError::Status(401).message().contains("sign in"));
        // A generic 5xx must not claim an auth problem.
        assert!(!ApiError::Status(500).message().contains("sign in"));
    }

    #[test]
    fn save_reports_progress_and_failure() {
        assert_eq!(Save::Idle.status(), None);
        assert!(Save::Saving.is_saving());
        assert_eq!(Save::Saving.status(), Some("Saving…"));
        assert_eq!(Save::Saved.status(), Some("Saved"));
        assert_eq!(
            Save::Failed(ApiError::Network).status(),
            Some(ApiError::Network.message())
        );
        assert!(!Save::Failed(ApiError::Network).is_saving());
    }

    #[test]
    fn methods_map_to_http_verbs() {
        assert_eq!(Method::Get.as_str(), "GET");
        assert_eq!(Method::Post.as_str(), "POST");
        assert_eq!(Method::Put.as_str(), "PUT");
        assert_eq!(Method::Delete.as_str(), "DELETE");
    }
}
