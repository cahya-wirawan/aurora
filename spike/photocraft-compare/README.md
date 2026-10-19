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
| `aurora-export` | `RAYON_NUM_THREADS=T aurora-headless IN --out OUT.png --threads T`: the app's own open routes, then its Save As PNG export composite (`composite_document`, one thread), `encode_by_extension` and the verified write |
| `aurora-pool` | the same, plus `--composite pool`: the same per-tile fold the background compositor runs, on a rayon pool of `T` threads (bit-identical to `aurora-export`, tested) |
| `aurora-gpu` | optional (`--tools`), `--gpu`: the canvas's GPU tile path, folding on the CPU where that path refuses |
| `photocraft` | `RAYON_NUM_THREADS=T photocraft-cli convert IN OUT.png`: its `files::open` then `files::save`, which flattens with `photocraft_compose::flatten_to_surface` (its CPU compositor) |

`--threads T` sizes Aurora's PSD encode pool and its pool composite;
`RAYON_NUM_THREADS=T` is set for both tools, so any other rayon use is also
held to `T`. The app's export composite itself is single-threaded.

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

The idle mode checks every file's pictures with `pngread.py`, an independent
stdlib PNG decoder (8/16-bit; 16-bit samples are rounded to 8 bits, the same
rounding the 16-bit reference gets).

* **Aurora is checked from a 16-bit output.** Its timed runs write the 8-bit,
  *dithered* PNG its Save As writes. That is the right thing to time, but
  dithering adds about half a level of noise to every pixel, which would
  count against Aurora in a comparison with an undithered picture. So one
  extra, untimed verification run per file writes the same pipeline's output
  as a 16-bit PNG (`--depth 16`). That run also writes the reference. The
  **dither floor** (the timed 8-bit output against the 16-bit one) is
  reported next to it. The 8-bit output's own score is in the table too.
* **PhotoCraft is checked from its timed output** (its warm-up run).
* **Tool vs tool:** dimensions, plus the mean and max absolute difference per
  channel in 8-bit levels, with colour premultiplied, plus the share of
  pixels with any channel more than 2 levels apart.
