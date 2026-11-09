# flife-manim contract

**Purpose.** FL-018 (`flife-jdf`) covers FrankenLife's FrankenManim bulk world adapter (plan §13.1, §13.2, §13.6).
FL-019 (`flife-g90`) adds the visual mappings, raw views and density rendering (plan §2.3, §13.3 step 5, §13.4).
The live ecology enters a FrankenManim scene as a few bulk primitives, never as one scene object per organism:

- `Viewport` is the exact map between world units, FrankenManim's scene, drawable pixels and logical window points,
  and the 3D camera whose frame it is.
- `ParticleCloud` is §13.2's `ParticleCloudView`: one bounded cloud lowered from a render epoch onto FrankenManim's
  3D camera route.
- `FieldLayer` draws the epoch's energy field under the organisms as one vertex-coloured grid.
- `DensityLayer` counts organisms per screen bin, for worlds denser than their pixels.
- `Style` holds a frame's drawing choices: the `ColorMapping`, the `Look` (glow or raw), the `Particles` (points or
  density) and whether the field is drawn.
- `WorldView` holds all of them, and builds each frame's `ThreeDJob`.

**Inputs.**
- An FL-017 `Epoch`, read only: identity-ordered positions, velocities, energies, lineages, generations and
  topology catalog indices, the bounded field texture, the world extent, the speed limit and the population ceiling
  (`Epoch::capacity`).
