//! FL-019's colour semantics, raw views and density rendering, on real
//! render epochs.

mod common;

use flife_manim::FieldLayer;
use flife_manim::{
    AFFINITY_CHROMA, AFFINITY_GAIN, AFFINITY_LIGHTNESS, BLOOM_SIGMA_POINTS,
    BLOOM_WIDEST_REACH_PIXELS, ColorMapping, DensityLayer, ENERGY_HALF, ENERGY_RAMP,
    GENERATION_RAMP, GENERATION_SCALE, LINEAGE_PALETTE, Look, MOTION_STILL, MOTION_WHEEL,
    ParticleCloud, Particles, Style, TOPOLOGY_PALETTE, Traits, Viewport, WorldView, affinity_rgb8,
    density_bins, density_level, energy_level, generation_level, heading, linear, mapped_color,
    mapped_rgb8, mix, ramp, raw_value, speed_level, traits, wheel,
};
use flife_render::FieldTexture;
use fmn_core::color::{LinearRgba, Srgb, srgb_to_oklab};
use fmn_render::{
    BLOOM_SIGMA_MAX_PIXELS, Bloom, SurfaceMaterial, Texture, TextureWrap, ThreeDDraw,
};

/// Every texel of `texture`, rows top to bottom, unpremultiplied.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn texels(texture: &Texture) -> Vec<LinearRgba> {
    (0..texture.height())
        .flat_map(|row| {
            (0..texture.width()).map(move |column| texture.texel(column, row).expect("a texel"))
        })
        .collect()
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn fit(extent: [f64; 2], drawable: [u32; 2]) -> Viewport {
    Viewport::fit(extent, drawable).expect("a valid viewport")
}

/// CIE L* of an sRGB colour (D65).
fn lightness([r, g, b]: [u8; 3]) -> f64 {
    let channel = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.040_45 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let y = 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
    if y > 216.0 / 24_389.0 {
        116.0 * y.cbrt() - 16.0
    } else {
        y * 24_389.0 / 27.0
    }
}

fn row(lineage: u64, energy: f32, generation: u32, topology: u8, velocity: [f32; 2]) -> Traits {
    Traits {
        lineage,
        energy,
        generation,
        topology,
        velocity,
        affinity: [0.0; 3],
    }
}

/// OKLab of an sRGB byte colour.
fn oklab([r, g, b]: [u8; 3]) -> [f64; 3] {
    let c = srgb_to_oklab(Srgb::from_rgb8(r, g, b));
    [c.l, c.a, c.b]
}

#[test]
fn energy_and_generation_ramps_rise_monotonically_in_lightness() {
    for (name, stops) in [
        ("energy", &ENERGY_RAMP[..]),
        ("generation", &GENERATION_RAMP[..]),
    ] {
        let mut previous = f64::NEG_INFINITY;
        for step in 0..=1000 {
            let current = lightness(ramp(stops, f64::from(step) / 1000.0));
            // Byte rounding may hold L* level between neighbouring samples.
            assert!(
                current >= previous - 0.2,
                "{name}: L* falls at {step}: {previous} -> {current}"
            );
            previous = previous.max(current);
        }
        let ends = (lightness(stops[0]), lightness(stops[stops.len() - 1]));
        assert!(ends.0 > 15.0, "{name}: the dark end shows on a dark canvas");
        assert!(ends.1 - ends.0 > 50.0, "{name}: a wide lightness range");
        for pair in stops.windows(2) {
            assert!(lightness(pair[1]) > lightness(pair[0]), "{name}: {pair:?}");
        }
    }
}

#[test]
fn energy_maps_through_a_fixed_saturating_scale() {
    assert_eq!(energy_level(0.0), 0.0);
    assert_eq!(energy_level(ENERGY_HALF as f32), 0.5);
    assert_eq!(energy_level(-1.0), 0.0);
    assert_eq!(energy_level(f32::NAN), 0.0);
    assert_eq!(energy_level(f32::INFINITY), 0.0);
    let mut previous = -1.0;
    for tenth in 0..=400 {
        let level = energy_level(tenth as f32 / 10.0);
        assert!(level > previous && level < 1.0);
        previous = level;
    }
    let at = |energy: f32| mapped_rgb8(ColorMapping::Energy, &row(1, energy, 0, 0, [0.0; 2]), 4.0);
    assert_eq!(at(0.0), ENERGY_RAMP[0]);
    assert_eq!(
        at(ENERGY_HALF as f32),
        ENERGY_RAMP[4],
        "half way is the middle stop"
    );
    assert!(lightness(at(8.0)) > lightness(at(2.0)));
}

#[test]
fn generations_map_logarithmically_up_to_a_fixed_scale() {
    assert_eq!(generation_level(0), 0.0);
    assert_eq!(generation_level(GENERATION_SCALE), 1.0);
    assert_eq!(generation_level(u32::MAX), 1.0);
    let mut previous = -1.0;
    for generation in 0..=GENERATION_SCALE {
        let level = generation_level(generation);
        assert!(level > previous, "generation {generation}");
        previous = level;
    }
    // Logarithmic: the first fifteen generations take half the scale.
    assert!((generation_level(15) - 0.5).abs() < 1e-3);
    let at = |generation| {
        mapped_rgb8(
            ColorMapping::Generation,
            &row(1, 1.0, generation, 0, [0.0; 2]),
            4.0,
        )
    };
    assert_eq!(at(0), GENERATION_RAMP[0]);
    assert_eq!(
        at(GENERATION_SCALE),
        GENERATION_RAMP[GENERATION_RAMP.len() - 1]
    );
}

