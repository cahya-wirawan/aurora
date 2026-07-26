# PSD write spike — findings

PLAN 0.6, [ADR 0004](../../docs/adr/0004-psd-full-write.md). Built 2026-07-26.

Aurora commits to full layered PSD *write*: a file edited in Aurora must reopen
in Photoshop with its layers intact. Phase 3 is ten months long and rests on that
being feasible. This spike writes a real layered PSD from scratch to test the
assumption.

```sh
cd spike/psd-write
cargo run        # writes out/spike.psd
./verify.sh      # checks it with two independent readers
```

## Verdict: feasible, and the easy 80 % is genuinely easy

A hand-written PSD with five layers — names, positions, per-layer alpha,
opacity, blend modes, visibility, and a non-ASCII name — is accepted by two
independent readers, with layer pixel data verified correct and the flattened
preview matching a re-composite from layer data to within rounding.

**But "accepted by a reader" is a much weaker claim than it sounds**, and the
findings below are mostly about that gap.

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

## Scope: what this did *not* touch

Deliberately the easy 20 % of the work. Untested and unimplemented:

- Groups and nesting (`lsct` section dividers)
- Layer masks and vector masks
- **Editable text layers** (`TySh`) — the hardest single item, and one ADR 0004
  promises
- Smart objects, embedded and linked
- Layer styles and effects
- Adjustment layers
- 16/32-bit, CMYK, Lab, Grayscale
- RLE and ZIP compression *(raw only here; real files are compressed and much
  larger uncompressed — this 320×240 file is 763 KB)*
- PSB (>30,000 px)
- ICC profiles and metadata in the image-resources section

**Do not read this spike as "PSD write is a solved problem."** It establishes
that the container, layer records, channel data, and Unicode naming are
tractable. The features listed above are where the ten months actually go, and
text layers in particular remain unassessed.

## Recommendations for Phase 3

1. **Gate on pixel comparison, not on files opening.** Finding 1 is the reason.
2. **Write the flatten through the real render graph**, so preview and canvas
   cannot diverge (finding 2).
3. **Build the 1,000-file corpus before the writer**, per PLAN 0.7, and diff
   every round-trip in CI from the first layer type.
4. **Spike text layers separately and early** — it is the largest remaining
   unknown, and if it proves infeasible the lossy-conversion warning policy
   (FR-001) has to cover it explicitly.
5. **Get a Photoshop licence for verification.** Nothing else settles ADR 0004,
   and an independent reader agreeing is not evidence that Photoshop will.
