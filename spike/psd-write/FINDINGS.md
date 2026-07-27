# PSD write spike — findings

PLAN 0.6, [ADR 0004](../../docs/adr/0004-psd-full-write.md). Built 2026-07-26.

Aurora commits to full layered PSD *write*: a file edited in Aurora must reopen
in Photoshop with its layers intact. Phase 3 is ten months long and rests on that
being feasible. This spike writes a real layered PSD from scratch to test the
assumption.

```sh
cd spike/psd-write
cargo run                  # writes out/spike.psd (layers + groups)
./verify.sh                # checks it with two independent readers
cargo run -- --tysh-demo   # parses + patches a real Photoshop text layer
cargo run -- --glyph-demo  # rasterizes text to out/glyph-demo.ppm (view in any image viewer)
cargo test                 # round-trip and rasterization tests against real bytes
```

## Verdict: pixel layers and groups are genuinely easy; text layers are harder than planned

A hand-written PSD with a nested layer tree — a top-level layer, a two-level
group hierarchy, per-layer alpha, opacity, blend modes, visibility, and a
non-ASCII name — is accepted by two independent readers, with layer pixel data
verified correct and the flattened preview matching hand-computed blend math
exactly.

**But "accepted by a reader" is a much weaker claim than it sounds**, and
findings 1–5 below are mostly about that gap.

**Text layers (findings 6–10) turned out to be the more consequential result
of this spike.** A real Photoshop text layer's `EngineData` is far richer
than a naive minimal example — hand-generating it from scratch carries real,
untested risk (finding 6). The lower-risk path — patch a real file's bytes
rather than generate from nothing — was proven out end-to-end (finding 7),
and both the `TySh` container and `EngineData`'s own text format are now
implemented and tested in Rust against real bytes from two independent PSD
libraries (findings 9, 10), catching one genuine Unicode-escaping bug along
the way. But the same investigation surfaced mandatory engineering that was
not in the original Phase 3 scope at all: **editing text content requires
rendering actual glyphs into the layer's pixel channels** (finding 8), or the
resulting file is visually inconsistent — descriptor says one thing, pixels
show another — and separately, **paragraph/style run lengths must be
recomputed to match the new text** (finding 10). Finding 8 is the single most
important finding in the document.

## What was verified

| Property | Result |
|---|---|
| File opens in Apple's system decoder (`sips`) | Yes |
| File opens in `psd-tools` (independent implementation) | Yes |
| Layer names, sizes, offsets | Round-trip exactly |
| Opacity, blend mode, visibility flags | Round-trip exactly |
| Per-layer pixel data (centre-pixel RGBA per layer) | Correct for all 5 |
| Non-ASCII layer name (`レイヤー 5`) | Correct, via the `luni` block |
| Flattened preview vs re-composite from layers | Max channel delta 3 (rounding) |
| **Groups: 2-level nesting, open/closed state, membership** | Correct (asserted in `verify.sh`, not just eyeballed) |
| **Multiply-blend compositing through nested groups** | Exact match to hand-computed pixel value |

**Not verified: Photoshop itself.** No copy was available. That check is still
outstanding and is the only one that settles ADR 0004. Everything here is a
lower bound — a file these readers rejected would certainly fail in Photoshop,
but the converse does not follow.

## Findings

### 1. A reader accepting the file proves almost nothing

The first version wrote the flattened composite **interleaved** (RGBRGB…) where
PSD requires **planar** (all R, then all G, then all B). Both readers opened it
without complaint and displayed a fine dither. No error, no warning — just
silently wrong pixels.

This is the single most important lesson for Phase 3: PSD has many fields whose
misuse produces a *readable* file rather than a rejected one. The Phase 3 gate
must therefore be **pixel comparison against a reference**, never "the file
opened". PRD §9 already words it that way; this is why.

### 2. The stored preview must apply blend modes

Every PSD carries a flattened copy for non-Photoshop viewers. Compositing it with
`normal` for every layer — while correctly *declaring* each layer's blend mode —
produced a file that Photoshop would render correctly (it recomputes from layer
data) but that **44 % of pixels differed** from what any other viewer shows,
with a max channel-sum delta of 243 out of 765.

After implementing multiply and screen in the flatten, the same comparison is
43 % of pixels differing but with a **max delta of 3** — pure rounding.

