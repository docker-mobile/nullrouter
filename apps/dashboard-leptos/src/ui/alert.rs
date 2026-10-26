//! Alerts: a block of text that has to be read before acting.
//!
//! Used for things the user must know *before* pressing a button -- that a tunnel publishes the
//! dashboard to the internet, that an install pulls several gigabytes, that a certificate rotation
//! signs everyone out. A `<p>` in muted grey gets skipped; this does not.

use leptos::prelude::*;

use super::icon::{Icon, IconName};

/// What an alert is telling the user.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AlertVariant {
    /// Neutral context.
    #[default]
    Info,
    /// The action will work, but something about it is not what the user might expect.
    Warning,
    /// The action will fail, or has failed.
    Destructive,
    /// The action cannot be taken on this build, or the data is not available.
    Muted,
}

impl AlertVariant {
    const fn classes(self) -> &'static str {
        match self {
            Self::Info => "border-border bg-muted/40 [&_[data-alert-icon]]:text-muted-foreground",
            Self::Warning => {
                "border-warning/35 bg-warning/[0.07] [&_[data-alert-icon]]:text-warning"
            }
            Self::Destructive => {
                "border-destructive/30 bg-destructive/[0.05] \
                 [&_[data-alert-icon]]:text-destructive"
            }
            Self::Muted => {
                "border-border border-dashed bg-transparent [&_[data-alert-icon]]:text-muted-foreground"
            }
        }
    }

    /// The glyph is a judgement call, not a bijection: warning and failure are both alerts, and
    /// neutral and informative are both notes.
    const fn icon(self) -> IconName {
        match self {
            Self::Info | Self::Muted => IconName::Info,
            Self::Warning | Self::Destructive => IconName::Alert,
        }
    }
}

impl From<&str> for AlertVariant {
    fn from(raw: &str) -> Self {
        match raw {
            "warning" => Self::Warning,
            "destructive" => Self::Destructive,
            "muted" => Self::Muted,
            _ => Self::Info,
        }
    }
}

/// The alert box.
///
/// `role="status"` rather than `role="alert"`: these sit in the flow of a page the user has just
/// opened, and an assertive announcement interrupts whatever the screen reader was already reading.
#[component]
pub fn Alert(
    #[prop(optional, into)] variant: Option<AlertVariant>,
    #[prop(optional, into)] class: Option<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let variant = variant.unwrap_or_default();
    let class = format!(
        "flex items-start gap-2.5 rounded-lg border px-3.5 py-3 text-sm {} {}",
        variant.classes(),
        class.unwrap_or_default(),
    );

    view! {
        <div class=class role="status">
            <span data-alert-icon class="mt-px shrink-0">
                <Icon name=variant.icon() class="size-4" />
            </span>
            <div class="min-w-0 flex-1 space-y-1">{children.map(|children| children())}</div>
        </div>
    }
}

/// The alert's first line.
#[component]
pub fn AlertTitle(
    #[prop(optional, into)] class: Option<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let class = format!(
        "text-[0.8125rem] font-medium leading-relaxed text-foreground {}",
        class.unwrap_or_default()
    );
    view! { <p class=class>{children.map(|children| children())}</p> }
}

/// The alert's explanation.
#[component]
pub fn AlertDescription(
    #[prop(optional, into)] class: Option<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let class = format!(
        "text-[0.8125rem] leading-relaxed text-muted-foreground {}",
        class.unwrap_or_default()
    );
    view! { <div class=class>{children.map(|children| children())}</div> }
}
