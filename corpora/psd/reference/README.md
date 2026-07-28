# PSD/PSB test corpus — first, not final

**PLAN 0.7 / PRD §13 Step 6.** A real, structurally-diverse PSD/PSB corpus
to develop and regression-test `aurora-io`'s reader/writer against, seeded
*before* Phase 3 rather than after (PRD §13's ordering: assemble corpora
before writing parsers).

## What this is

319 real PSD/PSB files (`psd-tools-fixtures/`, gitignored — 50 MB, fetch
with `./fetch-samples.sh`) from
[psd-tools](https://github.com/psd-tools/psd-tools)'s own test suite
(`tests/psd_files/`), pinned to commit `ad89f31` for reproducibility.
**MIT-licensed** (psd-tools' repo-wide `LICENSE`; the fixtures carry no
separate license and fall under it) — redistribution as part of our own
gitignored, fetch-scripted corpus is unambiguous, the same diligence
already applied to the RAW/ICC corpus (`spike/raw-icc/reference/README.md`).

**One file excluded deliberately**: `third-party-psds/cactus_top.psd` sits
in a directory psd-tools' own maintainers named "third-party," with no
attribution or license note recoverable from the repo or its commit
history (`git log` on that path finds only an unrelated 2020 bugfix
commit). Silence isn't a license. Excluded from `manifest.txt` and the
fetch; everything else here is psd-tools' own authored fixture content.

`inventory.py` opens every fixture with `psd-tools` (Python, already a
project dependency for independent verification — see `spike/psd-write`)
and reports what's actually inside. Current result, `inventory.md`:
**272/272 psd+psb files open successfully** (the remaining 47 files are
`.png` reference composites psd-tools' own tests compare against — useful
to us too, as independent expected-output images, not something to "open").
Real coverage across every PSD color mode (RGB/CMYK/Lab/Grayscale/Indexed/
Bitmap/Duotone/Multichannel), most adjustment-layer types, smart objects,
shape layers, groups, type layers, and artboards.

## What this is not

**Not the 1,000-file real-world corpus the Phase 3 exit criterion names**
(PRD §9: "round-trip a corpus of 1,000 real-world PSDs... reopen in
Photoshop"). Two gaps, not one:

- **319, not 1,000.** A real first set, not the final count.
- **Authored test fixtures, not real-world client files.** These were
  built by psd-tools' maintainers specifically to exercise format edge
  cases (which is exactly why they're *more* useful than random real files
  for structural coverage — see `inventory.md`'s feature spread) — but
  none of them are an actual designer's or photographer's working file.
  The eventual real-world corpus is a harder, separate acquisition
  problem the RAW/ICC corpus didn't have: real PSDs are frequently
  client artwork, so sourcing 1,000 of them raises consent/licensing
  questions a public camera-sample archive (`raw.pixls.us`) never does.
  Don't treat this corpus as satisfying that gate — it satisfies "have
  something real to develop the reader against before Phase 3 starts,"
  which is what Step 6 actually asks for at this stage.

## Files

- **`manifest.txt`** — the 319 relative paths fetched, one per line; the
  actual list, not regenerated from GitHub at fetch time, so it stays
  stable even as psd-tools' `main` branch moves.
- **`fetch-samples.sh`** — re-fetches anything missing from `manifest.txt`
  at the pinned commit.
- **`inventory.py`** — opens every fixture with `psd-tools`, reports
  pass/fail and structural coverage (color modes, layer kinds). Regenerate
  with `python3 inventory.py > inventory.md` after re-fetching.
- **`inventory.md`** — generated output, committed (text, not binary) so
  the coverage claim above is checkable without re-running anything.