So the flattening path is not a throwaway preview generator: it needs the same
blend-mode implementations as the real compositor, and it needs to agree with
them. In Aurora that argues for the PSD writer calling the actual render graph
rather than a separate simplified path.

### 3. Layer names have two encodings and you need both

The layer record's Pascal string is legacy-encoded (effectively MacRoman). A
UTF-8 name written there comes back as mojibake — `レイヤー 5` read as
`„É¨„Ç§„É§„Éº 5`. Unicode names require the additional `luni` block (`8BIM` +
`luni` + UTF-16BE), which modern readers prefer when present.

Both must be written: the `luni` block for correctness, the Pascal string for
old readers. Any non-Latin layer name is affected, so this is not an edge case
for a professional tool with international users.

### 4. Small format traps worth recording

- **Visibility is inverted.** Flag bit 1 set means *hidden*, not visible. Easy
  to ship a file whose layers are all invisible.
- **Layer order is bottom-up**, the opposite of how a layers panel reads.
- **Channel lengths include their own 2-byte compression field.** Omitting it
  shifts every subsequent layer's data.
- **Lengths prefix content whose size is not yet known**, so serialization is
  inherently two-pass or buffered.
- **Padding rules vary by field** — layer names pad to 4, layer info to 2.

None is hard. All are silent when wrong.

### 5. Groups are two invisible pseudo-layers, and the order is load-bearing

A group is not a container field on a layer record — it is represented by
**two extra zero-sized layer records** bracketing its members: a "bounding
divider" (`lsct` kind 3) at the *bottom* of the group's span, the member
records in between, and a "folder" record (`lsct` kind 1 open / 2 closed) at
the *top*, which is the one that actually carries the group's name, opacity,
blend mode, and visibility. Nesting is just recursion — a sub-group's own
bounding/folder pair sits inside its parent's span like any other member.

This was **not derived from the spec text** but confirmed by reading
`psd-tools`' own writer (`Group.new()` and `_build_record_tree()` in its
source) before writing a single byte, specifically because a plausible-looking
*wrong* order — bounding-first vs folder-first, which end holds the metadata —
would have produced exactly the kind of silently-broken file findings 1 and 2
already caught once. Trusting a working implementation over a guess at binary
layout was cheaper than debugging a mis-ordered group from the reader's side.

Open vs closed is UI-only — a closed group's contents still composite
normally, which the implementation had to get right rather than assume.

### 6. Text layers: real Photoshop `EngineData` is far richer than a minimal example, and that changes the risk

The plan for this spike originally deferred text layers as "the largest
remaining unknown," expecting a scope similar to groups. It is not — this
turned out to be the most consequential finding of the whole spike, and it
was found *before* writing any speculative code, by downloading a real
Photoshop-authored text layer and reading it structurally first (see
`reference/README.md`; `text.psd` is a genuine `psd-tools` test fixture, not
synthesized).

`TySh`'s top-level descriptor has six fields (`Txt `, `textGridding`, `Ornt`,
`AntA`, `TextIndex`, `EngineData`) — small and tractable. But `EngineData`
itself, for a plain three-line, single-style text layer with **no** Japanese
content, contains:

- Two near-duplicate resource dictionaries (`ResourceDict` and
  `DocumentResources`), each with a full `FontSet`, `StyleSheetSet`,
  `ParagraphSheetSet`
- Complete **kinsoku** (Japanese line-breaking) and **moji-kumi** (character
  spacing) rule tables — present and populated even though the text is plain
  English
- Full paragraph and style run structures with dozens of typesetting
  parameters (hyphenation, tracking, kerning, leading, fill/stroke colour,
  ligatures, superscript/subscript sizing) per run
- Rendered glyph geometry (`Rendered.Shapes.Children[].Cookie`)

A hand-rolled "minimal" `EngineData` based on the abbreviated example in
`psd-tools`' own docstring would omit essentially all of this. Whether
Photoshop's ATE tolerates that omission is unknown and untested — there was
no way to find out without Photoshop itself. Given the earlier findings in
this spike (a reader accepting a file proves little), the honest assessment
is: **hand-generating `EngineData` from scratch is meaningfully higher-risk
than everything else in this document**, including groups.

### 7. The lower-risk strategy: patch a real file's bytes, don't generate from scratch

