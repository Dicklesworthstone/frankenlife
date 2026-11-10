//! FL-031: phase 3's sampling on Metal is [`PyramidView::sample`], bit for
//! bit: every value and derivative of every plane, across grids with zero
//! to four coarse levels, square and stretched cells, fields holding zeros,
//! subnormals and both signs of moment, and queries at knots, cell edges and
//! the torus seams, at every level's own scale, between levels and at the
//! coarsest. The errors are the reference's: a scale or position it refuses,
//! a field on another grid, and a field holding a value that is not finite.

use flife_backend_metal::MetalSensors;
use flife_core::{CoreError, ValidatedConfig};
use flife_engine::{Arrangement, NumericalProfile, Stepped, World};
use flife_field::grid::FIELD_PLANES;
use flife_field::{
    BufferLimits, FieldGrid, FieldState, RawSample, SensorBackend, SensorPyramid, SensorQuery,
};

const LIMITS: BufferLimits = BufferLimits::new(1 << 30);
const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

/// `None` off macOS, where `open` must refuse with its typed error.
#[expect(clippy::panic, reason = "test helper")]
fn metal() -> Option<MetalSensors> {
    match MetalSensors::open() {
        Ok(sensors) => Some(sensors),
        Err(CoreError::MissingCapability { .. }) if !cfg!(target_os = "macos") => None,
        Err(error) => panic!("this host should have Metal: {error}"),
    }
}

/// xorshift64*.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in `[0, 1)`.
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1_u64 << 53) as f64
    }

    /// A field value of either sign over many magnitudes, with zeros and
    /// subnormals among them.
    fn value(&mut self, signed: bool) -> f32 {
        let magnitude = match self.next() % 16 {
            0 => 0.0,
            1 => f32::from_bits((self.next() % 0x007F_FFFF) as u32 + 1),
            2 => f32::MIN_POSITIVE * self.unit() as f32,
            3 => (self.unit() * 1.0e-30) as f32,
            4 => (self.unit() * 1.0e6) as f32,
            _ => (self.unit() * 4.0) as f32,
        };
        if signed && self.next().is_multiple_of(2) {
            -magnitude
        } else {
            magnitude
        }
    }
}

#[expect(clippy::expect_used, reason = "test helper")]
fn random_field(grid: FieldGrid, rng: &mut Rng) -> FieldState {
    let mut field = FieldState::zeroed(grid, &LIMITS).expect("field");
    for plane in 0..FIELD_PLANES {
        for value in field.plane_mut(plane).expect("plane") {
            *value = rng.value(plane != 0);
        }
    }
    field
}

fn bits(sample: &RawSample) -> Vec<u64> {
    sample
        .values
        .iter()
        .chain(&sample.dx)
        .chain(&sample.dy)
        .map(|value| value.to_bits())
        .collect()
}

fn below(value: f64) -> f64 {
    f64::from_bits(value.to_bits() - 1)
}

/// Coordinates along one axis of `length` with `spacings` (one per level):
/// every level's first, middle and last knot and the edges beside them, the
/// seams from both sides, and positions a few lengths away.
fn coordinates(length: f64, spacings: &[(f64, u32)]) -> Vec<f64> {
    let mut out = vec![
        0.0,
        -0.0,
        length,
        below(length),
        -1.0e-300,
        -length * 1.5,
        length * 3.25,
        f64::MIN_POSITIVE,
    ];
    for &(spacing, cells) in spacings {
        for index in [0, cells / 2, cells - 1] {
            let edge = f64::from(index) * spacing;
            out.extend([edge, edge + 0.5 * spacing, below(edge + 0.5 * spacing)]);
        }
    }
    out
}

/// The scales of every level, just past them, halfway between them in
/// log space, just short of the next, and `random` log-uniform ones.
#[expect(clippy::expect_used, reason = "test helper")]
fn scales(pyramid: &SensorPyramid, rng: &mut Rng, random: usize) -> Vec<f64> {
    let levels = pyramid.levels();
    let scale = |level| pyramid.level_scale(level).expect("a level");
    let mut out = Vec::new();
    for level in 0..=levels {
        out.push(scale(level));
        if level < levels {
            out.extend([
                f64::from_bits(scale(level).to_bits() + 1),
                scale(level) * std::f64::consts::SQRT_2,
                below(scale(level + 1)),
            ]);
        }
    }
    for _ in 0..random {
        let exponent = f64::from(levels) * rng.unit();
        out.push((scale(0) * exponent.exp2()).min(scale(levels)));
    }
    out
}

