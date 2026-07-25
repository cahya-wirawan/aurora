# Product Requirements Document (PRD)

**Project:** Aurora  
**Version:** 1.2  
**Document Owner:** Product Team  
**Status:** Draft — pre-implementation  
**Target Release:** TBD  
**Implementation Language:** Rust (see §8)  
**UI:** Custom GPU-rendered UI on `wgpu` — **decided** (see §8.3)

---

# 1. Executive Summary

Aurora is a modern professional image editing application inspired by Adobe Photoshop. The goal is to provide a cross-platform, GPU-accelerated, AI-first, non-destructive image editor capable of replacing Photoshop for approximately 90–95% of professional workflows.

Aurora is implemented in **Rust** end to end — core engines, UI, and first-party plugins. Rust is chosen for memory safety without a GC (a hard requirement given the 10 ms brush-latency budget), fearless concurrency across the tile pipeline, and a mature GPU ecosystem (`wgpu`) that abstracts Vulkan/Metal/DirectX 12 behind one API. Where a mature Rust crate does not yet exist (RAW decoding, ICC color management), Aurora wraps the established C library behind a safe Rust interface rather than reimplementing it.

Aurora will support:

- Professional photo editing
- Digital painting
- Graphic design
- UI/UX design
- Marketing materials
- Scientific imaging
- Illustration
- Print production
- AI-assisted editing
- Plugin ecosystem

---

# 2. Vision

Create the most modern image editing software by combining:

- Photoshop's power
- Figma's usability
- Blender's performance
- AI-assisted workflows
- Non-destructive editing
- Open plugin ecosystem
- Local-first architecture
- Cloud collaboration

---

# 3. Goals

## Primary Goals

- Replace Photoshop for professional users
- Modern GPU architecture
- Cross-platform
- Fast startup
- Responsive editing
- Unlimited undo
- PSD compatibility
- AI-assisted workflows
- Modern, elegant UI that users can restyle through themes without code (FR-027)

## Supported Platforms

### Desktop

- Windows
- macOS
- Linux

### Future

- iPad
- Android Tablets
- Web Application

## Non-Goals (v1.0)

Explicitly out of scope for the first shipping release. Listing these prevents scope drift during Phases 1–3:

- Video editing and motion graphics beyond frame animation (FR-020)
- 3D modelling, texturing, or procedural materials
- Node-based compositing UI (the render graph is internal; no user-facing node editor)
- Full Photoshop plugin binary compatibility (.8bf) — Aurora ships its own SDK
- Real-time multiplayer editing (Phase 5; v1.0 ships cloud documents + comments only)
- DAM / asset management beyond the local asset libraries in FR-023

## Requirement Priorities

Every FR in §5 carries a priority. Where unmarked, assume **Should**.

| Priority | Meaning |
|---|---|
| **Must** | v1.0 does not ship without it |
| **Should** | Targeted for v1.0; may slip one release |
| **Could** | Opportunistic; cut first under schedule pressure |
| **Won't (yet)** | Deferred past v1.0 by decision, not by oversight |

Must: FR-001, FR-002, FR-003, FR-004, FR-005, FR-007, FR-010, FR-012, FR-021, FR-024, FR-025, FR-026, FR-027.
Should: FR-006, FR-008, FR-009, FR-011, FR-013, FR-014, FR-016, FR-019, FR-023.
Could: FR-015, FR-017, FR-018.
Won't (yet): FR-020, FR-022.

---

# 4. User Personas

## Photographer

Needs

- RAW editing
- Batch processing
- Color correction
- Retouching

---

## Graphic Designer

Needs

- Layers
- Typography
- Export
- Print support

---

## Digital Artist

Needs

- Brushes
- Painting engine
- Tablet support
- Perspective tools

---

## UI Designer

Needs

- Artboards
- Vector graphics
- Export assets

---

## Marketing Team

Needs

- Templates
- Social media exports
- AI image generation

---

# 5. Functional Requirements

---

# FR-001 Document Management

## Features

- Create document
- Open document
- Save
- Save As
- Auto Save
- Recovery
- Version History
- Recent Files
- Templates
- Cloud Documents

## Supported Formats

Native

- Aurora (.aur)

Import

- PSD
- PSB
- PNG
- JPG
- JPEG
- TIFF
- BMP
- GIF
- WebP
- AVIF
- HEIF
- SVG
- PDF
- EXR
- HDR
- RAW
- ICO
- DDS

Export

- PNG
- JPG
- TIFF
- PDF
- SVG
- WebP
- AVIF
- PSD

---

# FR-002 Canvas

Features

- Infinite zoom
- Canvas rotation
- Flip
- Mirror
- Multiple tabs
- Multiple windows
- Guides
- Smart Guides
- Grid
- Snap
- Rulers
- Reference Images

---

# FR-003 Layer System

Layer Types

- Pixel
- Text
- Shape
- Smart Object
- Adjustment
- Fill
- Gradient
- Pattern
- Group
- Video
- Frame

Layer Features

- Unlimited layers
- Groups
- Nested groups
- Search
- Color labels
- Opacity
- Fill opacity
- Blend modes
- Layer styles
- Masks
- Clipping masks
- Locking
- Visibility
- Layer Comps

---

# FR-004 Selection Engine

Selection Tools

