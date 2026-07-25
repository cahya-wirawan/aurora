# Accessibility + IME spike — findings

ADR 0001's escape-hatch triggers. Built 2026-07-26 on macOS.

Reproduce:

```sh
cd spike/a11y-ime
cargo run -- --dump-tree   # tree construction, no window
cargo run                  # windowed; needs a human with a screen reader
```

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

## Human verification checklist — REQUIRED

Run `cargo run` and confirm, per platform:

**Screen reader**
- [ ] macOS VoiceOver (⌘F5): announces "Layer name, edit text, Background"
- [ ] Windows Narrator (⊞+Ctrl+Enter) — **untested, and Windows uses a different
      platform API (UIA); do not assume macOS success carries over**
- [ ] Linux Orca (AT-SPI) — untested
- [ ] Value changes are announced as you type
- [ ] Composition is announced while typing CJK, not just after commit

**IME**
- [ ] Pinyin: "ni hao" shows preedit inline, commits 你好
- [ ] Japanese: kana → kanji conversion, candidate window positioned at the field
      (`set_ime_cursor_area` should place it — verify it is not stuck at 0,0)
- [ ] Korean: jamo composition
- [ ] Dead keys (e.g. ´ + e = é) on a European layout
- [ ] The candidate window appears at the field, not the window corner

**If any screen-reader row fails structurally** — not "needs more code", but
"AccessKit cannot express this on this platform" — that is the ADR 0001
escape-hatch trigger, and the CXX-Qt fallback should be reconsidered before the
widget toolkit is written.

## What this does not cover

Selection ranges and `TextSelection` in the accessibility tree; multi-line
editing; RTL and bidi; screen-reader-driven text navigation (by character, word,
line); high-contrast and reduced-motion OS settings; focus order across multiple
widgets. Each is real work, and none is validated here.
