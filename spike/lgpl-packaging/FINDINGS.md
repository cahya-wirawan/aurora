# LGPL packaging architecture spike — findings

PLAN 0.6 follow-up. Built 2026-07-28, same day as `spike/raw-icc`. Extended
2026-07-28 (finding 4) after ADR 0007/0008 to re-verify with LibRaw itself.

`spike/raw-icc/FINDINGS.md` finding 2 established that every viable
full-featured RAW decoder (`rawler`, LibRaw) is LGPL, and that Rust's lack
of a stable cross-compiler dylib ABI makes the standard "just dynamically
link a `.so`" story less automatic than it is for a C library. This spike
prototypes the actual mechanism, rather than just discussing it, and
verifies it against the license text directly rather than against
secondhand summaries of what LGPL requires — first with `rawler`, then
(finding 4) with LibRaw, the library ADR 0007 actually chose.

```sh
cd spike/lgpl-packaging
cargo build --release
./target/release/host ./target/release/libraw_shim.so \
    ../raw-icc/reference/raw-samples/canon-eos-m200.cr3      # rawler-backed
./target/release/host ./target/release/liblibraw_shim.so \
    ../raw-icc/reference/raw-samples/canon-eos-m200.cr3      # LibRaw-backed — same host binary, unchanged
```

## Verdict: the mechanism works, verified concretely, on Linux — cross-platform packaging and legal sign-off are separate, real work still ahead

## 1. The actual legal target, quoted directly rather than assumed

Fetched the LGPL-2.1 text itself
(`https://www.gnu.org/licenses/old-licenses/lgpl-2.1.txt`) rather than
working from a summary. §6 offers five ways to combine an LGPL library
with other code; (b) is the only one compatible with shipping a normal
commercial application (the others require distributing source/object code
or a standing written offer to do so):

> b) Use a suitable shared library mechanism for linking with the Library.
> A suitable mechanism is one that (1) uses at run time a copy of the
> library already present on the user's computer system, rather than
> copying library functions into the executable, and (2) will operate
> properly with a modified version of the library, if the user installs
> one, as long as the modified version is interface-compatible with the
> version that the work was made with.

Two concrete, testable conditions. This spike built and tested a real
mechanism against both of them, rather than designing one on paper and
assuming it qualifies.

## 2. Architecture: `cdylib` + hand-written C ABI + runtime `dlopen`, not a Rust `dylib`

Two crates, deliberately in separate Cargo packages within one workspace so
their dependency graphs stay genuinely separate (not just conceptually
separate within one binary):

- **`raw-shim`** — the only crate that depends on `rawler` (LGPL-2.1).
  Compiled as `crate-type = ["cdylib"]`: a genuine OS shared library
  (`.so`/`.dylib`/`.dll`), not Rust's own `dylib` crate type. This
  distinction is the crux of the whole design: Rust's `dylib` format
  embeds compiler-version-specific metadata with no stable ABI across
  `rustc` versions, so a user's independently-built "modified version"
  would almost certainly *not* be interface-compatible even with identical
  source — failing condition (2) outright. `cdylib` + a hand-written
  `extern "C"` interface using only `#[repr(C)]` plain-old-data types
  sidesteps this: the ABI is whatever the C function signatures say it is,
  stable regardless of which Rust compiler built either side.
- **`host`** — zero dependency on `rawler`, `raw-shim`, or any LGPL crate.
  Loads `raw-shim`'s `.so` at run time via `libloading` (ISC-licensed,
  already on Aurora's `deny.toml` allow-list) — i.e. `dlopen`, not a
  build-time link.

The exposed surface is deliberately minimal (one function,
`aurora_raw_decode`, plus its paired `aurora_raw_free`) — enough to prove
the mechanism, not a real API surface. A real integration would cover more
of the decoder's surface and, more importantly, needs an explicit ABI
version story (see Scope, below) that this proof doesn't attempt.

## 3. Verified, not just built — both LGPL-2.1 §6(b) conditions checked directly

**Condition (1): loaded at run time, not copied into the executable.**
Checked with the actual tools that would reveal a false claim, not asserted:

```
$ file target/release/libraw_shim.so
libraw_shim.so: ELF 64-bit LSB shared object, ... dynamically linked

$ ldd target/release/host | grep raw_shim
(no output — no build-time link at all)

$ nm -D target/release/host | grep -ic rawler
0
```

The shim is a real shared object; the host binary has no link to it at
build time and contains zero `rawler` symbols. RAW-decode functionality
only exists in the process after `Library::new()` (i.e. `dlopen`) runs.

