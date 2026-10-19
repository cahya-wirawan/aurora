# Aurora vs PhotoCraft: a headless, outside-in comparison

Stage 1 of comparing Aurora's performance with
[PhotoCraft](https://github.com/storytold/photocraft) (MIT or Apache-2.0,
pure Rust, wgpu + egui). Both tools are measured **by us, on the same machine,
with the same files**. PhotoCraft publishes almost no performance numbers, so
none of its own claims are used here.

This directory is outside the Cargo workspace (the root `Cargo.toml` excludes
`spike`), so nothing here can become a dependency of Aurora. PhotoCraft is
cloned and built in a work directory **outside the repository**. None of its
code is copied here, and it is not a dependency.

## What is measured

Each run is one process doing **open a file, flatten it, write a PNG**:

| tool | invocation |
|---|---|
| `aurora-export` | `aurora-headless IN --out OUT.png --threads T`: the app's own open routes, then its Save As PNG export composite (`composite_document`, one thread), `encode_by_extension` and the verified write |
| `aurora-pool` | the same, plus `--composite pool`: the same per-tile fold the background compositor runs, on a rayon pool of `T` threads (bit-identical to `aurora-export`, tested) |
| `aurora-gpu` | optional (`--tools`), `--gpu`: the canvas's GPU tile path, folding on the CPU where that path refuses |
| `photocraft` | `RAYON_NUM_THREADS=T photocraft-cli convert IN OUT.png`: its `files::open` then `files::save`, which flattens with `photocraft_compose::flatten_to_surface` (its CPU compositor) |

Both tools are timed from outside with the same tool: `/usr/bin/time -v` on
Linux, `/usr/bin/time -l` on macOS (peak RSS), plus this script's own wall
clock. Aurora also prints its own phase times (open = decode + install,
composite, encode, write) as one JSON object, which the harness records.

`aurora-headless` lives in `crates/aurora-app/src/headless.rs` (with the
binary `src/bin/aurora-headless.rs`). The open and export code is private to
`aurora-app`, and `aurora-cli` may not depend on `aurora-app`
(`scripts/layering.json`), so the binary is a second target of `aurora-app`.
That needs no layering change.

## How to run

```sh
# Everything: clone + build PhotoCraft (release), build aurora-headless (release),
# generate the 24 MP and 60 MP synthetic inputs, run idle and loaded, 3 rounds each.
spike/photocraft-compare/run.sh

# Pin PhotoCraft to a commit, choose the work directory:
spike/photocraft-compare/run.sh --commit 4d4de453058759d944731722c3e7c5f9524cf27a --work ~/pc-work

# Arguments after `--` go to compare.py, e.g. the whole PSD corpus, one run, idle only:
spike/photocraft-compare/run.sh --skip-build -- --set corpus --runs 1 --modes idle --tools aurora-export,photocraft

# Some files only:
spike/photocraft-compare/run.sh --skip-build -- --set none --files a.psd b.png
```

The defaults are: work dir `../aurora-photocraft-work` (beside the repository),
`--runs 3`, `--modes idle,loaded`, `--load 8` (`yes` processes, killed on
exit), `--timeout 300` s per run, `--threads` = the CPU count for both tools
(Aurora `--threads`, PhotoCraft `RAYON_NUM_THREADS`; `0` leaves each tool's
default), `--tools aurora-export,aurora-pool,photocraft`. `RESULTS_DIR=...`
chooses the output directory; `SYNTHETIC_SIZES=24,60` the generated sizes.

Results go to `WORK/results-<stamp>/`:

* `summary.md` has the tables: wall time and peak RSS as min-max over the
  measured runs, Aurora's phases, pixel checks and failures;
* `results.json` has every run (warm-ups too), with the command, exit status,
  stderr tail, Aurora's JSON and the output's SHA-256;
* `runs.csv` has the same data, one row per run.

Requirements:

* `git`, `python3` (stdlib only), `/usr/bin/time` and `yes`;
* network access to clone PhotoCraft and fetch its crates. Its `craft-ui`,
  `craft-libs` and `winprint.rs` are git dependencies on GitHub;
