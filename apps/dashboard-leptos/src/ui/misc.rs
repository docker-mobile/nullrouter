//! Small shared pieces: status dots, skeletons, empty states, separators.
//!
//! These are the vocabulary the routes use to say the same thing the same way. A "nothing here yet"
//! that looks different on twelve pages is twelve chances for one of them to read as an error.

use leptos::prelude::*;

use super::icon::{Icon, IconName};

/// A filled circle whose colour means something.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StatusTone {
    /// Healthy, enabled, connected.
    Success,
    /// Degraded, disabled, not configured.
    #[default]
    Muted,
    /// Failing, refused, errored.
    Destructive,
    /// A caveat, or an update available.
    Warning,
}

impl StatusTone {
    const fn dot(self) -> &'static str {
        match self {
            Self::Success => "bg-success",
            Self::Muted => "bg-muted-foreground/40",
            Self::Destructive => "bg-destructive",
            Self::Warning => "bg-warning",
        }
    }
}

/// A dot plus a word, for a state that has a name.
///
/// The label is text rather than colour alone, because "green dot" is not a state anyone can act on
/// and not something a colour-blind reader can distinguish from amber.
#[component]
pub fn StatusDot(
    #[prop(into)] label: String,
    #[prop(optional, into)] tone: Option<StatusTone>,
    /// Pulse the dot. Only for something genuinely live, so movement keeps meaning.
    #[prop(optional, into)]
    pulse: Option<bool>,
) -> impl IntoView {
    let tone = tone.unwrap_or_default();
    let dot = format!(
        "size-1.5 shrink-0 rounded-full {} {}",
        tone.dot(),
        if pulse.unwrap_or(false) {
            "animate-pulse"
        } else {
            ""
        },
    );

    view! {
        <span class="inline-flex items-center gap-1.5 text-xs text-muted-foreground">
            <span class=dot aria-hidden="true" />
            <span>{label}</span>
        </span>
    }
}

/// A dot for a live connection, with its own label.
///
/// Separate from [`StatusDot`] because a stream's state is not a boolean: "Reconnecting…" is neither
/// healthy nor broken, and showing it as either is how a stream that is recovering looks dead.
#[component]
pub fn ConnectionDot(
    #[prop(into)] live: Signal<bool>,
    #[prop(into)] label: Signal<String>,
) -> impl IntoView {
    view! {
        <span class="inline-flex items-center gap-1.5 text-xs text-muted-foreground">
            <span
                class="size-1.5 shrink-0 rounded-full"
                class=("bg-success animate-pulse", move || live.get())
                class=("bg-muted-foreground/40", move || !live.get())
                aria-hidden="true"
            />
            <span>{move || label.get()}</span>
        </span>
    }
}

/// A spinner with a label, for the one action on a page that must not look frozen.
#[component]
pub fn Spinner(
    #[prop(optional, into)] label: Option<String>,
    #[prop(optional, into)] class: Option<String>,
) -> impl IntoView {
    let class = format!(
        "inline-flex items-center gap-2 text-sm text-muted-foreground {}",
        class.unwrap_or_default(),
    );
    view! {
        <span class=class role="status">
            <Icon name=IconName::Loader class="size-4" />
            {label.map(|text| view! { <span>{text}</span> })}
        </span>
    }
}

/// A neutral loading block, for a panel that is not a [`super::page::Panel`].
#[component]
pub fn Skeleton(#[prop(optional, into)] class: Option<String>) -> impl IntoView {
    let class = format!(
        "rounded bg-muted animate-pulse {}",
        class.unwrap_or_default()
    );
    view! { <div class=class aria-hidden="true" /> }
}

/// A rule between two groups of content.
#[component]
pub fn Separator(#[prop(optional, into)] class: Option<String>) -> impl IntoView {
    let class = format!(
        "shrink-0 bg-border {}",
        class.unwrap_or_else(|| "h-px w-full".to_owned())
    );
    view! { <div class=class role="separator" /> }
}

