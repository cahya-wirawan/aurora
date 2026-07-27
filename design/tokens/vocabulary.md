# Token vocabulary

**Draft, owned by Cahya Wirawan (PRD FR-027 Ownership).** This is the interface
every widget in `aurora-widgets`/`aurora-ui` will resolve against, per the PRD:
*"Design tokens — semantic, not literal ... so a theme redefines meaning
rather than patching individual widgets."*

This file names the semantic tokens and what each one means. It does not
assign colors — that happens once per theme in `../themes/*.toml`, which
point at the raw values in `palette.toml`. A widget must never reference
`palette.toml` or a literal value directly (PRD §7.3.10 / acceptance
criterion 3); it resolves only the names below.

Changing this file is a design decision, not a typo fix — see PLAN.md 0.5
and the CLAUDE.md note "don't invent tokens ad hoc."

## Surface

Background fills, back to front.

| Token | Meaning |
|---|---|
| `surface.canvas` | The neutral surround behind the document. User-settable independently of theme (hard requirement for color judgement) — this is its default. |
| `surface.app` | App chrome background, outside any panel. |
| `surface.panel` | Default panel background (Layers, Properties, Tool Options, ...). |
| `surface.raised` | Elevation 1: dropdowns, popovers, context menus. |
| `surface.overlay` | Elevation 2: modals, dialogs. |
| `surface.sunken` | Inset wells: text field bodies, color swatches, input backgrounds. |

## Text

| Token | Meaning |
|---|---|
| `text.primary` | Default UI text. Must hit 4.5:1 on every `surface.*` it appears on. |
| `text.secondary` | De-emphasized text (hints, metadata, captions). Must hit 4.5:1 on every `surface.*` it appears on — "secondary" means visual weight, not a license to fail contrast. |
| `text.disabled` | Disabled control labels. Exempt from the AA floor per WCAG (disabled content isn't required to meet contrast) but should still be legible; checked and reported, not gated. |
| `text.on_accent` | Text/label drawn on an `accent.*` fill (e.g. a filled primary button). |

## Icon

Mirrors `text.*` for glyph-based icons rather than label text.

| Token | Meaning |
|---|---|
| `icon.default` | Default icon color. |
| `icon.secondary` | De-emphasized icon (inactive tool, secondary action). |
| `icon.disabled` | Disabled icon. |
| `icon.on_accent` | Icon drawn on an `accent.*` fill. |

## Border

Non-text UI boundaries — WCAG 1.4.11 (non-text contrast) applies: 3:1 against
adjacent surfaces.

| Token | Meaning |
|---|---|
| `border.default` | Subtle dividers, panel edges, default input borders. |
| `border.strong` | Emphasized borders — active panel edge, pressed/selected state outline. |
| `border.focus` | Focus ring. Must hit 3:1 against every surface it can appear on (acceptance criterion 2) — checked independently of `border.default`/`strong` because it's a11y-load-bearing, not decorative. |

## Accent

The single brand/action hue. One hue for v1 (see palette.toml) — a second
accent axis is a post-Phase-0 decision, not assumed here.

| Token | Meaning |
|---|---|
| `accent.primary` | Primary action fill: primary buttons, active tool, selection highlight. |
| `accent.primary_hover` | Hover state of the above. |
| `accent.primary_active` | Pressed state of the above. |

## State

Transient interaction and status colors. Kept deliberately small — "elegance
is restraint" (PRD FR-027 Design Principles): every color difference must
encode a real difference in meaning, so this list should stay short as new
widgets are added, not grow one-off per component.

| Token | Meaning |
|---|---|
| `state.hover_overlay` | A translucent tint composited over any surface on hover — not a fixed color, so it works identically on every `surface.*`. |
| `state.active_overlay` | Same, for the pressed/active state. |
| `state.disabled_opacity` | Opacity multiplier applied to a control when disabled (not a color — composes with whatever colors the control already has). |
| `state.error` | Validation errors, destructive-action emphasis. Used sparingly — reserved for real errors, not decoration. |
| `state.warning` | Non-blocking warnings (e.g. lossy-save notice, PRD's itemized-warning requirement). |
| `state.success` | Rare confirmation (e.g. save/export completed). |

## Explicitly not yet decided

- A second accent hue / accent-family axis.
- Canvas-adjacent neutrality controls (user-settable surround) — token name
  reserved (`surface.canvas`) but the *setting* that overrides it per-user is
  application-level state, not a theme token, and isn't designed yet.
- Per-workspace theme pinning (FR-024) — consumes these tokens, doesn't add
  new ones.
- Icon set geometry itself (this vocabulary only covers icon *color*).
