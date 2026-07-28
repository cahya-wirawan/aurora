# RAW decode + ICC transform spike — findings

PLAN 0.6. Built 2026-07-28.

Aurora needs Camera RAW decode from major vendors (FR-015) and ICC color
transforms (FR-016). PRD §8.2 posed both as pure-Rust vs. FFI, and §14 named
a licensing risk it associated mainly with the FFI side — LibRaw is
LGPL-2.1/CDDL, `libheif` is LGPL, "prefer the pure-Rust alternative... even
at some capability cost." This spike tested that framing against real
libraries, real camera files, and real ICC profiles, rather than assuming it
still holds.

```sh
cd spike/raw-icc
./reference/fetch-samples.sh   # downloads 3 real RAW files (~50 MB, gitignored)
cargo run -- raw                # decodes all 3, writes preview PPMs to out/
cargo run -- icc                 # cross-validates lcms2 vs moxcms on real ICC profiles
```

## Verdict: RAW decode works well on all three major vendors, but the licensing picture is more complicated than the PRD assumed — for RAW, not ICC

## 1. `rawler` decodes real Canon/Nikon/Sony files correctly — visually confirmed, not just "returned Ok"

One real, unedited file per vendor from
[raw.pixls.us](https://raw.pixls.us) (the same public archive
`rawspeed`/`darktable`/`RawTherapee` use for testing — see
`reference/README.md`):

| Vendor | Camera | Format | Result |
|---|---|---|---|
| Canon | EOS M200 | CR3 (current container format, not legacy CR2) | 6288×4056, RGGB, range [482, 16383] |
| Nikon | 1 J1 | NEF | 3904×2606, RGGB, range [0, 4037] |
| Sony | DSC-RX1 | ARW | 6048×4024, RGGB, range [475, 16380] |

All three decoded on the first attempt: correct sensor dimensions, correct
camera model string, correct CFA pattern, plausible per-camera bit depth
(the Nikon 1 J1's 4037 max vs. the other two's ~16383 is a real sensor
difference, not a decode error — its bit depth is genuinely lower).

**Not just "decode_file returned Ok" — this spike's own established
discipline (finding 1 of the PSD spike: a reader accepting a file proves
little) applied here too.** Every file's raw sensor data was rendered to a
crude "R=G=B from the mosaic" preview PPM (same technique as `rawler`'s own
doc example) and inspected directly:

- Canon: a rooftop/skyline scene, correctly dark (raw linear sensor data,
  no exposure/gamma applied yet — expected).
- Nikon: a close-up of wooden prayer beads, carved detail clearly visible.
- Sony: a city skyline (San Francisco), fine architectural detail visible.

All three are unambiguously real photographs, not noise or a garbage
buffer. First bug caught in the process, in this spike's own preview code,
not `rawler`: a fixed `>>8` bit-shift assumed 16-bit sensor range, which
left the Nikon file (max value 4037, not ~65535) looking solid black even
though the decode was correct — fixed by stretching each preview by that
file's own actual [min, max] instead of a fixed shift.

## 2. `rawler` — the flagship pure-Rust RAW decoder — is itself LGPL-2.1

**This directly contradicts the framing in PRD §14 and this project's own
`deny.toml` comments**, both of which discuss LGPL risk only in terms of
the *FFI* library (LibRaw) and treat "prefer the pure-Rust alternative" as
if it sidesteps the licensing question. It doesn't, for RAW specifically:

```
$ cargo info rawler
license: LGPL-2.1
```

Confirmed directly from the crate's own published metadata, not inferred.
Checked the other pure-Rust alternatives cargo surfaces for "RAW decoder"
too, rather than assuming `rawler` was uniquely unlucky:

| Crate | License | Note |
|---|---|---|
| `rawler` | **LGPL-2.1** | Full decoder, all major vendors — the capable one |
| `zenraw` | AGPL-3.0 or commercial | Worse than LGPL, not viable |
| `raw_preview_rs` | GPL-3.0 | Copyleft, and preview-only in scope |
| `rawlib` | MIT | Permissive, but thumbnail-extraction only — not a real decoder |