- A drawable size in pixels. A logical window size in points, for pointer conversion.
- `[presentation] background` as `#rrggbb` (`hex_rgb`), and `[presentation] color_mapping`, a registered
  `presentation.color_mapping` name (`ColorMapping::configured`).

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
- Colour mappings (`mapping.rs`). Each is a fixed function of one epoch row and the world's own constants. It never
  reads other rows, earlier frames or a fitted statistic, so an unchanged organism keeps its colour:

  | `ColorMapping` | Registered name | Reads | Colour |
  |---|---|---|---|
  | `lineage` (Appendix A's default) | `founder-lineage-v1` | lineage id | `LINEAGE_PALETTE[lineage_slot(id)]`: 12 luminous categorical hues (teal, violet, magenta, pale cyan, amber, lime and six like them), slot a fixed SplitMix64 mix of the id |
  | `energy` | `energy-lightness-v1` | energy `e` | `ENERGY_RAMP` (magma 0.2 to 1, 9 stops) at `e / (e + 2)`; lightness rises monotonically |
  | `generation` | `generation-log-v1` | generation `g` | `GENERATION_RAMP` (plasma 0.33 to 1, 7 stops) at `ln(1 + g) / ln(257)`, clamped |
  | `motion` | `motion-heading-v1` | velocity | `MOTION_WHEEL` hue for the heading (east coral, turning counter-clockwise on screen), mixed from the grey `MOTION_STILL` by `√(speed / speed limit)` |
  | `topology` | `topology-catalog-v1` | catalog index `k` | `TOPOLOGY_PALETTE[k mod 8]`; the first five are the catalog members in size order |

  `FromStr` takes either name. Colours are for display only: no metric reads them back.
- `ParticleCloud::update_styled(epoch, viewport, style)` (`update` is the default style):
  - one `ThreeDDraw::TrueDot` per row, in identity order;
  - centred on the row's scene point;
  - radius `max(DOT_RADIUS = 0.35 world units, DOT_RADIUS_MIN_PIXELS = 1.5 px / scale)`;
  - colour `mapped_color(style.mapping, row, speed limit)`;
  - in the glow look, one halo per dot, in the same order and drawn under every dot: the dot's colour at alpha
    `HALO_ALPHA = 0.42`, radius `dot · halo_factor(energy)` with `halo_factor(e) = 2 + 2.5·e/(e + 2)` (2 to 4.5;
    2 for non-finite or negative energy). The raw look has no halos.
  - with `Particles::Density`, no dots or halos: the density layer draws instead.
  - `refit(viewport)` changes only the radii, halos in proportion, because scene points do not depend on the
    drawable.
- `FieldLayer::update(field)`:
  - one vertex per texel corner, `(texels + 1)²`, spanning the world exactly; each coloured by the mean of the four
    texels around it, wrapping, so opposite edges match and the torus shows no seam;
  - the mean maps through `sqrt(value / scale)` and the six-stop `FIELD_PALETTE`; `scale` is the brightest texel,
    or the previous scale times `SCALE_DECAY = 0.98` if larger, so the picture follows the field down slowly;
  - drawn unshaded and without depth test (`SurfaceDraw`, vertex colours), which FrankenManim's CPU and Metal 3D
    routes both accept.
  - `update_look(field, Look::Raw)` draws linear grey `raw_value = clamp(value / scale, 0, 1)` instead: no square
    root, no palette.
- `DensityLayer::update(epoch, style)`:
  - bins of about `DENSITY_BIN_PIXELS = 6` drawable pixels, aligned to the world so the torus wraps exactly:
    `ceil(extent · scale / 6)` per axis, in `1..=DENSITY_BINS_MAX = 512`;
  - each organism counted once, in the bin `floor(position / extent · bins)`;
  - `scale` is the fullest bin, or the previous scale times `SCALE_DECAY` if larger; it restarts when the bin grid
    changes;
  - glow: a bin shows the mean linear colour of the current mapping over its organisms, at opacity
    `ln(1 + n) / ln(1 + scale)`, over the field; an empty bin is transparent;
  - raw: opaque linear grey `n / scale`, and the field is not drawn;
  - one vertex-coloured grid with vertices on bin corners, each the premultiplied mean of the four bins around it,
    wrapping, drawn unshaded without depth test.
- `WorldView`:
  - `show(epoch)` lowers an epoch in the current style and records it as `shown()`, a `Prior`;
  - `set_style(style)` takes effect at the next `show`; showing the same epoch again redraws it in the new style;
  - `resize(drawable)` refits the viewport, camera, radii and density bins, returning whether the size changed;
  - `draws()` is the frame in painter order: the field (unless hidden, or under raw density), then the density
    grid, or every halo and then every dot; `job()` wraps it in a `ThreeDJob`;
  - `render_rgba8(threads)` renders it on FrankenManim's CPU 3D route to tight sRGB RGBA8 (for `flife render`);
  - `scene_objects()` is 3: the field, the cloud and the density layer.

**Invariants.**
- *Bounded scene objects (§13.2).* The whole population is one scene object, the field another, the density layer a
  third, and a frame one 3D job. There is no mobject or animation handle per organism.
- *Bounded memory.* The draw buffer is reserved once, with fallible reservation, at the population ceiling. No
  update or refit allocates or moves it: `reserved_bytes` and the buffer's address are constant for the cloud's
  life.
- *Observation neutrality.* Everything comes from an immutable epoch. Nothing here reaches the world.
- *Stable identity.* Draw `i` is epoch row `i`, which is identity order (FL-017). A lineage keeps its colour for the
  whole run, and a colour is a function of its own row alone.
- *Observation-neutral styles.* A style changes only what is drawn. Rendering one run in every style gives the same
  world digest.
- *Exact mapping.* `world_to_drawable` and `drawable_to_world` are inverses on the world. FrankenManim's own CPU 3D
  renderer draws each dot where the viewport says.

**Refusal cases.**
- `Viewport::fit` with an extent that is not finite and positive, or a drawable with a zero side:
  `NumericalInvalid`.
- A camera or frame FrankenManim refuses: `NumericalInvalid`, carrying FrankenManim's error.
- An epoch with more rows than the cloud holds: `BudgetExhausted`. An epoch of another world than the viewport's:
  `LayoutMismatch`, from the cloud or the density layer. Either way the previous draws, and what `WorldView` shows,
  are kept.
- `ColorMapping::configured` for a name that is not a registered colour mapping this crate draws:
  `UnknownAlgorithm`. Configuration validation already refuses unregistered names.
- A failed reservation: `AllocationFailed`.
- A `WorldView::resize` that fails leaves the view unchanged.

**Determinism.** Lowering is a pure function of the epoch, the viewport and the style, apart from the field's and
the density layer's decaying display scales, which also read the previous frame's scale: F32 positions widened
exactly to F64, one F64 affine map, fixed ramps and a fixed integer colour hash. The CPU render oracle uses FrankenManim's deterministic CPU 3D
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
- `tests/mapping.rs` (14 tests), on real render epochs where they matter:
  - The energy and generation ramps rise monotonically in CIE L*, from above 15 with a range above 50.
  - Energy maps through the fixed saturating `e / (e + 2)`, generations logarithmically to a fixed 256.
  - The motion hue is the heading (y flipped to screen), its strength `√(speed / limit)`.
  - The lineage and topology palettes are distinct (sRGB distance above 60) and luminous (L* above 45).
  - Mappings parse and name themselves in both spellings.
  - In every mapping, every dot of a real epoch takes its own row's colour.
  - A colour is a function of its row alone: an organism alive in two epochs of one world whose mapped inputs did
    not change keeps its colour bit for bit.
  - The raw look has no halos and a linear grey field; the raw field is linear in the density.
  - Density counts every organism once, in its own bin; the grid wraps without a seam and replaces the dots; under
    raw density the field is not drawn; the density scale restarts when the bins change.
  - Every registered `presentation.color_mapping` entry is implemented and drawn, in order, and a configuration
    naming each one selects it.
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
- In `flife-studio`, `tests/live.rs` restyles a paused real window with keys 2, R, D and F and restores it with 1,
  R, D and F. In `flife-cli`, `tests/render.rs` renders one run in a second style to the same digest, and the
  binary takes its colour from the configuration unless `--color` overrides it.
- Planted bugs in this crate were each caught (FL-018's listed in its commit; FL-019's 14 in its commit).

**Validity domain.** CPU host epochs (FL-017). The FrankenManim 3D camera route at `fmn` rev `f401a947`, CPU and
Metal. The whole torus is fitted into one drawable.

**Known gaps.**
- Lowering is CPU preparation per epoch: FrankenManim's `DotCloud`-equivalent reference route (§13.1). Resident
  device buffers and the shared-device resource bridge are FL-033's (and FL-028's typed Metal buffers).
- There is no chemical-affinity mapping (§13.4 lists one): epochs do not carry affinities.
- There is no on-screen legend: FrankenManim's Metal 3D route has no text, so the meaning is in the Studio's and
  `flife render`'s records.
- Density bins are about 6 px, not one per pixel: the Metal 3D route has no textured surfaces, and a vertex grid per
  pixel would be about four million triangles at 1080p.
- `[presentation] point_radius_pixels`, bloom and trails are not applied yet.
- Human review of the mappings against the plan's reference images (G5) has not been done.
- The view shows the whole torus. Pan, zoom, views that wrap across the seam, interpolation and selection are
  FL-020's.
- Rendering-performance qualification (§13.7: 1080p at 30 fps, frame-time percentiles together with simulation
  slowdown) is G6 work. Nothing here is a performance claim.
