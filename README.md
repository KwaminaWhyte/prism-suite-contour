# Contour

Vector graphics editor — the Illustrator analog and **app #2 of the Prism creative suite**
(sibling to [Pigment](https://github.com/KwaminaWhyte/prism-suite-pigment), the raster editor).

Built in Rust with [`eframe`](https://github.com/emilk/egui)/`egui` 0.34 (glow backend).
Vectors are drawn through egui's `Painter` — no custom GPU pass needed. Bezier path
math runs on [`kurbo`](https://crates.io/crates/kurbo); the document serializes with `serde`.

## Status — v0 scaffold

Real but scoped. It builds, launches, and lets you draw, select, move, style, and save shapes.

**Implemented**

- **Document model** — an ordered `Vec<Shape>` (`Rect`, `Ellipse`, `Line`, `Path`),
  bottom-up paint order (newest on top in the list).
- **Canvas** — artboard with cursor-anchored scroll zoom and drag/middle-drag pan;
  every shape repainted each frame via the egui painter.
- **Tools** (left palette): Select, Rectangle, Ellipse, Line, Pen.
  - Rect / Ellipse / Line: click-drag to create with the current fill + stroke.
  - Pen: click to append points; double-click or **Enter** closes/commits the path.
  - Select: click picks the topmost shape under the cursor (hit-test); drag moves it;
    **Delete** / **Backspace** removes it.
- **Inspector** (right panel): fill picker, stroke picker, stroke-width slider, and a
  shape list (select / delete, newest first).
- **Menus**: File (New, Save `.contour` → JSON via `serde` + `rfd` save dialog),
  Edit (Delete).

**Out of scope for v0** (noted): undo/redo, true bezier handles (paths are
straight-segment polylines for now — the `kurbo::BezPath` seam is already in place),
open/import, multi-select, grouping, text.

## Shared foundation

Contour depends on the suite's shared crate **`pigment-core`** by path
(`../pigment/crates/pigment-core`) to demonstrate the shared-foundation model:

- `pigment_core::Size` — the logical artboard dimensions.
- `pigment_core::color::{srgb_to_linear, linear_to_srgb}` — at the color encode boundary.
- `pigment_core::geometry::Rect` — returned from `Shape::bounds()`.

`pigment-core` declares `[lints] workspace = true`, so Contour's workspace mirrors
Pigment's `[workspace.lints]` block; otherwise building it here errors on an
undefined `workspace.lints`.

## Build & run

```sh
# from prism/contour
cargo run        # launches the editor window
cargo build      # debug build
cargo fmt        # formatting (clean)
cargo clippy     # lints (clean)
```

Binary name: `contour` (crate `contour-app`).

## Layout

```
contour/
├── Cargo.toml                  # workspace + shared lint config + pigment-core path dep
└── crates/contour-app/
    └── src/
        ├── main.rs             # eframe entry point
        ├── app.rs              # tool state, panels, menus, per-frame loop
        ├── canvas.rs           # pan/zoom transform + shape painting
        ├── document.rs         # Shape enum, Document, hit-testing, kurbo bounds
        ├── theme.rs            # Prism dark theme
        └── icons.rs            # egui-phosphor install + tool glyphs
```
