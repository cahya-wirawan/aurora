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
- **`engineData.txt`** — also from `ag-psd`'s test suite. **Not raw wire-format
  data** — checked directly (`0 in data` is `False` for the whole file) and
  it turned out to be a human-readable pretty-print with no embedded
  UTF-16BE/BOM bytes at all, unlike the real format inside `tysh.bin`. Kept
  here only because it was genuinely useful for confirming `EngineData`'s
  key names and nesting shape while designing `engine_data.rs` — it is not
  used as parser test input. See `engine_data.rs`'s module docs.

All fetched from the two upstream projects' own test suites (both MIT
licensed), used here only for local format research and testing.
