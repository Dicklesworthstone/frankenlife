# flife-manim contract

**Purpose.** FL-018 (`flife-jdf`) covers FrankenLife's FrankenManim bulk world adapter (plan §13.1, §13.2, §13.6).
FL-019 (`flife-g90`) adds the visual mappings, raw views and density rendering (plan §2.3, §13.3 step 5, §13.4).
FL-020 (`flife-err`, part) adds the camera, which pans and zooms across the torus's seams, picking by identity, and
the display between two committed epochs: organisms matched by identity, births at their epoch, deaths fading
(plan §13.5, §13.6, C.15).
The live ecology enters a FrankenManim scene as a few bulk primitives, never as one scene object per organism:

- `Viewport` is the exact map between world units, FrankenManim's scene, drawable pixels and logical window points,
  and the 3D camera whose frame it is. A view (center, zoom) shows one image of the torus, wrapping across the
  seams.
- `pick` finds the organism drawn under the pointer, by stable identity; `selection_ring` marks the selected one.
- `ParticleCloud` is §13.2's `ParticleCloudView`: one bounded cloud lowered from a render epoch onto FrankenManim's
  3D camera route.
- `FieldLayer` draws the epoch's energy field under the organisms as one texture on one quad (FL-033).
- `DensityLayer` counts organisms per screen bin, for worlds denser than their pixels.
- `Style` holds a frame's drawing choices: the `ColorMapping`, the `Look` (glow or raw), the `Particles` (points or
  density) and whether the field is drawn.
- `WorldView` holds all of them, and builds each frame's `ThreeDJob`.

**Inputs.**
- An FL-017 `Epoch`, read only: identity-ordered positions, velocities, energies, lineages, generations and
  topology catalog indices, the bounded field texture, the world extent, the speed limit and the population ceiling
  (`Epoch::capacity`).
- A drawable size in pixels, and its pixels per point (1 unless set; the Studio sets the screen's backing scale). A
  logical window size in points, for pointer conversion.
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
    `resized(drawable)` keeps the view in a new drawable; `refit()` is the whole torus again, centered at zoom 1, in
    the same drawable;
  - a viewport carries its drawable's pixels per point (`fit` gives 1). It changes no mapping, and every view above
    keeps it. `with_pixels_per_point(d)` takes a finite positive density; sizes given in points below are that many
    drawable pixels times it;
  - `periodic_quad(cells)` (FL-033) is the quad that draws a periodic texture of `cells` texels over the torus
    once: the window's image, `(±W/2, ±H/2)`, two triangles of four vertices facing the camera at full opacity,
    with texture coordinates `lo / L + ½ / n` at the window's start and one more at its end on each axis (rows
    downward). A texel is sampled at its centre, so texel `k` lands exactly on the torus corner `L · k / n`
    wherever the window puts its image; the texture repeats, so a window that cuts a cell shows the cut cell's
    corners blended bilinearly on both of its edges, one torus line.
- `pick(viewport, drawn, at, radius)` (`PICK_RADIUS_PIXELS = 8`): of the `(identity, world position)` pairs drawn,
  the nearest to the drawable position `at` by drawable distance through the view, within `radius` pixels; the
  lowest identity among equals; `None` for nothing in reach or a non-finite position or radius. It returns an
  identity, which a host validates against the world, never a slot.
- `selection_ring(viewport, position)`: an annulus of `SELECTION_RING_SEGMENTS = 48` quads around the organism's
  scene point, from `SELECTION_RING_GAP_POINTS = 3` outside its dot to `SELECTION_RING_WIDTH_POINTS = 2` beyond,
  in points at any zoom, in `SELECTION_RING_COLOR` (near-white, alpha 0.95), wound like the quads.
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
  - radius `DOT_RADIUS = 0.35` world units, at least the cloud's smallest radius on screen and at most
    `DOT_RADIUS_MAX_POINTS = 24` points (`dot_radius_at_least`). The smallest is `DOT_RADIUS_MIN_POINTS = 1.5`
    points unless `set_min_radius_points` sets one (finite, positive, at most 24; FL-019), and the Studio and
    `flife render` set `[presentation] point_radius_pixels`, in points like the window's size (the preset's
    0.8): the fine points of plan §2.3 where a world is too large for its window. Zoomed in past the cap (FL-020), dots stop growing on screen, so
    close organisms separate, and a dot stays small against the perspective frame's focal distance (about 1.2 frame
    heights), where its projection is the disk the viewport places;
  - colour `mapped_color(style.mapping, row, speed limit)`;
  - in the glow look, one halo per dot, in the same order and drawn under every dot: the dot's colour at alpha
    `HALO_ALPHA = 0.42`, radius `dot · halo_factor(energy)` with `halo_factor(e) = 2 + 2.5·e/(e + 2)` (2 to 4.5;
    2 for non-finite or negative energy). The raw look has no halos.
  - with `Particles::Density`, no dots or halos: the density layer draws instead.
  - `refit(viewport)` changes only the radii, halos in proportion, because scene points do not depend on the
    drawable.
