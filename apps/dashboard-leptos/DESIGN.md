# nullrouter-dashboard-wasm Design System

## 1. Atmosphere & Identity

A compact dark operations console for 9Router parity work. The signature is warm orange routing accents over charcoal panels, with honest status badges that distinguish wired runtime state from preview-only dashboard surfaces.

## 2. Color

### Palette

| Role | Token | Dark | Usage |
| --- | --- | --- | --- |
| Accent/primary | `--brand` | `#e56a4a` | Primary route identity, selected nav, main actions |
| Accent/hover | `--brand-hover` | `#cc5236` | Primary action hover |
| Surface/page | `--bg-dark` | `#1a1a1a` | App background |
| Surface/card | `--surface-dark` | `#262626` | Cards and sidebar |
| Surface/field | `--surface-dark-2` | `#303030` | Inner rows, tiles, empty states |
| Surface/raised | `--surface-raised` | `#353535` | Higher emphasis controls |
| Border/default | `--border-dark` | `#333333` | Row and tile borders |
| Border/subtle | `--border-subtle-dark` | `#2a2a2a` | Card borders |
| Text/primary | `--text-main-dark` | `#ededed` | Headings and primary labels |
| Text/muted | `--text-muted-dark` | `#9ca3af` | Secondary copy |
| Status/success | `--ok` | `#5cc782` | Connected and available state |
| Status/info | `--info` | `#8ab4ff` | Informational previews |
| Status/warning | `--warn` | `#f7c76f` | Unwired or degraded state |

### Rules

- Keep the warm orange accent as the only brand color.
- Use status colors only for truthful system state.
- New local CSS must reuse these host stylesheet tokens.

## 3. Typography

### Scale

| Level | Size | Weight | Line Height | Tracking | Usage |
| --- | --- | --- | --- | --- | --- |
| H1 | `1.75rem` | 700 | 1.08 | 0 | Topbar title |
| H2 | `1.05rem` | 700 | 1.3 | 0 | Card titles |
| H3 | `0.95rem` | 700 | 1.35 | 0 | Tile titles |
| Body | `0.9rem` | 400 | 1.5 | 0 | Tile and panel copy |
| Caption | `0.76rem` | 500 | 1.4 | 0 | Status pills, metadata |

### Font Stack

- Primary: `Inter, -apple-system, BlinkMacSystemFont, "SF Pro Text", "SF Pro Display", system-ui, sans-serif`.
- The host serves a local Latin WOFF2 subset with `font-display: swap`; no runtime Google Fonts request is allowed.
- Mono: browser monospace for endpoint and model code values.

### Rules

- Keep labels compact and wrap safely on mobile.
- Do not introduce display-scale marketing type inside dashboard panels.

## 4. Spacing & Layout

### Base Unit

All spacing uses the existing 4px-derived dashboard rhythm.

| Token | Value | Usage |
| --- | --- | --- |
| Tight | `8px` | Inline icon and label gaps |
| Row | `10px` | List gaps and compact row padding |
| Card | `16px` | Card padding |
| Page | `24px` | Desktop workspace padding |

### Grid

- Shell: `288px` sidebar plus fluid workspace from the upstream `1024px` large breakpoint upward.
- Below `1024px`, the sidebar is an inert off-canvas drawer with an overlay; at and above `1024px`, the drawer controls are absent from the interaction path.
- Header padding is `16px` inline on compact viewports and `32px` at the large breakpoint, with `12px` top and `8px` bottom padding.
- Standard route content uses `24px` padding below `1024px`, `40px` at and above `1024px`, and a centered `1280px` maximum width.
- Panel grids collapse from multi-column to one column by their established route breakpoints; shell visibility changes only at `1024px`.

### Rules

- Use CSS grid for route panels.
- Avoid nested cards; inner repeated items are tiles or rows.
- The MITM page uses a `1120px` maximum content width and a single vertical card flow.
- MITM server and model rows use their multi-column anatomy from the upstream `640px` small breakpoint upward; below `640px`, labels, inputs, and actions stack without horizontal scrolling.

