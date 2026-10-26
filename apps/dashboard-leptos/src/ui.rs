//! The dashboard's component layer.
//!
//! These are shadcn/ui components in the sense that matters: **source this repository owns**.
//! Upstream shadcn ships Radix primitives wrapped in React and you copy the wrapper in; on Leptos
//! there is no Radix, so each component here is the equivalent wrapper over plain HTML elements,
//! ARIA attributes, and signal-driven state. Being in-tree is the point rather than an accident --
//! the alternative, a published component crate, is what the previous version used, and it cost
//! three things this version does not pay for:
//!
//! - **Utilities that were not in the stylesheet.** Tailwind scans source text for class names. A
//!   component crate's classes live in `~/.cargo/registry`, which the stylesheet had to reach
//!   through a five-level relative glob. That glob stopped resolving, so a fresh `npm run css`
//!   silently dropped every component utility and shipped a component set that rendered unstyled.
//!   With the components here, `@source "../src/**/*.rs"` already covers them.
//! - **Props shaped by someone else's abstractions.** `leptos-shadcn-input` stores its value in an
//!   internal signal seeded from a prop, so a panel that re-renders after a save cannot clear a
//!   field. These are controlled: the caller owns the signal.
//! - **A `Style` wrapper from `leptos-style` on every prop list**, and a `supports!` macro in the
//!   view syntax of one deprecates-on-sight era of Leptos.
//!
//! # Conventions
//!
//! Every component takes `#[prop(optional, into)] class: Option<String>` and appends it to its own
//! classes, so a call site can adjust spacing without forking the component. Variants are enums with
//! a `From<&str>` impl so both `variant=BadgeVariant::Outline` and `variant="outline"` compile.
//!
//! Nothing here fetches. A component that needs server data takes a `Hydrate` from its caller, which
//! keeps the "loading, loaded, or explaining itself" rule visible at the call site instead of hiding
//! it inside a spinner.

pub mod alert;
pub mod badge;
pub mod button;
pub mod card;
pub mod dialog;
pub mod field;
pub mod icon;
pub mod input;
pub mod menu;
pub mod misc;
pub mod page;
pub mod table;
pub mod tabs;

pub use alert::{Alert, AlertDescription, AlertTitle, AlertVariant};
pub use badge::{Badge, BadgeVariant};
pub use button::{Button, ButtonSize, ButtonVariant, button_class};
pub use card::{Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle};
pub use dialog::{
    Dialog, DialogClose, DialogContent, DialogDescription, DialogDismiss, DialogFooter,
    DialogHeader, DialogTitle, DialogTrigger,
};
pub use field::{Checkbox, Field, FieldDescription, FieldLabel, Select, Switch, Textarea};
pub use icon::{Icon, IconName};
pub use input::{Input, InputGroup};
pub use menu::{Menu, MenuItem, MenuLabel, MenuSeparator, MenuTriggerVariant};
pub use misc::{
    ConnectionDot, EmptyState, ErrorNote, Kbd, KeyHint, Mono, RelativeTime, Separator, Skeleton,
    Spinner, StatusDot, StatusTone,
};
pub use page::{PageActions, PageBody, PageHeader, Panel, PanelError, PanelSkeleton};
pub use table::{Table, TableCell, TableEmpty, TableFrame, TableHead, TableHeaderCell, TableRow};
pub use tabs::{TabContent, Tabs};
