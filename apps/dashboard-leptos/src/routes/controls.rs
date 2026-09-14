//! The button that runs one server action and reports what the server said.
//!
//! The three lifecycle panels — CLI tools, token saver, headroom — all drive endpoints whose
//! refusals carry the reason in the body: `baseUrl, apiKey and model are required`, `PXPIPE is not
//! installed, and automatic installation is turned off`, `EXTERNAL_PROXY`. [`crate::api::request`]
//! folds a non-2xx into a bare [`crate::api::ApiError::Status`] and drops that body, which would
//! turn every one of them into "The router returned an error." So these go through
//! [`request_detailed`] and show the server's own wording, the way sign-in already does.
//!
//! A refusal is rendered as a refusal. There is deliberately no path here that reports a 4xx or a
//! 5xx as success, because the actions behind these buttons write config files and spawn processes:
//! a user who is told "applied" when nothing was applied will go on to rely on it.

use leptos::prelude::*;

use crate::api::{ApiError, Method, Save, decode, request_detailed};

/// The fields a refusal or a completion can carry, across all three panels' endpoints.
///
/// One struct rather than one per endpoint: the servers agree on `error`/`message` and disagree
/// only on which they populate, so reading both and preferring whichever is present covers every
/// shape without a decode that can fail on an unexpected combination.
#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActionReply {
    // `success` is deliberately not decoded. Every endpoint these panels call pairs
    // `"success": false` with a non-OK status, so the flag carries nothing the status has not
    // already said, and reading it would invite deciding `ok` from the body — which is what the
    // note above rules out. If an endpoint ever returns 200 with `success: false`, add it back as
    // `Option<bool>` so an absent field stays distinguishable from an explicit false.
    /// Set by every refusal in these three modules.
    #[serde(default)]
    error: String,
    /// Set by the completions, and by some refusals instead of `error`.
    #[serde(default)]
    message: String,
    /// Machine-readable cause: `NOT_INSTALLED`, `NPM_MISSING`, `EXTERNAL_PROXY`, `PIP_FAILED`, …
    #[serde(default)]
    code: String,
    /// `true` on the routes this build does not implement, so the panel can say so rather than
    /// reporting a generic failure.
    #[serde(default)]
    unsupported: bool,
}

/// How one action ended, in the words the server used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub ok: bool,
    pub detail: String,
}

/// Run one action and describe the result.
///
/// The HTTP status decides `ok`, never the body: a `{"success": true}` inside a 500 is not a
/// success, and a body that fails to decode inside a 200 is still a completed action.
async fn run(method: Method, path: &str, body: Option<&str>, fallback: &str) -> Outcome {
    match request_detailed(method, path, body).await {
        Ok(response) => {
            let reply = decode::<ActionReply>(&response.body).unwrap_or_default();
            let detail = first_non_empty(&[&reply.message, &reply.error, &reply.code]);
            Outcome {
                ok: response.ok,
                detail: if detail.is_empty() {
                    if response.ok {
                        fallback.to_owned()
                    } else {
                        ApiError::Status(response.status).message().to_owned()
                    }
                } else if reply.unsupported {
                    format!("{detail} (not supported by this build)")
                } else {
                    detail
                },
            }
        }
        Err(error) => Outcome {
            ok: false,
            detail: error.message().to_owned(),
        },
    }
}

fn first_non_empty(candidates: &[&str]) -> String {
    candidates
        .iter()
        .find(|text| !text.trim().is_empty())
        .map_or_else(String::new, |text| (*text).to_owned())
}

/// Visual weight, which is the only thing that separates a start from a stop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Tone {
    /// The action the panel expects to be used.
    Primary,
    /// Reversible, but not the default choice.
    Neutral,
    /// Removes something, or costs real time and disk.
    Destructive,
}

impl Tone {
    const fn classes(self) -> &'static str {
        match self {
            Self::Primary => "bg-primary text-primary-foreground hover:bg-primary/90",
            Self::Neutral => {
                "border border-border bg-background text-foreground hover:bg-accent \
                 hover:text-accent-foreground"
            }
            Self::Destructive => {
                "border border-destructive/40 bg-destructive/5 text-destructive \
                 hover:bg-destructive/10"
            }
        }
    }
}

