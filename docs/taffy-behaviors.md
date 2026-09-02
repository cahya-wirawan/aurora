# Verified `taffy` behaviors

Pinned to **`taffy 0.12.2`** (`Cargo.lock`). Every entry below was derived by
reading that exact version's source and confirmed by a real, measured
`compute_layout` result on a real `aurora_ui::build_workspace()` — not assumed,
and not carried over from a different taffy version or from CSS intuition.

**Why this file exists.** Four consecutive Gauntlet rounds in this session
(Layers, History, Properties, Dialogs — PLAN.md's M1.7/M1.8/M1.9 entries around
`0.77.0`–`0.77.7`) each independently rediscovered a taffy mechanic the previous
round's own review had already found, because the finding lived only in one
`doc` comment, not in a place a later round would think to search. One of
those doc-comment claims (`root_style`'s original "collapses the horizontal
auto margins" line) was **wrong** and shipped for a full patch version before a
later round's Red-team caught it by direct measurement. A single indexed
reference is cheaper than a fifth re-derivation, and safer than one crate's doc
comment being the only copy of a claim nothing else can check against.

**How to use this file.** Before writing a doc comment that asserts *why* a
`taffy::Style` resolves the way it does, check here first. If the mechanic you
need isn't listed, derive it from the pinned source yourself (cite the file
and line, and a real measured before/after) and add it here in the same
commit — don't let the finding live only in the widget's own doc comment.
If this file and a widget's doc comment ever disagree, re-verify against the
pinned source; don't assume either one is authoritative by default.

**When `taffy` is upgraded**, re-verify every entry below against the new
version's source before trusting it — a version bump is exactly the kind of
change that silently invalidates a specific line citation (this file's own
`root_style` history is a real example of a citation going stale across a
different kind of drift: pinning the wrong *version*, not the wrong *line*).

---

## 1. An auto-sized, childless root does not implicitly fill the viewport

There is no CSS-`body { width: 100% }`-style default. A `WidgetTree` root
built with `Style::default()` (no explicit `size`) stays at its content size —
zero, if it has no children yet — regardless of the window `compute_layout` is
given. `percent(1.0)` on both axes is an explicit opt-in every real root in
this workspace makes (`aurora_ui::workspace::build_workspace`'s own root
style), not a default `taffy` supplies.

- **Found:** M1.7 widget-toolkit build (`0.6x` era), before this document
  existed — see `PLAN.md`'s M1.7 section.
- **Consequence for test-writing:** a `taffy::Style::default()` test root is
  *auto-sized*. Any test that resolves a `percent(...)`-sized child against it
  needs a definite size on the root first (see entries 2–4 below, all of which
  depend on this).

## 2. Auto margins floor their share of free space at zero — they never go negative

When free space (`container_size − item_size`) is negative (the item
overflows its container), `taffy` computes the space to distribute among auto
margins as `free_space.f32_max(Size::ZERO)` **before** splitting it between the
margins. An overflowing item with auto margins on both sides of an axis is
therefore pinned to the **start** edge on that axis, not centered and not
symmetric — the floor removes the negative value the centering math would
otherwise need.

- **Source:** `taffy-0.12.2/src/compute/flexbox.rs:2274–2278`
  (`perform_absolute_layout_on_absolute_children`), the `free_space` line
  feeding the auto-margin split at `:2280` onward.
- **Measured:** a dialog overlay taller than its window, with `top`/`bottom`
  insets both `auto()` and `margin.top`/`margin.bottom` both `auto()`, pinned
  at `y: 0` regardless of overflow depth — the button stayed off-screen at every
  window height below the overflow point. Confirmed by reverting to this exact
  style and sweeping window heights 10–800px in an isolated scratch crate.
