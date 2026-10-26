# Dashboard UI API (in-tree shadcn layer)

This file is a working reference for agents editing `apps/dashboard-leptos/src/routes/*.rs`.
It describes exactly what is available. Do not invent props; if a prop is not listed here it
does not exist.

## Rules that will break the build if ignored

1. **`view!` moves what it reads.** Leptos 0.8's `view!` macro moves every variable it references
   into the closures it generates. A non-`Copy` value (`String`, `Locale`, a local struct) read
   inside a `view!` tree is *consumed* by it. Using it a second time in the same tree, or after the
   tree, is error E0382 `use of moved value`.
   - **Use `t!("key")` for every message.** It re-reads the locale from context at the point of
     render, so it captures nothing and may be used any number of times. Import it with
     `use crate::t;`.
   - A `String` that must appear twice needs two bindings (`let a = t!("x"); let b = a.clone();`).
   - A `String` read inside a `move ||` closure must be cloned before that closure.
2. **`t!` takes a string literal**, never a variable. `t!(key)` where `key: &str` does not compile.
   For a table of options, call `t!("…")` in the literal array.
3. **`PageHeader` takes its actions as children**:
   ```rust
   <PageHeader title=t!("nav.keys") description=t!("keys.description")>
       <Button label=t!("action.refresh") on_click=… />
   </PageHeader>
   ```
   A self-closing `<PageHeader … />` is correct when there are no actions.
4. **Every page body goes inside `<PageBody>`**, which supplies the vertical rhythm. Do not add
   `mb-4` / `mt-6` between sections; `PageBody` uses `space-y-4`.
5. **`Panel` is the only way to render server data.** Never render `Hydrate` yourself.
   ```rust
   <Panel state=rows on_retry=Callback::new(move |()| reload())
          children=move |data: KeysList| view! { <KeysTable rows=data.keys /> } />
   ```
   The `Panel` `children` closure takes the decoded value and returns a view.

## Imports

`crate::routes` re-exports the whole UI layer, so a page can list what it uses:

```rust
use crate::routes::{
    Alert, AlertVariant, AlertTitle, AlertDescription, Badge, BadgeVariant, Button, ButtonSize,
    ButtonVariant, Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle, Checkbox,
    ConnectionDot, Dialog, DialogClose, DialogContent, DialogDescription, DialogDismiss,
    DialogFooter, DialogHeader, DialogTitle, EmptyState, ErrorNote, Field, FieldDescription,
    Icon, IconName, Input, Menu, MenuItem, MenuSeparator, Mono, PageActions, PageBody, PageHeader,
    Panel, PanelError, RelativeTime, Select, Separator, Skeleton, Spinner, StatusDot, StatusTone,
    Switch, Table, TableCell, TableEmpty, TableFrame, TableHead, TableHeaderCell, TableRow, Tabs,
    Textarea, button_class, or_dash,
};
use crate::t;
```

## Components and their exact props

Props marked `optional` may be omitted. `into` props accept a `String` where `Option<String>` is
shown (so `class="mt-2"` works).

### `PageHeader`
| prop | type | notes |
|---|---|---|
| `title` | `String` (required, `into`) | renders `<h1>` |
| `description` | `Option<String>` | one sentence under the title |
| `eyebrow` | `Option<String>` | small uppercase line above the title |
| `children` | `Option<Children>` | page-level actions, rendered right-aligned |

### `PageBody`
`children: Option<Children>`. One per page, wraps everything under the header.

### `PageActions`
`children`. A flex row for 2+ actions when they are not in the header.

### `Card`, `CardHeader`, `CardTitle`, `CardDescription`, `CardContent`, `CardFooter`
All take `class: Option<String>` and `children`. `CardHeader` is already a flex row with
`justify-between`, so a title div plus a trailing badge/button is the intended use.
`CardContent` has `px-5 pb-4`. `CardFooter` is a muted bar with `justify-between` and a top border —
use it for a card's save action.