**Condition (2): works with a user's modified, interface-compatible
replacement, without recompiling the host.** Not just argued — demonstrated:
temporarily changed `raw-shim`'s CFA-string handling to return a fixed
marker (`b"MOD!"`) instead of the real pattern, rebuilt *only* `raw-shim`,
and re-ran the **already-built, unmodified** `host` binary against the new
`.so`:

```
$ ./target/release/host ./target/release/libraw_shim.so canon-eos-m200.cr3
...cfa=MOD!...   # was cfa=RGGB before the swap; host binary untouched
```

Reverted immediately afterward. This is the concrete version of condition
(2), not an assumption that dynamic linking implies it — the host binary
picked up the modified behavior with zero recompilation, exactly what a
user swapping in their own modified `raw-shim` build would experience.

**End-to-end correctness, not just mechanism.** Ran all three real RAW
files from `spike/raw-icc/reference/raw-samples/` through the dlopen'd
shim; every dimension/CFA/range/mean value matches the direct
`rawler::decode_file()` results from that spike **exactly**:

| File | Direct (raw-icc spike) | Through dlopen (this spike) |
|---|---|---|
| Canon CR3 | 6288×4056, RGGB, [482,16383], mean 1630 | identical |
| Nikon NEF | 3904×2606, RGGB, [0,4037], mean 500 | identical |
| Sony ARW | 6048×4024, RGGB, [475,16380], mean 1804 | identical |

No precision or correctness cost from crossing the FFI/dynamic-loading
boundary.

## 4. Re-verified with LibRaw itself, not just `rawler` — same host binary, unchanged

ADR 0007 (RAW: LibRaw via FFI, decided after this spike's first pass) named
a specific gap: the mechanism above was proven with `rawler`, but the
library actually chosen was LibRaw, and the argument that LibRaw's case is
simpler was sound reasoning, not independent verification. Closed directly
rather than left as an assumption: a second shim, `libraw-shim`, wraps
LibRaw's own C API (via `libraw_rs_vendor`, which vendors and statically
compiles LibRaw's actual source — no system `libraw-dev` headers needed,
the same property that made `lcms2-sys` build cleanly in `spike/raw-icc`)
behind the **identical** `RawImageFfi` layout and exported symbol names as
`raw-shim`. `host` — the exact same binary, not rebuilt or even
recompiled — loads either `.so` by path and works with both. That by
itself is a real, additional confirmation of the design: the C ABI
boundary is genuinely what matters, not anything specific to `rawler`.

**Both LGPL-2.1 §6(b) conditions re-verified against `liblibraw_shim.so`**,
the same way as finding 3: `file`/`ldd`/`nm` confirm a real shared object
with no build-time link and zero LibRaw symbols in `host`; a one-line
modification (`cfa = *b"MOD!"`), rebuilding only `libraw-shim`, worked
correctly with the same unmodified `host` binary, exactly reproducing the
hot-swap result finding 3 got with `rawler`.

**Decode correctness, cross-checked between two independently-coded
libraries, not just against itself:**

| File | `rawler` (via `raw-shim`) | LibRaw (via `libraw-shim`) |
|---|---|---|
| Canon CR3 | 6288×4056, RGGB, [482,16383], mean 1630 | 6288×4056, RGBG, [482,16383], mean 1630 — **exact** |
| Nikon NEF | 3904×2606, RGGB, [0,4037], mean 500 | 3904×2606, RGBG, [0,4035], mean 500 — max off by 2 |
| Sony ARW | 6048×4024, RGGB, [475,16380], mean 1804 | 6048×4024, RGBG, [476,16372], mean 1805 — min/max/mean each off by ≤8 |

Dimensions and CFA *pattern* (only the label string differs — `RGGB` vs.
`RGBG` describe the same physical 2×2 arrangement read in a different
convention, not a different pattern) match on all three files; pixel
statistics match exactly on one and closely (single-digit differences, out
of a 12–14-bit range) on the other two — plausibly different black-level or
saturation handling between the two decoders, not investigated further
since it's outside this spike's actual question (packaging mechanism, not
decoder-accuracy comparison). Recorded honestly rather than rounded up to
"identical."

## Scope: what this did *not* touch

- **Only Linux tested.** The same principle (a real shared library, loaded
  at run time, not statically linked) applies to macOS (`.dylib`, governed
  by `install_name`/`@rpath`) and Windows (`.dll`, governed by the DLL
  search path/delay-loading) — the *legal* argument is platform-independent,
  but the *mechanics* (how the OS finds and loads the file, how an
  installer lays it out, how a user would actually replace it) are
  meaningfully different per platform and unverified here.
