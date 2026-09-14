//! Password sign-in. The host serves this page before the dashboard session exists.

use leptos::prelude::*;
use leptos_router::NavigateOptions;
use leptos_router::hooks::use_navigate;

use crate::api::{ApiError, Method, decode, encode, get, request_detailed};
use crate::routes::types::{AuthStatus, LoginBody, LoginDenied, LoginSuccess};

/// Where a successful sign-in lands.
///
/// A constant rather than a `?next=` parameter: that parameter is attacker-controllable, and a
/// sanitiser for it would be a second implementation of a decision this screen does not need to
/// make. Anyone deep-linking to a section reaches it from the dashboard root in one more click.
const AFTER_LOGIN: &str = "/dashboard";

#[component]
pub fn Login() -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let navigate = use_navigate();
    let (password, set_password) = signal(String::new());
    let (error, set_error) = signal(None::<String>);
    let (busy, set_busy) = signal(false);

    // An already-valid session should not be asked for a password again.
    Effect::new({
        let navigate = navigate.clone();
        move |_| {
            let navigate = navigate.clone();
            leptos::task::spawn_local(async move {
                if already_signed_in().await {
                    navigate(AFTER_LOGIN, NavigateOptions::default());
                }
            });
        }
    });

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if busy.get() {
            return;
        }
        set_busy.set(true);
        set_error.set(None);

        let secret = password.get();
        let navigate = navigate.clone();
        leptos::task::spawn_local(async move {
            match sign_in(&secret).await {
                Ok(()) => navigate(AFTER_LOGIN, NavigateOptions::default()),
                Err(message) => set_error.set(Some(message)),
            }
            set_busy.set(false);
        });
    };

    view! {
        <div class="min-h-dvh grid place-items-center bg-background p-6">
            <section class="w-full max-w-sm rounded-lg border border-border bg-card p-6 space-y-4">
                <h1 class="text-xl font-semibold tracking-tight">"nullrouter"</h1>
                <p class="text-sm text-muted-foreground">
                    {locale.get("login.description").to_owned()}
                </p>
                <form class="space-y-3" on:submit=submit>
                    <label class="block space-y-1 text-sm">
                        <span>{locale.get("login.password").to_owned()}</span>
                        <input
                            type="password"
                            autocomplete="current-password"
                            class="w-full rounded-md border border-input bg-background px-3 py-2"
                            prop:value=move || password.get()
                            on:input=move |ev| set_password.set(event_target_value(&ev))
                        />
                    </label>
                    {move || {
                        error
                            .get()
                            .map(|message| {
                                view! {
                                    <p class="text-sm text-destructive" role="alert">
                                        {message}
                                    </p>
                                }
                            })
                    }}
                    <button
                        type="submit"
                        class="w-full rounded-md bg-primary px-3 py-2 text-sm font-medium \
                               text-primary-foreground disabled:opacity-50"
                        disabled=move || busy.get() || password.get().is_empty()
                    >
                        {locale.get("login.submit").to_owned()}
                    </button>
                </form>
            </section>
        </div>
    }
}

/// Whether this browser already holds a valid dashboard session.
///
/// Any failure answers `false`: an unreachable auth service must leave the sign-in form standing,
/// not wave the visitor through.
async fn already_signed_in() -> bool {
    get("/api/auth/status")
        .await
        .ok()
        .and_then(|body| decode::<AuthStatus>(&body).ok())
        .is_some_and(|status| status.authenticated)
}

/// Attempt sign-in, returning the message to show on refusal.
///
/// The refusal itself carries the reason -- a wrong password, a lockout -- so the body is read
/// rather than folded into a bare status, and the server's own wording is preferred over a
/// generic one.
async fn sign_in(password: &str) -> Result<(), String> {
    let body = encode(&LoginBody { password }).map_err(|error| error.message().to_owned())?;

    let response = request_detailed(Method::Post, "/api/auth/login", Some(&body))
        .await
        .map_err(|error| error.message().to_owned())?;

    if response.ok {
        // Decoded to confirm the shape is the one this screen expects; the flag itself carries no
        // decision here, because a password change is not implemented as a separate screen.
        let _ = decode::<LoginSuccess>(&response.body);
        return Ok(());
    }

    Err(decode::<LoginDenied>(&response.body)
        .ok()
        .map(|denied| denied.error)
        .filter(|message| !message.is_empty())
        .unwrap_or_else(|| ApiError::Status(response.status).message().to_owned()))
}
