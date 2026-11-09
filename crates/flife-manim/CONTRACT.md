# flife-manim contract

**Purpose.** FL-018 (`flife-jdf`) covers FrankenLife's FrankenManim bulk world adapter (plan §13.1, §13.2, §13.6).
The live ecology enters a FrankenManim scene as one bulk primitive, never as one scene object per organism:

- `Viewport` is the exact map between world units, FrankenManim's scene, drawable pixels and logical window points,
  and the 3D camera whose frame it is.
- `ParticleCloud` is §13.2's `ParticleCloudView`: one bounded cloud lowered from a render epoch onto FrankenManim's
  3D camera route.
- `FieldLayer` draws the epoch's energy field under the organisms as one vertex-coloured grid.
- `WorldView` holds all three, and builds each frame's `ThreeDJob`.

**Inputs.**
- An FL-017 `Epoch`, read only: identity-ordered positions, energies and lineages, the bounded field texture, the
  world extent and the population ceiling (`Epoch::capacity`).
- A drawable size in pixels. A logical window size in points, for pointer conversion.
- `[presentation] background` as `#rrggbb` (`hex_rgb`).

**Outputs.**
- `Viewport::fit(extent, drawable)`:
  - `scale = min(Dw / W, Dh / H)` drawable pixels per world unit, the same on both axes;
  - `origin = ((Dw − W·scale) / 2, (Dh − H·scale) / 2)`, so the whole torus is centred and the rest is letterbox.
- Coordinate spaces:
  - world `[0, W) × [0, H)` with y down;
  - scene `(x − W/2, H/2 − y, 0)`;
  - drawable `origin + world · scale`, with pixel `(i, j)` covering `[i, i + 1) × [j, j + 1)`;
  - logical points scale to the drawable by the drawable-to-logical ratio on each axis, so high-DPI is one ratio.
- Conversions:
  - `drawable_to_world` and `logical_to_world` return `None` in the margin, at the far (half-open) edges, and while
    the logical size is empty or not finite.
- `Viewport::camera(background)`:
  - a FrankenManim `ThreeDCamera` at the drawable's resolution;
  - its frame is `frame_shape() = (Dw / scale, Dh / scale)`, centred at the origin, at `DEFAULT_FOVY`, `zxz`;
  - `max_allowable_norm` is the world's diagonal, and there is no multisampling.
  - A scene point at `z = 0` projects to `world_to_drawable` of its world point.
- `ParticleCloud::update(epoch, viewport)`:
  - one `ThreeDDraw::TrueDot` per row, in identity order;
  - centred on the row's scene point;
  - radius `max(DOT_RADIUS = 0.35 world units, DOT_RADIUS_MIN_PIXELS = 1.5 px / scale)`;
  - colour `lineage_color(lineage)`: a fixed SplitMix64 mix of the lineage identity into a 12-colour categorical
    palette.
  - one glow halo per dot, in the same order and drawn under every dot: the lineage colour at alpha
    `HALO_ALPHA = 0.42`, radius `dot · halo_factor(energy)` with `halo_factor(e) = 2 + 2.5·e/(e + 2)` (2 to 4.5;
    2 for non-finite or negative energy).
  - `refit(viewport)` changes only the radii, halos in proportion, because scene points do not depend on the
    drawable.
- `FieldLayer::update(field)`:
  - one vertex per texel corner, `(texels + 1)²`, spanning the world exactly; each coloured by the mean of the four
    texels around it, wrapping, so opposite edges match and the torus shows no seam;
  - the mean maps through `sqrt(value / scale)` and the six-stop `FIELD_PALETTE`; `scale` is the brightest texel,
    or the previous scale times `SCALE_DECAY = 0.98` if larger, so the picture follows the field down slowly;
  - drawn unshaded and without depth test (`SurfaceDraw`, vertex colours), which FrankenManim's CPU and Metal 3D
    routes both accept.
- `WorldView`:
  - `show(epoch)` lowers an epoch and records it as `shown()`, a `Prior`;
  - `resize(drawable)` refits the viewport, camera and radii, returning whether the size changed;
  - `job()` is the frame: the field, then every halo, then every dot;
  - `render_rgba8(threads)` renders it on FrankenManim's CPU 3D route to tight sRGB RGBA8 (for `flife render`);
  - `scene_objects()` is 2: the field and the cloud.

**Invariants.**
- *Bounded scene objects (§13.2).* The whole population is one scene object, the field another, and a frame one 3D
  job. There is no mobject or animation handle per organism.
