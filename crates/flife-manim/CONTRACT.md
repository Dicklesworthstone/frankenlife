# flife-manim contract

**Purpose.** FL-018 (`flife-jdf`) covers FrankenLife's FrankenManim bulk world adapter (plan §13.1, §13.2, §13.6).
FL-019 (`flife-g90`) adds the visual mappings, raw views and density rendering (plan §2.3, §13.3 step 5, §13.4).
FL-020 (`flife-err`, part) adds the camera, which pans and zooms across the torus's seams, and picking by identity
(plan §13.5, §13.6, C.15).
The live ecology enters a FrankenManim scene as a few bulk primitives, never as one scene object per organism:

- `Viewport` is the exact map between world units, FrankenManim's scene, drawable pixels and logical window points,
  and the 3D camera whose frame it is. A view (center, zoom) shows one image of the torus, wrapping across the
  seams.
- `pick` finds the organism drawn under the pointer, by stable identity; `selection_ring` marks the selected one.
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
- A camera view: a center (a world point) and a zoom in `[1, MAXIMUM_ZOOM = 4096]`.
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
- Views (FL-020):
  - `with_view(center, zoom)` wraps the center into the world; `scale = fit_scale · zoom`; the window starts at
    `lo = center − (W/2, H/2)`, and `origin`, the drawable position of `lo`, is `((Dw − W·scale) / 2,
    (Dh − H·scale) / 2)`, so the center lands on the drawable's center;
  - `image(p)` is p's offset into the window, in `[0, W] × [0, H]`: `p − lo` where that lies in the closed window,
    otherwise wrapped into it, so every world point has one image and a grid corner at `W` keeps its far-edge image;
  - `world_to_drawable(p) = origin + image(p) · scale`; `scene_point(p) = (image_x − W/2, H/2 − image_y, 0)`;
  - `drawable_to_world` is `None` outside the half-open window, and otherwise `lo + offset`, wrapped into the world;
  - at the fit (center `(W/2, H/2)`, zoom 1) `lo` is zero and every coordinate is the fitted viewport's, bit for
    bit;
  - `zoomed_about(anchor, factor)` clamps the zoom to `[1, 4096]` and keeps the world point under the drawable
    anchor under it; `panned(delta)` moves the center by `−delta / scale`, so the world follows a drag;
    `resized(drawable)` keeps the view in a new drawable;
  - `periodic_grid(cells, buffers, corner)` places a periodic corner grid covering the torus once: the lattice at
    `L · k / n` when the window starts on a cell boundary (always at the fit), otherwise cut at the window's edges,
    `n + 2` lines per cut axis, the edge vertices coloured `(1 − w) · corner(k) + w · corner(k + 1)` (bilinear where
    both axes are cut), so both edges, one torus line, carry the same colours.
- `pick(viewport, drawn, at, radius)` (`PICK_RADIUS_PIXELS = 8`): of the `(identity, world position)` pairs drawn,
  the nearest to the drawable position `at` by drawable distance through the view, within `radius` pixels; the
  lowest identity among equals; `None` for nothing in reach or a non-finite position or radius. It returns an
  identity, which a host validates against the world, never a slot.
- `selection_ring(viewport, position)`: an annulus of `SELECTION_RING_SEGMENTS = 48` quads around the organism's
  scene point, from `SELECTION_RING_GAP_PIXELS = 3` outside its dot to `SELECTION_RING_WIDTH_PIXELS = 2` beyond,
  in drawable pixels at any zoom, in `SELECTION_RING_COLOR` (near-white, alpha 0.95), wound like the grids.
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
  - radius `DOT_RADIUS = 0.35` world units, at least `DOT_RADIUS_MIN_PIXELS = 1.5` and at most
    `DOT_RADIUS_MAX_PIXELS = 24` drawable pixels (FL-020): zoomed in past the cap, dots stop growing on screen, so
    close organisms separate, and a dot stays small against the perspective frame's focal distance (about 1.2 frame
    heights), where its projection is the disk the viewport places;
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
    routes both accept;
  - placed through `periodic_grid`, so a panned view cuts it at the window's edges; `place(field, look)` re-places
    it in a new view at the current display scale, and `refit(viewport)` takes the new view.
  - `update_look(field, Look::Raw)` draws linear grey `raw_value = clamp(value / scale, 0, 1)` instead: no square
    root, no palette.