#[test]
fn motion_hue_is_the_heading_and_its_strength_the_speed() {
    // World y points down, so screen-up is -y and a quarter turn.
    assert_eq!(heading([1.0, 0.0]), Some(0.0));
    assert_eq!(heading([0.0, -1.0]), Some(0.25));
    assert_eq!(heading([-1.0, 0.0]), Some(0.5));
    assert_eq!(heading([0.0, 1.0]), Some(0.75));
    assert_eq!(heading([0.0, 0.0]), None);
    assert_eq!(heading([f32::NAN, 1.0]), None);
    for (index, &stop) in MOTION_WHEEL.iter().enumerate() {
        assert_eq!(wheel(index as f64 / MOTION_WHEEL.len() as f64), stop);
    }
    assert_eq!(wheel(1.0), wheel(0.0), "the wheel is cyclic");

    let limit = 4.0;
    let motion = |velocity| mapped_rgb8(ColorMapping::Motion, &row(1, 1.0, 0, 0, velocity), limit);
    assert_eq!(motion([0.0, 0.0]), MOTION_STILL);
    assert_eq!(motion([4.0, 0.0]), MOTION_WHEEL[0], "full speed east");
    assert_eq!(motion([0.0, -4.0]), wheel(0.25), "full speed up the screen");
    // A quarter of the limit is half way to full colour (a square root).
    assert_eq!(motion([1.0, 0.0]), mix(MOTION_STILL, MOTION_WHEEL[0], 0.5));
    assert_eq!(motion([40.0, 0.0]), MOTION_WHEEL[0], "clamped at the limit");
    assert_eq!(speed_level([3.0, 4.0], 10.0), 0.5);
    assert_eq!(speed_level([3.0, 4.0], 0.0), 0.0);
    // The same speed in opposite directions is a different hue.
    assert_ne!(motion([4.0, 0.0]), motion([-4.0, 0.0]));
}

#[test]
fn categorical_palettes_are_distinct_and_luminous() {
    for (name, palette) in [
        ("lineage", &LINEAGE_PALETTE[..]),
        ("topology", &TOPOLOGY_PALETTE[..]),
    ] {
        for (index, colour) in palette.iter().enumerate() {
            assert!(
                lightness(*colour) > 45.0,
                "{name} {index} {colour:?} shows on a dark canvas"
            );
            for other in &palette[index + 1..] {
                let distance: i32 = colour
                    .iter()
                    .zip(other)
                    .map(|(&a, &b)| (i32::from(a) - i32::from(b)).abs())
                    .sum();
                assert!(
                    distance > 60,
                    "{name}: {colour:?} and {other:?} are too close"
                );
            }
        }
    }
    for topology in 0..=u8::MAX {
        assert_eq!(
            mapped_rgb8(
                ColorMapping::Topology,
                &row(1, 1.0, 0, topology, [0.0; 2]),
                4.0
            ),
            TOPOLOGY_PALETTE[usize::from(topology) % TOPOLOGY_PALETTE.len()]
        );
    }
}

#[test]
fn mappings_parse_and_name_themselves() {
    for mapping in ColorMapping::ALL {
        assert_eq!(mapping.name().parse::<ColorMapping>(), Ok(mapping));
        assert!(mapping.meaning().contains(mapping.name()));
    }
    assert!("chemistry".parse::<ColorMapping>().is_err());
    assert_eq!("raw".parse::<Look>(), Ok(Look::Raw));
    assert!("bloom".parse::<Look>().is_err());
    assert_eq!(Style::default().mapping, ColorMapping::Lineage);
}

#[test]
fn every_dot_takes_its_own_rows_colour_in_every_mapping() {
    let config = common::small();
    let mut world = common::found(&config);
    common::advance(&mut world, 40);
    let epoch = common::epoch(&world);
    let viewport = fit(epoch.extent(), [256, 256]);
    let mut cloud = ParticleCloud::for_epoch(&epoch).expect("cloud");
    let rows: Vec<Traits> = traits(&epoch).collect();
    assert_eq!(rows.len(), epoch.len());
    for (index, row) in rows.iter().enumerate() {
        assert_eq!(row.lineage, epoch.lineages()[index], "row {index}");
        assert_eq!(row.energy.to_bits(), epoch.energies()[index].to_bits());
        assert_eq!(row.generation, epoch.generations()[index]);
        assert_eq!(row.topology, epoch.topologies()[index]);
        assert_eq!(
            row.velocity.map(f32::to_bits),
            epoch.velocities()[index].map(f32::to_bits)
        );
        assert_eq!(
            row.affinity.map(f32::to_bits),
            epoch.affinities()[index].map(f32::to_bits)
        );
    }
    let mut palettes = Vec::new();
    for mapping in ColorMapping::ALL {
        let style = Style {
            mapping,
            ..Style::default()
        };
        let draws = cloud
            .update_styled(&epoch, &viewport, &style)
            .expect("update")
            .to_vec();
        assert_eq!(draws.len(), epoch.len());
        let mut colours = Vec::new();
        for (index, (draw, traits)) in draws.iter().zip(&rows).enumerate() {
            let ThreeDDraw::TrueDot(dot) = draw else {
                panic!("row {index} is not a dot");
            };
            assert_eq!(
                dot.color,
                mapped_color(mapping, traits, epoch.maximum_speed()),
                "{mapping} row {index}"
            );
            colours.push([dot.color.r, dot.color.g, dot.color.b].map(f64::to_bits));
        }
        // Lowering the same epoch again draws the same colours.
        let again = cloud
            .update_styled(&epoch, &viewport, &style)
            .expect("again")
            .to_vec();
        assert_eq!(
            format!("{again:?}"),
            format!("{draws:?}"),
            "{mapping} is a fixed function of the epoch"
        );
        palettes.push(colours);
    }
    // The mappings show different things.
    for (index, colours) in palettes.iter().enumerate() {
        for other in &palettes[index + 1..] {
            assert_ne!(colours, other);
        }
    }
    // The real rows span more than one value of each mapped quantity.
    let distinct = |values: Vec<u64>| {
        let mut values = values;
        values.sort_unstable();
        values.dedup();
        values.len()
    };
    assert!(distinct(rows.iter().map(|r| r.lineage).collect()) > 4);
    assert!(distinct(rows.iter().map(|r| u64::from(r.energy.to_bits())).collect()) > 4);
    assert!(
        distinct(
            rows.iter()
                .map(|r| u64::from(r.affinity[0].to_bits()))
                .collect()
        ) > 4
    );
    assert!(rows.iter().any(|r| heading(r.velocity).is_some()));
}