* Rust 1.97 (Aurora's pin). PhotoCraft declares `rust-version = 1.95`, and
  `run.sh` builds it with `cargo +1.97` (`PHOTOCRAFT_TOOLCHAIN` overrides this);
* 25 GiB free under the work directory. `run.sh` checks this. PhotoCraft's
  release target dir is several GiB and the 60 MP PSD is 645 MB. Delete the
  work directory when finished.

## Inputs

* **representative** (default): small to large corpus PSDs (groups, clipping,
  masks, effects, adjustments, blend modes, 16-bit, PSB), a JPEG and a PNG
  from the untracked `images/` folder, the Nyepi template (145 MB, effects
  and smart objects), and the synthetic files. Missing files are reported,
  not skipped;
* **corpus**: every `.psd`/`.psb` under `corpora/psd` (272 files), as
  enumerated by Aurora's `psd_fidelity_audit` (`images/` is not included);
* **synthetic** (`gen_synthetic.py`, stdlib only, written by neither tool): a
  3-layer 8-bit RGB PSD (raw channel data; Normal, a Multiply layer with
  varying alpha, and a Screen layer at 70 % opacity over the centre quarter)
  and a flat RGBA PNG, at 24 MP (6000 x 4000) and 60 MP (9486 x 6324). The
  PSD's merged image is the background only, so synthetic files have **no
  reference**. The ramps compress very well, so the PNG decode is not a
  stress test. These files go under `WORK/synth` and are never committed.

## Verification: speed is never reported for an unchecked picture

For each file in the idle mode, every tool's first output is decoded by
`pngread.py`, an independent stdlib PNG decoder (8/16-bit; 16-bit is
rounded to 8). Then:

* **tool vs tool**: dimensions, plus the mean and max absolute difference per
  channel in 8-bit levels. Colour is compared premultiplied, so the colour of
  a transparent pixel does not count. We also give the share of pixels with
  any channel more than 2 levels apart;
* **tool vs Photoshop**: for a PSD whose merged image holds real data (the
  audit's `audit_has_real_merged_data` check), Aurora writes the merged image
  (`--reference-out`, which is `decode_psd_merged_image_in_working_space`,
  the audit's reference) during its warm-up run. Each tool's output is put
  over white and compared like the audit, with score = mean of the R, G, B
  means.

Above `--verify-max-pixels` (6 M) pixels, every k-th pixel is compared.
The decode itself is always whole. Later runs are checked by SHA-256, and
the failures list says when a tool's output changes between runs.

## Fairness notes

* **Same machine, both release builds.** Aurora is built with the workspace's
  release profile (`panic = "abort"`, with debuginfo). PhotoCraft is built
  with its own release profile. Same threads for both (`--threads`).
* **Different feature coverage.** PhotoCraft's and Aurora's PSD support
  differ: effects, adjustments, smart objects, fill layers, text, and how
  each converts colour profiles. A file one tool renders more completely can
  cost that tool more time. Read every time next to the pixel checks and
  the reference score. Do not read the time alone.
* **Different work per run.** Aurora's export writes through its verified
  safe-save path: a temp file, `fsync`, a re-decode, a rename and a directory
  `fsync`. PhotoCraft writes its file directly. Aurora always writes an 8-bit
  dithered PNG. PhotoCraft's PNG keeps 8 or 16 bits by its own rules (its
  README: "PNG keeps 8/16-bit samples"). `results.json` records each
  output's bit depth (`verify.tools`). Aurora's
  `aurora-export` composites on one thread because that is what the app's
  export does today. `aurora-pool` shows the same fold on `T` threads.
  Aurora's phase times show where its time goes.
* **Peak RSS includes each tool's own caches.** For Aurora, that is the tile
  store's resident budget (a quarter of RAM, at most 8 GiB) and the effects
  coverage cache. It is not a measure of the smallest memory either tool
  could use.
* **Cold vs warm.** One warm-up run per tool and file comes first and is
  reported separately in `results.json`/`runs.csv`. After it, the file is in
  the OS page cache for every measured run. Aurora's warm-up also writes the
  reference image, which makes it longer. The rounds rotate the tool order,
  so neither tool always runs first.
* **Load.** The loaded mode runs `--load` busy `yes` processes. That is
  ordinary contention, not an adversarial setup. Both tools see the same load.
* **Never a single sample.** The tables give min-max over the measured runs.
  Treat a ratio of two single runs as noise.

## Not compared (stage 2)

UI frame rate, brush latency, and interactive pan/zoom are not compared
here. Those need the windowed apps, measured from outside: the Metal HUD
(`MTL_HUD_ENABLED=1`) on the Mac, and MangoHud on Linux. Filters,
adjustments as operations, RAW, and saving PSD are not compared either.
