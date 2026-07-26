# Accessibility + IME spike — findings

ADR 0001's escape-hatch triggers. Built 2026-07-26 on macOS.

Reproduce:

```sh
cd spike/a11y-ime
cargo run -- --dump-tree     # accessibility tree, no window
cargo run -- --demo-report   # what a completed result file looks like
cargo run                    # the actual test; needs a human
```

**Running the test.** The window shows a ten-item checklist, one item at a time
with instructions. Answer each with **ctrl+Y** (pass), **ctrl+N** (fail), or
**ctrl+K** (skip); **ctrl+B** goes back. Plain typing and IME composition are
untouched by those bindings, so you can test the field while the checklist is
open. Press **Esc** to write `result-<os>.md`, which is meant to be pasted into
an issue or below.

Two things the program records for itself rather than asking, because a log
beats recollection: every `Ime::*` event received, and whether an assistive
technology ever actually connected (the platform only requests the tree when
something is listening).

## Status: macOS verified (9/10), Windows and Linux outstanding

**macOS/VoiceOver result, human-verified 2026-07-26, two consecutive runs:**
8–9 of 10 items pass consistently. Role, label, value, focus, screen-reader
navigation, and full CJK IME composition (preedit, candidate positioning,
commit) all work through a custom-rendered `wgpu` field. One real, reproducible
failure (`sr-live`, live value-change announcements) and one item left
unanswered by the tester's own judgment (`ime-deadkey`, inconclusive due to a
test-ordering mistake, and marked optional in the checklist itself). Full
results below.

**This is a strong result for ADR 0001 on macOS specifically.** It is not yet
a de-risked decision project-wide: **Windows (UIA) and Linux (AT-SPI) remain
completely untested**, and per the original caution in this document, macOS
success does not carry over — they are different platform APIs entirely.

## Stack

| Concern | Crate | Verdict |
|---|---|---|
| Accessibility | `accesskit` 0.24 + `accesskit_winit` 0.33 | Role/label/value/focus/nav verified via VoiceOver on macOS. Live value-change announcements broken (finding 6). Windows/Linux untested |
| Text shaping/render | `glyphon` 0.12 (`cosmic-text` 0.19) | Renders; CJK glyphs (preedit + committed 你好-class text) render correctly on macOS |
| Windowing + IME | `winit` 0.30 | `set_ime_allowed` + `set_ime_cursor_area` verified working — preedit, correctly-positioned candidate window, and commit all confirmed via VoiceOver + Pinyin on macOS |

## Findings so far

### 1. Accessibility constrains window creation order

`accesskit_winit` **panics if the adapter is created after the window is first
shown**:

> The AccessKit winit adapter must be created before the window is shown (made
> visible) for the first time.

So windows must be created hidden, adapted, then made visible. This is a
constraint on `aurora-app`'s window management, not on widget code, and it would
be irritating to discover after a window/tab/multi-window layer exists (FR-002
requires multiple windows). Worth encoding in the window-creation helper from the
start.

### 2. Everything a screen reader knows must be stated explicitly

The tree is hand-built: role, label, value, focus, and the label→field
relationship are all separate assertions. Nothing is inferred from drawing. This
is invariant §7.3.9 in practice, and it confirms the cost §8.3 accepted — for
every widget, forever.

Composition state needs announcing too, or a CJK user hears silence while typing.
The spike sets a description; the correct mechanism is richer (text selection and
composition ranges) and needs design work in `aurora-widgets`.

### 3. A text field is entirely ours

Cursor movement, backspace over multi-byte characters, and preedit handling are
all hand-written here. The backspace implementation has to be char-wise — a
byte-wise delete corrupts UTF-8 — which is exactly the class of bug a platform
toolkit would never have exposed us to. Multiply by selection, word motion,
double-click, drag-select, undo, clipboard, RTL, and bidi.

### 4. `cosmic-text` forced a toolchain bump

`cosmic-text` 0.19 requires rustc ≥ 1.89; the workspace was pinned at 1.88.
Bumped to 1.97 (current stable). Minor in itself, but it shows the text stack
sets the toolchain floor, and the original pin was already a year stale.

### 5. Custom `Role::Window` nested in a real window needs an explicit "interact" step

VoiceOver's plain arrow-key navigation (VO+Left/Right) does not descend into
our tree by default — both directions from the focused field landed on the
window's own title, skipping past the label entirely. The label *is* correctly
exposed (confirmed via the Rotor, VO+U, which lists it as "Layer name text"),
but reaching it via simple linear navigation requires VoiceOver's "interact"
command (VO+Shift+Down) first, the same command used for entering nested
groups or embedded web content.

Best guess at the cause: our tree root uses `Role::Window`
(`tree.rs`), and that node lives *inside* an already-real, OS-native window
that `winit` created. VoiceOver most likely treats this nested window-shaped
node as a sub-context requiring explicit interaction, rather than a plain
child to walk into automatically. Worth testing with a plainer root role
(e.g. a generic container/pane) instead of `Role::Window` for content that
already sits inside a native window — this may also be the cause of finding 6.

Diagnosed live via VoiceOver's Rotor (VO+U) after plain arrow navigation
became ambiguous — the Rotor lists every accessible element directly, which
turned out to be a far more reliable diagnostic than narrating step-by-step
arrow navigation.

