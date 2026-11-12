# flife-manim contract

**Purpose.** FL-018 (`flife-jdf`) covers FrankenLife's FrankenManim bulk world adapter (plan §13.1, §13.2, §13.6).
FL-019 (`flife-g90`) adds the visual mappings, raw views and density rendering (plan §2.3, §13.3 step 5, §13.4).
FL-020 (`flife-err`, part) adds the camera, which pans and zooms across the torus's seams, picking by identity, and
the display between two committed epochs: organisms matched by identity, births at their epoch, deaths fading
(plan §13.5, §13.6, C.15). FL-024 (`flife-xut`, part) adds the metric overlay: lines of text fixed to the
drawable's corner, drawn by the 3D route as triangles (plan §12.2).
The live ecology enters a FrankenManim scene as a few bulk primitives, never as one scene object per organism:

- `Viewport` is the exact map between world units, FrankenManim's scene, drawable pixels and logical window points,
  and the 3D camera whose frame it is. A view (center, zoom) shows one image of the torus, wrapping across the
  seams.
- `pick` finds the organism drawn under the pointer, by stable identity; `selection_ring` marks the selected one.
- `region_outline` outlines the region a brush would act on (FL-022, plan §14.3).
- `ParticleCloud` is §13.2's `ParticleCloudView`: one bounded cloud lowered from a render epoch onto FrankenManim's
  3D camera route.
- `FieldLayer` draws the epoch's field under the organisms as one texture on one quad (FL-033): its energy, its
  chemical composition, or one chosen moment channel (FL-019, plan §13.4's "Local field").
- `DensityLayer` counts organisms per screen bin, for worlds denser than their pixels.
- `Style` holds a frame's drawing choices: the `ColorMapping`, the `Look` (glow or raw), the `Particles` (points or
  density), whether the field is drawn and its `FieldMapping` (energy, composition or a component). `field_drawn()` is whether a
  frame draws the field: when it is on, except under raw density, which is opaque.
- `WorldView` holds all of them, and builds each frame's `ThreeDJob`.

