//! HTTP access and the loading states every panel is built from.
//!
//! The rule this module exists to enforce: a panel is either loading, holding data the server
//! actually sent, or explaining why it has none. There is deliberately no state in which invented
//! data can reach the screen, because the failure that produces -- a toggle that silently discards
//! writes, a list of accounts that do not exist -- is invisible until someone relies on it.
//!
//! [`Hydrate`] covers reads and [`Save`] covers writes. They are separate because a write must not
//! replace what the panel is showing: the row stays visible while saving, and a failure has to be
//! recoverable without losing the rest of the page.
//!
//! Only the calls needing a `Window` are wasm-gated. The native counterparts report the absence
//! rather than faking a response, which keeps parsing and state logic unit-testable off-browser.

use leptos::prelude::*;

pub mod sse;

/// Why a request produced no usable data.
///
/// Coarse on purpose. A panel needs to tell someone what to do next, not surface transport
/// internals they cannot act on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApiError {
    /// No browser environment was available.
    Environment,
    /// The request could not be constructed.
    RequestBuild,
    /// The request never reached the server.
    Network,
    /// The server answered, with a non-2xx status.
    Status(u16),
    /// The body could not be read or did not parse.
    Body,
}

impl ApiError {
    /// A short, actionable sentence for the user.
    pub const fn message(self) -> &'static str {
        match self {
            Self::Environment => "Browser environment unavailable.",
            Self::RequestBuild => "Could not build the request.",
            Self::Network => "Could not reach the router. Is the service running?",
            Self::Status(401) => "Not signed in. Reload and sign in again.",
            Self::Status(403) => "This action is not permitted.",
            Self::Status(404) => "That endpoint is not available on this build.",
            Self::Status(409) => "That conflicts with the current state. Reload and retry.",
            Self::Status(429) => "Too many requests. Wait a moment and retry.",
            Self::Status(503) => "The service is temporarily unavailable.",
            Self::Status(_) => "The router returned an error.",
            Self::Body => "The response could not be read.",
        }
    }

    /// Whether retrying the same request could plausibly succeed.
    ///
    /// Drives whether a failed panel offers a retry button. A 404 or a 403 will not change on its
    /// own, and offering to retry one is a false promise.
    pub const fn is_retryable(self) -> bool {
        match self {
            Self::Network | Self::Body | Self::Status(429 | 503) => true,
            Self::Environment | Self::RequestBuild | Self::Status(_) => false,
        }
    }

    /// Whether this means the session is gone and the user must sign in again.
    pub const fn is_unauthenticated(self) -> bool {
        matches!(self, Self::Status(401))
    }
}

/// HTTP verb.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl Method {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }
}

/// The state of one piece of server-owned data.
///
/// Panels match this exhaustively, so "failed" and "empty" always render as themselves instead of
/// collapsing into a placeholder that reads as "nothing here".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Hydrate<T> {
    #[default]
    Loading,
    Ready(T),
    Failed(ApiError),
}

impl<T> Hydrate<T> {
    pub const fn ready(&self) -> Option<&T> {
        match self {
            Self::Ready(value) => Some(value),
            Self::Loading | Self::Failed(_) => None,
        }
    }

    pub const fn is_loading(&self) -> bool {
        matches!(self, Self::Loading)
    }

    pub const fn failure(&self) -> Option<ApiError> {
        match self {
            Self::Failed(error) => Some(*error),
            Self::Loading | Self::Ready(_) => None,
        }
    }

    /// Apply a function to the held value.
    pub fn map<U, F: FnOnce(T) -> U>(self, transform: F) -> Hydrate<U> {
        match self {
            Self::Ready(value) => Hydrate::Ready(transform(value)),
            Self::Loading => Hydrate::Loading,
            Self::Failed(error) => Hydrate::Failed(error),
        }
    }
}

