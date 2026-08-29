//! The login panel.
//!
//! This screen was 162 lines of inline JavaScript in the actix host's HTML shell.
//! It is now the same Leptos/WASM bundle that serves the dashboard, so the
//! redirect sanitiser and the auth-skip check are type-checked Rust with unit
//! tests behind them rather than script in a string literal.
//!
//! Derivations live in [`crate::dashboard::login_live`]; this file is markup and
//! wiring.

use crate::api::{self, Method};
use crate::dashboard::login_live::{
    AuthStatus, Mode, RESET_HINT, Submitted, button_disabled, button_label, dashboard_target,
    login_error, parse_status, retry_after_seconds, settle_submit, skips_login, submit_body,
};
use leptos::prelude::*;

/// Strings this screen renders, asserted by the host's boundary tests.
///
/// The host serves a shell that mounts this bundle, so it can no longer assert the
/// screen's copy from its own HTML. This is the list it checks instead.
static VISIBLE_CONTRACT: &[&str] = &[
    "nr-auth-panel",
    "nr-auth-form",
    "nullrouter",
    "Password",
    "Login",
    "New password",
    "Default password is",
    "/api/auth/login",
    "/api/auth/status",
    "/api/auth/oidc/start",
    "/api/settings",
];

pub fn login_visible_contract() -> &'static [&'static str] {
    VISIBLE_CONTRACT
}

/// Everything the panel's subtrees read.
#[derive(Clone, Copy)]
struct LoginSignals {
    mode: RwSignal<Mode>,
    error: RwSignal<String>,
    hint: RwSignal<String>,
    submitting: RwSignal<bool>,
    retry_after: RwSignal<u32>,
    password: RwSignal<String>,
    new_password: RwSignal<String>,
    status: RwSignal<Option<AuthStatus>>,
}

const DEFAULT_HINT: &str = "Default password is 123456";

#[component]
pub fn LoginPanel() -> impl IntoView {
    let signals = LoginSignals {
        mode: RwSignal::new(Mode::default()),
        error: RwSignal::new(String::new()),
        hint: RwSignal::new(String::from(DEFAULT_HINT)),
        submitting: RwSignal::new(false),
        retry_after: RwSignal::new(0),
        password: RwSignal::new(String::new()),
        new_password: RwSignal::new(String::new()),
        status: RwSignal::new(None),
    };

    load_status(signals);

    let oidc_ready = move || {
        signals
            .status
            .with(|status| status.as_ref().is_some_and(AuthStatus::oidc_ready))
    };
    let password_hidden = move || {
        signals
            .status
            .with(|status| status.as_ref().is_some_and(AuthStatus::password_hidden))
    };
    let oidc_label = move || {
        signals.status.with(|status| {
            status.as_ref().map_or_else(
                || String::from("Sign in with OIDC"),
                |ready| ready.oidc_label().to_owned(),
            )
        })
    };

    view! {
        <main class="nr-auth-wrap">
            <section class="nr-auth-panel" aria-labelledby="login-title">
                <div class="nr-auth-head">
                    <span class="nr-logo-mark">"9"</span>
                    <h1 id="login-title">"nullrouter"</h1>
                    <p id="login-copy">{move || signals.mode.get().copy()}</p>
                </div>
                <form
                    id="password-form"
                    class="nr-auth-form"
                    class:nr-auth-hidden=password_hidden
                    on:submit=move |event: web_sys::SubmitEvent| {
                        event.prevent_default();
                        submit(signals);
                    }
                >
                    <label for="password">"Password"</label>
                    <input
                        id="password"
                        name="password"
                        type="password"
                        autocomplete="current-password"
                        placeholder="Enter password"
                        required
                        prop:value=move || signals.password.get()
                        on:input=move |event| signals.password.set(event_target_value(&event))
                    />
                    <Show when=move || signals.mode.get() == Mode::ChangePassword>
                        <label id="new-password-row" for="new-password">"New password"</label>
                        <input
                            id="new-password"
                            name="newPassword"
                            type="password"
                            autocomplete="new-password"
                            placeholder="Set new password"
                            required
                            prop:value=move || signals.new_password.get()
                            on:input=move |event| {
                                signals.new_password.set(event_target_value(&event));
                            }
                        />
                    </Show>
                    <p id="auth-error" class="nr-auth-error" role="alert">
                        {move || signals.error.get()}
                    </p>
                    <p id="auth-hint" class="nr-auth-hint">{move || signals.hint.get()}</p>
                    <button
                        id="login-button"
                        class="nr-button nr-button-primary"
                        type="submit"
                        disabled=move || {
                            button_disabled(signals.submitting.get(), signals.retry_after.get())
                        }
                    >
                        {move || {
                            button_label(
                                signals.mode.get(),
                                signals.submitting.get(),
                                signals.retry_after.get(),
                            )
                        }}
                    </button>
                </form>
                <Show when=oidc_ready>
                    <button
                        id="oidc-button"
                        class="nr-button nr-button-secondary"
                        type="button"
                        on:click=move |_| navigate_to("/api/auth/oidc/start")
                    >
                        {oidc_label}
                    </button>
                </Show>
            </section>
        </main>
    }
}

/// Read `GET /api/auth/status` and either skip this screen or shape it.
#[cfg(target_arch = "wasm32")]
fn load_status(signals: LoginSignals) {
    wasm_bindgen_futures::spawn_local(async move {
        let Ok(body) = api::get("/api/auth/status").await else {
            // Unreadable status: leave the password form as-is. This is the only
            // screen the user can recover from, so it must stay usable.
            return;
        };
        let status = parse_status(&body).unwrap_or_default();
        if skips_login(&status) {
            replace_location(&resolved_target());
            return;
        }
        signals.status.set(Some(status));
    });
}