## 5. Components

### Dashboard Shell

- **Structure**: full-height flex shell, fixed `288px` desktop sidebar, fluid workspace, faint `40px` grid, sticky route header, scrollable content region, mobile overlay, and off-canvas drawer.
- **Desktop state**: sidebar remains visible and the workspace owns the only page scroll surface.
- **Compact state**: the closed drawer is `inert` and `aria-hidden`; overlay click, Escape, and successful navigation close it.
- **Accessibility**: landmarks are unique, controls expose expanded state and controlled element ids, focus never enters a closed drawer, and reduced-motion users receive immediate state changes.

### Sidebar Nav

- **Brand**: exact frozen copy `9Router Proxy` and `v0.5.20`, with the Material Symbols `hub` mark, warm shadow, and macOS traffic lights.
- **Primary order**: Endpoint & Key, Providers, Combos, Usage, Quota Tracker, Token Saver, CLI Tools.
- **System anatomy**: Media Providers accordion; Proxy Pools; Skills; retained MITM route; Console Log; conditional Translator; Remote; Settings.
- **Media children**: Embedding, Text to Image, Text To Speech, Speech To Text, Web Fetch & Search.
- **Structure**: compact row with the exact Material Symbols icon and label; abbreviation tiles are forbidden.
- **Variants**: default, hover, active, nested, accordion trigger.
- **States**: active uses `--brand` tint and filled icon; nested active state does not leave the parent ambiguous; the accordion chevron rotates only on state change.
- **Accessibility**: route controls update the URL hash/path state, accordion trigger exposes `aria-expanded`, and mobile navigation closes after selection.

### Header

- **Structure**: mobile menu button; route Material Symbols icon; title; desktop-only description; compact right-side controls.
- **Search**: icon trigger opens a keyboard-operable route search panel, autofocuses the input, filters completed dashboard destinations, shows a visible no-results state, supports Escape, and closes after navigation.
- **Language**: icon trigger opens a menu with the frozen locale inventory; choosing a locale updates the visible locale label locally without claiming server-side translation persistence.
- **Account menu**: `grid_view` trigger opens Change Log, Theme, Shutdown, and Logout rows. Change Log, Theme, and Shutdown remain visibly disabled; authenticated status enables Logout with pending/error announcements, while click-outside and Escape keep the frozen close behavior.
- **Reference extras**: Donate and theme controls may be present only when their visible behavior is complete and side-effect free; they must not replace search, language, or account controls.
- **Accessibility**: every icon button has an accessible name and tooltip, popovers have controlled ids and menu/dialog semantics, and only one header popover is open at a time.

### Card

- **Structure**: `.nr-card` with `.nr-card-head` and body rows or tiles; base surface uses a subtle border, `14px` radius, and `--shadow-soft`.
- **Variants**: base, elevated (`--shadow-elev`), hoverable (`--shadow-warm`), metric, provider group, settings, parity panel.
- **States**: empty and warning states are explicit in card body.
- **Accessibility**: headings remain semantic and copy states backend gaps plainly.

### Badge

- **Structure**: inline-flex pill with optional `6px` status dot and optional Material Symbols icon.
- **Sizes**: small `10px`, medium `12px`, large `14px`; all use semibold text and upstream padding.
- **Variants**: default, primary, success, warning, error, info.
- **Accessibility**: visible status text is mandatory; color and dot are supplementary.

### Button

- **Structure**: inline-flex command with optional left/right Material Symbols icon.
- **Sizes**: small `28px` high with `8px` radius; medium `36px` and large `44px` with `10px` radius.
- **Variants**: primary, secondary, outline, ghost, danger, success.
- **States**: hover, focus-visible, active `scale(0.97)`, disabled, and loading. Disabled commands never animate or advertise an available side effect.
- **Accessibility**: icon-only buttons require a tooltip and accessible name; loading state remains named.