#[test]
fn the_raw_look_has_no_halos_and_a_linear_grey_field() {
    let config = common::small();
    let mut world = common::found(&config);
    common::advance(&mut world, 12);
    let epoch = common::epoch(&world);
    let mut view = WorldView::for_epoch(&epoch, [128, 128], [0, 0, 0]).expect("view");
    view.set_style(Style {
        look: Look::Raw,
        ..Style::default()
    });
    view.show(&epoch).expect("show");
    assert!(view.cloud().halos().is_empty());
    assert_eq!(view.cloud().draws().len(), epoch.len());
    let job_draws = view.draws().expect("draws").len();
    assert_eq!(job_draws, 1 + epoch.len(), "the field and one dot each");
    assert!(matches!(view.field().draw(), Some(ThreeDDraw::Surface(_))));
    let scale = view.field().scale();
    assert!(scale > 0.0);
    for colour in texels(view.field().texture().expect("the field's texture")) {
        assert!(colour.r == colour.g && colour.g == colour.b && colour.a == 1.0);
        assert!((0.0..=1.0).contains(&colour.r));
    }
    assert_eq!(raw_value(1.5, 3.0), 0.5);
    assert_eq!(raw_value(6.0, 3.0), 1.0);
    assert_eq!(raw_value(f32::NAN, 3.0), 0.0);
    let rgba = view.render_rgba8(2).expect("a frame");
    assert_eq!(rgba.len(), 128 * 128 * 4);

    // The glow look of the same epoch has a halo per dot.
    view.set_style(Style::default());
    view.show(&epoch).expect("show");
    assert_eq!(view.cloud().halos().len(), epoch.len());
}

#[test]
fn the_glow_look_blooms_as_configured_and_the_raw_look_never_does() {
    // FL-019 (plan §2.3): FrankenManim's controlled bloom, BLOOM_SIGMA_POINTS
    // wide at the screen's pixel density, on glowing frames only. The oracle
    // is FrankenManim's own bloom applied to the frame of the unbloomed job.
    let config = common::small();
    let mut world = common::found(&config);
    common::advance(&mut world, 12);
    let epoch = common::epoch(&world);
    let drawable = [160, 120];
    let pixels = 160 * 120;
    let mut view = WorldView::for_epoch(&epoch, drawable, [2, 3, 8]).expect("view");
    view.show(&epoch).expect("show");
    assert_eq!(view.viewport().pixels_per_point(), 1.0);
    assert!(view.bloom().is_none());
    assert!(view.job().expect("job").bloom().is_none());
    assert_eq!(view.bloom_bytes(drawable), 0);
    let plain = view.job().expect("job").render(2).expect("frame");
    let plain_rgba = view.render_rgba8(2).expect("frame");
    let plain_offline = view.offline_bytes(2);

    view.set_bloom(Some(0.15)).expect("a bloom");
    let bloom = Bloom::new(0.15, BLOOM_SIGMA_POINTS).expect("bloom");
    assert_eq!(view.bloom(), Some(&bloom));
    let job = view.job().expect("job");
    assert_eq!(job.bloom(), Some(&bloom));
    let mut expected = plain.clone();
    bloom.apply(&mut expected, 2).expect("blooms");
    assert_ne!(expected.as_bytes(), plain.as_bytes(), "it shows");
    assert_eq!(
        job.render(3).expect("frame").as_bytes(),
        expected.as_bytes()
    );
    assert_ne!(view.render_rgba8(2).expect("frame"), plain_rgba);
    // The CPU route's row sums are charged.
    assert_eq!(view.bloom_bytes(drawable), pixels * 16);
    assert_eq!(view.offline_bytes(2), plain_offline + pixels * 16);
    // The charge is FrankenManim's ring for its widest bloom, a band and
    // the widest reach above and below it, so it holds at any pixel
    // density and bounds the configured bloom's ring.
    let widest = Bloom::new(0.15, BLOOM_SIGMA_MAX_PIXELS).expect("bloom");
    assert_eq!(widest.radius(), BLOOM_WIDEST_REACH_PIXELS as usize);
    for tall in [
        [160, 120],
        [100, 320],
        [100, 321],
        [3840, 2160],
        [7, 100_000],
    ] {
        let charged = view.bloom_bytes(tall);
        assert_eq!(
            Some(charged),
            widest.scratch_bytes(tall[0], tall[1]),
            "{tall:?}"
        );
        assert!(
            bloom.scratch_bytes(tall[0], tall[1]) <= Some(charged),
            "{tall:?}"
        );
    }
    assert_eq!(view.bloom_bytes([3840, 2160]), 3840 * 320 * 16);

    // Twice the pixel density, twice the width in pixels; FrankenManim's
    // widest at most.
    view.set_pixels_per_point(2.0).expect("density");
    let wide = Bloom::new(0.15, 2.0 * BLOOM_SIGMA_POINTS).expect("bloom");
    assert_eq!(view.bloom(), Some(&wide));
    view.set_pixels_per_point(64.0).expect("density");
    assert_eq!(view.bloom(), Some(&widest));
    view.set_pixels_per_point(1.0).expect("density");
    assert_eq!(view.bloom(), Some(&bloom));

    // The raw look is the diagnostic view: no bloom, though it stays
    // configured and charged. Glowing density blooms.
    view.set_style(Style {
        look: Look::Raw,
        ..Style::default()
    });
    view.show(&epoch).expect("show");
    assert!(view.frame_bloom().is_none());
    assert!(view.job().expect("job").bloom().is_none());
    assert_eq!(view.bloom(), Some(&bloom));
    assert_eq!(view.bloom_bytes(drawable), pixels * 16);
    view.set_style(Style {
        particles: Particles::Density,
        ..Style::default()
    });
    view.show(&epoch).expect("show");
    assert_eq!(view.job().expect("job").bloom(), Some(&bloom));

    // A strength outside the convex mix is refused and changes nothing;
    // none turns it off.
    for strength in [1.5, -0.1, f64::NAN] {
        assert!(view.set_bloom(Some(strength)).is_err(), "{strength}");
        assert_eq!(view.bloom(), Some(&bloom));
    }
    view.set_bloom(None).expect("no bloom");
    assert!(view.bloom().is_none());
    assert!(view.job().expect("job").bloom().is_none());
    assert_eq!(view.bloom_bytes(drawable), 0);
}

