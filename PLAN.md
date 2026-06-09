# Contour — Open Source Illustrator Alternative

Professional vector graphics editor in Rust, and **app #2 of the Prism suite** (sibling to
[Pigment](https://github.com/KwaminaWhyte/prism-suite-pigment), the raster editor). **Goal: reach ≥85% of Adobe Illustrator's real-world
capability** — features, reliability, and ease-of-use — in staged milestones, on the suite's shared
engine (`prism-core` / `prism-color`), with Bézier math on `kurbo`, tessellation on `lyon`, and
boolean ops on `i_overlay`.

> Companion docs: [RESEARCH.md](./RESEARCH.md) (cited findings + crate matrix), [README.md](./README.md) (current build), [SUITE.md](https://github.com/KwaminaWhyte/prism-suite-prism/blob/main/SUITE.md) (four-app vision + interop). This PLAN expands the README's v0 scaffold into a parity roadmap; the README still tracks what runs *today*.

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
- [x] **Direct-select** (M) — drag anchors/handles; add/delete/convert anchor (smooth↔corner); marquee anchors. *(Done: a dedicated **Direct-Select tool** (`A`; `V` switches back to Select) that picks, marquees, and drags individual anchors of a `Path` **and** of every sub-contour of a `Compound` (addressed by a `(contour, anchor)` pair via new `Shape::contour` / `contour_mut` / `set_anchor` / `set_handle`). Selected anchors expose **mirrored Bézier handles** you drag to reshape the curve live; **click a segment** inserts an anchor (de Casteljau, shape-preserving), **Delete** removes the selected anchors and re-fits the path (min-2-points guard), **Alt-click** converts smooth↔corner (corner = no handle, smooth = neighbour-derived mirrored tangent). **Marquee** rubber-bands anchors (Shift adds). On-canvas overlay draws selected vs unselected glyphs (round=smooth, square=corner) plus handle lines/knobs, pixel-aligned via the existing view mapping. All edits route through the undo system (drags coalesce; no-ops drop). New pure helpers `anchors_in_rect` / `handle_endpoints` / `make_corner` / `make_smooth` with 9 unit tests. **Still open:** **broken/independent** handles (the model stores one symmetric out-offset per anchor, so a corner with two independent tangents isn't representable yet — corner means "no handles"); a toolbar/menu convert action (only Alt-click today); anchor editing on `Rect`/`Ellipse`/`Line` without first converting them to paths.)*

### Phase 2 — Selection, organize, transform  *(the daily-driver core)*
- [ ] Selection (M): multi-select (shift/marquee), group-select, **groups** + isolation mode, lasso, magic-wand (same fill/stroke), select-same, lock/hide per layer
- [~] **Layers panel** (M): real layer tree, sublayers, reorder/drag, lock/hide/target, layer color. *(Done: a real Layers panel listing every object top-to-bottom in z-order, with groups shown as expandable/collapsible parent rows (via the pure `layers::rows`). Per row: **visibility** toggle, **lock** toggle (locked objects can't be selected/picked — gated through the shared `Shape::selectable()` used by every canvas pick path and the marquee), an **editable name** (blank → type label), a **layer-colour** swatch (click set / right-click clear), **click-to-target** selection kept in sync with the canvas (group header selects the whole group), and **reorder** via up/down + bring-to-front/send-to-back routed through the tested `arrange` ops. Name / lock / colour / hidden persist to `.contour` (all `#[serde(default)]` → back-compat). Pure row layout + lock/hide gating + serde round-trip are unit-tested. **Still open:** a true nested **sublayer tree** (the model is a flat `Vec<Shape>` tagged by group id, not a recursive tree), **drag-to-reorder** (button reorder ships instead), and per-layer **target dot** for appearance.)*
- [~] Transform (M): rotate / scale / reflect / shear / free-transform with on-canvas handles; transform-each; numeric transform; **Transform Again**. *(Done: on-canvas free-transform box — scale (corner/edge handles, Shift = aspect-lock), rotate (ring outside a corner), shear (Cmd/Ctrl-drag an edge); numeric scale + move in the inspector; 90°/180° rotate + flip H/V + rotate-by; **Transform Again** (Cmd/Ctrl+D) replays the last gesture about the current centre. Pure affine math unit-tested. Still open: transform-each per-object pivots; a floating numeric dialog with shear/rotate fields.)*
- [x] **Align & distribute** (S): edges/centers, distribute spacing, align-to (selection/artboard/key-object). *(Done: a pure, unit-tested `align` module turning a slice of bounding rects into per-object `(dx, dy)` translation deltas — six **edge/centre alignments** (left / H-centre / right / top / V-centre / bottom) against a reference frame, plus **distribute** by feature (edges/centres) **and distribute-spacing** (equal gaps) for 3+ shapes, which sorts by visual order so input order doesn't matter. **Align-to** is **selection bounds** or the **artboard** rect (`AlignTo`). Wired into both an inspector **Align section** (relative-to combo + edge/centre/distribute buttons, disabled until the selection is big enough) and an **Object ▸ Align / Distribute** menu; each click applies the deltas through the existing `translate` + `checkpoint` undo path as one labelled step (`Align Left`, `Distribute Horizontal Gaps`, …). No model change — it only moves existing shapes, so `.contour` files are unaffected. **Still open:** **align-to-key-object** (pick one selected shape as the fixed anchor); align/distribute on a per-anchor basis.)*
- [ ] **Guides / grid / smart guides / rulers** (M): snapping (point/grid/object/anchor), pixel-snap, measurement
- [ ] **Artboards** (M): multiple artboards, add/resize/reorder, per-artboard export, presets
- [ ] Arrange: bring-to-front/back, group/ungroup, lock, paste-in-place/in-front
- [ ] Tests: boolean/transform determinism, snapping math

### Phase 3 — Appearance, strokes, gradients  *(non-destructive depth — Illustrator's edge)*
- [x] **Stroke options** (M): caps/joins/miter, **dashes**, **arrowheads**, align stroke (center/in/out), **width profiles** (variable-width via `kurbo` offset). *(Done: **caps** (butt/round/square), **joins** (miter/round/bevel) + miter limit, and **dashes** (pattern + offset) model `StrokeStyle` and render on canvas (tiny-skia) + export (SVG `stroke-dasharray`/`-linecap`/`-linejoin`, PNG `StrokeDash`). **Align stroke** (center/inside/outside) shifts the band by offsetting the centerline ±w/2 along its outward normal (winding-independent), via pure `stroke::aligned_geometry`. **Arrowheads** (None/Triangle/Open/Circle, start + end, scalable) are **baked geometry** at the endpoint — portable to all three renderers (no SVG markers), with line-trim so a filled head's base meets the line (`stroke::arrowhead`/`arrow_decorations`/`trim_polyline`). All additive on `StrokeStyle` (`#[serde(default)]`) → pre-existing `.contour` files load centered & arrow-less. Canvas takes the shared tiny-skia raster path when a stroke needs align/arrows (`Appearance::needs_stroke_decor`) so canvas == PNG; the Stroke-options inspector edits all of it. **Still open:** **width profiles** / variable-width strokes (kurbo offset along arclength); align / arrowheads on **compound** paths and per-Appearance-stroke-layer in the inspector (the model supports per-layer; the panel edits the topmost stroke today).)*
- [x] **Appearance panel** (L) — multiple fills/strokes per object; reorder; per-item opacity/blend/visibility; stack-aware inspector; canvas + SVG/PNG export walk the stack bottom-to-top. Additive `appearance: Option<Appearance>` (`#[serde(default)]`) with on-demand legacy migration (single fill/stroke → one-element vecs). *(Done, **incl. blend compositing**: per-fill/stroke blend modes now really composite — separable Porter-Duff math (Multiply/Screen/Overlay/Darken/Lighten/ColorDodge/ColorBurn/HardLight/SoftLight/Difference/Exclusion) on premultiplied RGBA8 via the shared `tiny-skia` raster path on canvas + PNG, `mix-blend-mode` on SVG. **Still open:** a true **per-object** blend mode (per-fill/stroke today); non-separable **HSL** modes; live **effects** beyond Drop Shadow + Gaussian Blur; **mesh gradients**; **patterns**; group-level appearance.)*
- [~] **Gradients** (L): linear / radial / **angle** / **freeform (mesh-free)**; multi-stop + opacity stops; **gradient on stroke**; **dither + perceptual interpolation** (kill banding, smoother blends — IL 2025 parity). *(Done: multi-stop **linear / radial / angle (conic)** gradient fills **and strokes**, with per-stop RGBA (colour + opacity), spread mode (pad/repeat/reflect), and a per-gradient **perceptual (linear-light) ↔ sRGB interpolation** toggle + **Bayer ordered dithering** — all modelled on the shared `prism_core::gradient` primitive and rendered consistently on canvas (egui mesh), **PNG** (tiny-skia `LinearGradient`/`RadialGradient`, conic via a `prism-core`-style per-pixel `Pattern`) and **SVG** (`<linearGradient>`/`<radialGradient>` defs with perceptual stops pre-expanded into linear-light sub-stops; conic falls back to a directional linear def — SVG 1.1 has no conic). Additive `interpolation`/`dither` fields (`#[serde(default)]`) keep old `.contour` files byte-identical (sRGB, un-dithered). Inspector edits kind/spread/angle/stops/interpolation/dither. **Still open:** separate **opacity stops** as their own editor rail (opacity is per-stop in the RGBA colour today), the `Reflected`/`Diamond` geometries, a native SVG conic export, and **freeform / mesh** gradients.)*
- [ ] **Mesh gradient** (L): grid of color nodes, smooth multi-direction blends (`Object → Mesh`)
- [ ] **Patterns** (M): tile from selection, pattern fill/stroke, edit-pattern mode, seamless offsets
- [~] **Live effects** (L): non-destructive drop-shadow / blur / glow / transform / distort (warp, zig-zag, roughen, pucker/bloat) / round-corners; raster effects via shared `prism-fx`; effect re-eval on edit. *(**Drop Shadow** + **Gaussian Blur** Done: additive `effects: Vec<Effect>` on `Appearance`; rasterize-blur-composite via `tiny-skia` on canvas + PNG, standard `<filter>` (`feDropShadow`/`feGaussianBlur`) on SVG; add/remove/reorder/edit in the Appearance panel; pure box-blur / shadow math unit-tested. **Still open:** Transform, Outer Glow, the distort family, round-corners, effect blend compositing, and promotion of the raster effects to shared `prism-fx`.)*
- [x] **Graphic styles** (S): save/apply an Appearance as a named style. *(Done: a document-level `GraphicStyles` library (pure `graphic_styles` module) where each entry captures a full `Appearance` snapshot (fills / strokes / effects with their paint / opacity / blend / visibility). The inspector's new **Graphic Styles** section **saves** the selection's effective appearance as a named style, **applies** a style to the whole selection (overwriting each shape's appearance via the existing `set_appearance` path, one labelled "Apply Graphic Style" undo step), and **renames** / **deletes** styles (unique-name discipline mirroring Swatches). Additive `graphic_styles: GraphicStyles` document field (`#[serde(default)]` → empty) so pre-existing `.contour` files round-trip; the library serializes with the document. Pure save/apply/rename/round-trip unit-tested. **Still open:** drag-and-drop reorder, a thumbnail swatch preview per style, and merge-into-existing-style on re-save.)*
- [~] **Blend modes + opacity masks** (M): **Done** — 12 separable blend modes (Multiply…Exclusion) really composite on the Appearance stack via a `tiny-skia` premultiplied Porter-Duff raster path (canvas + PNG; `mix-blend-mode` on SVG), and **opacity masks** (`Object ▸ Opacity Mask ▸ Make / Release / Invert`) drive an object's alpha by another shape's luminance (additive `omask` tags, serde-default; luminance→alpha multiply in the raster path; SVG `<mask>` def). **Still open:** per-*object* blend mode, non-separable **HSL** modes, **group/layer** masks, and **knockout**
- [ ] Tests: stroke-outline correctness, gradient/mesh sampling, effect re-eval

### Phase 4 — Pathfinder, shapes, path tools  *(complete the geometry surface)*
- [x] **Full Pathfinder** (M): add Exclude, Minus-Back, Divide, Trim, Merge, Crop, Outline; **compound paths** (even-odd/non-zero). *(Done: all ten Pathfinder ops in `boolean.rs` on `i_overlay` — the original Union / Intersect / Minus Front (Difference) plus **Exclude** (Xor), **Minus Back** (InverseDifference), **Divide**, **Trim**, **Merge**, **Crop** and **Outline** (unfilled stroked boundary). A user-selectable **fill rule** (non-zero vs **even-odd**) threads through every op so nested input fills the Illustrator way; even-odd carves holes. New `Object ▸ Pathfinder` submenu (fill-rule toggle + all ops, enable-gated on exactly two selected shapes); per-op icons. **Now closed: the true compound-path object** — area-conserving ops return their regions **grouped**, so a result with holes is **one `Shape::Compound`** (outer ring + hole sub-contours) instead of separate rings, with `Object ▸ Compound Path ▸ Make / Release` (`Cmd/Ctrl+8` / `Alt+Cmd/Ctrl+8`) and a per-object even-odd/non-zero fill rule rendered + hit-tested + exported correctly. **Merge is exact** now: it welds adjacent same-coloured faces and trims (removes the hidden part of) different-coloured overlaps, **preserving each face's own fill** (was a simplified weld to the back colour). 16 boolean tests + compound model / export tests.)*
- [x] **Shape Builder tool** (L): interactive merge/subtract by dragging across regions. *(Done: `M` activates the tool; with two-or-more overlapping shapes selected, a plain drag **unites** every region the pointer crosses into one path, an Alt/Option-drag **deletes** them. A pure `shapebuilder` module builds the **region graph** (atomic faces) from the selection's `i_overlay` intersections, hit-tests which face the pointer is over (`face_at` / `faces_along`), and merges / subtracts on release (one undo step), inheriting the back-most face's paint. Live overlay shades the crossed regions + traces the drag. 7 unit tests cover face count / tiling, picking, drag collection, unite / subtract, and paint inheritance.)*
- [ ] **Live shapes** (M): editable rounded-rect corners, polygon/star sides & radius, arc/spiral/grid; live corner widget
- [ ] **More creation tools** (M): polygon, star, rounded-rect, arc, spiral, rectangular/polar grid, **Pencil** (freehand fit), **Curvature** tool, **Blob brush**, line/segment
- [ ] **Path editing** (M): join, average, simplify (reduce anchors), smooth, **scissors**, **knife**, reshape, offset path, outline stroke
- [ ] **Width / Warp-family tools** (M): width, warp, twirl, pucker, bloat, scallop, crystallize, wrinkle
- [~] **Blends** (M): blend tool between objects (steps/smooth-color), blend along a spine path. *(**Make / Release / Expand** (`Object ▸ Blend`, **specified-steps** mode) Done: generate *N* intermediate objects morphing between two selected shapes — **arc-length resample** both outlines to a common point count via `kurbo` (handles differing anchor counts), point-by-point geometry interpolation, plus fill/stroke colour + opacity + width interpolation (linear, straight sRGB). Expand-on-create (real `Path` steps spliced between the ends); additive `blend` / `blend_step` tags (`#[serde(default)]`); Steps-count UI in the Object menu; resample / interpolate / step math unit-tested. **Still open:** persistent **live re-blend** (re-blend when an end moves), **smooth-color** (spread-steps) mode, **blend along a spine** path, bezier-**handle** interpolation (steps are corner paths), point-correspondence rotation, and **mixed open/closed** topology.)*
- [ ] **Envelope distort / puppet warp** (L): warp by shape / mesh / top-object
- [~] Tests: ~~compound-path fill rules~~ (done), ~~shape-builder region picking~~ (done), simplify error bound

### Phase 5 — Type, color, symbols, brushes  *(production features)*
- [~] **Type** (L): **point type** + **convert to outlines** — *landed* (Type tool `T`; click-to-place + inline edit with backspace/enter/esc; font-size + left/centre/right alignment; multi-line; real glyph outlines from a bundled font via `ttf-parser`, cached on `Shape::Text` and composing / exporting like a compound path through canvas + SVG + PNG; `Object ▸ Type ▸ Create Outlines` → editable `Compound`; `.contour` round-trip). *Open:* **area type**, **type-on-path**, threaded/overflow text, character + paragraph panels, **font selection** + OpenType shaping / kerning + **variable fonts**, glyphs panel, text wrap, tabs, character/paragraph **styles**.
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
| Pathfinder | union/intersect/difference/exclude/minus-back/divide/trim/merge/crop/outline + even-odd/non-zero | **Done** — all 10 ops + fill rules, **true compound-path object** (holes kept as sub-contours of one object; `Compound Path ▸ Make/Release`), **Shape Builder** tool (drag to merge/subtract), and **exact Merge** (weld same-colour / trim different) | 1,4 |
| Stroke options / width profiles | dashes/arrows/caps/joins/variable | **Done** (caps/joins/miter, dashes, align stroke center/inside/outside, arrowheads start/end scalable — canvas + SVG + PNG); **width profiles** (variable-width) **Planned** | 3 |
| Appearance (multi fill/stroke/fx) | full non-destructive stack | **Done** (multi fill/stroke, reorder, per-item opacity/blend/visibility; blend **compositing** real now — 12 separable modes; live **effects**: Drop Shadow + Gaussian Blur); per-object/HSL blend + remaining effects **Planned** | 3 |
| Gradients (linear/radial/freeform) | + dither + perceptual | **Done** — linear/radial/**angle** multi-stop fills **+ strokes**, perceptual↔sRGB interpolation + Bayer dither (canvas/PNG/SVG); separate opacity-stop rail, Reflected/Diamond, native SVG conic & **freeform/mesh** **Planned** | 3 |
| Mesh gradient | full | **Planned** | 3 |
| Patterns / symbols / brushes | full | **Planned** | 3,5 |
| Live effects | non-destructive fx | **Partial** — Drop Shadow + Gaussian Blur (canvas/SVG/PNG); rest **Planned** (via `prism-fx`) | 3 |
| Blend modes / opacity masks | full | **Done** — 12 separable blend modes composite (canvas/PNG/SVG); opacity masks (luminance→alpha, invert); per-object/HSL/group-mask/knockout **Planned** | 3 |
| Blends / envelope / puppet warp | full | **Planned** | 4 |
| Type / area / on-path / variable | full | **Point type + convert-to-outlines done; area / on-path / variable planned** | 5 |
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
2. [x] **Direct-select** anchor/handle editing UI (model already stores handles). *(Done — dedicated Direct-Select tool (`A`); see Phase 4.)*
3. [ ] **Phase 2 core** — multi-select, groups, real layers panel, transform handles, snapping.
4. [ ] **Appearance + stroke options + gradients** (Phase 3) — Illustrator's non-destructive edge.
5. [ ] Coordinate the **`prism-vector`** promotion with Pigment/Pulse owners before moving path/boolean code into a shared crate.

*Foundations are free. The product is the polish — and the glue between apps.*

---

## UI/UX & workspace

What a pro vector app's *shell* needs that we still lack. The inspector and tool
column are now **scrollable + collapsible** (Affinity-Studio style: a right-side
stack of collapsible property groups, a compact left tool column); the rest of
this list is the road to a workspace that scales. Effort tags **S/M/L**.

- [x] **Scrollable panels** (S) — tool palette + inspector wrapped in `ScrollArea::vertical` so nothing is unreachable on a short window. *(Done.)*
- [x] **Collapsible property groups** (S) — inspector sections grouped under `CollapsingHeader` with sensible default-open/closed. *(Done.)*
- [ ] **Dockable / floating panels** (L) — drag panels out, redock, split; tear-off windows. (egui has no native docking; needs `egui_dock` or a custom dock tree.)
- [ ] **Saveable workspaces** (M) — named layouts (panel positions/visibility/sizes) saved to prefs and switchable; ship Essentials/Layout/Typography presets like Illustrator.
- [x] **Window menu — panel show/hide** (S) — a `Window` menu lists every panel (Tools / Inspector / Status bar) with a checkbox to toggle visibility, plus a reset-panels command; the canvas always fills the remaining space. *(Done.)*
- [ ] **Contextual tool-options bar** (M) — a top strip under the menu that shows the active tool's options (e.g. corner radius for Rect, anchor controls for Pen, stroke for shape tools) — Illustrator's Control bar / Affinity's context toolbar.
- [ ] **Customizable tool palette** (M) — reorderable/groupable tools, flyout groups (e.g. shape tools share a slot), show/hide tools, single vs double column.
- [ ] **Tabbed panel groups** (M) — stack multiple panels into one frame with tabs (Affinity Studio); drag a tab between groups.
- [ ] **Keyboard-shortcut map** (M) — a viewable/remappable shortcut table + a command palette; per-tool single-key activation (V/M/L/P…); export/import shortcut sets. *(Overlaps Phase 8 "keyboard shortcuts".)*
- [ ] **Dark / light theme toggle** (S) — switch the Prism theme between dark and light (and follow-OS) at runtime; persist in prefs.
- [ ] **Panel resize + min/max + scroll-on-overflow polish** (S) — consistent resize grips, remembered widths, density (compact/comfortable) option.
- [x] **Status / context bar** (S) — bottom bar: cursor coords (px), selection count, active artboard, zoom % + quick 1:1 / fit-zoom buttons; toggleable from the Window menu. *(Done.)*

---

## Parity gaps spotted vs Illustrator + Affinity Designer

Skim of PLAN against both apps. Most are already filed in §4 (cross-referenced);
the genuinely-missing or under-specified ones are added here so nothing slips.
Record-only — not scheduled into a phase yet.

- [x] **Appearance / Effects panel as a first-class UI** (L) — the panel surface for the Appearance stack (add/reorder/toggle fills & strokes per object, plus live effects). *(Done: stack-aware inspector over an `appearance` module + additive `Option<Appearance>`; canvas + SVG/PNG walk the stack; an **Effects** section adds/removes/reorders/edits Drop Shadow + Gaussian Blur.)*
- [~] **Live shape effects** (M) — non-destructive effect entries (drop-shadow, blur, glow, round-corners, warp/distort) editable after the fact via the Appearance panel. *(**Drop Shadow** + **Gaussian Blur** Done — see §3 "Live effects". Glow / round-corners / warp / distort still open.)*
- [ ] **Layer effects / fx on layers & groups** (M) — apply the same live effects to a whole layer or group, not just a single object (Affinity's per-layer FX).
- [ ] **Isolation mode** (M) — double-click a group to edit its contents in isolation, dimming everything else; breadcrumb to exit. *(Listed in §2/§8; surface as its own deliverable.)*
- [ ] **Recolor Artwork** (M) — remap a selection's colors via a palette/harmony UI, global-color edits, reduce-to-N-colors. *(In §5; the interactive dialog is the missing piece.)*
- [ ] **Image Trace** (M) — `vtracer` raster→vector with B/W / color / sketch presets and an expand-to-paths step. *(In §5; flagged here as a headline parity feature.)*
- [ ] **Mesh & freeform gradient** (L) — gradient mesh objects and Illustrator-2019-style freeform gradient points. *(In §3; large effort, surfaced.)*
- [x] **Clipping masks + compound paths** (M) — `Object → Clip` (mask by topmost shape) and even-odd/non-zero compound paths; both everyday operations. *(Done: clipping masks shipped earlier; **compound paths now a real object** — `Object ▸ Compound Path ▸ Make / Release` (`Cmd/Ctrl+8`), a `Shape::Compound` with an even-odd/non-zero fill rule, holes kept as sub-contours, rendered + hit-tested + SVG/PNG-exported correctly, and Pathfinder results with holes produce one compound. See §4.)*
- [ ] **Export presets / slices / Export for Screens** (M) — saved per-artboard export presets, drag-to-export assets, slice regions, multi-scale (@1x/2x/3x). *(In §6; called out as a polish gap.)*
- [x] **Eyedropper / paste-in-place / paste-in-front-back** (S) — sample appearance from any object; precise paste positioning. (Affinity + Illustrator staple.) *(Done: `eyedropper` module + Eyedropper tool (`I`) samples/applies fill, gradient, stroke colour/width/style; paste-in-place / -front / -back shipped with the clipboard.)*
- [ ] **Symbols & global swatches in the UI** (M) — a Swatches panel and a Symbols panel (instances + edit-master). *(Model in §5; the panels are the gap.)*
- [ ] **Pixel-preview / overprint / soft-proof preview modes** (M) — view modes that render the artwork as it will export (pixel grid, CMYK proof, overprint). *(Partly §6; the view-mode toggles are missing.)*