/// A button that sends one request and hands the outcome back to the panel.
///
/// `body` is called at click time rather than being passed as a value, so a form's *current*
/// contents are what get posted. Returning `None` from it cancels the send, which is how a panel
/// refuses to post a payload it could not build without also having to disable the button on every
/// keystroke.
///
/// A [`Callback`] rather than a generic closure parameter: the prop is optional, and a generic that
/// appears only in an omitted argument has nothing to infer itself from, so every caller that left
/// `body` off would need a turbofish naming a closure type it does not have.
#[component]
pub fn Action(
    label: String,
    path: String,
    /// Called with the outcome. Refetching lives here, so a failed action cannot leave the panel
    /// claiming the new state.
    on_done: Callback<Outcome>,
    #[prop(default = Method::Post)] method: Method,
    #[prop(default = Tone::Neutral)] tone: Tone,
    #[prop(optional)] body: Option<Callback<(), Option<String>>>,
    /// Shown when the server completes without saying anything.
    #[prop(optional, into)]
    done_label: Option<String>,
    /// Held down by the panel while the action cannot apply — an unwritable tool, a proxy this
    /// machine does not host.
    #[prop(optional, into)]
    disabled: Option<Signal<bool>>,
) -> impl IntoView {
    let (save, set_save) = signal(Save::Idle);
    let fallback = done_label.unwrap_or_else(|| "Done.".to_owned());

    view! {
        <button
            type="button"
            class=move || {
                format!(
                    "{} rounded-md px-3 py-2 text-sm font-medium transition-colors \
                     disabled:opacity-50 disabled:pointer-events-none",
                    tone.classes(),
                )
            }
            disabled=move || {
                save.get().is_saving() || disabled.is_some_and(|held| held.get())
            }
            on:click=move |_| {
                if save.get().is_saving() {
                    return;
                }
                // Read the form now: a body captured at render time would post whatever the fields
                // held when the button was drawn.
                let payload = match body {
                    Some(build) => match build.run(()) {
                        Some(payload) => Some(payload),
                        // The panel could not build a body it was willing to send.
                        None => return,
                    },
                    None => None,
                };
                let path = path.clone();
                let fallback = fallback.clone();
                set_save.set(Save::Saving);
                leptos::task::spawn_local(async move {
                    let outcome = run(method, &path, payload.as_deref(), &fallback).await;
                    set_save
                        .set(
                            if outcome.ok {
                                Save::Saved
                            } else {
                                Save::Failed(ApiError::Body)
                            },
                        );
                    on_done.run(outcome);
                });
            }
        >
            {move || {
                if save.get().is_saving() {
                    format!("{label}…")
                } else {
                    label.clone()
                }
            }}
        </button>
    }
}

/// The last outcome, rendered as itself.
#[component]
pub fn OutcomeLine(outcome: ReadSignal<Option<Outcome>>) -> impl IntoView {
    view! {
        {move || {
            outcome
                .get()
                .map(|outcome| {
                    let class = if outcome.ok {
                        "text-sm text-foreground whitespace-pre-wrap break-words"
                    } else {
                        "text-sm text-destructive whitespace-pre-wrap break-words"
                    };
                    view! {
                        <p class=class role="status">
                            {outcome.detail}
                        </p>
                    }
                })
        }}
    }
}

/// A labelled value in a definition list, with an em dash for "the server did not say".
#[component]
pub fn Field(label: String, value: String) -> impl IntoView {
    let shown = if value.trim().is_empty() {
        "—".to_owned()
    } else {
        value
    };
    view! {
        <div class="flex items-start justify-between gap-4">
            <dt class="text-muted-foreground shrink-0">{label}</dt>
            <dd class="text-right break-all">{shown}</dd>
        </div>
    }
}

