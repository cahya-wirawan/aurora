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
  This is what `src/descriptor.rs`'s tests and `--tysh-demo` parse. Extraction
  script is in `FINDINGS.md`; re-run it if this needs regenerating.

Both are fetched from the upstream project's own test suite, used here only
for local format research and testing.