Given finding 6, the spike pivoted to testing a different strategy: instead
of generating `EngineData` from nothing, parse a real Photoshop-authored
`TySh` block, modify only the specific fields that must change, and
re-serialize everything else byte-for-byte untouched. The boilerplate
(kinsoku tables, resource dicts) is Photoshop's own static data — copying it
verbatim carries none of finding 6's risk.

**Proof of concept (Python, using `psd-tools`' own low-level reader/writer,
full file level):**

1. Confirmed the baseline: an *unmodified* real file round-trips
   **byte-identical** through `psd-tools`' low-level `PSD.read`/`.write` —
   93,949 bytes in, 93,949 bytes out, identical. This is the necessary
   precondition for trusting any patch built on top of it.
2. Modified only `text_data[Txt ]` and `EngineData.EngineDict.Editor.Text`
   (plus the paragraph/style run-length bookkeeping, since the text got
   shorter), re-serialized, and **independently re-parsed in a fresh
   process**: `layer.text` returned the new string correctly.
3. Verified with **two independent readers**, matching this spike's
   established methodology: Apple's `sips` decoded the file, and a fresh
   `psd-tools` process both read the modified text back and rendered the
   composite.

**This is the strategy to take into Phase 3 for text layers**, not
from-scratch generation. It is not free — see finding 8 — but it is
substantially lower-risk.

### 8. The pixel/vector sync gap — the most important thing this spike found

Patching only the `TySh` descriptor leaves the layer's **rasterized pixel
preview unchanged**. Photoshop text layers carry two representations: the
editable ATE description (what `TySh`/`EngineData` hold) and rendered pixel
channels (what a flattened composite, and simple viewers, actually show).
Editing the descriptor without also re-rendering the pixels produces a file
that is internally inconsistent — confirmed by direct visual inspection:

| | |
|---|---|
| `layer.text` (re-parsed, fresh process) | `"Aurora spike"` — correct |
| Composite / rendered pixels | Still show `"Line 1 / Line 2 / Line 3 and text"` |

Photoshop itself resolves this automatically because its ATE re-renders on
every edit. **Aurora's PSD writer does not have Photoshop's ATE.** Whenever
Aurora changes text content, `aurora-io` must render actual glyphs — through
Aurora's own text stack (`cosmic-text`/`glyphon`, per PRD §8.3) — into the
layer's pixel channels, not just serialize the descriptor. This is real,
non-optional engineering work with no shortcut, and it was not visible until
tested end-to-end with a real file.

### 9. Two format traps specific to `TySh`, both caught before they became bugs

- **`text_data` and `warp` are not plain `Descriptor`s — they're
  `DescriptorBlock`s, with an extra leading `u32` version field (`= 16`)
  that a plain nested descriptor doesn't have.** The Rust parser's first
  attempt, built by reading the general `_DescriptorMixin` format alone,
  desynced a few bytes into a real file and produced nonsense — not a crash,
  a *plausible-looking wrong parse*, exactly the failure mode findings 1–2
  warned about, one layer deeper. Caught immediately by testing against real
  bytes rather than trusting the parser because it compiled.
- **`read_length_and_key`'s zero-length shorthand means byte-identity is the
  wrong invariant to test for this format.** A length field of `0` means
  "the key is the next 4 bytes" — real Photoshop uses this shorthand for
  recognized terms (confirmed directly: the real file's own `classID` field
  is `0x00000000` + `"TxLr"`, not `0x00000004` + `"TxLr"`). This writer
  always emits the explicit length instead of replicating Photoshop's own
  lookup table of known terms — both are valid per the format's own read
  rule, and any conformant reader must accept either. So the correct
  round-trip test for `Descriptor` is *same length, semantically identical
  on re-parse*, not byte-for-byte identical (unlike the plain layer-record
  format in finding 5, which is byte-identical). Getting this distinction
  right, rather than either chasing an unnecessary byte-match or silently
  accepting a real bug, required understanding *why* the format allows two
  valid encodings — see `src/descriptor.rs` for the mechanics.