/// Whether a write is in flight, and how the last one ended.
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

    pub const fn failure(&self) -> Option<ApiError> {
        match self {
            Self::Failed(error) => Some(*error),
            Self::Idle | Self::Saving | Self::Saved => None,
        }
    }
}

/// A response whose status, `Retry-After`, and body are all still readable.
///
/// [`request`] folds a non-2xx into [`ApiError::Status`] and drops the body, which is what most
/// panels want. Sign-in is the exception: the refusal itself carries the remaining-attempts count
/// and the must-change-password flag, and a lockout's countdown is in the `Retry-After` header.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DetailedResponse {
    pub status: u16,
    pub ok: bool,
    pub retry_after: Option<String>,
    /// The body, empty when it could not be read.
    pub body: String,
}

#[cfg(target_arch = "wasm32")]
fn build_request(
    method: Method,
    path: &str,
    body: Option<&str>,
) -> Result<web_sys::Request, ApiError> {
    use web_sys::{Request, RequestCache, RequestCredentials, RequestInit};

    let init = RequestInit::new();
    init.set_method(method.as_str());
    // Same-origin so the dashboard session cookie reaches session-gated routes.
    init.set_credentials(RequestCredentials::SameOrigin);
    // No-store so a panel never renders a snapshot the router has already moved past.
    init.set_cache(RequestCache::NoStore);
    if let Some(payload) = body {
        init.set_body(&wasm_bindgen::JsValue::from_str(payload));
    }

    let request = Request::new_with_str_and_init(path, &init).map_err(|_| ApiError::RequestBuild)?;
    if body.is_some() {
        request
            .headers()
            .set("content-type", "application/json")
            .map_err(|_| ApiError::RequestBuild)?;
    }
    Ok(request)
}

/// Send a request, returning the body on success.
#[cfg(target_arch = "wasm32")]
pub async fn request(method: Method, path: &str, body: Option<&str>) -> Result<String, ApiError> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::Response;

    let request = build_request(method, path, body)?;
    let window = web_sys::window().ok_or(ApiError::Environment)?;
    let response = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|_| ApiError::Network)?
        .dyn_into::<Response>()
        .map_err(|_| ApiError::Body)?;

    if !response.ok() {
        return Err(ApiError::Status(response.status()));
    }
    JsFuture::from(response.text().map_err(|_| ApiError::Body)?)
        .await
        .map_err(|_| ApiError::Body)?
        .as_string()
        .ok_or(ApiError::Body)
}

/// Native builds have no browser to fetch from. Any call here is a programming error, reported
/// rather than faked.
#[cfg(not(target_arch = "wasm32"))]
#[expect(clippy::unused_async, reason = "mirrors the wasm signature so callers stay target-agnostic")]
pub async fn request(
    _method: Method,
    _path: &str,
    _body: Option<&str>,
) -> Result<String, ApiError> {
    Err(ApiError::Environment)
}

