# Changelog

All notable changes to Aurora are listed here, newest version first, in the
spirit of [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and
[Semantic Versioning](https://semver.org/) (Aurora is 0.x: a minor version is
one task-level unit of work, a patch version is a bug fix).

This file is **generated** by `scripts/gen_changelog.py` from git history: one
line per version, taken from that version's commit subject, dated with the
commit date. Do not edit it by hand; change the script or the commit subjects.
The full account of every version (design decisions, measurements,
disclosed weaknesses) is in [PLAN.md](PLAN.md); where Aurora stands today is in
[STATUS.md](STATUS.md). Versions are ordered by version number, so a patch
merged late (for example 0.186.1, merged to `main` after 0.187.0) sits in
version position, and each date is the commit date of that version's own
commit, not of the merge.

## [Unreleased]

Work on a branch that has not been merged to `main` is not listed here until
it is merged.

## [0.283.0] - 2026-10-20

### Added

- Record the PSD fidelity round

## [0.282.0] - 2026-10-20

### Added

- Record CMYK and 32-bit float PSD reading

## [0.281.0] - 2026-10-20

### Added

- Record the centred small document

## [0.280.1] - 2026-10-20

### Added

- Record the Windows display-test fix

## [0.280.0] - 2026-10-19

### Added

- Record the persistent display texture

## [0.279.0] - 2026-10-19

### Added

- Record the PhotoCraft frame-rate comparison

## [0.278.0] - 2026-10-19

### Added

- Record the zoomed-out view's blend between pyramid levels

## [0.277.0] - 2026-10-19

### Added

- Record the faster open and export: an exact colour-conversion memo, banded dithering and a pooled export composite

## [0.276.1] - 2026-10-19

### Added

- Record the CI fix after the PhotoCraft merge

## [0.276.0] - 2026-10-19

### Added

- Record the PhotoCraft comparison: a headless harness measured on Linux and the Mac

## [0.275.0] - 2026-10-19

### Added

- Record the zoomed-out view: the canvas zooms out to 1/32 over a composite pyramid, with Fit on Screen and 100%

## [0.274.0] - 2026-10-18

### Added

- Record 60 FPS round 9: 24-60 MP photos measured, Curves/Levels tabulated, the pool's snapshot scoped, the dab's page faults ended

## [0.273.0] - 2026-10-18

### Added

- Record 60 FPS round 8: the frame pool's caller composites, opaque chunks skip straightening

## [0.272.0] - 2026-10-18

### Added

- Record 60 FPS round 7: inlined CPU fold blend term, frame pool for larger recomposites

## [0.271.0] - 2026-10-17

### Added

- Record 60 FPS round 6: per-mode CPU fold, background blend pipelines, startup warm-up

## [0.270.2] - 2026-10-17

### Fixed

- Thumbnail tests: settle waits until no further update is due

## [0.270.1] - 2026-10-17

### Fixed

- Cargo deny: ignore the zbus-family unmaintained advisories reached via accesskit_unix

## [0.270.0] - 2026-10-17

### Changed

- 60 FPS round 5: docs and version

## [0.269.0] - 2026-10-17

### Changed

- 60 FPS round 4: docs and version; gate the endian-specific upload tests

## [0.268.0] - 2026-10-17

### Changed

- 60 FPS round 3: docs, version, stale anchor comments, pan test name

## [0.267.0] - 2026-10-17

### Changed

- 60 FPS round 2: a live Move invalidates narrowly, docs and version

## [0.266.1] - 2026-10-17

### Fixed

- Colour tests: compare ICC bytes outside lcms2's time stamps

## [0.266.0] - 2026-10-17

### Changed

- Clip flat images after a profile conversion; repair four corpus tests: docs and version

## [0.265.0] - 2026-10-17

### Changed

- Faster GPU-path frames and the stale-canvas fix: docs and version

## [0.264.0] - 2026-10-17

### Changed

- Colour management round 1: embedded profiles are honoured, docs and version

## [0.263.1] - 2026-10-17

### Fixed

- Fix the redundant rustdoc link that failed the Docs job

## [0.263.0] - 2026-10-17

### Changed

- PSD save writes layer effects, verified kinds only: docs and version

## [0.262.0] - 2026-10-17

### Changed

- Tooltips appear in the app: docs and version

## [0.261.0] - 2026-10-17

### Changed

- Gradient and pattern strokes are drawn: docs and version

## [0.260.1] - 2026-10-16

### Fixed

- Command palette: its scrollbar takes presses and drags

## [0.260.0] - 2026-10-16

### Changed

- Document tabs: a window of tabs and a More documents menu

## [0.259.0] - 2026-10-16

### Changed

- Menus and dropdown lists stay inside the window and scroll

## [0.258.0] - 2026-10-16

### Fixed

- Fix the overlapping Command Palette rows: fixed rows and a scrolling list

## [0.257.0] - 2026-10-16

### Changed

- Polish round: Photography and Graphic workspaces, new icons, first-run welcome

## [0.256.0] - 2026-10-16

### Changed

- Gradient Map stop editor with a GradientBar preview

## [0.255.0] - 2026-10-16

### Changed

- Exposure in Grayscale PSDs through the file's gray profile

## [0.254.0] - 2026-10-16

### Added

- Draw Pattern Overlay layer effects

## [0.253.0] - 2026-10-16

### Added

- Draw Gradient Overlay layer effects

## [0.252.0] - 2026-10-16

### Changed

- Upgrade zip to 9.0.3

## [0.251.0] - 2026-10-16

### Added

- Draw pixel-less fill layers and artboard backgrounds when a PSD opens

## [0.250.0] - 2026-10-16

### Changed

- GPU compositing for the seven newest adjustment layers

## [0.249.0] - 2026-10-16

### Changed

- Grayscale PSD adjustments: Curves, Levels, Invert, Threshold, Posterize, Brightness/Contrast

## [0.248.0] - 2026-10-16

### Changed

- Gradient Map adjustment layer and the Color Lookup finding

## [0.247.0] - 2026-10-15

### Changed

- Safe export and non-fatal directory-sync failure

## [0.246.0] - 2026-10-15

### Fixed

- Harden the .aur save: durable Save As, hostile origins, deterministic manifest

## [0.245.0] - 2026-10-15

### Changed

- Channel Mixer and Selective Color adjustment layers

## [0.244.0] - 2026-10-15

### Changed

- Invert, Threshold, Posterize and Brightness/Contrast adjustment layers

## [0.243.0] - 2026-10-15

### Changed

- PSD fidelity audit, Pass Through group Fill, group clip bases and blend poles

## [0.242.0] - 2026-10-15

### Changed

- GPU compositing for Vibrance, Exposure, Photo Filter, Black & White and Color Balance

## [0.241.0] - 2026-10-15

### Changed

- PSD read and write for Vibrance, Exposure, Photo Filter, Black & White and Color Balance

## [0.240.0] - 2026-10-15

### Changed

- Black & White and Color Balance adjustment layers

## [0.239.0] - 2026-10-15

### Changed

- Vibrance, Exposure and Photo Filter adjustment layers

## [0.238.0] - 2026-10-15

### Changed

- Select Colour Range and the Quick Select tool

## [0.237.0] - 2026-10-14

### Changed

- Spot Healing tool: paint a blemish away, healed on release as one undo step

## [0.236.0] - 2026-10-14

### Changed

- Crop ratio presets: ratio dropdown, Portrait, whole-pixel constrained geometry

## [0.235.0] - 2026-10-14

### Changed

- WIP 092: pixel clipboard pure module, arboard image-data

## [0.234.0] - 2026-10-14

### Changed

- Paths, round 2: Fill Path, Stroke Path, Make Work Path from Selection, Pen editing

## [0.233.0] - 2026-10-14

### Changed

- WIP: path model

## [0.232.0] - 2026-10-14

### Changed

- WIP: Channels panel

## [0.231.0] - 2026-10-14

### Changed

- Crop polish: cursor, arrow-key nudging and rule-of-thirds guides

## [0.230.0] - 2026-10-14

### Changed

- Composite the soft Brush's strokes instead of keeping the higher alpha

## [0.229.0] - 2026-10-14

### Changed

- Give the Eraser Opacity and Flow

## [0.228.3] - 2026-10-14

### Fixed

- Stop the options bar squeezing its readouts in a narrow window

## [0.228.2] - 2026-10-14

### Fixed

- Gate the legacy pin's imports to Linux so macOS and Windows build warning-free

## [0.228.1] - 2026-10-14

### Fixed

- Pin the legacy paint hashes on Linux only; macOS powf differs in the last bit

## [0.228.0] - 2026-10-14

### Added

- Add Opacity and Flow to the Brush, Pencil and Clone Stamp

## [0.227.0] - 2026-10-14

### Added

- Add the Crop tool's shield and the crop.shield token

## [0.226.1] - 2026-10-14

### Fixed

- Fix the soft Brush's beading with dense, per-event-capped dabs

## [0.226.0] - 2026-10-13

### Added

- Add the Crop tool and Crop to Selection: a non-destructive, undoable crop

## [0.225.0] - 2026-10-13

### Added

- Add an in-window menu bar for Linux and Windows

## [0.224.0] - 2026-10-13

### Added

- Add the Dodge, Burn and Sponge tools: one toning engine on straight sRGB

## [0.223.0] - 2026-10-13

### Added

- Add the Blur and Sharpen tools: one filter engine, 3x3 [1 2 1] average

## [0.222.0] - 2026-10-13

### Added

- Add the Smudge tool: carried patch, strength, finger painting off

## [0.221.0] - 2026-10-13

### Added

- Add the Clone Stamp tool: aligned mode, stroke-start source

## [0.220.0] - 2026-10-13

### Added

- Add the Pencil tool: a hard-edged, aliased dab

## [0.219.0] - 2026-10-13

### Added

- Add selection ants, Quick Mask overlay and control size tokens

## [0.218.0] - 2026-10-13

### Changed

- Persist selections in .aur and bound selection history

## [0.217.1] - 2026-10-13

### Fixed

- Harden two timing tests after CI and a Mac Metal run

## [0.217.0] - 2026-10-13

### Added

- Add Quick Mask mode

## [0.216.0] - 2026-10-13

### Added

- Add Feather, Expand, Contract, Smooth and Border selection commands

## [0.215.0] - 2026-10-13

### Added

- Add selection modes: add, subtract, intersect

## [0.214.0] - 2026-10-13

### Added

- Add Lasso, Polygonal Lasso and Magic Wand selection tools

## [0.213.0] - 2026-10-12

### Changed

- Raster selection model, Elliptical Marquee and undoable selection steps

## [0.212.0] - 2026-10-12

### Added

- Show the selection as marching ants; add Select All, Deselect, Inverse

## [0.211.1] - 2026-10-12

### Fixed

- Measure per-texel conditioning in the translucent GPU adjustment tests

## [0.211.0] - 2026-10-12

### Changed

- Paint through the active selection: Brush, Eraser, Paint Bucket, Gradient

## [0.210.0] - 2026-10-12

### Changed

- Bound pixel-history memory: compress snapshots, spill the oldest to scratch

## [0.209.1] - 2026-10-12

### Fixed

- Raise the translucent GPU test alpha floor to 0.25 and widen the amplifying-mode bound

## [0.209.0] - 2026-10-12

### Changed

- Paint Bucket and Gradient tools

## [0.208.2] - 2026-10-12

### Fixed

- Widen the translucent GPU bound for Apple adapters; whitespace-insensitive source guard for CRLF checkouts

## [0.208.1] - 2026-10-12

### Fixed

- Colour controls: clamp Eyedropper picks, pure App call-site helpers, doc and sync cleanup

## [0.208.0] - 2026-10-12

### Changed

- Foreground/background colour: model, Tools panel swatches, picker popover, commands

## [0.207.1] - 2026-10-12

### Fixed

- Widen GPU adjustment test bounds for WARP/Metal adapters; merge main

## [0.207.0] - 2026-10-12

### Changed

- Rebuild the Tools panel Photoshop-style: icon-only, two columns, grouped

## [0.206.1] - 2026-10-12

### Fixed

- Hold steep Hue/Saturation GPU cases to the opaque tolerance on translucent backdrops

## [0.206.0] - 2026-10-12

### Changed

- GPU passes for Levels and Hue/Saturation adjustment layers

## [0.205.0] - 2026-10-12

### Changed

- Save Curves, Levels and Hue/Saturation adjustment layers to PSD

## [0.204.0] - 2026-10-12

### Changed

- Hue/Saturation per-colour-range adjustments

## [0.203.0] - 2026-10-12

### Added

- Add a Hue/Saturation adjustment layer

## [0.202.1] - 2026-10-12

### Fixed

- Make the thumbnail test helper wait for deferred scratch reads

## [0.202.0] - 2026-10-12

### Added

- Add a Levels adjustment layer

## [0.201.1] - 2026-10-12

### Added

- Add a Clip icon to the Layers panel's clipped rows

## [0.201.0] - 2026-10-12

### Added

- Add clipping masks

## [0.200.0] - 2026-10-12

### Changed

- Preserve PSD smart objects on open and save

## [0.199.0] - 2026-10-12

### Added

- Add name search and kind filter to the Layers panel

## [0.198.0] - 2026-10-12

### Changed

- Enforce layer locks and add the Lock row to the Layers panel

## [0.197.0] - 2026-10-12

### Added

- Show layer thumbnails in the Layers panel rows

## [0.196.0] - 2026-10-11

### Added

- Add the Layers panel toolbar and a New Group command

## [0.195.1] - 2026-10-11

### Fixed

- Count pending hits as paging in a residency test

## [0.195.0] - 2026-10-11

### Changed

- Rebuild the Layers panel rows Photoshop-style: eye, chevrons, Fill

## [0.194.0] - 2026-10-11

### Added

- Add a provisional vector icon foundation for the Layers panel

## [0.193.0] - 2026-10-11

### Changed

- Clip the live canvas to the document; pan bound is the document corner

## [0.192.1] - 2026-10-11

### Fixed

- Stop the canvas encoding colours twice on sRGB surfaces

## [0.192.0] - 2026-10-11

### Changed

- Bound caches and open overlap on small machines

## [0.191.1] - 2026-10-11

### Fixed

- Hide a knockout drop shadow under the outside stroke band too

## [0.191.0] - 2026-10-11

### Added

- Draw Softer Outer Glow layer effects

## [0.190.0] - 2026-10-11

### Added

- Draw solid Stroke layer effects: outside, inside, center

## [0.189.0] - 2026-10-11

### Changed

- Speed up opening large PSDs: tables, blank tiles, parallel encode

## [0.188.0] - 2026-10-11

### Changed

- Composite the background worker's tiles in parallel

## [0.187.0] - 2026-10-11

### Changed

- Give the active document the bigger share of the tile pool

## [0.186.1] - 2026-10-11

### Fixed

- Skip the background-composite corpus test when no PSD is present

## [0.186.0] - 2026-10-11

### Changed

- Composite the CPU path's whole-view recomposite on a background worker

## [0.185.1] - 2026-10-11

### Fixed

- Fix two tests that failed in CI without the local corpus

## [0.185.0] - 2026-10-11

### Changed

- Size the tile residency budget from system RAM

## [0.184.0] - 2026-10-11

### Changed

- Skip transparent source chunks and cut per-fold cost

## [0.183.0] - 2026-10-11

### Changed

- Cull layers hidden under an opaque cover, and measure where time goes

## [0.182.0] - 2026-10-11

### Added

- Draw Photoshop drop shadows, with a coverage cache

## [0.181.0] - 2026-10-11

### Added

- Apply layer Fill and draw Color Overlay effects

## [0.180.0] - 2026-10-11

### Changed

- Parse and store PSD layer effects, without rendering

## [0.179.0] - 2026-10-10

### Changed

- Composite Photoshop Pass Through groups for real

## [0.178.0] - 2026-10-10

### Added

- Write groups and layer masks to PSD

## [0.177.0] - 2026-10-10

### Changed

- Save As Photoshop from the app, with an itemised warning

## [0.176.0] - 2026-10-10

### Added

- Add a safe, streaming PSD writer for flat documents

## [0.175.1] - 2026-10-10

### Fixed

- Fix tests that pressed Ctrl for shortcuts that are Cmd on macOS

## [0.175.0] - 2026-10-10

### Changed

- Polish document tabs: macOS Quit review, Cmd chords, tab close buttons

## [0.174.0] - 2026-10-10

### Changed

- Track unsaved changes, close documents, and review on quit

## [0.173.0] - 2026-10-10

### Added

- Show open documents in a tab strip above the canvas

## [0.172.0] - 2026-10-10

### Added

- Open several documents and switch between them

## [0.171.1] - 2026-10-10

### Fixed

- Harden per-session autosave before multiple documents

## [0.171.0] - 2026-10-10

### Changed

- Give each document session its own crash-recovery autosave

## [0.170.0] - 2026-10-10

### Changed

- Extract the open document into a DocumentSession

## [0.169.0] - 2026-10-10

### Changed

- Route dialog action choices back to the App

## [0.168.0] - 2026-10-10

### Added

- Add workspace presets

## [0.167.0] - 2026-10-10

### Changed

- Float panels inside the main window

## [0.166.0] - 2026-10-10

### Changed

- Drag docked panels to re-dock them, with the arrangement saved

## [0.165.2] - 2026-10-10

### Fixed

- Fix a load-sensitive scratch lock race test

## [0.165.1] - 2026-10-10

### Fixed

- Fix a CI race in an insert_encoded store test

## [0.165.0] - 2026-10-10

### Changed

- Collapse the right rail to a narrow label strip

## [0.164.0] - 2026-10-10

### Changed

- Group Properties and History as tabs in one rail slot

## [0.163.0] - 2026-10-10

### Added

- Show Photoshop's physical zoom in the status bar

## [0.162.0] - 2026-10-09

### Added

- Add a status bar; make the Layers row cap a token

## [0.161.1] - 2026-10-09

### Fixed

- Fix a CI race in the background-autosave install test

## [0.161.0] - 2026-10-09

### Changed

- Keep the Curves editor visible in Properties

## [0.160.0] - 2026-10-09

### Added

- Add a left tools panel and an options bar

## [0.159.0] - 2026-10-09

### Changed

- Composite Curves layers on the GPU path

## [0.158.0] - 2026-10-09

### Changed

- Import PSD Curves adjustment layers

## [0.157.0] - 2026-10-09

### Changed

- Interpolate curves like Photoshop: natural spline, 19 points

## [0.156.0] - 2026-10-09

### Changed

- Curves UI: editor, channel tabs and histogram

## [0.155.0] - 2026-10-09

### Added

- Add a non-destructive Curves adjustment layer

## [0.154.0] - 2026-10-09

### Changed

- Stream PSD opens layer by layer through staging files

## [0.153.0] - 2026-10-09

### Changed

- Insert pre-encoded tiles on open

## [0.152.0] - 2026-10-09

### Added

- Write the autosave off the UI thread

## [0.151.0] - 2026-10-09

### Changed

- Decode opened files off the UI thread

## [0.150.0] - 2026-10-09

### Added

- Apply PSD vector masks on open

## [0.149.0] - 2026-10-09

### Added

- Apply PSD mask density, non-destructively

## [0.148.0] - 2026-10-08

### Changed

- Click a History row to jump to that step

## [0.147.1] - 2026-10-08

### Fixed

- History panel: list strokes, mark the current step

## [0.147.0] - 2026-10-08

### Added

- Apply PSD layer masks and open Grayscale PSDs

## [0.146.0] - 2026-10-08

### Added

- Show a scrollbar on overflowing dock panels

## [0.145.0] - 2026-10-08

### Changed

- Scroll the docked panels with the wheel and trackpad

## [0.144.1] - 2026-10-08

### Fixed

- Stop Layers rows overlapping in a short window

## [0.144.0] - 2026-10-08

### Added

- Open Photoshop files: PSD/PSB layered read

## [0.143.1] - 2026-10-08

### Fixed

- Never fail silently when opening or saving a file

## [0.143.0] - 2026-10-08

### Added

- Add and delete layers: New Layer, Delete Layer

## [0.142.0] - 2026-10-08

### Changed

- Make the Layers panel recognisable: titles, active-row highlight, ellipsis

## [0.141.0] - 2026-09-28

### Added

- Draw dialog titles

## [0.140.0] - 2026-09-28

### Changed

- Measure text in layout; draw checkbox labels

## [0.139.0] - 2026-09-28

### Changed

- Blink the text caret without busy-looping

## [0.138.0] - 2026-09-28

### Changed

- Place the text-field caret with the pointer; sticky field scroll

## [0.137.0] - 2026-09-28

### Changed

- Keyboard chords in a focused text field: clipboard, select all, field undo

## [0.136.0] - 2026-09-28

### Changed

- Edit the brush and eraser radius from the Properties panel

## [0.135.0] - 2026-09-28

### Changed

- Edit the active layer's opacity, blend mode and visibility from the Layers panel

## [0.134.0] - 2026-09-28

### Changed

- Match the slider to the design mockup

## [0.133.1] - 2026-09-28

### Fixed

- Stop the slider and text field stretching in a column

## [0.133.0] - 2026-09-26

### Added

- Draw text-field content, caret and selection; palette, tooltip and dialog text

## [0.132.0] - 2026-09-26

### Added

- Draw real text: bundled font, glyph atlas, widget labels

## [0.131.1] - 2026-09-26

### Fixed

- Let the command palette's query take a space

## [0.131.0] - 2026-09-26

### Changed

- Widget Gallery: drags, tooltip hover timer, text-field typing

## [0.130.0] - 2026-09-26

### Added

- Add an in-app Widget Gallery panel with click and keyboard input

## [0.129.0] - 2026-09-25

### Changed

- Paint a keyboard focus ring: focus-visible, two-colour, inline

## [0.128.0] - 2026-09-25

### Changed

- Route accesskit action requests to widgets

## [0.127.0] - 2026-09-25

### Added

- Add a popover layer: overlays paint on top, escape clipping, hit first

## [0.126.0] - 2026-09-25

### Added

- Add a Curve editor widget and a reusable ToneCurve model

## [0.125.0] - 2026-09-25

### Added

- Add a Colour picker widget: 2-D saturation/value square, hue strip, preview

## [0.124.0] - 2026-09-25

### Added

- Add a vertex-coloured gradient paint primitive, a separate pipeline

## [0.123.0] - 2026-09-25

### Added

- Add a Menu widget: single-level popup menu with separators; share bordered_surface

## [0.122.0] - 2026-09-25

### Added

- Add a Tooltip widget: delayed, hoverable, dismissible Role::Tooltip child

## [0.121.0] - 2026-09-25

### Added

- Add a Tab bar widget: roving-focus tab list with automatic activation

## [0.120.0] - 2026-09-25

### Added

- Add a Dropdown widget: select-only combo box with a real option list

## [0.119.1] - 2026-09-25

### Fixed

- Correct six documentation defects from the Divide round

## [0.119.0] - 2026-09-25

### Added

- Port Divide to the GPU compositing path

## [0.118.1] - 2026-09-25

### Fixed

- Correct five documentation defects from the Subtract round

## [0.118.0] - 2026-09-25

### Added

- Port Subtract to the GPU compositing path

## [0.117.1] - 2026-09-20

### Fixed

- Correct SoftLight's kill margin, bound its "every finite cb" claim

## [0.117.0] - 2026-09-19

### Added

- Port SoftLight to the GPU compositing path, completing the overlay family

## [0.116.2] - 2026-09-05

### Fixed

- Correct PinLight's backdrop-wins degeneracy, its stale counts, and pin the unclamped divide

## [0.116.1] - 2026-09-05

### Fixed

- Fix three broken intra-doc links 0.115.2 shipped

## [0.116.0] - 2026-09-05

### Added

- Port PinLight to the GPU compositing path

## [0.115.2] - 2026-09-05

### Fixed

- Stop a forgotten tile's still-in-flight write from orphaning its file

## [0.115.1] - 2026-09-05

### Fixed

- Close the HardMix round's coverage regression and six disclosure defects

## [0.115.0] - 2026-09-05

### Added

- Port HardMix to the GPU compositing path

## [0.114.1] - 2026-09-05

### Fixed

- Correct VividLight's transpose-observability claim and eight stale counts

## [0.114.0] - 2026-09-05

### Added

- Port VividLight to the GPU compositing path

## [0.113.1] - 2026-09-05

### Fixed

- Make the transpose guard ask the arithmetic, not the opacity

## [0.113.0] - 2026-09-05

### Added

- Port LinearLight to the GPU compositing path

## [0.112.1] - 2026-09-05

### Fixed

- Close three real holes in 0.112.0's own count guards

## [0.112.0] - 2026-09-05

### Changed

- Derive and cross-check the drifting blend-mode counts

## [0.111.1] - 2026-09-05

### Fixed

- Sweep the HardLight port's surviving Overlay-side prose claims

## [0.111.0] - 2026-09-05

### Added

- Port HardLight to the GPU compositing path

## [0.110.1] - 2026-09-05

### Fixed

- Correct 0.110.0's own prose: five review findings, zero logic

## [0.110.0] - 2026-09-05

### Added

- Port Overlay to the GPU compositing path

## [0.109.1] - 2026-09-05

### Fixed

- Resolve 0.109.0's open NaN question and correct its prose

## [0.109.0] - 2026-09-05

### Changed

- Extract composite.wgsl's shared straighten/fold into two helpers

## [0.108.1] - 2026-09-05

### Fixed

- Correct 0.108.0's stale, miscounted and mode-transplanted prose

## [0.108.0] - 2026-09-05

### Added

- Port ColorDodge to the GPU compositing path

## [0.107.1] - 2026-09-05

### Fixed

- Correct 0.107.0's stale mode counts and wrong illustrative values

## [0.107.0] - 2026-09-05

### Added

- Port ColorBurn to the GPU compositing path

## [0.106.3] - 2026-09-05

### Fixed

- Fix the 22nd broken intra-doc link, invisible to the Linux-only docs job

## [0.106.2] - 2026-09-05

### Fixed

- Fix all 21 private-item broken intra-doc links, and make CI catch them

## [0.106.1] - 2026-09-05

### Fixed

- Retire two stale mode counts, and close the transpose guard's occlusion hole

## [0.106.0] - 2026-09-05

### Added

- Port LinearBurn to the real GPU compositing path

## [0.105.3] - 2026-09-05

### Fixed

- Document and guard the transpose-coverage property

## [0.105.2] - 2026-09-04

### Fixed

- Close the Lighten/Screen/Difference dispatch-arm transpose gap

## [0.105.1] - 2026-09-04

### Fixed

- Close LinearDodge's dispatch-arm transpose gap, and five doc fixes

## [0.105.0] - 2026-09-04

### Added

- Port BlendMode::LinearDodge to the real GPU compositing path

## [0.104.1] - 2026-09-04

### Fixed

- Correct 0.104.0's fixture-selection prose and tighten two lists

## [0.104.0] - 2026-09-04

### Added

- Port BlendMode::Difference to the real GPU compositing path

## [0.103.1] - 2026-09-04

### Added

- Apply 0.103.0's review follow-ups: cfg-gate Dissolve's guard, retract two overclaims

## [0.103.0] - 2026-09-04

### Changed

- Merge the three GPU dispatch-proof counters and retrofit Multiply/Dissolve

## [0.102.1] - 2026-09-04

### Fixed

- Finish 0.102.0's mode-count/doc sweep, which was incomplete

## [0.102.0] - 2026-09-04

### Added

- Port BlendMode::Screen to the GPU compositing path

## [0.101.1] - 2026-09-04

### Fixed

- Correct 0.101.0's over-broad rayon NO-GO scope

## [0.101.0] - 2026-09-04

### Fixed

- Close the composite-fold rayon question NO-GO on measured numbers

## [0.100.1] - 2026-09-04

### Fixed

- Correct 0.100.0's overclaims and tighten its census assertion

## [0.100.0] - 2026-09-04

### Added

- Build a two-root benchmark fixture that reaches fold_onto_opaque

## [0.99.0] - 2026-09-04

### Changed

- Re-measure a full frame end-to-end after 0.98.x

## [0.98.3] - 2026-09-04

### Fixed

- Remove a self-contradicting statement the LinearLight fix left behind

## [0.98.2] - 2026-09-04

### Fixed

- Correct 0.98.1's own blanket claim: LinearLight is a real regression

## [0.98.1] - 2026-09-04

### Fixed

- Correct 0.98.0's regression claims and PLAN.md reporting accuracy

## [0.98.0] - 2026-09-04

### Changed

- Batch composite_layer_into's f16 <-> f32 conversions

## [0.97.1] - 2026-09-04

### Fixed

- Withdraw 0.97.0's GO: T1 fails in the condition that matters

## [0.97.0] - 2026-09-04

### Changed

- Measure the CPU blend-math loop before parallelizing it

## [0.96.2] - 2026-09-04

### Fixed

- Keep the tile-upload frame path sequential by default; parallel arm stays

## [0.96.1] - 2026-09-04

### Fixed

- Own and bound the tile-upload rayon pool, killing an abort path

## [0.96.0] - 2026-09-04

### Changed

- Parallelize the tile-upload serialize loop with rayon

## [0.95.1] - 2026-09-04

### Fixed

- Restore a dropped Lighten test, fix four doc bugs, untrack report.md

## [0.95.0] - 2026-09-04

### Changed

- Give BlendMode::Lighten a real GPU fast path

## [0.94.1] - 2026-09-04

### Fixed

- Correct 0.94.0's measurement claims and guard its skip

## [0.94.0] - 2026-09-04

### Changed

- Skip the un-premultiply pass for a zero-fold composite tile

## [0.93.1] - 2026-09-04

### Fixed

- Correct the phase-1 sub-cost attribution and its fake self-check

## [0.93.0] - 2026-09-04

### Changed

- Split recomposite phase 1 into five per-branch sub-costs

## [0.92.2] - 2026-09-03

### Fixed

- Fix a digit typo and disclose two profile/platform caveats

## [0.92.1] - 2026-09-03

### Fixed

- Route upload_mip through the vectorized serializer too

## [0.92.0] - 2026-09-03

### Changed

- Vectorize the premultiply-and-serialize f16 conversion

## [0.91.2] - 2026-09-03

### Fixed

- Fix a self-contradicting comment and disclose a real forever-retry cost

## [0.91.1] - 2026-09-03

### Fixed

- Drop a failed page-in's atlas slot, so the owed upload is not lost

## [0.91.0] - 2026-09-03

### Changed

- Keep a tile's dirtiness across an eviction, so an owed upload survives

## [0.90.2] - 2026-09-03

### Fixed

- Track the eviction hole as a live defect, name the guard's structural gap

## [0.90.1] - 2026-09-03

### Fixed

- Gate the eyedropper's composite read on contains_tile

## [0.90.0] - 2026-09-03

### Changed

- Skip a composite tile's dirty mark when it recomposites unchanged

## [0.89.2] - 2026-09-03

### Fixed

- Fix stale citations, a doc asymmetry, and commit the extreme-value proof

## [0.89.1] - 2026-09-03

### Fixed

- Pin the hot-path tile serializer per texel, and correct 0.89.0's claims

## [0.89.0] - 2026-09-03

### Changed

- Batch the tile serializer's four per-channel byte appends into one

## [0.88.2] - 2026-09-03

### Fixed

- Fix three follow-up doc issues the Judge caught in 0.88.1

## [0.88.1] - 2026-09-03

### Fixed

- Correct 0.88.0's reading of its own per-stage numbers

## [0.88.0] - 2026-09-03

### Changed

- Break the two real frame benchmarks down per stage

## [0.87.2] - 2026-09-03

### Fixed

- Fix three prose-accuracy nits the Judge caught in 0.87.1's docs

## [0.87.1] - 2026-09-03

### Fixed

- Guard clip_color's divisions against an achromatic NaN

## [0.87.0] - 2026-09-03

### Changed

- Skip a never-painted layer's composite work for a blank tile

## [0.86.2] - 2026-09-03

### Fixed

- Fix a third contradictory doc site and a stale test cross-reference

## [0.86.1] - 2026-09-03

### Fixed

- Correct 0.86.0's cost claims and drop three unfireable guards

## [0.86.0] - 2026-09-03

### Changed

- Batch one composite tile's GPU work into a single submit

## [0.85.2] - 2026-09-03

### Fixed

- Reattach begin_gpu_composite_tile's orphaned doc comment

## [0.85.1] - 2026-09-03

### Fixed

- Correct the Darken round's claims, make its dispatch observable

## [0.85.0] - 2026-09-03

### Added

- Port BlendMode::Darken to the GPU composite path

## [0.84.2] - 2026-09-03

### Added

- Apply the Judge's follow-ups from the GPU Multiply wiring review

## [0.84.1] - 2026-09-03

### Fixed

- Make the GPU composite accumulator pair lazy, and correct 0.84.0's scope

## [0.84.0] - 2026-09-03

### Changed

- Composite a Multiply-blend document on the real GPU path

## [0.83.2] - 2026-09-03

### Added

- Apply the Judge's follow-ups from the GPU Multiply blend review

## [0.83.1] - 2026-09-03

### Fixed

- Harden the GPU Multiply slice against spatial, binding and chaining bugs

## [0.83.0] - 2026-09-03

### Changed

- Prove GPU blend math by sampling a former render target, with Multiply

## [0.82.2] - 2026-09-03

### Fixed

- Defer the elided-tile clear until a .aur read fully succeeds

## [0.82.1] - 2026-09-03

### Fixed

- Clear a .aur file's elided-blank tiles instead of skipping them

## [0.82.0] - 2026-09-03

### Changed

- Free the previous document's tiles before writing the new one's

## [0.81.2] - 2026-09-03

### Added

- Apply the Judge's follow-ups from the mask-coverage-clear review round

## [0.81.1] - 2026-09-03

### Fixed

- Say what the mask-surface undo residual really is

## [0.81.0] - 2026-09-03

### Changed

- Clear a mask's stale coverage when a new one replaces it

## [0.80.1] - 2026-09-03

### Fixed

- Batch the document-discard sweep, and sweep the journal too

## [0.80.0] - 2026-09-03

### Changed

- Free a discarded document's tile-store surfaces

## [0.79.2] - 2026-09-03

### Added

- Apply the Judge's follow-ups from the dialog-paint review round

## [0.79.1] - 2026-09-03

### Fixed

- Close the dialog-paint review round's Light-theme visibility and radius-coverage gaps

## [0.79.0] - 2026-09-03

### Changed

- Give a modal dialog a real painted surface

## [0.78.0] - 2026-09-03

### Changed

- Give the real window a minimum size, closing the dialog round's last gap

## [0.77.7] - 2026-09-03

### Fixed

- Close the dialog-overlay review round's test-vacuity and small-window gaps

## [0.77.6] - 2026-09-03

### Fixed

- Lay a modal dialog out as a real centered overlay

## [0.77.5] - 2026-09-02

### Fixed

- Close the Properties-panel review round's width-floor and label-sanitization gaps

## [0.77.4] - 2026-09-02

### Fixed

- Give the Properties panel's rows a real, hittable size

## [0.77.3] - 2026-09-02

### Fixed

- Close the History-panel review round's doc-accuracy and paint-overflow gaps

## [0.77.2] - 2026-09-02

### Fixed

- Give History panel rows a real, hittable size

## [0.77.1] - 2026-09-02

### Fixed

- Close the Layers-panel review round's unbounded-height and description gaps

## [0.77.0] - 2026-09-02

### Changed

- Give the Layers panel real TreeView rows instead of a hand-built list

## [0.76.1] - 2026-09-02

### Fixed

- Close the TreeView review round's focus-panic and silent-deletion gaps

## [0.76.0] - 2026-09-02

### Added

- Add a TreeView widget with real hierarchical accessibility

## [0.75.1] - 2026-09-02

### Fixed

- Close the Scrollbar review round's NaN/panic/layout/gallery gaps

## [0.75.0] - 2026-09-02

### Added

- Add a Scrollbar widget to aurora-widgets

## [0.74.1] - 2026-09-02

### Fixed

- Keep a .aur file's record of what it lost, honestly and safely

## [0.74.0] - 2026-09-02

### Changed

- Persist a best-effort write's dropped tiles, and warn on reopen

## [0.73.4] - 2026-09-02

### Fixed

- Correct four stale accounts of the narrowed-invalidation round

## [0.73.3] - 2026-09-02

### Fixed

- Make the narrowed invalidation one cache pass, and unmissable

## [0.73.2] - 2026-09-02

### Fixed

- Force a full recomposite when an undo flips the compositing path

## [0.73.1] - 2026-09-02

### Fixed

- Stop narrowing a structural undo/redo on the declared-bounds rect

## [0.73.0] - 2026-09-02

### Changed

- Narrow undo/redo composite invalidation to the region actually dirtied

## [0.72.0] - 2026-09-02

### Changed

- Skip the composite bump when a layer switch does not move the grid anchor

## [0.71.4] - 2026-09-02

### Fixed

- Test the cross-crate mask frame, and correct three stale claims

## [0.71.3] - 2026-09-02

### Fixed

- Bound a mask's extent where the mask is created

## [0.71.2] - 2026-09-02

### Fixed

- Roll a failed .aur read's own committed tiles back out of the store

## [0.71.1] - 2026-09-02

### Fixed

- Give the .aur writer the reader's own whole-document pre-flight

## [0.71.0] - 2026-09-02

### Changed

- Persist per-pixel mask coverage through a .aur round trip

## [0.70.4] - 2026-09-02

### Fixed

- Correct the doc comments 0.70.0 left saying masks are rectangles

## [0.70.3] - 2026-09-02

### Fixed

- Stop materializing never-painted mask tiles on every composite

## [0.70.2] - 2026-09-02

### Fixed

- Fail a mask read open after inversion, not before it

## [0.70.1] - 2026-09-02

### Fixed

- Refuse a layer id that aliases another layer's mask surface

## [0.70.0] - 2026-09-02

### Changed

- Give layer masks real per-pixel grayscale coverage

## [0.69.1] - 2026-09-02

### Fixed

- Route Ctrl+Z/Ctrl+Shift+Z through perform_undo_redo's full order

## [0.69.0] - 2026-09-01

### Changed

- Stream begin_gpu_composite_tile's siblings instead of collecting them

## [0.68.8] - 2026-09-01

### Fixed

- Re-take the scratch lock on TileStore's own recreate path too

## [0.68.7] - 2026-09-01

### Fixed

- Stop sync eating a budget-skipped tile's dirty flag

## [0.68.6] - 2026-09-01

### Fixed

- Re-take the scratch liveness lock when the directory is recreated

## [0.68.5] - 2026-09-01

### Fixed

- Make the non-Unix scratch lock infallible, and count what the sweep skips

## [0.68.4] - 2026-09-01

### Fixed

- Box-filter mips in the premultiplied domain, not the straight one

## [0.68.3] - 2026-09-01

### Fixed

- Fold the atlas premultiply into the byte serialization in sync

## [0.68.2] - 2026-09-01

### Fixed

- Stop marking a dialog refusal reported when it never opened

## [0.68.1] - 2026-09-01

### Fixed

- Publish the scratch lock file already locked, closing a TOCTOU race

## [0.68.0] - 2026-09-01

### Changed

- Premultiply the atlas at upload, not in the shader

## [0.67.0] - 2026-09-01

### Changed

- Sweep orphaned scratch directories at startup, on Unix

## [0.66.0] - 2026-09-01

### Changed

- Surface the Move tool's boundary refusal to the user

## [0.65.0] - 2026-09-01

### Changed

- Surface export refusal for unreadable tiles to the user

## [0.64.2] - 2026-09-01

### Fixed

- Bound the name sanitizer's input, and pin both journal bounds

## [0.64.1] - 2026-09-01

### Fixed

- Keep a failed startup from deleting the previous run's recovery data

## [0.64.0] - 2026-09-01

### Changed

- Bound describe()'s search for a restored subtree's root name

## [0.63.0] - 2026-09-01

### Changed

- Run the clean-shutdown cleanup on every exit path, not just CloseRequested

## [0.62.0] - 2026-08-31

### Added

- Add a manual workflow to probe macos-latest for a real GPU adapter

## [0.61.0] - 2026-08-31

### Fixed

- Harden scratch-path adoption against symlink races

## [0.60.0] - 2026-08-30

### Changed

- Let a runner require a real GPU adapter, not just any adapter

## [0.59.0] - 2026-08-30

### Fixed

- Stop cloning an evicted tile's encoded bytes on the caller's thread

## [0.58.0] - 2026-08-30

### Added

- Create scratch tile files owner-only, not at the umask default

## [0.57.15] - 2026-08-30

### Fixed

- Bound and sanitize a file-supplied layer name before it reaches a label

## [0.57.14] - 2026-08-30

### Fixed

- Hold LayerTree::validate() to its own live-edit origin bar

## [0.57.13] - 2026-08-29

### Fixed

- Validate a layer's document position, closing an integer-overflow gap

## [0.57.11] - 2026-08-26

### Fixed

- Bound canvas panning at the document's far edge too

## [0.57.9] - 2026-08-26

### Fixed

- Fix Windows CI failure: scratch-dir non-directory check was Unix-only

## [0.57.8] - 2026-08-26

### Fixed

- Fix pan boundary violations across layer changes, drags, and zoom

## [0.57.4] - 2026-08-25

### Fixed

- Fix canvas checkerboard at low zoom, and the geometry bugs the fix exposed

## [0.57.0] - 2026-08-25

### Fixed

- Fix brush stroke dead zones and phantom/lost undo entries around broken tiles

## [0.54.0] - 2026-08-25

### Changed

- Give every background write its own generation so a stale one can't clear a newer one

## [0.53.0] - 2026-08-25

### Changed

- Give every session its own private, collision-proof tile scratch directory

## [0.52.2] - 2026-08-25

### Fixed

- A tile whose page-in fails stays an error on every read

## [0.52.1] - 2026-08-24

### Fixed

- Reject a scratch-disk tile that doesn't decode to exactly one whole tile

## [0.52.0] - 2026-08-24

### Fixed

- Fix compositing returning premultiplied alpha where straight alpha belongs

## [0.51.0] - 2026-08-24

### Changed

- Composite groups by folding one layer at a time, not collecting them all

## [0.50.0] - 2026-08-24

### Changed

- Refuse a live edit that would nest a layer tree past its own depth bound

## [0.49.2] - 2026-08-24

### Fixed

- Give resolve_tile its own recursion bound, independent of aurora-doc

## [0.49.1] - 2026-08-24

### Fixed

- Make LayerTree's shape validator reject inconsistency, not just cycles

## [0.49.0] - 2026-08-24

### Changed

- Recover real painted pixels on crash, not just structure

## [0.48.1] - 2026-08-24

### Fixed

- Fix design/build_tokens_css.py crashing on scalar tokens

## [0.48.0] - 2026-08-13

### Changed

- Eyedropper samples the composited document, not just the active layer

## [0.47.1] - 2026-08-13

### Fixed

- Clamp canvas pan to the document's own top-left edge

## [0.47.0] - 2026-08-13

### Added

- Implement real sub-tile fractional canvas scroll

## [0.46.1] - 2026-08-13

### Fixed

- Fix canvas rendering at scale_factor != 1.0: paint no longer offset from the cursor

## [0.46.0] - 2026-08-13

### Changed

- Invalidate only the composite tiles a dab actually touched

## [0.45.0] - 2026-08-13

### Changed

- Populate the Properties (tool-options) panel, closing M1.8's last panel gap

## [0.44.0] - 2026-08-12

### Changed

- TileResidency: handle window resize, rebuilding the atlas at the new size

## [0.43.0] - 2026-08-12

### Changed

- PNG export: write the lighter sRGB chunk instead of iCCP for sRGB images

## [0.42.0] - 2026-08-12

### Changed

- Scale aurora-vector tessellation tolerance by DPI scale factor

## [0.41.0] - 2026-08-12

### Changed

- Re-trigger autosave on live document edits, off the UI thread

## [0.40.1] - 2026-08-12

### Fixed

- Fix broken CI: bad rustdoc link and CI-unsafe benchmark budgets

## [0.40.0] - 2026-08-12

### Changed

- Batch GPU tile-composite readback into one poll per frame

## [0.39.1] - 2026-08-12

### Fixed

- Fix TileStore eviction/revisit race with an in-memory pending map

## [0.39.0] - 2026-08-12

### Changed

- Measure real end-to-end frame cost at the 300,000px ceiling

## [0.38.0] - 2026-08-12

### Fixed

- Fix non-Normal blend math inside a translucent group isolation pass

## [0.37.0] - 2026-08-12

### Changed

- Aggregate LayerMask into real compositing, a rectangular clip

## [0.36.0] - 2026-08-12

### Added

- Add Dissolve, the 27th and final blend mode

## [0.35.0] - 2026-08-11

### Added

- Add GPU-accelerated multi-layer compositing, Normal-blend fast path

## [0.34.0] - 2026-08-11

### Changed

- Aggregate group opacity and blend mode into isolated compositing

## [0.33.0] - 2026-08-11

### Fixed

- Fix layer-groups render-order recursion

## [0.32.1] - 2026-08-11

### Fixed

- Fix RUSTSEC-2026-0253: bump lru to 0.18.2

## [0.32.0] - 2026-08-11

### Added

- Wire DarkerColor and LighterColor blend modes

## [0.31.0] - 2026-08-11

### Added

- Wire the non-separable HSL blend modes

## [0.30.0] - 2026-08-11

### Added

- Wire the overlay/light blend mode family

## [0.29.0] - 2026-08-11

### Changed

- Un-ignore all 24 newly-blessed gallery goldens

## [0.28.0] - 2026-08-11

### Added

- Wire the dodge/burn blend modes

## [0.27.0] - 2026-08-11

### Added

- Wire real blend-mode compositing, first 8 modes

## [0.26.0] - 2026-08-11

### Fixed

- Fix flat-format export to composite every layer, not just the active one

## [0.25.0] - 2026-08-11

### Added

- Wire the density axis into Scales

## [0.24.0] - 2026-08-11

### Added

- Add Colour-Critical gallery coverage, all six widgets

## [0.23.0] - 2026-08-11

### Added

- Add High Contrast Light gallery coverage, all six widgets

## [0.22.0] - 2026-08-11

### Added

- Add High Contrast Dark gallery coverage, all six widgets

## [0.21.0] - 2026-08-10

### Added

- Land the Colour-Critical theme

## [0.20.0] - 2026-08-10

### Added

- Land the High Contrast Light theme

## [0.19.0] - 2026-08-10

### Added

- Land the High Contrast Dark theme

## [0.18.0] - 2026-08-10

### Added

- Add mandatory control-outline border infrastructure

## [0.17.0] - 2026-08-10

### Added

- Add Light-theme ColorSwatch gallery coverage

## [0.16.0] - 2026-08-10

### Added

- Add Light-theme CommandPalette gallery coverage

## [0.15.0] - 2026-08-09

### Added

- Add Light-theme TextField gallery coverage

## [0.14.0] - 2026-08-09

### Added

- Add Light-theme Slider gallery coverage

## [0.13.0] - 2026-08-09

### Added

- Add Light-theme Checkbox gallery coverage

## [0.12.0] - 2026-08-09

### Added

- Add Light-theme Button gallery coverage

## [0.11.0] - 2026-08-09

### Added

- Land the Light built-in theme

## [0.10.2] - 2026-08-09

### Fixed

- Raise panel/raised/overlay contrast against the rail

## [0.10.1] - 2026-08-09

### Fixed

- Relayout after native menu panel toggles

## [0.10.0] - 2026-08-09

### Added

- Add a real border to panels -- the fill alone was nearly invisible

## [0.9.0] - 2026-08-09

### Changed

- Give docked panels a real painted background

## [0.8.0] - 2026-08-09

### Changed

- Persist dock layout (rail width, panel collapsed state) across sessions

## [0.7.1] - 2026-08-08

### Fixed

- Fix command palette rendering nothing in the real app (Ctrl+Shift+P)

## [0.7.0] - 2026-08-08

### Added

- Add dock-rail resize (canvas<->rail divider drag)

## [0.6.0] - 2026-08-08

### Added

- Add panel close, scoped to reuse collapse's mechanism (Option 3)

## [0.5.0] - 2026-08-08

### Added

- Add panel toggle entries to the native macOS menu bar

## [0.4.0] - 2026-08-08

### Added

- Add panel collapse/expand, wired into the command palette

## [0.3.0] - 2026-08-08

### Changed

- Bless color_swatch_gallery.png and command_palette_gallery.png

## [0.2.2] - 2026-08-08

### Fixed

- Fix clippy::doc_markdown on macOS-only accessibility doc comment

## [0.2.1] - 2026-08-08

### Fixed

- Fix objc2-app-kit feature gate for NSWorkspace accessibility methods

## [0.2.0] - 2026-08-08

### Changed

- Detect OS accessibility preferences (macOS), apply to Scales

## [0.1.0] - 2026-08-08

### Changed

- Highlight the selected CommandPalette result row

## [0.0.1] - 2026-08-07

### Added

- Add software versioning and a tag-triggered release workflow

## [0.0.0] - 2026-07-25

### Added

- Add ADRs, Cargo workspace skeleton, and CI