- **Found:** Dialog-overlay round (`0.77.7`), when a prescribed fix ("mirror
  the horizontal auto-margin centering technique vertically") was checked
  against this mechanism and found not to work — the horizontal case never
  overflows in this app (the dialog is 50% of window width), so the floor is
  never triggered there; the vertical case does overflow in a short window,
  where the floor becomes visible.
- **What actually centers under overflow:** entry 3.

## 3. `AlignItems::CENTER` / `align_self: Center` is an *Unsafe* alignment — it centers even under overflow, with no start-fallback

`taffy` tags every alignment keyword with a "safety" — `Safe` alignments fall
back to `Start` when the item overflows its container (so the item never goes
off the start edge); `Unsafe` alignments do not fall back, and can position an
item's origin at a negative coordinate relative to its container. `Center` is
`Unsafe`. Combined with a definite item size and no competing definite insets
on that axis, `align_self: Center` centers an absolutely-positioned item
symmetrically even when it's taller or wider than its containing block —
exactly the property entry 2's auto-margin floor lacks.

- **Source:** `taffy-0.12.2/src/style/alignment.rs:143`
  (`AlignItems::CENTER` is `{ keyword: Center, safety: AlignmentSafety::Unsafe }`);
  the fallback rule itself is `taffy-0.12.2/src/compute/common/alignment.rs:11–17`
  (`resolve_self_alignment_safety` only substitutes `Start` when
  `safety == Safe`); the actual position formula (which has no `.max(0.0)`
  clamp) is `taffy-0.12.2/src/compute/flexbox.rs:2467–2475`.
- **Measured:** switching a dialog overlay from entry 2's auto-margin approach
  to `align_self: Center` (both vertical insets `auto()`) moved the minimum
  working window height from **~73px down to ~35px** at a fixed dialog content
  height of ~89px — a real, swept, before/after measurement, not a theoretical
  improvement.
- **Found:** Dialog-overlay round (`0.77.7`).
- **Caveat, same round:** this only applies on the axis a flex item's
  `align-self` actually governs — the *cross* axis. On the *main* axis of its
  flex-direction parent, `align_self` is ignored; `justify_content` (default
  `Start`) governs instead, and that fallback is `Safe`-shaped by default. A
  `Column`-direction parent therefore needs the auto-margin technique (entry 2)
  on its main (vertical) axis regardless of `align_self`, which is why
  `aurora-widgets`' `dialog::root_style` keeps *both* mechanisms — `align_self:
  Center` for when it's a cross-axis child, auto vertical margins as the
  `Column`-parent main-axis fallback.

## 4. A definite pair of opposite insets, with the item's own size on that axis left `auto`, stretches the item to fill the containing block — and this does *not* disturb the other axis's auto-margin centering

If both `top` and `bottom` (or both `left`/`right`) are definite (not `auto`)
and the item's `size` on that axis is `auto`, `taffy` resolves the item's
extent on that axis by filling the gap between the two insets — the item is
stretched to the containing block's full size on that axis, not left at its
content size. This is the CSS "over-constrained absolute positioning" rule,
present in `taffy` too. Critically, this stretch is **axis-local**: it does not
reset or collapse a definite-inset auto-margin pair on the perpendicular axis.

- **Source:** `taffy-0.12.2/src/compute/flexbox.rs:2241–2246` — the
  height-from-insets fill-in fires specifically for the
  `(size: None, top: Some, bottom: Some)` shape.
- **Measured:** a dialog styled with `top: percent(0.15)`, `bottom:
  Dimension::ZERO` (both definite) and `height: auto()` resolved to
  `Rect { y: 90, height: 510 }` in an 800×600 test — full available height,
  not content height — while its *horizontal* centering (`left`/`right: ZERO`,
  `margin.left/right: auto()`, `width: percent(0.5)`) stayed exactly
  `Rect { x: 200, width: 400 }`, unaffected by the vertical stretch.
- **Found:** Dialog-overlay round (`0.77.6`, corrected `0.77.7`). The first
  version of `root_style`'s own doc comment claimed the stretch *also*
  "collapses the horizontal auto margins" — that clause was never measured and
  turned out to be false; only the axis actually given the definite inset pair
  is affected. Corrected once this was checked directly instead of assumed
  from CSS-adjacent intuition.
- **Why this matters for a modal overlay:** `bottom: auto()` (not `ZERO`) on
  the axis you want content-sized, paired with a definite `top` and definite
  `size`, is what keeps a dialog's height at its real content height instead
  of silently stretching to the full window on every layout call.

## 5. `min_size` on a flex item participates in its parent's layout on the item's own *main axis only* — and even there, it's a fallback, not a clamp

When taffy computes a flex item's base size, it reads `min_size` **on that
item's main axis** to decide whether to skip a real min-content
*measurement* of the item's subtree. If `min_size` on that axis is a definite
value, that value replaces the measurement outright — nothing descends further
and the parent's own base size can't inherit anything from below. If it's
`auto` (`None`), the code falls through (`unwrap_or`) to a real min-content
measurement, which recursively takes *each descendant's own* `min_size` (on
*their* main axis in their own parent's coordinate frame) as a floor on its
contribution — and that measurement can propagate upward as a real,
undesired floor on an ancestor's size.

Two consequences that are easy to get backwards:

- **Pinning `min_size` to a value (even zero) *stops* propagation on that
  item's own main axis** — a floor is what's being propagated; setting one
  explicitly (even to zero) short-circuits the measurement that would
  otherwise happen.
- **`min_size` on an item's *cross* axis is never read by this code path at
  all.** Pinning it there does nothing, however analogous it looks to the
  main-axis case.
- **This is inert on any item whose `position` is `Absolute`.** Absolutely
  positioned items are filtered out of ordinary flex-item generation entirely
  and laid out in a separate pass; their own `min_size` never participates in
  a flex parent's main-axis measurement at all, on either axis. `position:
  Absolute` genuinely removes an item from this mechanism — it isn't merely
  unlikely to matter, it structurally can't.

- **Source:** the main-axis-only read + fallback-not-clamp: `taffy-0.12.2/
  src/compute/flexbox.rs:817–820`, inside `determine_flex_base_size`. The
  absolute-item exclusion: `taffy-0.12.2/src/compute/flexbox.rs:527` (filtered
  out of flex-item generation) and `:2159–2258` (the separate absolute-layout
  pass).
- **Measured (the cross-axis miss):** pinning `min_size.width: ZERO` on
  `aurora-ui::panel`'s `root_style`/`body_style` (both `Column`-direction
  flex items, so width is their *cross* axis) did **nothing** — a real
  `build_workspace` at `compute_layout(1.0, 200.0)` still returned a
  21px-wide dock rail (one `row_height`) against a 1px window, with those
  pins in place. The floor was actually propagating through a `Column`
  *row*'s own `min_size.width` (its main axis, in *its* parent's frame), not
  through anything the panel-level pins touched.
- **Measured (the real fix, and the absolute-item exclusion holding):** the
  real fix — pinning `min_size.width: ZERO` on the dock rail itself
  (`aurora_ui::workspace::rail_style`, a `Row`-item of the window root, so
  width really is its main axis there) — closed it: the same probe returns a
  1px-wide rail. Separately, opening a dialog (an `Absolute` item with its own
  nonzero `min_size`) at window sizes far below that `min_size` produced
  **zero** change in the root/canvas/rail bounds at every size tested (1000px
  down to 40×40), confirming the absolute-item exclusion above rather than
  assuming it from the CSS spec.
- **Found:** Properties-panel round (`0.77.4`–`0.77.5`), the cross-axis miss
  and the real fix; Dialog-overlay round (`0.77.6`–`0.77.7`), the
  absolute-item confirmation, checked specifically because a naive reader
  might expect the *class* of bug just closed for panels to reopen here.

---

## Process note this file exists to reinforce

A prescribed fix that touches `taffy` layout is a **hypothesis**, not an
instruction, even (especially) when it "sounds analogous" to a fix that
worked somewhere else in this codebase. Two rounds this session (Properties,
then Dialogs) each had an orchestrator-prescribed fix turn out to be wrong
once actually measured against the pinned source — both times because the
analogy held on four of taffy's five relevant axes
(`display` / `position` / *which* axis / definite-vs-auto insets / alignment
safety) and silently failed on the fifth. Treat every claim above the same
way future ones should be treated: discharged by a source citation plus a
real measured before/after, or not trusted.