- **No ABI versioning discipline.** A real implementation must define what
  "interface-compatible" means precisely (a version field in the struct? a
  negotiation function? semantic versioning on the `.so`'s own filename/
  `SONAME`?) and test that an *actually* modified, independently-built
  `raw-shim` — not just this spike's one-line same-source edit — still
  works. This proof used the same compiler and same struct layout on both
  sides; real interface compatibility across different builds, is a
  separate, harder property to guarantee and wasn't tested.
- **LibRaw was re-verified (finding 4), with one remaining nuance.**
  `libraw-shim` proves the `cdylib` + hand-written-ABI + `dlopen` mechanism
  works with LibRaw's own decode calls, not just `rawler`'s — but it does so
  via `libraw_rs_vendor`, which vendors and *statically* compiles LibRaw's
  C++ source into the shim `.so`. The even simpler variant ADR 0007
  describes — linking `aurora-io` dynamically against a *separately
  packaged* `libraw.so`/`.dylib`/`.dll`, with no custom Rust wrapper at all
  — is a different, plausible, and probably still-simpler architecture that
  remains unverified. Both satisfy §6(b) the same way (the LGPL code ends
  up in its own dynamically-loaded shared object either way); which is
  preferable is a packaging-convenience question, not a licensing one, and
  wasn't decided here.
- **`libheif`** (PRD §14's other named LGPL dependency) — untested; likely
  the same architecture applies (it's already a C library with native
  shared-library support, so closer to the "simpler" LibRaw case above),
  but not verified.
- **Packaging/installer integration** — where the `.so`/`.dylib`/`.dll`
  physically lives relative to the main executable, how each platform's
  installer lays it out, and how `RPATH`/`install_name`/DLL search order is
  configured so the OS actually finds it at run time. Real, non-trivial
  work per platform, not attempted here.
- **This is engineering verification, not legal advice.** The spike confirms
  the *mechanism* satisfies the literal text of §6(b)(1) and (2) as read by
  someone doing careful engineering, not as adjudicated by a lawyer. One
  specific ambiguity worth flagging rather than glossing over: §6(b)(1)
  says "a copy of the library **already present on the user's computer
  system**" — read most literally, this describes relying on a
  pre-existing OS/distro package (normal on Linux, unusual for how macOS
  and Windows apps are typically distributed), not a copy Aurora bundles in
  its own installer. Bundling a separate, dynamically-loaded `.so` file
  alongside the executable (rather than statically linking it into one
  blob) is the widely-used industry practice for satisfying this clause on
  macOS/Windows, and is almost certainly the intended reading given the
  rest of the clause — but "almost certainly" is an engineering judgment,
  not a legal one. Recommend real legal review before shipping, same as
  `spike/raw-icc/FINDINGS.md` recommendation 6 already said for the library
  choice itself.

## Recommendations for Phase 3 / ADR 0005+

1. **This architecture is the way to satisfy LGPL for RAW, and is now proven
   to work mechanically on Linux with the actual chosen library (LibRaw,
   ADR 0007), not just by analogy from `rawler`.** Extend to macOS/Windows
   and define the ABI-versioning story before committing further.
2. **Get real legal review specifically on the §6(b)(1) "already present on
   the user's system" question** before shipping — this spike's reading
   (bundled-but-separate-file satisfies it) is an engineering judgment
   informed by common practice, not a legal conclusion.
3. **Decide between `libraw-shim`'s vendored-and-statically-compiled
   approach and a direct dynamic link against a separately packaged
   `libraw.so`/`.dylib`/`.dll`** — finding 4's remaining open nuance. Both
   satisfy LGPL the same way; the choice is about build reproducibility
   (vendoring pins the exact LibRaw version and needs no system package) vs.
   avoiding a from-source C++ build of LibRaw in Aurora's own build (using
   whatever the OS/package manager already provides). Since LibRaw already
   has its own stable C API either way, this is a build-engineering decision
   with no ongoing hand-rolled-ABI cost regardless of which way it goes —
   the point PRD §14's "prefer pure Rust" framing missed (see
   `spike/raw-icc/FINDINGS.md` finding 2): that cost is specific to wrapping
   `rawler`, not to choosing FFI at all.
4. **Same discipline as every finding in the PSD and RAW/ICC spikes**: verify
   the actual claim with the actual tool (`file`, `ldd`, `nm`, and a real
   before/after swap test), don't accept "it's a cdylib, therefore it must
   satisfy LGPL" as self-evidently true without checking.