#[test]
fn density_counts_every_organism_once_in_its_own_bin() {
    let config = common::small();
    let mut world = common::found(&config);
    common::advance(&mut world, 30);
    let epoch = common::epoch(&world);
    for drawable in [[64, 64], [200, 120], [600, 600]] {
        let viewport = fit(epoch.extent(), drawable);
        let bins = density_bins(&viewport);
        let mut layer = DensityLayer::new(&viewport);
        assert!(layer.draw().is_none());
        layer.update(&epoch, &Style::default()).expect("update");
        assert_eq!(layer.bins(), bins);
        let [columns, rows] = bins.map(|n| n as usize);
        // An independent recount from the positions.
        let mut expected = vec![0_u32; columns * rows];
        let [width, height] = epoch.extent();
        for &[x, y] in epoch.positions() {
            let column =
                ((f64::from(x) / width * columns as f64).floor() as usize).min(columns - 1);
            let row = ((f64::from(y) / height * rows as f64).floor() as usize).min(rows - 1);
            expected[row * columns + column] += 1;
        }
        assert_eq!(layer.counts(), expected.as_slice(), "{drawable:?}");
        assert_eq!(
            layer.counts().iter().map(|&n| n as usize).sum::<usize>(),
            epoch.len()
        );
        let fullest = expected.iter().copied().max().unwrap_or(0);
        assert_eq!(layer.scale(), f64::from(fullest));
        // At most DENSITY_BIN_POINTS per bin, and one bin fewer would be
        // wider.
        let span = epoch.extent()[0] * viewport.scale();
        assert!(span / f64::from(bins[0]) <= 6.0);
        assert!(bins[0] == 1 || span / f64::from(bins[0] - 1) > 6.0);
    }
    assert_eq!(density_level(0, 9.0, Look::Glow), 0.0);
    assert_eq!(density_level(9, 9.0, Look::Glow), 1.0);
    assert_eq!(density_level(3, 9.0, Look::Raw), 1.0 / 3.0);
    assert!(density_level(1, 9.0, Look::Glow) > density_level(1, 9.0, Look::Raw));
    assert_eq!(density_level(3, 0.0, Look::Raw), 0.0);
}