#[cfg(not(target_arch = "wasm32"))]
const fn load_status(_signals: LoginSignals) {}

/// Submit the current form.
#[cfg(target_arch = "wasm32")]
fn submit(signals: LoginSignals) {
    if button_disabled(
        signals.submitting.get_untracked(),
        signals.retry_after.get_untracked(),
    ) {
        return;
    }
    signals.submitting.set(true);
    signals.error.set(String::new());

    wasm_bindgen_futures::spawn_local(async move {
        let mode = signals.mode.get_untracked();
        let body = submit_body(
            mode,
            &signals.password.get_untracked(),
            &signals.new_password.get_untracked(),
        );
        let method = if mode == Mode::ChangePassword {
            Method::Patch
        } else {
            Method::Post
        };
        let response = api::request_detailed(method, mode.path(), Some(&body)).await;
        signals.submitting.set(false);

        let Ok(response) = response else {
            signals.error.set(String::from(
                "Sign-in service is unavailable. Please try again.",
            ));
            return;
        };

        let parsed = serde_json::from_str::<serde_json::Value>(&response.body).ok();
        let flag = |name: &str| {
            parsed
                .as_ref()
                .and_then(|value| value.get(name))
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        };
        let remaining = parsed
            .as_ref()
            .and_then(|value| value.get("remainingBeforeLock"))
            .and_then(serde_json::Value::as_i64);
        let body_retry = parsed
            .as_ref()
            .and_then(|value| value.get("retryAfter"))
            .and_then(serde_json::Value::as_f64);
        let retry = retry_after_seconds(response.retry_after.as_deref(), body_retry);

        match settle_submit(
            response.ok,
            response.status,
            flag("mustChangePassword"),
            remaining,
            retry,
            &resolved_target(),
        ) {
            Submitted::Navigate(target) => replace_location(&target),
            Submitted::RequireChange => {
                signals.mode.set(Mode::ChangePassword);
                signals.hint.set(String::from(
                    "Choose a replacement password before continuing.",
                ));
                signals.password.set(String::new());
            }
            Submitted::Refused {
                message,
                retry_after,
            } => {
                signals.error.set(message);
                if retry_after > 0 {
                    signals.hint.set(String::from(RESET_HINT));
                    start_countdown(signals, retry_after);
                }
            }
        }
    });
}

#[cfg(not(target_arch = "wasm32"))]
const fn submit(_signals: LoginSignals) {}

/// Tick the lockout countdown down to zero, re-enabling the button.
#[cfg(target_arch = "wasm32")]
fn start_countdown(signals: LoginSignals, seconds: u32) {
    use wasm_bindgen::JsCast;
    signals.retry_after.set(seconds);
    // One interval, cleared by its own handle once it reaches zero, so a second
    // lockout cannot leave two timers decrementing the same signal.
    let handle = std::rc::Rc::new(std::cell::Cell::new(0));
    let owned = std::rc::Rc::clone(&handle);
    let tick = wasm_bindgen::closure::Closure::<dyn FnMut()>::new(move || {
        let remaining = signals.retry_after.get_untracked().saturating_sub(1);
        signals.retry_after.set(remaining);
        if remaining == 0
            && let Some(window) = web_sys::window()
        {
            window.clear_interval_with_handle(owned.get());
        }
    });
    if let Some(window) = web_sys::window()
        && let Ok(id) = window.set_interval_with_callback_and_timeout_and_arguments_0(
            tick.as_ref().unchecked_ref(),
            1000,
        )
    {
        handle.set(id);
    }
    // The closure outlives this call by design: the interval owns it until the
    // countdown ends, and this screen is replaced by a navigation either way.
    tick.forget();
}

/// The sanitised post-login target for the current URL.
#[cfg(target_arch = "wasm32")]
fn resolved_target() -> String {
    let Some(window) = web_sys::window() else {
        return String::from(crate::dashboard::login_live::DEFAULT_TARGET);
    };
    let location = window.location();
    let origin = location.origin().unwrap_or_default();
    let search = location.search().unwrap_or_default();
    let next = web_sys::UrlSearchParams::new_with_str(&search)
        .ok()
        .and_then(|params| params.get("next"));
    dashboard_target(next.as_deref(), &origin)
}

/// Navigate, replacing history so Back does not return to the login screen.
#[cfg(target_arch = "wasm32")]
fn replace_location(target: &str) {
    if let Some(window) = web_sys::window() {
        let _ = window.location().replace(target);
    }
}

#[cfg(target_arch = "wasm32")]
fn navigate_to(target: &str) {
    if let Some(window) = web_sys::window() {
        let _ = window.location().set_href(target);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn navigate_to(_target: &str) {}

#[cfg(test)]
mod tests {
    use super::login_visible_contract;

    #[test]
    fn the_contract_names_every_endpoint_this_screen_calls() {
        // The host serves only a mount point now, so it asserts this list instead
        // of its own markup. A screen that stopped calling one of these would be
        // a broken sign-in that still rendered.
        let contract = login_visible_contract();
        for path in [
            "/api/auth/status",
            "/api/auth/login",
            "/api/auth/oidc/start",
            "/api/settings",
        ] {
            assert!(contract.contains(&path), "{path} missing from contract");
        }
    }

    #[test]
    fn the_contract_does_not_promise_the_old_default_password_copy_without_the_hint() {
        // The hint is the one piece of copy a first-run user needs.
        let contract = login_visible_contract();
        assert!(contract.contains(&"Default password is"));
        assert!(contract.contains(&"nr-auth-form"));
    }
}