- `DensityLayer::update(epoch, style)`:
  - bins of about `DENSITY_BIN_PIXELS = 6` drawable pixels at the fit, aligned to the world so the torus wraps
    exactly: `ceil(extent · fit_scale / 6)` per axis, in `1..=DENSITY_BINS_MAX = 512`; the zoom does not change them;
  - each organism counted once, in the bin `floor(position / extent · bins)`;
  - `scale` is the fullest bin, or the previous scale times `SCALE_DECAY` if larger; it restarts when the bin grid
    changes;
  - glow: a bin shows the mean linear colour of the current mapping over its organisms, at opacity
    `ln(1 + n) / ln(1 + scale)`, over the field; an empty bin is transparent;
  - raw: opaque linear grey `n / scale`, and the field is not drawn;
  - one vertex-coloured grid with vertices on bin corners, each the premultiplied mean of the four bins around it,
    wrapping, drawn unshaded without depth test, placed through `periodic_grid`; `place(style)` re-places the last
    binned epoch at the current display scale.
- `WorldView`:
  - `show(epoch)` lowers an epoch in the current style and records it as `shown()`, a `Prior`;
  - `set_style(style)` takes effect at the next `show`; showing the same epoch again redraws it in the new style;
  - `resize(drawable)` refits the viewport, camera, radii and density bins, keeping the view's center and zoom,
    returning whether the size changed;
  - `set_view(viewport)` takes a new view of the same world and drawable (returning whether it changed), and
    `place(epoch)` re-lowers the shown epoch there without moving the field's or density's display scale, so a pan
    or zoom does not age the picture;
  - it keeps the shown epoch's identities and positions (`shown_rows()`, reserved once at the ceiling), so
    `pick_in(viewport, at, radius)` picks among the organisms on screen;
  - `select(organism)` holds at most one organism by identity, only if the shown epoch has it; every `show` or
    `place` revalidates it and drops an identity the new epoch lacks, whatever now occupies its old slot;
    `selected()`, `selected_position()`, and `selection_draw()`, the ring, rebuilt for every view change;
  - `draws()` is the frame in painter order: the field (unless hidden, or under raw density), then the density
    grid, or every halo and then every dot, then the selection ring; `job()` wraps it in a `ThreeDJob`;
  - `render_rgba8(threads)` renders it on FrankenManim's CPU 3D route to tight sRGB RGBA8 (for `flife render`);
  - `scene_objects()` is 4: the field, the cloud, the density layer and the selection ring.

**Invariants.**
- *Bounded scene objects (§13.2).* The whole population is one scene object, the field another, the density layer a
  third, the selection ring a fourth, and a frame one 3D job. There is no mobject or animation handle per organism.
- *Bounded memory.* The draw buffer is reserved once, with fallible reservation, at the population ceiling. No
  update or refit allocates or moves it: `reserved_bytes` and the buffer's address are constant for the cloud's
  life.
- *Observation neutrality.* Everything comes from an immutable epoch. Nothing here reaches the world.
- *Stable identity.* Draw `i` is epoch row `i`, which is identity order (FL-017). A lineage keeps its colour for the
  whole run, and a colour is a function of its own row alone.
- *Observation-neutral styles.* A style changes only what is drawn. Rendering one run in every style gives the same
  world digest.
- *Exact mapping.* `world_to_drawable` and `drawable_to_world` are inverses on the world, through any view, up to
  F64 rounding of the wrap. Every world point has exactly one image in a view's window. FrankenManim's own CPU 3D
  renderer draws each dot where the viewport says, panned and zoomed too.

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
- `with_view` with a non-finite center or a zoom outside `[1, 4096]`; `zoomed_about` with a factor that is not
  finite and positive, or a non-finite anchor; `panned` with a non-finite delta; `periodic_grid` with an empty grid:
  `NumericalInvalid`. `WorldView::set_view` with a viewport of another world or drawable: `LayoutMismatch`, the view
  unchanged.

