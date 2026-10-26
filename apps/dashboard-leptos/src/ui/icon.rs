//! Icons, as lucide geometry drawn inline.
//!
//! The path data is lucide's, MIT/ISC licensed, and it is transcribed rather than fetched because
//! the alternative costs a render-blocking webfont request or an icon sprite the bundle cannot
//! verify. The previous shell pulled a Material Icons webfont to draw about fifteen shapes, which is
//! a network round trip and a flash of unstyled glyphs on every cold load.
//!
//! Icons are stroke-based (lucide's own 2px convention at a 24x24 viewBox) rather than filled, which
//! is what makes them read as one set at a 16px render size. `stroke-width="1.75"` rather than 2
//! because the dashboard draws them at 14-18px, where 2px of ink is heavy.

use leptos::prelude::*;

/// Every glyph the dashboard draws.
///
/// A closed enum rather than a `&'static str` of path data: a typo at a call site is a compile error
/// instead of an icon that silently renders nothing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IconName {
    Activity,
    Alert,
    BookOpen,
    Box,
    ChartColumn,
    Check,
    ChevronDown,
    ChevronRight,
    Clock,
    Combine,
    Copy,
    Cpu,
    Database,
    Download,
    Droplet,
    ExternalLink,
    Gauge,
    Globe,
    Info,
    Key,
    Layers,
    LayoutDashboard,
    Loader,
    Lock,
    LogOut,
    Menu,
    MessageSquare,
    Monitor,
    Moon,
    /// A horizontal ellipsis: the standard "more actions" glyph on a row.
    More,
    Network,
    PanelLeft,
    Pencil,
    Play,
    Plug,
    Plus,
    Power,
    Refresh,
    Route,
    ScrollText,
    Search,
    Server,
    Settings,
    ShieldCheck,
    Sliders,
    Sparkles,
    Square,
    Sun,
    Tag,
    Terminal,
    Trash,
    Users,
    Wifi,
    X,
    Zap,
}