**Rust implementation**: `src/descriptor.rs` — a `Descriptor`/`DescriptorBlock`
reader/writer covering exactly the five value types confirmed present in a
real file (`String`, `Double`, `Integer`, `Enumerated`, `RawData`, plus
nested descriptors for `warp`). Deliberately scoped to fail loudly on any
other type rather than guess (`rejects_unknown_types_loudly_instead_of_guessing`
test). Four tests, all passing, all against the real extracted bytes in
`reference/tysh.bin`: parses real Photoshop data, round-trips
(same-length/semantic-identity, per finding 9), patches the text field and
stays internally consistent, and rejects unknown types. Run via
`cargo run -- --tysh-demo` for a human-readable walkthrough of the same
parse-then-patch operation.

**Follow-up (same day):** the full Rust-side splice was still not
implemented (see below), but `EngineData`'s own text-format *was* — see
finding 10.

### 10. `EngineData`'s own format is now implemented and tested in Rust — corpus generalized to a second library, one real bug found and fixed by testing rather than assuming

Following finding 7's validated strategy, `src/engine_data.rs` implements a
full reader/writer for `EngineData`'s text-based format (`<<...>>` dicts,
`/Key` properties, `[...]` lists, numbers, `true`/`false`, and the
UTF-16BE-with-BOM parenthesized strings) — not just treating it as an opaque
blob. `--tysh-demo` now patches **both** the top-level `Txt ` field and the
nested `EngineDict.Editor.Text`, closing the gap the original version of
this demo explicitly left open.

