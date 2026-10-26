//! Cards: a bordered surface with an optional header and footer.
//!
//! The pieces are separate components rather than one `Card(title, children)` because panels disagree
//! about their headers: some need a description, some a row of actions, some a badge beside the title
//! and none of them want the padding a fixed layout would impose.

use leptos::prelude::*;

/// The surface.
///
/// `gap-0 py-0` rather than a fixed padding: the header, content and footer each own their own
/// inset, so a card whose content is a full-bleed table is not padded by two ancestors.
#[component]
pub fn Card(
    #[prop(optional, into)] class: Option<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let class = format!(
        "flex flex-col overflow-hidden rounded-xl border border-border bg-card \
         text-card-foreground shadow-xs {}",
        class.unwrap_or_default(),
    );
    view! { <div class=class>{children.map(|children| children())}</div> }
}

/// The card's title row.
#[component]
pub fn CardHeader(
    #[prop(optional, into)] class: Option<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let class = format!(
        "flex items-start justify-between gap-3 px-5 pt-4 pb-3 {}",
        class.unwrap_or_default(),
    );
    view! { <div class=class>{children.map(|children| children())}</div> }
}

/// The card's title.
///
/// `text-sm` and `font-medium`, not a display size. A dashboard is a dense page of many cards, and a
/// 24px title in each one out-shouts the page heading that is supposed to come first.
#[component]
pub fn CardTitle(
    #[prop(optional, into)] class: Option<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let class = format!(
        "text-sm font-medium text-foreground {}",
        class.unwrap_or_default()
    );
    view! { <h2 class=class>{children.map(|children| children())}</h2> }
}

/// A sentence under the title saying what the card is about.
#[component]
pub fn CardDescription(
    #[prop(optional, into)] class: Option<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let class = format!(
        "text-xs leading-relaxed text-muted-foreground {}",
        class.unwrap_or_default()
    );
    view! { <p class=class>{children.map(|children| children())}</p> }
}

/// The card's body.
#[component]
pub fn CardContent(
    #[prop(optional, into)] class: Option<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let class = format!("px-5 pb-4 {}", class.unwrap_or_default());
    view! { <div class=class>{children.map(|children| children())}</div> }
}

/// The card's last row, for the action that applies what the body describes.
#[component]
pub fn CardFooter(
    #[prop(optional, into)] class: Option<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let class = format!(
        "flex items-center justify-between gap-3 border-t border-border bg-muted/30 \
         px-5 py-3 {}",
        class.unwrap_or_default(),
    );
    view! { <div class=class>{children.map(|children| children())}</div> }
}
