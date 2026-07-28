# The 95% — Concrete Workflows

**PRD §13 Step 2.** Turns the §10 success metric ("support 95% of common
Photoshop workflows") into a written, ranked list of concrete workflows
across the §4 personas. This list is the acceptance suite and the arbiter
for every "could we cut this?" question from Phase 2 onward — a feature
request gets checked against it, not against intuition.

## How to use this

Each workflow is a real end-to-end task a professional performs, not a
feature. It's tagged with the FRs it exercises and a tier:

- **Tier 1** — done daily/weekly by that persona; if Aurora can't do this,
  it hasn't replaced Photoshop for that persona, full stop.
- **Tier 2** — done regularly but not every session; a real gap, but not
  launch-blocking on its own.
- **Tier 3** — occasional/specialist; nice to cover, first to slip.

When a scope question comes up ("could we cut FR-X?"), check §4 below: an
FR touched by zero Tier-1 workflows is a real candidate to cut or defer,
regardless of how interesting it is to build. An FR touched by Tier-1
workflows across multiple personas is load-bearing and should be protected
even under schedule pressure.

## Methodology and its limits

This is a first draft written from general professional Photoshop usage
patterns and the FR list (§5), not from user interviews or telemetry —
Aurora has no users yet to interview. **Treat it as a hypothesis, not a
finding.** Cahya is both the design owner (FR-027) and, for now, the only
person who can sanity-check this against real professional experience;
revise freely rather than treating the tiers below as settled. The value
of writing it down isn't that it's correct on the first pass — it's that
disagreements now have something concrete to disagree *with*, instead of
each scope conversation re-litigating priorities from scratch.

---

## Core spine — shared by every persona (Tier 1 for all)

The workflow every persona performs regardless of specialty. If this
breaks, nothing else matters.

- **CORE-1**: Open a document (new, or import PSD/PNG/JPEG/RAW), make
  edits across multiple layers with adjustments and/or paint, undo/redo
  through a long session without loss, save, close, reopen with everything
  intact. *FR-001, FR-003, FR-010, FR-025.*
- **CORE-2**: Export a finished document to a delivery format (PNG/JPEG/
  PDF/TIFF as needed) at the right resolution and color space. *FR-016,
  FR-021.*
- **CORE-3**: Work across a session with dockable panels, keyboard
  shortcuts, and a customized workspace layout without fighting the UI.
  *FR-024, FR-027.*

---

## Photographer

*Needs: RAW editing, batch processing, color correction, retouching.*

**Tier 1**
- **PHO-1**: Import a shoot's RAW files, apply consistent exposure/white
  balance/lens correction across the batch, export client-ready JPEGs.
  *FR-015, FR-018, FR-021.*
- **PHO-2**: Grade a single RAW — exposure, white balance, curves,
  selective color, local adjustment masks — to a print-ready file.
  *FR-015, FR-010, FR-007, FR-016.*
- **PHO-3**: Portrait retouch — frequency-separation-style layer stack,
  spot healing, dodge/burn on a separate layer, subtle liquify, output
  sharpened for web or print. *FR-014, FR-011, FR-003, FR-021.*

**Tier 2**
- **PHO-4**: Batch resize, watermark, and rename a full gallery for client
  delivery. *FR-018, FR-021.*
- **PHO-5**: Exposure-blend or HDR merge via layer masks and blend modes
  for a high-dynamic-range scene. *FR-003, FR-007.*
- **PHO-6**: Convert to black & white with per-channel control (not a flat
  desaturate) for a fine-art print. *FR-010.*

**Tier 3**
- **PHO-7**: Dust/sensor-spot removal across a batch from a single
  reference frame. *FR-014, FR-018.*

---

## Graphic Designer

*Needs: Layers, Typography, Export, Print support.*

**Tier 1**
- **GD-1**: Compose a multi-layer piece (poster/flyer/ad) combining
  placed images, editable text, shape layers, blend modes, guides/grid,
  and export print-ready with correct color mode. *FR-003, FR-008, FR-009,
  FR-016, FR-021.*
- **GD-2**: Round-trip a PSD with a photographer or agency colleague on
  actual Photoshop — open their PSD, edit layers/text, save, they reopen
  in Photoshop with structure intact. *FR-001 (PSD/PSB compatibility).*
  This is the single highest-stakes Tier-1 workflow in the whole
  document — it's the literal precondition for adoption inside a mixed
  Aurora/Photoshop team, and the one failure mode (silent file corruption)
  the PRD singles out as unacceptable (R3b).
- **GD-3**: Build a layered template with editable text layers, reused
  across several localized/branded variants. *FR-003, FR-009, FR-013.*
- **GD-4**: Package export assets at multiple sizes/formats for a client
  or dev hand-off (logos, icons as PNG/SVG at several resolutions).
  *FR-021.*

**Tier 2**
- **GD-5**: Vector logo or icon touch-up — pen tool paths, boolean
  operations — placed into a raster layout. *FR-008.*
- **GD-6**: Soft-proof a CMYK document before sending to press.
  *FR-016.*

---

## Digital Artist

*Needs: Brushes, Painting engine, Tablet support, Perspective tools.*

**Tier 1**
- **DA-1**: Paint a piece start to finish — line art, flat colors,
  shading — in one layered document, with custom brushes and tablet
  pressure/tilt, entirely inside Aurora. *FR-005, FR-006, FR-003.* This is
  effectively the Phase 2 exit criterion in workflow form ("an illustrator
  completes a real piece without leaving Aurora," PRD §9) — if any single
  workflow in this document deserves to be the literal Phase 2 acceptance
  test, it's this one.
- **DA-2**: Build and reuse a personal brush set (texture, dual brush,
  scatter, dynamics) across a project. *FR-005, FR-023.*

**Tier 2**
- **DA-3**: Concept environment art using perspective-painting guides.
  *FR-006, FR-012.*
- **DA-4**: Photobash — clone/heal/mix reference photos into a painted
  piece. *FR-006, FR-014.*

**Tier 3**
- **DA-5**: Symmetry painting for character/creature design. *FR-006.*

---

## UI Designer

*Needs: Artboards, Vector graphics, Export assets.*

**Tier 1**
- **UID-1**: Lay out multiple app/web screens as separate boards in one
  document, using vector shapes and icons with consistent spacing via
  guides. *FR-002, FR-008.* **Gap surfaced by this exercise**: §4 lists
  "Artboards" as a named need, but no FR in §5 owns multiple named boards
  in one canvas — FR-002 (Canvas) has guides/grid/snap/rulers/multiple
  tabs and windows, but not artboards specifically, and FR-008 (Vector
  Graphics) doesn't mention them either. This is either an implicit
  sub-feature of FR-002 that was never written down, or a real scope gap.
  Flagged in PRD.md; needs an owner decision, not an assistant guess.
- **UID-2**: Export assets at multiple densities (@1x/2x/3x or similar)
  as PNG/SVG for a dev handoff. *FR-021.*

**Tier 2**
- **UID-3**: Build a reusable component (buttons, icons) via linked/smart
  objects so one edit propagates everywhere it's placed. *FR-013.*

**Tier 3**
- **UID-4**: Redline/spec export — a layer-metadata-annotated sheet for
  developer handoff. *FR-021, FR-023.*

---

## Marketing Team

*Needs: Templates, Social media exports, AI image generation.*

**Tier 1**
- **MKT-1**: Start from a template, swap image/text/brand elements, export
  at the aspect ratios a given platform requires. *FR-001 (Templates),
  FR-021 (batch export presets).*

**Tier 2**
- **MKT-2**: Generate or extend an image with AI (generative fill/expand,
  background removal) as a starting point for a campaign asset. *FR-017.*
  Tier 2, not Tier 1, deliberately — FR-017 is priority **Could** (§3);
  this workflow is real but not load-bearing enough to justify pulling AI
  features forward.

**Tier 3**
- **MKT-3**: Batch-produce a set of social variants from one master via
  automation/actions. *FR-018.*

---

## Cross-persona rollup

FRs touched by Tier-1 workflows from **three or more personas** — the
clearest "protect this" signal in the whole document:

| FR | Touched by Tier-1 workflows from |
|---|---|
| FR-001 Document Management | Core, Photographer, Graphic Designer, Marketing |
| FR-003 Layer System | Core, Photographer, Graphic Designer, Digital Artist |
| FR-021 Export | Core, Photographer, Graphic Designer, UI Designer, Marketing |
| FR-016 Color Management | Core, Photographer, Graphic Designer |

FRs touched by **zero** Tier-1 workflows anywhere in this document —
candidates to double-check against §3's MoSCoW table, not a recommendation
to cut on their own:

- FR-017 AI Features (Could) — only Tier 2, only Marketing
- FR-018 Automation (Could) — only Tier 2/3
- FR-019 Plugin SDK (Won't yet) — touched by no workflow in this document at all; consistent with its already-deferred status
- FR-020 Animation (Won't yet) — same
- FR-022 Collaboration (Won't yet) — same

This is a consistency check, not new information: everything landing in
"zero Tier-1" here already matches Could/Won't-yet in §3's priority table.
That agreement is the point — if a Must/Should FR had shown up with no
Tier-1 workflow, *that* would have been a finding worth raising.

---

## Gaps this exercise surfaced

- **Artboards (UID-1)** — a named persona need in §4 with no explicit
  owning feature in §5. **Format-level question resolved 2026-07-28**
  (`spike/psd-write/FINDINGS.md` finding 17, found while assembling the
  PSD test corpus): an artboard is a plain layer group plus one
  `Descriptor`-shaped tagged block (`artb`) this spike's code already
  knows how to read/write the shape of — FR-003 + FR-001 territory, not a
  new document primitive. What's still undecided is the *product* question
  — does Aurora expose artboards as a first-class UI concept, or leave
  them as plain groups — which is a real decision for Cahya, just a
  cheaper one than it looked before this was checked.

---

*Written 2026-07-28 as part of closing out PLAN.md 0.9 / PRD §13 Step 2.
Revise the tiers and add/remove workflows as real usage (or Cahya's own
professional judgment) contradicts them — this document is meant to be
argued with, not archived.*
