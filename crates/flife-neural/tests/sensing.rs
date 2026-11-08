//! FL-007 evidence (plan §23.1): density-preserving levels,
//! derivative/order fixtures, hidden-vs-output activation and stable
//! decoding (§6.1, §6.3, A.1, Appendix C.10, C.11).
//!
//! The pyramid is checked against a slow direct implementation of the same
//! rules: block averages of base densities, and bilinear interpolation
//! written as a sum of periodic tent functions with right-hand derivatives.
//! Neither shares code with the implementation.

use flife_core::numerics::{sigmoid, tanh};
use flife_core::{CoreError, TimeStep, ValidatedConfig};
use flife_field::pyramid::{PLANES, RawSample};
use flife_field::state::CHANNELS;
use flife_field::{BufferLimits, FieldGrid, FieldState, SensorPyramid};
use flife_neural::actions::{
    CHILD_SHARE, EMISSION_DIRECTION, EMISSION_GATE, EMISSION_SPEND, REPRODUCTION,
};
use flife_neural::sensors::{
    AGE, ENERGY_CHANGE, INTERNAL_ENERGY, VALUES, VELOCITY_X, VELOCITY_Y, X_DERIVATIVES,
    Y_DERIVATIVES,
};
use flife_neural::{
    ACTION_WIDTH, ActionDecoding, Intrinsic, SENSOR_WIDTH, SensorScales, decode, observe,
};

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn appendix_a() -> ValidatedConfig {
    let appendix = &PLAN[PLAN.find("## Appendix A").expect("Appendix A heading")..];
    let start = appendix.find("```toml\n").expect("Appendix A TOML block") + "```toml\n".len();
    let length = appendix[start..].find("```").expect("closing fence");
    ValidatedConfig::from_toml(&appendix[start..start + length]).expect("Appendix A is valid")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn zeroed(cells_x: u32, cells_y: u32, width: f64, height: f64) -> FieldState {
    let grid = FieldGrid::new(cells_x, cells_y, width, height).expect("grid");
    FieldState::zeroed(grid, &BufferLimits::host()).expect("field")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn pyramid(field: &FieldState, levels: u32) -> SensorPyramid {
    SensorPyramid::new(*field.grid(), levels, &BufferLimits::host()).expect("pyramid")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn scales() -> SensorScales {
    SensorScales::from_config(&appendix_a()).expect("Appendix A sensor scales")
}

/// Fill plane `plane` with `value(x, y)`.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn fill(field: &mut FieldState, plane: u32, value: impl Fn(u32, u32) -> f32) {
    let cells_x = field.grid().cells_x();
    let cells = field.plane_mut(plane).expect("plane");
    for (index, slot) in (0_u32..).zip(cells.iter_mut()) {
        *slot = value(index % cells_x, index / cells_x);
    }
}

/// splitmix64, for reproducible fields and positions.
fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn uniform(state: &mut u64) -> f64 {
    (splitmix(state) >> 11) as f64 / (1_u64 << 53) as f64
}

/// A random field: energies in `[0, 10)` and moments within the cone.
fn random_field(field: &mut FieldState, seed: u64) {
    let mut state = seed;
    let width = field.grid().cells_x();
    let energies: Vec<f32> = (0..field.energy().len())
        .map(|_| (10.0 * uniform(&mut state)) as f32)
        .collect();
    let at = |values: &[f32], x: u32, y: u32| values.get((y * width + x) as usize).copied();
    fill(field, 0, |x, y| at(&energies, x, y).unwrap_or(0.0));
    for plane in 1..=CHANNELS as u32 {
        let moments: Vec<f32> = energies
            .iter()
            .map(|&e| (f64::from(e) * (uniform(&mut state) - 0.5) / 3.0) as f32)
            .collect();
        fill(field, plane, |x, y| at(&moments, x, y).unwrap_or(0.0));
    }
}

/// The slow oracle's level density: the block average of base densities.
fn oracle_density(field: &FieldState, level: u32, plane: u32, x: u32, y: u32) -> f64 {
    let grid = field.grid();
    let block = 1_u32 << level;
    let source = field.plane(plane).unwrap_or(&[]);
    let mut sum = 0.0;
    for dy in 0..block {
        for dx in 0..block {
            let cell = grid
                .cell_index(x * block + dx, y * block + dy)
                .unwrap_or(usize::MAX);
            sum += f64::from(source.get(cell).copied().unwrap_or(f32::NAN)) / grid.cell_area();
        }
    }
    sum / f64::from(block * block)
}

/// The tent `max(0, 1 − |d|/h)` and its right-hand derivative in `d`.
fn tent(d: f64, h: f64) -> (f64, f64) {
    let weight = (1.0 - d.abs() / h).max(0.0);
    let slope = if (0.0..h).contains(&d) {
        -1.0 / h
    } else if (-h..0.0).contains(&d) {
        1.0 / h
    } else {
        0.0
    };
    (weight, slope)
}

/// The minimum image of `d` on a circle of length `length`, in
/// `[−L/2, L/2)`.
fn image(d: f64, length: f64) -> f64 {
    if d >= length / 2.0 {
        d - length
    } else if d < -length / 2.0 {
        d + length
    } else {
        d
    }
}

/// The slow oracle's sample of one level: a sum of tent functions over every
/// cell, with right-hand derivatives.
fn oracle_sample(field: &FieldState, level: u32, position: [f64; 2]) -> RawSample {
    let grid = field.grid();
    let block = 1_u32 << level;
    let (cells_x, cells_y) = (grid.cells_x() / block, grid.cells_y() / block);
    let spacing_x = grid.cell_width() * f64::from(block);
    let spacing_y = grid.cell_height() * f64::from(block);
    let mut sample = RawSample {
        values: [0.0; PLANES],
        dx: [0.0; PLANES],
        dy: [0.0; PLANES],
    };
    for y in 0..cells_y {
        let center_y = (f64::from(y) + 0.5) * spacing_y;
        let (wy, sy) = tent(
            image(position[1] - center_y, grid.world_height()),
            spacing_y,
        );
        for x in 0..cells_x {
            let center_x = (f64::from(x) + 0.5) * spacing_x;
            let (wx, sx) = tent(image(position[0] - center_x, grid.world_width()), spacing_x);
            if wx == 0.0 && wy == 0.0 && sx == 0.0 && sy == 0.0 {
                continue;
            }
            for plane in 0..PLANES {
                let d = oracle_density(field, level, plane as u32, x, y);
                sample.values[plane] += wx * wy * d;
                sample.dx[plane] += sx * wy * d;
                sample.dy[plane] += wx * sy * d;
            }
        }
    }
    sample
}

fn close(a: f64, b: f64, tolerance: f64) -> bool {
    (a - b).abs() <= tolerance * (1.0 + a.abs().max(b.abs()))
}

fn assert_samples_close(ours: &RawSample, oracle: &RawSample, tolerance: f64, what: &str) {
    for plane in 0..PLANES {
        assert!(
            close(ours.values[plane], oracle.values[plane], tolerance),
            "{what}: value {plane}: {} vs {}",
            ours.values[plane],
            oracle.values[plane]
        );
        assert!(
            close(ours.dx[plane], oracle.dx[plane], tolerance),
            "{what}: ∂x {plane}: {} vs {}",
            ours.dx[plane],
            oracle.dx[plane]
        );
        assert!(
            close(ours.dy[plane], oracle.dy[plane], tolerance),
            "{what}: ∂y {plane}: {} vs {}",
            ours.dy[plane],
            oracle.dy[plane]
        );
    }
}

const STILL: Intrinsic = Intrinsic {
    energy: 2.0,
    age_seconds: 3.0,
    velocity: [0.5, -0.25],
    energy_change_per_second: 0.125,
};

#[test]
fn c10_constant_density_is_identical_across_scales() {
    // h = 2, e = 8 and m = 4a with a = (½, ½, ½, ½, 0, …): R = 2 and M = a.
    let mut field = zeroed(32, 32, 64.0, 64.0);
    fill(&mut field, 0, |_, _| 8.0);
    for plane in 1..=4 {
        fill(&mut field, plane, |_, _| 2.0);
    }
    let mut storage = pyramid(&field, 3);
    let view = storage.build(&field).expect("build");
    for level in 0..=3 {
        for (x, y) in [(0, 0), (1, 2), (3, 3)] {
            assert_eq!(
                view.density(level, 0, x, y),
                Some(2.0),
                "R at level {level}"
            );
            assert_eq!(
                view.density(level, 1, x, y),
                Some(0.5),
                "M at level {level}"
            );
            assert_eq!(
                view.density(level, 9, x, y),
                Some(0.0),
                "M_8 at level {level}"
            );
        }
    }
    // Averaging the integrated energy and dividing by the coarse area would
    // give 8/16 one level up.
    let coarse = view.pyramid().level_grid(1).expect("level 1");
    let wrong = 8.0 / (coarse.spacing_x * coarse.spacing_y);
    assert_eq!(wrong, 0.5);
    assert_ne!(view.density(1, 0, 0, 0), Some(wrong));

    let scales = scales();
    let mut reference = None;
    for scale in [2.0, 2.5, 3.0, 4.0, 5.656_854_249_492_38, 8.0, 11.3, 16.0] {
        for position in [[0.0, 0.0], [13.37, 51.2], [63.99, 0.01], [31.0, 31.0]] {
            let raw = view.sample(position, scale).expect("sample");
            assert!(
                raw.dx.iter().chain(&raw.dy).all(|&d| d == 0.0),
                "{scale} {position:?}"
            );
            let observation = observe(&raw, &STILL, &scales).expect("observe");
            assert!(observation.gradients.iter().flatten().all(|&g| g == 0.0));
            // Z = a/(2 + R_floor) and q = 2/3.
            let z = 0.5 / (2.0 + 1.0e-6);
            assert_eq!(observation.inputs[VALUES], 2.0 / 3.0);
            assert_eq!(&observation.inputs[1..5], &[z; 4]);
            assert!(observation.inputs[5..PLANES].iter().all(|&v| v == 0.0));
            match reference {
                None => reference = Some(observation),
                Some(first) => assert_eq!(first, observation, "{scale} {position:?}"),
            }
        }
    }
}

#[test]
fn pyramid_matches_a_slow_direct_oracle() {
    // Anisotropic cells (h_x = 1.5, h_y = 2.5), three levels.
    let mut field = zeroed(32, 16, 48.0, 40.0);
    random_field(&mut field, 0x00f1_0007);
    let mut storage = pyramid(&field, 3);
    let view = storage.build(&field).expect("build");
    for level in 0..=3 {
        let geometry = view.pyramid().level_grid(level).expect("level");
        for y in 0..geometry.cells_y {
            for x in 0..geometry.cells_x {
                for plane in [0, 1, 17, 32] {
                    let ours = view.density(level, plane, x, y).expect("density");
                    let oracle = oracle_density(&field, level, plane as u32, x, y);
                    assert!(
                        close(ours, oracle, 1.0e-14),
                        "level {level} ({x}, {y}) plane {plane}"
                    );
                }
            }
        }
    }

    let mut state = 0x5eed_u64;
    for level in 0..=3 {
        let geometry = view.pyramid().level_grid(level).expect("level");
        let mut positions = vec![[0.0, 0.0], [47.999, 39.999]];
        for i in [0, 1, geometry.cells_x - 1] {
            for j in [0, geometry.cells_y - 1] {
                // A knot, a cell edge, and a mixed point.
                let knot = [
                    (f64::from(i) + 0.5) * geometry.spacing_x,
                    (f64::from(j) + 0.5) * geometry.spacing_y,
                ];
                positions.push(knot);
                positions.push([
                    f64::from(i) * geometry.spacing_x,
                    f64::from(j) * geometry.spacing_y,
                ]);
                positions.push([knot[0], f64::from(j) * geometry.spacing_y]);
            }
        }
        for _ in 0..40 {
            positions.push([48.0 * uniform(&mut state), 40.0 * uniform(&mut state)]);
        }
        for position in positions {
            let ours = view.sample_level(level, position).expect("sample");
            let oracle = oracle_sample(&field, level, position);
            assert_samples_close(
                &ours,
                &oracle,
                1.0e-12,
                &format!("level {level} at {position:?}"),
            );
        }
    }

    // Scale blending against the same oracle and `std`'s log₂.
    for _ in 0..60 {
        let scale = 2.5 * 8.0_f64.powf(uniform(&mut state));
        let position = [48.0 * uniform(&mut state), 40.0 * uniform(&mut state)];
        let ratio = (scale / 2.5).log2();
        let level = ratio.floor() as u32;
        let t = ratio - ratio.floor();
        let ours = view.sample(position, scale).expect("sample");
        let lower = oracle_sample(&field, level, position);
        let blended = if level == 3 {
            lower
        } else {
            let upper = oracle_sample(&field, level + 1, position);
            let mix = |a: [f64; PLANES], b: [f64; PLANES]| {
                let mut out = [0.0; PLANES];
                for i in 0..PLANES {
                    out[i] = (1.0 - t) * a[i] + t * b[i];
                }
                out
            };
            RawSample {
                values: mix(lower.values, upper.values),
                dx: mix(lower.dx, upper.dx),
                dy: mix(lower.dy, upper.dy),
            }
        };
        assert_samples_close(
            &ours,
            &blended,
            1.0e-11,
            &format!("scale {scale} at {position:?}"),
        );
    }
}

#[test]
fn knots_take_the_right_hand_interval_and_edges_are_interior() {
    // Unit cells; R(i) = |i − 3| + 1 along x, and the same along y in M_0.
    let mut field = zeroed(8, 8, 8.0, 8.0);
    fill(&mut field, 0, |x, _| (x as f32 - 3.0).abs() + 1.0);
    fill(&mut field, 1, |_, y| (y as f32 - 3.0).abs() + 1.0);
    let mut storage = pyramid(&field, 0);
    let view = storage.build(&field).expect("build");

    // At the knot x = 3.5 the right-hand slope is +1; the left is −1 and
    // their average 0.
    let knot = view.sample_level(0, [3.5, 3.5]).expect("sample");
    assert_eq!((knot.values[0], knot.dx[0]), (1.0, 1.0));
    assert_eq!((knot.values[1], knot.dy[1]), (1.0, 1.0));
    // A geometric cell edge is inside the interval [2, 3]: no kink there.
    let edge = view.sample_level(0, [3.0, 3.0]).expect("sample");
    assert_eq!((edge.values[0], edge.dx[0]), (1.5, -1.0));
    assert_eq!((edge.values[1], edge.dy[1]), (1.5, -1.0));
    // The origin edge wraps to the interval [7, 0].
    let origin = view.sample_level(0, [0.0, 0.0]).expect("sample");
    assert_eq!((origin.values[0], origin.dx[0]), (4.5, -1.0));
    assert_eq!((origin.values[1], origin.dy[1]), (4.5, -1.0));
    // Derivatives along the other axis vanish.
    assert_eq!((knot.dy[0], knot.dx[1]), (0.0, 0.0));

    // A scale exactly at a level uses that level alone.
    let mut field = zeroed(32, 32, 32.0, 32.0);
    random_field(&mut field, 11);
    let mut storage = pyramid(&field, 3);
    let view = storage.build(&field).expect("build");
    for level in 0..=3 {
        let scale = f64::from(1_u32 << level);
        assert_eq!(view.blend(scale).expect("blend"), (level, 0.0));
        assert_eq!(
            view.sample([7.3, 19.1], scale).expect("sample"),
            view.sample_level(level, [7.3, 19.1]).expect("sample")
        );
    }
    let (level, t) = view.blend(2.0_f64.sqrt() * 2.0).expect("blend");
    assert_eq!(level, 1);
    assert!((t - 0.5).abs() <= 2.0 * f64::EPSILON);
}

#[test]
fn observations_follow_the_abi_order_and_formulas() {
    // Anisotropic cells (h_x = 2, h_y = 0.5) and smooth ramps, so that a
    // wrong spacing changes every derivative.
    let mut field = zeroed(16, 8, 32.0, 4.0);
    fill(&mut field, 0, |x, y| 1.0 + 0.25 * x as f32 + 0.5 * y as f32);
    for channel in 0..CHANNELS as u32 {
        let k = channel as f32;
        fill(&mut field, channel + 1, move |x, y| {
            0.01 * (k + 1.0) * (x as f32 - 2.0 * y as f32) + 0.003 * k
        });
    }
    let mut storage = pyramid(&field, 2);
    let view = storage.build(&field).expect("build");
    let scales = scales();
    let intrinsic = Intrinsic {
        energy: 4.0,
        age_seconds: 60.0,
        velocity: [4.0, -2.0],
        energy_change_per_second: 0.5,
    };
    // Interior points of an interval at every level (cell centres sit at
    // (i + 1/2) h 2^l, so x = 9 would be a level-0 knot), with a blended
    // scale.
    for (position, scale) in [([9.4, 1.3], 2.0), ([9.4, 1.3], 3.0), ([20.5, 2.9], 5.5)] {
        let raw = view.sample(position, scale).expect("sample");
        let observation = observe(&raw, &intrinsic, &scales).expect("observe");
        let inputs = &observation.inputs;
        assert_eq!(inputs.len(), SENSOR_WIDTH);
        let (r, rx, ry) = (raw.values[0], raw.dx[0], raw.dy[0]);
        let floor = scales.composition_floor();
        let length = scales.gradient_length();
        // q, ∂q and their saturated inputs.
        assert_eq!(inputs[VALUES], r / (r + 1.0));
        let dq = [rx / ((r + 1.0) * (r + 1.0)), ry / ((r + 1.0) * (r + 1.0))];
        assert!(close(observation.gradients[0][0], dq[0], 1.0e-15));
        assert!(close(observation.gradients[0][1], dq[1], 1.0e-15));
        assert_eq!(
            inputs[X_DERIVATIVES],
            tanh(length * observation.gradients[0][0]).expect("tanh")
        );
        assert_eq!(
            inputs[Y_DERIVATIVES],
            tanh(length * observation.gradients[0][1]).expect("tanh")
        );
        for k in 0..CHANNELS {
            let (m, mx, my) = (raw.values[k + 1], raw.dx[k + 1], raw.dy[k + 1]);
            // The quotient rule in a different algebraic form.
            let denominator = r + floor;
            let quotient = [
                (mx * denominator - m * rx) / (denominator * denominator),
                (my * denominator - m * ry) / (denominator * denominator),
            ];
            assert_eq!(inputs[VALUES + 1 + k], m / denominator, "Z_{k}");
            assert!(
                close(observation.gradients[k + 1][0], quotient[0], 1.0e-12),
                "∂Z_{k}/∂x"
            );
            assert!(
                close(observation.gradients[k + 1][1], quotient[1], 1.0e-12),
                "∂Z_{k}/∂y"
            );
            assert_eq!(
                inputs[X_DERIVATIVES + 1 + k],
                tanh(length * observation.gradients[k + 1][0]).expect("tanh")
            );
            assert_eq!(
                inputs[Y_DERIVATIVES + 1 + k],
                tanh(length * observation.gradients[k + 1][1]).expect("tanh")
            );
        }
        assert_eq!(inputs[INTERNAL_ENERGY], 0.5);
        assert_eq!(inputs[AGE], 0.5);
        assert_eq!(inputs[VELOCITY_X], tanh(1.0).expect("tanh"));
        assert_eq!(inputs[VELOCITY_Y], tanh(-0.5).expect("tanh"));
        assert_eq!(inputs[ENERGY_CHANGE], tanh(0.5).expect("tanh"));

        // Raw derivatives are per world-length: central differences inside
        // the interval agree, and Z's derivative matches its quotient.
        for (axis, delta) in [(0, 0.01), (1, 0.002)] {
            let mut ahead = position;
            let mut behind = position;
            ahead[axis] += delta;
            behind[axis] -= delta;
            let a = view.sample(ahead, scale).expect("sample");
            let b = view.sample(behind, scale).expect("sample");
            let derivative = if axis == 0 { &raw.dx } else { &raw.dy };
            for plane in 0..PLANES {
                let difference = (a.values[plane] - b.values[plane]) / (2.0 * delta);
                assert!(
                    close(derivative[plane], difference, 1.0e-9),
                    "plane {plane} axis {axis}"
                );
            }
            let za = observe(&a, &intrinsic, &scales).expect("observe");
            let zb = observe(&b, &intrinsic, &scales).expect("observe");
            for k in 0..CHANNELS {
                let difference = (za.inputs[1 + k] - zb.inputs[1 + k]) / (2.0 * delta);
                assert!(
                    close(observation.gradients[k + 1][axis], difference, 1.0e-6),
                    "∂Z_{k} axis {axis}: {} vs {difference}",
                    observation.gradients[k + 1][axis]
                );
            }
        }
    }
}

#[test]
fn coarse_composition_is_a_ratio_of_averages() {
    // One 2 × 2 block of very unequal cells with equal moments: the coarse
    // composition is M/(R + R_floor) = 0.5/5, not the mean of the per-cell
    // ratios (0.5 + 0.5/9 + 0.5 + 0.5/9)/4 ≈ 0.278.
    let mut field = zeroed(2, 2, 2.0, 2.0);
    fill(&mut field, 0, |x, _| if x == 0 { 1.0 } else { 9.0 });
    fill(&mut field, 1, |_, _| 0.5);
    let mut storage = pyramid(&field, 1);
    let view = storage.build(&field).expect("build");
    assert_eq!(view.density(1, 0, 0, 0), Some(5.0));
    assert_eq!(view.density(1, 1, 0, 0), Some(0.5));
    let scales = scales();
    let observation = observe(
        &view.sample_level(1, [1.0, 1.0]).expect("sample"),
        &STILL,
        &scales,
    )
    .expect("observe");
    let floor = scales.composition_floor();
    assert_eq!(observation.inputs[1], 0.5 / (5.0 + floor));
    let ratio_average = (0.5 / (1.0 + floor) + 0.5 / (9.0 + floor)) / 2.0;
    assert!((observation.inputs[1] - ratio_average).abs() > 0.1);
}

#[test]
fn grid_aligned_translations_preserve_observations() {
    let mut field = zeroed(32, 32, 32.0, 32.0);
    random_field(&mut field, 0x7a5);
    // Shift by (8, 16) cells: a multiple of the coarsest level's 8-cell
    // blocks, so every level's anchors map onto themselves.
    let mut shifted = zeroed(32, 32, 32.0, 32.0);
    for plane in 0..PLANES as u32 {
        let source = field.plane(plane).expect("plane").to_vec();
        fill(&mut shifted, plane, |x, y| {
            let from = ((y + 32 - 16) % 32) * 32 + (x + 32 - 8) % 32;
            source[from as usize]
        });
    }
    let scales = scales();
    let mut first = pyramid(&field, 3);
    let mut second = pyramid(&shifted, 3);
    let a = first.build(&field).expect("build");
    let b = second.build(&shifted).expect("build");
    let mut state = 99_u64;
    // Positions on a 2^-40 lattice, so that adding a whole cell is exact.
    let lattice = |u: f64| (32.0 * u * 1_099_511_627_776.0).floor() / 1_099_511_627_776.0;
    for _ in 0..50 {
        let position = [lattice(uniform(&mut state)), lattice(uniform(&mut state))];
        let moved = [(position[0] + 8.0) % 32.0, (position[1] + 16.0) % 32.0];
        let scale = 1.0 + 7.0 * uniform(&mut state);
        let here = observe(&a.sample(position, scale).expect("sample"), &STILL, &scales);
        let there = observe(&b.sample(moved, scale).expect("sample"), &STILL, &scales);
        assert_eq!(
            here.expect("observe"),
            there.expect("observe"),
            "{position:?} at {scale}"
        );
    }

    // A one-cell shift preserves level 0 exactly, but not the coarse levels'
    // anchors: there it is a resampling, not an identity.
    let mut nudged = zeroed(32, 32, 32.0, 32.0);
    for plane in 0..PLANES as u32 {
        let source = field.plane(plane).expect("plane").to_vec();
        fill(&mut nudged, plane, |x, y| {
            source[(y * 32 + (x + 31) % 32) as usize]
        });
    }
    let mut third = pyramid(&nudged, 3);
    let c = third.build(&nudged).expect("build");
    let position = [10.3, 20.7];
    let moved = [position[0] + 1.0, position[1]];
    assert_eq!(
        a.sample(position, 1.0).expect("sample"),
        c.sample(moved, 1.0).expect("sample")
    );
    assert_ne!(
        a.sample(position, 8.0).expect("sample"),
        c.sample(moved, 8.0).expect("sample")
    );
}

#[test]
fn sensing_refuses_bad_scales_layouts_and_values() {
    let field = zeroed(32, 32, 32.0, 32.0);
    let grid = *field.grid();
    let host = BufferLimits::host();
    assert!(matches!(
        SensorPyramid::new(FieldGrid::new(30, 32, 30.0, 32.0).expect("grid"), 2, &host),
        Err(CoreError::NumericalInvalid { .. })
    ));
    assert!(SensorPyramid::new(grid, 32, &host).is_err());
    assert!(matches!(
        SensorPyramid::new(grid, 2, &BufferLimits::new(1024)),
        Err(CoreError::BudgetExhausted { .. })
    ));
    let other = zeroed(16, 16, 32.0, 32.0);
    let mut storage = pyramid(&field, 2);
    assert!(matches!(
        storage.build(&other),
        Err(CoreError::LayoutMismatch { .. })
    ));
    let view = storage.build(&field).expect("build");
    for scale in [0.5, 4.000_001, f64::NAN, f64::INFINITY, -1.0] {
        assert!(view.sample([1.0, 1.0], scale).is_err(), "scale {scale}");
    }
    assert!(view.sample([f64::NAN, 1.0], 2.0).is_err());
    assert!(view.sample_level(3, [1.0, 1.0]).is_err());
    assert_eq!(view.density(3, 0, 0, 0), None);
    assert_eq!(view.density(0, PLANES, 0, 0), None);
    assert_eq!(view.density(2, 0, 8, 0), None);

    // Appendix A: a 128² grid and sensing scales 1–16 need four levels.
    let config = appendix_a();
    let appendix = SensorPyramid::from_config(&config, &host).expect("pyramid");
    assert_eq!(appendix.levels(), 4);
    assert_eq!(appendix.level_scale(4), Some(16.0));
    assert_eq!(
        appendix.allocated_bytes(),
        33 * 8 * (64 * 64 + 32 * 32 + 16 * 16 + 8 * 8)
    );
    let scales = SensorScales::from_config(&config).expect("scales");
    assert!(scales.admits_scale(1.0) && scales.admits_scale(16.0) && !scales.admits_scale(16.5));

    let good = [1.0, 1.0, 1.0e-6, 4.0, 60.0, 4.0, 1.0];
    assert!(SensorScales::new(good, 1.0, 16.0).is_ok());
    for index in 0..7 {
        for bad in [0.0, -1.0, f64::NAN] {
            let mut normalizers = good;
            normalizers[index] = bad;
            assert!(
                SensorScales::new(normalizers, 1.0, 16.0).is_err(),
                "{index} = {bad}"
            );
        }
    }
    assert!(SensorScales::new(good, 2.0, 1.0).is_err());

    let raw = RawSample {
        values: [0.0; PLANES],
        dx: [0.0; PLANES],
        dy: [0.0; PLANES],
    };
    let mut negative = raw;
    negative.values[0] = -1.0e-9;
    assert!(observe(&negative, &STILL, &scales).is_err());
    let mut nonfinite = raw;
    nonfinite.dx[5] = f64::INFINITY;
    assert!(observe(&nonfinite, &STILL, &scales).is_err());
    for intrinsic in [
        Intrinsic {
            energy: -1.0,
            ..STILL
        },
        Intrinsic {
            age_seconds: -1.0,
            ..STILL
        },
        Intrinsic {
            velocity: [f64::NAN, 0.0],
            ..STILL
        },
        Intrinsic {
            energy_change_per_second: f64::INFINITY,
            ..STILL
        },
    ] {
        assert!(observe(&raw, &intrinsic, &scales).is_err(), "{intrinsic:?}");
    }
    // An empty field is neutral: q = 0, Z = 0, no gradients.
    let empty = observe(&raw, &STILL, &scales).expect("observe");
    assert!(empty.inputs[..3 * PLANES].iter().all(|&v| v == 0.0));
}

fn logits(entries: &[(usize, f64)]) -> [f64; ACTION_WIDTH] {
    let mut raw = [0.0; ACTION_WIDTH];
    for &(index, value) in entries {
        raw[index] = value;
    }
    raw
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn appendix_decoding() -> ActionDecoding {
    ActionDecoding::from_config(&appendix_a()).expect("Appendix A decoding")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn step(denominator: u32) -> TimeStep {
    TimeStep::new(1, denominator).expect("Δt")
}

#[test]
fn c11_sigmoid_is_applied_once_to_the_linear_outputs() {
    let decoding = appendix_decoding();
    let dt = step(128);
    // A logit of 8 decodes near 0.99966465; tanh first would give 0.731.
    let raw = logits(&[(EMISSION_GATE, 8.0), (EMISSION_DIRECTION, 1.0)]);
    let decoded = decode(&raw, dt, &decoding).expect("decode");
    assert!((decoded.emission_gate - 0.999_664_65).abs() < 5.0e-9);
    let doubled = tanh(8.0).and_then(sigmoid).expect("σ(tanh 8)");
    assert!((doubled - 0.731_058_53).abs() < 1.0e-8);

    // Child shares reach their whole admitted interval [0.1, 0.9]. A hidden
    // activation before the sigmoid would confine them to [0.315, 0.685].
    let high = decode(&logits(&[(CHILD_SHARE, 8.0)]), dt, &decoding).expect("decode");
    let low = decode(&logits(&[(CHILD_SHARE, -8.0)]), dt, &decoding).expect("decode");
    assert!((high.child_fraction - (0.1 + 0.8 * 0.999_664_649_869_533_6)).abs() < 1.0e-15);
    assert!(high.child_fraction > 0.89 && low.child_fraction < 0.11);
    let narrowed = 0.1 + 0.8 * tanh(8.0).and_then(sigmoid).expect("σ");
    assert!(high.child_fraction > narrowed + 0.2);
    let saturated = decode(&logits(&[(CHILD_SHARE, 1.0e300)]), dt, &decoding).expect("decode");
    assert_eq!(saturated.child_fraction, 0.9);
    let floor = decode(&logits(&[(CHILD_SHARE, -1.0e300)]), dt, &decoding).expect("decode");
    assert_eq!(floor.child_fraction, 0.1);

    // Every decoded rate is σ of the raw output, exactly once.
    for logit in [-6.0, -0.5, 0.0, 0.75, 3.0] {
        let decoded = decode(&logits(&[(REPRODUCTION, logit)]), dt, &decoding).expect("decode");
        assert_eq!(decoded.attempt_rate, sigmoid(logit).expect("σ"));
    }
}

#[test]
fn c11_rates_decode_to_timestep_consistent_fractions_with_exact_zero_limits() {
    let decoding = appendix_decoding();
    let dt = step(128);
    let open = [(EMISSION_GATE, 4.0), (EMISSION_DIRECTION, 1.0)];

    // A vanishing hazard: p = λΔt to full relative precision, where an F32
    // 1 − exp(−λΔt) is exactly zero.
    let tiny = decode(&logits(&[(REPRODUCTION, -40.0)]), dt, &decoding).expect("decode");
    assert!(tiny.attempt_rate > 0.0 && tiny.attempt_rate < 1.0e-17);
    let first_order = tiny.attempt_rate / 128.0;
    assert!((tiny.attempt_probability / first_order - 1.0).abs() < 1.0e-15);
    assert_eq!(1.0_f32 - (-(tiny.attempt_rate as f32) / 128.0).exp(), 0.0);
    // The exact zero-rate limit.
    let never = decode(&logits(&[(REPRODUCTION, -1.0e4)]), dt, &decoding).expect("decode");
    assert_eq!((never.attempt_rate, never.attempt_probability), (0.0, 0.0));
    let mut spend_nothing = open.to_vec();
    spend_nothing.push((EMISSION_SPEND, -1.0e4));
    let idle = decode(&logits(&spend_nothing), dt, &decoding).expect("decode");
    assert_eq!(idle.emission.map(|e| e.spend_fraction), Some(0.0));
    let mut spend_tiny = open.to_vec();
    spend_tiny.push((EMISSION_SPEND, -40.0));
    let trickle = decode(&logits(&spend_tiny), dt, &decoding).expect("decode");
    let fraction = trickle.emission.map_or(0.0, |e| e.spend_fraction);
    assert!(fraction > 0.0 && fraction < 1.0e-20);

    // Saturated: λ = λ_max and r = r_max.
    let mut full = open.to_vec();
    full.extend([(REPRODUCTION, 60.0), (EMISSION_SPEND, 60.0)]);
    let busy = decode(&logits(&full), dt, &decoding).expect("decode");
    assert_eq!(busy.attempt_rate, 1.0);
    assert!(
        (busy.attempt_probability - -(-1.0_f64 / 128.0).exp_m1()).abs()
            <= 2.0 * f64::EPSILON * 0.01
    );
    let spend = busy.emission.map_or(0.0, |e| e.spend_fraction);
    assert!((spend - -(-0.25_f64 / 128.0).exp_m1()).abs() <= 2.0 * f64::EPSILON * 0.01);

    // Timestep refinement: two half ticks compose to one tick, as a
    // constant-rate process does.
    for logit in [-3.0, 0.0, 2.5, 8.0] {
        let mut raw = open.to_vec();
        raw.extend([(REPRODUCTION, logit), (EMISSION_SPEND, logit)]);
        let whole = decode(&logits(&raw), dt, &decoding).expect("decode");
        let half = decode(&logits(&raw), step(256), &decoding).expect("decode");
        // 1 − (1 − p)² written without cancellation.
        let p = half.attempt_probability;
        let composed = p * (2.0 - p);
        assert!(
            (composed - whole.attempt_probability).abs()
                <= 8.0 * f64::EPSILON * whole.attempt_probability
        );
        let whole_spend = whole.emission.map_or(0.0, |e| e.spend_fraction);
        let f = half.emission.map_or(0.0, |e| e.spend_fraction);
        assert!((f * (2.0 - f) - whole_spend).abs() <= 8.0 * f64::EPSILON * whole_spend);
    }
}

#[test]
fn emission_needs_an_open_gate_and_a_valid_direction() {
    let decoding = appendix_decoding();
    let dt = step(128);
    let direction = |values: &[f64]| -> Vec<(usize, f64)> {
        values
            .iter()
            .enumerate()
            .map(|(k, &v)| (EMISSION_DIRECTION + k, v))
            .collect()
    };
    // σ(0) = ½ is not above the threshold ½.
    let mut shut = direction(&[3.0, 4.0]);
    shut.push((EMISSION_GATE, 0.0));
    assert_eq!(
        decode(&logits(&shut), dt, &decoding)
            .expect("decode")
            .emission,
        None
    );
    let mut open = direction(&[3.0, 4.0]);
    open.push((EMISSION_GATE, 1.0e-3));
    let emission = decode(&logits(&open), dt, &decoding)
        .expect("decode")
        .emission
        .expect("an open gate and a direction emit");
    assert!((emission.direction[0] - 0.6).abs() <= f64::EPSILON);
    assert!((emission.direction[1] - 0.8).abs() <= f64::EPSILON);
    let squared: f64 = emission.direction.iter().map(|a| a * a).sum();
    assert!(squared <= 1.0);
    assert!(emission.direction[2..].iter().all(|&a| a == 0.0));

    // no-emission-no-charge: a zero or sub-threshold direction emits nothing.
    for values in [
        vec![0.0; CHANNELS],
        vec![3.0e-9, 4.0e-9],
        vec![6.0e-9, 7.9e-9],
    ] {
        let mut raw = direction(&values);
        raw.push((EMISSION_GATE, 6.0));
        let decoded = decode(&logits(&raw), dt, &decoding).expect("decode");
        assert_eq!(decoded.emission, None, "{values:?}");
        assert!(decoded.emission_gate > 0.99);
    }
    let mut just_above = direction(&[6.0e-9, 8.1e-9]);
    just_above.push((EMISSION_GATE, 6.0));
    assert!(
        decode(&logits(&just_above), dt, &decoding)
            .expect("decode")
            .emission
            .is_some()
    );

    // Faults are refused, not decoded.
    let mut overflowing = direction(&[1.0e200, 1.0e200]);
    overflowing.push((EMISSION_GATE, 6.0));
    assert!(decode(&logits(&overflowing), dt, &decoding).is_err());
    for index in [
        REPRODUCTION,
        CHILD_SHARE,
        EMISSION_GATE,
        EMISSION_DIRECTION + 31,
        EMISSION_SPEND,
    ] {
        for bad in [f64::NAN, f64::INFINITY] {
            assert!(
                matches!(
                    decode(&logits(&[(index, bad)]), dt, &decoding),
                    Err(CoreError::NumericalInvalid { .. })
                ),
                "output {index} = {bad}"
            );
        }
    }
}

#[test]
fn decoding_constants_are_checked() {
    let appendix = appendix_decoding();
    assert_eq!(
        appendix,
        ActionDecoding::new(1.0, [0.1, 0.9], 0.5, 0.25, 1.0e-8).expect("decoding")
    );
    for (rate, fractions, gate, spend, zero) in [
        (-1.0, [0.1, 0.9], 0.5, 0.25, 1.0e-8),
        (1.0, [0.0, 0.9], 0.5, 0.25, 1.0e-8),
        (1.0, [0.6, 0.5], 0.5, 0.25, 1.0e-8),
        (1.0, [0.1, 1.0], 0.5, 0.25, 1.0e-8),
        (1.0, [0.1, 0.9], 1.0, 0.25, 1.0e-8),
        (1.0, [0.1, 0.9], -0.1, 0.25, 1.0e-8),
        (1.0, [0.1, 0.9], 0.5, -0.25, 1.0e-8),
        (1.0, [0.1, 0.9], 0.5, 0.25, -1.0e-8),
        (f64::NAN, [0.1, 0.9], 0.5, 0.25, 1.0e-8),
    ] {
        assert!(
            matches!(
                ActionDecoding::new(rate, fractions, gate, spend, zero),
                Err(CoreError::NumericalInvalid { .. })
            ),
            "{rate} {fractions:?} {gate} {spend} {zero}"
        );
    }
    // A zero maximum rate never attempts and never spends.
    let idle = ActionDecoding::new(0.0, [0.1, 0.9], 0.5, 0.0, 1.0e-8).expect("decoding");
    let raw = logits(&[
        (REPRODUCTION, 50.0),
        (EMISSION_GATE, 50.0),
        (EMISSION_DIRECTION, 1.0),
        (EMISSION_SPEND, 50.0),
    ]);
    let decoded = decode(&raw, step(128), &idle).expect("decode");
    assert_eq!(decoded.attempt_probability, 0.0);
    assert_eq!(decoded.emission.map(|e| e.spend_fraction), Some(0.0));
}
