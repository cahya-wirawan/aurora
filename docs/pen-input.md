# Pen input contract (task 169)

Aurora reads a pen through one type, `aurora_core::PenSample`
(`crates/aurora-core/src/pen.rs`): position (physical window pixels),
`pressure` 0..1, `tilt_x`/`tilt_y` in degrees (-90..90, 0 = upright, W3C
Pointer Events signs), `rotation` in degrees (0..360), and a `PointerSource`
(`Mouse`, `Touch`, `Pen`). `PenSample::new` sanitises: non-finite values
become neutral (pressure 1, tilt 0, rotation 0, position 0), others are
clamped, rotation wraps. A mouse sample (`PenSample::mouse`) has pressure 1,
so a consumer that ignores pressure behaves as before.

## What is wired

* `WindowEvent::Touch` is mapped by `aurora-app/src/pen_input.rs` and drives
  the same handlers as the mouse (Started = press, Moved = move, Ended =
  release). Cancelled interrupts the gesture exactly like `CursorLeft`
  (a live stroke keeps its undo step).
* Exactly one source owns a gesture: one touch id at a time (other fingers
  are ignored); a touch cannot start while a mouse button is down, and mouse
  events are ignored while a touch is active.
* Losing window focus forgets held mouse buttons and the active touch, so a
  lost release or `Ended` cannot lock either source out; a touch gesture live
  at that moment is interrupted like a cancelled one. A late `Ended` for it
  is ignored.
* A touch release clears the hover highlights and any tooltip (a lifted
  finger or pen has no hover position). A touch does update hover while it
  moves, as the mouse does.
* `App::pen_sample` holds the latest sample and is read at `paint_dab`.
  **Nothing maps pressure or tilt to size, opacity or flow yet**; that is a
  design decision for the design owner.

## Force mapping

`Force::Normalized(f)` gives pressure `f`. `Force::Calibrated` gives
`force / max_possible_force`; its `altitude_angle` gives only the tilt
magnitude (90 degrees minus the altitude), put on `tilt_x`, because winit
reports no azimuth. No force gives pressure 1. winit reports no rotation.
A pen delivered as touch cannot be told from a finger, so the source is
`Touch`.

## Platforms through winit 0.30.13

* Windows: pen and touch arrive as `Touch` with `force` (documented by
  winit for Windows 8+). Not verified here, no tablet.
* macOS, X11, Wayland: winit 0.30 documents no pen data (macOS `Touch` is
  unsupported). A tablet there is a mouse. Not verified, no tablet.

## Next backends

* octotablet for Windows Ink and Wayland tablet protocol (tilt, rotation,
  eraser end, hover).
* macOS `NSEvent` tablet fields (pressure, tilt, rotation) via platform glue.
* winit 0.31 when stable and when `accesskit_winit` supports it.