* **Tool vs Photoshop:**
  * The reference is the PSD's own merged image, for files whose merged
    image holds real data (the audit's `audit_has_real_merged_data` check).
  * It is decoded by Aurora (`--reference-out`, which is
    `decode_psd_merged_image_in_working_space`, the audit's reference).
  * Each output is compared over white, like the audit. Score = the mean of
    the R, G, B mean errors.
  * The reference exists only for files Aurora opens, so files Aurora
    refuses (CMYK, Lab, 32-bit, ...) have no fidelity score for either tool.
* **Colour spaces:**
  * Aurora converts a file tagged with an RGB profile that is not sRGB to its
    sRGB working space, and writes sRGB. It does **not** apply grey
    profiles, or profiles it judges invalid: those files keep their stored
    values. `aurora-headless` reports what it converted from
    (`colour_converted_from`).
  * PhotoCraft (as measured at 4d4de45) keeps the source profile and embeds
    it (`iCCP`): "Adobe RGB (1998)" for Nyepi, "Generic RGB Profile" for
    `images/curve-02.jpg`.
  * `pngread.py` reads each output's ICC *description*, not the profile. An
    output with no profile, or with a description containing "sRGB", is
    treated as sRGB and scored against the working-space reference. An
    untagged output that is not really sRGB would therefore be mis-scored.
  * Other profiles are scored against the merged image as stored, which
    Aurora writes as `<ref>.stored.png` when its open converts colours.
  * A tool-vs-tool row is marked "not comparable" only when Aurora actually
    converted the file. When Aurora did not convert, both tools hold the
    file's own values, even though PhotoCraft tags them with the profile
    (for example a grey file), so the row is compared.
  * No colour conversion is done in Python.
* **Hashes:** every timed run's output, idle and loaded, is compared by
  SHA-256 with the same tool's idle warm-up output, which is the picture
  that was checked (for Aurora, via the dither floor). The timing tables'
  "pixel check" column shows the score, the agreement and this hash check
  for every row.

Above `--verify-max-pixels` (6 M) pixels, every k-th pixel is compared.
The decode itself is always whole.

## Fairness notes

`summary.md` repeats these at its top.

* **Same machine, release builds.** Aurora is built with the workspace's
  release profile (thin LTO, `panic = "abort"`, debuginfo). PhotoCraft is
  built from source at the pinned commit with its own release profile. It
  is not a release binary of theirs.
* **Different feature coverage.** PhotoCraft's and Aurora's PSD support
  differ: effects, adjustments, smart objects, fill layers, text, colour
  modes and depths. A file one tool renders more completely can cost that
  tool more time. Read every time next to the pixel check.
* **Different work per run.**
  * Aurora's export writes through its verified safe-save path: a temp
    file, `fsync`, a re-decode, a rename and a directory `fsync`. PhotoCraft
    writes its file directly.
  * Aurora writes an 8-bit dithered PNG. On large documents that encode is
    most of Aurora's time; see the phase table.
  * PhotoCraft's encoder and bit depth are its own. The pixel table lists
    each output's bit depth and size.
  * Aurora converts RGB profiles at open; PhotoCraft does not. Files where this
    happens carry a note; so do files whose profile Aurora did not apply.
  * `aurora-export` composites on one thread because the app's export does.
* **Peak RSS includes each tool's own caches.** For Aurora, that is the tile
  store's resident budget (a quarter of RAM, at most 8 GiB) and the effects
  coverage cache. `aurora-pool` also holds every composited tile until it
  assembles the image (a buffer of the headless command, not of the app).
* **Cold vs warm.** One warm-up run per tool and file comes first and is
  reported separately in `results.json`/`runs.csv`. The measured runs then
  read from the OS page cache. The 16-bit verification run is untimed and
  separate. The rounds rotate the tool order.
* **Load.** The loaded mode runs `--load` busy `yes` processes. Each sits in
  its own process group with a shell watchdog, so they die within a second
  even if `compare.py` itself is killed with SIGKILL.
* **One harness at a time.** `compare.py` takes a lock in the work
  directory. Two concurrent runs would share outputs and load each other.
  (Before the lock existed, an accidental second run deleted the other's
  60 MP output once.)
* **Never a single sample.** The tables give min-max over the measured runs.
  Treat a ratio of two single runs as noise.

## Stage 2: interactive frame rate (`stage2/`)

Stage 2 runs the two **live, windowed apps** and times their presented frames
from outside, the same way for both. Results and the full caveat list are in
`FINDINGS-stage2.md`.

### Method (Linux)

* **Display.** A private `Xvfb` server (`:99`, 2560 x 1440 x 24). Both apps
  render with the NVIDIA Vulkan driver on the real GPU (wgpu picks the RTX 3090
  on Xvfb; the log names the adapter) and present through the driver's X11 path
  into the Xvfb window: a GPU-to-CPU copy of each 1280 x 800 frame and an X
  image put. That copy is the same for both apps, but it makes every absolute
  number a property of this present path, not of a real monitor.
* **Same window.** Each app's toplevel is moved to (0, 0) and set to 1280 x 800
  (Aurora's own default size, scale factor 1) and given the input focus. No
  window manager runs.
* **Same input.** XTEST, from the harness (`python-xlib`), at a fixed schedule:
  * `pan`: Ctrl+1 (100 % in both), then a middle-button drag (both apps pan with
    it) along a circle of radius 250 px, one turn a second, 240 pointer events a
    second, for `--duration` s;
  * `zoom`: Ctrl+0 (fit in both), then wheel notches, 120 a second, alternating
    12 in and 12 out. Aurora zooms with the plain wheel; PhotoCraft with
    Alt+wheel (its plain wheel scrolls, as in Photoshop), so the harness holds
    Alt for PhotoCraft only. The zoom per notch differs between the apps;
  * `brush`: Ctrl+1, `B` (the Brush in both), then a left-button drag along a
    circle of radius 150 px, half a turn a second, 240 events a second. Each
    app's default brush size, hardness and spacing.
* **Same observer.** An X DAMAGE monitor on the app's window, in the harness
  process, records the monotonic time of every presented frame
  (`DamageReportNonEmpty`, subtracted on each event, so one event per
  present). Neither app's own counter is used, no layer is injected into
  either process.
* **Same present mode.** `--present novsync` sets wgpu's `AutoNoVsync` in both
  (Aurora: `AURORA_PRESENT_MODE`, an environment variable added for this
  measurement; PhotoCraft: its own `PHOTOCRAFT_PRESENT_MODE`). Xvfb has no
  vblank anyway.
* **Same open.** Aurora opens the file through `AURORA_OPEN` (environment
  variable added for this measurement; it calls the same `open_file` the
  File > Open command uses); PhotoCraft takes the path as its argument. The
  harness waits until the window title names the file and no frame has been
  presented for 3 s, pressing Return when the window is quiet to take a
  blocking dialog's default (Aurora: "Opened With Changes"; PhotoCraft:
  "Embedded Profile Mismatch", whose default keeps the embedded profile, as in
  stage 1). Every launch gets its own empty `HOME`, `TMPDIR` and XDG
  directories, so no preference, recovery file or layout carries over.
* **Metrics** per scenario: frames presented and frames per second over the
  input period plus the tail until the window is quiet; the distribution of
  frame-to-frame intervals (mean, p50, p95, p99, max, % above 16.7 ms); the
  time from each injected event to the next presented frame (p50, p95; an
  approximation of latency that includes X and the app's event handling);
  process CPU time over the scenario (100 % = one core); whole-GPU utilisation
  from `nvidia-smi` (other processes included).
* **A quiet machine.** Before every launch the harness waits until no
  `cargo`/`rustc`/`rustdoc`/`clippy-driver` process runs and, in idle mode,
  until the 1-minute load average is below `--max-load` (1.0), and records
  both (`machine_before`, `waited_s`, `gate_met`).
* **Vsync.** `--present fifo` runs the same scenarios with vsync on. On Xvfb
  it changes nothing (no vblank; the NVIDIA X11 path copies each frame), so a
  60 Hz cap can only be measured on a real display.
* **Runs.** `--runs` rounds, each launching every app once per file, the app
  order alternating; idle and with `--load` (8) `yes` processes. Every launch
  records the load average and the number of `cargo`/`rustc` processes before
  and after. Screenshots after each phase (`--screens`) show whether the
  picture actually moved, zoomed or was painted.

### How to run

```sh
python3 -m venv venv && venv/bin/pip install python-xlib
# Xvfb without root: apt-get download xvfb && dpkg -x xvfb_*.deb root/
LD_LIBRARY_PATH=root/usr/lib/x86_64-linux-gnu root/usr/bin/Xvfb :99 -screen 0 2560x1440x24 -nolisten tcp &
cargo build --release -p aurora-app --bin aurora-app
(cd photocraft && cargo +1.97 build --release -p photocraft)    # pinned commit, as stage 1
venv/bin/python spike/photocraft-compare/stage2/stage2.py --no-xvfb \
  --aurora target/release/aurora-app --photocraft WORK/target/release/photocraft \
  --files A.psd B.png ... --out WORK/s2 --runs 3 --modes idle,loaded \
  --duration 6 --wheel-rate 120 --zoom-burst 12 --screens
python3 spike/photocraft-compare/stage2/summarize.py WORK/s2/results.json > WORK/s2/summary.md
```

Without `--no-xvfb` the harness starts `--xvfb` itself on `--display`.

### Stage 2 on the Mac (procedure, not yet run)

The Mac reachable from the dev box has no GUI session, so this part is written,
not run. It needs a logged-in session and a Terminal with Accessibility
permission (for synthetic input) and `pip install pyobjc-framework-Quartz`.

1. Build both apps in release on the Mac (as above; PhotoCraft at the pinned
   commit with `cargo +1.97`).
2. For each file and app, alternating the app order, idle and loaded (8 `yes`):
   `stage2/mac_stage2.py --app aurora --bin target/release/aurora-app --file F --out OUT/<tag>`.
   It launches the app with `MTL_HUD_ENABLED=1 MTL_HUD_LOG_ENABLED=1` and
   the same present mode (`novsync`), records `log stream` of the Metal
   subsystem, places the window at 1280 x 800, presses Return for each app's
   post-open dialog, and injects the same pan, zoom and brush input through
   CGEvent (Cmd+1 / Cmd+0 instead of Ctrl; Option+wheel for PhotoCraft).
3. Check `ready.png`/`end.png`: the document must be open and the canvas must
   have moved. There is no outside "ready" signal on the Mac yet: raise
   `--open-wait` for Nyepi and the 60 MP files.
4. `mac_stage2.py --out OUT/<tag> --parse` prints the same statistics from the
   Metal HUD's per-frame log lines. **The HUD log's line format has not been
   checked**: read `hud.log` first and adjust `FRAME_RE`. If the HUD does not
   log per frame on that macOS version, the fair alternative is an outside
   ScreenCaptureKit stream of the app's window (frame-change timestamps,
   the Mac analogue of the X DAMAGE monitor); not written yet.
5. Both apps present through `CAMetalLayer` with the same present mode; check
   the HUD's reported refresh/present mode for each app before comparing.

## Not compared

Filters, adjustments as operations, RAW, and saving PSD are not compared.
Stage 2's limits are listed in `FINDINGS-stage2.md`.