/// "There is nothing here", with the next step when there is one.
///
/// Distinct from a failure on purpose: an empty list and a list that could not be loaded look the same
/// if both render grey text, and the difference is whether the user should retry or add something.
#[component]
pub fn EmptyState(
    #[prop(into)] title: String,
    #[prop(optional, into)] description: Option<String>,
    #[prop(optional, into)] icon: Option<IconName>,
    /// A call to action, usually a button that opens the create form.
    #[prop(optional)]
    children: Option<Children>,
) -> impl IntoView {
    view! {
        <div class="flex flex-col items-center justify-center gap-2 px-6 py-12 text-center">
            {icon
                .map(|icon| {
                    view! {
                        <div class="mb-1 grid size-9 place-items-center rounded-full border border-border bg-muted/50 text-muted-foreground">
                            <Icon name=icon class="size-4" />
                        </div>
                    }
                })}
            <p class="text-sm font-medium text-foreground">{title}</p>
            {description
                .filter(|text| !text.trim().is_empty())
                .map(|text| {
                    view! {
                        <p class="max-w-md text-xs leading-relaxed text-muted-foreground">
                            {text}
                        </p>
                    }
                })}
            {children
                .map(|children| {
                    view! { <div class="mt-2 flex flex-wrap justify-center gap-2">{children()}</div> }
                })}
        </div>
    }
}

/// A message from a failed write, in the server's own words when it had any.
///
/// `role="alert"` so it is announced when it appears, which is the whole point: a save that failed
/// silently is the failure mode this dashboard exists to avoid.
#[component]
pub fn ErrorNote(
    #[prop(into)] message: String,
    #[prop(optional, into)] class: Option<String>,
) -> impl IntoView {
    let class = format!(
        "flex items-start gap-2 text-[0.8125rem] leading-relaxed text-destructive {}",
        class.unwrap_or_default(),
    );
    view! {
        <p class=class role="alert">
            <Icon name=IconName::Alert class="mt-0.5 size-3.5 shrink-0" />
            <span class="min-w-0 whitespace-pre-wrap break-words">{message}</span>
        </p>
    }
}

/// Text set in the monospace face, for identifiers the user might copy.
///
/// `break-all` rather than `break-words`: an API key or a hash has no break opportunities, so
/// word-wrapping alone lets it overflow its container.
#[component]
pub fn Mono(
    #[prop(into)] value: String,
    #[prop(optional, into)] class: Option<String>,
) -> impl IntoView {
    let class = format!(
        "font-mono text-[0.8125rem] break-all text-foreground {}",
        class.unwrap_or_default()
    );
    view! { <span class=class>{value}</span> }
}

/// A timestamp rendered as the server sent it.
///
/// Deliberately not reformatted into a local time: the server's strings are UTC, the dashboard does not
/// know the operator's zone, and a "3 minutes ago" that drifts is worse than a stamp that is exactly
/// what the log says. `title` carries the full value when the cell shows an abbreviated one.
#[component]
pub fn RelativeTime(
    #[prop(into)] value: String,
    #[prop(optional, into)] class: Option<String>,
) -> impl IntoView {
    let class = format!(
        "whitespace-nowrap font-mono text-xs tabular-nums text-muted-foreground {}",
        class.unwrap_or_default()
    );
    let shown = if value.trim().is_empty() {
        "—".to_owned()
    } else {
        value
    };
    let title = shown.clone();
    view! { <span class=class title=title>{shown}</span> }
}

/// A keyboard key, for instructions that name one.
#[component]
pub fn Kbd(#[prop(into)] key: String) -> impl IntoView {
    view! {
        <kbd class="inline-flex h-5 min-w-5 items-center justify-center rounded border \
                    border-border bg-muted px-1 font-mono text-[0.6875rem] font-medium \
                    text-muted-foreground">
            {key}
        </kbd>
    }
}

/// The `kbd`-style hint that explains how to submit a text field.
#[component]
pub fn KeyHint(#[prop(into)] label: String, #[prop(into)] key: String) -> impl IntoView {
    view! {
        <span class="inline-flex items-center gap-1.5 text-xs text-muted-foreground">
            <Kbd key=key />
            <span>{label}</span>
        </span>
    }
}
