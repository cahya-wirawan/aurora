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
cargo test                 # descriptor.rs round-trip tests against real bytes
```

## Verdict: pixel layers and groups are genuinely easy; text layers are harder than planned

A hand-written PSD with a nested layer tree — a top-level layer, a two-level
group hierarchy, per-layer alpha, opacity, blend modes, visibility, and a
non-ASCII name — is accepted by two independent readers, with layer pixel data
verified correct and the flattened preview matching hand-computed blend math
exactly.

**But "accepted by a reader" is a much weaker claim than it sounds**, and
findings 1–5 below are mostly about that gap.

**Text layers (findings 6–9) turned out to be the more consequential result
of this spike.** A real Photoshop text layer's `EngineData` is far richer
than a naive minimal example — hand-generating it from scratch carries real,
untested risk (finding 6). The lower-risk path — patch a real file's bytes
rather than generate from nothing — was proven out end-to-end (finding 7) and
its core mechanics validated in Rust against real bytes (finding 9). But that
same investigation surfaced a mandatory piece of engineering that was not in
the original Phase 3 scope at all: **editing text content requires rendering
actual glyphs into the layer's pixel channels**, or the resulting file is
visually inconsistent — descriptor says one thing, pixels show another
(finding 8). This is the most important finding in the document.

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

**What the Rust side does *not* do, on purpose:** the full end-to-end
file-level splice (patching a complete PSD's byte stream, including the
enclosing layer-record and layer-info length prefixes) was proven out in
Python (finding 7) and not re-implemented in Rust for this spike — once the
format understanding was validated in the target language (finding 9's
tests), re-deriving the same file-level result a second time would have
spent effort on engineering polish rather than answering a further open
question. That full Rust-side splice, plus rendering glyphs into pixel
channels (finding 8), is Phase 3 scope.

## Scope: what this did *not* touch

Groups and the text-layer *descriptor* mechanics are done; the rest is still
the easy-versus-hard split. Untested and unimplemented:

- Layer masks and vector masks
- **`EngineData`'s own text-format writer** — this spike's Rust code treats
  `EngineData` as an opaque `tdta` blob (finding 6); generating or modifying
  its internal paragraph/style/resource structure from scratch is unstarted
  and, per finding 6, higher-risk than anything else in this document
- **Rendering glyphs into pixel channels to keep text layers visually
  consistent** (finding 8) — the single most important piece of unstarted
  work this spike surfaced
- Smart objects, embedded and linked
- Layer styles and effects
- Adjustment layers
- 16/32-bit, CMYK, Lab, Grayscale
- RLE and ZIP compression *(raw only here; real files are compressed and much
  larger uncompressed — this 320×240 file is 763 KB)*
- PSB (>30,000 px)
- ICC profiles and metadata in the image-resources section

**Do not read this spike as "PSD write is a solved problem," and do not read
the text-layer result as "text layers are solved."** What's established: the
container, layer records, channel data, Unicode naming, and groups are
tractable from scratch (finding 5); the `TySh` *container* format and a
patch-a-real-file strategy are tractable and demonstrated end-to-end
(findings 7, 9); but full from-scratch `EngineData` generation is
higher-risk than assumed (finding 6), and the pixel/vector sync requirement
(finding 8) is real, mandatory, unstarted engineering work.

## Recommendations for Phase 3

1. **Gate on pixel comparison, not on files opening.** Finding 1 is the reason.
2. **Write the flatten through the real render graph**, so preview and canvas
   cannot diverge (finding 2).
3. **Build the 1,000-file corpus before the writer**, per PLAN 0.7, and diff
   every round-trip in CI from the first layer type.
4. **For text layers, patch real files rather than generate `EngineData`
   from scratch** (finding 7) — collect a small corpus of real Photoshop text
   layers spanning common cases (multi-style runs, paragraph vs point text,
   warped text) and build the writer against them, the same way finding 7's
   proof-of-concept did for the simple case.
5. **Budget real engineering time for glyph rendering into pixel channels**
   (finding 8) — this is not a follow-on detail, it's required for any text
   edit to produce a non-broken file, and it did not appear in the original
   Phase 3 scoping.
6. **Get a Photoshop licence for verification.** Nothing else settles ADR 0004,
   and an independent reader agreeing is not evidence that Photoshop will.
7. **When a binary layout is ambiguous from the spec text alone, read a
   working implementation rather than infer it** (findings 5, 9). This cost
   a search and a few minutes of reading each time; guessing wrong would have
   cost a session of debugging a file that opens but composites incorrectly.
