# Aurora design system — Phase 0 scaffold

This directory is PRD FR-027's Phase 0 deliverables (PLAN.md 0.5), owned by
Cahya Wirawan. Like `spike/`, it's deliberately outside the Cargo workspace —
nothing here is Rust, and none of it is consumed by any crate yet. It becomes
input to `aurora-theme` (M1.6) and `aurora-widgets` (M1.7) once Phase 1
starts; until then it's the place token and theme decisions get made and
reviewed.

**Everything under `tokens/` and `themes/` is a first draft**, not a
ratified design. Values were chosen to be a reasonable, contrast-checked
starting point — not guessed blindly, but also not something to treat as
final. Per CLAUDE.md: token names are a design decision to raise, not a gap
for anyone (including an assistant) to fill ad hoc — treat every file here
as something to actively revise, not rubber-stamp.

## Layout

| Path | What it is |
|---|---|
| `tokens/vocabulary.md` | The semantic token names and what each means — the interface widgets will resolve against. Read this first. |
| `tokens/palette.toml` | Primitive color ramps (raw hex). Themes reference these; widgets never do. |
| `tokens/scales.toml` | Type scale, spacing scale, radius, elevation, motion — shared across all themes and density modes. |
| `themes/dark.toml` | The one required built-in theme (FR-027 deliverable 3), mapping semantic tokens to palette values. |
| `check_contrast.py` | WCAG 2.1 AA check over `themes/dark.toml`'s resolved token pairs. Run after any edit to `tokens/palette.toml` or a theme file. |
| `build_tokens_css.py` | Generates `tokens.css` (CSS custom properties) from the TOML sources, for the HTML mockups/gallery below. The TOML is the source of truth — never hand-edit `tokens.css`. |
| `mockups/workspace.html` | Static mockup: main workspace with Layers/Properties/History docked (FR-027 deliverable 4). |
| `gallery/index.html` | Component gallery skeleton — the review surface and golden-image target (FR-027 deliverable 5). |

## Workflow

```sh
cd design
python3 check_contrast.py              # validate themes/dark.toml
python3 build_tokens_css.py > tokens.css   # regenerate after any token edit
```

Then open `mockups/workspace.html` or `gallery/index.html` in a browser —
no build step, no server needed.

**Important scoping note:** the HTML/CSS here is a Phase 0 review tool only.
Aurora renders its own UI on `wgpu` (PRD §8.3, ADR 0001) — nothing in
`aurora-widgets` will use CSS or a DOM. The token *values* and *names*
transfer directly; the CSS delivery mechanism does not.

## What's deliberately not here yet

- **Light, High Contrast (×2), and Color-Critical themes.** FR-027 only
  requires Dark to be complete for the Phase 0 gate; the others extend it
  once Dark itself is settled, per the theme-inheritance model.
- **A chosen variable font family** (`tokens/scales.toml` `[type].family` is
  a placeholder) — needs a font with a true variable weight axis; not
  blocking the rest of the token system.
- **Icon set geometry.** This scaffold only covers icon *color* tokens.
- **Scrollbar, tree, menu, curve editor** in the gallery — flagged inline in
  `gallery/index.html` rather than stubbed badly.
- **Outside critique** (PLAN.md 0.5's R2f mitigation) — get a second opinion
  on the mockups before the token vocabulary hardens into something every
  widget depends on.

## Status against PRD FR-027's five Phase 0 deliverables

1. Token vocabulary — drafted, `tokens/vocabulary.md`
2. Scales (type/spacing/radius/elevation/motion) — drafted, `tokens/scales.toml`
3. One complete built-in theme (Dark), contrast passing — drafted and
   verified, `themes/dark.toml` + `check_contrast.py` (all gated pairs PASS)
4. Static mockups — drafted, `mockups/workspace.html`
5. Component gallery skeleton — drafted, `gallery/index.html`

All five have a first pass. None are ratified — that review is next.
