# Contour — Research Findings (June 2026)

Cited findings backing [PLAN.md](./PLAN.md). Verify all crate versions against crates.io at build time
— third-party version metadata is sometimes stale. Contour is **app #2 of the Prism suite**; shared
infrastructure decisions live in [SUITE.md](https://github.com/KwaminaWhyte/prism-suite-prism/blob/main/SUITE.md) and mirror [Pigment's research](https://github.com/KwaminaWhyte/prism-suite-pigment/blob/main/RESEARCH.md).

---

## 1. Vector path engine (kurbo / lyon / i_overlay)

- **`kurbo` 0.11** — the linebender Bézier/affine library: `BezPath`, `CubicBez`, arclength,
  nearest-point, **stroke outlining and path offset** (the basis for variable-width strokes and
  outline-stroke), tight bounding boxes, area/winding. Already the backbone of `document.rs`
  (`bez_path`, `flatten`, `bounds`). Curve flattening at a tolerance (we use 0.25 doc-units) feeds
  hit-testing, polygon fills, and boolean input.
- **`lyon` 1.0** — fill/stroke **tessellation** to GPU triangle meshes. Not needed while the doc is
  small and drawn through egui's `Painter`, but the path off the per-frame painter for 10k+ object
  documents: tessellate once, cache the mesh, draw via wgpu. Fill rules (even-odd / non-zero) map to
  compound paths.
- **`i_overlay` 1.x** — robust polygon **boolean** (union/intersect/difference/xor) with fill rules;
  already wraps Union/Intersect/Difference in `boolean.rs`. Extends to the full Pathfinder set
  (Exclude = xor; Divide/Trim/Merge/Crop/Outline = compositions of overlay results) and to the
  interactive **Shape Builder** (classify each sub-region by which inputs cover it). Beziers are
  flattened before overlay, so post-boolean curve fidelity is approximate — the same trade-off
  Illustrator makes when it expands.

Sources: github.com/linebender/kurbo · docs.rs/lyon_tessellation · lib.rs/crates/i_overlay

## 2. SVG / PDF / raster IO

- **SVG out** — already hand-emitted in `export.rs` (rect/ellipse/line/path with cubic `C` commands,
  fill/stroke + opacity). To reach faithful export we add gradients (`linearGradient`/`radialGradient`),
  patterns, clip paths, groups, and text. **`usvg`** parses/normalizes SVG for **import**, and
  **`resvg`** renders SVG to a pixmap — useful as a *reference rasterizer* to diff our own output
  against in tests.
- **PDF out** — **`pdf-writer`** (low-level, precise PDF construction: content streams, fonts, ICC,
  multiple pages = multiple artboards, bleed/trim boxes) is the right tool for print-grade vector PDF;
  **`printpdf`** is a higher-level alternative. Illustrator's `.ai` is PDF-compatible, so a PDF reader
  doubles as a best-effort `.ai` import path.
- **Raster** — **`tiny-skia`** (already used) is a fast CPU anti-aliased rasterizer (Skia's algorithms
  in safe Rust) for PNG export and for rasterizing live-effects/previews. Encoders for
  PNG/JPEG/WebP/AVIF come from the suite stack (`image`/`oxipng`/`mozjpeg`/`webp`/`ravif`).

Sources: crates.io/crates/usvg · crates.io/crates/resvg · crates.io/crates/pdf-writer ·
crates.io/crates/printpdf · crates.io/crates/tiny-skia

## 3. Image Trace (raster → vector)

- **`vtracer` 0.6.5** (VisionCortex, Rust, MIT) — converts raster (PNG/JPEG) to compact **SVG**. Unlike
  Potrace (binarized input only), VTracer has a full image-processing pipeline handling **colored,
  high-resolution** scans and photographs, and its output is *more compact* (fewer shapes) than
  Illustrator's Image Trace. Tunables: color precision, layer/gradient step, curve-fitting mode
  (pixel/polygon/spline), corner threshold, path simplification — these become the Trace presets
  (B/W, color, sketch, silhouette). "Expand" converts the traced result to editable Contour paths.

Sources: crates.io/crates/vtracer · lib.rs/crates/vtracer (VisionCortex)

## 4. Gradients, mesh, appearance (Illustrator's non-destructive core)

- **Gradients** — linear/radial/angle/freeform; multi-stop with opacity stops; gradient-on-stroke.
  Illustrator 2025 added **gradient dither** (kill banding) and **perceptual interpolation** (blend in
  a perceptual space — OKLab-class — for smoother, more natural transitions): both are cheap shader/
  sampling changes and worth matching. Color stops interpolate through `prism-color`.
- **Mesh gradient** — a grid of color nodes where colors flow in multiple directions and transition
  smoothly (Illustrator's Gradient Mesh). Model as a Coons/bilinear patch grid; convert a
  gradient-filled object to a mesh on demand. Rasterize via tessellated patches (`lyon`) or a CPU
  evaluator.
- **Appearance** — Illustrator's defining non-destructive idea: an object/group carries an ordered
  stack of **multiple fills, multiple strokes, and live effects**, re-evaluated to a render. This is
  the vector analog of Pigment's render graph; raster live-effects (blur/shadow/glow) route through the
  suite-shared **`prism-fx`** (OpenFX-style) so an effect is authored once and reused across apps.
  Graphic styles = a saved Appearance.

Sources: helpx.adobe.com/illustrator (gradients, mesh objects, appearance, what's-new 2025) ·
en.wikipedia.org/wiki/Coons_patch · ../pigment/RESEARCH.md §7–8 (prism-fx, layer-style passes)

## 5. Type & fonts

- **`cosmic-text`** (shaping via HarfRust, raster via swash; bidi; editable buffers) is the suite's text
  engine (Pigment already uses it). For Contour it additionally supplies **glyph outline extraction**
  (swash/`ttf-parser`) for *convert-to-outlines*, plus **OpenType features** and **variable-font** axes
  (Illustrator 2025 animates/sets variable axes from one file). Area type, point type, threaded text, and
  **type-on-path** are layout passes on top: type-on-path samples glyph positions along a `kurbo`
  arclength parameterization of the path.

Sources: github.com/pop-os/cosmic-text · docs.rs/ttf-parser · helpx.adobe.com/illustrator (type, variable fonts)

## 6. Color, swatches, recolor

- Color is the suite's `prism-color` (linear-light, ICC v2/v4 via `lcms2`, CMYK/Lab, soft-proof; `qcms`
  for wasm/RGB). Contour adds the *authoring* layer: swatches, **global colors** (edit once → updates
  everywhere), **spot colors** (named separations for print), color groups. **Recolor Artwork** =
  cluster the artwork's colors, map them onto a target palette or **harmony rule** (complementary/
  analogous/triadic generated from color theory), preserving relationships — a pure algorithm, no model
  required; an optional AI palette-synthesis step (Firefly's "Generative Recolor") is the Phase-7 add.
- **Live Paint** = treat overlapping paths as a planar map of fillable faces/edges (built from the same
  `i_overlay` planar subdivision used by Shape Builder).

Sources: helpx.adobe.com/illustrator (recolor artwork, global/spot color, live paint) · github.com/kornelski/rust-lcms2

## 7. Undo, automation, performance

- **Undo** — the document is cheap (paths + params, no pixels), so a linear/tree command stack (`undo`
  crate or custom) recording structural + parameter deltas is sufficient; no tile-COW needed (contrast
  Pigment, where pixel ops force tile diffs). This is the single biggest current reliability gap.
- **Automation** — **`rhai`** (sandboxed, pure-Rust, easy binding) exposes a document/layer/path API for
  scripting and recorded **actions**; batch export and data-merge (variables → N artboard variants) sit
  on top. Plugins = suite `prism-fx` OpenFX-style effects.
- **Performance at scale** — Illustrator handles tens of thousands of objects. The current
  egui-`Painter`-per-frame repaint won't; the path is: tessellate with **`lyon`** and cache meshes on a
  wgpu canvas (the suite already runs wgpu in Pigment), add an **R-tree** (`rstar`) spatial index for
  hit-test/marquee/selection, and parallelize boolean/tessellation with **`rayon`**.

Sources: crates.io/crates/undo · rhai.rs · crates.io/crates/rstar · docs.rs/lyon_tessellation · github.com/rayon-rs/rayon

## 8. AI (feature-gated, shared runtime)

Reuse the suite's `pigment-ai`/**`ort`** (ONNX Runtime, CoreML/DirectML/CUDA EPs) rather than a separate
stack. Vector-relevant uses: **subject/auto-select** and matting (BiRefNet/SAM) to seed a trace region;
**Generative Recolor** palette synthesis; optional **vectorize-from-prompt** (cloud or local diffusion →
`vtracer`). Models are not bundled — fetched on first use behind a feature flag with license surfacing;
every AI tool degrades gracefully when models/GPU are absent. (Details: [Pigment RESEARCH.md §10](https://github.com/KwaminaWhyte/prism-suite-pigment/blob/main/RESEARCH.md).)

Sources: github.com/pykeio/ort · github.com/ZhengPeng7/BiRefNet · ../pigment/RESEARCH.md §10