- `ParticleCloud::update_between(prior, current, between, fraction, viewport, style)` (FL-020) draws the display
  `fraction` of the way from `prior` to `current`, as an `flife_render::Interpolation` places it:
  - first each organism that died in the gap, at its `prior` position in its `prior` colour, dot and halo alpha
    multiplied by `1 − fraction` (none at fraction 1), in identity order, so the dead fade under the living;
  - then each of `current`'s rows the interpolation shows, in identity order, at its drawn position in its
    `current` colour; a birth is not shown before fraction 1;
  - while the display moves the living and the dying are `prior`'s rows, so the ceiling holds and nothing
    allocates. At opacity 1 a dot is bit for bit what `update_styled` draws.
- `FieldLayer::update(field)`:
  - one texel per field texel corner, the field's own `texels` each way (`texture()`, rows top to bottom,
    premultiplied linear `f32`, opaque, built with FrankenManim's `Texture::from_premultiplied_linear`); texel
    `(x, y)` is the colour of the mean of the four field texels around that corner, wrapping, so the seam joins its
    two sides' colours;
  - the mean maps through `sqrt(value / scale)` and the six-stop `FIELD_PALETTE`; `scale` is the brightest texel,
    or the previous scale times `SCALE_DECAY = 0.98` if larger, so the picture follows the field down slowly;
  - drawn on the viewport's `periodic_quad` as FrankenManim's `SurfaceDraw::image`: unlit, without depth test,
    repeating both ways, which its CPU and Metal 3D routes both sample. FrankenManim interpolates the corners
    bilinearly, of premultiplied colour (Gouraud shading of a vertex grid before FL-033);
  - `place(field, look)` places the field in a new view at the current display scale (`refit(viewport)` takes the
    new view). The texture depends only on the field's values and texels, the scale and the look, so the layer
    keeps a copy of the values it was built from (4 bytes a texel): a placement of bitwise the same values and
    texels at the same scale in the same look keeps the texture and rebuilds only the quad (FL-033), and any other
    rebuilds both. Epoch identities, unique only within a feed, are not the key.
  - `update_look(field, Look::Raw)` draws linear grey `raw_value = clamp(value / scale, 0, 1)` instead: no square
    root, no palette.
- `DensityLayer::update(epoch, style)`:
  - bins of about `DENSITY_BIN_POINTS = 6` points at the fit, aligned to the world so the torus wraps exactly:
    `ceil(extent · fit_scale / (6 · pixels per point))` per axis, in `1..=DENSITY_BINS_MAX = 512`; the zoom does not change them;
  - each organism counted once, in the bin `floor(position / extent · bins)`;
  - `scale` is the fullest bin, or the previous scale times `SCALE_DECAY` if larger; it restarts when the bin grid
    changes;
  - glow: a bin shows the mean linear colour of the current mapping over its organisms, at opacity
    `ln(1 + n) / ln(1 + scale)`, over the field; an empty bin is transparent;
  - raw: opaque linear grey `n / scale`, and the field is not drawn;
  - one texture of the bins' corners (`texture()`, `bins` texels each way, rows top to bottom, premultiplied
    linear `f32`), each the premultiplied mean of the four bins around it, wrapping, drawn like the field on the
    viewport's `periodic_quad`; `place(style)` places the last binned epoch at the current viewport and display
    scale: a texture built for the style's look is kept, since its texels depend on the binned counts and colours,
    the scale and the look alone, and only the quad moves; for another look the texture is rebuilt.