- Rectangle
- Ellipse
- Single Row
- Single Column
- Lasso
- Polygon Lasso
- Magnetic Lasso
- Magic Wand
- Quick Selection
- Object Selection

Selection Commands

- Select Subject
- Select Sky
- Hair Selection
- Color Range
- Focus Area
- Feather
- Expand
- Contract
- Border
- Inverse
- Save Selection
- Load Selection
- Refine Edge
- Select and Mask

---

# FR-005 Brush Engine

Brush Types

- Brush
- Pencil
- Airbrush
- Mixer Brush
- Ink
- Watercolor
- Oil
- Chalk
- Pastel
- Pixel Brush
- Pattern Brush
- Clone Brush
- Healing Brush

Brush Settings

- Size
- Hardness
- Flow
- Opacity
- Rotation
- Pressure
- Tilt
- Scatter
- Dynamics
- Wetness
- Texture
- Dual Brush
- Stabilization

Tablet Support

- Wacom
- Apple Pencil
- Surface Pen

---

# FR-006 Painting Engine

Features

- GPU painting
- Symmetry
- Perspective painting
- Real-time preview
- Brush smoothing
- Smudge
- Mixer
- Blend
- Clone
- Healing

---

# FR-007 Masks

Supported Masks

- Layer Mask
- Vector Mask
- Clipping Mask
- Gradient Mask
- Color Mask
- Luminosity Mask
- Quick Mask

Operations

- Paint
- Feather
- Density
- Invert
- Disable
- Link/Unlink

---

# FR-008 Vector Graphics

Tools

- Pen
- Curvature Pen
- Rectangle
- Ellipse
- Polygon
- Triangle
- Star
- Line
- Custom Shapes

Features

- Boolean Operations
- Path Editing
- Anchor Editing
- Variable Stroke
- SVG Support

---

# FR-009 Typography

Features

- Character Panel
- Paragraph Panel
- OpenType
- Variable Fonts
- Glyph Browser
- Warp Text
- Vertical Text
- RTL Support
- Text on Path
- Area Text
- Emoji Support

---

# FR-010 Image Adjustments

Adjustment Layers

- Brightness
- Contrast
- Levels
- Curves
- Exposure
- Hue
- Saturation
- Vibrance
- Color Balance
- Black & White
- Selective Color
- Gradient Map
- Posterize
- Threshold
- LUT Support

---

# FR-011 Filters

Blur

- Gaussian
- Motion
- Lens
- Surface
- Tilt Shift

Sharpen

- Smart Sharpen
- Unsharp Mask

Noise

- Add Noise
- Reduce Noise

Distort

- Liquify
- Ripple
- Wave
- Pinch
- Twirl

Render

- Clouds
- Lens Flare
- Lighting

Stylize

- Oil Paint
- Emboss
- Find Edges

Camera RAW Filter

Plugin Filters

---

# FR-012 Transformations

- Move
- Scale
- Rotate
- Skew
- Distort
- Warp
- Perspective Warp
- Puppet Warp
- Mesh Warp
- Free Transform

---

# FR-013 Smart Objects

Features

- Embedded
- Linked
- Replace Content
- Nested Objects
- Non-destructive Editing
- Smart Filters

---

# FR-014 Retouching

Tools

- Spot Healing
- Healing Brush
- Patch Tool
- Remove Tool
- Clone Stamp
- Dodge
- Burn
- Sponge
- Red Eye Removal
- Dust Removal

---

# FR-015 Camera RAW

Features

- RAW Import
- Exposure
- White Balance
- Lens Correction
- Sharpening
- Noise Reduction
- Color Grading
- Local Masks
- Batch Editing

---

# FR-016 Color Management

Modes

- RGB
- CMYK
- Lab
- XYZ
- Grayscale
- HDR

Features

- ICC Profiles
- Soft Proof
- Color Picker
- Swatches
- Pantone Support

---

# FR-017 AI Features

Image Editing

- Background Removal
- Object Removal
- Sky Replacement
- Generative Fill
- Generative Expand
- Smart Crop
- Image Upscaling
- Colorization
- Restoration

Generation

- Text-to-Image
- Image-to-Image
- Prompt Editing
- Style Transfer

Support

- Local AI Models
- Cloud AI Models

---

# FR-018 Automation

Features

- Actions
- Batch Processing
- Macro Recording
- CLI (headless `aurora-cli` binary)
- Lua API (embedded, in-process)
- Python API (out-of-process over IPC)
- JavaScript API — *deferred past v1.0*

---

# FR-019 Plugin SDK

Plugin Types

- Filters
- Panels
- Brushes
- Importers
- Exporters
- AI Extensions

Languages

Plugins compile to **WASM** and run sandboxed (§8.4). Any language with a WASM target is supported; Rust and C/C++ are first-class. Native unsandboxed plugins are permitted only behind an explicit user trust prompt.

Marketplace

- Install
- Update
- Rating
- Reviews

---

# FR-020 Animation

Features

- Timeline
- Frame Animation
- GIF Export
- MP4 Export
- Keyframes

---

# FR-021 Export

Formats

- PNG
- JPG
- TIFF
- PSD
- PDF
- SVG
- WebP
- AVIF
- GIF

Features

- Batch Export
- Presets
- Compression
- Metadata
- Transparency

