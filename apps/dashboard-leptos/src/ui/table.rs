//! Tables: a header row, hoverable rows, and an empty state that says so.
//!
//! Built as plain `<table>` elements rather than a data-grid component. Every list in this dashboard
//! is server-paginated or small enough to render whole, and a grid library would add sorting,
//! virtualisation and selection state that no panel here uses -- while making it harder to see what is
//! actually on screen.
//!
//! The scroll container is inside [`TableFrame`], not the caller, because a wide table has to scroll
//! in its own box. Without the wrapper a `<table>` refuses to shrink inside a flex child and pushes
//! the whole layout sideways instead.

use leptos::prelude::*;

/// The bordered, horizontally scrollable box a table lives in.
#[component]
pub fn TableFrame(
    #[prop(optional, into)] class: Option<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let class = format!(
        "overflow-x-auto rounded-xl border border-border bg-card {}",
        class.unwrap_or_default(),
    );
    view! { <div class=class>{children.map(|children| children())}</div> }
}

/// A `<table>` sized to fill its frame.
#[component]
pub fn Table(#[prop(optional)] children: Option<Children>) -> impl IntoView {
    view! { <table class="w-full caption-bottom text-sm">{children.map(|c| c())}</table> }
}

/// The header row's container.
#[component]
pub fn TableHead(#[prop(optional)] children: Option<Children>) -> impl IntoView {
    view! {
        <thead class="[&_tr]:border-b [&_tr]:border-border">
            {children.map(|children| children())}
        </thead>
    }
}

/// One header cell.
///
/// `text-[0.6875rem] uppercase tracking-wider` rather than a heavier weight: the header is a legend
/// for the columns, and making it the loudest text in the table competes with the data.
#[component]
pub fn TableHeaderCell(
    #[prop(optional, into)] class: Option<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let class = format!(
        "h-9 whitespace-nowrap px-3 text-left align-middle text-[0.6875rem] font-medium \
         uppercase tracking-wider text-muted-foreground {}",
        class.unwrap_or_default(),
    );
    view! { <th class=class>{children.map(|children| children())}</th> }
}

/// A row. Hover is enabled by default because a hoverable row is what signals "this is a record".
#[component]
pub fn TableRow(
    #[prop(optional, into)] class: Option<String>,
    #[prop(optional, into)] hoverable: Option<bool>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let class = format!(
        "border-b border-border/70 transition-colors last:border-0 {} {}",
        if hoverable.unwrap_or(true) {
            "hover:bg-muted/40"
        } else {
            ""
        },
        class.unwrap_or_default(),
    );
    view! { <tr class=class>{children.map(|children| children())}</tr> }
}

/// One body cell.
#[component]
pub fn TableCell(
    #[prop(optional, into)] class: Option<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let class = format!("px-3 py-2.5 align-middle {}", class.unwrap_or_default());
    view! { <td class=class>{children.map(|children| children())}</td> }
}

/// A message spanning the table when there is nothing to show.
///
/// A separate component rather than a `<tr><td colspan=…>` written per panel, because the `colspan`
/// has to match the real column count and a wrong one draws a message squeezed into the first column.
#[component]
pub fn TableEmpty(
    #[prop(into)] message: String,
    /// How many columns the table has.
    #[prop(default = 1)]
    columns: usize,
    #[prop(optional, into)] hint: Option<String>,
) -> impl IntoView {
    view! {
        <tbody>
            <tr>
                <td colspan=columns.to_string() class="px-3 py-10 text-center">
                    <p class="text-sm text-muted-foreground">{message}</p>
                    {hint
                        .filter(|text| !text.trim().is_empty())
                        .map(|text| {
                            view! {
                                <p class="mx-auto mt-1 max-w-md text-xs leading-relaxed text-muted-foreground/80">
                                    {text}
                                </p>
                            }
                        })}
                </td>
            </tr>
        </tbody>
    }
}
