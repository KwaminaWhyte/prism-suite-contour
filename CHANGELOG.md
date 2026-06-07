# Changelog

All notable changes to **Contour** (the Prism suite's vector editor) are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Appearance panel (multiple fills / strokes per object, reorderable, with
  per-item opacity / blend / visibility)** — a new pure, unit-tested `appearance`
  module and an additive `appearance: Option<Appearance>` on every shape
  (`#[serde(default)]`), promoting Contour's former single-fill / single-stroke
  shape into Illustrator's non-destructive **Appearance stack**:
  - An **`Appearance`** is an ordered `Vec` of **`Fill`**s and a `Vec` of
    **`Stroke`**s. Each entry carries a **`Paint`** (a solid straight-sRGB RGBA
    colour or an overriding multi-stop `Gradient`), a per-item **opacity**
    (`0..=1`), a **blend mode**, and a **visibility** toggle; a stroke also keeps
    its own width and `StrokeStyle` (caps / joins / dashes). Painting walks the
    fills then the strokes **bottom-to-top**, so later entries sit over earlier
    ones — exactly like a layer stack. The reorder helpers (`raise_fill` /
    `lower_fill` / `raise_stroke` / `lower_stroke`), the legacy bridge
    (`from_legacy`), and the per-item `apply_opacity` are pure functions pinned by
    unit tests (no egui context).
  - A new **stack-aware inspector** Appearance section: **add / remove** fills and
    strokes, **reorder** each within its list, toggle a per-item **visibility**
    eye, and edit the selected item (paint — solid ⇄ gradient — opacity, blend,
    and a stroke's width / style). Edits land as single undo steps and feed the
    app's paint defaults so the next new shape inherits the look.
  - Rendered consistently across all three surfaces from the same model: the
    **on-canvas** painter, **PNG** export, and **SVG** export each walk the stack
    bottom-to-top and emit one layered paint per entry, with per-item opacity
    folded into each paint's alpha.
  - **Backward compatible.** `appearance` is additive (`#[serde(default)]` →
    `None`): a shape with `None` renders identically from its legacy single
    fill / stroke fields, so every pre-existing `.contour` file loads and renders
    the same. `Appearance::from_legacy` migrates a single fill / stroke into a
    one-element-each stack **on demand** (the first time the user opens the
    Appearance section on an old shape) — a zero-width or fully-transparent legacy
    stroke, and a fill-less line, migrate without inventing an empty entry.
    Appearance stacks round-trip through serde, with per-item fields
    (`opacity` / `blend` / `visible`) defaulted so a minimal or older stack still
    loads.
  - **Deferred this pass** (the struct is the seam where they attach): per-shape
    **blend compositing** (blend modes are stored and editable but only `Normal`
    composites today), a live **effects** vec, **mesh gradients**, and
    **patterns**.

- **Swatches panel (named colour library, global swatches)** — a new pure,
  unit-tested `swatches` module and an additive `swatches: Swatches` palette on
  the `Document` (`#[serde(default)]`), bringing the everyday Illustrator /
  Affinity colour-library staple Contour was missing:
  - A **`Swatch`** is a named straight-sRGB RGBA colour with a stable id and a
    **global** flag; a **`Swatches`** collection is the document palette — an
    ordered, name-unique list (a clash gets a numeric suffix, à la Illustrator)
    with colour de-duplication so adding the same colour twice never piles up
    duplicates. A fresh document opens with a small starter palette (white /
    black / grey + red / orange / yellow / green / blue). All of the palette
    bookkeeping — `next_id`, `unique_name`, `add` (de-dup by colour),
    `rename` (unique, but a self-rename is a no-op), `set_global`, `recolor`,
    `remove`, `id_for_color` — is pure and pinned by unit tests (no egui
    context).
  - A new **Swatches panel** docked on the left (toggled from the Window menu,
    which now lists it): a grid of colour chips you **click** to paint the
    selection's fill (and set the default fill so the next shape inherits it),
    **Shift-click** to paint the stroke, or **Alt-click** to select for editing.
    The chip naming the current fill gets an accent ring; a global swatch shows a
    small corner dot. A `+` button adds a swatch from the current fill, and an
    editor for the selected swatch renames it, recolours it, toggles its global
    flag, applies it to fill / stroke, or deletes it. Each edit is one undo step.
  - **Global swatches** recolour the artwork: editing a global swatch's colour
    hands back its `(old, new)` pair, and `Document::remap_color` walks every
    shape — remapping the old colour across solid fills, strokes, **and gradient
    stops** (with the same picker-rounding tolerance the swatch model uses) — so
    artwork painted with that swatch follows the edit, and the status bar reports
    how many shapes changed. A non-global swatch is a plain shortcut: editing it
    touches only the swatch, and deleting any swatch leaves the artwork's colours
    intact. The `swatches` field is additive (`#[serde(default)]`), so a
    pre-swatches `.contour` loads with the default starter palette; palettes
    (names, colours, global flags) round-trip through serde. The document-level
    `remap_color` integration is pinned by unit tests alongside the module's pure
    helpers.

- **Clipping masks (`Object → Clipping Mask → Make / Release`)** — a new pure,
  unit-tested `clip` module and two additive `(clip, mask)` tags on every shape
  (`#[serde(default)]`), bringing the everyday Illustrator / Affinity operation
  of cropping artwork to the shape on top:
  - **Make** turns the selection into a **clip set**: the **topmost** selected
    shape becomes the **mask**, and the shapes below it are clipped to its
    outline. **Release** restores the originals. Wired into an **Object →
    Clipping Mask** submenu, an inspector "Clipping Mask" section, and the
    Illustrator keys **Cmd/Ctrl + 7** (make) / **Alt + Cmd/Ctrl + 7** (release).
    Make needs 2+ unclipped objects; Release lights up whenever the selection
    touches a clip set. Each is a single undo step, and Make re-stacks the set
    into one contiguous block (mask on top) the way grouping does.
  - The mask is modelled non-destructively: the mask and the clipped content
    keep their original geometry (so Release is loss-free and the set round-trips
    through serde), and the *rendered* result is **derived**, never stored.
    `Document::render_shapes` resolves clip sets on the fly — the mask paints
    nothing (an Illustrator clipping path has no fill/stroke), and each content
    shape is replaced by its outline **intersected against the mask** via
    `i_overlay` (`clip::clip_polygon`), yielding an ordinary single-ring polygon.
    Content lying entirely outside the mask drops out; an unusable mask degrades
    gracefully to the unclipped original.
  - Rendered consistently across all three surfaces by routing each through
    `render_shapes`: the **on-canvas** painter (clipped bodies, with a selected
    mask shown as a dashed accent outline so it stays editable), **PNG** export,
    and **SVG** export (clipped content emitted already cropped, so the file
    matches the canvas without `<clipPath>` plumbing).
  - A clip set selects and moves as one unit, like a group: clicking,
    shift-clicking, or marquee-touching any member selects the whole set (the
    group/clip selection expansion is now unified). The `clip`/`mask` tags are
    additive (`#[serde(default)]` → `None` / `false`), so older `.contour` files
    load unclipped. The clip-set planning helpers (`next_clip_id`, `can_make`,
    `can_release`, `members_of`, `mask_of`, `selected_clip_ids`) and the
    intersection geometry are pure functions pinned by unit tests (no egui
    context), alongside document-level `render_shapes` resolution tests.

- **Eyedropper tool (sample / apply paint appearance)** — a new pure,
  unit-tested `eyedropper` module and an **Eyedropper** tool in the left palette
  (keyboard **I**), bringing the everyday Illustrator / Affinity "copy the look
  of that object onto this one" staple:
  - An **`Appearance`** is the *paint* of a shape — its fill (solid colour
    and/or an overriding gradient), stroke colour, stroke width, and stroke
    style (caps / joins / dashes) — detached from the shape's geometry, group
    membership, and visibility. `Appearance::sample` reads it off a shape;
    `Appearance::apply_to` writes it onto another shape **without disturbing
    geometry** (an ellipse stays an ellipse, only its colours change). Both are
    pure functions pinned by unit tests (no egui context).
  - **Click** a shape with the Eyedropper to sample its appearance: the look is
    applied to the current selection as one undo step, and the app's default
    paint is loaded from the sample so the next shape you draw inherits it.
    With **no selection**, the click only loads the defaults (nothing to apply
    to yet). **Alt-click** never paints — it samples into the defaults only
    (Illustrator's "pick up but don't apply" modifier). Clicking empty canvas is
    a no-op.
  - Edge cases match Illustrator: a **`Line`** carries no fill, so sampling it
    leaves a filled target's fill colour intact while clearing any gradient and
    copying the stroke; applying any appearance to a line takes only the stroke.
    New `Shape::stroke_color` / `set_stroke_color` / `stroke_width` /
    `set_stroke_width` accessors back the transfer (shaped like the existing
    `fill_color` / `stroke_style` accessors). The inspector shows a usage hint
    while the Eyedropper is active.

- **Clipboard (copy / cut / paste / duplicate)** — a new pure, unit-tested
  `clipboard` module and a `Clipboard` buffer on the app, bringing the
  everyday Illustrator clipboard staples Contour was missing:
  - **Edit → Cut / Copy / Paste / Paste in Place / Paste in Front / Paste in
    Back** and **Duplicate**, plus the Illustrator keys **Cmd/Ctrl + C**
    (copy), **X** (cut), **V** (paste), **Shift + V** (paste in place), **F**
    (paste in front), **B** (paste in back), and **D** (duplicate).
  - **Copy** snapshots the selection (expanded to whole **groups**, in paint
    order) into a detached buffer that lives *outside* the document, so it
    survives undo/redo and can be pasted repeatedly (or into a new document).
    **Cut** copies then deletes the selection as one undo step.
  - **Paste** drops the buffer on top of the stack, fanned out by a growing
    diagonal nudge (`clipboard::paste_offset`) so repeated pastes step
    down-and-right instead of hiding behind one another, à la Illustrator.
    **Paste in Place** lands the copies at their original coordinates on top;
    **Paste in Front / Back** do the same but force the copies to the front /
    back of the paint order. **Duplicate** makes a single nudged copy without
    disturbing the clipboard. Each paste / duplicate is one undo step and
    selects the new copies.
  - Pasted **group** membership is preserved without colliding with the
    destination: `clipboard::remap_group_ids` rewrites the buffer's group ids
    to fresh ones (starting above every id already in use), keeping shapes that
    shared an id in the buffer grouped together and distinct buffer groups
    distinct, while ungrouped shapes stay loose. The paste nudge growth and the
    group-id remap are pure functions pinned by unit tests (no egui context).

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