**Inputs.**
- An FL-017 `Epoch`, read only: identity-ordered positions, velocities, energies, lineages, generations,
  topology catalog indices and affinity projections (FL-019), the bounded field texture, the world extent, the speed limit and the population ceiling
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
- `region_outline(viewport, region)` (FL-022, plan §14.3: a preview shows its "affected region, and whether it
  adds/removes energy"). A `Region` is a `RegionShape`, a cell's `Rectangle` or a removal's `Disc` in world units,
  and its `RegionEffect`: a stroke that `Adds` energy, one that `Removes` it, or a measurement region that
  `Measures` (plan §14.3's temporary measurement region, the Studio's measuring tool), changing nothing. The
  outline is a band `REGION_WIDTH_POINTS = 1.5` points wide just outside the region, at any zoom and pixel
  density, so what is inside stays visible. A rectangle's band is four mitred quads (8 triangles); a disc's is an
  annulus of `REGION_SEGMENTS = 48` quads from its edge outward. The quads are wound like the ring's, placed where
  the viewport draws the region's centre, in `REGION_ADDS_COLOR` (amber), `REGION_REMOVES_COLOR` (red) or
  `REGION_MEASURES_COLOR` (cyan, 0.45, 0.9, 1.0), alpha 0.9. An empty, inverted or non-finite rectangle, or a
  radius that is not positive, is refused.
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
  | `affinity` | `affinity-projection-v1` | the epoch's affinity projection `q` (flife-render) | `g = AFFINITY_GAIN · q`, squashed to `s = g / √(1 + ‖g‖²)`; OKLab `L = 0.74 + 0.10 s₃`, `a = 0.16 s₁`, `b = 0.16 s₂` (`AFFINITY_LIGHTNESS`, `AFFINITY_CHROMA`), then FrankenManim's `oklab_to_srgb` and its clamp to RGBA8 |

  `FromStr` takes either name. The affinity colour is grey for no affinity; organisms with like chemistry have
  like colours, and opposite directions have opposite hues. `AFFINITY_GAIN = √(128/3)` puts the founders, whose
  vectors are at the limit, at a mean gained radius of about 2. About 9% of directions leave the sRGB gamut and
  lose chroma there. The square root is correctly rounded and FrankenManim's sRGB transfer uses its own `fmn_dmath::pow`, not the
  host's, so the colours are the same on every host. Colours are for display only: no metric reads them back.
- `ColorMapping::legend(topologies)` is the mapping's key (FL-019): one line of text, the mapping's name, then each
  swatch (`HUD_SWATCH`, a filled square) and what it stands for. `swatches` holds each square's sRGB colour, the
  colour `mapped_rgb8` gives that value:
  - lineage: the 12 `LINEAGE_PALETTE` colours, then `by founder` (a lineage's colour is a hash of its identity, so
    lineages share them);
  - energy: `LEGEND_ENERGIES` 0, 1, 2, 4, 8, 16 on the ramp;
  - generation: `LEGEND_GENERATIONS` 0, 1, 3, 15, 63 and `256+`;
  - motion: east, north, west and south at the speed limit (`LEGEND_HEADINGS`), then the grey `still, greyer is
    slower`;
  - topology: each of `topologies` (the run's catalog members' hidden widths, in catalog order) in its palette
    colour, labelled `64, 32`;
  - affinity: `none`, then six directions 60 degrees apart in the first two coordinates at a founder's typical
    radius (`legend_affinities`), then `like hue, like chemistry`.
- `FieldMapping` (FL-019, plan §13.4's "Local field"): `energy` (the default), `composition` (the fixed
  projection), or `component-C` (`Component(FieldComponent)`, chosen components: moment channel C, 0 to
  `FIELD_COMPONENTS` − 1 = 31), by `name()` and `FromStr` (exactly those names: `component-07` and `component-32`
  are refused). `ALL` is energy, composition, then the 32 components in channel order. `next(component)` is what
  the Studio's C key switches to: energy, composition, `component`, energy. `FieldComponent::new(c)` is `None`
  past 31; `next` and `previous` wrap. `FieldMapping::legend()` is the composition's key, `field composition ■ none
  ■■■■■■ as affinity, darker is less energy`, its swatches the affinity key's colours at full brightness, or a
  component's, `field channel C / energy ■ -1 ■ ■ 0 ■ ■ +1, darker is less energy`, its swatches
  `COMPONENT_PALETTE`; the energy field, which the palette explains, has none.
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
  - `update_mapped(field, look, FieldMapping::Composition)` (FL-019) colours each corner by its chemistry: the four
    texels' compositions (`FieldTexture::composition`) averaged with their densities as weights
    (`corner_composition`: their energy together, as a texel's composition is its block's; none without density),
    through `affinity_rgb8`, the affinity mapping's own colour, then scaled in linear light by what the energy
    shows there (`display_value` for the glow look, `raw_value` for raw). Hue is chemistry and brightness energy:
    an organism at its norm limit shares the hue of the material it converts best, a corner with no energy is
    black, and one with energy but no composition is the affinity mapping's neutral grey. The display scale
    follows the energy as in the energy mapping. `place_mapped` keeps the texture only for bitwise the same values
    and compositions, scale, look and mapping; the layer then also keeps a copy of the compositions (12 bytes a
    texel).
  - `update_mapped(field, look, FieldMapping::Component(c))` (FL-019, chosen components) does the same with channel
    `c` of `FieldTexture::components`: a corner's value is its four texels' steps averaged with their densities as
    weights, over 127 (`corner_component`: in `[−1, 1]`, 0 without density), coloured by `component_rgb8`, the
    diverging `COMPONENT_PALETTE` (ColorBrewer's five-class RdYlBu: blue at −1, pale at 0, red at +1, interpolated
    in sRGB bytes, clamped, 0 for what is not finite), then scaled by what the energy shows. A corner with energy
    but none of the channel is pale; one with no energy is black. `place_mapped` keeps the texture only for bitwise
    the same values and the same channel's steps, scale, look and mapping; the layer then keeps a copy of that
    channel's steps (a byte a texel, within the composition's 12).
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
  - `set_region(region)` (FL-022) outlines a region over the organisms and under the ring until it is set to
    none. It returns whether the region changed, rebuilds the outline for every view change and density, and
    leaves the view as it was when the region is refused; `region()` and `region_draw()`;
  - `draws()` is the frame in painter order: the field (when `Style::field_drawn`, in the style's field mapping),
    then the density
    layer, or every halo and then every dot, then the brush's region, then the selection ring; `job()` wraps it
    in a `ThreeDJob`;
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
    density's textures, their quads, the ring's mesh and a region's at its most (a disc), twice, since one is built
    before the one it replaces is dropped; and once, the copy of the field's values its texture was built from, and of its compositions, 16 bytes
    a texel in all, a component's one-byte copy within them). `texel_bytes()` (FL-033) is the
    layers' textures, 16 bytes a texel: the field's once the view knows it and the density's at the viewport's bins,
    whatever the style; FrankenManim's CPU route samples them in place
    and its Metal route uploads those a frame draws. `preparation_limit()` is `PREPARATION_BYTES_PER_DOT` (1,536)
    for a dot and a halo per organism of the ceiling, `PREPARATION_BYTES_PER_TRIANGLE` (640) for the two quads', the
    ring's and a disc region's triangles, and `PREPARATION_BYTES_FIXED` (1 MiB), whatever the style. `job()` compiles within it
    (FrankenManim's `max_working_bytes`): a tile bin past it is left out, which only slows the frame, and rows past
    it refuse the frame. `set_preparation_threads(threads)` (FL-033) has `job()` compile the frame's dots on up to
    that many threads (FrankenManim's `ThreeDJob::new_with_threads`); one, the default, compiles them on the
    calling thread, and no row, refusal or byte of the accounting depends on it. `frame_bytes()` is the draw list, `JOB_BYTES_PER_DRAW` (1,024) for each draw the view can
    make (FL-033: what FrankenManim's job holds beyond its preparation accounting, measured at 865 to 872 bytes a
    dot and 1,180 a quad), and that limit. `offline_bytes(threads)` is what
    `render_rgba8` adds: the RGBA16F and RGBA8 frames, each thread's tile scratch at the finest sampling, and
    `bloom_bytes` at the drawable. A view
    made `for_epoch` knows its field's texels; one made with `new` learns them from its first epoch.
  - `scene_objects()` is 5: the field, the cloud, the density layer, the brush's region and the selection ring.
- `hud` (FL-024): the 3D route draws triangles, not text.
  - `HudFont::bundled()` loads FrankenManim's bundled font book (`fmn-text`), once.
  - `HudFont::text_with_swatches(text, em_pixels, swatches)` is `text` with each `HUD_SWATCH` in the next of
    `swatches` (linear light), in order, the first `HUD_MAX_SWATCHES` (32); a swatch with no colour is drawn as
    text. `HudText::swatches()` names each coloured square's vertices and colour.
  - `HudFont::text(text, em_pixels)` lays `text` out in IBM Plex Sans, `\n` breaking lines, at an em of
    `em_pixels`. Each glyph's quadratic outline is flattened (`flatten_quadratic`: even steps in the parameter,
    enough that no chord departs from the curve by more than `HUD_FLATNESS_PIXELS` = 0.2 px, at most
    `HUD_MAX_CURVE_SEGMENTS` = 64 a curve). Its contours nest by containment, an even depth ink and an odd one a
    hole, and each ink contour is ear-clipped with its holes (`fmn-geom`'s `triangulate`, an exact cover or a named
    refusal). The result is a `HudText`: vertices in drawable pixels from the block's top-left, y down, each
    contour's range, and triangles.
  - A glyph whose outline cannot be read or triangulated is left out and counted (`refused`). The text stops at
    `HUD_MAX_VERTICES` and `HUD_MAX_TRIANGLES` (68,608 and 65,536, a quarter as much again as the Studio's longest
    overlay, its persistent telemetry (FL-024, plan §14.1), colour key, field key (FL-019), brush preview (FL-022,
    plan §14.3) and inspector panel with what the field offers the organism where it stands, what it sensed, its
    activations, its recent transactions and its descendants and ancestors (FL-021) included, at 2x, rounded up to
    a multiple of 1,024: 54,110 and 52,402; at 3x it is still set whole),
    counting the glyphs beyond them as `left_out`. Its buffers are reserved at those ceilings
    (`HudText::reserved_bytes`).
  - `hud_scene_point(viewport, position)` is the camera's own map from a drawable position to the scene: the frame
    is the whole drawable, centred on the scene's origin, `scale` pixels a scene unit, y up.
  - `hud_plate` is a rectangle `HUD_MARGIN_POINTS` (10) from the drawable's top and left, `HUD_PADDING_POINTS`
    (6) around the text, black at `HUD_PLATE_OPACITY` (0.6). `hud_glyphs` puts the text at `hud_origin`, the
    margin and the padding, in `HUD_TEXT_COLOR` (linear 0.8 grey), each coloured swatch in its colour. Both meshes are clockwise in the scene, as the
    view's others are.
  - `WorldView::enable_overlay()` makes a view draw text. From then on it charges, whether it shows text or not:
    - in `reserved_bytes`, two texts' buffers and two plates and two meshes at the ceilings (one is built before
      the one it replaces is dropped);
    - in `preparation_limit`, the plate's and the ceiling's triangles;
    - in `frame_bytes`, two draws.
  - `set_overlay(Some(text))` shows a text from the next frame and `None` or an empty text shows none; without
    `enable_overlay` it is refused (`StateConflict`). The overlay's plate and glyph draws come last in `draws()`,
    unlit with no depth test. A pan, zoom, resize or new density rebuilds its meshes, so the text keeps its place
    and size on the screen.

**Invariants.**
- *Bounded scene objects (§13.2).* The whole population is one scene object, the field another, the density layer a
  third, the selection ring a fourth, the brush's region a fifth, and a frame one 3D job. There is no mobject or animation handle per organism.
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
- `tests/mapping.rs` (19 tests), on real render epochs where they matter:
  - The energy and generation ramps rise monotonically in CIE L*, from above 15 with a range above 50.
  - Energy maps through the fixed saturating `e / (e + 2)`, generations logarithmically to a fixed 256.
  - The motion hue is the heading (y flipped to screen), its strength `√(speed / limit)`.
  - The lineage and topology palettes are distinct (sRGB distance above 60) and luminous (L* above 45).
  - The affinity colour (FL-019) is its recorded formula: the gain's bits are `√(128/3)`'s, the origin and NaN or
    infinite inputs are grey at L 0.74, five in-gamut points are the formula's OKLab values within 0.006 (computed
    in the test from `√(128/3)`, not the constant), each axis moves only its own OKLab coordinate in its own sign,
    chroma rises with radius, and over a grid of the unit ball every colour is luminous (L* above 45) and within the recorded
    chroma and lightness. On a real founding epoch the mean squared gained radius is 2.4 to 5.6 and at least 90%
    of the founders' colours are distinct.
  - Mappings parse and name themselves in both spellings.
  - In every mapping, every dot of a real epoch takes its own row's colour, and every row's traits are the epoch's
    own views, bit for bit.
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
- `tests/field.rs` (9 tests):
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
  - The composition mapping (FL-019), against corner colours computed in the test: a field of two chemistries and
    energies with a hole and an uncomposed cell. Inside a region a corner is the affinity colour of its axis at
    full brightness; the hole's middle is black; a corner shared by bright and dim texels takes their
    density-weighted composition (0.8 and 0.2, not half each); energy alone is grey everywhere; raw is linear; the
    same energy with other chemistry, then the energy mapping, in either look, are each drawn as given.
  - The field is drawn exactly when `field_drawn()`, never under raw density.
  - A component mapping (FL-019), against corner colours computed in the test: a field wholly along channel 3 on
    the left (density 4), wholly against it on the right (density 1), with a hole and a cell holding only channel
    5. Inside the left a corner is red at full brightness, inside the right blue at half; the hole's middle is
    black; a corner shared by bright and dim texels takes their density-weighted value (0.6, not 0); channel 5
    draws its own colours; raw is linear; channel 3 reversed with the same energy is drawn anew, and channel 5's
    unchanged values as before; energy alone is pale everywhere. The palette's stops, a mix between two, the clamp
    and a non-finite value are checked by hand.
  - A real epoch of the small world holds chemistry and components; the view draws its field in all 34 mappings
    as computed, each component's frame differing from the energy's and the channels' frames not all alike.
  - Field mappings' names parse and print, `next` cycles energy, composition and the chosen component, channels
    wrap, and unknown names (`chemistry`, `component-32`, `component-07`, `component-`, `component`) are refused.
- In `flife-studio`, `tests/live.rs` shows real epochs of the 4,096-organism reference world through this adapter in
  a real window.
- In `flife-studio`, `tests/live.rs` restyles a paused real window with keys 2, R, D and F and restores it with 1,
  R, D and F. In `flife-cli`, `tests/render.rs` renders one run in a second style to the same digest, and the
  binary takes its colour from the configuration unless `--color` overrides it.
- Planted bugs in this crate were each caught (FL-018's listed in its commit; FL-019's 14 in its commit; FL-020's
  camera, picking and world-view mutants, with `flife-render`'s interpolation ones, in its commit). FL-022's
  region outline: 10 of 12 caught (the band inside the region, its width in pixels not points, the colours
  swapped, a disc's band from inside its edge, the outline not rebuilt for a new view, the same region reported
  as a change, the region not drawn or drawn over the ring, the limit's and the frame's region terms left out).
  The outline mirrored top to bottom survives: it is equivalent, since a rectangle and a disc are symmetric about
  their centre and only the winding differs, which FrankenManim draws either way. The reserved bytes without the
  region's mesh survive: no test pins the reservation's total, and the measured heap stays well within it.
- `tests/frame_bytes.rs` (3 tests, FL-043 and FL-033): FL-033's compares a view of 4,096 organisms, every style, at
  the fit and at 8x, with a selection's ring and a removal's disc: the job on 8 preparation threads equals the job on
  one, every draw, bin and count as FrankenManim prints them and the preparation bytes, and in the points style
  the rendered frame bit for bit. FL-043's run over Appendix A worlds at 1,024 and 16,384 organisms (a ceiling of
  16,384) and at a ceiling of 64, every style, 640 x 360 to 3840 x 2160, one and two pixels per point and zooms of
  1, 8 and 64, each with a selection's ring and a removal's disc, 216 frames: FrankenManim's own accounting of an unlimited preparation is within the view's limit
  (the fullest at 75.7%, and 32.6% at the ceiling of 64), so the limit leaves nothing out of them, and the job
  compiled within it holds the same bytes. The limit is the ceiling's dots, the quads', ring's and a disc region's
  triangles and the fixed part exactly, whatever the field or the bins; `texel_bytes` is the field's texture once known and the
  density's at the viewport's bins, each reserved twice, the field's values once, and follows a new pixel density.
  In `flife-cli`, `tests/memory_admission.rs` measures the view's own heap and a CPU frame against these terms, and
  that a pan or zoom allocates no layer texture (at most 440 bytes, against the field's 256 KiB texture); a 4K
  CPU frame with the preset's bloom held 8.08 MiB more than without, its ring of 138 rows at sigma 1.5 (8.09
  MiB), within the 18.75 MiB charged, where the whole frame's row sums took 126.56 MiB;
  and
  in `flife-studio`, `tests/frame_upload.rs` the Metal route's upload.
- `tests/mapping.rs`'s `every_key_swatch_is_the_colour_its_mapping_gives_its_label` (FL-019): each key's line is
  its mapping's name and a label per swatch, and each swatch is the colour `mapped_rgb8`, the dots' path, gives
  the labelled value: each energy and generation (and a later generation for `256+`); each heading at the speed
  limit and `still` at rest, with half the limit nearer the grey; each catalog member's index; the set of colours
  of 10,000 lineages is exactly the 12 swatches; no affinity, and six directions whose OKLab hues turn 60 degrees
  apart (within 10). A catalog longer than the palette repeats its colours.
- `tests/hud.rs` (8 tests, FL-024 and FL-019), each oracle independent of the triangulation:
  - six quadratics, from straight to a hairpin, flattened: the polyline ends on the anchor, a straight curve is one
    segment, and 4,097 samples of each curve lie within 0.2 px of it;
  - all 95 printable ASCII characters and `×·–—…°`, at ems of 13 and 26 px: one glyph each, none refused or left
    out, inside its box. At about 12.8 million sample points on a grid an em wider than the glyph all round (those
    within 1e-9 em of an edge left out), a point is in a triangle exactly when the winding number of the flattened
    contours about it is nonzero, the font's fill rule;
  - a second line is the first moved down 1.2 em exactly, and a space draws nothing; a zero or NaN em is refused;
  - 30 lines of glyphs with holes stop within 10% of the triangle ceiling, and 30 lines of glyphs without within
    10% of the vertex ceiling (each binds first for its kind), with every glyph drawn, refused or left out;
  - at three drawable shapes and four viewports (fit, zoomed over a seam, 2x, zoomed into a corner), the overlay's
    map of the drawable position of a world point is the scene point the view draws it at, to 1e-9;
  - a view refuses text until it draws text, then charges exactly the terms above. Its last two draws are the plate
    (four vertices at 0.6 opacity) and the glyphs, at `hud_origin` through `hud_scene_point`, inside the plate's
    padding. After a zoom the glyphs are at the same drawable pixels, and no text or a blank one draws nothing;
  - a CPU frame of the preset with `Hg8` at an 80 px em: every pixel more than 1 px outside the plate is
    byte-identical to the frame without text; under the plate, each pixel 1.5 px or more from an outline edge is the
    text's colour (sRGB 231, within 1.5) where the winding number is nonzero, and otherwise the world under the
    plate's black at 0.6 opacity, mixed in linear light (within 1.5). Over 500 ink and 2,000 plate pixels are checked;
  - `■a■■` with two colours: the first two squares take them in order and the third is text; the triangles are the
    uncoloured text's; the glyph mesh colours exactly the squares' vertices; in a CPU frame every pixel 2 px inside
    a square is its sRGB colour (within 1.5), over 400 a square.
- `tests/region.rs` (6 tests, FL-022, plan §14.3):
  - A measurement region, a cell's and a disc's, is outlined at the same vertices, bit for bit, and with the same
    triangles as an addition's or a removal's of the same shape, every vertex in its own colour; the three colours
    differ.
  - A cell's outline at fit, at two pixels a point, and zoomed 16: its inner corners are the cell's where the
    viewport draws them, each outer corner 1.5 points out on both axes, and its eight triangles, wound alike,
    cover exactly the band's area. A disc's at fit and zoomed 8: every vertex on its two circles, the radius and
    1.5 points beyond, in the removal's red, its triangles covering the band between the polygons.
  - Empty, inverted and non-finite rectangles and zero, negative, NaN and off-world-infinite discs are refused.
  - The view outlines its region as the last draw, and under the ring when an organism is selected. Setting the
    same region again changes nothing. Through a zoom and a new density the outline is rebuilt, as computed in
    the test. A disc replaces the cell; a refused region leaves it; none removes it.
  - On the CPU route: amber just outside a cell's left edge and red just outside a disc's right edge; inside both,
    and where an old outline was, the frame is the frame without them.
- In `flife-cli`, `tests/memory_admission.rs` measures a 4K view at 2x with the overlay: the Studio's longest
  overlay, the motion key's swatches and the inspector panel included, laid out twice and shown, and two zooms. Its
  peak heap, the texts' layout included, is 12.21 MiB of its 15.08 MiB reserved.

**Validity domain.** CPU host epochs (FL-017). The FrankenManim 3D camera route at `fmn` rev `293d7fcd`, CPU and Metal. A view shows one image of the torus at zoom 1 to
4,096.

**Known gaps.**
- Lowering is CPU preparation per epoch: FrankenManim's `DotCloud`-equivalent reference route (§13.1). The layers'
  textures are rebuilt on the CPU when what they show changes (a show of a new epoch, a restyle; a placement keeps
  the density's, and the field's for bitwise the same values), and both are uploaded with every Metal frame;
  resident device textures and buffers and the shared-device resource bridge are still FL-033's (and FL-028's typed
  Metal buffers).
- The overlay's text is triangles flattened for the em it was laid out at. A host lays it out again for a new pixel
  density (the Studio does). A glyph's edges are the 3D route's triangle edges, so it is not hinted or subpixel
  antialiased. A bloom blurs the text a little, as it does everything drawn before it, and the overlay is not
  excluded from it.
- A colour key shows sample values, not a continuous ramp, and the lineage key cannot say which lineage a colour
  is (12 colours are shared by every lineage). `flife render` draws no overlay or key.
- Density bins are still about 6 px, not one per pixel. Since FL-033 a finer bin costs 16 texel bytes, not two
  triangles, but finer bins are not drawn yet.
- FrankenManim's preparation accounting (`ThreeDJob::preparation_bytes`, which its `max_working_bytes` bounds)
  leaves out about 870 bytes a draw that its CPU job holds, so the limit does not bound the job's heap by itself;
  `frame_bytes` adds `JOB_BYTES_PER_DRAW` from a measurement, not from FrankenManim's accounting.
- Trails are not applied yet. The bloom's width is a constant
  (`BLOOM_SIGMA_POINTS`), not a configuration key; the plan names none. The bloom runs on the raw frame before
  the RGBA8 table: there is no separate tone-mapping step (§13.3 step 8) before or after it.
- Human review of the mappings against the plan's reference images (G5) has not been done, nor of the field's
  composition or its components. The composition is a fixed three-axis projection of 32 channels: two materials
  that differ only off the three axes show the same hue, which a component mapping then tells apart, one channel
  at a time. A component is shown in 127ths of its energy, and one channel at a time: no mapping combines chosen
  channels.
  There are no trails and no camera smoothing (§13.5 allows both; neither is drawn). The validator refuses
  `[presentation] trails_enabled = true` rather than ignoring it. A dot per kept position and organism is not
  affordable on this route: the Studio charges each dot `PREPARATION_BYTES_PER_DOT` three times (in the frame and
  in the host and device copies of its upload) and `JOB_BYTES_PER_DRAW`, 5,632 bytes before its draw rows, so four
  kept positions at the preset's ceiling of 16,384, with 16 bytes each to keep them, come to at least 353 MiB, where
  the torch backend gets 235.9 MiB. Trails wait for resident particle primitives (FL-033). Births appear at the first
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
