# Reference fixtures

Real, Photoshop-authored files, used as ground truth instead of guessing at
binary layouts from spec text alone (see `FINDINGS.md` for why that
distinction mattered here specifically).

- **`text.psd`** — a psd-tools test fixture (`tests/psd_files/text.psd` in
  [psd-tools/psd-tools](https://github.com/psd-tools/psd-tools)), genuinely
  authored by Photoshop, containing one three-line text layer over a
  background layer. Not synthesized or hand-built.
- **`tysh.bin`** — the `TySh` (Type Tool Object Setting) tagged-block payload
  extracted from `text.psd`'s text layer via `psd-tools`, saved standalone.
  This is what `src/descriptor.rs` and `src/engine_data.rs`'s tests parse
  (its `EngineData` field, specifically, is the only *genuine* raw-wire-format
  `EngineData` payload in this directory). Extraction script is in
  `FINDINGS.md`; re-run it if this needs regenerating.
- **`tysh-single-run.bin`**, **`tysh-multi-run.bin`** — `TySh` blocks from
  [`Agamnentzar/ag-psd`](https://github.com/Agamnentzar/ag-psd)'s test suite
  (`test/text-simple.psd`, `test/text-test.psd`) — a second, independently
  written PSD library (unlike `psd-tools`, it also *writes* text layers).
  Whether these specific files are genuine Photoshop exports or ag-psd's own
  generated test data is unconfirmed either way; treated as "an independent
  codebase's understanding of the format," not as Photoshop-authored ground
  truth like `text.psd`. `tysh-multi-run.bin` has two separate style runs
  across two characters — a case `text.psd` doesn't cover (its one style run
  spans all 30 characters).
- **`tysh-paragraph.bin`** — a `TySh` block from `ag-psd`'s test suite
  (`test/read/text-paragraph-align/src.psd`). Every other fixture in this
  directory is **point text**; this one is **paragraph (area) text** —
  confirmed via `psd_tools`' `TypeLayer.text_type` before extraction, and
  independently re-confirmed inside Rust by
  `engine_data::tests::distinguishes_point_from_paragraph_text` (the
  point/paragraph distinction lives inside `EngineData`, not the outer `TySh`
  descriptor, so it can't be checked from `descriptor.rs` alone). Closes one
  of the two corpus gaps named in `FINDINGS.md` recommendation 4 / PLAN.md 0.6.
- **`tysh-warp-arc.bin`** — a `TySh` block from `ag-psd`'s test suite
  (`test/read/text-complex/src.psd`). Every other fixture in this directory
  has `warpStyle = warpNone`; this one has Layer > Type > Warp Text actually
  applied (`warpStyle = warpArc`, `warpValue = 50.0`, `warpRotate = Hrzn`).
  Closes the other named corpus gap.
- **`engineData.txt`** — also from `ag-psd`'s test suite. **Not raw wire-format
  data** — checked directly (`0 in data` is `False` for the whole file) and
  it turned out to be a human-readable pretty-print with no embedded
  UTF-16BE/BOM bytes at all, unlike the real format inside `tysh.bin`. Kept
  here only because it was genuinely useful for confirming `EngineData`'s
  key names and nesting shape while designing `engine_data.rs` — it is not
  used as parser test input. See `engine_data.rs`'s module docs.

All fetched from the two upstream projects' own test suites (both MIT
licensed), used here only for local format research and testing.

## `fonts/`

Not a PSD fixture — a font bundled for `src/glyph.rs`'s standalone glyph
rasterization proof (FINDINGS.md finding 13), so rendering is reproducible on
any machine or in CI rather than depending on whatever happens to be
installed on the host.

- **`DejaVuSans.ttf`** — from the [DejaVu Fonts
  project](https://dejavu-fonts.github.io/), Bitstream Vera License (see
  `DejaVu-LICENSE`) — permissive, redistributable, distinct from this
  project's own MIT licence but compatible with it. Chosen only for being a
  small, reliable, widely-available Latin font; not a statement about what
  font Aurora ships with. A real implementation renders whatever font the
  PSD's own `ResourceDict.FontSet` names, which this proof does not attempt
  to resolve (see `glyph.rs`'s module docs).