**Corpus generalization.** Beyond `text.psd`, two more `TySh` blocks were
pulled from [`ag-psd`](https://github.com/Agamnentzar/ag-psd)'s test suite —
a second, independently written PSD library (unlike `psd-tools`, one that
also *writes* text layers) — including a genuine multi-style-run case
(`text.psd`'s one file had a single style run covering all 30 characters;
one of these has two separate runs). The existing `Descriptor`/`DescriptorBlock`
parser required **zero code changes** to handle bytes from a codebase it was
never informed by — real evidence the container-format understanding
generalizes, not just fits the one file it was built against.

**A genuine bug, found by testing against real bytes rather than trusting a
"reasonable-looking" first implementation.** The first version of the string
writer escaped `\`, `(`, `)` only when they appeared as the *low* byte of an
ASCII-range UTF-16 unit. That's wrong: some non-ASCII codepoints (e.g.
U+29xx) have `0x29` — literal `)` — as their *high* byte. An unescaped `)`
there is silently misread by the reader as the string's own terminator,
truncating the content with no error at all. Caught by a test constructing
exactly that case, not by inspection. Fixed to match `psd-tools`' own
approach: a naive whole-buffer byte-level replace (backslash first, so
newly-inserted escape characters aren't re-escaped), which is
alignment-unaware and therefore correct for arbitrary Unicode content. Same
lesson as findings 1, 2, and 9, one format deeper: something that looks
locally reasonable is not the same as something verified against real data.

**One reference file turned out to be the wrong kind of evidence, and was
caught before it caused a false result.** `ag-psd`'s `engineData.txt` looked
like a plain-text `EngineData` dump — genuinely useful for confirming key
names and nesting shape while designing the parser — but checking its raw
bytes directly (`0 in data` → `False` for the whole 19 KB file) showed it has
no embedded UTF-16BE/BOM bytes at all. It's a human-readable pretty-print,
not the wire format a real `.psd` file embeds. Using it as parser test input
would have meant testing against a format the parser doesn't actually need
to handle. All test fixtures instead use the `EngineData` payload genuinely
extracted from a Photoshop-authored `TySh` block (`reference/tysh.bin`).
Recorded in `reference/README.md` so the distinction isn't lost.

**Was not done as of this finding, closed by finding 12:** `--tysh-demo`'s own
output said so explicitly rather than implying more completeness than
existed — after patching both text fields, the `ParagraphRun`/`StyleRun`
`RunLengthArray`s still summed to the *old* text's length. A real writer must
recompute these against the new text, or Photoshop's own run bookkeeping is
internally wrong. This was additional, separate work from finding 8's
pixel/vector sync gap, not the same issue — finding 8 (glyph rendering) is
still open.

### 11. Corpus extended to paragraph text and warped text — container format holds, no code changes needed

Recommendation 4 (below) named two specific corpus gaps: paragraph-vs-point
text and warped text (`warpStyle != warpNone`). Both are now covered:
`reference/tysh-paragraph.bin` and `reference/tysh-warp-arc.bin`, both from
`ag-psd`'s test suite (`test/read/text-paragraph-align/` and
`test/read/text-complex/`), same extraction method as `tysh-single-run.bin`/
`tysh-multi-run.bin` — see `reference/README.md`.

As with finding 10's corpus generalization, **`descriptor.rs` needed zero
code changes** to parse either fixture — real evidence the `TySh` container
understanding holds beyond the cases it was built against, not just re-fitting
to new data. Two things specifically confirmed against real bytes rather than
assumed:

- **Point vs. paragraph text is not a `TySh`-level field at all.** It's
  encoded inside `EngineData`, at
  `EngineDict.Rendered.Shapes.Children[0].Cookie.Photoshop.ShapeType` (`0`
  for point, `1` for paragraph) — confirmed against `psd_tools`' own
  `TypeLayer.text_type` property before trusting it, then tested in Rust
  (`engine_data::tests::distinguishes_point_from_paragraph_text`). Reaching
  it required `Value::get_path` to cross a `List` (`Children`), which the
  existing dict-only path walker doesn't do — handled by indexing the list
  directly in the test rather than widening `get_path`'s contract for one
  caller.
- **`warpRotate`'s enum *category* is `"Ornt"`, not `"warpRotate"`.** Every
  enum seen before this (`warpStyle`, `textGridding`) happened to have its
  category equal its own item key, which is easy to over-generalize into "the
  category is always the key name." It isn't — `Ornt` is Photoshop's shared
  orientation enum, reused across features. Caught immediately by asserting
  against the real bytes (`descriptor::tests::parses_warped_text`) rather
  than writing the assertion from the pattern seen so far.

### 12. `RunLengthArray` recomputation implemented — closes finding 10's named gap, deliberately scoped to whole-text replacement

`engine_data::recompute_run_lengths` fixes the exact inconsistency finding 10
flagged: after `--tysh-demo` patches `EngineDict.Editor.Text`, the
`ParagraphRun`/`StyleRun` `RunLengthArray`s now correctly sum to the *new*
text's length (confirmed: `[7, 7, 16]` → `[13]` and `[30]` → `[13]` for the
demo's "Aurora spike\r" patch), instead of silently still summing to the old
30 characters.

**Scope, deliberate, not a corner cut:** this collapses each run array down
to a single run — the *first* existing entry's `ParagraphSheet`/`StyleSheet`
formatting, reused, given the whole new length. That's the correct behavior
exactly when an edit replaces the entire text with one paragraph in one
style, which is what `--tysh-demo` does and the only edit shape this spike's
patch-in-place model supports. It is **not** general run-preserving editing
(e.g. inserting a character in the middle of a multi-style word without
disturbing the runs around it) — that needs a real cursor/selection model
over the text, which belongs to Aurora's own text-editing engine in Phase 3,
not this exercise. Recorded here so a future reader doesn't mistake "the
demo's specific edit is now internally consistent" for "arbitrary text edits
are handled."

**One more instance of the same lesson as findings 1, 2, 9, and 10's own bug
find:** `RunLengthArray` sums to the text's **UTF-16 code unit** count, not
its Unicode scalar (`char`) count — confirmed against `ag-psd`'s
`text-test.psd` fixture (`RunLengthArray: [1, 1]` for a 2-run, 2-code-unit
layer) before writing `recompute_run_lengths`, not assumed from the fact that
every fixture seen so far happened to be ASCII where the two counts coincide.

Tested in `engine_data::tests::patches_text_and_recomputes_run_lengths`:
recomputes both arrays, confirms the retained run still carries the
*original* first run's formatting (not a blanked placeholder), and the
result still round-trips. 13 tests total now, all against real bytes.

### 13. Glyph rendering: a first, standalone proof that `cosmic-text` can rasterize real text headlessly — not yet wired into the writer

Finding 8 named the single biggest piece of unstarted work this spike
surfaced: editing a text layer's descriptor without re-rendering its pixel
channels leaves the file internally inconsistent. `src/glyph.rs` answers the
narrower first question — can Aurora's chosen text stack (`cosmic-text`, PRD
§8.3) actually rasterize real text to an RGBA8 buffer, headlessly, with a
font Aurora controls — before attempting the bigger, riskier step of wiring
that into `psd.rs`'s layer writer.

**`psd.rs`'s writer has no `TySh` slot at all.** Worth stating plainly: the
full-file `cargo run` writer (`Layer`/`FlatRecord` in `psd.rs`) only knows
about plain pixel layers, `luni` names, and `lsct` group markers. All the
`TySh`/`EngineData` work so far (findings 9–12) operates on a *standalone*
extracted block via `--tysh-demo`, never embedded in a written PSD. Closing
finding 8 for real needs a new tagged-block hook in the layer-record writer,
not just a rasterizer — that's the next step, deliberately not attempted
this session (see "Scope" below).

**What this proof establishes, via `cargo run -- --glyph-demo` and
`glyph::tests`:**

- `cosmic-text` renders real, correctly-shaped, legible text headlessly —
  confirmed by eye (`out/glyph-demo.ppm` for "Aurora spike" is unambiguously
  legible, not just "some pixels changed") and by four Rust tests: non-trivial
  glyph coverage, an empty string producing zero ink (rules out
  unconditional painting), longer text producing a wider canvas, and
  byte-for-byte deterministic output for identical input.
- **The font is bundled** (`reference/fonts/DejaVuSans.ttf`, loaded via
  `FontSystem::new_with_fonts` + `fontdb::Source::Binary`, never touching the
  host's installed fonts), so rendering is reproducible on any machine or in
  CI — a real, deliberate choice, not an oversight: matching Photoshop's own
  chosen font exactly is a separate, harder problem (resolving
  `ResourceDict.FontSet` names to actual font files) that this proof
  sidesteps rather than solves.
- **`cosmic-text`'s `Buffer::draw` callback fires per-subpixel-run, not
  per-glyph** — confirmed by reading the upstream `terminal` example before
  writing `glyph.rs`, not assumed. Every run observed from this font/shaper
  combination is 1×1, matching that example; the code asserts this rather
  than silently mishandling a wider run if one ever appeared, the same
  "fail loudly, don't guess" discipline as findings 1, 2, and 9.
- Canvas size is derived from the shaped layout itself (widest line ×
  line-height × line count) rather than a fixed box — correct for *point*
  text, which has no bounding box at all (`reference/tysh.bin`'s own `bbox`
  is `(0,0,0,0)`). *Paragraph* text's fixed wrap-box case (finding 11) is a
  real, different problem, out of scope for this proof.

**Scope, deliberate:** this is rasterization only. It does not: read
`StyleRun`'s actual `FillColor`/font/size and use them (the color/size are
hardcoded call arguments here); resolve the real font from
`ResourceDict.FontSet`; handle multiple style runs or paragraph line breaks
from `EngineData`; or embed the result into a written PSD at all. Each of
those is real, separate work — the point of doing this narrow slice first
is the same reason finding 7 did a Python proof-of-concept before findings
9/10's Rust port: prove the riskiest unknown (can the chosen library even do
this, headlessly, reproducibly) before spending effort on the integration
plumbing around it.

One color-encoding fact worth recording now, confirmed against `ag-psd`'s
own encoder (not just its reader) since `psd-tools` doesn't model this
field at all — it stays an opaque descriptor to that library:
`StyleSheetData.FillColor` is `{Type, Values}` where `Type: 1` means RGB and
`Values` is `[alpha_or_1, R, G, B]` in 0.0–1.0 floats (`Type: 0` = grayscale
`[1, K]`; `Type: 2` = CMYK `[1, C, M, Y, K]`). The real fixture's
`{Type: 1, Values: [1.0, 0.0, 0.0, 0.0]}` is therefore opaque black — the
expected default text color. Needed before `glyph.rs` can read a real
`FillColor` instead of taking color as a hardcoded argument; not yet wired
in.

## Scope: what this did *not* touch

Groups, the `TySh` container, `EngineData`'s text format, and
`RunLengthArray` recomputation for whole-text-replacement edits are all now
implemented and tested against real bytes. Remaining, still untested and
unimplemented:

- **General `RunLengthArray` preservation across an edit that keeps multiple
  paragraphs/style runs** (finding 12) — finding 12 only handles the
  whole-text-replacement case; a real cursor/selection-aware editor is
  needed for anything richer
- **Rendering glyphs into pixel channels to keep text layers visually
  consistent** (finding 8) — finding 13 proves the rasterizer works
  standalone, but it is **not wired into the writer**: `psd.rs` still has no
  `TySh` slot at all, `glyph.rs` still takes color/font as hardcoded
  arguments rather than reading `FillColor`/`ResourceDict.FontSet`, and
  nothing embeds rendered pixels into a written PSD. Still the single
  biggest gap between this spike and a real implementation.
- Layer masks and vector masks
- Smart objects, embedded and linked
- Layer styles and effects
- Adjustment layers
- 16/32-bit, CMYK, Lab, Grayscale
- RLE and ZIP compression *(raw only here; real files are compressed and much
  larger uncompressed — this 320×240 file is 763 KB)*
- PSB (>30,000 px)
- ICC profiles and metadata in the image-resources section
- The full Rust-side file-level splice (patching a complete PSD's byte
  stream in place, including enclosing layer-record/layer-info length
  prefixes) — proven out in Python (finding 7); the *format* understanding
  needed for it is now validated natively in Rust (findings 9, 10), but the
  file-splice plumbing itself is not built

**Do not read this spike as "PSD write is a solved problem," and do not read
the text-layer result as "text layers are solved."** What's established: the
container, layer records, channel data, Unicode naming, and groups are
tractable from scratch (finding 5); the `TySh` container and `EngineData`
formats are both implemented and tested against real, independently-sourced
bytes (findings 9, 10); the patch-a-real-file strategy is validated
end-to-end at the file level (finding 7); `RunLengthArray` bookkeeping is
correct for the one edit shape this spike supports — whole-text replacement
(finding 12); and Aurora's chosen text stack can rasterize real text
headlessly and reproducibly (finding 13). What's still open: full
from-scratch `EngineData` generation remains higher-risk than assumed
(finding 6) — the corpus-and-patch approach sidesteps rather than resolves
that; `RunLengthArray` bookkeeping for richer, run-preserving edits (finding
12's own named scope boundary) is real, separate, unstarted work; and the
pixel/vector sync requirement (finding 8) has a proven rasterizer (finding
13) but **no writer integration at all** — `psd.rs` has no `TySh` slot, and
nothing reads a real `FillColor`/font/layout — which remains by far the
biggest remaining item.

## Recommendations for Phase 3

1. **Gate on pixel comparison, not on files opening.** Finding 1 is the reason.
2. **Write the flatten through the real render graph**, so preview and canvas
   cannot diverge (finding 2).
3. **Build the 1,000-file corpus before the writer**, per PLAN 0.7, and diff
   every round-trip in CI from the first layer type.
4. **For text layers, patch real files rather than generate `EngineData`
   from scratch** (finding 7) — collect a small corpus of real Photoshop text
   layers spanning common cases and build the writer against them, the same
   way finding 7's proof-of-concept did for the simple case, finding 10
   extended into Rust with a second library's fixtures, and finding 11 closed
   the paragraph-vs-point-text and warped-text gaps specifically. Still open:
   this corpus is 5 fixtures, not the 1,000-file PSD corpus recommendation 3
   calls for — vertical text, RTL, and multiple simultaneous style runs
   within one paragraph remain untested.
5. **Budget real engineering time for glyph rendering into pixel channels**
   (finding 8) — this is not a follow-on detail, it's required for any text
   edit to produce a non-broken file, and it did not appear in the original
   Phase 3 scoping. **The rasterizer itself is now proven feasible**
   (finding 13, `cosmic-text` headless, bundled font) — what's still
   unbudgeted is the writer-integration work: a `TySh` slot in `psd.rs`,
   reading real `FillColor`/font/size instead of hardcoded values, and
   resolving `ResourceDict.FontSet` font references to actual files.
6. **Recompute `ParagraphRun`/`StyleRun` `RunLengthArray`s whenever text
   length changes** (finding 10) — a second, separate piece of mandatory
   bookkeeping alongside finding 8's pixel sync. **Done for whole-text
   replacement** (finding 12, `engine_data::recompute_run_lengths`). Still
   open: a real editor needs this to work for edits that preserve multiple
   paragraphs/style runs, not just full replacement — that needs a
   cursor/selection model this spike doesn't have.
7. **Get a Photoshop licence for verification.** Nothing else settles ADR 0004,
   and an independent reader agreeing is not evidence that Photoshop will.
8. **When a binary layout is ambiguous from the spec text alone, read a
   working implementation rather than infer it** (findings 5, 9, 10). This cost
   a search and a few minutes of reading each time; guessing wrong would have
   cost a session of debugging a file that opens but composites incorrectly.