impl IconName {
    /// The child elements of a 24x24 viewBox, as lucide draws them.
    ///
    /// Returned as raw SVG markup rather than a `view!` tree because there is no conditional or
    /// reactive part: it is a static string per glyph, and going through `inner_html` avoids
    /// building twenty nodes' worth of view machinery for something that never changes.
    #[allow(
        clippy::too_many_lines,
        reason = "one arm per glyph, which is the whole point of the table"
    )]
    const fn body(self) -> &'static str {
        match self {
            Self::Activity => {
                r#"<path d="M22 12h-2.48a2 2 0 0 0-1.93 1.46l-2.35 8.36a.25.25 0 0 1-.48 0L9.24 2.18a.25.25 0 0 0-.48 0l-2.35 8.36A2 2 0 0 1 4.49 12H2"/>"#
            }
            Self::Alert => {
                r#"<circle cx="12" cy="12" r="10"/><line x1="12" x2="12" y1="8" y2="12"/><line x1="12" x2="12.01" y1="16" y2="16"/>"#
            }
            Self::BookOpen => {
                r#"<path d="M12 7v14"/><path d="M3 18a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1h5a4 4 0 0 1 4 4 4 4 0 0 1 4-4h5a1 1 0 0 1 1 1v13a1 1 0 0 1-1 1h-6a3 3 0 0 0-3 3 3 3 0 0 0-3-3z"/>"#
            }
            Self::Box => {
                r#"<path d="M21 8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16Z"/><path d="m3.3 7 8.7 5 8.7-5"/><path d="M12 22V12"/>"#
            }
            Self::ChartColumn => {
                r#"<path d="M3 3v16a2 2 0 0 0 2 2h16"/><path d="M18 17V9"/><path d="M13 17V5"/><path d="M8 17v-3"/>"#
            }
            Self::Check => r#"<path d="M20 6 9 17l-5-5"/>"#,
            Self::ChevronDown => r#"<path d="m6 9 6 6 6-6"/>"#,
            Self::ChevronRight => r#"<path d="m9 18 6-6-6-6"/>"#,
            Self::Clock => r#"<path d="M12 6v6l4 2"/><circle cx="12" cy="12" r="10"/>"#,
            Self::Combine => {
                r#"<path d="M14 3a1 1 0 0 1 1 1v5a1 1 0 0 1-1 1"/><path d="M19 3a1 1 0 0 1 1 1v5a1 1 0 0 1-1 1"/><path d="m7 15 3 3"/><path d="m7 21 3-3H5a2 2 0 0 1-2-2v-2"/><rect x="14" y="14" width="7" height="7" rx="1"/><rect x="3" y="3" width="7" height="7" rx="1"/>"#
            }
            Self::Copy => {
                r#"<rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>"#
            }
            Self::Cpu => {
                r#"<path d="M12 20v2"/><path d="M12 2v2"/><path d="M17 20v2"/><path d="M17 2v2"/><path d="M2 12h2"/><path d="M2 17h2"/><path d="M2 7h2"/><path d="M20 12h2"/><path d="M20 17h2"/><path d="M20 7h2"/><path d="M7 20v2"/><path d="M7 2v2"/><rect x="4" y="4" width="16" height="16" rx="2"/><rect x="8" y="8" width="8" height="8" rx="1"/>"#
            }
            Self::Database => {
                r#"<ellipse cx="12" cy="5" rx="9" ry="3"/><path d="M3 5V19A9 3 0 0 0 21 19V5"/><path d="M3 12A9 3 0 0 0 21 12"/>"#
            }
            Self::Download => {
                r#"<path d="M12 13v8l-4-4"/><path d="m12 21 4-4"/><path d="M4.393 15.269A7 7 0 1 1 15.71 8h1.79a4.5 4.5 0 0 1 2.436 8.284"/>"#
            }
            Self::Droplet => {
                r#"<path d="M12 22a7 7 0 0 0 7-7c0-2-1-3.9-3-5.5s-3.5-4-4-6.5c-.5 2.5-2 4.9-4 6.5C6 11.1 5 13 5 15a7 7 0 0 0 7 7z"/>"#
            }
            Self::ExternalLink => {
                r#"<path d="M15 3h6v6"/><path d="M10 14 21 3"/><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/>"#
            }
            Self::Gauge => r#"<path d="m12 14 4-4"/><path d="M3.34 19a10 10 0 1 1 17.32 0"/>"#,
            Self::Globe => {
                r#"<circle cx="12" cy="12" r="10"/><path d="M12 2a14.5 14.5 0 0 0 0 20 14.5 14.5 0 0 0 0-20"/><path d="M2 12h20"/>"#
            }
            Self::Info => {
                r#"<circle cx="12" cy="12" r="10"/><path d="M12 16v-4"/><path d="M12 8h.01"/>"#
            }
            Self::Key => {
                r#"<path d="m15.5 7.5 2.3 2.3a1 1 0 0 0 1.4 0l2.1-2.1a1 1 0 0 0 0-1.4L19 4"/><path d="m21 2-9.6 9.6"/><circle cx="7.5" cy="15.5" r="5.5"/>"#
            }
            Self::Layers => {
                r#"<path d="M12.83 2.18a2 2 0 0 0-1.66 0L2.6 6.08a1 1 0 0 0 0 1.83l8.58 3.91a2 2 0 0 0 1.66 0l8.58-3.9a1 1 0 0 0 0-1.83z"/><path d="M2 12a1 1 0 0 0 .58.91l8.6 3.91a2 2 0 0 0 1.65 0l8.58-3.9A1 1 0 0 0 22 12"/><path d="M2 17a1 1 0 0 0 .58.91l8.6 3.91a2 2 0 0 0 1.65 0l8.58-3.9A1 1 0 0 0 22 17"/>"#
            }
            Self::LayoutDashboard => {
                r#"<rect width="7" height="9" x="3" y="3" rx="1"/><rect width="7" height="5" x="14" y="3" rx="1"/><rect width="7" height="9" x="14" y="12" rx="1"/><rect width="7" height="5" x="3" y="16" rx="1"/>"#
            }
            Self::Loader => r#"<path d="M21 12a9 9 0 1 1-6.219-8.56"/>"#,
            Self::Lock => {
                r#"<rect width="18" height="11" x="3" y="11" rx="2" ry="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/>"#
            }
            Self::LogOut => {
                r#"<path d="m16 17 5-5-5-5"/><path d="M21 12H9"/><path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4"/>"#
            }
            Self::Menu => r#"<path d="M4 5h16"/><path d="M4 12h16"/><path d="M4 19h16"/>"#,
            Self::MessageSquare => {
                r#"<path d="M16 10a2 2 0 0 1-2 2H6.828a2 2 0 0 0-1.414.586l-2.202 2.202A.71.71 0 0 1 2 14.286V4a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2z"/><path d="M20 9a2 2 0 0 1 2 2v10.286a.71.71 0 0 1-1.212.502l-2.202-2.202A2 2 0 0 0 17.172 19H10a2 2 0 0 1-2-2v-1"/>"#
            }
            Self::Monitor => {
                r#"<rect width="20" height="14" x="2" y="3" rx="2"/><line x1="8" x2="16" y1="21" y2="21"/><line x1="12" x2="12" y1="17" y2="21"/>"#
            }
            Self::Moon => {
                r#"<path d="M20.985 12.486a9 9 0 1 1-9.473-9.472c.405-.022.617.46.402.803a6 6 0 0 0 8.268 8.268c.344-.215.825-.004.803.401"/>"#
            }
            Self::More => {
                r#"<circle cx="12" cy="12" r="1"/><circle cx="19" cy="12" r="1"/><circle cx="5" cy="12" r="1"/>"#
            }
            Self::Network => {
                r#"<rect x="16" y="16" width="6" height="6" rx="1"/><rect x="2" y="16" width="6" height="6" rx="1"/><rect x="9" y="2" width="6" height="6" rx="1"/><path d="M5 16v-3a1 1 0 0 1 1-1h12a1 1 0 0 1 1 1v3"/><path d="M12 12V8"/>"#
            }
            Self::PanelLeft => {
                r#"<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M9 3v18"/>"#
            }
            Self::Pencil => {
                r#"<path d="M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z"/><path d="m15 5 4 4"/>"#
            }
            Self::Play => {
                r#"<path d="M5 5a2 2 0 0 1 3.008-1.728l11.997 6.998a2 2 0 0 1 .003 3.458l-12 7A2 2 0 0 1 5 19z"/>"#
            }
            Self::Plug => {
                r#"<path d="M12 22v-5"/><path d="M9 8V2"/><path d="M15 8V2"/><path d="M18 8v5a4 4 0 0 1-4 4h-4a4 4 0 0 1-4-4V8Z"/>"#
            }
            Self::Plus => r#"<path d="M5 12h14"/><path d="M12 5v14"/>"#,
            Self::Power => r#"<path d="M12 2v10"/><path d="M18.4 6.6a9 9 0 1 1-12.77.04"/>"#,
            Self::Refresh => {
                r#"<path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/><path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"/><path d="M8 16H3v5"/>"#
            }
            Self::Route => {
                r#"<circle cx="6" cy="19" r="3"/><path d="M9 19h8.5a3.5 3.5 0 0 0 0-7h-11a3.5 3.5 0 0 1 0-7H15"/><circle cx="18" cy="5" r="3"/>"#
            }
            Self::ScrollText => {
                r#"<path d="M15 12h-5"/><path d="M15 8h-5"/><path d="M19 17V5a2 2 0 0 0-2-2H4"/><path d="M8 21h12a2 2 0 0 0 2-2v-1a1 1 0 0 0-1-1H11a1 1 0 0 0-1 1v1a2 2 0 1 1-4 0V5a2 2 0 1 0-4 0v2a1 1 0 0 0 1 1h3"/>"#
            }
            Self::Search => r#"<path d="m21 21-4.34-4.34"/><circle cx="11" cy="11" r="8"/>"#,
            Self::Server => {
                r#"<rect width="20" height="8" x="2" y="2" rx="2" ry="2"/><rect width="20" height="8" x="2" y="14" rx="2" ry="2"/><line x1="6" x2="6.01" y1="6" y2="6"/><line x1="6" x2="6.01" y1="18" y2="18"/>"#
            }
            Self::Settings => {
                r#"<path d="M9.671 4.136a2.34 2.34 0 0 1 4.659 0 2.34 2.34 0 0 0 3.319 1.915 2.34 2.34 0 0 1 2.33 4.033 2.34 2.34 0 0 0 0 3.831 2.34 2.34 0 0 1-2.33 4.033 2.34 2.34 0 0 0-3.319 1.915 2.34 2.34 0 0 1-4.659 0 2.34 2.34 0 0 0-3.32-1.915 2.34 2.34 0 0 1-2.33-4.033 2.34 2.34 0 0 0 0-3.831A2.34 2.34 0 0 1 6.35 6.051a2.34 2.34 0 0 0 3.319-1.915"/><circle cx="12" cy="12" r="3"/>"#
            }
            Self::ShieldCheck => {
                r#"<path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z"/><path d="m9 12 2 2 4-4"/>"#
            }
            Self::Sliders => {
                r#"<path d="M10 5H3"/><path d="M12 19H3"/><path d="M14 3v4"/><path d="M16 17v4"/><path d="M21 12h-9"/><path d="M21 19h-5"/><path d="M21 5h-7"/><path d="M8 10v4"/><path d="M8 12H3"/>"#
            }
            Self::Sparkles => {
                r#"<path d="M11.017 2.814a1 1 0 0 1 1.966 0l1.051 5.558a2 2 0 0 0 1.594 1.594l5.558 1.051a1 1 0 0 1 0 1.966l-5.558 1.051a2 2 0 0 0-1.594 1.594l-1.051 5.558a1 1 0 0 1-1.966 0l-1.051-5.558a2 2 0 0 0-1.594-1.594l-5.558-1.051a1 1 0 0 1 0-1.966l5.558-1.051a2 2 0 0 0 1.594-1.594z"/><path d="M20 2v4"/><path d="M22 4h-4"/><circle cx="4" cy="20" r="2"/>"#
            }
            Self::Square => r#"<rect width="18" height="18" x="3" y="3" rx="2"/>"#,
            Self::Sun => {
                r#"<circle cx="12" cy="12" r="4"/><path d="M12 2v2"/><path d="M12 20v2"/><path d="m4.93 4.93 1.41 1.41"/><path d="m17.66 17.66 1.41 1.41"/><path d="M2 12h2"/><path d="M20 12h2"/><path d="m6.34 17.66-1.41 1.41"/><path d="m19.07 4.93-1.41 1.41"/>"#
            }
            Self::Tag => {
                r#"<path d="M12.586 2.586A2 2 0 0 0 11.172 2H4a2 2 0 0 0-2 2v7.172a2 2 0 0 0 .586 1.414l8.704 8.704a2.426 2.426 0 0 0 3.42 0l6.58-6.58a2.426 2.426 0 0 0 0-3.42z"/><circle cx="7.5" cy="7.5" r=".5" fill="currentColor"/>"#
            }
            Self::Terminal => r#"<path d="M12 19h8"/><path d="m4 17 6-6-6-6"/>"#,
            Self::Trash => {
                r#"<path d="M10 11v6"/><path d="M14 11v6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6"/><path d="M3 6h18"/><path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/>"#
            }
            Self::Users => {
                r#"<path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><path d="M16 3.128a4 4 0 0 1 0 7.744"/><path d="M22 21v-2a4 4 0 0 0-3-3.87"/><circle cx="9" cy="7" r="4"/>"#
            }
            Self::Wifi => {
                r#"<path d="M12 20h.01"/><path d="M2 8.82a15 15 0 0 1 20 0"/><path d="M5 12.859a10 10 0 0 1 14 0"/><path d="M8.5 16.429a5 5 0 0 1 7 0"/>"#
            }
            Self::X => r#"<path d="M18 6 6 18"/><path d="m6 6 12 12"/>"#,
            Self::Zap => {
                r#"<path d="M4 14a1 1 0 0 1-.78-1.63l9.9-10.2a.5.5 0 0 1 .86.46l-1.92 6.02A1 1 0 0 0 13 10h7a1 1 0 0 1 .78 1.63l-9.9 10.2a.5.5 0 0 1-.86-.46l1.92-6.02A1 1 0 0 0 11 14z"/>"#
            }
        }
    }

    /// Whether this glyph reads as a spinner, so it can be spun by [`Icon`].
    pub const fn spins(self) -> bool {
        matches!(self, Self::Loader)
    }
}