- *Bounded memory.* The draw buffer is reserved once, with fallible reservation, at the population ceiling. No
  update or refit allocates or moves it: `reserved_bytes` and the buffer's address are constant for the cloud's
  life.
- *Observation neutrality.* Everything comes from an immutable epoch. Nothing here reaches the world.
- *Stable identity.* Draw `i` is epoch row `i`, which is identity order (FL-017). A lineage keeps its colour for the
  whole run.
- *Exact mapping.* `world_to_drawable` and `drawable_to_world` are inverses on the world. FrankenManim's own CPU 3D
  renderer draws each dot where the viewport says.

**Refusal cases.**
- `Viewport::fit` with an extent that is not finite and positive, or a drawable with a zero side:
  `NumericalInvalid`.
- A camera or frame FrankenManim refuses: `NumericalInvalid`, carrying FrankenManim's error.
- An epoch with more rows than the cloud holds: `BudgetExhausted`. An epoch of another world than the viewport's:
  `LayoutMismatch`. Either way the previous draws, and what `WorldView` shows, are kept.
- A failed reservation: `AllocationFailed`.
- A `WorldView::resize` that fails leaves the view unchanged.

**Determinism.** Lowering is a pure function of the epoch and the viewport: F32 positions widened exactly to F64,
one F64 affine map, and a fixed integer colour hash. The CPU render oracle uses FrankenManim's deterministic CPU 3D
route.

**Independent tests.**
- `tests/viewport.rs` (6 tests):
  - Letterbox fit for wide, tall and exact aspects.
  - World and drawable round trips; the margin and the half-open far edges are not the world.
  - Retina-style logical points through the drawable ratio; empty and NaN logical sizes.
  - Scene centring with y up, and the frame shape for exact, wide and tall drawables.
  - Degenerate extents and drawables are refused.
  - *CPU render oracle:* FrankenManim's CPU `ThreeDJob::render` draws a dot at five world points of a 160 × 90 and
    a 90 × 160 drawable, letterboxed each way. The coverage-weighted centroid of each, decoded from the `Rgba16F`
    frame, is within 0.05 px of `world_to_drawable`. FrankenManim keeps a frame's height and fits its width to
    the pixel aspect, so only the tall drawable tests the frame height through the render.
- `tests/cloud.rs` (6 tests):
  - Every row of a real 24-tick epoch becomes one dot, in identity order, at its scene point with its lineage
    colour.
  - Six re-lowerings and a resize keep the reserved bytes and the buffer address; one scene object.
  - Ceiling and wrong-world refusals keep what was shown.
  - A resize refits only the radius, and the pixel floor binds when the world is small.
  - The palette spreads 4,096 lineages over all 12 colours, between 250 and 450 each.
  - `#rrggbb` parsing.
- `tests/field.rs` (5 tests):
  - The palette is monotone dark to bright; the display value is a clamped square root, zero for NaN, negatives and
    a zero scale.
  - A ramp field's grid spans the world, matches across both seams, and colours the seam corner by its wrapped
    neighbours (3.5, not the interior 0.5).
  - The display scale rises at once and falls by `SCALE_DECAY` per update.
  - Every dot of a real epoch has one halo at its centre, sized by its energy, in the lineage colour at
    `HALO_ALPHA`; a resize keeps halos proportional.
  - A real frame rendered offline is opaque and lit by the field beyond the black background.
- In `flife-studio`, `tests/live.rs` shows real epochs of the 4,096-organism reference world through this adapter in
  a real window.
- Planted bugs in this crate were each caught (listed in the FL-018 commit).

**Validity domain.** CPU host epochs (FL-017). The FrankenManim 3D camera route at `fmn` rev `303189d3`. The whole
torus is fitted into one drawable.

**Known gaps.**
- Lowering is CPU preparation per epoch: FrankenManim's `DotCloud`-equivalent reference route (§13.1). Resident
  device buffers and the shared-device resource bridge are FL-033's (and FL-028's typed Metal buffers).
- Sizes and colours are provisional. Visual mappings, `founder-lineage-v1`, density rendering, palettes and raw views
  are FL-019's. `[presentation] point_radius_pixels`, bloom and trails are not applied yet.
- The view shows the whole torus. Pan, zoom, views that wrap across the seam, interpolation and selection are
  FL-020's.
- Rendering-performance qualification (§13.7: 1080p at 30 fps, frame-time percentiles together with simulation
  slowdown) is G6 work. Nothing here is a performance claim.
