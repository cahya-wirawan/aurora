# Findings: Aurora vs PhotoCraft, headless open -> flatten -> PNG (Linux)

Stage 1, measured on 2026-10-19 by `spike/photocraft-compare/`. The numbers come from these runs:
- the representative set at harness commit `642470e`: idle with 3 interleaved rounds, and loaded with 3 rounds under 8 `yes`. Its `summary.md` was regenerated at `8ae6ac3` (`compare.py --rewrite`) so its colour notes follow the corrected policy (Nyepi's from its stored-profile reference; `curve-02.jpg`'s reads "not recorded" there, and the colour-check run below records it); no timing or pixel number changed;
- the 272-file corpus pass at harness commit `8ae6ac3` (`9df7bd0`'s code plus a README edit): idle, 1 round. Its timed pipeline is the same as at `642470e`; `aurora-headless` only gained the `colour_converted_from` field and the `--gpu`/`--composite pool` refusal;
- a colour-check run of `curve-02.jpg` and the Nyepi template at `8ae6ac3` (idle, 3 rounds), which records what Aurora's open converted;
- the Nyepi colour-conversion attribution, marked where it appears.

Read README.md for the method and the fairness notes. `summary.md` in each results directory carries the full tables, the caveats and every failure.

## Machine and tools

| | |
|---|---|
| machine | Intel Core i3-10100 (4 cores / 8 threads), 62.5 GiB RAM, NVIDIA RTX 3090 (driver 560.35.03), Linux 6.8.0-136, python 3.11.5 |
| Aurora | 0.275.0, `aurora-headless` built at commit `642470e` (workspace release profile) |
| PhotoCraft | photocraft-cli 0.10.0, commit `4d4de453058759d944731722c3e7c5f9524cf27a`, built from source with `cargo +1.97 build --release -p photocraft-cli` (rustc 1.97.1) |
| threads | 8 for both (`RAYON_NUM_THREADS=8`; Aurora `--threads 8`) |
| invocation | `aurora-headless IN --out OUT.png` (`aurora-export`: the app's export composite, one thread; `aurora-pool`: the same fold on 8 threads); `photocraft-cli convert IN OUT.png` |

## What the timed run is, per tool

The two tools do not do the same work per run. Read every time next to these differences:
- **Aurora** runs its Save As PNG path: open (including the conversion to sRGB of a file tagged with a non-sRGB RGB profile; grey profiles are not applied), the composite, an 8-bit dithered encode, and a safe write (temp file, `fsync`, a re-decode, a rename and a directory `fsync`).
- **PhotoCraft** opens the file, flattens it with its CPU compositor, and writes the PNG directly. It keeps the source colour profile instead of converting it, writes RGB (no alpha) for opaque documents, and writes 16 bits for a 16-bit source.

## Results: wall time in seconds / peak RSS in MiB, min-max over 3 measured runs

"Check" summarises the pixel check: scores against Photoshop's merged image (A = Aurora's 16-bit verification output, P = PhotoCraft), or the A-vs-P difference. Every timed output, idle and loaded, had the same SHA-256 as the same tool's checked idle output. `aurora-pool`'s PNG is bit-identical to `aurora-export`'s on every file.

### Idle

| file | px | aurora-export | aurora-pool | photocraft | check |
|---|---|---|---|---|---|
| psd-tools/1layer.psd | 101x55 | 0.022-0.022 / 8 | 0.018-0.021 / 8 | 0.054-0.058 / 13 | PS: A 0.00, P 0.00 |
| psd-tools/group.psd | 100x200 | 0.024-0.056 / 9 | 0.023-0.054 / 9 | 0.057-0.114 / 14 | PS: A 0.00, P 0.00 |
| psd-tools/clipping-mask.psd | 360x200 | 0.043-0.090 / 13 | 0.039-0.092 / 15-16 | 0.064-0.117 / 18 | PS: A 0.00, P 0.00 |
| psd-tools/layer_mask_data.psd | 200x200 | 0.038-0.076 / 13 | 0.037-0.083 / 13 | 0.062-0.122 / 16 | PS: A 18.88, P 0.16 |
| psd-tools/layer_effects.psd | 800x640 | 0.220-0.255 / 45 | 0.130-0.165 / 68-75 | 0.182-0.215 / 108-120 | PS: A 5.76, P 0.06 |
| psd-tools/adjustments/curves_rgb.psd | 200x200 | 0.054-0.065 / 16 | 0.058-0.065 / 16-17 | 0.078-0.085 / 16 | PS: A 0.18, P 0.16 |
| psd-tools/blend-modes/rgb-blend-modes.psd | 530x512 | 0.161-0.220 / 79-80 | 0.147-0.190 / 89-90 | 0.187-0.205 / 36-40 | PS: A 0.16, P 0.18 |
| psd-tools/pen-text.psb (32-bit) | - | Aurora refuses | refuses | 0.082-0.084 / 69 | none |
| images/curve-02.jpg | 2548x1344 | 0.718-0.904 / 153 | 0.694-0.742 / 156-158 | 0.196-0.205 / 59 | colour spaces differ |
| images/instagram-01.png | 2387x1620 | 0.561-0.586 / 101-102 | 0.565-0.710 / 173-174 | 0.202-0.227 / 64-65 | A = P (max 0) |
| Nyepi template (145 MB, 277 layers) | 1080x1920 | 17.6-17.7 / 1785-1788 | 13.3-13.9 / 1817-1865 | 3.92-3.96 / 1744-1764 | PS: A 0.72, P 0.11 (each in its own colour space) |
| synthetic 24 MP PSD (258 MB) | 6000x4000 | 3.95-4.05 / 682-683 | 3.92-3.95 / 848 | 1.75-1.80 / 961 | A~P mean 0.03, max 1 |
| synthetic 24 MP PNG | 6000x4000 | 3.53-3.64 / 527 | 3.53-3.54 / 616-626 | 0.686-0.723 / 225 | A = P (max 0) |
| synthetic 60 MP PSD (645 MB) | 9486x6324 | 9.41-9.60 / 1586-1993 | 9.36-9.49 / 2025-2026 | 4.05-4.25 / 2392-2393 | A~P mean 0.03, max 1 |
| synthetic 60 MP PNG | 9486x6324 | 8.44-8.59 / 1239 | 8.33-8.41 / 1481-1559 | 1.48-1.52 / 520 | A = P (max 0) |

### Loaded (8 `yes`)

| file | aurora-export | aurora-pool | photocraft |
|---|---|---|---|
| psd-tools/1layer.psd | 0.031-0.036 | 0.034-0.047 | 0.146-0.149 |
| psd-tools/layer_effects.psd | 0.447-0.558 | 0.351-0.369 | 0.521-0.542 |
| psd-tools/blend-modes/rgb-blend-modes.psd | 0.536-0.637 | 0.529-0.687 | 0.899-0.920 |
| images/curve-02.jpg | 1.36-1.48 | 1.53-1.72 | 0.333-0.436 |
| images/instagram-01.png | 1.06-1.41 | 1.24-1.30 | 0.353-0.584 |
| Nyepi template | 41.1-42.3 | 34.3-35.3 | 9.71-9.93 |
| synthetic 24 MP PSD | 7.26-7.88 | 7.53-7.90 | 4.03-4.26 |
| synthetic 60 MP PSD | 17.0-19.1 | 17.4-18.0 | 8.94-9.74 |
| synthetic 60 MP PNG | 15.2-16.2 | 15.5-16.6 | 2.75-3.41 |

Peak RSS under load is within a few MiB of idle (for example `group.psd` 9 to 12 MiB, `1layer.psd` 8 to 9 MiB; within a few percent on the large files); the full table is in `summary.md`.

### Where Aurora's time goes (idle, its own phases, ms)

| file | open | composite (export / pool) | 8-bit dithered encode | safe write |
|---|---|---|---|---|
| Nyepi template | 11,446-11,591 | 5,738-5,749 / 1,656-1,896 | 178-180 | 48-55 |
| synthetic 60 MP PSD | 1,372-1,423 | 1,827-1,915 / 1,417-1,484 | 5,273-5,341 | 792-934 |
| synthetic 60 MP PNG | 1,243-1,333 | 1,025-1,106 / 787-861 | 5,188-5,229 | 867-932 |
| synthetic 24 MP PSD | 577-664 | 855-875 / 634-690 | 2,066-2,150 | 391-395 |
| curve-02.jpg (3.4 MP) | 291-390 | 52-63 / 31-32 | 283-369 | 70-80 |

- **The 8-bit dithered PNG encode is Aurora's largest cost on large flat documents.** At 60 MP it is 5.2-5.3 s of 9.4-9.6 s.
- **Nyepi's open is mostly the Adobe RGB to sRGB conversion.** A copy with its ICC resource (1039) renamed opened in 1.35 s, against 11.45 s with it. That is one sample each, measured with `aurora-headless` at `adc13a2` (same open code), so it is an attribution, not a ratio to quote. PhotoCraft does no conversion here.
- `aurora-export` composites on one thread, as the app's export does; `aurora-pool` cuts Nyepi's composite from about 5.7 s to 1.7-1.9 s.

## Pixel checks

**Dither-fair scoring.** Aurora is checked from a 16-bit output of the same pipeline, in one extra untimed run per file. The dither floor of its timed 8-bit output is shown separately: 0.40 on `16bit5x5.psd`, 0.26 on Nyepi, 0.37 on the synthetic PSDs, 0.008 on `1layer.psd`. Against Photoshop, dithering alone moved `16bit5x5.psd` from 0.000 (16-bit) to 0.347 (8-bit timed).

**Corpus, 216 files with a real merged image.** Score = the mean of the R, G, B mean errors in levels, over white, as in `psd_fidelity_audit`.

| output | median | <1 | <3 | <10 | >=10 |
|---|---|---|---|---|---|
| Aurora, 16-bit verification output | 0.053 | 174 | 13 | 17 | 12 |
| Aurora, 8-bit dithered (timed) | 0.111 | 174 | 13 | 17 | 12 |
| PhotoCraft | 0.000 | 206 | 2 | 5 | 3 |

- **The medians mostly measure rounding.** 81 Aurora (16-bit) scores and 114 PhotoCraft scores are 0.0000 at the 4 decimals `results.json` keeps (a mean error under 0.00005 level); 77 and 110 are pixel-identical to the reference (max 0). `summary.md` prints scores at 3 decimals, where 86 and 116 read 0.000. These counts are printed under "Derived counts" in the corpus `summary.md`. Compare the buckets and the per-file list, not the medians.
- **Per file** (Aurora 16-bit vs PhotoCraft, threshold 0.5 level): PhotoCraft is closer on 45 files, Aurora on 4, and they are within 0.5 on 167.

Largest gaps where Aurora is worse:

| file | Aurora | PhotoCraft |
|---|---|---|
| advanced-blending | 58.3 | 0.003 |
| stroke-effects | 29.9 | 0.31 |
| 4x4_16bit_rgb | 26.5 | 0.17 |
| clipping-mask2 | 21.5 | 1.33 |
| double-stroke-effects | 20.6 | 0.003 |
| layer_mask_data (.psd / .psb) | 18.9 / 19.1 | 0.16 / 0.14 |
| group-clipping | 18.5 | 0.0 |
| 4x4_16bit_grayscale | 16.8 | 0.31 |
| fill_adjustments | 13.2 | 0.84 |

Largest gaps where PhotoCraft is worse:

| file | Aurora | PhotoCraft |
|---|---|---|
| vector-mask2 | 4.7 | 69.5 |
| huesaturation_saturation_rgb | 0.41 | 7.88 |
| patterns | 0.02 | 3.32 |
| passthrough_fill_blendmode | 1.99 | 2.63 |

`dissolve` (66.2 / 66.7) is random by definition.

**Validation.**
- Aurora's buckets match its own `psd_fidelity_audit`'s recorded 175/13/17/12 to within one borderline file. Spot checks: the audit gives 18.88 and 5.76, the harness 18.876 and 5.758.
- PhotoCraft's layered renders are real composites: its PSD import reads the merged image only for flattened, multichannel or extra-channel files (`crates/io/src/psd_import.rs`, `merged_needed`); the synthetic PSDs, whose merged image is only the background, come out within 0.03 levels (max 1) of Aurora's.

**Tool vs tool on the corpus.** Both tools produced output on 235 files, and dimensions agree on all 235. PhotoCraft tags 23 of its outputs with a non-sRGB profile:
- 5 are Adobe RGB files that Aurora converted to sRGB at open. They are in different colour spaces and are not compared.
- 18 are grey files (the `*_grayscale` adjustment files, `gray0`/`gray1` (.psd/.psb), `gray-blend-modes`, the 8- and 16-bit `4x4_*_grayscale` and `stroke-color-descriptors-gray`). Aurora does not apply grey profiles, so both tools hold the same unconverted grey values, and these pairs are compared.

Of the 230 compared pairs, 57 have more than 1 % of pixels more than 2 levels apart. Six of those are grey files: `brightnesscontrast_grayscale` (mean 8.2 levels), `brightnesscontrast_legacy_grayscale` (5.2), `4x4_16bit_grayscale` (16.9), `stroke-color-descriptors-gray` (4.7), `levels_grayscale` (0.37) and `posterize_grayscale` (0.62). An earlier version of this section counted 23 not-comparable pairs and 51; it wrongly treated the grey files as converted by Aurora. All these counts are printed under "Derived counts" in the corpus `summary.md`.

**Colour spaces.** PhotoCraft keeps the source profile ("Adobe RGB (1998)" for Nyepi, "Generic RGB Profile" for `curve-02.jpg`). Aurora converts an RGB file with a non-sRGB profile to sRGB: the colour-check run's `colour_converted_from` is "Adobe RGB (1998)" for Nyepi and "Generic RGB Profile" for `curve-02.jpg`. It does not apply grey profiles. Each tool is scored against the reference in its own space. On Nyepi: Aurora 0.72 against the sRGB merged image, PhotoCraft 0.11 against the merged image as stored.

## Failures

- **Aurora refuses 37 of the 272 corpus files**, with `Decode(Unsupported...)` in every run (this includes `pen-text.psb` in the representative set): 17 are 32-bit, 13 CMYK (mode 4), 3 Lab (9), and one each of modes 0 (Bitmap), 2 (Indexed), 7 (Multichannel) and 8 (Duotone).
- **PhotoCraft opened all 272.** No timeouts or crashes, and no output hash changed between runs.
- **Selection bias:** the Photoshop reference is decoded by Aurora, so PhotoCraft's fidelity on the 37 files Aurora refuses is not measured.
- **An earlier 60 MP glitch is explained.** In a previous pass, PhotoCraft's warm-up output for the 60 MP PSD went missing before verification although the run exited 0. Cause: a shell `;` after a failed `cargo fmt --check` started a second, concurrent harness run; both shared the outputs directory and one deleted the other's file. Those passes and their timings were discarded. `compare.py` now takes a lock in the work directory and refuses to run twice.

## Caveats

These are also at the top of every `summary.md`:
- Aurora's safe write (fsync, re-decode, rename, directory fsync) is extra work compared with PhotoCraft's plain write.
- The encoders differ: Aurora's 8-bit dithered encode against PhotoCraft's encoder and bit depth. Aurora's PNGs are 3-9x larger on the synthetic files.
- `aurora-export` composites on one thread; `aurora-pool` runs the same fold on 8.
- Aurora's open converts non-sRGB RGB profiles to sRGB; PhotoCraft's does not. In the corpus these are 5 Adobe RGB files; in the representative set, Nyepi and `curve-02.jpg`. Aurora does not apply grey profiles.
- The page cache is warm: one warm-up run per tool precedes the measured runs.
- Peak RSS includes caches: Aurora's tile-store budget (a quarter of RAM, at most 8 GiB) and its effects cache; for `aurora-pool`, the headless command's buffer of composited tiles.
- Outputs are classed as sRGB by their ICC description, not by reading the profile.
- PhotoCraft is built from source at the pinned commit, not an official release binary.

## Not compared (stage 2)

- UI frame rate, brush latency and interactive pan/zoom. These would be measured from outside: Metal HUD (`MTL_HUD_ENABLED=1`) on the Mac, MangoHud on Linux.
- Filters and adjustments as operations, RAW, and PSD save.
- `aurora-gpu`: available through `--tools`, but not in these tables.

## Mac (Apple M2 Pro), same harness at `f8f428a`

Apple M2 Pro (10 cores), 32 GiB, macOS 26.6.2, python 3.9.6; Aurora 0.275.0 and PhotoCraft built in release at the same pinned commit; 10 threads; loaded = 10 `yes`; 3 measured runs per cell, one warm-up. Measured on the separate `aurora-diag` checkout. Full tables: the Mac's `summary.md` (representative set, idle and loaded) and the 272-file corpus pass.

Wall time in seconds, min-max over 3 runs (peak RSS in the full summary):

| file | idle export | idle pool | idle photocraft | loaded export | loaded pool | loaded photocraft |
|---|---|---|---|---|---|---|
| psd-tools/1layer.psd | 0.018-0.019 | 0.018-0.019 | 0.037-0.038 | 0.041-0.073 | 0.037-0.042 | 0.051-0.078 |
| psd-tools/layer_effects.psd | 0.095-0.096 | 0.069-0.073 | 0.105-0.108 | 0.151-0.253 | 0.187-0.245 | 0.22-0.231 |
| psd-tools/blend-modes/rgb-blend-modes.psd | 0.126-0.144 | 0.116-0.120 | 0.123-0.127 | 0.388-0.508 | 0.357-0.567 | 0.178-0.249 |
| images/curve-02.jpg | 0.291-0.302 | 0.285-0.291 | 0.121-0.128 | 0.373-0.473 | 0.396-0.451 | 0.144-0.171 |
| images/instagram-01.png | 0.230-0.234 | 0.222-0.226 | 0.121 | 0.274-0.354 | 0.318-0.344 | 0.147-0.158 |
| Nyepi template | 7.63-7.74 | 6.22-6.46 | 1.79-1.82 | 12.9-13.5 | 12.1-12.3 | 3.35-4.26 |
| synthetic 24 MP PSD | 1.60-1.61 | 1.49-1.52 | 0.674-0.682 | 2.25-2.31 | 2.47-2.58 | 1.28-1.41 |
| synthetic 24 MP PNG | 1.39-1.43 | 1.32-1.34 | 0.268-0.273 | 1.90-1.99 | 1.87-1.96 | 0.310-0.362 |
| synthetic 60 MP PSD | 3.78-3.85 | 3.48-3.53 | 1.45-1.52 | 5.37-5.53 | 6.18-6.32 | 2.68-2.80 |
| synthetic 60 MP PNG | 3.13-3.14 | 2.93-2.95 | 0.558-0.597 | 4.32-4.71 | 4.67-4.89 | 0.705-0.917 |

- The picture matches Linux: tiny files are a tie or Aurora is ahead; large and layered files, and flat PNGs, are 2-5x slower in Aurora (ranges above, not single-sample ratios). On this machine Aurora's 60 MP PSD is 3.8 s against 1.5 s, where Linux gave 9.4-9.6 s against 4.1-4.3 s.
- Aurora's phases on Nyepi (idle): open 5.06-5.16 s, composite 2.38-2.48 s (pool 0.96-1.16 s), encode 58-60 ms.
- Pixel checks and the corpus pass reproduce the Linux counts exactly: 235 pairs, dimensions agree on all, 5 converted, 18 grey compared, 57 of 230 over 1 %, 45 / 4 / 167 against Photoshop, 81 / 114 scores of 0.0000. Aurora refuses the same files (`pen-text.psb`: 32-bit).
- Mac peak RSS is within a few percent of Linux for the large files.

## Known gaps in this record

- `summary.md`'s "conversion is not recorded: 1" line counts `4x4_32bit_grayscale.psd`, a file Aurora refuses, so it is not a pair.
- The 3-decimal zero counts (86 / 116) are not printed in `summary.md`; the 4-decimal ones are.
- On the PSD path, `colour_converted_from` is set when the conversion is planned; if every layer's conversion then failed it would still name the profile (not seen in the corpus).
- Only the "no conversion" case of that field has a unit test; the converted case is shown by the colour check run.
- `--composite export --gpu` becomes GPU mode silently (export is the default).