/// FL-033: the density layer is one texture of its bin corners on the
/// viewport's periodic quad, drawn as a repeating image. Texel
/// `(column, row)` is the premultiplied mean of bins `column - 1` and
/// `column` by `row - 1` and `row`, wrapping: its opacity is the mean of
/// their levels, and its colour, recounted here from the epoch, the
/// level-weighted mean of their colours.
#[test]
fn the_density_texture_wraps_without_a_seam_and_draws_in_place_of_the_dots() {
    let config = common::small();
    let mut world = common::found(&config);
    common::advance(&mut world, 30);
    let epoch = common::epoch(&world);
    let mut view = WorldView::for_epoch(&epoch, [192, 192], [0, 0, 0]).expect("view");
    let density = Style {
        particles: Particles::Density,
        ..Style::default()
    };
    view.set_style(density);
    view.show(&epoch).expect("show");
    assert!(view.cloud().draws().is_empty() && view.cloud().halos().is_empty());
    let Some(ThreeDDraw::Surface(layer)) = view.density().draw() else {
        panic!("the density layer draws one surface");
    };
    assert!(!layer.depth_test);
    let SurfaceMaterial::Texture(material) = layer.material else {
        panic!("the density layer is textured");
    };
    // An image, not a surface: a surface would discard clear texels and
    // replace their opacity with the vertices', drawing the bins opaque.
    assert!(material.dark.is_none() && !material.discard_transparent);
    assert_eq!(
        (material.sampler.wrap_u, material.sampler.wrap_v),
        (TextureWrap::Repeat, TextureWrap::Repeat),
        "the texture wraps with the torus"
    );
    let bins = view.density().bins();
    assert_eq!(
        layer.mesh,
        &view.viewport().periodic_quad(bins).expect("a quad"),
        "on the viewport's periodic quad"
    );
    let texture = view.density().texture().expect("the density's texture");
    assert!(std::ptr::eq(material.light, texture));
    assert_eq!((texture.width(), texture.height()), (bins[0], bins[1]));
    let [columns, rows] = bins.map(|n| n as usize);
    let counts = view.density().counts();
    let scale = view.density().scale();
    let level = |column: usize, row: usize| {
        density_level(
            counts[(row % rows) * columns + column % columns],
            scale,
            Look::Glow,
        )
    };
    let shown = texels(texture);
    for row in 0..rows {
        for column in 0..columns {
            let (left, up) = (column + columns - 1, row + rows - 1);
            let mean =
                (level(left, up) + level(column, up) + level(left, row) + level(column, row)) / 4.0;
            let alpha = shown[row * columns + column].a;
            assert!(
                (alpha - mean).abs() <= 1e-6,
                "corner ({column}, {row}): {alpha} != {mean}"
            );
        }
    }
    // Each bin's colour, recounted from the epoch: the mean mapped colour
    // of its organisms.
    let [width, height] = epoch.extent();
    let speed = epoch.maximum_speed();
    let mut sums = vec![[0.0_f64; 3]; columns * rows];
    for (&[x, y], row_traits) in epoch.positions().iter().zip(traits(&epoch)) {
        let column = ((f64::from(x) / width * columns as f64).floor() as usize).min(columns - 1);
        let row = ((f64::from(y) / height * rows as f64).floor() as usize).min(rows - 1);
        let colour = mapped_color(density.mapping, &row_traits, speed);
        let sum = &mut sums[row * columns + column];
        sum[0] += colour.r;
        sum[1] += colour.g;
        sum[2] += colour.b;
    }
    let mut coloured = 0;
    for row in 0..rows {
        for column in 0..columns {
            let (left, up) = ((column + columns - 1) % columns, (row + rows - 1) % rows);
            let mut premultiplied = [0.0_f64; 4];
            for cell in [
                up * columns + left,
                up * columns + column,
                row * columns + left,
                row * columns + column,
            ] {
                if counts[cell] == 0 {
                    continue;
                }
                let level = density_level(counts[cell], scale, Look::Glow);
                for channel in 0..3 {
                    premultiplied[channel] += sums[cell][channel] / f64::from(counts[cell]) * level;
                }
                premultiplied[3] += level;
            }
            if premultiplied[3] == 0.0 {
                continue;
            }
            let texel = shown[row * columns + column];
            for (got, sum) in [texel.r, texel.g, texel.b].into_iter().zip(premultiplied) {
                let want = sum / premultiplied[3];
                assert!(
                    (got - want).abs() <= 1e-5,
                    "corner ({column}, {row}): {got} != {want}"
                );
            }
            coloured += 1;
        }
    }
    assert!(coloured > 0);
    // Glow: translucent over the field, empty corners clear.
    assert!(shown.iter().any(|colour| colour.a == 0.0));
    assert!(shown.iter().any(|colour| colour.a > 0.0 && colour.a < 1.0));
    assert_eq!(view.draws().expect("draws").len(), 2, "field and density");
    let glow = view.render_rgba8(2).expect("a glow density frame");

    // Raw: opaque grey, and it hides the field.
    view.set_style(Style {
        look: Look::Raw,
        ..density
    });
    view.show(&epoch).expect("show");
    assert!(matches!(
        view.density().draw(),
        Some(ThreeDDraw::Surface(_))
    ));
    let counts = view.density().counts();
    let scale = view.density().scale();
    let level = |column: usize, row: usize| {
        density_level(
            counts[(row % rows) * columns + column % columns],
            scale,
            Look::Raw,
        )
    };
    let shown = texels(view.density().texture().expect("the density's texture"));
    for row in 0..rows {
        for column in 0..columns {
            let colour = shown[row * columns + column];
            assert!(colour.r == colour.g && colour.g == colour.b && colour.a == 1.0);
            let (left, up) = (column + columns - 1, row + rows - 1);
            let mean =
                (level(left, up) + level(column, up) + level(left, row) + level(column, row)) / 4.0;
            assert!(
                (colour.r - mean).abs() <= 1e-6,
                "raw grey is the mean level"
            );
        }
    }
    assert_eq!(view.draws().expect("draws").len(), 1, "density alone");
    let raw = view.render_rgba8(2).expect("a raw density frame");
    assert_ne!(glow, raw);

    // Field off: points alone.
    view.set_style(Style {
        field: false,
        ..Style::default()
    });
    view.show(&epoch).expect("show");
    assert_eq!(
        view.draws().expect("draws").len(),
        2 * epoch.len(),
        "halos and dots, no field"
    );
}

