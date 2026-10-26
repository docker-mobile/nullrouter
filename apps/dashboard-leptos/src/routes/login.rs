//! Password sign-in. The host serves this page before the dashboard session exists.
//!
//! # Why this page is centred and narrow
//!
//! It is the only page in the dashboard with exactly one job. A split-screen layout with product copy
//! beside the form would be more interesting to look at and worse to use: the operator is here to type a
//! password, and anything else on screen is in the way of that.

use leptos::prelude::*;

use crate::t;
use leptos_router::NavigateOptions;
use leptos_router::hooks::use_navigate;

use crate::api::{ApiError, Method, decode, encode, get, request_detailed};
use crate::routes::types::{AuthStatus, LoginBody, LoginDenied, LoginSuccess};
use crate::ui::{Button, ButtonSize, Card, CardContent, ErrorNote, Field, Input};

/// Where a successful sign-in lands.
///
/// A constant rather than a `?next=` parameter: that parameter is attacker-controllable, and a sanitiser
/// for it would be a second implementation of a decision this screen does not need to make. Anyone
/// deep-linking to a section reaches it from the dashboard root in one more click.
const AFTER_LOGIN: &str = "/dashboard";

#[allow(
    clippy::too_many_lines,
    reason = "the sign-in screen is one form; splitting it would spread its state"
)]
#[component]
pub fn Login() -> impl IntoView {
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

    let submit = {
        let navigate = navigate.clone();
        move || {
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
        }
    };

    let headline = t!("login.description");
    let password_label = t!("login.password");
    // The card heading and the field's label are the same word but two separate `String`s: both are
    // moved into the tree, and one binding cannot be used twice.
    let field_label = password_label.clone();
    let submit_label = t!("login.submit");

    view! {
        <div class="grid min-h-dvh place-items-center bg-muted/30 px-4 py-10">
            <div class="w-full max-w-[22rem] space-y-5">
                <div class="flex flex-col items-center gap-2 text-center">
                    <div class="grid size-10 place-items-center rounded-xl bg-primary">
                        <svg
                            class="size-6 text-primary-foreground"
                            viewBox="0 0 32 32"
                            fill="none"
                            aria-hidden="true"
                        >
                            <g stroke="currentColor" stroke-width="2.6" stroke-linecap="round">
                                <path d="M5 9h6l4 4" />
                                <path d="M5 16h6" />
                                <path d="M5 23h6l4-4" />
                                <path d="M20 16h7" />
                            </g>
                            <circle cx="16" cy="16" r="3.4" fill="currentColor" />
                            <path
                                d="M12.4 19.6 19.6 12.4"
                                stroke="var(--color-primary, currentColor)"
                                stroke-width="2.6"
                                stroke-linecap="round"
                                class="text-primary"
                            />
                        </svg>
                    </div>
                    <p class="text-base font-semibold tracking-tight">"nullrouter"</p>
                    <p class="text-sm leading-relaxed text-muted-foreground">{headline}</p>
                </div>

                <Card>
                    <CardContent class="px-5 py-5">
                        <form
                            class="space-y-3"
                            on:submit=move |event| {
                                event.prevent_default();
                                submit();
                            }
                        >
                            // One heading for the card, and the field's own label beneath it. A second
                            // `<h1>` saying "Password" above a field labelled "Password" read as a
                            // duplicate rather than as structure.
                            <h1 class="text-sm font-medium">{password_label}</h1>
                            <Field label=field_label>
                                <Input
                                    input_type="password"
                                    autocomplete="current-password"
                                    value=Signal::derive(move || password.get())
                                    on_input=Callback::new(move |next| set_password.set(next))
                                />
                            </Field>

                            {move || {
                                error
                                    .get()
                                    .map(|message| view! { <ErrorNote message=message /> })
                            }}

                            <Button
                                label=submit_label.clone()
                                button_type="submit"
                                size=ButtonSize::Default
                                class="w-full"
                                disabled=Signal::derive(move || password.get().is_empty())
                                busy=Signal::derive(move || busy.get())
                            />
                        </form>
                    </CardContent>
                </Card>

                <p class="text-center text-xs leading-relaxed text-muted-foreground">
                    {t!("login.footer")}
                </p>
            </div>
        </div>
    }
}

/// Whether this browser already holds a valid dashboard session.
///
/// Any failure answers `false`: an unreachable auth service must leave the sign-in form standing, not
/// wave the visitor through.
async fn already_signed_in() -> bool {
    get("/api/auth/status")
        .await
        .ok()
        .and_then(|body| decode::<AuthStatus>(&body).ok())
        .is_some_and(|status| status.authenticated)
}

/// Attempt sign-in, returning the message to show on refusal.
///
/// The refusal itself carries the reason -- a wrong password, a lockout -- so the body is read rather
/// than folded into a bare status, and the server's own wording is preferred over a generic one.
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
