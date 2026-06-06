# Contour — Open Source Illustrator Alternative

Professional vector graphics editor in Rust, and **app #2 of the Prism suite** (sibling to
[Pigment](../pigment), the raster editor). **Goal: reach ≥85% of Adobe Illustrator's real-world
capability** — features, reliability, and ease-of-use — in staged milestones, on the suite's shared
engine (`prism-core` / `prism-color`), with Bézier math on `kurbo`, tessellation on `lyon`, and
boolean ops on `i_overlay`.

> Companion docs: [RESEARCH.md](./RESEARCH.md) (cited findings + crate matrix), [README.md](./README.md) (current build), [../SUITE.md](../SUITE.md) (four-app vision + interop). This PLAN expands the README's v0 scaffold into a parity roadmap; the README still tracks what runs *today*.

---

## 0. Why this can work

- Vector algorithms are solved and free: Bézier math (`kurbo`), tessellation (`lyon`), polygon
  booleans (`i_overlay`), SVG/PDF emission, image trace (`vtracer`).
- The suite already gives us a document/color/geometry foundation (`prism-core`, `prism-color`) and a
  proven egui/eframe app shell shared with Pigment.
- Illustrator's real moat is **interoperation + polish**, not any single algorithm. We get interop for
  free if the document, color, and (eventually) vector-path engine are one shared codebase.

**Non-negotiable principle:** the document is a **resolution-independent path graph**; rasterization
(screen, PNG, live-effects) is always derived and cached, never the source of truth. Color lives in
`prism-color` so a swatch is identical in Contour, Pigment, and Pulse and on export.

---

## 0a. Suite boundaries — what belongs in Contour vs Pigment / Pulse

Contour shares `prism-core` / `prism-color` with the suite. Every feature is filed against three rules
so we never duplicate or overwrite a sibling app's work:

- **Contour-owned (vector authoring):** paths, anchors/handles, shapes, pathfinder/booleans, strokes &
  stroke profiles, gradients (incl. mesh/freeform), patterns, symbols, blends, type-on-path, artboards,
  vector live-effects, SVG/PDF/EPS IO, image trace. Lives in `contour-app` or vector-only shared modules.
- **Shared-crate, app-agnostic:** anything promoted into `prism-core`/`prism-color` (path math, boolean
  ops, stroking/tessellation, gradient/LUT, color transforms, geometry, the document container) **must
  not** assume Contour. Pigment & Pulse already lean on these — additions are additive and never
  vector-UI-coupled. A new suite-level **`prism-vector`** crate (paths/booleans/stroke/tessellation) is
  the natural promotion target once the model stabilizes; coordinate before promoting so Pigment's shape
  layers and Pulse's shape layers consume the *same* primitives.
- **Out of scope — a sibling app's domain (do not build here):**
  - *Deep raster painting, photo retouch, raster filters/adjustments as the primary surface* → **Pigment**.
    Contour embeds/links raster images and can apply raster *live-effects* (via the shared `prism-fx`),
    but a placed photo is edited in Pigment (smart-object round-trip via suite interop).
  - *Timeline, keyframes, animation, motion graphics, video* → **Pulse / Reel**. Illustrator has no
    timeline; Contour is static vector. Animated/parametric repeats are fine (they're spatial, not
    temporal); anything time-based is a Pulse comp.
  - *Cross-app interop glue* (Dynamic Link, the `prism-doc` interchange container, shared clipboard &
    asset library) is **suite-level** — Contour consumes it (an artboard places live into Pigment/Pulse)
    but does not define it unilaterally.

---

## 1. Current state (what runs today)

Grounded in `contour/crates/contour-app/src/`. Past the README's "v0 scaffold" label:

- **Document model** (`document.rs`) — ordered `Vec<Shape>`; `Shape = Rect | Ellipse | Line | Path`.
  `Path` carries real cubic-Bézier **handles** (per-anchor out-tangent offsets, mirrored in-tangent),
  open/closed, fill/stroke/width, visibility. `kurbo` `BezPath` build + flatten + tight bounds; robust
  hit-testing (fill polygon + stroked segments); translate. Legacy-JSON-compatible (`#[serde(default)]`).
- **Tools** (`app.rs`) — Select, Rectangle, Ellipse, Line, **Pen** (Bézier; Enter/double-click closes).
- **Canvas** (`canvas.rs`) — artboard, cursor-anchored scroll zoom, drag/middle-drag pan, per-frame
  repaint via egui `Painter`.