#[test]
fn a_pan_or_zoom_draws_the_density_texture_a_fresh_build_draws() {
    // A pan or zoom keeps the density texture built for the look and moves
    // only its quad; a look it was not built for rebuilds it. Either way the
    // layer draws what a layer built from scratch at that view draws.
    let config = common::small();
    let mut world = common::found(&config);
    common::advance(&mut world, 30);
    let epoch = common::epoch(&world);
    let drawn = |view: &WorldView| {
        let density = view.density();
        let Some(ThreeDDraw::Surface(layer)) = density.draw() else {
            panic!("the density layer draws one surface");
        };
        let texture = density.texture().expect("the density's texture");
        let bits: Vec<[u64; 4]> = texels(texture)
            .iter()
            .map(|texel| [texel.r, texel.g, texel.b, texel.a].map(f64::to_bits))
            .collect();
        (
            density.bins(),
            density.scale().to_bits(),
            bits,
            layer.mesh.clone(),
        )
    };
    for (look, other) in [(Look::Glow, Look::Raw), (Look::Raw, Look::Glow)] {
        let style = Style {
            particles: Particles::Density,
            look,
            ..Style::default()
        };
        let mut view = WorldView::for_epoch(&epoch, [192, 160], [0, 0, 0]).expect("view");
        view.set_style(style);
        view.show(&epoch).expect("show");
        let shown = drawn(&view);
        let mut last = shown.3.clone();
        for step in 0..3 {
            let viewport = view.viewport();
            let moved = match step {
                0 => viewport.zoomed_about([70.0, 100.0], 3.0),
                1 => viewport.panned([17.0, -9.0]),
                _ => viewport.zoomed_about([150.0, 20.0], 16.0),
            }
            .expect("moves");
            view.set_view(moved).expect("looks");
            view.place(&epoch).expect("places");
            let mut fresh = WorldView::for_epoch(&epoch, [192, 160], [0, 0, 0]).expect("view");
            fresh.set_style(style);
            fresh.show(&epoch).expect("show");
            fresh.set_view(moved).expect("looks");
            fresh.show(&epoch).expect("show");
            let placed = drawn(&view);
            assert_eq!(placed, drawn(&fresh), "{look:?}, step {step}");
            assert_eq!(
                placed.2, shown.2,
                "{look:?}, step {step}: the texels do not move"
            );
            assert_ne!(placed.3, last, "{look:?}, step {step}: the quad does");
            last = placed.3;
        }
        // The other look, placed without a show, rebuilds the texels for it.
        let restyled = Style {
            look: other,
            ..style
        };
        view.set_style(restyled);
        view.place(&epoch).expect("places");
        let mut fresh = WorldView::for_epoch(&epoch, [192, 160], [0, 0, 0]).expect("view");
        fresh.set_style(restyled);
        fresh.show(&epoch).expect("show");
        fresh.set_view(*view.viewport()).expect("looks");
        fresh.show(&epoch).expect("show");
        let placed = drawn(&view);
        assert_eq!(placed, drawn(&fresh), "{look:?} restyled to {other:?}");
        assert_ne!(placed.2, shown.2, "{other:?} texels are not {look:?}'s");
    }
}

#[test]
fn a_pan_or_zoom_draws_the_field_texture_a_fresh_build_draws() {
    // A pan or zoom of one epoch keeps the field's texture and moves only
    // its quad; a later epoch, though its identity may be the same (each
    // standalone epoch is 1), rebuilds it. Either way the layer draws what a
    // layer built from scratch at that view draws.
    let config = common::small();
    let mut world = common::found(&config);
    common::advance(&mut world, 30);
    let epoch = common::epoch(&world);
    common::advance(&mut world, 5);
    let later = common::epoch(&world);
    assert_ne!(
        epoch.field().values(),
        later.field().values(),
        "the field moved"
    );
    let drawn = |view: &WorldView| {
        let field = view.field();
        let Some(ThreeDDraw::Surface(layer)) = field.draw() else {
            panic!("the field layer draws one surface");
        };
        let texture = field.texture().expect("the field's texture");
        let bits: Vec<[u64; 4]> = texels(texture)
            .iter()
            .map(|texel| [texel.r, texel.g, texel.b, texel.a].map(f64::to_bits))
            .collect();
        (field.scale().to_bits(), bits, layer.mesh.clone())
    };
    let moves = |view: &WorldView, step: usize| {
        let viewport = view.viewport();
        match step % 3 {
            0 => viewport.zoomed_about([70.0, 100.0], 3.0),
            1 => viewport.panned([17.0, -9.0]),
            _ => viewport.zoomed_about([150.0, 20.0], 16.0),
        }
        .expect("moves")
    };
    for look in [Look::Glow, Look::Raw] {
        let style = Style {
            look,
            ..Style::default()
        };
        let mut view = WorldView::for_epoch(&epoch, [192, 160], [0, 0, 0]).expect("view");
        view.set_style(style);
        view.show(&epoch).expect("show");
        let shown = drawn(&view);
        let mut last = shown.2.clone();
        for step in 0..3 {
            let moved = moves(&view, step);
            view.set_view(moved).expect("looks");
            view.place(&epoch).expect("places");
            let mut fresh = WorldView::for_epoch(&epoch, [192, 160], [0, 0, 0]).expect("view");
            fresh.set_style(style);
            fresh.show(&epoch).expect("show");
            fresh.set_view(moved).expect("looks");
            fresh.show(&epoch).expect("show");
            let placed = drawn(&view);
            assert_eq!(placed, drawn(&fresh), "{look:?}, step {step}");
            assert_eq!(
                placed.1, shown.1,
                "{look:?}, step {step}: the texels do not move"
            );
            assert_ne!(placed.2, last, "{look:?}, step {step}: the quad does");
            last = placed.2;
        }
        // The later epoch, then a pan of it.
        view.show(&later).expect("show");
        let moved = moves(&view, 1);
        view.set_view(moved).expect("looks");
        view.place(&later).expect("places");
        let mut fresh = WorldView::for_epoch(&epoch, [192, 160], [0, 0, 0]).expect("view");
        fresh.set_style(style);
        fresh.show(&epoch).expect("show");
        fresh.set_view(moved).expect("looks");
        fresh.show(&later).expect("show");
        let placed = drawn(&view);
        assert_eq!(placed, drawn(&fresh), "{look:?}, the later epoch");
        assert_ne!(placed.1, shown.1, "{look:?}: the later field's texels");
    }
}

