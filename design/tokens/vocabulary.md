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
| `border.control` | The colour of a mandatory outline drawn around every interactive control (buttons, checkboxes, sliders, text fields, the command palette panel, colour swatches) — invisible by default (`border.control_opacity` at `0`), opaque in the two High Contrast themes, where relying on fill-colour contrast alone isn't enough. |
| `border.control_opacity` | Opacity multiplier for `border.control`'s stroke — `0.0` in every theme except the two High Contrast ones, where it's `1.0`. A theme toggle, not a widget-state one (contrast `state.disabled_opacity`, which is about a control's own enabled/disabled state, not the active theme). |

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

## Selection and Quick Mask

Added in 0.219.0 (design owner Cahya's decision; the values were chosen by
Claude on Cahya's delegation, not seen by eye). Identical in every built-in
theme: the ants must read on any image, so they do not follow the surfaces.

| Token | Meaning |
|---|---|
| `selection.ants_light` | The light dash of the marching-ants selection outline. Pure white (`hc.white`: the palette's only pure white; the `neutral` ramp ends at `#f5f5f6`). |
| `selection.ants_dark` | The dark dash of the outline. Pure black (`hc.black`). The pair is gated at 3:1 against each other (21:1 as chosen); each is never gated against a surface, because the two tones together are what stay visible on any backdrop. |
| `quick_mask.overlay` | The Quick Mask tint over the unselected area: a `{ color, alpha }` table like `state.hover_overlay`. `semantic.red.500` at `0.5`, Photoshop's red 50 %. The alpha is the tint's *maximum* (a fully excluded area); partial coverage draws proportionally less. Not contrast-gated (a tint, not content). |
| `crop.shield` | The Crop tool's shield over the canvas outside the crop rectangle (0.227.0): a `{ color, alpha }` table like `quick_mask.overlay`. `hc.black` at `0.5`, identical in all five themes (design owner, Cahya). Drawn flat, beneath the selection ants and the crop handles. Not contrast-gated (a dimming, not content). |

## Explicitly not yet decided

- A second accent hue / accent-family axis.
- Canvas-adjacent neutrality controls (user-settable surround) — token name
  reserved (`surface.canvas`) but the *setting* that overrides it per-user is
  application-level state, not a theme token, and isn't designed yet.
- Per-workspace theme pinning (FR-024) — consumes these tokens, doesn't add
  new ones.
- Icon set geometry itself (this vocabulary only covers icon *color*).