---

# FR-022 Collaboration

Features

- Cloud Documents
- Comments
- Live Collaboration
- Version History
- Shared Assets

---

# FR-023 Asset Libraries

Libraries

- Brushes
- Patterns
- Gradients
- Shapes
- Fonts
- Templates
- AI Prompts

---

# FR-024 User Interface

Rendered by Aurora's own `wgpu` widget toolkit (§8.3). Because no platform toolkit is involved, the platform-integration items below are **first-party engineering work in Phase 1**, not inherited behaviour.

Features

- Dockable Panels
- Custom Workspaces
- Dark Theme
- Light Theme
- Command Palette
- Search
- Contextual Toolbar
- Keyboard Shortcuts

Platform Integration (custom-UI obligations)

- Screen reader support — Windows UIA, macOS NSAccessibility, Linux AT-SPI (via `accesskit`)
- Keyboard navigation and focus rings across all panels
- IME composition for CJK input; dead keys; RTL text editing
- Text field editing: selection, caret, undo, clipboard, word-wise motion
- Native menu bar (macOS), native file dialogs, drag & drop, system clipboard
- Per-monitor DPI and fractional scaling; multi-monitor with mixed DPI
- Respect OS settings: reduced motion, high contrast, text size, cursor size
- Custom cursors matching platform conventions

Visual design and theming are specified separately in **FR-027**.

---


Features

- Unlimited Undo
- Redo
- History Panel
- Snapshots
- Branch History

---

# FR-026 Preferences

Settings

- Performance
- GPU
- Memory
- Scratch Disk
- Shortcuts
- Language
- Theme (see FR-027)
- Plugins
- Autosave
- UI density and scale (see FR-027)

---

# FR-027 Visual Design & Theming

**Priority: Must.** Aurora's interface must be modern, beautiful, and elegant, and must be easy to restyle through themes without touching code. Since Aurora renders its own UI (§8.3), it controls every pixel — the same reason it inherits no design for free. Visual quality is therefore a requirement with acceptance criteria, not a matter of taste applied at the end.

## Design Principles

These are the standard a design review judges against:

- **The work is the interface.** Chrome recedes; the image is the brightest, most saturated thing on screen. UI surfaces are low-chroma and near-neutral so they never compete with or bias perception of the user's colours.
- **Calm by default, rich on demand.** Progressive disclosure: a clean default workspace, with depth available for those who want it. Complexity is opt-in.
- **Elegance is restraint.** A small, consistently applied set of type sizes, spacing steps, radii, and elevations — not decoration. Every visual difference must encode a real difference in meaning.
- **Density is a user choice.** Professional users work long sessions on large displays and legitimately disagree about density. Compact / Comfortable / Spacious are supported modes, not one designer's preference.
- **Motion clarifies, never delays.** Transitions explain state changes (panel docking, tool switching). Nothing animates longer than 200 ms; nothing blocks input. All motion is disabled under OS reduced-motion.
- **Beauty never overrides legibility or accessibility.** Where they conflict, accessibility wins — see the contrast floors below.

## Design System

A single source of truth, consumed by every widget. No widget may hardcode a colour, size, or spacing value.

- **Design tokens** — semantic, not literal (`surface.panel`, `text.secondary`, `accent.primary`, `border.focus`), so a theme redefines meaning rather than patching individual widgets
- **Type scale** — one variable font family, a fixed modular scale, defined weights and line heights
- **Spacing scale** — a single base unit with fixed multiples; arbitrary pixel values are a review failure
- **Elevation, radius, and border sets** — small, fixed vocabularies
- **Iconography** — one geometrically consistent vector icon set, resolution-independent and themeable
- **Motion tokens** — standard durations and easing curves
- **Component library** — every widget documented in a live, browsable gallery (see Acceptance below)

## Theming

- **Built-in themes:** Dark (default), Light, High Contrast Dark, High Contrast Light, and a neutral-grey **Color-Critical** theme for colour-accurate work
- **Follow OS theme** and OS accent colour, including live switching without restart
- **User-editable themes** — declarative theme files (TOML), hand-editable, no compilation, no code
- **Live reload** — editing a theme file updates the running application; no restart
- **Theme inheritance** — a theme may extend a built-in and override only what it changes, so a small theme stays small
- **In-app theme editor** — visual token editing with immediate preview, exporting a theme file
- **Distribution** — themes are shareable files, installable from disk or the marketplace (FR-019); themes are declarative data and are **not** executable code
- **Independent axes** — theme, density mode, UI scale, accent colour, and icon set can be set independently
- **Canvas-adjacent neutrality** — the surround colour behind the canvas is user-settable independently of the theme (a hard requirement for colour judgement)
- **Per-workspace themes** — a workspace (FR-024) may pin a theme, so e.g. a retouching workspace can force the colour-critical theme
- **Versioned schema** — theme files declare a schema version; Aurora migrates or warns rather than breaking on upgrade
- **Fallback** — missing tokens inherit from the base theme; a malformed theme degrades to the default with a clear diagnostic, never a broken or unusable UI

## Acceptance Criteria

Testable, so "beautiful" does not become unfalsifiable:

