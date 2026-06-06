# Changelog

All notable changes to **Contour** (the Prism suite's vector editor) are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Arrange (stacking order)** — a new pure, unit-tested `arrange` module that
  reorders the flat paint-order list for the four Illustrator commands
  (**Bring to Front**, **Bring Forward**, **Send Backward**, **Send to Back**),
  always returning a true permutation that preserves the relative order of both
  the moved and the untouched shapes:
  - Wired into an **Object → Arrange** menu, an inspector "Arrange" button row,
    and the Illustrator keys **Cmd/Ctrl + ]** (forward) / **[** (backward), with
    **Shift** for to-front / to-back.
  - Multi-selections move as blocks; a command is a single undo step and the
    selection is remapped through the same permutation so the same shapes stay
    selected. No-op moves (selection already at the extreme) are disabled in the
    UI and skip the undo checkpoint.

- **Marquee (rubber-band) selection** — dragging the Select tool over empty
  canvas draws a translucent accent box and live-selects every visible shape
  whose bounding box intersects it (a new unit-tested `rects_intersect` helper).
  **Shift-drag** is additive (extends the prior selection); a plain marquee
  replaces it. The topmost intersected shape stays primary. A marquee never
  mutates the document, so it records no undo entry.

- **Snapping, guides, grid & rulers** — a new pure `snap` module (unit-tested
  nearest-target snapping over per-axis candidate coordinates) wired into the
  canvas and a new **View** menu:
  - **Rulers** (on by default) frame the canvas with top and left strips showing
    document-unit ticks at a zoom-aware "nice" step (1 / 2 / 5 × 10ⁿ), plus an
    accent cursor read-out tracking the pointer on both rulers.
  - **Ruler guides** — drag out of the left ruler for a vertical guide, the top
    ruler for a horizontal one; drag an in-progress guide back onto a ruler to
    discard it. Guides persist on the `Document` (additive
    `#[serde(default)]`, so older `.contour` files load with none) and are a
    single undo step. **View → Clear guides** removes them all.
  - **Grid** — an optional document grid at the configurable grid size (every
    fifth line emphasised), auto-hidden when it would be too dense to read.
  - **Snapping** — moving a selection, creating a shape, or dropping a guide
    snaps to any combination of the **grid**, **guides**, and **other objects'**
    edges/centres (each toggled independently in View → Snap to). The closest of
    the moving box's left/centre/right and top/middle/bottom features wins per
    axis, à la Illustrator's smart guides; the active snap lines draw in magenta.
    Snap tolerance is a fixed pixel distance pulled into document units, so it
    feels identical at every zoom. A dragged shape never snaps to itself.

- **Transform box (rotate / scale / reflect)** — a new pure `transform` module
  (a 2×3 `Affine` matrix with `scale_about` / `rotate_about` / `flip_*` pivot
  constructors, plus handle-drag → scale-factor and rotate-angle helpers; all
  unit-tested) wired into an on-canvas free-transform box and an inspector +
  **Object → Transform** menu:
  - The Select tool draws a dashed bounding box around the selection with eight
    handles (four corner + four edge). **Drag a handle** to scale — corner
    handles scale both axes, edge handles a single axis, and the *opposite*
    handle stays pinned as the pivot. **Shift-drag** a corner locks the aspect
    ratio. **Drag just outside a corner** to rotate about the box centre. Each
    drag is exact (re-applied from a start-of-drag snapshot every frame, so no
    float drift) and lands as a single undo step.
  - Inspector "Transform" section and **Object → Transform** menu add quick
    **Rotate 90° CW/CCW**, **Rotate 180°**, **Flip Horizontal/Vertical**, and a
    numeric **Rotate by** (degrees) about the selection's centre.
  - `Shape::apply_affine` transforms `Line`/`Path` in place (handles, being
    offsets, transform by the matrix's *linear* part only). Axis-aligned
    `Rect`/`Ellipse` stay their own variant under pure translate/scale/flip; a
    rotation or shear rasterises them into an editable `Path` via the new
    `Shape::to_path` (rect → four-corner polygon, ellipse → four-anchor cubic
    approximation), matching how Illustrator turns a rotated primitive into a
    path.

- **Multi-selection** — the Select tool now maintains a full selection *set*
  rather than a primary/secondary pair. Plain-click selects one shape;
  **shift-click** toggles a shape in or out of the set (in the canvas and the
  Layers list); dragging any selected shape moves the **whole selection**
  together as a single undo step. The most-recently-added shape is the *primary*
  (drives the inspector and direct-select path editing). Boolean ops now require
  exactly two selected shapes (subject = first, clip = second).

- **Align & distribute** — a new `align` module (pure, unit-tested geometry over
  bounding boxes) plus an inspector "Align" section and an **Object → Align /
  Distribute** menu:
  - **Align** the selection's left / horizontal-center / right edges and top /
    vertical-center / bottom edges to a reference frame, switchable between the
    **selection bounds** and the **artboard** (so a single shape can be centered
    on the artboard).
  - **Distribute** three-or-more shapes by evenly spacing a chosen feature (left
    / right / top / bottom edges or centers) *or* by equalising the **gaps**
    between them (horizontal / vertical "distribute spacing", à la Illustrator).
    Distribution sorts by visual position, so selection order does not matter and
    the two outermost shapes stay fixed.
  - Each align/distribute action is a single undo step; controls disable until
    the selection is large enough (2+ to align, 3+ to distribute).

- **Stroke options** — every shape now carries a `StrokeStyle` (line **cap**:
  butt / round / square; line **join**: miter / round / bevel; **miter limit**;
  and a **dash pattern** with phase **offset**), edited from a new "Stroke
  options" inspector section with dash presets (Solid / Dashed / Dotted /
  Dash-dot). The style applies to new shapes and, when a shape is selected, to
  the selection live as a single undo step. Rendered faithfully across all three
  surfaces: the on-canvas painter (dashes), **PNG** export (full caps / joins /
  miter / dashes via `tiny-skia`), and **SVG** export (`stroke-linecap`,
  `stroke-linejoin`, `stroke-miterlimit`, `stroke-dasharray`,
  `stroke-dashoffset`; default attributes omitted to keep output compact). The
  field is additive (`#[serde(default)]`), so older `.contour` files load as a
  solid butt/miter stroke.

- **Direct-select path editing** — complete on-canvas anchor/handle editing for
  `Path` shapes with the Select tool:
  - Double-click a path segment to **add an anchor** (straight segments split at
    the click point; curved segments split via de Casteljau, preserving the
    exact curve shape).
  - Double-click an anchor to **delete** it (refuses to drop below two anchors).
  - Alt-click an anchor to **convert** it between **smooth** (auto-tangent from
    neighbouring anchors) and **corner** (no handle).
  - Anchors render with Illustrator-style glyphs: round for smooth, square for
    corner.
  - A contextual "Edit path" hint appears in the inspector when a path is
    selected. All edits are a single undo step.

## [0.0.1] - 2026-06-06

### Added

- **Vector editor scaffold** — eframe/egui app shell, Prism dark theme, Phosphor
  tool-glyph icons, and an artboard canvas with cursor-anchored scroll zoom and
  drag / middle-drag pan.
- **Shapes & document model** — ordered `Vec<Shape>` (`Rect`, `Ellipse`, `Line`,
  `Path`) with hit-testing, bounds, and translation; `.contour` save (JSON via
  `serde`). Tools: Select, Rectangle, Ellipse, Line.
- **Bézier pen tool** — click-to-place anchors with drag-to-set cubic tangent
  handles; Enter / double-click closes the path. Anchor/handle dragging on a
  selected path.
- **Pathfinder v1** — Union / Intersect / Difference on closed shapes via
  `i_overlay`.
- **Export** — standalone SVG (with cubic curve commands) and PNG (rasterized via
  `tiny-skia`) sized to the artboard.
- **Layers panel** — shape list with per-layer visibility toggle, reorder
  up/down, delete, and click / shift-click selection.
- **Undo / redo** — snapshot history stack over the whole document
  (`Cmd`/`Ctrl`+`Z`, `Cmd`/`Ctrl`+`Shift`+`Z` or `Ctrl`+`Y`, plus Edit-menu
  entries); coalesces drags into single entries, drops no-op drags, capped depth.

### Changed

- Depend on the suite-level shared crate `prism-core` (was a `pigment-core` path
  dependency) for `Size`, `geometry::Rect`, and the sRGB↔linear color boundary.

[Unreleased]: https://github.com/prism-suite/contour/compare/v0.0.1...HEAD
[0.0.1]: https://github.com/prism-suite/contour/releases/tag/v0.0.1