### Status Pill

- **Structure**: rounded inline label with dot.
- **Variants**: connected, degraded, idle.
- **States**: status copy must not claim live execution unless host data is wired.

### Tile Row

- **Structure**: compact bordered row/tile on `--surface-dark-2`.
- **Variants**: provider, model, capability, placeholder.
- **States**: disabled or preview-only copy uses muted text and warning/info status.

### MITM Risk Banner

- **Structure**: one warning surface before the server card with the upstream warning icon and risk copy.
- **States**: always visible on the MITM route; the frozen warning emoji is retained as part of the source copy.
- **Accessibility**: uses an alert landmark and plain-language risk text.

### MITM Server Card

- **Structure**: one `.nr-card` containing the stopped status, Cert/Trusted/Server checks, purpose and flow rows, base URL, API key, action, and unsupported notice.
- **States**: server, certificate, and trust defaults are false; native inputs and Start Server are disabled while host control is unavailable.
- **Accessibility**: fields keep explicit labels, disabled controls remain discoverable, and false status is not communicated by color alone.

### MITM Tool Accordion

- **Structure**: a top-level tool tile with provider image, compact provider name, conditional server status, an accessible expand button, hosts rows, model mapping rows, and Start DNS.
- **Variants**: Antigravity, GitHub Copilot, and Kiro use their exact provider image paths, host lists, and mapping aliases.
- **States**: expansion is local UI state; a stopped server shows only `Server off`, while `DNS off` is retained for the future running-server state; mapping inputs, Select, and Start DNS remain disabled with an explanatory status.
- **Accessibility**: the header button exposes `aria-expanded`; images have provider alt text; every mapping input has an associated label; keyboard focus uses the brand token.

## 6. Motion & Interaction

### Timing

| Type | Duration | Easing | Usage |
| --- | --- | --- | --- |
| Micro | `120ms` | ease-out | Button and nav hover |
| Standard | `200ms` | ease-in-out | Drawer, menu, accordion, and toggle transitions |

### Rules

- Motion is limited to interactive affordances.
- Only `transform`, `opacity`, and `filter` animate; layout properties do not.
- `prefers-reduced-motion` removes nonessential transitions without hiding state changes.
- MITM accordion content changes visibility immediately; the chevron uses the upstream-style `150ms` rotation between settled states.

## 7. Depth & Surface

### Strategy

Mixed: subtle borders plus soft shadows. Cards use `--shadow-soft`; primary action and brand mark can use `--shadow-warm`. Inner tiles use borders and tonal shifts rather than additional shadows.

## 8. Accessibility Constraints

- Native disabled attributes are required for unsupported MITM inputs and actions.
- The mobile drawer is unreachable to keyboard and assistive technology while closed, and focus returns to the menu trigger after Escape or overlay dismissal.
- Header search, language, and account controls support keyboard open, navigation, selection, click-outside dismissal, and Escape dismissal.
- Long route labels and titles wrap or truncate without covering controls at `375px`, `639px`, `640px`, `768px`, `1024px`, and `1440px`.
- Every interactive element has a visible `--shadow-focus` focus ring with at least a `2px` visual boundary.
- Status labels include visible text such as `Stopped` and `Server off`; `DNS off` is shown only when server state makes that distinction meaningful, and color remains supplementary.
- The MITM layout must remain readable at 200 percent zoom and at `375px`, `768px`, and `1280px` viewports.
- Tool expansion must work with keyboard activation and expose its state to assistive technology.

## 9. Accepted Design Debt

- Live MITM certificate, server, DNS, and model mapping control is intentionally absent from this Rust/WASM slice.
- The page presents deterministic upstream defaults and disabled controls until a host API is explicitly implemented.
- Remote promotion, updater, shutdown, persisted locale, changelog network fetch, profile actions, and OIDC actions are not yet backed by Rust host services. Their shell entries must be omitted or visibly disabled; they may not perform privileged or external side effects.