#[expect(clippy::expect_used, reason = "test helper")]
fn queries(pyramid: &SensorPyramid, rng: &mut Rng, random: usize) -> Vec<SensorQuery> {
    let grid = *pyramid.grid();
    let spacings = |x: bool| -> Vec<(f64, u32)> {
        (0..=pyramid.levels())
            .map(|level| {
                let geometry = pyramid.level_grid(level).expect("a level");
                if x {
                    (geometry.spacing_x, geometry.cells_x)
                } else {
                    (geometry.spacing_y, geometry.cells_y)
                }
            })
            .collect()
    };
    let xs = coordinates(grid.world_width(), &spacings(true));
    let ys = coordinates(grid.world_height(), &spacings(false));
    let special = scales(pyramid, rng, 0);
    let mut out = Vec::new();
    for (index, &x) in xs.iter().enumerate() {
        let y = ys[index % ys.len()];
        let other = ys[(rng.next() % ys.len() as u64) as usize];
        for &scale in &special {
            out.push(SensorQuery {
                position: [x, y],
                scale,
            });
            out.push(SensorQuery {
                position: [other, x.min(grid.world_height() * 3.0)],
                scale,
            });
        }
    }
    for scale in scales(pyramid, rng, random) {
        let position = [
            (rng.unit() * 5.0 - 2.0) * grid.world_width(),
            (rng.unit() * 5.0 - 2.0) * grid.world_height(),
        ];
        out.push(SensorQuery { position, scale });
    }
    out
}

/// Compare the device's samples of `queries` with the reference's; the
/// number of queries compared.
#[expect(clippy::expect_used, clippy::panic, reason = "test helper")]
fn compare(
    device: &mut MetalSensors,
    name: &str,
    pyramid: &mut SensorPyramid,
    field: &FieldState,
    queries: &[SensorQuery],
) -> usize {
    let mut samples = Vec::new();
    device
        .sample(pyramid, field, queries, &mut samples)
        .unwrap_or_else(|error| panic!("{name}: the device refused: {error}"));
    assert_eq!(samples.len(), queries.len(), "{name}");
    let view = pyramid.build(field).expect("the reference builds");
    for (index, (query, got)) in queries.iter().zip(&samples).enumerate() {
        let expected = view
            .sample(query.position, query.scale)
            .expect("the reference samples");
        assert_eq!(
            bits(got),
            bits(&expected),
            "{name}: query {index} {query:?} differs"
        );
    }
    queries.len()
}

#[test]
fn the_metal_samples_are_the_reference_samples_bit_for_bit() {
    let Some(mut device) = metal() else { return };
    let mut rng = Rng(0x5EED_0F31_A11C_E5ED);
    // (name, cells, world, levels)
    let cases: [(&str, [u32; 2], [f64; 2], u32); 6] = [
        ("no coarse level", [8, 8], [8.0, 8.0], 0),
        ("one coarse level", [16, 16], [16.0, 16.0], 1),
        ("stretched cells", [32, 16], [40.0, 48.0], 3),
        ("narrow torus", [64, 8], [64.0, 8.0], 3),
        ("odd spacing", [48, 48], [17.0, 31.0], 4),
        ("the preset's grid", [128, 128], [128.0, 128.0], 4),
    ];
    let mut compared = 0;
    let mut first = None;
    for (name, cells, world, levels) in cases {
        let grid = FieldGrid::new(cells[0], cells[1], world[0], world[1]).expect("grid");
        let mut pyramid = SensorPyramid::new(grid, levels, &LIMITS).expect("pyramid");
        let field = random_field(grid, &mut rng);
        let queries = queries(&pyramid, &mut rng, 400);
        compared += compare(&mut device, name, &mut pyramid, &field, &queries);
        first.get_or_insert((pyramid, field, queries));
    }
    // The first grid again, on buffers allocated for another: fewer queries,
    // then more than its first capacity.
    let (mut pyramid, field, queries) = first.expect("a case ran");
    compared += compare(&mut device, "again", &mut pyramid, &field, &queries[..3]);
    let mut more = queries.clone();
    more.extend_from_slice(&queries);
    more.extend_from_slice(&queries);
    compared += compare(&mut device, "more", &mut pyramid, &field, &more);
    println!("compared {compared} samples of {FIELD_PLANES} planes");
    assert!(compared > 5_000, "{compared}");
}

