//! FL-004 evidence (plan §23.1): exact source totals, periodic stencil, CFL
//! and decay fixtures (Appendix C.6, C.17, §4.5, §11.5, §21.4).
//!
//! Oracles are independent: rational arithmetic for dyadic fixtures, a
//! separately written `rem_euclid` stencil for random fields, and `std`'s
//! `exp` for the host-independent exponential.

use flife_core::ids::SourceId;
use flife_core::{CoreError, TickSpan, TimeStep, ValidatedConfig};
use flife_field::numerics::exp_neg;
use flife_field::sources::deposit;
use flife_field::sources::unit_direction;
use flife_field::state::CHANNELS;
use flife_field::transport::{MAX_SUBSTEPS, transport};
use flife_field::{
    BufferLimits, Cell, ConeTolerance, FieldGrid, FieldPair, FieldState, PathSegment, Source,
    SourceKernel, SourceSupport, TransportPlan, TransportScratch, advance_field,
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
fn grid(cells_x: u32, cells_y: u32, width: f64, height: f64) -> FieldGrid {
    FieldGrid::new(cells_x, cells_y, width, height).expect("grid")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn tick() -> TimeStep {
    TimeStep::new(1, 128).expect("Δt")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn zeroed(grid: FieldGrid) -> FieldState {
    FieldState::zeroed(grid, &BufferLimits::host()).expect("allocate")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn plan(grid: FieldGrid, diffusion: f64, decay: f64) -> TransportPlan {
    TransportPlan::new(&grid, tick(), diffusion, decay).expect("plan")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn run(plan: &TransportPlan, current: &FieldState) -> FieldState {
    let mut next = zeroed(*current.grid());
    let mut scratch =
        TransportScratch::new(current.grid(), &BufferLimits::host()).expect("scratch");
    transport(plan, current, &mut next, &mut scratch).expect("transport");
    next
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn at(state: &FieldState, plane: u32, x: u32, y: u32) -> f32 {
    let index = state.grid().cell_index(x, y).expect("cell");
    state.plane(plane).expect("plane")[index]
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn set(state: &mut FieldState, plane: u32, x: u32, y: u32, value: f32) {
    let index = state.grid().cell_index(x, y).expect("cell");
    state.plane_mut(plane).expect("plane")[index] = value;
}

fn direction(components: &[(usize, f64)]) -> [f64; CHANNELS] {
    let mut out = [0.0; CHANNELS];
    for &(channel, value) in components {
        out[channel] = value;
    }
    out
}

/// A deterministic sequence in [0, 1).
fn lcg(seed: u64) -> impl FnMut() -> f64 {
    let mut state = seed;
    move || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        f64::from(u32::try_from(state >> 40).unwrap_or(0)) / 16_777_216.0
    }
}

fn ulp(value: f32) -> f64 {
    f64::from(f32::from_bits(value.to_bits() + 1)) - f64::from(value)
}

// ------------------------------------------------------------- C.6 stencil

#[test]
fn c6_unit_impulse_gives_one_fifth_to_center_and_each_neighbor() {
    // r = D Δt / h² = 25.6 / 128 = 0.2 on unit cells.
    let grid = grid(8, 8, 8.0, 8.0);
    let plan = plan(grid, 25.6, 0.0);
    assert_eq!(plan.requested_ratios(), (0.2, 0.2));
    assert_eq!(plan.substeps(), 1);
    let mut field = zeroed(grid);
    set(&mut field, 0, 3, 4, 1.0);
    let next = run(&plan, &field);
    // The rational answer is exactly 1/5 at five cells; F32 holds the
    // nearest value, within half an ulp.
    for (x, y) in [(3, 4), (2, 4), (4, 4), (3, 3), (3, 5)] {
        let value = at(&next, 0, x, y);
        assert!(
            (f64::from(value) - 0.2).abs() <= ulp(value) / 2.0,
            "({x}, {y}): {value}"
        );
    }
    let total: f64 = next.energy().iter().map(|&value| f64::from(value)).sum();
    let others = next.energy().iter().filter(|&&value| value != 0.0).count();
    assert_eq!(others, 5);
    assert!((total - 1.0).abs() <= 5.0 * ulp(0.2) / 2.0, "sum {total}");
}

#[test]
fn c6_dyadic_stencil_is_exact_and_preserves_the_cone() {
    // r = 0.125: center 1/2, neighbors 1/8, all exact in F32.
    let grid = grid(8, 8, 8.0, 8.0);
    let plan = plan(grid, 16.0, 0.0);
    assert_eq!(plan.ratios(), (0.125, 0.125));
    assert_eq!(plan.center(), 0.5);
    let mut field = zeroed(grid);
    let mut cell = Cell::EMPTY;
    cell.energy = 1.0;
    cell.moments[7] = -1.0; // on the cone: ||m|| = e
    field.set_cell(3, 4, &cell).expect("in range");
    let next = run(&plan, &field);
    for ((x, y), value) in [
        ((3, 4), 0.5),
        ((2, 4), 0.125),
        ((4, 4), 0.125),
        ((3, 3), 0.125),
        ((3, 5), 0.125),
    ] {
        assert_eq!(at(&next, 0, x, y), value);
        assert_eq!(at(&next, 8, x, y), -value);
    }
    let summary = next
        .check_invariants(ConeTolerance::EXACT)
        .expect("cone holds exactly");
    assert_eq!(summary.energy_total, 1.0);
    assert_eq!(summary.max_cone_residual, 0.0);
}

#[test]
fn periodic_stencil_wraps_at_every_edge() {
    let grid = grid(8, 8, 8.0, 8.0);
    let plan = plan(grid, 16.0, 0.0);
    let mut field = zeroed(grid);
    set(&mut field, 0, 0, 0, 1.0);
    let next = run(&plan, &field);
    for ((x, y), value) in [
        ((0, 0), 0.5),
        ((7, 0), 0.125),
        ((1, 0), 0.125),
        ((0, 7), 0.125),
        ((0, 1), 0.125),
    ] {
        assert_eq!(at(&next, 0, x, y), value, "({x}, {y})");
    }
    assert_eq!(
        next.energy().iter().filter(|&&value| value != 0.0).count(),
        5
    );
}

#[test]
fn rectangular_stencil_matches_an_independent_oracle_and_conserves() {
    // h_x = 1, h_y = 2, so r_y = r_x / 4; random nonnegative energy and
    // signed moments.
    let grid = grid(16, 8, 16.0, 16.0);
    let plan = plan(grid, 25.6, 0.0);
    let (r_x, r_y) = plan.ratios();
    assert_eq!((r_x, r_y), (0.2, 0.05));
    let mut field = zeroed(grid);
    let mut draw = lcg(7);
    for plane in 0..33 {
        for value in field.plane_mut(plane).expect("plane") {
            let sample = draw();
            *value = if plane == 0 {
                sample as f32
            } else {
                (sample - 0.5) as f32 / 64.0
            };
        }
    }
    let next = run(&plan, &field);
    let (w, h) = (16_i64, 8_i64);
    for plane in 0..33 {
        let source = field.plane(plane).expect("plane");
        let target = next.plane(plane).expect("plane");
        let value = |x: i64, y: i64| {
            let index = usize::try_from(y.rem_euclid(h) * w + x.rem_euclid(w)).unwrap_or(0);
            f64::from(source[index])
        };
        for y in 0..h {
            for x in 0..w {
                let oracle = (1.0 - 2.0 * r_x - 2.0 * r_y) * value(x, y)
                    + r_x * value(x - 1, y)
                    + r_x * value(x + 1, y)
                    + r_y * value(x, y - 1)
                    + r_y * value(x, y + 1);
                let got = target[usize::try_from(y * w + x).unwrap_or(0)];
                assert!(
                    (f64::from(got) - oracle).abs() <= ulp(got.abs().max(f32::MIN_POSITIVE)),
                    "plane {plane} ({x}, {y}): {got} vs {oracle}"
                );
                if plane == 0 {
                    assert!(got >= 0.0);
                }
            }
        }
    }
    let before: f64 = field.energy().iter().map(|&v| f64::from(v)).sum();
    let after: f64 = next.energy().iter().map(|&v| f64::from(v)).sum();
    // Conservation up to one F32 rounding per cell.
    assert!(
        (after - before).abs() <= 128.0 * f64::from(f32::EPSILON),
        "{before} → {after}"
    );
    assert!(
        next.check_invariants(ConeTolerance::new(0.0, 1.0e-6).expect("tol"))
            .is_ok()
    );
}

// ------------------------------------------------------------ CFL, subcycle

#[test]
fn cfl_bound_subcycles_instead_of_going_negative() {
    let square = grid(8, 8, 8.0, 8.0);
    // r = 0.3 per axis (sum 0.6): two substeps of 0.15, center 0.4.
    let fast = plan(square, 38.4, 0.0);
    assert_eq!(fast.requested_ratios(), (0.3, 0.3));
    assert_eq!(fast.substeps(), 2);
    assert!(fast.center() >= 0.0);
    let half = plan(square, 19.2, 0.0);
    assert_eq!(half.substeps(), 1);
    assert_eq!(fast.ratios(), half.ratios());
    let mut field = zeroed(square);
    let mut draw = lcg(11);
    for value in field.energy_mut() {
        *value = draw() as f32;
    }
    let twice = run(&half, &run(&half, &field));
    let once = run(&fast, &field);
    assert_eq!(
        once, twice,
        "one subcycled tick equals two half ticks bit for bit"
    );

    // Exactly at the bound: one substep and a zero center coefficient.
    let edge = plan(square, 32.0, 0.0);
    assert_eq!((edge.substeps(), edge.center()), (1, 0.0));

    // Rectangular cells: the bound is r_x + r_y ≤ 1/2, not r ≤ 1/4.
    let rectangular = plan(grid(16, 8, 16.0, 16.0), 38.4, 0.0);
    assert!(rectangular.requested_ratios().0 > 0.25);
    assert_eq!(rectangular.substeps(), 1);

    // Across a sweep the substep count is minimal and the center is
    // never negative.
    let mut diffusion = 0.01;
    while diffusion < 1.0e4 {
        let p = plan(square, diffusion, 0.0);
        assert!(p.center() >= 0.0, "D = {diffusion}");
        if p.substeps() > 1 {
            let (r_x, r_y) = p.requested_ratios();
            let fewer = f64::from(p.substeps() - 1);
            assert!(
                1.0 - 2.0 * (r_x / fewer) - 2.0 * (r_y / fewer) < 0.0,
                "D = {diffusion}"
            );
        }
        diffusion *= 1.37;
    }
}

#[test]
fn transport_refuses_what_it_cannot_run() {
    let square = grid(8, 8, 8.0, 8.0);
    let too_fast = 128.0 * 0.5 * f64::from(MAX_SUBSTEPS) * 1.5;
    for (diffusion, decay) in [
        (-1.0, 0.0),
        (f64::NAN, 0.0),
        (0.0, -0.1),
        (0.0, f64::INFINITY),
        (too_fast, 0.0),
    ] {
        assert!(
            matches!(
                TransportPlan::new(&square, tick(), diffusion, decay),
                Err(CoreError::NumericalInvalid { .. })
            ),
            "D = {diffusion:?}, λ = {decay:?}"
        );
    }
    let p = plan(square, 1.0, 0.0);
    let other = zeroed(grid(4, 4, 4.0, 4.0));
    let mut next = zeroed(square);
    let mut scratch = TransportScratch::new(&square, &BufferLimits::host()).expect("scratch");
    assert!(matches!(
        transport(&p, &other, &mut next, &mut scratch),
        Err(CoreError::LayoutMismatch { .. })
    ));
    let mut small = TransportScratch::new(other.grid(), &BufferLimits::host()).expect("scratch");
    assert!(matches!(
        transport(&p, &zeroed(square), &mut next, &mut small),
        Err(CoreError::LayoutMismatch { .. })
    ));
}

#[test]
fn appendix_a_transport_plan() {
    let plan = TransportPlan::from_config(&appendix_a()).expect("plan");
    // D Δt / h² = 0.15 / 128 on unit cells: far inside the bound.
    assert_eq!(plan.requested_ratios(), (0.15 / 128.0, 0.15 / 128.0));
    assert_eq!(plan.substeps(), 1);
    assert_eq!(plan.decay_factor(), exp_neg(0.01 / 128.0).expect("factor"));
}

// -------------------------------------------------------------------- decay

#[test]
fn host_independent_exp_matches_std() {
    assert_eq!(exp_neg(0.0), Some(1.0));
    assert_eq!(exp_neg(-1.0), None);
    assert_eq!(exp_neg(f64::NAN), None);
    assert_eq!(exp_neg(f64::INFINITY), None);
    assert_eq!(exp_neg(1.0e4), Some(0.0));
    let mut x = 1.0e-9;
    while x < 40.0 {
        let ours = exp_neg(x).expect("finite");
        let reference = (-x).exp();
        let relative = (ours - reference).abs() / reference;
        // Within 1 ulp of the correctly rounded value; `std` within 1 more.
        assert!(
            relative <= 4.0 * f64::EPSILON,
            "x = {x}: {ours} vs {reference}"
        );
        x *= 1.9;
    }
    // Cross-host guard: the bits every IEEE-754 host computes, pinned on
    // macOS arm64 and checked again on Linux x86_64. All three are the
    // correctly rounded values (Python `Decimal`, 60 digits): Appendix A's
    // decay factor, x = 0.1, and the kernel cutoff x = 8. The full table is
    // in `flife-core/tests/numerics.rs`.
    for (x, bits) in [
        (0.01 / 128.0, 0x3fef_ff5c_2a99_2df2_u64),
        (0.1, 0x3fec_f46d_99d5_2b3a),
        (8.0, 0x3f35_fc21_0410_27ad),
    ] {
        assert_eq!(exp_neg(x).map(f64::to_bits), Some(bits), "exp_neg({x:?})");
    }
}

#[test]
fn decay_charges_exactly_what_it_removes() {
    let grid = grid(4, 4, 4.0, 4.0);
    // λΔt = 0.1: the factor is exp(−0.1).
    let plan = plan(grid, 0.0, 12.8);
    let factor = plan.decay_factor();
    assert!((factor - (-0.1_f64).exp()).abs() <= 4.0 * f64::EPSILON);
    let mut field = zeroed(grid);
    let mut cell = Cell::EMPTY;
    cell.energy = 1.0;
    cell.moments[0] = 1.0;
    for y in 0..4 {
        for x in 0..4 {
            field.set_cell(x, y, &cell).expect("in range");
        }
    }
    let mut next = zeroed(grid);
    let mut scratch = TransportScratch::new(&grid, &BufferLimits::host()).expect("scratch");
    let report = transport(&plan, &field, &mut next, &mut scratch).expect("transport");
    let decayed = (1.0 * factor) as f32;
    assert!(next.energy().iter().all(|&value| value == decayed));
    assert!(
        next.moment(0)
            .expect("moment")
            .iter()
            .all(|&value| value == decayed)
    );
    assert_eq!(report.energy_before, 16.0);
    assert_eq!(report.energy_transported, 16.0);
    assert_eq!(report.transport_residual, 0.0);
    assert_eq!(report.decay_removed, 16.0 * (1.0 - f64::from(decayed)));
    assert_eq!(
        report.energy_after + report.decay_removed,
        report.energy_before
    );
    next.check_invariants(ConeTolerance::EXACT)
        .expect("a common factor keeps the cone");

    let none = run(&plan_with_no_decay(grid), &field);
    assert_eq!(none, field, "λ = 0 and D = 0 change nothing");
}

fn plan_with_no_decay(grid: FieldGrid) -> TransportPlan {
    plan(grid, 0.0, 0.0)
}

// ----------------------------------------------------------- C.17 sources

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn source(center: [f64; 2], direction: [f64; CHANNELS]) -> Source {
    Source::new(SourceId::new(0), center, direction, 80.0).expect("source")
}

/// `(σ, center, support cells, each cell's share)`.
type SupportCase<'a> = (f64, [f64; 2], &'a [(u32, u32)], f32);

#[test]
fn small_supports_deposit_exact_totals() {
    let grid = grid(8, 8, 8.0, 8.0);
    let mut support = SourceSupport::new();
    let chemistry = direction(&[(0, 0.5), (1, -0.5)]);
    // P Δt = 80/128 = 0.625, split over 1, 2 and 4 cells (the last across
    // the periodic corner).
    let cases: [SupportCase<'_>; 3] = [
        (0.1, [3.5, 2.5], &[(3, 2)], 0.625),
        (0.15, [4.0, 2.5], &[(3, 2), (4, 2)], 0.3125),
        (0.2, [0.0, 0.0], &[(0, 0), (7, 0), (0, 7), (7, 7)], 0.156_25),
    ];
    for (sigma, center, cells, share) in cases {
        let kernel = SourceKernel::new(sigma, 4.0).expect("kernel");
        let mut field = zeroed(grid);
        let report = deposit(
            &mut field,
            &kernel,
            &source(center, chemistry),
            tick(),
            &mut support,
        )
        .expect("deposit");
        assert_eq!(report.support_cells, cells.len());
        assert_eq!(report.declared, 0.625);
        // Σ k may round (3w is not always exact); the stored shares do not.
        assert!((report.normalized_total - 0.625).abs() <= 4.0 * f64::EPSILON);
        assert_eq!(report.applied, 0.625);
        for &(x, y) in cells {
            assert_eq!(at(&field, 0, x, y), share, "{center:?} ({x}, {y})");
            assert_eq!(at(&field, 1, x, y), share / 2.0);
            assert_eq!(at(&field, 2, x, y), -share / 2.0);
        }
        let summary = field
            .check_invariants(ConeTolerance::EXACT)
            .expect("inside the cone");
        assert_eq!(summary.energy_total, 0.625);
    }
}

#[test]
fn source_totals_do_not_depend_on_resolution_or_position() {
    let config = appendix_a();
    let kernel = SourceKernel::from_config(&config).expect("Appendix A kernel");
    assert_eq!((kernel.sigma(), kernel.radius()), (10.0, 40.0));
    let mut support = SourceSupport::new();
    let mut counts = Vec::new();
    for cells in [64, 128, 256] {
        let grid = grid(cells, cells, 128.0, 128.0);
        for center in [[64.0, 64.0], [0.3, 127.9], [127.99, 0.01], [31.25, 90.5]] {
            let mut field = zeroed(grid);
            let report = deposit(
                &mut field,
                &kernel,
                &source(center, direction(&[(5, 1.0)])),
                tick(),
                &mut support,
            )
            .expect("deposit");
            let weights: f64 = support.normalized().map(|(_, weight)| weight).sum();
            // Up to ~20,000 F64 terms, summed in cell order.
            assert!(
                (weights - 1.0).abs() <= 1.0e-12,
                "{cells}² {center:?}: Σw = {weights}"
            );
            assert!((report.normalized_total - 0.625).abs() <= 1.0e-12);
            // Each stored cell rounds once to F32.
            assert!((report.applied - 0.625).abs() <= 0.625 * f64::from(f32::EPSILON));
            let stored: f64 = field.energy().iter().map(|&v| f64::from(v)).sum();
            assert_eq!(stored, report.applied);
            counts.push(report.support_cells);
        }
    }
    counts.dedup();
    assert!(counts.len() >= 3, "supports of different sizes: {counts:?}");
}

#[test]
fn supports_wrap_without_duplicates_and_empty_supports_are_refused() {
    let grid = grid(8, 8, 8.0, 8.0);
    let mut support = SourceSupport::new();
    SourceKernel::new(100.0, 4.0)
        .expect("kernel")
        .support_into(&grid, [1.0, 7.0], &mut support)
        .expect("whole grid");
    assert_eq!(support.len(), 64, "every cell once");

    let tiny = SourceKernel::new(0.01, 1.0).expect("kernel");
    assert!(matches!(
        tiny.support_into(&grid, [4.0, 4.0], &mut support),
        Err(CoreError::NumericalInvalid {
            quantity: "source support",
            ..
        })
    ));
    assert!(matches!(
        tiny.support_into(&grid, [8.0, 4.0], &mut support),
        Err(CoreError::NumericalInvalid { .. })
    ));
}

#[test]
fn chemical_directions_stay_inside_the_unit_ball() {
    assert!(
        Source::new(
            SourceId::new(1),
            [0.0, 0.0],
            direction(&[(0, 0.8), (1, 0.6000001)]),
            1.0
        )
        .is_err()
    );
    assert!(Source::new(SourceId::new(1), [0.0, 0.0], [0.0; CHANNELS], -1.0).is_err());
    assert!(Source::new(SourceId::new(1), [f64::NAN, 0.0], [0.0; CHANNELS], 1.0).is_err());
    let mut draw = lcg(3);
    for _ in 0..1000 {
        let normals = [(); CHANNELS].map(|()| draw() * 2.0 - 1.0);
        let unit = unit_direction(&normals).expect("direction");
        let norm: f64 = unit.iter().map(|v| v * v).sum();
        assert!(norm <= 1.0, "||a||² = {norm}");
        assert!((norm - 1.0).abs() <= 1.0e-14);
        assert!(Source::new(SourceId::new(2), [1.0, 1.0], unit, 1.0).is_ok());
    }
    assert!(unit_direction(&[0.0; CHANNELS]).is_err());
    // fixed-per-source: moving a source keeps its chemistry.
    let moving = source([1.0, 1.0], direction(&[(3, 0.25)]));
    let moved = moving.moved_to([2.5, 6.0]).expect("move");
    assert_eq!(moved.direction(), moving.direction());
    assert_eq!(moved.center(), [2.5, 6.0]);
}

// ------------------------------------------------------------ C.17 paths

#[test]
fn smooth_paths_join_continuously_within_the_speed_limit() {
    let world = grid(128, 128, 128.0, 128.0);
    let segment_ticks = TickSpan::new(3840); // 30 s
    let first = PathSegment::new(
        &world,
        [10.0, 20.0],
        [1.0, 0.0],
        [0.0, 1.0],
        1.0,
        segment_ticks,
        tick(),
    )
    .expect("segment");
    // s(1) = vT (a + b)/2 = 30 (0.5, 0.5).
    assert_eq!(first.position(&world, 1.0).expect("end"), [25.0, 35.0]);
    assert_eq!(first.velocity(1.0).expect("end"), [0.0, 1.0]);
    let second = first.next(&world, [-1.0, 0.0], tick()).expect("next");
    assert_eq!(
        second.position(&world, 0.0).expect("start"),
        first.position(&world, 1.0).expect("end")
    );
    assert_eq!(
        second.velocity(0.0).expect("start"),
        first.velocity(1.0).expect("end")
    );
    for segment in [first, second] {
        for k in 0..segment.ticks() {
            let u = segment.mid_tick(k).expect("inside");
            let [vx, vy] = segment.velocity(u).expect("velocity");
            assert!((vx * vx + vy * vy).sqrt() <= 1.0 + 1.0e-12, "u = {u}");
        }
    }
    assert_eq!(first.mid_tick(0).expect("first"), 1.0 / 7680.0);
    assert!(first.mid_tick(3840).is_err());

    // Paths wrap on the torus.
    let edge = PathSegment::new(
        &world,
        [127.0, 64.0],
        [1.0, 0.0],
        [1.0, 0.0],
        1.0,
        segment_ticks,
        tick(),
    )
    .expect("segment");
    assert_eq!(edge.position(&world, 1.0).expect("end"), [29.0, 64.0]);

    for (anchor, start, ticks) in [
        ([128.0, 0.0], [1.0, 0.0], 1),
        ([0.0, 0.0], [0.6, 0.6], 1),
        ([0.0, 0.0], [1.0, 0.0], 0),
    ] {
        assert!(
            PathSegment::new(
                &world,
                anchor,
                start,
                [1.0, 0.0],
                1.0,
                TickSpan::new(ticks),
                tick()
            )
            .is_err()
        );
    }
}

// ------------------------------------------------- phase 2 and the ledger

#[test]
fn field_step_accounts_for_every_joule() {
    let config = appendix_a();
    let world = FieldGrid::from_config(&config).expect("grid");
    let plan = TransportPlan::from_config(&config).expect("plan");
    let kernel = SourceKernel::from_config(&config).expect("kernel");
    let step = config.report().timestep;
    let limits = BufferLimits::host();
    let mut pair = FieldPair::zeroed(world, &limits).expect("pair");
    let mut scratch = TransportScratch::new(&world, &limits).expect("scratch");
    let mut support = SourceSupport::new();
    let mut draw = lcg(5);
    let sources: Vec<Source> = (0..8_u32)
        .map(|id| {
            let normals = [(); CHANNELS].map(|()| draw() * 2.0 - 1.0);
            Source::new(
                SourceId::new(id),
                [draw() * 128.0, draw() * 128.0],
                unit_direction(&normals).expect("direction"),
                80.0,
            )
            .expect("source")
        })
        .collect();

    let first = advance_field(
        &mut pair,
        &plan,
        &kernel,
        &sources,
        step,
        &mut scratch,
        &mut support,
    )
    .expect("tick");
    assert!(
        pair.current().energy().iter().all(|&v| v == 0.0),
        "the committed field is untouched"
    );
    assert_eq!(first.injected_declared, 5.0);
    assert_eq!(first.deposits.len(), 8);
    assert!(first.residual.abs() <= 5.0 * f64::from(f32::EPSILON));
    pair.commit();

    // One simulated second of transport, decay and sources: the cumulative
    // residual stays far inside §21.4's proposed F32 budget of 1e-5 of the
    // energy plus the absolute transfers.
    // Far from every source, diffusion leaves subnormal F32 energies (about
    // 1e-40), where F32 has no relative precision: moment rounding there
    // left a 5e-45 cone residual at e = 6.4e-41. A declared cone tolerance
    // therefore needs an absolute floor at F32's subnormal scale; the
    // smallest normal F32 is used here.
    let tolerance = ConeTolerance::new(f64::from(f32::MIN_POSITIVE), 1.0e-5).expect("tolerance");
    let mut cumulative_residual = first.residual;
    let (mut injected, mut removed) = (first.injected_declared, first.transport.decay_removed);
    for _ in 1..128 {
        let report = advance_field(
            &mut pair,
            &plan,
            &kernel,
            &sources,
            step,
            &mut scratch,
            &mut support,
        )
        .expect("tick");
        assert_eq!(report.injected_declared, 5.0);
        assert!(report.transport.decay_removed > 0.0);
        cumulative_residual += report.residual;
        injected += report.injected_declared;
        removed += report.transport.decay_removed;
        pair.commit();
    }
    let summary = pair
        .current()
        .check_invariants(tolerance)
        .expect("INV-01, INV-02");
    assert_eq!(injected, 640.0);
    // The ledger closes: E_128 = I − D + ε.
    let closing = summary.energy_total - (injected - removed);
    assert!(
        (closing - cumulative_residual).abs() <= 1.0e-9,
        "{closing} vs {cumulative_residual}"
    );
    let normalized = cumulative_residual.abs() / (injected + removed);
    assert!(
        normalized < 1.0e-6,
        "normalized cumulative residual {normalized:e}"
    );
    println!(
        "128 ticks: E = {}, injected {injected}, decayed {removed}, cumulative residual {cumulative_residual:e} ({normalized:e} normalized)",
        summary.energy_total
    );
}