/// Send a request, reporting status and headers without folding a refusal into an error.
#[cfg(target_arch = "wasm32")]
pub async fn request_detailed(
    method: Method,
    path: &str,
    body: Option<&str>,
) -> Result<DetailedResponse, ApiError> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::Response;

    let request = build_request(method, path, body)?;
    let window = web_sys::window().ok_or(ApiError::Environment)?;
    let response = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|_| ApiError::Network)?
        .dyn_into::<Response>()
        .map_err(|_| ApiError::Body)?;

    // An unreadable body is not fatal: the status alone still yields a message, so the caller
    // reports the refusal rather than a transport error it did not have.
    let text = match response.text() {
        Ok(promise) => JsFuture::from(promise)
            .await
            .ok()
            .and_then(|value| value.as_string())
            .unwrap_or_default(),
        Err(_) => String::new(),
    };

    Ok(DetailedResponse {
        status: response.status(),
        ok: response.ok(),
        retry_after: response.headers().get("Retry-After").ok().flatten(),
        body: text,
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[expect(clippy::unused_async, reason = "mirrors the wasm signature so callers stay target-agnostic")]
pub async fn request_detailed(
    _method: Method,
    _path: &str,
    _body: Option<&str>,
) -> Result<DetailedResponse, ApiError> {
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

/// `PATCH` a JSON body.
pub async fn patch(path: &str, body: &str) -> Result<String, ApiError> {
    request(Method::Patch, path, Some(body)).await
}

/// `DELETE` a path.
pub async fn delete(path: &str) -> Result<String, ApiError> {
    request(Method::Delete, path, None).await
}

/// Deserialize a response body.
///
/// A shape change upstream becomes [`ApiError::Body`], so it surfaces as a visible failure rather
/// than an empty panel that reads as "no data".
pub fn decode<T: serde::de::DeserializeOwned>(body: &str) -> Result<T, ApiError> {
    serde_json::from_str(body).map_err(|_| ApiError::Body)
}

/// Serialize a request body.
pub fn encode<T: serde::Serialize>(value: &T) -> Result<String, ApiError> {
    serde_json::to_string(value).map_err(|_| ApiError::Body)
}

/// Fetch, decode, and drive a [`Hydrate`] signal.
pub fn load<T>(path: impl Into<String>, into: WriteSignal<Hydrate<T>>)
where
    T: serde::de::DeserializeOwned + Send + Sync + 'static,
{
    let path = path.into();
    spawn(async move {
        let next = match get(&path).await {
            Ok(body) => decode(&body).map_or_else(Hydrate::Failed, Hydrate::Ready),
            Err(error) => Hydrate::Failed(error),
        };
        into.set(next);
    });
}

/// Fetch and drive a [`Hydrate`] signal through a custom parse.
///
/// For the endpoints whose useful shape is not their wire shape -- a list that needs sorting, a
/// map that the UI wants as ordered rows.
pub fn load_with<T, F>(path: impl Into<String>, into: WriteSignal<Hydrate<T>>, parse: F)
where
    T: Send + Sync + 'static,
    F: Fn(&str) -> Result<T, ApiError> + 'static,
{
    let path = path.into();
    spawn(async move {
        let next = match get(&path).await {
            Ok(body) => parse(&body).map_or_else(Hydrate::Failed, Hydrate::Ready),
            Err(error) => Hydrate::Failed(error),
        };
        into.set(next);
    });
}

/// Run a write, driving a [`Save`] signal through its lifecycle.
///
/// `on_success` runs only when the write succeeded, which is where a panel refetches or applies the
/// change locally. It does not run on failure, so a rejected write cannot leave the UI claiming it
/// was applied.
pub fn submit<F, Fut, S>(state: WriteSignal<Save>, send: F, on_success: S)
where
    F: FnOnce() -> Fut + 'static,
    Fut: std::future::Future<Output = Result<String, ApiError>> + 'static,
    S: FnOnce(String) + 'static,
{
    state.set(Save::Saving);
    spawn(async move {
        match send().await {
            Ok(body) => {
                state.set(Save::Saved);
                on_success(body);
            }
            Err(error) => state.set(Save::Failed(error)),
        }
    });
}

/// Spawn a future on the browser's task queue.
#[cfg(target_arch = "wasm32")]
fn spawn<F: std::future::Future<Output = ()> + 'static>(future: F) {
    wasm_bindgen_futures::spawn_local(future);
}

/// Native builds have no task queue to spawn onto, and no browser for the future to talk to. The
/// future is dropped unpolled rather than blocking a test thread on work that cannot complete.
#[cfg(not(target_arch = "wasm32"))]
fn spawn<F: std::future::Future<Output = ()> + 'static>(future: F) {
    drop(future);
}

#[cfg(test)]
mod tests {
    use super::{ApiError, Hydrate, Method, Save, decode, encode};

    #[test]
    fn hydrate_starts_loading_and_holds_nothing() {
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
    fn hydrate_map_preserves_the_non_ready_states() {
        assert_eq!(Hydrate::Ready(2_u8).map(|value| value * 2), Hydrate::Ready(4_u8));
        assert_eq!(Hydrate::<u8>::Loading.map(|value| value * 2), Hydrate::Loading);
        let failed = Hydrate::<u8>::Failed(ApiError::Body);
        assert_eq!(failed.map(|value| value * 2), Hydrate::Failed(ApiError::Body));
    }

    #[test]
    fn every_error_reads_as_an_actionable_sentence() {
        for error in [
            ApiError::Environment,
            ApiError::RequestBuild,
            ApiError::Network,
            ApiError::Body,
            ApiError::Status(401),
            ApiError::Status(403),
            ApiError::Status(404),
            ApiError::Status(409),
            ApiError::Status(429),
            ApiError::Status(500),
            ApiError::Status(503),
        ] {
            let message = error.message();
            assert!(!message.is_empty(), "{error:?} has no message");
            assert!(
                message.ends_with('.') || message.ends_with('?'),
                "{error:?} should read as a sentence: {message}"
            );
        }
    }

    #[test]
    fn unauthorized_is_the_only_status_that_asks_for_a_sign_in() {
        assert!(ApiError::Status(401).is_unauthenticated());
        assert!(ApiError::Status(401).message().contains("sign in"));
        for status in [403, 404, 409, 429, 500, 503] {
            assert!(!ApiError::Status(status).is_unauthenticated(), "{status}");
            assert!(
                !ApiError::Status(status).message().contains("sign in"),
                "{status} must not claim an auth problem"
            );
        }
    }

    #[test]
    fn only_transient_failures_offer_a_retry() {
        // Offering to retry a 404 or a 403 is a false promise: neither changes on its own.
        for error in [ApiError::Network, ApiError::Body, ApiError::Status(429), ApiError::Status(503)]
        {
            assert!(error.is_retryable(), "{error:?}");
        }
        for error in [
            ApiError::Environment,
            ApiError::RequestBuild,
            ApiError::Status(401),
            ApiError::Status(403),
            ApiError::Status(404),
            ApiError::Status(500),
        ] {
            assert!(!error.is_retryable(), "{error:?}");
        }
    }

    #[test]
    fn save_reports_progress_and_recoverable_failure() {
        assert!(!Save::Idle.is_saving());
        assert!(Save::Saving.is_saving());
        assert!(Save::Saved.failure().is_none());
        assert_eq!(Save::Failed(ApiError::Network).failure(), Some(ApiError::Network));
        assert!(!Save::Failed(ApiError::Network).is_saving());
    }

    #[test]
    fn methods_map_to_verbs_and_know_which_carry_a_body() {
        assert_eq!(Method::Get.as_str(), "GET");
        assert_eq!(Method::Post.as_str(), "POST");
        assert_eq!(Method::Put.as_str(), "PUT");
        assert_eq!(Method::Patch.as_str(), "PATCH");
        assert_eq!(Method::Delete.as_str(), "DELETE");
    }

    #[test]
    fn a_shape_change_upstream_becomes_a_body_error() {
        // Not a panic, and not a default value that would render as real data.
        assert_eq!(decode::<Vec<u8>>("{\"not\":\"an array\"}").unwrap_err(), ApiError::Body);
        assert_eq!(decode::<Vec<u8>>("truncated").unwrap_err(), ApiError::Body);
        assert_eq!(decode::<Vec<u8>>("[1,2,3]").unwrap_or_default(), vec![1, 2, 3]);
    }

    #[test]
    fn bodies_round_trip() {
        let encoded = encode(&vec![1_u8, 2, 3]).unwrap_or_default();
        assert_eq!(encoded, "[1,2,3]");
        assert_eq!(decode::<Vec<u8>>(&encoded).unwrap_or_default(), vec![1, 2, 3]);
    }
}