1. Every built-in theme meets **WCAG 2.1 AA contrast** — 4.5:1 body text, 3:1 large text and UI boundaries — verified by an automated CI check over the token set, not by eye.
2. Focus indicators are visible in every theme at 3:1 against adjacent colours.
3. **Zero hardcoded style values** in widget code; CI lints for literal colours and pixel sizes outside the token definitions.
4. A user can produce a complete custom theme by editing a text file only, using published documentation — no source access, no build step.
5. Theme switching and live reload complete in under 100 ms with no visual artifacts.
6. A **component gallery** builds from source, showing every widget in every state (default, hover, active, focused, disabled, error) across all built-in themes and density modes; it is the review surface and the golden-image test target (§8.5).
7. All three density modes remain fully usable — no clipped labels, no overlapping elements — at UI scales from 100% to 300%.
8. Colour-critical theme surfaces are verified neutral (chroma below an agreed threshold in a perceptual colour space).

---

# 6. Non-Functional Requirements

## Performance

- Startup < 3 seconds
- Brush latency < 10 ms
- 60 FPS canvas interaction
- GPU acceleration
- Multi-threading
- Progressive rendering

---

## Scalability

Support

- 500,000 × 500,000 pixel documents
- Unlimited layers
- Unlimited history
- Large PSD files (>2 GB)

---

## Reliability

- Crash recovery
- Auto save
- Corruption detection
- Recovery mode

---

## Security

- Plugin sandbox
- Secure cloud sync
- Encrypted documents
- Permission system

---

# 7. Technical Architecture

## 7.1 Layer Model

```
Application

├── UI
│   ├── Workspace
│   ├── Panels
│   └── Tools
│
├── Document Engine
│   ├── Layers
│   ├── Canvas
│   ├── History
│   └── Assets
│
├── Render Engine
│   ├── GPU Renderer
│   ├── Tile Manager
│   ├── Image Cache
│   └── Render Graph
│
├── Processing Engine
│   ├── Filters
│   ├── AI
│   ├── Brushes
│   └── Color Management
│
├── Plugin System
│
├── File System
│
└── Cloud Services
```

## 7.2 Cargo Workspace Layout

Aurora is a single Cargo workspace. Dependencies point **downward only**; a lower crate never depends on a higher one. This is enforced in CI, not by convention.

| Crate | Responsibility | May depend on |
|---|---|---|
| `aurora-core` | Geometry, color types, pixel formats, error types, IDs | — |
| `aurora-tile` | Tile store, paging to scratch disk, LRU image cache | core |
| `aurora-graph` | Render graph: node definitions, dirty tracking, scheduling | core, tile |
| `aurora-gpu` | `wgpu` device management, shader library, GPU tile residency | core, tile |
| `aurora-render` | Executes the graph on GPU/CPU, progressive & tiled output | core, tile, graph, gpu |
| `aurora-doc` | Document model, layer tree, masks, selections, history | core, tile, graph |
| `aurora-color` | ICC, working spaces, soft proof, HDR transforms | core |
| `aurora-filters` | Filter and adjustment node implementations | core, gpu, graph, color |
| `aurora-brush` | Brush engine, stroke input, stabilization, dab scheduling | core, tile, gpu |
| `aurora-vector` | Paths, booleans, stroking, tessellation | core |
| `aurora-text` | Shaping, layout, OpenType, text-on-path | core, vector |
| `aurora-io` | Format import/export (PSD, PNG, TIFF, RAW, …) | core, tile, doc, color |
| `aurora-ai` | Inference sessions, model registry, local/cloud dispatch | core, tile |
| `aurora-plugin` | Host ABI, WASM sandbox, capability grants | core, tile, graph |
| `aurora-theme` | Design tokens, theme file parsing & inheritance, hot reload, contrast validation (FR-027) | core, color |
| `aurora-widgets` | Widget toolkit: layout, input routing, damage tracking, accessibility nodes, text fields (§8.3) | core, gpu, vector, text, theme |
| `aurora-ui` | Panels, docking, workspace, tools, command palette | all above |
| `aurora-app` | Binary: window/event loop, wiring, crash recovery | all above |
| `aurora-cli` | Headless batch/automation binary | all except widgets, ui |

`aurora-widgets` is a general-purpose toolkit with no knowledge of documents or layers — it must be usable and testable headlessly, without a document open. Aurora-specific panels live in `aurora-ui`. Keeping this seam sharp is what makes the §8.3 escape hatch viable and the UI unit-testable.

## 7.3 Architectural Invariants

These are load-bearing. Violating any one of them invalidates a headline requirement, so they are stated as rules rather than preferences:

1. **Nothing assumes a document fits in memory.** All pixel access goes through `aurora-tile`. A 500,000 × 500,000 px document at 8-bit RGBA is ~1 PB; only the visible working set is resident. Tiles page to the scratch disk under memory pressure.
2. **Edits are non-destructive by default.** Adjustments, filters, and smart objects are *nodes in the render graph*, never baked pixels. Baking happens only at export or on explicit user rasterization.
3. **History stores operations, not snapshots.** "Unlimited history" is only affordable if undo records a reversible operation plus the tiles it dirtied. Full-document snapshots are reserved for explicit user Snapshots (FR-025).
4. **The UI thread never blocks on rendering.** Rendering is asynchronous and progressive: the canvas always presents the best currently-available result and refines it. This is what makes 60 FPS achievable independently of scene complexity.
5. **Brush input bypasses the general graph.** The 10 ms latency budget cannot survive a full graph re-evaluation. The active stroke renders into a dedicated scratch layer composited on top, and merges into the graph on stroke end.
6. **Color is explicit.** Every buffer carries its color space. There is no "default RGB" — untagged data is an error, not a fallback.
7. **Plugins are untrusted.** They run sandboxed with explicitly granted capabilities and cannot hold raw pointers into document memory.
8. **UI and canvas share one GPU device and one frame.** The UI is not a separate surface composited over the canvas; both are drawn by `aurora-gpu` into the same swapchain image (§8.3). No interop layer, no readback, no compositing seam.
9. **Every widget carries an accessibility node.** Accessibility is part of a widget's definition, not a later pass. Since Aurora renders its own UI, nothing is accessible for free — a widget without an `accesskit` node is incomplete (§8.3).
10. **No style value is hardcoded.** Widgets resolve every colour, spacing, size, radius, and duration from design tokens (FR-027); CI rejects literal style values in widget code. Retheming must remain a data change. A single hardcoded colour is a bug because it is the one thing a user's theme cannot fix.

---

# 8. Technology Stack

## 8.1 Core Stack

| Layer | Technology | Notes |
|---|---|---|
| Language | **Rust** (edition 2024, stable toolchain) | No GC; safety without runtime cost |
| Build | Cargo workspace | Single workspace, crates per §7.2 |
| GPU abstraction | `wgpu` | One backend-agnostic API |
| Shaders | WGSL | Compiled via `naga`; single shader source |
| macOS backend | Metal (via wgpu) | |
| Windows backend | DirectX 12 / Vulkan (via wgpu) | |
| Linux backend | Vulkan (via wgpu) | |
| CPU parallelism | `rayon` | Tile-parallel CPU fallback paths |
| Async runtime | `tokio` | I/O, cloud sync, background tasks — **not** the render loop |
| SIMD | `std::simd` / `wide` | CPU pixel paths |
| Windowing / input | `winit` | Includes tablet pressure & tilt events |
| Serialization | `serde` + `postcard`/`rkyv` | `.aur` container format |
| Error handling | `thiserror` (libs), `anyhow` (binaries) | |
| Logging / tracing | `tracing` + `tracing-subscriber` | Also feeds the profiler |

## 8.2 Domain Libraries

| Need | Choice | Rationale / risk |
|---|---|---|
| Image codecs (PNG/JPG/TIFF/WebP/GIF/BMP) | `image`, `zune-image` | Mature, pure Rust |
| AVIF / HEIF | `libavif` / `libheif` bindings | No mature pure-Rust encoder yet |
| OpenEXR / HDR | `exr` crate | Pure Rust, actively maintained |
| RAW decoding | `rawler`, or LibRaw via FFI | **Risk:** pure-Rust RAW coverage is thinner than LibRaw; decide in Phase 0 spike |
| Color management | `lcms2` bindings (ICC), `ocio` optional | **Risk:** no mature pure-Rust ICC engine; FFI wrapper required |
| PSD/PSB | Custom crate (`aurora-io`) | Existing crates are read-only and incomplete; write support must be built |
| PDF | `pdf-writer` (export), `pdfium`/`pdf` (import) | Import fidelity is the hard part |
| SVG | `usvg` / `resvg` | |
| Vector rasterization | `lyon` (tessellation) + custom GPU path renderer | |
| Text shaping | `rustybuzz` + `swash` / `cosmic-text` | Covers OpenType, variable fonts, RTL, emoji |
| Font enumeration | `font-kit` / `fontdb` | |
| AI runtime | `ort` (ONNX Runtime bindings), `candle` for pure-Rust paths | Local + cloud dispatch |
| Compression | `zstd`, `lz4_flex` | Tile and history compression |

## 8.3 UI Strategy — Custom UI on `wgpu` (Decided)

**Decision: Aurora builds its own retained-mode widget toolkit rendered through `wgpu`, sharing one GPU device and one frame with the canvas.** No third-party UI toolkit is used. Alternatives considered and rejected: egui (weak accessibility and IME, no real docking), Iced (docking and panel model unsolved), Qt 6 via CXX-Qt (reintroduces C++ and a two-language build).

### Rationale

- **One device, one frame.** UI and canvas share a `wgpu` device, swapchain, and frame. No cross-API interop layer, no texture copies between a widget toolkit's surface and the canvas, no compositing seam. This is the decisive argument: with any external toolkit, the canvas is a foreign surface embedded in someone else's frame.
- **Precedent.** Blender and Figma both took this path for the same reason — a professional creative tool's UI is mostly custom (canvas, docking, timeline, curve editors, color wheels, layer trees) and gains little from stock widgets.
- **Latency control.** The 10 ms brush budget and 60 FPS target require owning the entire input→present path. An external toolkit's event loop and frame pacing sit directly in that path.
- **Consistency and theming.** FR-024 requires custom workspaces, dark/light themes, and a contextual toolbar behaving identically on three platforms. Custom rendering makes this the default rather than a per-platform fight.

### Cost accepted