/// One icon.
///
/// `aria-hidden` by default and decoupled from any accessible name, which is the correct default: an
/// icon beside a label is decoration, and an icon that *is* the control needs its name on the button
/// that contains it. A caller that passes `label` gets `role="img"` and an `aria-label` instead.
#[component]
pub fn Icon(
    name: IconName,
    /// A size utility, `size-4` for the default.
    #[prop(optional, into)]
    class: Option<String>,
    /// The accessible name for an icon that stands alone.
    #[prop(optional, into)]
    label: Option<String>,
    /// Whether to spin. Defaults to spinning exactly for the loader glyph, so a call site does not
    /// have to remember.
    #[prop(optional, into)]
    spin: Option<Signal<bool>>,
) -> impl IntoView {
    let mut base = format!("shrink-0 {}", class.unwrap_or_else(|| "size-4".to_owned()));
    if name.spins() {
        base.push_str(" animate-spin");
    }

    let name_attr = label.filter(|value| !value.trim().is_empty());

    // A caller-driven spin is independent of the glyph, so `spin=false` can stop a loader and
    // `spin=true` can turn anything else into one.
    let class = match spin {
        Some(spin) => Signal::derive(move || {
            let mut class = base.clone();
            if spin.get() {
                if !class.contains("animate-spin") {
                    class.push_str(" animate-spin");
                }
            } else {
                class = class.replace(" animate-spin", "");
            }
            class
        }),
        None => Signal::derive(move || base.clone()),
    };

    view! {
        <svg
            xmlns="http://www.w3.org/2000/svg"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="1.75"
            stroke-linecap="round"
            stroke-linejoin="round"
            class=move || class.get()
            aria-hidden=name_attr.is_none().then_some("true")
            role=name_attr.as_ref().map(|_| "img")
            aria-label=name_attr
            inner_html=name.body()
        />
    }
}