#[test]
fn a_colour_is_a_function_of_its_row_alone() {
    // Two epochs of one world: an organism alive in both, whose mapped
    // inputs did not change, keeps its colour bit for bit, whatever the
    // rest of the population did in between.
    let config = common::small();
    let mut world = common::found(&config);
    common::advance(&mut world, 10);
    let early = common::epoch(&world);
    common::advance(&mut world, 30);
    let late = common::epoch(&world);
    let (early_rows, late_rows): (Vec<Traits>, Vec<Traits>) =
        (traits(&early).collect(), traits(&late).collect());
    let survivors: Vec<(usize, usize)> = (0..early.len())
        .filter_map(|row| {
            let id = early.id_at(row)?;
            Some((row, late.row(id)?))
        })
        .collect();
    assert!(
        !survivors.is_empty(),
        "some founders live through both epochs"
    );
    let mut compared = [0_usize; 6];
    for (slot, mapping) in ColorMapping::ALL.into_iter().enumerate() {
        for &(row, later) in &survivors {
            let (a, b) = (early_rows[row], late_rows[later]);
            let same = match mapping {
                ColorMapping::Lineage => a.lineage == b.lineage,
                ColorMapping::Energy => a.energy.to_bits() == b.energy.to_bits(),
                ColorMapping::Generation => a.generation == b.generation,
                ColorMapping::Motion => {
                    a.velocity.map(f32::to_bits) == b.velocity.map(f32::to_bits)
                }
                ColorMapping::Topology => a.topology == b.topology,
                ColorMapping::Affinity => {
                    a.affinity.map(f32::to_bits) == b.affinity.map(f32::to_bits)
                }
            };
            if same {
                compared[slot] += 1;
                assert_eq!(
                    mapped_color(mapping, &a, early.maximum_speed()),
                    mapped_color(mapping, &b, late.maximum_speed()),
                    "{mapping}"
                );
            }
        }
    }
    // Lineage, generation, topology and affinity are fixed for a
    // survivor's life.
    assert_eq!(compared[0], survivors.len());
    assert_eq!(compared[2], survivors.len());
    assert_eq!(compared[4], survivors.len());
    assert_eq!(compared[5], survivors.len());
    // And the colour does change when the mapped input does.
    assert_ne!(
        linear(ENERGY_RAMP[0]),
        mapped_color(ColorMapping::Energy, &row(1, 9.0, 0, 0, [0.0; 2]), 4.0)
    );
}

#[test]
fn the_raw_field_is_linear_in_the_density() {
    // A ramp along x, as in the glow seam test: the raw corner between
    // columns 7 and 0 shows the mean 3.5 at 3.5 / 7 linear light.
    let viewport = fit([8.0, 4.0], [160, 80]);
    let mut layer = FieldLayer::new(&viewport);
    let plane: Vec<f32> = (0..32).map(|cell| (cell % 8) as f32).collect();
    let texture = FieldTexture::from_plane([8, 4], [1.0, 1.0], &plane).expect("a texture");
    layer.update_look(&texture, Look::Raw).expect("update");
    let field = layer.texture().expect("the layer's texture");
    let red = |column| field.texel(column, 1).expect("a texel").r;
    let stored = |value: f64| f64::from(value as f32);
    assert_eq!(red(0), 0.5);
    assert_eq!(red(1), stored(raw_value(0.5, 7.0)));
    assert_eq!(red(4), stored(raw_value(3.5, 7.0)));
    assert!(red(1) < 0.5_f64.powi(2), "linear, not a square root");
}

#[test]
fn the_density_scale_restarts_when_the_bins_change() {
    let config = common::small();
    let mut world = common::found(&config);
    common::advance(&mut world, 30);
    let epoch = common::epoch(&world);
    let coarse = fit(epoch.extent(), [24, 24]);
    let mut layer = DensityLayer::new(&coarse);
    layer.update(&epoch, &Style::default()).expect("coarse");
    let coarse_scale = layer.scale();
    // Finer bins hold fewer organisms each; the coarse scale must not dim
    // them.
    let fine = fit(epoch.extent(), [600, 600]);
    layer.refit(&fine);
    layer.update(&epoch, &Style::default()).expect("fine");
    let fullest = layer.counts().iter().copied().max().unwrap_or(0);
    assert!(f64::from(fullest) < coarse_scale);
    assert_eq!(layer.scale(), f64::from(fullest));
    // The same bins again: the scale follows down slowly instead.
    layer.update(&epoch, &Style::default()).expect("again");
    assert_eq!(layer.scale(), f64::from(fullest));
}

#[test]
fn every_registered_colour_mapping_is_drawn_and_configurable() {
    use flife_core::registry::{REGISTRY, Role, Status};
    let registered: Vec<&str> = REGISTRY
        .iter()
        .filter(|entry| entry.role == Role::ColorMapping)
        .map(|entry| {
            assert!(
                matches!(entry.status, Status::Implemented { .. }),
                "{} is implemented",
                entry.name
            );
            entry.name
        })
        .collect();
    let drawn: Vec<&str> = ColorMapping::ALL
        .iter()
        .map(|mapping| mapping.registry_name())
        .collect();
    assert_eq!(
        registered, drawn,
        "the registry and the mappings agree, in order"
    );
    for mapping in ColorMapping::ALL {
        assert_eq!(
            ColorMapping::from_registry(mapping.registry_name()),
            Some(mapping)
        );
        assert_eq!(mapping.registry_name().parse::<ColorMapping>(), Ok(mapping));
        let config = common::config(&[(
            "presentation",
            "color_mapping",
            &format!("\"{}\"", mapping.registry_name()),
        )]);
        assert_eq!(ColorMapping::configured(&config), Ok(mapping));
    }
    // Appendix A names the lineage mapping.
    assert_eq!(
        ColorMapping::configured(&common::config(&[])),
        Ok(ColorMapping::Lineage)
    );
}

