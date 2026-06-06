# Changelog

All notable changes to **Contour** (the Prism suite's vector editor) are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Window menu (show/hide panels) + status bar** — a new pure, unit-tested
  `workspace` module and a `Workspace` panel-visibility state on the app, giving
  the editor's shell the workspace controls a pro vector app needs:
  - A **Window** menu in the menu bar lists every dockable panel (Tools,
    Inspector, Status bar) with a checkbox to toggle its visibility — the central
    canvas always fills whatever space is left — plus a **Reset panels** command
    (disabled when the layout already matches the default) that restores the
    everything-shown layout. The checkboxes bind straight to the workspace flags
    (`Workspace::flag_mut`), so hiding the inspector or tool column reclaims the
    space for the canvas, à la Illustrator's Window menu.
  - A **bottom status / context bar** spanning the window under the artwork:
    live document-space cursor coordinates (`X / Y px`, a `–` placeholder when
    the pointer leaves the canvas), the selection count (`No selection` /
    `1 selected` / `N selected`), the active artboard name, and the zoom
    percentage. The right side carries quick **1:1** (reset zoom to 100%) and
    **Fit** (fit all artboards) buttons, mirroring Illustrator's bottom-left zoom
    field. The bar itself is toggleable from the Window menu.
  - The status-line wording and zoom-percentage formatting are pure functions
    (`workspace::status_line` / `workspace::zoom_percent`) so the exact text is
    pinned by unit tests; the `Workspace` struct's visibility bookkeeping
    (default-all-shown, per-panel flags, reset, is-default) is likewise tested
    without an egui context, and is `#[serde(default)]`-friendly for a future
    saved-workspaces feature.

- **Multiple artboards** — a new pure, unit-tested `artboard` module and an
  additive `artboards: Vec<Artboard>` + `active_artboard: usize` on the
  `Document` (both `#[serde(default)]`), promoting the editor's former single
  fixed-`Size` artboard into a stack of named rectangles, the way Illustrator
  lays out several artboards on one canvas:
  - An **`Artboard`** is a named `[x, y, w, h]` rectangle in document space. The
    module's pure geometry — default placement of a new board (tiled to the
    right of the rightmost existing board, top-aligned), topmost-hit picking, and
    the union of all boards — lives apart from any UI so it is unit-tested
    directly.
  - A new **Artboard tool** in the left palette: drag on empty canvas to create a
    board, drag an existing board to move it, click to make a board active. Every
    artboard paints under the artwork with its name labelled above the top-left
    corner; the **active** board gets an accent frame + label, inactive boards a
    muted grey frame. Create / move land as single undo steps (artboards live on
    the `Document`, so the snapshot history already captures them).
  - An inspector **Artboards** section lists every board (click to activate,
    trash to delete down to one), an **+ Add** button (and **Object → New
    Artboard**) tiles a fresh board sized like the active one, and a per-board
    editor renames + resizes the active board (one undo step). **View → Fit
    artboards** zooms/pans so the union of all boards fills the window.
  - **Per-artboard export** — SVG and PNG now crop to the *active* artboard's
    rectangle: the PNG canvas is the board's pixel size with the artwork
    translated by `(-x, -y)`, and the SVG sizes its `viewBox` to the board and
    wraps the body in a `translate(-x, -y)` group (omitted at the origin) — so a
    board placed anywhere on the canvas exports as its own cropped image, à la
    Illustrator's per-artboard export. **Align → To artboard** now measures
    against the active board rather than a fixed origin rectangle.
  - The fields are additive (`#[serde(default)]`), so a pre-artboards `.contour`
    loads with exactly one default 1000×700 board at the origin (its former
    behaviour); artboards round-trip through serde, and opening a file repairs a
    corrupt / hand-edited stack (always ≥1 board, active index clamped).

- **Grouping (group / ungroup)** — a new pure, unit-tested `group` module and an
  additive `group: Option<u64>` tag on every shape (`#[serde(default)]`), so
  shapes sharing an id form one group that selects, moves, transforms and arranges
  as a single unit, the way Illustrator's groups behave for day-to-day editing —
  without refactoring the flat `Vec<Shape>` document into a tree:
  - **Object → Group / Ungroup**, an inspector "Group" button row, and the
    Illustrator keys **Cmd/Ctrl + G** (group) / **Cmd/Ctrl + Shift + G**
    (ungroup). Group needs 2+ selected shapes; Ungroup lights up whenever the
    selection touches a group. Each is a single undo step.
  - **Group** assigns a fresh group id (never colliding with an existing one) to
    the selection and gathers its members into one contiguous block in paint
    order, anchored at the frontmost selected shape (so a group reads as a single
    stacked unit). The selection is remapped to the moved block so the group stays
    selected. **Ungroup** clears the tag on every member of each group the
    selection touches.
  - **Selection propagation** — clicking (or shift-clicking, marquee-selecting, or
    picking in the Layers list) any member of a group selects the *whole* group; a
    shift-click toggles the entire group in or out atomically. Dragging then moves,
    and the transform box scales/rotates, the whole group together. A leading group
    glyph marks grouped shapes in the Layers list.
  - The tag is additive (`#[serde(default)]` → `None`), so older `.contour` files
    load ungrouped and group membership round-trips through serde. Group ids
    survive `Shape::to_path` (so rotating a grouped rect/ellipse keeps it in its
    group), and boolean-op results are ungrouped.

- **Gradient fills (multi-stop, linear & radial)** — a new pure, unit-tested
  `gradient` module and an additive `fill_gradient: Option<Gradient>` on every
  filled shape (`Rect` / `Ellipse` / `Path`) that overrides the solid `fill`
  when present:
  - A `Gradient` is geometry-free — an ordered set of colour **stops** at
    parametric offsets `0..=1`, a **kind** (linear at a chosen **angle**, or
    radial), and a **spread mode** (Pad / Repeat / Reflect). The renderer maps
    the `0..=1` parameter onto the shape's bounding box (`linear_endpoints` /
    `radial_params`), so a gradient always follows the object's bounds the way
    Illustrator's gradient fills do. `color_at` interpolates between the two
    bracketing sorted stops, clamping/repeating/reflecting per the spread mode.
  - An inspector **Fill** section toggles **Solid ⇄ Gradient**; the gradient
    editor picks kind, spread and angle, and edits the **stop list** (add a stop
    at the widest gap sampled in place, recolour, drag the offset, remove down to
    two). Like the stroke section it edits the primary selected shape (one undo
    step per change) and tracks the app default so the next new shape inherits
    the fill.
  - Rendered consistently across all three surfaces: the **on-canvas** painter
    (a per-vertex gradient triangle-fan `Mesh` — exact for convex shapes, a
    faithful preview otherwise), **PNG** export (tiny-skia `LinearGradient` /
    `RadialGradient` shaders with matching spread mode), and **SVG** export
    (`<linearGradient>` / `<radialGradient>` defs in user space, one per
    gradient-filled shape, referenced via `fill="url(#…)"`; multi-stop with
    `stop-opacity` and `spreadMethod`).
  - The field is additive (`#[serde(default)]`), so older `.contour` files load
    as a solid fill; gradients round-trip through serde. Boolean-op results
    inherit the subject shape's gradient.

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