This is the most expensive option, and the PRD states that plainly. Aurora must build and own: text input and editing, IME (CJK) composition, accessibility, DPI and multi-monitor scaling, native menus, drag & drop, clipboard, file dialogs, and text selection. **These are Phase 1 scope, not polish.** The failure mode for custom-UI applications is treating them as post-v1.0 work; Aurora budgets them explicitly and gates Phase 1 on them (§9).

### Non-negotiable requirements on the toolkit

1. **Accessibility via `accesskit` from the first widget.** Every widget exposes its accessibility node as part of its definition, not retrofitted. Screen readers must work on Windows (UIA), macOS (NSAccessibility), and Linux (AT-SPI). A widget without an accessibility node does not pass review.
2. **Platform text input, not custom keyboard handling.** IME composition, dead keys, and RTL editing route through `winit`'s platform IME support into a shared text-editing core (`cosmic-text`) used by both UI fields and canvas text (FR-009). One text stack, two consumers.
3. **Native platform integration where users expect it.** OS menu bar (macOS), native file dialogs (`rfd`), system clipboard, drag & drop, and cursor conventions. The UI may look custom; it must not *behave* foreign.
4. **Retained-mode with damage tracking.** Immediate-mode redraw of the full UI every frame wastes budget the canvas needs, and hurts battery. Only damaged regions repaint; a still UI costs nothing.
5. **Vector-first rendering.** UI geometry is resolution-independent (via the `aurora-vector` path renderer, shared with FR-008) so fractional DPI and per-monitor scaling are correct by construction.
6. **Automated UI testing.** Since there are no platform widgets to drive, the toolkit exposes a headless mode plus golden-image tests (§8.5) from the start.
7. **Fully tokenized styling.** Every widget resolves colours, spacing, type, radii, and motion from the design-token system (FR-027) at draw time. No widget hardcodes a style value, and no widget reads a *theme* — it reads semantic tokens. This is what makes retheming a data change rather than a code change, and it is only cheap if enforced from the first widget. Retrofitting tokens into an already-written toolkit is a rewrite.

### Implementation stack

| Concern | Choice |
|---|---|
| Rendering | `wgpu` + WGSL, shared device with canvas |
| Vector/path rendering | `aurora-vector` (`lyon` tessellation + GPU renderer) |
| Text shaping & editing | `cosmic-text` (`rustybuzz` + `swash`) |
| Windowing, input, IME | `winit` |
| Accessibility | `accesskit` |
| Native dialogs | `rfd` |
| Layout | Custom flexbox-style engine (`taffy` if it fits) |
| Theme files | TOML via `serde`, hot-reloaded with `notify` |
| Theme colour math | `palette` — perceptual (Oklch) colour handling for contrast checks and token derivation |
| Icons | In-house vector set, rendered via `aurora-vector` |

### Escape hatch

If accessibility or IME quality proves unacceptable at the Phase 1 gate — measured, not assumed — the contained fallback is to keep the custom canvas and host it inside Qt 6 via CXX-Qt for chrome only. `aurora-ui` is therefore kept free of `wgpu`-specific assumptions in its widget *API*, so the renderer can be swapped without rewriting panel logic. This is a documented contingency, not a plan.

## 8.4 Plugin and Scripting Stack

Revised from the original C++/Rust/Python plan. Native dynamic-library plugins cannot satisfy the "plugin sandbox" security requirement (§6), so:

| Layer | Technology |
|---|---|
| Plugin sandbox | **WASM** via `wasmtime` (WASI + custom host ABI) |
| Plugin languages | Any language targeting WASM — Rust and C/C++ first-class |
| Capability model | Explicit grants (filesystem, network, document regions) |
| Native plugins | Permitted but marked *unsandboxed*, with an explicit user trust prompt |
| Scripting | **Lua** (`mlua`) embedded for actions/macros |
| Python API | Out-of-process over IPC — keeps CPython out of the trusted core |
| JavaScript API | Deferred past v1.0 (was FR-018; reduces three script runtimes to one) |

## 8.5 Tooling

`cargo clippy` (warnings denied in CI), `cargo fmt`, `cargo nextest`, `cargo deny` (licenses/advisories), `criterion` for benchmarks, `insta` for snapshot tests, and image-diff golden tests for render correctness.

UI-specific CI gates (FR-027): a custom lint rejecting hardcoded style values in widget crates, an automated WCAG contrast check across every built-in theme's token set, and golden-image tests of the component gallery in every theme and density mode.

---

# 9. Milestones

Each phase has an **exit criterion** — a measurable gate, not a feature checklist. A phase is not complete until its gate passes on all three target platforms.

## Phase 0 — Technical De-risking

*New.* The original plan began at Phase 1 with the stack assumed settled. Several decisions in §8 are unresolved and are expensive to reverse after Phase 1 code exists. Phase 0 resolves them with throwaway spikes.

- `wgpu` performance validation on all three platforms
- Tile store + scratch-disk paging prototype
- **UI toolkit spike** — the toolkit choice is settled (§8.3), but its two riskiest obligations are not: prove `accesskit` drives a real screen reader on all three platforms, and prove `winit` + `cosmic-text` handle CJK IME composition correctly. These are the escape-hatch triggers, so they are tested first
- Widget toolkit foundations: layout engine, damage tracking, input routing, text field
- **Design language and token system** (FR-027) — the token vocabulary and one built-in theme must exist before widgets are written, since tokens cannot be retrofitted cheaply (invariant §7.3.10)
- RAW and ICC library decision (pure Rust vs. FFI)
- PSD format study and read/write feasibility
- Skeleton workspace, CI, and golden-image test harness