### `Button`
| prop | type | notes |
|---|---|---|
| `label` | `Option<String>` | omit for an icon-only button |
| `variant` | `ButtonVariant` | `Default`, `Destructive`, `Outline`, `Secondary`, `Ghost`, `Link` |
| `size` | `ButtonSize` | `Default`, `Sm`, `Lg`, `Icon`, `IconSm` |
| `disabled` | `Option<Signal<bool>>` | |
| `busy` | `Option<Signal<bool>>` | appends `…` and disables |
| `aria_label` | `Option<String>` | **required** when there is no label |
| `button_type` | `Option<String>` | `"submit"` inside a form |
| `class` | `Option<String>` | |
| `on_click` | `Option<Callback<()>>` | |

For a link that must look like a button:
```rust
<a href="/dashboard/keys" class=button_class(ButtonVariant::Outline, ButtonSize::Sm)>
    <Icon name=IconName::Key class="size-4" />
    {t!("nav.keys")}
</a>
```
For an icon inside a `Button`, do **not** pass children — `Button` renders only its `label`. Write a
raw `<button class=button_class(..)>` when the control needs a glyph plus text.

### `Input`
`value: MaybeProp<String>`, `on_input: Option<Callback<String>>`, `placeholder`, `input_type`
(`"password"`, `"search"`, `"number"`, `"url"`), `name`, `id`, `autocomplete`,
`disabled: Option<Signal<bool>>`, `invalid: Option<Signal<bool>>`, `aria_label`, `described_by`,
`class`. Controlled: the caller owns the signal.

```rust
<Input value=Signal::derive(move || name.get())
       on_input=Callback::new(move |value| set_name.set(value))
       placeholder=t!("keys.name_placeholder") />
```

### `Textarea`
Same shape as `Input` plus `rows: Option<u32>`.

### `Field`
`label: String` (required), `description: Option<String>`, `required: Option<bool>`,
`error: Option<String>`, `class`, `children`. Wraps the control in a `<label>`.

### `FieldLabel`, `FieldDescription`
`FieldLabel { text, for_id, class }`; `FieldDescription { class, children }`.

### `Select`
`value: MaybeProp<String>`, `options: Signal<Vec<(String, String)>>` (value, label),
`on_change: Option<Callback<String>>`, `placeholder`, `id`, `aria_label`, `disabled`, `class`.

### `Switch`
`checked: Signal<bool>`, `on_change: Option<Callback<bool>>`, `disabled`, `aria_label`, `id`.
Renders `<button role="switch">`; it does not take children.

### `Checkbox`
`checked: Signal<bool>`, `on_change: Option<Callback<bool>>`, `disabled`, `label: Option<String>`,
`aria_label`.

### `Badge`
`variant: BadgeVariant` (`Secondary` default, `Success`, `Destructive`, `Warning`, `Outline`,
`Code`), `class`, `children`.

### `Alert`, `AlertTitle`, `AlertDescription`
`Alert { variant: AlertVariant (Info default, Warning, Destructive, Muted), class, children }`.
Has a leading icon automatically; put `AlertTitle` and `AlertDescription` inside.

### `TableFrame`, `Table`, `TableHead`, `TableHeaderCell`, `TableRow`, `TableCell`, `TableEmpty`
```rust
<TableFrame>
    <Table>
        <TableHead><tr>
            <TableHeaderCell>{t!("keys.col_name")}</TableHeaderCell>
            <TableHeaderCell class="text-right">{t!("table.actions")}</TableHeaderCell>
        </tr></TableHead>
        <tbody>{rows.into_iter().map(|row| view! { <Row row=row /> }).collect_view()}</tbody>
    </Table>
</TableFrame>
```
`TableEmpty { message: String, columns: usize, hint: Option<String> }` renders its own `<tbody>`, so
use it **instead of** the `<table>` body, not inside one.
`TableRow { class, hoverable: Option<bool>, children }`. `TableCell { class, children }`.

### `EmptyState`
`title: String`, `description: Option<String>`, `icon: Option<IconName>`, `children` (a call to
action). For "nothing stored yet" — not for errors.

### `Dialog` family
```rust
<Dialog open=Signal::derive(move || open.get())
        on_open_change=Callback::new(move |next| set_open.set(next))>
    <DialogHeader>
        <DialogTitle>{t!("nodes.create")}</DialogTitle>
        <DialogDismiss />
    </DialogHeader>
    <DialogContent>…</DialogContent>
    <DialogFooter>
        <Button label=t!("nodes.cancel") variant=ButtonVariant::Outline
                on_click=Callback::new(move |()| set_open.set(false)) />
        <Button label=t!("nodes.save") on_click=Callback::new(move |()| save()) />
    </DialogFooter>
</Dialog>
```
`Dialog` already handles Escape, the backdrop, focus trapping and the top layer. `width` overrides
`max-w-lg`.

