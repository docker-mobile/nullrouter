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
use crate::routes::{
    Alert, AlertVariant, Badge, BadgeVariant, ButtonSize, ButtonVariant, Card, CardContent,
    CardHeader, CardTitle, ErrorNote, Icon, IconName, StatusDot, StatusTone, button_class,
};
use crate::t;

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
    /// The button weight this tone maps onto.
    ///
    /// Presentation lives in the shared button layer now, so the three tones resolve to three of its
    /// variants rather than to three hand-written class lists here. A stop and a start still differ by
    /// weight alone, which is the only thing [`Tone`] ever meant.
    const fn variant(self) -> ButtonVariant {
        match self {
            Self::Primary => ButtonVariant::Default,
            Self::Neutral => ButtonVariant::Outline,
            Self::Destructive => ButtonVariant::Destructive,
        }
    }

    /// The full class list for a button of this weight.
    fn classes(self) -> String {
        button_class(self.variant(), ButtonSize::Default)
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
            class=tone.classes()
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
///
/// A refusal is an [`ErrorNote`] -- `role="alert"`, an icon, the server's own sentence -- because the
/// endpoints behind these buttons answer 4xx with the reason, and that wording is the only thing that
/// tells the user which field or which package the server wanted. A completion is the quieter line: it
/// is a confirmation, not a warning.
#[component]
pub fn OutcomeLine(outcome: ReadSignal<Option<Outcome>>) -> impl IntoView {
    view! {
        {move || {
            outcome
                .get()
                .map(|outcome| {
                    if outcome.ok {
                        view! {
                            <p
                                class="flex items-start gap-2 text-[0.8125rem] leading-relaxed text-foreground"
                                role="status"
                            >
                                <Icon name=IconName::Check class="mt-0.5 size-3.5 shrink-0 text-success" />
                                <span class="min-w-0 whitespace-pre-wrap break-words">
                                    {outcome.detail}
                                </span>
                            </p>
                        }
                            .into_any()
                    } else {
                        view! { <ErrorNote message=outcome.detail /> }.into_any()
                    }
                })
        }}
    }
}

/// A labelled value in a definition list, with an em dash for "the server did not say".
#[component]
pub fn Field(#[prop(into)] label: String, #[prop(into)] value: String) -> impl IntoView {
    // `or_dash` takes `&str`; the owned value is dropped here rather than threaded through.
    let shown = crate::routes::or_dash(&value);
    drop(value);
    view! {
        <div class="flex items-start justify-between gap-4 py-1">
            <dt class="shrink-0 text-muted-foreground">{label}</dt>
            <dd class="min-w-0 text-right font-mono text-xs break-all text-foreground">{shown}</dd>
        </div>
    }
}

/// A boolean the server reported, as a dot and a word.
///
/// The dot colour is not a judgement about the value: `false` is muted rather than red, because a
/// token saver that is off is a configuration, not a fault.
#[component]
pub fn Flag(label: String, on: bool) -> impl IntoView {
    let state = if on {
        t!("state.enabled")
    } else {
        t!("state.disabled")
    };
    view! {
        <div class="flex items-center justify-between gap-4 py-1">
            <dt class="truncate text-muted-foreground">{label}</dt>
            <dd class="shrink-0">
                <StatusDot
                    label=state
                    tone=if on { StatusTone::Success } else { StatusTone::Muted }
                />
            </dd>
        </div>
    }
}

/// A named flag and its value, as one badge.
///
/// The label alone ("Installed") is ambiguous once colour is the only thing that changes it, and a
/// badge whose text does not say which state it is in is a coloured rectangle. So the badge spells
/// both: the name the server used and the word for the value.
///
/// `off_variant` exists because "off" is not always neutral -- a proxy that is not answering is a
/// warning, while a token saver that is not loaded is a configuration.
#[component]
// Owned, not `&str`: the label is a translated message, so it is a `String` at every call site and a
// borrow would have to be kept alive for the render.
#[allow(clippy::needless_pass_by_value)]
pub fn StateBadge(
    /// The name the server used for the state's subject.
    #[prop(into)]
    label: String,
    on: bool,
    #[prop(optional, into)] off_variant: Option<BadgeVariant>,
) -> impl IntoView {
    let state = if on {
        t!("state.enabled")
    } else {
        t!("state.disabled")
    };
    let variant = if on {
        BadgeVariant::Success
    } else {
        off_variant.unwrap_or(BadgeVariant::Secondary)
    };
    view! { <Badge variant=variant>{format!("{label} · {state}")}</Badge> }
}

/// A card with a heading, matching the one `overview` uses.
///
/// `Card` plus `CardHeader`/`CardContent` rather than a hand-written `<section>`: the border, radius,
/// shadow and inset are the same ones every other panel draws, and a panel that spelled them out
/// itself would drift the moment the shared surface changed.
#[component]
pub fn Section(title: String, children: Children) -> impl IntoView {
    view! {
        <Card>
            <CardHeader>
                <CardTitle>{title}</CardTitle>
            </CardHeader>
            <CardContent>{children()}</CardContent>
        </Card>
    }
}

/// A warning about something that will take real time or real disk if pressed.
///
/// An [`Alert`] rather than a hand-styled box: it carries the icon, the warning palette and the
/// `role="status"` that makes the sentence announced, and those have to match every other caution in
/// the dashboard.
#[component]
pub fn Caution(text: String) -> impl IntoView {
    view! {
        <Alert variant=AlertVariant::Warning>
            <p class="text-[0.8125rem] leading-relaxed text-foreground">{text}</p>
        </Alert>
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