**Exit criterion:** a throwaway prototype paints a stroke onto a 100,000 × 100,000 px tiled document at 60 FPS with sub-10 ms input latency on Windows, macOS, and Linux, with custom-rendered docked panels in the same frame; a screen reader reads a panel's controls on each platform; CJK text is composed into a custom text field. Every §8 entry marked "risk" is resolved in a written decision record.

Duration: 3 months

---

## Phase 1

- Document system
- Canvas
- Layers
- Rendering
- Basic tools
- **Widget toolkit and application shell** (§8.3) — docking, panels, text input, accessibility, platform integration
- **Design system, built-in themes, and user theme files** (FR-027)

**Exit criterion:** create, edit, save, reopen, and export a multi-layer document with blend modes and unlimited undo, holding 60 FPS at the Phase 0 document size — *and* the shell passes an accessibility audit (screen reader navigation of all panels) and an IME audit (CJK entry into every text field) on all three platforms — *and* the component gallery renders every widget in every state across all built-in themes with the automated contrast check passing (FR-027).

**Duration: 9 months** (raised from 6). Building the widget toolkit is roughly a third of this phase. The original 6-month estimate assumed an off-the-shelf toolkit; §8.3 makes that work first-party, and the schedule reflects it rather than absorbing it silently.

---

## Phase 2

- Selections
- Brushes
- Masks
- Filters
- Adjustments

**Exit criterion:** a professional illustrator completes a real piece end to end in Aurora without leaving for another tool.

Duration: 8 months

---

## Phase 3

- Smart Objects
- Camera RAW
- Color Management
- PSD Compatibility

**Exit criterion:** round-trip a corpus of 1,000 real-world PSDs with no layer loss and pixel-accurate composites within tolerance.

Duration: 8 months

---

## Phase 4

- AI Features
- Plugin SDK
- Automation
- Cloud Sync

**Exit criterion:** a third-party developer ships a working sandboxed plugin using only public documentation.

Duration: 10 months

---

## Phase 5

- Collaboration
- Animation
- Mobile
- Web

Duration: 12 months

---

# 10. Success Metrics

- Open a 2 GB PSD in under 5 seconds
- Maintain 60 FPS during editing
- Support 95% of common Photoshop workflows
- Crash rate below 0.1%
- Plugin marketplace with 100+ extensions
- User satisfaction (NPS) > 50
- Startup time under 3 seconds
- Export 1 GB documents in under 10 seconds

---

# 11. Risks

| # | Risk | Impact | Mitigation |
|---|---|---|---|
| R1 | Custom UI toolkit is a large build absorbing Phase 1 capacity, competing with engine work | High | Scope owned explicitly: Phase 1 extended to 9 months (§9); `aurora-widgets` kept document-agnostic and independently testable |
| R2 | Accessibility (screen readers) in a custom GPU UI — nothing is free, and it is a legal/procurement requirement in many markets | High | `accesskit` from the first widget (invariant §7.3.9); proven on all three platforms in Phase 0; Phase 1 gate includes an accessibility audit |
| R2b | IME / CJK text input in custom text fields — the classic failure of custom-UI apps, and it blocks entire markets | High | `winit` platform IME + `cosmic-text`, validated in Phase 0; Phase 1 gate includes an IME audit |
| R2c | Custom UI feels foreign — wrong cursors, missing menu conventions, no OS setting respect | Medium | Platform-integration checklist is explicit FR-024 scope; per-platform UX review at the Phase 1 gate |
| R2d | Visual design quality is subjective and can stall decisions or drift late in the project | Medium | FR-027 makes it testable (contrast, gallery, token lint); a single design owner decides; design language settled in Phase 0 before widgets exist |
| R2e | Theming flexibility becomes a compatibility burden — user themes break on upgrade as tokens change | Medium | Versioned theme schema, semantic tokens, inherit-and-override, documented migration; malformed themes degrade rather than fail |
| R2f | No design resource — the plan assumes engineers alone can deliver "beautiful" | High | FR-027 requires a design owner; it is a staffing dependency, not something the token system solves |
| R3 | PSD compatibility is a large reverse-engineering effort with no complete spec | High | Start the format study in Phase 0; build the test corpus before the parser |
| R4 | Pure-Rust RAW/ICC coverage is thinner than LibRaw/LCMS | Medium | FFI wrappers are acceptable; decide in Phase 0 |
| R5 | Scope: 26 FRs is multiple products' worth of work | High | §3 non-goals + MoSCoW priorities; enforce at phase gates |
| R6 | 500,000 × 500,000 px target may be over-specified vs. real demand | Medium | Validate with target users; a lower ceiling materially simplifies the tile store |
| R7 | AI features imply model hosting, licensing, and per-user inference cost | Medium | Local-first; treat cloud AI as a separate business decision |
| R8 | `wgpu` abstraction may block platform-specific optimizations | Medium | Phase 0 benchmarks; escape hatch to native backends per-platform |
| R9 | Team hiring — senior Rust + GPU + imaging is a narrow talent pool | High | Factor into the schedule; the durations in §9 assume a staffed team |