### `Menu` family
```rust
<Menu aria_label=t!("table.actions") icon=IconName::More align_end=true trigger_variant=MenuTriggerVariant::Ghost>
    <MenuItem label=t!("nodes.edit") icon=IconName::Pencil on_select=Callback::new(move |()| …) />
    <MenuSeparator />
    <MenuItem label=t!("nodes.delete") icon=IconName::Trash destructive=true on_select=… />
</Menu>
```
`Menu { label, aria_label, icon, trigger_variant (Outline default, Ghost, Default), width, align_end, class, children }`.
`MenuItem { label, on_select, icon, description, destructive, href }`.
`Menu`'s children must be `'static + Clone`-able — build them inside the component that owns the
`Menu`, not inside a per-render closure if that causes trouble. If the compiler complains about
`FnOnce`, that is why.

### `Tabs`
```rust
let selected = Signal::derive(move || tab.get());
<Tabs labels=Signal::derive(|| vec![t!("a"), t!("b")]) selected=selected
      on_select=Callback::new(move |index| set_tab.set(index)) />
<TabContent index=0 selected=selected>…</TabContent>
```
Index-driven; `TabContent` renders only while selected.

### `Icon` and `IconName`
`Icon { name: IconName, class: Option<String> ("size-4" default), label: Option<String>, spin }`.
Available `IconName` variants: `Activity Alert BookOpen Box ChartColumn Check ChevronDown
ChevronRight Clock Combine Copy Cpu Database Download Droplet ExternalLink Gauge Globe Info Key
Layers LayoutDashboard Loader Lock LogOut Menu MessageSquare Monitor Moon Network PanelLeft Pencil
Play Plug Plus Power Refresh Route ScrollText Search Server Settings ShieldCheck Sliders Sparkles
Square Sun Tag Terminal Trash Users Wifi X Zap`.

### Misc
- `StatusDot { label: String, tone: StatusTone (Muted default, Success, Destructive, Warning), pulse }`
  — a dot **plus** the word. Never a bare coloured dot.
- `ConnectionDot { live: Signal<bool>, label: Signal<String> }` — for SSE panels.
- `Separator { class }` — `h-px w-full` by default.
- `Skeleton { class }` — a pulsing block.
- `Spinner { label: Option<String>, class }`.
- `ErrorNote { message: String, class }` — `role="alert"`; use for a failed *write*.
- `Mono { value: String, class }` — monospace, `break-all`.
- `RelativeTime { value: String, class }` — monospace timestamp, em dash when empty.
- `or_dash(&str) -> String` — free function, `"—"` for blank.

## Visual conventions

- Page heading: `PageHeader`. Section grouping: `Card` with `CardHeader`/`CardContent`.
- A list of records: `TableFrame` + `Table`. A list of *entities with several fields each*: `Card` per
  entity in a `grid gap-3 sm:grid-cols-2 xl:grid-cols-3`.
- Grids: `grid gap-3` (tight), `gap-4` (between cards). Breakpoints `sm:` `lg:` `xl:`.
- Form rows inside a card: `space-y-3`, fields as `<Field>`.
- Never write `rounded-lg border border-border bg-card p-5` by hand — that is `Card` + `CardContent`.
- Never write `bg-primary px-3 py-2 text-sm font-medium text-primary-foreground` — that is `Button`.
- Never use `text-3xl`/`text-4xl` for a section heading. `CardTitle` is `text-sm font-medium`, and a
  hero number is `font-mono text-2xl font-semibold`.
- Destructive actions use `ButtonVariant::Destructive` or `MenuItem destructive=true`.
- Every async write must show its failure. `Save::message()` gives the sentence; render `<ErrorNote>`.
  There is no path that reports success for a failed write.

## Verification

```bash
cd /home/nullbyte/Documents/GitHub/nullrouter
cargo check -p nullrouter-dashboard-wasm
cargo test  -p nullrouter-dashboard-wasm
```

Both must be clean. Preserve every existing `#[cfg(test)]` module and every doc comment that explains
a non-obvious decision; extend them rather than deleting them.