### 6. Live value-change announcements don't reach VoiceOver — real bug, not structural

Typing into the field updates the on-screen text and the accessibility tree
correctly every keystroke (confirmed via debug logging: `value`, `self.focused`,
and `a11y_activated` are all correct on every keypress), but **VoiceOver never
announces the change**. Reproduced twice, including after a fresh relaunch and
an explicit mouse click into the field beforehand — not an artifact of prior
navigation state.

This was investigated down to the `accesskit_macos` 0.26.3 source
(`event.rs`): its `node_updated` diff handler does compare old vs. new
`value()` on any node and queues `NSAccessibilityValueChangedNotification`
when it differs — generically, not restricted to a specific role. On paper,
our case should trigger it. Since role, label, value, and focus all
demonstrably reach VoiceOver correctly elsewhere, this looks like a real,
narrow bug rather than a platform-level inability — plausibly connected to
finding 5's `Role::Window` nesting disrupting VoiceOver's normal
focus/notification tracking, though this is not confirmed.

**Per the report's own framing, this is *not* ADR 0001's escape-hatch
trigger** — it is a specific, investigatable implementation gap, with role,
label, and IME composition all independently proven to work through the same
stack.

## Human verification results

The ten items are in the app itself (`src/checklist.rs`); run it rather than
working from a copy here, which would drift.

| Platform | Screen reader | Status |
|---|---|---|
| **macOS** | **VoiceOver** | **9/10 — see below** |
| Windows | Narrator (UIA — a *different* API; macOS success does not carry over) | **not run** |
| Linux | Orca (AT-SPI) | **not run** |

### macOS / VoiceOver — human-verified 2026-07-26

Two consecutive full runs, consistent results.

| Item | Result |
|---|---|
| `sr-window` — screen reader announces the window at all | PASS |
| `sr-role` — field announced as an editable text field | PASS |
| `sr-label` — "Layer name" announced with the field | PASS (via Rotor; see finding 5 for the navigation-depth caveat) |
| `sr-value` — current field value announced | PASS |
| `sr-live` — typing announces the value change | **FAIL — finding 6** |
| `sr-nav` — field reachable by screen-reader navigation | PASS (with the finding 5 caveat: requires "interact", not plain arrows) |
| `ime-preedit` — inline composition while typing CJK | PASS |
| `ime-candidates` — candidate window at the field, not a corner | PASS |
| `ime-commit` — commit inserts the composed text correctly | PASS |
| `ime-deadkey` — dead-key accent composition | Not answered — optional, and the one attempt was confounded by the Pinyin IME still being active (see below) |

**8–9 passed, 1 failed, 0–1 inconclusive.**

Two things worth recording about how the data was gathered, since they'll
matter for whoever runs the Windows/Linux legs:

- The IME commit test produced literal `"nihao"`/`"nihau"` rather than 你好 —
  this is **correct, expected behavior**, not a bug: macOS's Pinyin IME
  commits the raw Latin text when Enter is pressed without picking a
  candidate from the popup. Confirmed by re-running with a typo (`nihau`)
  and getting the identical raw-text-back pattern.
- The `ime-deadkey` attempt showed a preedit of `"e"` that cleared with no
  commit, followed by a `€` landing in the field via the *plain* keyboard
  path (absent from the IME event log entirely). Best explanation: the
  Pinyin IME was still active from the prior test, so Option+e was
  interpreted as the start of a new Pinyin syllable rather than the OS's
  Latin dead-key mechanism. A clean retest would switch back to a plain
  US/ABC input source first. Left unanswered rather than guessed — the
  right call, and the item is marked optional in the checklist anyway.

**Interpreting a failure.** A failed item is not automatically the ADR 0001
escape-hatch trigger. The trigger is a *structural* failure — AccessKit or winit
cannot express the thing on that platform — as distinct from our code not doing
it yet. `sr-live` (finding 6) is judged the latter. Windows and Linux may surface
genuinely structural failures that macOS did not; that possibility remains open.

## What this does not cover

Selection ranges and `TextSelection` in the accessibility tree; multi-line
editing; RTL and bidi; screen-reader-driven text navigation (by character, word,
line); high-contrast and reduced-motion OS settings; focus order across multiple
widgets. Each is real work, and none is validated here.

## Recommended follow-ups

1. **Run the Windows and Linux legs.** The one thing that would actually settle
   ADR 0001 project-wide. Different platform APIs (UIA, AT-SPI) — nothing here
   predicts the outcome.
2. **Try a plainer root role than `Role::Window`** (finding 5) as a quick
   experiment — a generic container/pane, since our tree already lives inside
   a real native window. If this also fixes finding 6, that confirms a single
   shared root cause rather than two separate bugs.
3. **Re-run `ime-deadkey`** with a plain Latin input source active throughout
   (no CJK IME switched on beforehand) — cheap, and closes the one
   inconclusive item.
4. **If Windows or Linux surface something *structural*** — AccessKit or the
   platform genuinely cannot express role/label/focus/IME, as opposed to a
   fixable gap like finding 6 — that is the point to revisit ADR 0001's
   CXX-Qt fallback. Nothing found on macOS meets that bar.