- **Pathfinder** (`boolean.rs`) — Union / Intersect / Difference on closed shapes via `i_overlay`.
- **Inspector** — fill/stroke pickers, stroke-width, shape list (select / delete / visibility).
- **Export** (`export.rs`) — standalone **SVG** (rect/ellipse/line/path with cubic `C` commands) and
  **PNG** (rasterized via `tiny-skia` to artboard size). Save `.contour` (JSON via `serde` + `rfd`).
- **Shell** — `theme.rs` Prism dark theme, `icons.rs` phosphor tool glyphs; depends on `prism-core`
  (`Size`, `geometry::Rect`, `color::{srgb_to_linear, linear_to_srgb}`).

**Known gaps vs the README's own "out of scope" list, now scheduled below:** ~~undo/redo~~ (done),
multi-select, grouping, open/import, direct-anchor editing UI, real layers panel.

---

## 2. Validated tech stack (verify with `cargo add` at build)

| Concern | Crate | Notes |
|---|---|---|
| Bézier / path math | `kurbo` 0.11 | Curves, arclength, nearest-point, offset/stroke outline, bounds, area. Already in use. |
| Tessellation | `lyon` 1.0 | Fill/stroke → GPU triangle mesh when we move off the egui painter for big docs. |
| Boolean / pathfinder | `i_overlay` 1.x | Union/intersect/diff/xor, fill rules. Already in use; extend to the full pathfinder set. |
| App shell + UI | `eframe`/`egui` 0.34 | Shared with the suite. Glow backend today; wgpu path available if perf needs it. |
| Color | `prism-color` (+ `lcms2`/`qcms`) | Swatches, global/spot, CMYK, ICC — shared with the suite. |
| Doc / geometry | `prism-core` | `Size`, `Rect`, color boundary; promote path graph to `prism-vector` later. |
| Raster export | `tiny-skia` | CPU AA rasterizer (in use for PNG); also backs raster previews of live-effects. |
| Image trace | `vtracer` 0.6 | Raster → compact SVG; handles color photos (beats Potrace's binarized-only input). |
| SVG parse/render | `usvg` + `resvg` | Import SVG (and as a render reference); `resvg` for verifying our own output. |
| PDF / print | `pdf-writer` (+ `printpdf`) | Vector PDF export, multi-artboard, bleed/marks. |
| Fonts / type | `cosmic-text` + `swash` / `harfrust` | Shaping (incl. OpenType + variable fonts), outline extraction for convert-to-outlines. |
| Undo | `undo` / custom | Command stack over the path graph (structural + param deltas; cheap, no pixels). |
| Serde / util | `serde`, `glam`, `rayon` | Doc IO, math, parallel tessellation/boolean for big docs. |

---

## 3. Architecture (target)

```
┌──────────────────────────────────────────────────────────┐
│  contour-app  (eframe + egui)                            │
│  panels: tools · layers · appearance · swatches · props  │
│  canvas: artboards, guides/grid/snap, selection/handles  │
├──────────────────────────────────────────────────────────┤
│  vector model (promote → prism-vector when stable)       │
│  PathGraph · Anchor/Handle · Shape · Group · Layer       │
│  Appearance(fills/strokes/effects) · CommandStack(undo)  │
├──────────────────────────────────────────────────────────┤
│  kurbo (curves) · i_overlay (pathfinder) · lyon (tess)   │
│  prism-color (swatches/ICC) · prism-fx (raster effects)  │
├──────────────────────────────────────────────────────────┤
│  IO: usvg/resvg (SVG) · pdf-writer (PDF) · vtracer (trace)│
│      tiny-skia (raster) · .contour (native)              │
└──────────────────────────────────────────────────────────┘
```

### Core data model (target)
- **Document** = artboards + a layer tree + swatches/symbols/brushes/graphic-styles + color profile.
- **Layer** = ordered children: `Path | Group | CompoundPath | Text | Image(linked/embedded) | Symbol | MeshGradientObject`.
- **Appearance** (Illustrator's key non-destructive idea) = an ordered list of fills, strokes, and live
  effects per object/group, re-evaluated to a render. This is the vector analog of Pigment's render
  graph and the seam where `prism-fx` raster effects plug in.
- **CommandStack** = every edit reversible; the doc is cheap (paths + params), so full structural undo
  is affordable without tile tricks.

---

## 4. Phased backlog (toward ≥85% parity)

Effort tags **S/M/L** (solo-equivalent). "shared?" = touches/promotes a `prism-*` crate → keep
app-agnostic. Phases 0–1 are largely **done** (see §1); the rest is the road to parity.

### Phase 0 — Skeleton & canvas  *(DONE)*
- [x] eframe app, Prism theme/icons, artboard canvas, pan + cursor-anchored zoom
- [x] `Shape` model + `.contour` save (serde JSON), shape list, fill/stroke inspector

### Phase 1 — Draw, edit, pathfinder  *(DONE / near-done)*
- [x] Tools: Select, Rect, Ellipse, Line, Pen (Bézier handles, close on Enter/dbl-click)
- [x] Hit-test, translate, per-shape visibility
- [x] Pathfinder v1: Union / Intersect / Difference (`i_overlay`)
- [x] Export: SVG + PNG (`tiny-skia`)
- [x] **Undo/redo** (M) — snapshot history stack over the document (`history.rs`); Cmd/Ctrl+Z, Cmd/Ctrl+Shift+Z (or Ctrl+Y), Edit-menu entries. Coalesces drags (move / anchor-edit) into single entries; drops no-op drags; capped depth
- [ ] **Direct-select** (M) — drag anchors/handles; add/delete/convert anchor (smooth↔corner); marquee anchors

### Phase 2 — Selection, organize, transform  *(the daily-driver core)*
- [ ] Selection (M): multi-select (shift/marquee), group-select, **groups** + isolation mode, lasso, magic-wand (same fill/stroke), select-same, lock/hide per layer
- [ ] **Layers panel** (M): real layer tree, sublayers, reorder/drag, lock/hide/target, layer color
- [ ] Transform (M): rotate / scale / reflect / shear / free-transform with on-canvas handles; transform-each; numeric transform; **Transform Again**
- [ ] **Align & distribute** (S): edges/centers, distribute spacing, align-to (selection/artboard/key-object)
- [ ] **Guides / grid / smart guides / rulers** (M): snapping (point/grid/object/anchor), pixel-snap, measurement
- [ ] **Artboards** (M): multiple artboards, add/resize/reorder, per-artboard export, presets
- [ ] Arrange: bring-to-front/back, group/ungroup, lock, paste-in-place/in-front
- [ ] Tests: boolean/transform determinism, snapping math

### Phase 3 — Appearance, strokes, gradients  *(non-destructive depth — Illustrator's edge)*
- [ ] **Stroke options** (M): caps/joins/miter, **dashes**, **arrowheads**, align stroke (center/in/out), **width profiles** (variable-width via `kurbo` offset)
- [ ] **Appearance panel** (L): multiple fills/strokes per object/group; reorder; per-attribute opacity/blend
- [ ] **Gradients** (L): linear / radial / **angle** / **freeform (mesh-free)**; multi-stop + opacity stops; **gradient on stroke**; **dither + perceptual interpolation** (kill banding, smoother blends — IL 2025 parity)
- [ ] **Mesh gradient** (L): grid of color nodes, smooth multi-direction blends (`Object → Mesh`)
- [ ] **Patterns** (M): tile from selection, pattern fill/stroke, edit-pattern mode, seamless offsets
- [ ] **Live effects** (L): non-destructive drop-shadow / blur / glow / transform / distort (warp, zig-zag, roughen, pucker/bloat) / round-corners; raster effects via shared `prism-fx`; effect re-eval on edit
- [ ] **Graphic styles** (S): save/apply an Appearance as a named style
- [ ] **Blend modes + opacity masks** (M): reuse `prism-core` 18 blend modes; opacity mask from a shape; knockout
- [ ] Tests: stroke-outline correctness, gradient/mesh sampling, effect re-eval

### Phase 4 — Pathfinder, shapes, path tools  *(complete the geometry surface)*
- [ ] **Full Pathfinder** (M): add Exclude, Minus-Back, Divide, Trim, Merge, Crop, Outline; **compound paths** (even-odd/non-zero)
- [ ] **Shape Builder tool** (L): interactive merge/subtract by dragging across regions
- [ ] **Live shapes** (M): editable rounded-rect corners, polygon/star sides & radius, arc/spiral/grid; live corner widget
- [ ] **More creation tools** (M): polygon, star, rounded-rect, arc, spiral, rectangular/polar grid, **Pencil** (freehand fit), **Curvature** tool, **Blob brush**, line/segment
- [ ] **Path editing** (M): join, average, simplify (reduce anchors), smooth, **scissors**, **knife**, reshape, offset path, outline stroke
- [ ] **Width / Warp-family tools** (M): width, warp, twirl, pucker, bloat, scallop, crystallize, wrinkle
- [ ] **Blends** (M): blend tool between objects (steps/smooth-color), blend along a spine path
- [ ] **Envelope distort / puppet warp** (L): warp by shape / mesh / top-object
- [ ] Tests: compound-path fill rules, shape-builder region picking, simplify error bound

### Phase 5 — Type, color, symbols, brushes  *(production features)*
- [ ] **Type** (L): point + **area type**, **type-on-path**, threaded/overflow text, character + paragraph panels, OpenType + **variable fonts**, glyphs panel, text wrap, tabs, **convert to outlines**, character/paragraph **styles**
- [ ] **Color system** (M, shared): swatches, **global** + **spot** colors, color groups, CMYK/RGB/spot modes via `prism-color`, **Recolor Artwork** (palette remap/harmony rules), eyedropper, live-paint bucket
- [ ] **Symbols** (M): symbol library, instances, edit-master propagation, symbol sprayer/shifter/sizer set
- [ ] **Brushes** (L): calligraphic, art, scatter, pattern, bristle brushes along paths; brush library
- [ ] **Image trace** (M): `vtracer` raster→vector with presets (B/W, color, sketch, silhouette), threshold/path-fitting controls, expand to paths
- [ ] **Place / link images** (M): embed or link raster; clipping mask; crop image; round-trip placed `.pigment` via suite interop
- [ ] Tests: type-on-path layout, recolor mapping, trace path counts

### Phase 6 — IO, interchange, print  *(opens/saves everything)*
- [ ] **SVG** (M): full import (`usvg`) + faithful export (gradients, patterns, clips, text); SVG presets
- [ ] **PDF** (L): vector PDF export (`pdf-writer`) — multi-artboard pages, bleed/trim marks, embedded fonts/ICC; PDF import (paths/text)
- [ ] **EPS / AI bridge** (M): EPS export; best-effort `.ai` (PDF-compatible) read for interchange
- [ ] **Export for Screens** (M): per-artboard, multi-scale (@1x/2x/3x), multi-format (PNG/JPEG/WebP/SVG/PDF) via the suite encoders
- [ ] **Asset export** (S): drag-to-export, slices, naming conventions
- [ ] **Color-managed export** (M, shared `prism-color`): ICC embed, CMYK separations, soft-proof, overprint preview
- [ ] **Native `.contour`** (S): extend to layers/artboards/appearance/symbols; versioned; round-trip-tested
- [ ] Tests: SVG round-trip fidelity, PDF render vs `resvg` reference

### Phase 7 — AI, automation, extensibility  *(modern + pro workflows)*
- [ ] **AI (feature-gated, models on demand via shared `pigment-ai`/`ort`):** auto image-trace cleanup, **Generative Recolor** (palette synthesis), subject/auto-select, vectorize-from-prompt (optional cloud/local, BYO key) — degrade gracefully with no model
- [ ] **Automation** (M): actions (record/replay), batch export, scripting via `rhai` (document/path API); variables/data-merge → N variants
- [ ] **Plugins** (L, shared): `prism-fx` OpenFX-style effects (raster live-effects authored once for the suite)
- [ ] **Presets/assets** (S): shared suite library — swatches/brushes/symbols/styles/patterns; import `.ase`/`.ai`-swatches where feasible

### Phase 8 — Reliability, performance & ease-of-use  *(the polish that earns trust)*
- [ ] **Performance** (L): move large-doc rendering off the per-frame egui painter to a `lyon`→wgpu mesh cache; spatial index (R-tree) for hit-test/selection on docs with 10k+ objects; parallel boolean/tessellation (`rayon`)
- [ ] **Autosave + crash recovery** (M); **preferences** (units/snapping/UI/guides); **keyboard shortcuts** (full remappable map + command palette)
- [ ] **Workspaces & panels** (M): dockable/floating, save/load workspaces; tool options bar
- [ ] **Multi-document tabs** (M); **Navigator** + outline/preview modes; **Isolation mode** polish
- [ ] **Onboarding** (S): templates/new-doc presets, smart-guide hints, recent files
- [ ] Tests: autosave round-trip; 10k-object frame-time benchmark gate

---

## 4b. Parity coverage matrix (vs Illustrator surface)

| Category | Illustrator surface | Status | Phase |
|---|---|---|---|
| Canvas / pan / zoom / artboard | view + single artboard | **Done**; multi-artboard **Planned** | 0,2 |
| Draw: pen/shapes | rect/ellipse/line/Bézier pen | **Done** core; pencil/curvature/live-shapes/star/spiral **Planned** | 1,4 |
| Anchor / handle editing | smooth/corner, add/del/convert | **Partial** (handles in model) → UI **Planned** | 1 |
| Selection / groups / layers | multi/group/isolation, layer tree | **Planned** | 2 |
| Transform / align / distribute | rotate/scale/reflect/shear/free | **Planned** | 2 |
| Guides / grid / snap / rulers | full | **Planned** | 2 |
| Pathfinder | union/intersect/difference | **Done** 3 of ~10; rest + compound + Shape Builder **Planned** | 1,4 |
| Stroke options / width profiles | dashes/arrows/caps/joins/variable | **Planned** | 3 |
| Appearance (multi fill/stroke/fx) | full non-destructive stack | **Planned** | 3 |
| Gradients (linear/radial/freeform) | + dither + perceptual | **Planned** | 3 |
| Mesh gradient | full | **Planned** | 3 |
| Patterns / symbols / brushes | full | **Planned** | 3,5 |
| Live effects | non-destructive fx | **Planned** (via `prism-fx`) | 3 |
| Blend modes / opacity masks | full | **Planned** (reuse `prism-core`) | 3 |
| Blends / envelope / puppet warp | full | **Planned** | 4 |
| Type / area / on-path / variable | full | **Planned** | 5 |
| Color: swatches/spot/CMYK/recolor | full | **Planned** (shared `prism-color`) | 5,6 |
| Image trace | Image Trace | **Planned** (`vtracer`) | 5 |
| Place / link / clip images | full | **Planned** (suite interop) | 5 |
| Export: SVG/PNG | yes | **Done**; PDF/EPS/AI/for-screens **Planned** | 1,6 |
| Undo/redo | full | **Done** (snapshot history, drag-coalesced) | 1 |
| AI (recolor/trace/generative) | Firefly | **Planned** (feature-gated) | 7 |
| Automation / scripting / plugins | actions/scripts/SDK | **Planned** | 7 |
| Perf (huge docs) / autosave / prefs / workspaces / multi-doc | full | **Planned** | 8 |
| 3D (extrude/revolve) / perspective grid | yes | **Later / optional** (lower priority) | 8+ |
| Timeline / animation / video | — | **Won't** (Pulse/Reel) | — |
| Deep raster painting / retouch | — | **Won't** (Pigment) | — |

---

## 5. Milestones

| Milestone | Phases | Capability | Approx parity |
|---|---|---|---|
| **Scaffold** | 0–1 | Draw/edit shapes & Bézier, pathfinder v1, SVG/PNG, save | ~25% *(here today)* |
| **MVP** | 2 | + undo, multi-select, groups, layers, transform, align, artboards, snapping | ~45% |
| **Pro draw** | 3–4 | + appearance, strokes, gradients/mesh, full pathfinder, shape/path tools, blends | ~70% |
| **Production** | 5–6 | + type, color/spot/CMYK, symbols, brushes, image trace, PDF/SVG/for-screens IO | **~85%** |
| **Parity+** | 7–8 | + AI, automation/plugins, perf at scale, autosave/prefs/workspaces | **≥90%** |

**The ≥85% line lands at the end of Phase 6.** Highest felt-parity-per-effort first: **undo (Ph1) →
selection/layers/transform (Ph2) → appearance/strokes/gradients (Ph3)**.

---

## 6. Hard problems (mitigations)

1. **No undo today** → add a command stack over the path graph first; the doc is small (paths+params), so full structural undo is cheap — no tile/COW tricks needed.
2. **Performance at scale** → egui-painter-per-frame won't hold 10k objects; cache `lyon`→wgpu meshes, add an R-tree spatial index, parallelize boolean/tessellation.
3. **Boolean robustness** → `i_overlay` float overlay; flatten Béziers at a controlled tolerance (curve fidelity is approximate post-boolean — acceptable, matches Illustrator's expanded output).
4. **Variable-width strokes & offset** → `kurbo` stroke-outline / offset; sample width profile along arclength.
5. **Color identity across the suite** → all color through `prism-color`; spot/CMYK as first-class, soft-proofed on export.
6. **Shared-crate discipline** → promote only generic vector primitives to `prism-vector`/`prism-core`; never couple them to Contour's UI, or Pigment's shape layers and Pulse's shape layers break.
7. **Non-destructive depth** → the Appearance stack is the vector render graph; live effects re-evaluate from params, raster effects route through `prism-fx`.

---

## 7. Immediate next steps

1. [x] **Undo/redo** command stack — unblocks confident editing of everything else. *(Done: `history.rs` snapshot stack, wired into every mutation.)*
2. [ ] **Direct-select** anchor/handle editing UI (model already stores handles).
3. [ ] **Phase 2 core** — multi-select, groups, real layers panel, transform handles, snapping.
4. [ ] **Appearance + stroke options + gradients** (Phase 3) — Illustrator's non-destructive edge.
5. [ ] Coordinate the **`prism-vector`** promotion with Pigment/Pulse owners before moving path/boolean code into a shared crate.

*Foundations are free. The product is the polish — and the glue between apps.*