/// A boolean the server reported, as a dot and a word.
///
/// The dot colour is not a judgement about the value: `false` is muted rather than red, because a
/// token saver that is off is a configuration, not a fault.
#[component]
pub fn Flag(label: String, on: bool) -> impl IntoView {
    let locale = crate::i18n::use_locale();
    let state = if on {
        locale.get("state.enabled").to_owned()
    } else {
        locale.get("state.disabled").to_owned()
    };
    view! {
        <div class="flex items-center justify-between gap-4">
            <dt class="text-muted-foreground truncate">{label}</dt>
            <dd class="flex items-center gap-2 shrink-0">
                <span class=if on {
                    "size-1.5 rounded-full bg-success"
                } else {
                    "size-1.5 rounded-full bg-muted-foreground/40"
                } />
                <span>{state}</span>
            </dd>
        </div>
    }
}

/// A card with a heading, matching the one `overview` uses.
#[component]
pub fn Section(title: String, children: Children) -> impl IntoView {
    view! {
        <section class="rounded-lg border border-border bg-card p-5 space-y-4">
            <h2 class="text-sm font-medium text-muted-foreground">{title}</h2>
            {children()}
        </section>
    }
}

/// A warning about something that will take real time or real disk if pressed.
#[component]
pub fn Caution(text: String) -> impl IntoView {
    view! {
        <div
            class="rounded-md border border-warning/40 bg-warning/5 px-3 py-2 text-sm text-foreground"
            role="note"
        >
            {text}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::{ActionReply, Tone, first_non_empty};

    #[test]
    fn the_first_populated_field_is_the_one_shown() {
        assert_eq!(first_non_empty(&["", "second", "third"]), "second");
        assert_eq!(first_non_empty(&["  ", "", "third"]), "third");
        assert_eq!(first_non_empty(&["", "", ""]), "");
    }

    #[test]
    fn a_refusal_body_decodes_to_its_reason() {
        // The exact body `POST /api/cli-tools/codex` returns for an incomplete payload.
        let reply: ActionReply =
            serde_json::from_str(r#"{"error":"baseUrl, apiKey and model are required"}"#)
                .unwrap_or_default();
        assert_eq!(reply.error, "baseUrl, apiKey and model are required");
        assert!(reply.message.is_empty());
    }

    #[test]
    fn an_unsupported_route_is_recognised_as_one() {
        // `POST /api/cli-tools/devin`, which has no mutation upstream either.
        let reply: ActionReply = serde_json::from_str(
            r#"{"success":false,"unsupported":true,
                "message":"CLI tool configuration is not supported by nullrouter-api"}"#,
        )
        .unwrap_or_default();
        assert!(reply.unsupported);
        assert!(reply.message.contains("not supported"));
    }

    #[test]
    fn a_coded_refusal_keeps_its_code_as_a_last_resort() {
        // `POST /api/pxpipe/start` with the runtime down.
        let reply: ActionReply = serde_json::from_str(
            r#"{"success":false,"code":"RUNTIME_UNREACHABLE",
                "error":"The runtime service holds the transform and could not be reached"}"#,
        )
        .unwrap_or_default();
        assert_eq!(
            first_non_empty(&[&reply.message, &reply.error, &reply.code]),
            "The runtime service holds the transform and could not be reached"
        );
    }

    #[test]
    fn an_unreadable_body_yields_no_detail_rather_than_a_wrong_one() {
        // `unwrap_or_default` on a failed decode must leave every field empty, so `first_non_empty`
        // finds nothing and `run` falls back to the HTTP status for its wording. A field that
        // defaulted to something non-empty would put invented text in front of the user.
        let reply: super::ActionReply = serde_json::from_str("not json").unwrap_or_default();
        assert!(reply.error.is_empty());
        assert!(reply.message.is_empty());
        assert!(reply.code.is_empty());
        assert!(!reply.unsupported);
        assert!(first_non_empty(&[&reply.message, &reply.error, &reply.code]).is_empty());
    }

    #[test]
    fn every_tone_carries_classes() {
        for tone in [Tone::Primary, Tone::Neutral, Tone::Destructive] {
            assert!(!tone.classes().is_empty(), "{tone:?}");
        }
    }
}