- `WorldView`:
  - `show(epoch)` lowers an epoch in the current style and records it as `shown()`, a `Prior`;
  - `set_style(style)` takes effect at the next `show`; showing the same epoch again redraws it in the new style;
  - `resize(drawable)` refits the viewport, camera, radii and density bins, keeping the view's center and zoom,
    returning whether the size changed; `set_pixels_per_point(d)` refits the same way for a new density;
  - `set_view(viewport)` takes a new view of the same world and drawable (returning whether it changed), and
    `place(epoch)` re-lowers the shown epoch there without moving the field's or density's display scale, so a pan
    or zoom does not age the picture;
  - `show_between(prior, current, fraction)` (FL-020, §13.5) shows `current` with the display `fraction` of the
    way from `prior`: each organism alive in both where `flife_render::interpolate` places it, by identity (the
    minimum wrapped displacement for an unambiguous gap, snapped to `current` for an ambiguous one), births from
    fraction 1, deaths fading (`ParticleCloud::update_between`). The field and density are `current`'s. It returns
    the `Motion` (the prior `Prior`, the fraction, the `Gap`, the fading count), also `motion()`. At fraction 1,
    without a prior, or for a prior that is not an earlier state of the same world (`Gap::between` refuses it: a
    later or equal tick, another extent, timestep or speed limit), `current` is shown at rest and the motion is
    `None`. `place_between` does the same without moving the display scales; `advance` redraws only the organisms
    and the ring, for the shown epoch only. `show` and `place` are these with no prior;
  - it keeps the shown epoch's identities and where each row is drawn, and whether it is yet (an
    `Interpolation`, reserved once at the ceiling): `shown_rows()` is the drawn rows at their drawn positions, so
    `pick_in(viewport, at, radius)` picks among what is on screen, never the fading dead or the unborn;
  - `select(organism)` holds at most one organism by identity, only if the shown epoch has it drawn; every
    lowering revalidates it and drops an identity the new epoch lacks, whatever now occupies its old slot;
    `selected()`, `selected_position()` (where it is drawn), and `selection_draw()`, the ring, which follows the
    organism as the display moves and is rebuilt for every view change;
  - `draws()` is the frame in painter order: the field (unless hidden, or under raw density), then the density
    layer, or every halo and then every dot, then the selection ring; `job()` wraps it in a `ThreeDJob`;
  - `set_point_radius(points)` (FL-019) sets the dots' smallest radius on screen
    (`ParticleCloud::set_min_radius_points`); the shown dots, their halos and the selection ring change at
    once, and the ring is `selection_ring_around` the view's dot at every refit and reselection;
    `point_radius()` is the one in force;
  - `set_bloom(strength)` (FL-019, §2.3, §13.3 step 7) configures FrankenManim's controlled bloom from
    `[presentation] bloom_enabled` and `bloom_strength` (`None` is off), which the Studio and `flife render`
    pass: a convex mix of the raw frame with a binomial blur of it, `BLOOM_SIGMA_POINTS` (1.5) points wide at
    the viewport's pixel density, up to FrankenManim's widest (32 pixels), rebuilt for a new density.
    `bloom()` is the configured one; `frame_bloom()`, the one `job()` attaches, is it in the glow look and
    none in the raw look, the diagnostic view. Because the mix is convex, a thin dark boundary keeps at
    least `1 - 2 x strength` of its contrast (0.7 at the preset's 0.15), and dense light does not run to
    white. The bits are FrankenManim's, the same on its CPU and Metal routes for the same raw frame;
    `bloom_bytes(drawable)` is the most its row sums hold while a bloom is configured, whatever the look: 16
    bytes a pixel of FrankenManim's ring for its widest bloom (`BLOOM_WIDEST_REACH_PIXELS`, 96 rows above
    and below a band of 128), or of the whole drawable when that is shorter, so a new pixel density needs no
    new admission;
  - `render_rgba8(threads)` renders it on FrankenManim's CPU 3D route to tight sRGB RGBA8 (for `flife render`);
  - FL-043, its bytes: `reserved_bytes()` is the most its own buffers hold (identities, drawn positions, the cloud's
    draws and halos, each reserved at the ceiling; the density bins at the viewport's count; the field's and the
    density's textures, their quads and the ring's mesh, twice, since one is built before the one it replaces is
    dropped; and once, the copy of the field's values its texture was built from). `texel_bytes()` (FL-033) is the
    layers' textures, 16 bytes a texel: the field's once the view knows it and the density's at the viewport's bins,
    whatever the style; FrankenManim's CPU route samples them in place
    and its Metal route uploads those a frame draws. `preparation_limit()` is `PREPARATION_BYTES_PER_DOT` (1,536)
    for a dot and a halo per organism of the ceiling, `PREPARATION_BYTES_PER_TRIANGLE` (640) for the two quads' and
    the ring's triangles, and `PREPARATION_BYTES_FIXED` (1 MiB), whatever the style. `job()` compiles within it
    (FrankenManim's `max_working_bytes`): a tile bin past it is left out, which only slows the frame, and rows past
    it refuse the frame. `frame_bytes()` is the draw list, `JOB_BYTES_PER_DRAW` (1,024) for each draw the view can
    make (FL-033: what FrankenManim's job holds beyond its preparation accounting, measured at 865 to 872 bytes a
    dot and 1,180 a quad), and that limit. `offline_bytes(threads)` is what
    `render_rgba8` adds: the RGBA16F and RGBA8 frames, each thread's tile scratch at the finest sampling, and
    `bloom_bytes` at the drawable. A view
    made `for_epoch` knows its field's texels; one made with `new` learns them from its first epoch.
  - `scene_objects()` is 4: the field, the cloud, the density layer and the selection ring.

**Invariants.**
- *Bounded scene objects (§13.2).* The whole population is one scene object, the field another, the density layer a
  third, the selection ring a fourth, and a frame one 3D job. There is no mobject or animation handle per organism.
- *Bounded memory.* The draw buffer is reserved once, with fallible reservation, at the population ceiling. No
  update or refit allocates or moves it: `reserved_bytes` and the buffer's address are constant for the cloud's
  life.
- *Observation neutrality.* Everything comes from an immutable epoch. Nothing here reaches the world.
- *Stable identity.* At rest, draw `i` is epoch row `i`, which is identity order (FL-017); between epochs, the
  fading dead come first, then the drawn rows in identity order. A lineage keeps its colour for the whole run, and a
  colour is a function of its own row alone. Interpolation pairs an organism only with itself: a reused storage
  slot never continues another organism's path (§13.5).
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
- `show_between`, `place_between` or `advance` with a fraction outside `[0, 1]`: `NumericalInvalid`; with either
  epoch above the ceiling: `BudgetExhausted`; of another world than the viewport's: `LayoutMismatch`. The organisms
  drawn are kept. `advance` for an epoch that is not the shown one: `StateConflict`.
- `ParticleCloud::update_between` with an interpolation that does not describe `current` row for row, or fading
  rows `prior` lacks: `LayoutMismatch`; more dots than the ceiling: `BudgetExhausted`; a fraction outside
  `[0, 1]`: `NumericalInvalid`. The previous draws are kept.
- `ColorMapping::configured` for a name that is not a registered colour mapping this crate draws:
  `UnknownAlgorithm`. Configuration validation already refuses unregistered names.
- A failed reservation: `AllocationFailed`.
- A `WorldView::resize` that fails leaves the view unchanged.
- `with_view` with a non-finite center or a zoom outside `[1, 4096]`; `zoomed_about` with a factor that is not
  finite and positive, or a non-finite anchor; `panned` with a non-finite delta; `periodic_quad` with an empty axis:
  `NumericalInvalid`. A layer texture FrankenManim refuses: `NumericalInvalid`, the previous texture and quad kept. `WorldView::set_view` with a viewport of another world or drawable: `LayoutMismatch`, the view
  unchanged. `set_bloom` with a strength outside `[0, 1]` (or NaN): `NumericalInvalid`, the view unchanged;
  configuration validation already bounds `presentation.bloom_strength` to `[0, 1]`. `set_point_radius` (and
  `ParticleCloud::set_min_radius_points`) with a radius not finite and positive, or above
  `DOT_RADIUS_MAX_POINTS`: `NumericalInvalid`, the view unchanged. Configuration validation bounds
  `presentation.point_radius_pixels` below only (positive), so a radius above 24 fails the Studio's and
  `flife render`'s start with that error.

**Determinism.** Lowering is a pure function of the epoch, the viewport and the style, apart from the field's and
the density layer's decaying display scales, which also read the previous frame's scale: F32 positions widened
exactly to F64, one F64 affine map, fixed ramps and a fixed integer colour hash. The CPU render oracle uses FrankenManim's deterministic CPU 3D
route.

**Independent tests.**
- `tests/viewport.rs` (16 tests):
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
    of the whole world returns; a panned letterboxed axis shows the torus once; refusals; and the CPU render oracle
    at three panned and zoomed views, the dot a few pixels across at every zoom.
  - FL-033: the periodic quad is the window at four views (the fit, a cut axis, both axes cut, and a 3x zoom across
    both seams); there, every texel of a numbered texture samples exactly (within 1e-12) on its torus corner, both
    window edges sample alike, and the cut edges blend the cut cell's corners `0.75 / 0.25`. On FrankenManim's CPU
    route, every pixel of the quad at those views is the bilinear blend of the torus corners around the world point
    the viewport maps it to, computed from world coordinates alone (largest difference 9.8e-5, the RGBA16F
    frame's precision; bound 2e-3).
  - FL-020 density: pixels per point changes no mapping at the fit or in a view, survives a view, a zoom, a pan, a
    resize and a refit, and only a finite positive density is taken.
- `tests/camera.rs` (4 tests, FL-020):
  - A pick through a view centered on the seam is the nearest drawn identity across it; out of reach, none; ties
    to the lowest identity; non-finite inputs; at zoom 64 a pick reaches only the organism under the pointer.
  - On a real epoch, a picked organism is removed and a new one introduced in its cell takes its storage slot: the
    world no longer has the picked identity, and the slot's occupant is a new one.
  - The world view keeps its center and zoom through a resize, takes a new view once (not an equal one) and refuses
    another drawable's; fifty placements for a pan leave both display scales unchanged; the panned density layer is
    the view's periodic quad, its corners the window's and its texture coordinates starting between corners on
    both axes; at zoom 4096 the dot radius is the 24 px cap.
  - The world view picks and selects by identity on a real epoch; the ring is the last draw, every vertex on its
    two circles (within 1e-9) around the organism, through a pan and zoom too; an identity the epoch lacks selects
    nothing; once the organism is removed and an introduced one takes its slot, the next shown epoch drops the
    selection and the old identity selects nothing; a living selection follows its organism a tick on.
- `tests/motion.rs` (3 tests, FL-020), on real epochs of one world from one publisher; the oracle for every drawn
  position is `flife_render::interpolate`, which `flife-render`'s tests check against tick-by-tick paths:
  - A gap of four ticks with one organism removed and one introduced: at fractions 0, 0.25, 0.5 and 0.75 the motion
    is reported exactly; every dot is the oracle's (the dead first at their prior positions and colours, alpha
    `1 − fraction`, halos too; then every drawn row at its interpolated position), the dots are the prior's rows,
    the newborn is not drawn, fraction 0 is the prior state, nothing grows; picks and `shown_rows()` read the drawn
    rows; the dead and the unborn select nothing; a continuing organism is picked and selected where it is drawn.
    At fraction 1 the view is at rest and draws bit for bit what showing the later epoch alone draws, and the
    newborn is selectable.
  - `advance` moves the organisms and the selection's position along the oracle without redrawing the field or
    moving its scale, rests at fraction 1, and refuses an epoch that is not shown.
  - A 600-tick gap on a 32-unit world is ambiguous: every drawn organism is at its later position, none on a path.
    A later or equal prior is shown at rest, bit for bit; a prior of another world is refused; fractions -0.1, 1.5
    and NaN are refused with the organisms kept.
- `tests/cloud.rs` (7 tests):
  - Every row of a real 24-tick epoch becomes one dot, in identity order, at its scene point with its lineage
    colour.
  - Six re-lowerings and a resize keep the reserved bytes and the buffer address; one scene object.
  - Ceiling and wrong-world refusals keep what was shown.
  - A resize refits only the radius, and the pixel floor binds when the world is small.
  - The palette spreads 4,096 lineages over all 12 colours, between 250 and 450 each.
  - `#rrggbb` parsing.
  - FL-020 density: a 128-pixel view at 2 pixels per point draws what a 64-pixel view at 1 draws (the floored and the
    capped dots, halos, the selection ring and the density bins); the change refits the shown epoch without a new
    show, an unchanged density is no change, and NaN is refused.
- `tests/mapping.rs` (17 tests), on real render epochs where they matter:
  - The energy and generation ramps rise monotonically in CIE L*, from above 15 with a range above 50.
  - Energy maps through the fixed saturating `e / (e + 2)`, generations logarithmically to a fixed 256.
  - The motion hue is the heading (y flipped to screen), its strength `√(speed / limit)`.
  - The lineage and topology palettes are distinct (sRGB distance above 60) and luminous (L* above 45).
  - Mappings parse and name themselves in both spellings.
  - In every mapping, every dot of a real epoch takes its own row's colour.
  - A colour is a function of its row alone: an organism alive in two epochs of one world whose mapped inputs did
    not change keeps its colour bit for bit.
  - The raw look has no halos and a linear grey field; the raw field is linear in the density.
  - The glow look blooms as configured and the raw look never does (FL-019): a real epoch's job carries the
    bloom at the preset's strength and 1.5 points at one pixel per point, and its frame (three threads) is
    FrankenManim's bloom of the unbloomed job's frame, byte for byte, and differs from it; twice the density
    doubles the width and 64 pixels a point takes FrankenManim's widest; the raw look attaches none while it
    stays configured and charged, glowing density blooms; strengths 1.5, -0.1 and NaN are refused with the
    view unchanged; `None` turns it off; `offline_bytes` grows by exactly 16 bytes a pixel of the 160 x 120
    frame, shorter than the ring. The charge equals FrankenManim's `scratch_bytes` for its widest bloom (radius
    96) at 160 x 120, 100 x 320, 100 x 321, 3840 x 2160 (320 rows) and 7 x 100,000, and bounds the
    configured bloom's.
  - The configured point radius is the dots' smallest on screen (FL-019, `tests/cloud.rs`): at 2 px a world
    unit, 0.8 points draws every dot 0.4 world units across at once, each halo keeping its multiple, the
    selection ring around the 0.4-unit dot (not the default's), after a re-show and through a density
    change; showing again draws what the refit drew; a 2x screen twice as wide draws the same world radius;
    where the world radius binds 0.8 changes nothing and 10 points binds over it; zoomed all the way in the
    cap binds whatever the floor; 0, -0.8, NaN, infinity and 24.5 are refused with the view unchanged.
  - Density counts every organism once, in its own bin; its texture is on the view's periodic quad, every texel's
    opacity the mean level of the four bins around its corner, wrapping (within 1e-6), and it replaces the dots;
    under raw density every texel is the opaque grey of that mean and the field is not drawn; the density scale
    restarts when the bins change.
  - Every registered `presentation.color_mapping` entry is implemented and drawn, in order, and a configuration
    naming each one selects it.
  - A pan or zoom of a density view (a zoom of 3, a pan, a zoom of 16, in both looks) draws what a view built from
    scratch at that viewport draws, bit for bit: the same bins, scale and texels, on a quad that moved. Placed in
    the other look without a show, the texture is rebuilt for it, again equal to a fresh build.
  - The same three moves of a points view, in both looks, draw the field texture a view built from scratch draws,
    bit for bit (scale, texels, quad), the texels still and the quad moving; a later epoch of the world, shown and
    panned, rebuilds it, again equal to a fresh build, though two standalone epochs share identity 1.
- `tests/field.rs` (6 tests):
  - The palette is monotone dark to bright; the display value is a clamped square root, zero for NaN, negatives and
    a zero scale.
  - A ramp field's texture has a texel per corner, each the palette colour of its wrapped mean, so the seam corner
    is 3.5, not the interior 0.5; it is drawn as a repeating, unlit image without depth test on the view's periodic
    quad, and a placement in a panned view moves only the quad, keeping the scale.
  - The display scale rises at once and falls by `SCALE_DECAY` per update.
  - A placement keeps the texture only for bitwise the same values, texels, scale and look: against an oracle of
    corner colours, the same values from another texture at a pan, one texel brightened, the other look, the same
    values on other texels, the same values re-shown as the scale falls (raw, so every lit texel shows it) and a
    16 x 8 field changed past its first 64 values are each drawn as given.
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
- `tests/frame_bytes.rs` (2 tests, FL-043): over Appendix A worlds at 1,024 and 16,384 organisms (a ceiling of
  16,384) and at a ceiling of 64, every style, 640 x 360 to 3840 x 2160, one and two pixels per point and zooms of
  1, 8 and 64, 216 frames: FrankenManim's own accounting of an unlimited preparation is within the view's limit
  (the fullest at 75.6%, and 27.3% at the ceiling of 64), so the limit leaves nothing out of them, and the job
  compiled within it holds the same bytes. The limit is the ceiling's dots, the quads' and ring's triangles and the
  fixed part exactly, whatever the field or the bins; `texel_bytes` is the field's texture once known and the
  density's at the viewport's bins, each reserved twice, the field's values once, and follows a new pixel density.
  In `flife-cli`, `tests/memory_admission.rs` measures the view's own heap and a CPU frame against these terms, and
  that a pan or zoom allocates no layer texture (at most 440 bytes, against the field's 256 KiB texture); a 4K
  CPU frame with the preset's bloom held 8.08 MiB more than without, its ring of 138 rows at sigma 1.5 (8.09
  MiB), within the 18.75 MiB charged, where the whole frame's row sums took 126.56 MiB;
  and
  in `flife-studio`, `tests/frame_upload.rs` the Metal route's upload.

**Validity domain.** CPU host epochs (FL-017). The FrankenManim 3D camera route at `fmn` rev `c6ad9c5b`, CPU and Metal. A view shows one image of the torus at zoom 1 to
4,096.

**Known gaps.**
- Lowering is CPU preparation per epoch: FrankenManim's `DotCloud`-equivalent reference route (§13.1). The layers'
  textures are rebuilt on the CPU when what they show changes (a show of a new epoch, a restyle; a placement keeps
  the density's, and the field's for bitwise the same values), and both are uploaded with every Metal frame;
  resident device textures and buffers and the shared-device resource bridge are still FL-033's (and FL-028's typed
  Metal buffers).
- There is no chemical-affinity mapping (§13.4 lists one): epochs do not carry affinities.
- There is no on-screen legend: FrankenManim's Metal 3D route has no text, so the meaning is in the Studio's and
  `flife render`'s records.
- Density bins are still about 6 px, not one per pixel. Since FL-033 a finer bin costs 16 texel bytes, not two
  triangles, but finer bins are not drawn yet.
- FrankenManim's preparation accounting (`ThreeDJob::preparation_bytes`, which its `max_working_bytes` bounds)
  leaves out about 870 bytes a draw that its CPU job holds, so the limit does not bound the job's heap by itself;
  `frame_bytes` adds `JOB_BYTES_PER_DRAW` from a measurement, not from FrankenManim's accounting.
- Trails are not applied yet. The bloom's width is a constant
  (`BLOOM_SIGMA_POINTS`), not a configuration key; the plan names none. The bloom runs on the raw frame before
  the RGBA8 table: there is no separate tone-mapping step (§13.3 step 8) before or after it.
- Human review of the mappings against the plan's reference images (G5) has not been done.
  There are no trails and no camera smoothing (§13.5 allows both; neither is drawn). Births appear at the first
  epoch that has them, not at their tick within a longer gap: an epoch does not carry birth ticks. The field and
  density show the newer epoch of a pair at once, up to one state ahead of the moving organisms. A selection is
  validated against the newest shown epoch, a projection of a committed world, not by a query to the session's
  world.
- Density bins do not refine with the zoom (they keep the fit's), so a deep zoom shows wide bins.
- FrankenManim's frame is a perspective one; there is no orthographic camera, so dots are capped on screen rather
  than drawn at world size at deep zoom.
- The field's texture covers the whole torus. A pan keeps it by comparing the placed values with a copy of those it
  was built from (64 KiB for the reference world's 128 x 128 texels), not by epoch identity, which is unique only
  within a feed. Its speed is not qualified: on the 4K points view (16,384 organisms, two pixels per point) two
  interleaved runs gave a paired ratio of 0.65 to 0.66 for a pan's `place` at zoom 1 and 16, but at load averages
  of 48 to 78 the block medians of both sides moved together between two levels, with coefficients of variation
  of 8.1% to 15.1%, above the 5% an admissible run needs. A new epoch's show, which compares and copies, was
  unchanged (ratio 1.001, coefficients 1.3% or less). The density's texture, which shows the
  last binned epoch whatever epoch is placed, is kept: a pan of the 4K density view (16,384 organisms, two pixels
  per point) placed in 0.71 ms instead of 2.73, at zoom 1 and 16 alike (one interleaved invocation, ten blocks a
  side, coefficients of variation at most 3.7%). At a deep zoom the Metal route uploads the whole textures, where
  FrankenManim used to cull a grid's off-screen triangles: a 4K density frame at 16x copied 1.95 MB, against 1.45 MB
  before.
- Rendering-performance qualification (§13.7: 1080p at 30 fps, frame-time percentiles together with simulation
  slowdown) is G6 work. Nothing here is a performance claim.