#[test]
fn the_metal_samples_of_the_running_preset_are_the_reference_samples() {
    let Some(mut device) = metal() else { return };
    let config = ValidatedConfig::from_toml(PRESET).expect("the preset validates");
    let mut world = World::found(
        &config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("the world founds");
    let mut pyramid = SensorPyramid::from_config(&config, &LIMITS).expect("pyramid");
    let mut rng = Rng(0x0005_EC0F_FEE5);
    let mut compared = 0;
    for tick in 0..60 {
        match world.step().expect("the world steps") {
            Stepped::Published(_) => {}
            other => panic!("unexpected outcome {other:?}"),
        }
        if tick % 20 == 0 {
            let queries = queries(&pyramid, &mut rng, 2_000);
            let field = world.field().clone();
            compared += compare(&mut device, "preset", &mut pyramid, &field, &queries);
        }
    }
    assert!(compared > 0);
}

#[test]
fn the_metal_sampler_refuses_what_the_reference_refuses() {
    let Some(mut device) = metal() else { return };
    let mut rng = Rng(0xBAD5_CA1E);
    let grid = FieldGrid::new(16, 16, 32.0, 32.0).expect("grid");
    let mut pyramid = SensorPyramid::new(grid, 2, &LIMITS).expect("pyramid");
    let field = random_field(grid, &mut rng);
    let finest = pyramid.level_scale(0).expect("level 0");
    let coarsest = pyramid.level_scale(2).expect("level 2");
    let mut samples = vec![];

    // Scales and positions the reference refuses.
    let refused = [
        ([3.0, 4.0], below(finest)),
        ([3.0, 4.0], f64::from_bits(coarsest.to_bits() + 1)),
        ([3.0, 4.0], f64::NAN),
        ([3.0, 4.0], f64::INFINITY),
        ([3.0, 4.0], 0.0),
        ([f64::NAN, 4.0], finest),
        ([3.0, f64::INFINITY], 3.0 * finest),
        ([f64::NEG_INFINITY, 4.0], coarsest),
    ];
    for (position, scale) in refused {
        let view = pyramid.build(&field).expect("the reference builds");
        assert!(
            view.sample(position, scale).is_err(),
            "{position:?} {scale}"
        );
        let fine = SensorQuery {
            position: [1.0, 1.0],
            scale: finest,
        };
        let query = SensorQuery { position, scale };
        assert!(
            matches!(
                device.sample(&pyramid, &field, &[fine, query], &mut samples),
                Err(CoreError::NumericalInvalid { .. })
            ),
            "{position:?} {scale}"
        );
    }

    // A field on another grid.
    let other = FieldState::zeroed(FieldGrid::new(16, 16, 64.0, 32.0).expect("grid"), &LIMITS)
        .expect("field");
    let fine = [SensorQuery {
        position: [1.0, 1.0],
        scale: finest,
    }];
    assert!(pyramid.build(&other).is_err());
    assert!(matches!(
        device.sample(&pyramid, &other, &fine, &mut samples),
        Err(CoreError::LayoutMismatch { .. })
    ));

    // No queries: no samples, and nothing left from an earlier call.
    device
        .sample(&pyramid, &field, &fine, &mut samples)
        .expect("one sample");
    assert_eq!(samples.len(), 1);
    device
        .sample(&pyramid, &field, &[], &mut samples)
        .expect("no queries");
    assert!(samples.is_empty());

    // A value that is not finite: the device refuses exactly the queries the
    // reference refuses, those that read it directly or through a coarse
    // level (softfloat64 makes it a NaN there), and serves the others bit
    // for bit.
    for (plane, bad) in [(0, f32::INFINITY), (7, f32::NAN), (32, f32::NEG_INFINITY)] {
        let mut broken = field.clone();
        broken.plane_mut(plane).expect("plane")[0] = bad;
        let at_cell = SensorQuery {
            position: [1.0, 1.0],
            scale: finest,
        };
        let far = SensorQuery {
            position: [17.0, 17.0],
            scale: finest,
        };
        // The coarsest level's cell above it.
        let coarse = SensorQuery {
            position: [3.0, 3.0],
            scale: coarsest,
        };
        let view = pyramid.build(&broken).expect("the reference builds");
        assert!(view.sample(at_cell.position, at_cell.scale).is_err());
        assert!(view.sample(coarse.position, coarse.scale).is_err());
        assert!(view.sample(far.position, far.scale).is_ok());
        for queries in [vec![at_cell], vec![coarse], vec![far, coarse]] {
            assert!(
                matches!(
                    device.sample(&pyramid, &broken, &queries, &mut samples),
                    Err(CoreError::NumericalInvalid { .. })
                ),
                "plane {plane} {queries:?}"
            );
        }
        assert_eq!(
            compare(&mut device, "far", &mut pyramid, &broken, &[far]),
            1
        );
    }

    // The same without coarse levels.
    let mut flat = SensorPyramid::new(grid, 0, &LIMITS).expect("pyramid");
    let mut broken = field.clone();
    broken.plane_mut(3).expect("plane")[0] = f32::NAN;
    let far = [SensorQuery {
        position: [17.0, 17.0],
        scale: finest,
    }];
    assert_eq!(compare(&mut device, "flat", &mut flat, &broken, &far), 1);
    let at_cell = [SensorQuery {
        position: [1.0, 1.0],
        scale: finest,
    }];
    assert!(
        device
            .sample(&flat, &broken, &at_cell, &mut samples)
            .is_err()
    );

    // The device still matches the reference afterwards.
    let queries = queries(&pyramid, &mut rng, 100);
    compare(
        &mut device,
        "after the errors",
        &mut pyramid,
        &field,
        &queries,
    );
}