**Determinism.** Lowering is a pure function of the epoch, the viewport and the style, apart from the field's and
the density layer's decaying display scales, which also read the previous frame's scale: F32 positions widened
exactly to F64, one F64 affine map, fixed ramps and a fixed integer colour hash. The CPU render oracle uses FrankenManim's deterministic CPU 3D
route.

**Independent tests.**
- `tests/viewport.rs` (15 tests):
  - Letterbox fit for wide, tall and exact aspects.
  - World and drawable round trips; the margin and the half-open far edges are not the world.
  - Retina-style logical points through the drawable ratio; empty and NaN logical sizes.
  - Scene centring with y up, and the frame shape for exact, wide and tall drawables.
  - Degenerate extents and drawables are refused.
  - *CPU render oracle:* FrankenManim's CPU `ThreeDJob::render` draws a dot at five world points of a 160 × 90 and
    a 90 × 160 drawable, letterboxed each way. The coverage-weighted centroid of each, decoded from the `Rgba16F`
    frame, is within 0.05 px of `world_to_drawable`. FrankenManim keeps a frame's height and fits its width to
    the pixel aspect, so only the tall drawable tests the frame height through the render.
  - FL-020: the fitted center at zoom 1 is the fit itself (and a resize of it); a panned view draws a point at its
    one image (worked numbers) and maps 200 points back within 1e-9 at zooms 1, 2.5 and 64; with the seam under
    the center, two points 0.2 apart across it are drawn 0.2 · scale apart, not across the drawable; zooming about
    five anchors keeps the world under each and clamps at 1 and 4096; a drag moves a point by the drag, and a drag
    of the whole world returns; a panned letterboxed axis shows the torus once; refusals; the periodic grid is the
    lattice at the fit and, panned, is cut into `n + 2` columns at worked offsets with matching edge colours that
    blend the cut cell's corners `0.75 / 0.25`; and the CPU render oracle at three panned and zoomed views, the dot
    a few pixels across at every zoom.
- `tests/camera.rs` (4 tests, FL-020):
  - A pick through a view centered on the seam is the nearest drawn identity across it; out of reach, none; ties
    to the lowest identity; non-finite inputs; at zoom 64 a pick reaches only the organism under the pointer.
  - On a real epoch, a picked organism is removed and a new one introduced in its cell takes its storage slot: the
    world no longer has the picked identity, and the slot's occupant is a new one.
  - The world view keeps its center and zoom through a resize, takes a new view once (not an equal one) and refuses
    another drawable's; fifty placements for a pan leave both display scales unchanged; the panned density grid is
    cut on both axes, `(n + 2)²` vertices inside the window; at zoom 4096 the dot radius is the 24 px cap.
  - The world view picks and selects by identity on a real epoch; the ring is the last draw, every vertex on its
    two circles (within 1e-9) around the organism, through a pan and zoom too; an identity the epoch lacks selects
    nothing; once the organism is removed and an introduced one takes its slot, the next shown epoch drops the
    selection and the old identity selects nothing; a living selection follows its organism a tick on.
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
- Planted bugs in this crate were each caught (FL-018's listed in its commit; FL-019's 14 in its commit; FL-020's
  camera, picking and world-view mutants, with `flife-render`'s interpolation ones, in its commit).

**Validity domain.** CPU host epochs (FL-017). The FrankenManim 3D camera route at `fmn` rev `2fe8d299` (unchanged since
`f401a947`; that repin moved only frankentorch), CPU and Metal. A view shows one image of the torus at zoom 1 to
4,096.

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
- FL-020's remaining parts: drawing an interpolated epoch and fading the dead in the live view; trails; camera
  smoothing at display cadence; and a drawable at the screen's backing scale (the Studio still draws one pixel per
  point). A selection is validated against the newest shown epoch, a projection of a committed world, not by a
  query to the session's world.
- Density bins do not refine with the zoom (they keep the fit's), so a deep zoom shows wide bins.
- FrankenManim's frame is a perspective one; there is no orthographic camera, so dots are capped on screen rather
  than drawn at world size at deep zoom.
- The field and density grids cover the whole torus and are re-placed on every pan, at a cost linear in their
  vertices.
- Rendering-performance qualification (§13.7: 1080p at 30 fps, frame-time percentiles together with simulation
  slowdown) is G6 work. Nothing here is a performance claim.
