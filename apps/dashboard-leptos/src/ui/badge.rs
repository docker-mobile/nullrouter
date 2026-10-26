//! Badges: a small piece of metadata attached to a row.
//!
//! `variant` carries meaning, not decoration. The four semantic variants map to what the thing *is*
//! -- positive, negative, in-between, neutral -- so a table of statuses reads consistently without
//! each panel picking its own colours.

use leptos::prelude::*;

/// What a badge is saying.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BadgeVariant {
    /// The default: a fill that recedes, for counts and categories.
    #[default]
    Secondary,
    /// Nothing is wrong. Enabled, connected, answered.
    Success,
    /// Something is wrong, or the action is destructive.
    Destructive,
    /// Neither: a caveat, a deprecation, a "not yet".
    Warning,
    /// A bordered chip with no fill, for a value the panel is quoting rather than judging.
    Outline,
    /// A machine identifier, rendered in mono with a border.
    Code,
}

impl BadgeVariant {
    /// The variant a class-name string names.
    pub fn parse(raw: &str) -> Self {
        match raw {
            "success" => Self::Success,
            "destructive" => Self::Destructive,
            "warning" => Self::Warning,
            "outline" => Self::Outline,
            "code" => Self::Code,
            _ => Self::Secondary,
        }
    }

    const fn classes(self) -> &'static str {
        match self {
            Self::Secondary => "border-transparent bg-secondary text-secondary-foreground",
            Self::Success => {
                "border-transparent bg-success/12 text-success-emphasis dark:bg-success/18"
            }
            Self::Destructive => {
                "border-transparent bg-destructive/10 text-destructive dark:bg-destructive/20"
            }
            Self::Warning => {
                "border-transparent bg-warning/15 text-warning-emphasis dark:bg-warning/18"
            }
            Self::Outline => "border-border text-foreground",
            Self::Code => "border-border bg-muted/60 font-mono text-muted-foreground",
        }
    }
}

impl From<&str> for BadgeVariant {
    fn from(raw: &str) -> Self {
        Self::parse(raw)
    }
}

impl From<String> for BadgeVariant {
    fn from(raw: String) -> Self {
        Self::parse(&raw)
    }
}

/// A short label attached to something else.
#[component]
pub fn Badge(
    #[prop(optional, into)] variant: Option<BadgeVariant>,
    #[prop(optional, into)] class: Option<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let variant = variant.unwrap_or_default();
    let class = format!(
        "inline-flex shrink-0 items-center gap-1 rounded-md border px-1.5 py-0.5 \
         text-[0.6875rem] font-medium leading-4 whitespace-nowrap \
         {} {}",
        variant.classes(),
        class.unwrap_or_default(),
    );

    view! { <span class=class>{children.map(|children| children())}</span> }
}

#[cfg(test)]
mod tests {
    use super::BadgeVariant;

    #[test]
    fn variants_parse_from_their_names() {
        assert_eq!(BadgeVariant::from("success"), BadgeVariant::Success);
        assert_eq!(BadgeVariant::from("outline"), BadgeVariant::Outline);
        assert_eq!(BadgeVariant::from("code"), BadgeVariant::Code);
        assert_eq!(BadgeVariant::from("nonsense"), BadgeVariant::Secondary);
    }

    #[test]
    fn a_semantic_variant_differs_from_the_default() {
        // If these ever collapse, every status badge in a table reads the same and the colour stops
        // carrying information.
        let base = BadgeVariant::Secondary.classes();
        for other in [
            BadgeVariant::Success,
            BadgeVariant::Destructive,
            BadgeVariant::Warning,
        ] {
            assert_ne!(base, other.classes(), "{other:?}");
        }
    }
}