---

# 12. Open Questions

These block or reshape implementation and need owners and answers, most before Phase 1.

*Resolved in v1.2: the UI toolkit question — Aurora builds a custom `wgpu` UI (§8.3).*

1. **Accessibility conformance target** — WCAG 2.1 AA equivalent, or a specific procurement standard (Section 508 / EN 301 549)? This sets the bar the §9 Phase 1 audit measures against, and custom UI means it is earned widget by widget.
2. **Team size and funding** — the §9 durations (now 50 months with Phase 0 and the extended Phase 1) presuppose a staffed team. What is it? Custom UI adds specialist needs: text input, IME, and accessibility are their own discipline, and FR-027 requires a dedicated design owner.
2b. **Who owns visual design?** FR-027 raises "beautiful and elegant" to a Must, but a token system and a contrast check only prevent ugliness — they do not produce beauty. This needs a named designer with final say on the design language, settled in Phase 0. Without one, FR-027 will not be met regardless of the infrastructure.
3. **Business model** — perpetual, subscription, or open source? This determines whether cloud services (FR-022) and the marketplace (FR-019) are viable at all.
4. **Is the 500,000 px document target real?** It drives the entire tile architecture. What is the largest document actual target users open?
5. **Colour precision floor** — is 8-bit-per-channel supported internally, or is the pipeline always ≥16-bit float? Affects every buffer in the system.
6. **PSD scope** — read-only, or full write? Write fidelity is dramatically harder and may not be needed if users export flattened.
7. **`.aur` format** — must it be forward-compatible across versions from v1.0? Decide before the first byte is written.
8. **AI models** — first-party, bundled third-party, or bring-your-own? Licensing and download size follow from this.
9. **Minimum GPU baseline** — what hardware must Aurora run on? Sets the `wgpu` feature floor.
10. **Which Photoshop workflows are the 95%?** The success metric in §10 is currently unmeasurable without this list.

---

# 13. Next Steps Before Implementation

In order. Nothing here is Phase 1 feature code — this is the work that makes Phase 1 safe to start.

## Step 1 — Resolve the blocking decisions

The UI toolkit decision is made (§8.3) and should be written up as `docs/adr/0001-custom-wgpu-ui.md`, capturing the alternatives rejected and the escape-hatch trigger — the reasoning matters more than the verdict when someone revisits it in year two.

Still open and blocking: Open Questions 4, 5, and 6 (§12) — the document-size ceiling, the color precision floor, and PSD write scope. Each becomes its own ADR. They determine the shape of the tile store and the render graph, and all are expensive to reverse once Phase 1 code exists.

## Step 2 — Define the 95%

Turn the §10 success metric into a written list of concrete Photoshop workflows, ranked by frequency among the §4 personas. This list becomes the acceptance suite and the arbiter for every "Could we cut this?" question later.

## Step 3 — Stand up the workspace

Create the Cargo workspace with the crate skeleton from §7.2, and CI enforcing: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo nextest`, `cargo deny`, and the dependency-direction rule. Build on Windows, macOS, and Linux from the first commit — cross-platform problems found in month 30 are catastrophic; found in week 1 they are trivial.

## Step 4 — Build the vertical slice

The single most valuable pre-implementation artifact: one throwaway prototype that exercises the whole stack top to bottom — window → `wgpu` surface → tile store → render graph → single brush stroke → save/reload, with a custom-drawn docked panel and one text field **in the same frame as the canvas**. Narrow but complete. It validates the §7.3 invariants (including 8 and 9) against reality and produces the honest latency and throughput numbers that the §6 budgets are currently only asserting.

Include the two escape-hatch triggers here, not later: a screen reader reading the panel, and CJK composition in the text field. They are the cheapest things to test now and the most expensive to discover in Phase 2.

## Step 4b — Settle the design language

Before widgets are written, not after. Produce the token vocabulary, type and spacing scales, and one complete built-in theme, together with static mockups of the main workspace and two or three panels. Stand up the component gallery as the first UI artifact — it is where the design is reviewed and where golden-image tests attach.

This ordering is the whole point: tokens are cheap to adopt at widget #1 and a rewrite at widget #200 (invariant §7.3.10). It also needs the design owner from Open Question 2b to exist first.

## Step 5 — Prove the risky dependencies

Spikes for R3 and R4: parse a real PSD with layers; decode a RAW file from each major camera vendor; run an ICC transform. Small, fast, and each one can invalidate a §8.2 choice while that is still cheap.

## Step 6 — Assemble the test corpora

Before writing the parsers. Collect the PSD corpus (Phase 3 gate), RAW samples per vendor, and ICC profiles. Stand up the golden-image diff harness so render correctness is regression-tested from the first filter onward.

## Step 7 — Re-plan

With Steps 1–6 done, revisit §9. The durations are currently estimates made without a prototype; after the vertical slice they can be grounded. Expect them to move.

**Do not begin Phase 1 feature work until Steps 1, 3, and 4 are complete.**

---

# 14. Future Roadmap

- Node-based compositing
- 3D editing
- Procedural materials
- Collaborative whiteboarding
- AI agents for editing
- Video compositing
- Motion graphics
- Mobile companion apps
- Browser-based editor
- Distributed rendering
- Cloud GPU processing
- Real-time multiplayer editing