/// FL-019, plan §13.4: chemical affinity is a fixed projection with a
/// recorded matrix (flife-render's `affinity_projection`) and a recorded
/// scaling and colour: no refit, so an organism keeps its colour, grey for
/// no affinity, opposite hues for opposite directions, and every colour
/// visible on the dark canvas.
#[test]
fn affinity_is_a_fixed_projection_with_its_recorded_matrix_and_scale() {
    assert_eq!(AFFINITY_GAIN.to_bits(), (128.0_f64 / 3.0).sqrt().to_bits());
    assert_eq!(AFFINITY_LIGHTNESS, [0.74, 0.10]);
    assert_eq!(AFFINITY_CHROMA, 0.16);
    // No affinity: a neutral grey at OKLab lightness 0.74.
    let grey = affinity_rgb8([0.0; 3]);
    assert_eq!(grey[0], grey[1]);
    assert_eq!(grey[1], grey[2]);
    assert!((oklab(grey)[0] - 0.74).abs() < 0.01, "{grey:?}");
    assert_eq!(affinity_rgb8([f32::NAN, f32::INFINITY, 0.0]), grey);
    // In the gamut, a colour is the recorded formula's OKLab point, up to the
    // byte rounding: gain √(128/3), the radial squash, then lightness
    // 0.74 + 0.10 s₃ and chroma 0.16 (s₁, s₂).
    for q in [
        [0.05_f32, 0.0, 0.0],
        [0.0, -0.05, 0.0],
        [0.0, 0.0, 0.2],
        [-0.04, 0.03, -0.1],
        [0.02, 0.06, 0.05],
    ] {
        let g = q.map(|c| f64::from(c) * (128.0_f64 / 3.0).sqrt());
        let squash = (1.0 + g.iter().map(|c| c * c).sum::<f64>()).sqrt();
        let s = g.map(|c| c / squash);
        let expected = [0.74 + 0.10 * s[2], 0.16 * s[0], 0.16 * s[1]];
        let got = oklab(affinity_rgb8(q));
        for (component, (g, e)) in got.iter().zip(expected).enumerate() {
            assert!(
                (g - e).abs() < 0.006,
                "{q:?} component {component}: {got:?} for {expected:?}"
            );
        }
    }
    // The first two coordinates are the hue's axes, signs and all; the
    // third the lightness.
    for magnitude in [0.05_f32, 0.15, 0.3] {
        let [_, a_plus, _] = oklab(affinity_rgb8([magnitude, 0.0, 0.0]));
        let [_, a_minus, _] = oklab(affinity_rgb8([-magnitude, 0.0, 0.0]));
        let [_, _, b_plus] = oklab(affinity_rgb8([0.0, magnitude, 0.0]));
        let [_, _, b_minus] = oklab(affinity_rgb8([0.0, -magnitude, 0.0]));
        assert!(
            a_plus > 0.01 && a_minus < -0.01,
            "{magnitude}: {a_plus} {a_minus}"
        );
        assert!(
            b_plus > 0.01 && b_minus < -0.01,
            "{magnitude}: {b_plus} {b_minus}"
        );
        let [light, ..] = oklab(affinity_rgb8([0.0, 0.0, magnitude]));
        let [dark, ..] = oklab(affinity_rgb8([0.0, 0.0, -magnitude]));
        assert!(light > 0.75 && dark < 0.73, "{magnitude}: {light} {dark}");
    }
    // Stronger along one direction is more chromatic, and never wraps.
    let chroma = |q: [f32; 3]| {
        let [_, a, b] = oklab(affinity_rgb8(q));
        a.hypot(b)
    };
    let mut previous = 0.0;
    for step in 1..=20 {
        let t = step as f32 / 20.0;
        let c = chroma([0.6 * t, 0.4 * t, 0.0]);
        assert!(c >= previous - 0.003, "step {step}: {c} after {previous}");
        previous = c;
    }
    // Every colour over a grid of the unit ball shows on the dark canvas,
    // and the squash keeps it within the recorded chroma and lightness
    // (gamut clipping only takes chroma away).
    for x in -4_i8..=4 {
        for y in -4_i8..=4 {
            for z in -4_i8..=4 {
                let q = [x, y, z].map(|c| f32::from(c) / 4.0);
                if q.iter().map(|c| c * c).sum::<f32>() <= 1.0 {
                    let rgb = affinity_rgb8(q);
                    assert!(lightness(rgb) > 45.0, "{q:?}");
                    let [l, a, b] = oklab(rgb);
                    assert!(a.hypot(b) <= AFFINITY_CHROMA + 0.012, "{q:?}: {rgb:?}");
                    assert!((0.62..0.86).contains(&l), "{q:?}: {rgb:?} at {l}");
                }
            }
        }
    }
    // The mapping draws the row's own coordinates.
    let mut one = row(1, 1.0, 0, 0, [0.0; 2]);
    one.affinity = [0.2, -0.1, 0.05];
    assert_eq!(
        mapped_rgb8(ColorMapping::Affinity, &one, 4.0),
        affinity_rgb8([0.2, -0.1, 0.05])
    );
    // On the founding epoch the gain puts founders at a radius of about 2,
    // as recorded: their mean squared gained radius is near 4.
    let config = common::small();
    let world = common::found(&config);
    let epoch = common::epoch(&world);
    let rows: Vec<Traits> = traits(&epoch).collect();
    let mean = rows
        .iter()
        .map(|row| {
            row.affinity
                .iter()
                .map(|&c| (f64::from(c) * AFFINITY_GAIN).powi(2))
                .sum::<f64>()
        })
        .sum::<f64>()
        / rows.len() as f64;
    assert!(
        (2.4..5.6).contains(&mean),
        "founders' mean squared radius {mean}"
    );
    // Distinct founders get distinct colours.
    let mut colours: Vec<[u8; 3]> = rows
        .iter()
        .map(|row| mapped_rgb8(ColorMapping::Affinity, row, epoch.maximum_speed()))
        .collect();
    colours.sort_unstable();
    colours.dedup();
    assert!(
        colours.len() * 10 >= rows.len() * 9,
        "{} of {}",
        colours.len(),
        rows.len()
    );
}