**No permissively-licensed, full-featured Rust RAW decoder was found.**
This is a materially different risk than PRD §14 anticipated: choosing
"pure Rust" does not avoid LGPL for RAW decode, and `deny.toml`'s current
comment ("LGPL dependencies (LibRaw, libheif...) are NOT on the allow
list") doesn't yet account for this — `rawler` would be rejected by the
existing config exactly like LibRaw would, which is correct, but the
config's own comment implies only C libraries carry this risk.

**Why this specifically matters more for Rust than for the LibRaw/C case
PRD §14 already reasoned about:** LGPL's core relinking obligation assumes
a user can swap in a modified version of the library and relink — trivial
for a C shared library (replace the `.so`). Rust has no equivalent stable
dynamic-linking ABI in general use; a `rawler` dependency compiled directly
into Aurora's binary has no practical "swap the library and relink" story
without deliberately architecting the RAW decoder as a separate
dynamically-loadable component (e.g. a `cdylib` behind a stable C ABI) —
real, non-trivial packaging work, and the *same* work LibRaw would need,
not work avoided by picking the Rust option. **The "capability cost" PRD
§14 named as the tradeoff for avoiding LibRaw does not, in fact, buy an
avoided licensing obligation for RAW** — both paths need the same
relinking-capable architecture.

## 3. ICC is a different, better story: Little CMS's *core* is MIT, not LGPL

Checked directly rather than assumed grouped with RAW's licensing risk,
since PRD §14 discusses LibRaw and `libheif` by name but not Little CMS
specifically:

```
$ cat /usr/share/doc/liblcms2-2/copyright
License: MIT and GPL-3 (GPL-3 for the fast_float plugin only)
```

Little CMS (`lcms2`)'s core engine is MIT. Only one optional plugin
(`fast_float`, not required for normal operation) is GPL-3. **The FFI path
for ICC carries none of RAW's licensing complexity** — no dynamic-linking
architecture is needed at all, and `lcms2-sys` (the Rust binding crate)
doesn't even build against a system library: it vendors and statically
compiles Little CMS's actual C source itself (`lcms2-sys-4.0.7/vendor/`),
confirmed by inspecting the crate's own source rather than assumed from it
building without `liblcms2-dev` installed (this machine has no root access
to install system dev packages at all — `lcms2-sys` built anyway, which is
what led to checking why).

## 4. `moxcms` (pure Rust) and `lcms2` (FFI) agree exactly, once configured to agree — the corpus-cross-validation discipline from the PSD spike, applied here

`moxcms` isn't a hypothetical alternative — it's already a transitive
dependency of `rawler` itself (pulled in during step 1's `cargo build`,
unprompted), meaning a real, actively-maintained project already trusts it
for its own RAW color pipeline. Real ICC profiles (`reference/icc-profiles/`,
CC0-licensed — see `reference/README.md`), real sRGB → ECI-RGBv2 transform,
six known colors including three that go out-of-gamut (saturated
red/green/blue), cross-checked against `lcms2` the same way finding 14 in
the PSD spike cross-checked `engine_data.rs`'s writer against `psd-tools`:
two independent implementations agreeing is real corroboration; one
agreeing with itself is not.

**First pass disagreed significantly** (green's G channel: `1.0093` from
`lcms2` vs. `1.0000` from `moxcms`; blue's G channel: `-0.3513` vs.
`0.0000`) — worth investigating properly rather than either dismissing it
or reporting it as a moxcms bug without checking:

1. First hypothesis: mismatched rendering intent (`moxcms` defaults to
   `Perceptual`, which deliberately compresses out-of-gamut colors; `lcms2`
   was called with `RelativeColorimetric`). Matched explicitly — **no
   change**, ruling this out.
2. Second hypothesis: `prefer_fixed_point` (`moxcms`'s default, `true`)
   silently downgrading precision. Disabled — in-gamut colors tightened
   from 4-decimal to exact agreement, but the out-of-gamut clamping
   **persisted unchanged** — a real effect, but not the cause of the
   clamping specifically.
3. Found it by reading `moxcms`'s own `TransformOptions` source rather than
   guessing further: `allow_extended_range_rgb_xyz` exists exactly for
   this — matrix-shaper RGB profiles preserving true extended-range
   (negative / >1.0) values instead of clamping — but is gated behind a
   Cargo feature (`extended_range`) not enabled by default. Enabled it.

**Result: exact agreement, to 4 decimal places, on every color including
the extended-range ones:**

```
color                         lcms2                   moxcms      max |Δ|
white      [1.0000, 1.0000, 1.0000] [1.0000, 1.0000, 1.0000]       0.0000
black      [0.0000, 0.0000, 0.0000] [0.0000, 0.0000, 0.0000]       0.0000
mid-gray   [0.5339, 0.5339, 0.5339] [0.5339, 0.5339, 0.5339]       0.0000
red        [0.8514, 0.1237, 0.1387] [0.8514, 0.1237, 0.1387]       0.0000
green      [0.6204, 1.0093, 0.2249] [0.6204, 1.0093, 0.2249]       0.0000
blue       [0.2115, -0.3513, 0.9789] [0.2115, -0.3513, 0.9789]       0.0000
```

This matters specifically for Aurora: invariant §7.3.1b requires HDR and
scene-referred values >1.0 to be preserved through the graph rather than
clipped. A CMS that silently clamps to [0,1] — `moxcms`'s own *default*
behavior, before this was found — would silently violate that invariant.
The correct configuration exists and works correctly; it just isn't the
default, and finding that took real investigation rather than accepting
the first (wrong-looking) result at face value.

**Corrects PRD §14/PRD.md:1051's risk framing** ("no mature pure-Rust ICC
engine; FFI wrapper required") — that may have been accurate when written,
but a pure-Rust engine now exists, is used by a real project, is
permissively licensed (BSD-3-Clause OR Apache-2.0), and produces
numerically identical results to the industry-standard FFI option on a
real transform, once correctly configured.

## Scope: what this did *not* touch

- **Demosaicing.** `rawler::decode_file` returns raw sensor (Bayer mosaic)
  data, not a demosaiced RGB image — matching Aurora's own architecture
  (invariant §7.3.2: edits are non-destructive render-graph nodes, so
  demosaic belongs in Aurora's own graph, not baked in by the decoder).
  This spike's preview is a crude "R=G=B from the mosaic," not a real
  demosaic algorithm.
- **Lens correction, white balance, noise reduction** (FR-015's other
  named features) — out of scope for a decode-feasibility spike.
- **RAW file writing / DNG conversion** — FR-015 is import-only; no write
  side was tested or is required.
- **A full RAW corpus.** One file per vendor, not PLAN 0.7's eventual
  "RAW samples per camera vendor" breadth (multiple bodies, compressed vs.
  uncompressed variants, older/newer sensor generations). Enough to answer
  "can this decode real files from major vendors at all," not enough to
  gate a real implementation.
- **CMYK, Lab-native profiles, or profile creation/editing** — only RGB→RGB
  matrix-shaper profiles were tested (`sRGB.icc`, `ECI-RGBv2.icc`); both
  are simple TRC/matrix profiles, not full LUT-based ones. A LUT-based
  profile (common for CMYK, or gamut-mapped output profiles) is untested
  and may behave differently.
- **`libheif`** (PRD §14's other named LGPL dependency, for HEIF/AVIF) —
  not touched at all this session; the same Rust-static-linking relinking
  question raised for RAW in finding 2 likely applies to it too, unverified.
- **Actual packaging/relinking architecture** — this spike establishes
  *that* LGPL applies and *why* Rust static linking makes the obligation
  harder to satisfy than the C-library case PRD §14 reasoned about; it does
  not design or prototype the dynamically-loadable-component architecture
  that would actually satisfy it. That is real, separate engineering work.

## Recommendations for Phase 3 / ADR 0005+

1. **RAW: expect to need LGPL packaging architecture regardless of which
   library is chosen.** `rawler` (pure Rust, full-featured) and LibRaw (C,
   full-featured) are both LGPL; no permissive full-featured alternative
   exists in the Rust ecosystem today. Budget the dynamically-loadable
   relinking architecture as required work, not a LibRaw-specific cost —
   picking `rawler` over LibRaw doesn't remove this line item, only changes
   which language the relinkable component is written in.
2. **ICC: `lcms2` (FFI, vendored, MIT-core) has no comparable licensing
   complexity.** Given it's already vendored/statically-linked in the Rust
   binding, there's no dynamic-linking packaging burden at all — genuinely
   the simpler of the two library choices this spike covered, contrary to
   PRD §14's grouping of RAW and ICC together as both "FFI/copyleft" risk.
3. **`moxcms` is a real, viable pure-Rust ICC alternative** — update PRD
   §12/§14's "no mature pure-Rust ICC engine" risk note; it's outdated. Not
   yet a final recommendation over `lcms2` (this spike didn't test LUT-based
   or CMYK profiles, or perf), but the numerical-correctness bar is cleared.
4. **`allow_extended_range_rgb_xyz` (or whatever the eventual chosen
   library's equivalent is) needs a regression test in Phase 1/3, not just
   a spike observation.** Its own default is the wrong one for Aurora's
   HDR invariant — a real, easy-to-miss footgun for whoever wires up
   `aurora-color`, worth a named test asserting out-of-gamut values survive
   a transform rather than silently clamping.
5. **Same "read a working implementation, don't guess" discipline that
   served the PSD spike well, applied here too** (findings 5, 9, 10, 14 of
   that spike) — the extended-range discrepancy in finding 4 was resolved
   by reading `moxcms`'s own `TransformOptions` source for an
   already-existing flag, not by trial and error or by concluding one
   library was simply wrong.
6. **Decide RAW and ICC libraries formally as ADRs** (PLAN 0.6's original
   ask) — this spike provides the evidence; the decision itself, especially
   RAW's now-confirmed-unavoidable LGPL packaging question, needs
   Cahya's sign-off given its architectural weight, not just a spike
   recommendation.