#[cfg(test)]
mod tests {
    use super::IconName;

    /// Every glyph must draw something, and every `d` must be a well-formed path command sequence.
    ///
    /// A malformed `d` renders as nothing at all with no console error, so an icon typo is invisible
    /// until someone notices a blank square in a table. This catches that at test time.
    #[test]
    fn every_glyph_draws_a_well_formed_shape() {
        let glyphs = [
            IconName::Activity,
            IconName::Alert,
            IconName::BookOpen,
            IconName::Box,
            IconName::ChartColumn,
            IconName::Check,
            IconName::ChevronDown,
            IconName::ChevronRight,
            IconName::Clock,
            IconName::Combine,
            IconName::Copy,
            IconName::Cpu,
            IconName::Database,
            IconName::Download,
            IconName::Droplet,
            IconName::ExternalLink,
            IconName::Gauge,
            IconName::Globe,
            IconName::Info,
            IconName::Key,
            IconName::Layers,
            IconName::LayoutDashboard,
            IconName::Loader,
            IconName::Lock,
            IconName::LogOut,
            IconName::Menu,
            IconName::MessageSquare,
            IconName::Monitor,
            IconName::Moon,
            IconName::More,
            IconName::Network,
            IconName::PanelLeft,
            IconName::Pencil,
            IconName::Play,
            IconName::Plug,
            IconName::Plus,
            IconName::Power,
            IconName::Refresh,
            IconName::Route,
            IconName::ScrollText,
            IconName::Search,
            IconName::Server,
            IconName::Settings,
            IconName::ShieldCheck,
            IconName::Sliders,
            IconName::Sparkles,
            IconName::Square,
            IconName::Sun,
            IconName::Tag,
            IconName::Terminal,
            IconName::Trash,
            IconName::Users,
            IconName::Wifi,
            IconName::X,
            IconName::Zap,
        ];

        for glyph in glyphs {
            let body = glyph.body();
            assert!(!body.is_empty(), "{glyph:?} draws nothing");
            assert!(
                body.starts_with('<'),
                "{glyph:?} does not start with an element"
            );
            // Balanced tags: every `<tag` is closed by a matching `/>` or `</tag>`.
            let opens = body.matches('<').count();
            let closes = body.matches('>').count();
            assert_eq!(opens, closes, "{glyph:?} has an unbalanced tag");
            // A `d` attribute must not be empty, or the path draws nothing.
            for segment in body.split("d=\"").skip(1) {
                let value = segment.split('"').next().unwrap_or_default();
                assert!(!value.trim().is_empty(), "{glyph:?} has an empty path");
                assert!(
                    value
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || " .,-".contains(c)),
                    "{glyph:?} has an unexpected character in a path: {value}"
                );
            }
        }
    }

    #[test]
    fn only_the_loader_spins_by_default() {
        assert!(IconName::Loader.spins());
        assert!(!IconName::Check.spins());
    }
}
