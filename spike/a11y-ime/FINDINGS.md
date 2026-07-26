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

## Status: PARTIAL — the decisive test has not been run

**What is verified:** the stack builds, the accessibility tree is constructed
correctly, the platform adapter initializes without error, and a `wgpu` window
with custom-rendered text runs stably.

**What is not:** whether a screen reader actually *speaks* the field, and whether
CJK composition actually *works*. Both require a human — one to listen, one to
type. Nothing automated substitutes for either, and until they are done ADR 0001
is not de-risked. Do not treat this document as a green light.

## Stack

| Concern | Crate | Verdict so far |
|---|---|---|
| Accessibility | `accesskit` 0.24 + `accesskit_winit` 0.33 | Tree builds; adapter initializes |
| Text shaping/render | `glyphon` 0.12 (`cosmic-text` 0.19) | Renders; CJK glyph coverage untested |
| Windowing + IME | `winit` 0.30 | `set_ime_allowed` present; delivery untested |

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

## Human verification — REQUIRED, and outstanding on every platform

The ten items are in the app itself (`src/checklist.rs`); run it rather than
working from a copy here, which would drift. Results go in the table below as
they come in.

| Platform | Screen reader | Status | Result file |
|---|---|---|---|
| macOS | VoiceOver | **not run** | — |
| Windows | Narrator (UIA — a *different* API; macOS success does not carry over) | **not run** | — |
| Linux | Orca (AT-SPI) | **not run** | — |

**Interpreting a failure.** A failed item is not automatically the ADR 0001
escape-hatch trigger. The trigger is a *structural* failure — AccessKit or winit
cannot express the thing on that platform — as distinct from our code not doing
it yet. Most failures will be the latter and are simply bugs. Note which it
looked like; the generated report says the same thing so a tester does not have
to remember it.

## What this does not cover

Selection ranges and `TextSelection` in the accessibility tree; multi-line
editing; RTL and bidi; screen-reader-driven text navigation (by character, word,
line); high-contrast and reduced-motion OS settings; focus order across multiple
widgets. Each is real work, and none is validated here.
