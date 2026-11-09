//! FL-019's colour semantics, raw views and density rendering, on real
//! render epochs.

mod common;

use flife_manim::FieldLayer;
use flife_manim::{
    ColorMapping, DensityLayer, ENERGY_HALF, ENERGY_RAMP, GENERATION_RAMP, GENERATION_SCALE,
    LINEAGE_PALETTE, Look, MOTION_STILL, MOTION_WHEEL, ParticleCloud, Particles, Style,
    TOPOLOGY_PALETTE, Traits, Viewport, WorldView, density_bins, density_level, energy_level,
    generation_level, heading, linear, mapped_color, mapped_rgb8, mix, ramp, raw_value,
    speed_level, traits, wheel,
};
use flife_render::FieldTexture;
use fmn_render::ThreeDDraw;

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
    }
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
    let Some(ThreeDDraw::Surface(field)) = view.field().draw() else {
        panic!("the field draws one surface");
    };
    let scale = view.field().scale();
    assert!(scale > 0.0);
    for vertex in field.mesh.vertices() {
        let colour = vertex.color;
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
        // At most DENSITY_BIN_PIXELS per bin, and one bin fewer would be
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

#[test]
fn the_density_grid_wraps_without_a_seam_and_draws_in_place_of_the_dots() {
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
    let Some(ThreeDDraw::Surface(grid)) = view.density().draw() else {
        panic!("the density layer draws one surface");
    };
    assert!(!grid.depth_test);
    let [columns, rows] = view.density().bins().map(|n| n as usize);
    let vertices = grid.mesh.vertices();
    assert_eq!(vertices.len(), (columns + 1) * (rows + 1));
    let stride = columns + 1;
    for row in 0..=rows {
        assert_eq!(
            vertices[row * stride].color,
            vertices[row * stride + columns].color
        );
    }
    for column in 0..=columns {
        assert_eq!(
            vertices[column].color,
            vertices[rows * stride + column].color
        );
    }
    assert_eq!(vertices[0].point, view.viewport().scene_point([0.0, 0.0]));
    // Glow: translucent over the field, empty bins clear.
    assert!(vertices.iter().any(|vertex| vertex.color.a == 0.0));
    assert!(vertices.iter().any(|vertex| vertex.color.a > 0.0));
    assert_eq!(view.draws().expect("draws").len(), 2, "field and density");
    let glow = view.render_rgba8(2).expect("a glow density frame");

    // Raw: opaque grey, and it hides the field.
    view.set_style(Style {
        look: Look::Raw,
        ..density
    });
    view.show(&epoch).expect("show");
    let Some(ThreeDDraw::Surface(grid)) = view.density().draw() else {
        panic!("the density layer draws one surface");
    };
    for vertex in grid.mesh.vertices() {
        let colour = vertex.color;
        assert!(colour.r == colour.g && colour.g == colour.b && colour.a == 1.0);
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
    let mut compared = [0_usize; 5];
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
    // Lineage, generation and topology are fixed for a survivor's life.
    assert_eq!(compared[0], survivors.len());
    assert_eq!(compared[2], survivors.len());
    assert_eq!(compared[4], survivors.len());
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
    let Some(ThreeDDraw::Surface(draw)) = layer.draw() else {
        panic!("the layer draws one surface");
    };
    let vertices = draw.mesh.vertices();
    assert_eq!(vertices[9].color.r, 0.5);
    assert_eq!(vertices[9 + 1].color.r, raw_value(0.5, 7.0));
    assert_eq!(vertices[9 + 4].color.r, raw_value(3.5, 7.0));
    assert!(
        vertices[9 + 1].color.r < 0.5_f64.powi(2),
        "linear, not a square root"
    );
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
