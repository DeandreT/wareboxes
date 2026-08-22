# Web operations visual system

`web-ops` is a desktop operations console. Its interface favors scan speed,
side-by-side evidence, and high information.

## CSS ownership

The cascade order is declared once and must remain:

```text
reset → tokens → base → features → components → layouts → utilities
```

- `style/main.css` owns the reset, global tokens, and base HTML controls.
- `public/workbench.css` is the only feature-style entry point. Feature sheets are
  imported into the `features` layer.
- `public/presentation.css` owns shared components, shell behavior, semantic
  variants, theme and density preferences, and final layout contracts.
- `public/workspace-layout.css` owns the reusable split-pane layout.

Feature styles must be rooted under their workspace class. Do not introduce an
unqualified shared-looking selector such as `.status`, `.form-grid`,
`.danger-action`, `.modal-panel`, `.empty-detail`, or `.inline-command-error` in a
feature sheet. Extend the shared component or use a feature-prefixed class.

## Semantic tokens

Use the canonical variables instead of literal colors:

- Text: `--ink`, `--ink-soft`
- Surfaces: `--surface`, `--surface-raised`, `--surface-subtle`
- Borders: `--line`, `--line-strong`
- Selection and information: `--blue`, `--blue-soft`
- Success: `--green`, `--green-soft`
- Attention: `--amber`, `--amber-soft`
- Danger: `--red`, `--red-soft`
- Geometry: `--control-height`, `--radius-sm`, `--radius-md`, spacing tokens

Legacy aliases remain temporarily available, but new work should use the
canonical names. Theme rules change tokens, not individual feature selectors.
Density rules change shared control and table metrics.

## Workspace patterns

Choose one scroll owner:

- A normal page lets `.workspace` scroll.
- A viewport workbench fills the workspace and scrolls inside its panels.
- Master/detail workflows use `SplitPaneState`, `PaneControls`, and
  `SplitPaneHandle`; they must remain usable when either pane is collapsed.

Avoid page-local `calc(100vh - …)` sizing. The application shell owns `100dvh`
and provides the remaining height to the route. Prefer container queries because
the sidebar and navigation preference change available workspace width.

Tables should have a concise caption, sticky headings inside a `.table-scroll`,
an explicit empty/loading/error row, and a visible disclosure control for selected
detail. Do not make a row's only action mouse-dependent.

## Interaction and accessibility

- Use semantic status variants: `success`, `info`/`active`, `warning`, `danger`,
  or `neutral` on one of the shared status classes.
- Preserve a visible focus indicator for every interactive element.
- Dialogs require `role="dialog"`, `aria-modal="true"`, and a programmatic label.
  The presentation initializer supplies focus entry, trapping, Escape close when
  a recognizable close action exists, scroll locking, and focus restoration.
- Dynamic failures use `role="alert"`; nonurgent progress uses `role="status"`.
- Icon-only controls need an accessible name and at least a 28px desktop target.
- Neutral row selection is blue. Amber and red are reserved for operational
  conditions, warnings, and destructive actions.

## Verification matrix

For any shell or shared-component change, verify representative routes at 1024,
1280, 1440, and 1920 CSS pixels; at 768, 900, and 1080 pixel heights; in light
and dark themes; in compact and standard density; with navigation shown and
hidden; with keyboard-only navigation; and with reduced motion. Browser checks
must run headlessly or on an isolated virtual display.
