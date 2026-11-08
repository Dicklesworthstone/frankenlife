//! FL-005 evidence (plan §23.1): contention, rounding and no-overdraw tests
//! for demand/grant uptake, and the energy ledger closing over them (§4.4,
//! §5.1, §5.2, §21.2 INV-03–05, §21.4, §21.5, Appendix C.1).
//!
//! Grants are whole F32 quanta of their cell, so the oracles are integers:
//! a cell's grants must sum to exactly its debit, and its new energy must be
//! exactly the old energy minus that debit.

use flife_core::ids::{OrganismId, SourceId};
use flife_core::ledger::CompensatedSum;
use flife_core::{
    CoreError, EnergyBudget, EnergyLedger, Flow, Holdings, LedgerReport, ResidualBudget, Tick,
    TickLedger, TimeStep, Transfer, ValidatedConfig,
};
use flife_field::sources::unit_direction;
use flife_field::state::CHANNELS;
use flife_field::{
    BufferLimits, Cell, ConeTolerance, Demand, ExchangeStencil, FieldGrid, FieldPair, FieldState,
    Grants, Source, SourceKernel, SourceSupport, TransportPlan, TransportScratch, advance_field,
    apply_grants, resolve_uptake, saturating_request,
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
fn zeroed(grid: FieldGrid) -> FieldState {
    FieldState::zeroed(grid, &BufferLimits::host()).expect("allocate")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn tick() -> TimeStep {
    TimeStep::new(1, 128).expect("Δt")
}

/// A cell holding `energy` with composition `direction` scaled by
/// `purity ≤ 1`, so `||m|| = purity · energy`.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn put(
    field: &mut FieldState,
    x: u32,
    y: u32,
    energy: f32,
    direction: &[f64; CHANNELS],
    purity: f64,
) {
    let moments = direction.map(|component| (f64::from(energy) * purity * component) as f32);
    field
        .set_cell(x, y, &Cell { energy, moments })
        .expect("cell inside the grid");
}

/// The center of cell `(x, y)`, where the exchange footprint is that one
/// cell.
fn center(grid: &FieldGrid, x: u32, y: u32) -> [f64; 2] {
    [
        (f64::from(x) + 0.5) * grid.cell_width(),
        (f64::from(y) + 0.5) * grid.cell_height(),
    ]
}

fn demand(id: u64, position: [f64; 2], amount: f64) -> Demand {
    Demand {
        organism: OrganismId::new(id),
        position,
        amount,
    }
}

fn axis(channel: usize) -> [f64; CHANNELS] {
    let mut direction = [0.0; CHANNELS];
    direction[channel] = 1.0;
    direction
}

/// A fixed linear congruential generator in `[0, 1)`.
fn lcg(seed: u64) -> impl FnMut() -> f64 {
    let mut state = seed;
    move || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        f64::from(u32::try_from(state >> 40).unwrap_or(0)) / 16_777_216.0
    }
}

/// `(Q, n)` with `e = n Q`: the F32 quantum of a cell and its count.
fn quanta(energy: f32) -> (f64, u64) {
    let bits = energy.to_bits();
    let exponent = (bits >> 23) & 0xff;
    let fraction = u64::from(bits & 0x7f_ffff);
    let count = if exponent == 0 {
        fraction
    } else {
        fraction | 0x80_0000
    };
    let power = i32::try_from(exponent.max(1)).unwrap_or(1) - 150;
    (2.0_f64.powi(power), count)
}

/// The grant in whole quanta; fails unless the grant is a whole number.
fn whole_quanta(granted: f64, quantum: f64) -> u64 {
    let count = granted / quantum;
    assert_eq!(
        count.fract(),
        0.0,
        "grant {granted} is not whole quanta of {quantum}"
    );
    // Integral and below 2^25.
    count as u64
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn holdings(field: &FieldState, organisms: &[f64]) -> Holdings {
    let mut total = CompensatedSum::ZERO;
    for &energy in organisms {
        total.add(energy);
    }
    Holdings::new(field.energy_total(), total.value()).expect("holdings")
}

/// Every INV-04 property of one resolution, checked from the outside:
/// grants are whole quanta, sum to the debit, never exceed the cell, and
/// leave exactly the remainder.
fn assert_no_overdraw(field_before: &FieldState, grants: &Grants) {
    let energy = field_before.energy();
    let mut packets = grants.packets().iter().peekable();
    for debit in grants.debits() {
        assert_eq!(debit.energy_before.to_bits(), energy[debit.cell].to_bits());
        let (quantum, count) = quanta(debit.energy_before);
        let mut granted = 0_u64;
        while let Some(packet) = packets.next_if(|packet| packet.cell == debit.cell) {
            assert!(packet.granted >= 0.0);
            granted += whole_quanta(packet.granted, quantum);
        }
        assert!(granted <= count, "INV-04: cell {} overdrawn", debit.cell);
        assert_eq!(whole_quanta(debit.granted, quantum), granted);
        assert_eq!(
            f64::from(debit.energy_after),
            (count - granted) as f64 * quantum,
            "the new energy is exactly the remainder"
        );
        assert_eq!(
            f64::from(debit.energy_after) + debit.granted,
            f64::from(debit.energy_before)
        );
    }
    assert!(packets.next().is_none(), "every packet belongs to a debit");
}

/// Appendix C.1: one cell holds `e = 10` and `m = 10a`. Requests of 8 and 12
/// are granted 4 and 6. With efficiencies 0.5 and 0.25 the gains are 2 and
/// 1.5, the cell loses 10, and the ledger dissipates exactly 6.5.
#[test]
fn c1_contended_uptake_balances_exactly() {
    let world = grid(4, 4, 4.0, 4.0);
    let mut field = zeroed(world);
    let mut direction = [0.0; CHANNELS];
    direction[0] = 0.6;
    direction[1] = 0.8;
    put(&mut field, 1, 2, 10.0, &direction, 1.0);
    let before = field.clone();
    let demands = [
        demand(7, center(&world, 1, 2), 8.0),
        demand(3, center(&world, 1, 2), 12.0),
    ];
    let mut grants = Grants::new();
    let summary = resolve_uptake(&field, &demands, &mut grants).expect("resolve");
    assert_eq!(grants.totals(), [4.0, 6.0]);
    assert_eq!(summary.requested, 20.0);
    assert_eq!(summary.granted, 10.0);
    assert_eq!(
        (
            summary.cells,
            summary.contended_cells,
            summary.drained_cells
        ),
        (1, 1, 1)
    );
    assert_no_overdraw(&before, &grants);

    let mut energies = vec![1.0, 1.0];
    let mut ledger = EnergyLedger::new(holdings(&field, &energies), ResidualBudget::EXACT);
    let mut tick = ledger
        .open(Tick::new(0), holdings(&field, &energies))
        .expect("opens");
    assert_eq!(apply_grants(&mut field, &mut grants).expect("apply"), 10.0);
    let cell = field.cell(1, 2).expect("cell");
    assert_eq!(cell.energy, 0.0);
    assert!(
        cell.moments.iter().all(|&m| m.to_bits() == 0),
        "the moment leaves with the energy"
    );
    for ((energy, &granted), efficiency) in
        energies.iter_mut().zip(grants.totals()).zip([0.5, 0.25])
    {
        let gain = granted * efficiency;
        tick.transfer(Transfer::Uptake, granted, gain)
            .expect("uptake");
        *energy += gain;
    }
    assert_eq!(energies, [3.0, 2.5]);
    let closure = tick
        .close(holdings(&field, &energies))
        .expect("closes exactly");
    assert_eq!(closure.residual(), 0.0);
    assert_eq!(closure.dissipated(), 6.5);
    assert_eq!(closure.opening().total() - closure.closing().total(), 6.5);
    let report = ledger.commit(&closure).expect("commits");
    assert_eq!(report.residual, 0.0);
}

/// An uncontended cell grants every request in full and keeps the rest,
/// with the same proportion of its moment.
#[test]
fn uncontended_cells_grant_requests_and_keep_the_rest() {
    let world = grid(4, 4, 4.0, 4.0);
    let mut field = zeroed(world);
    let mut direction = [0.0; CHANNELS];
    direction[0] = 0.6;
    direction[1] = 0.8;
    put(&mut field, 2, 2, 10.0, &direction, 1.0);
    let demands = [
        demand(1, center(&world, 2, 2), 3.0),
        demand(2, center(&world, 2, 2), 4.0),
    ];
    let mut grants = Grants::new();
    let summary = resolve_uptake(&field, &demands, &mut grants).expect("resolve");
    assert_eq!(grants.totals(), [3.0, 4.0]);
    assert_eq!((summary.contended_cells, summary.drained_cells), (0, 0));
    apply_grants(&mut field, &mut grants).expect("apply");
    let cell = field.cell(2, 2).expect("cell");
    assert_eq!(cell.energy, 3.0);
    assert_eq!(cell.moments[0], (6.0_f64 * 0.3) as f32);
    assert_eq!(cell.moments[1], (8.0_f64 * 0.3) as f32);
    assert!((f64::from(cell.moments[0]) - 1.8).abs() <= 1.8 * f64::from(f32::EPSILON));
    field
        .check_invariants(ConeTolerance::new(0.0, 1.0e-6).expect("tolerance"))
        .expect("cone");
}

/// Three equal claims on `e = 1`: the ideal share is `2^23/3` quanta each.
/// The two leftover quanta go to the two lowest identities, whatever order
/// the demands arrive in.
#[test]
fn grants_follow_identity_not_input_order() {
    let world = grid(4, 4, 4.0, 4.0);
    let mut field = zeroed(world);
    put(&mut field, 0, 0, 1.0, &axis(5), 0.5);
    let at = center(&world, 0, 0);
    let ids = [40_u64, 9, 23];
    let orders = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    let quantum = 2.0_f64.powi(-23);
    let expected = |id: u64| {
        let quanta = if id == 40 { 2_796_202.0 } else { 2_796_203.0 };
        quanta * quantum
    };
    let mut reference: Option<Vec<f32>> = None;
    for order in orders {
        let demands: Vec<Demand> = order.iter().map(|&i| demand(ids[i], at, 1.0)).collect();
        let mut grants = Grants::new();
        resolve_uptake(&field, &demands, &mut grants).expect("resolve");
        for (demand, &total) in demands.iter().zip(grants.totals()) {
            assert_eq!(total, expected(demand.organism.get()), "order {order:?}");
        }
        let mut after = field.clone();
        apply_grants(&mut after, &mut grants).expect("apply");
        let bits = after.as_slice().to_vec();
        match &reference {
            None => reference = Some(bits),
            Some(reference) => assert_eq!(&bits, reference, "the field is identical"),
        }
    }
}

/// Rounding happens once per cell, after aggregation. A request below half
/// a quantum gets nothing; two such requests together get one quantum.
#[test]
fn rounding_is_per_cell_and_never_creates_energy() {
    let world = grid(4, 4, 4.0, 4.0);
    let mut field = zeroed(world);
    put(&mut field, 3, 1, 1.0, &axis(0), 1.0);
    let at = center(&world, 3, 1);
    let quantum = 2.0_f64.powi(-23);
    let cases: [(&[f64], &[f64]); 4] = [
        (&[0.3], &[0.0]),
        (&[0.3, 0.3], &[1.0, 0.0]),
        (&[0.5, 0.5, 0.5], &[1.0, 1.0, 0.0]),
        (&[0.25, 0.25, 0.25, 0.25], &[1.0, 0.0, 0.0, 0.0]),
    ];
    for (requests, granted) in cases {
        let demands: Vec<Demand> = requests
            .iter()
            .zip(1_u64..)
            .map(|(&quanta, id)| demand(id, at, quanta * quantum))
            .collect();
        let mut grants = Grants::new();
        let summary = resolve_uptake(&field, &demands, &mut grants).expect("resolve");
        let expected: Vec<f64> = granted.iter().map(|&quanta| quanta * quantum).collect();
        assert_eq!(
            grants.totals(),
            expected.as_slice(),
            "requests {requests:?}"
        );
        assert_no_overdraw(&field, &grants);
        // The F32 cell changes by exactly what was granted, so nothing was
        // created even when nothing was granted.
        let mut after = field.clone();
        apply_grants(&mut after, &mut grants).expect("apply");
        assert_eq!(
            f64::from(after.energy()[world.cell_index(3, 1).expect("cell")]),
            1.0 - summary.granted
        );
    }
}

/// §21.5: a million competing requests to one cell cannot overdraw it.
#[test]
fn a_million_competing_requests_cannot_overdraw_a_cell() {
    let world = grid(8, 8, 8.0, 8.0);
    let mut field = zeroed(world);
    put(&mut field, 5, 6, 1000.0, &axis(2), 0.75);
    let at = center(&world, 5, 6);
    let mut draw = lcg(3);
    let demands: Vec<Demand> = (0..1_000_000_u64)
        .map(|id| demand(id * 7 + 1, at, 0.001 + draw()))
        .collect();
    let mut grants = Grants::new();
    let summary = resolve_uptake(&field, &demands, &mut grants).expect("resolve");
    assert_eq!(summary.largest_cell_packets, 1_000_000);
    assert_eq!((summary.contended_cells, summary.drained_cells), (1, 1));
    assert_eq!(summary.granted, 1000.0, "the cell is drained exactly");
    assert_no_overdraw(&field, &grants);
    let (quantum, count) = quanta(1000.0);
    let scale = count as f64 / summary.requested;
    for (demand, &total) in demands.iter().zip(grants.totals()) {
        let ideal = demand.amount * scale;
        let quanta = whole_quanta(total, quantum) as f64;
        assert!((quanta - ideal).abs() < 1.0 + 1.0e-9, "within one quantum");
        assert!(total <= demand.amount, "nobody gets more than it asked");
    }
    apply_grants(&mut field, &mut grants).expect("apply");
    let cell = field.cell(5, 6).expect("cell");
    assert_eq!(cell.energy, 0.0);
    assert!(cell.moments.iter().all(|&m| m.to_bits() == 0));
}

/// Uniform demand plus an adversarial hotspot on a 64² field with random
/// composition: INV-04 holds in every cell, the field loses exactly what is
/// granted, and the cone survives the debit.
#[test]
fn hotspots_and_uniform_demand_conserve_every_cell() {
    let world = grid(64, 64, 64.0, 64.0);
    let mut field = zeroed(world);
    let mut draw = lcg(41);
    for y in 0..64 {
        for x in 0..64 {
            let normals = [(); CHANNELS].map(|()| draw() * 2.0 - 1.0);
            let direction = unit_direction(&normals).expect("direction");
            let energy = if (x + y) % 9 == 0 {
                0.0
            } else {
                (draw() * 4.0) as f32
            };
            put(&mut field, x, y, energy, &direction, draw());
        }
    }
    let before = field.clone();
    let hotspot = center(&world, 30, 32);
    let demands: Vec<Demand> = (0..100_000_u64)
        .map(|id| {
            let position = if id % 2 == 0 {
                [
                    hotspot[0] + (draw() - 0.5) * 0.2,
                    hotspot[1] + (draw() - 0.5) * 0.2,
                ]
            } else {
                [draw() * 64.0, draw() * 64.0]
            };
            demand(id, position, draw() * 0.05)
        })
        .collect();
    let mut grants = Grants::new();
    let summary = resolve_uptake(&field, &demands, &mut grants).expect("resolve");
    assert!(summary.contended_cells >= 4, "the hotspot is contended");
    assert!(summary.cells > 3_000);
    assert_no_overdraw(&before, &grants);
    let granted_by_demand: f64 = grants.totals().iter().sum();
    assert!((granted_by_demand - summary.granted).abs() <= 1.0e-12 * summary.granted);

    apply_grants(&mut field, &mut grants).expect("apply");
    let loss = before.energy_total() - field.energy_total();
    assert!((loss - summary.granted).abs() <= 1.0e-12 * before.energy_total());
    // Composition is removed in proportion: m/e is unchanged up to F32
    // rounding, and the cone holds.
    for debit in grants
        .debits()
        .iter()
        .filter(|debit| debit.energy_after > 0.0)
    {
        for channel in 0..32_u32 {
            let old = f64::from(before.moment(channel).expect("plane")[debit.cell]);
            let new = f64::from(field.moment(channel).expect("plane")[debit.cell]);
            let expected = old * f64::from(debit.energy_after) / f64::from(debit.energy_before);
            assert!(
                (new - expected).abs()
                    <= expected.abs() * f64::from(f32::EPSILON) + f64::from(f32::MIN_POSITIVE)
            );
        }
    }
    let tolerance = ConeTolerance::new(f64::from(f32::MIN_POSITIVE), 1.0e-6).expect("tolerance");
    before.check_invariants(tolerance).expect("before");
    field.check_invariants(tolerance).expect("after");
}

/// `grid-cic-v1`: bilinear weights at `(x/h − ½, y/h − ½)`, periodic, local,
/// with repeated cells combined on grids one cell wide.
#[test]
fn exchange_footprints_are_bilinear_local_and_periodic() {
    let world = grid(8, 4, 16.0, 4.0);
    let index = |x: u32, y: u32| world.cell_index(x, y).expect("cell");

    let at_center = ExchangeStencil::at(&world, center(&world, 3, 2)).expect("stencil");
    assert_eq!(at_center.cells(), [index(3, 2)]);
    assert_eq!(at_center.weights(), [1.0]);

    // Between four centers: four equal weights.
    let corner = ExchangeStencil::at(&world, [8.0, 2.0]).expect("stencil");
    assert_eq!(
        corner.cells(),
        [index(3, 1), index(4, 1), index(3, 2), index(4, 2)]
    );
    assert_eq!(corner.weights(), [0.25; 4]);

    // A quarter cell from the origin wraps to the far row and column.
    let origin = ExchangeStencil::at(&world, [0.5, 0.25]).expect("stencil");
    assert_eq!(
        origin.cells(),
        [index(0, 0), index(7, 0), index(0, 3), index(7, 3)]
    );
    // u = 0.5/2 − ½ = −¼ and v = 0.25/1 − ½ = −¼: three quarters of the
    // way from column 7 to column 0 and from row 3 to row 0.
    assert_eq!(
        origin.weights(),
        [0.75 * 0.75, 0.25 * 0.75, 0.75 * 0.25, 0.25 * 0.25]
    );

    // Positions outside the period wrap first.
    let wrapped = ExchangeStencil::at(&world, [8.0 + 16.0 * 3.0, 2.0 - 4.0]).expect("stencil");
    assert_eq!(wrapped, corner);

    // One cell wide: both columns are the same cell, combined.
    let thin = grid(1, 4, 1.0, 4.0);
    let combined = ExchangeStencil::at(&thin, [0.75, 1.0]).expect("stencil");
    assert_eq!(combined.cells(), [0, 1]);
    assert_eq!(combined.weights(), [0.5, 0.5]);
    let single = ExchangeStencil::at(&grid(1, 1, 3.0, 3.0), [0.75, 0.75]).expect("stencil");
    assert_eq!(single.cells(), [0]);
    assert_eq!(single.weights(), [1.0]);

    // Random positions: weights are positive, sum to one, and every cell is
    // within one cell spacing of the position.
    let mut draw = lcg(8);
    for _ in 0..10_000 {
        let position = [draw() * 16.0, draw() * 4.0];
        let stencil = ExchangeStencil::at(&world, position).expect("stencil");
        let total: f64 = stencil.weights().iter().sum();
        assert!(
            (total - 1.0).abs() <= 4.0 * f64::EPSILON,
            "weights sum to {total}"
        );
        assert!(stencil.weights().iter().all(|&weight| weight > 0.0));
        assert!(stencil.cells().windows(2).all(|pair| pair[0] < pair[1]));
        for &cell in stencil.cells() {
            let (x, y) = (cell % 8, cell / 8);
            let cell_center = center(
                &world,
                u32::try_from(x).expect("small"),
                u32::try_from(y).expect("small"),
            );
            let [dx, dy] = world
                .displacement(position, cell_center)
                .expect("displacement");
            assert!(dx.abs() <= 2.0 && dy.abs() <= 1.0, "consumption is local");
        }
    }
    assert!(ExchangeStencil::at(&world, [f64::NAN, 0.0]).is_err());
}

/// A.1's request law, with the density read from the same footprint.
#[test]
fn saturating_requests_follow_appendix_a() {
    let step = tick();
    let ceiling = 0.4 / 128.0;
    assert_eq!(saturating_request(step, 0.4, 0.1, 0.0).expect("zero"), 0.0);
    let half = saturating_request(step, 0.4, 0.1, 0.1).expect("half");
    assert!((half - ceiling / 2.0).abs() <= 2.0 * f64::EPSILON * ceiling);
    let mut previous = 0.0;
    for density in [1.0e-6, 0.01, 0.1, 1.0, 10.0, 1.0e6] {
        let request = saturating_request(step, 0.4, 0.1, density).expect("request");
        assert!(request > previous && request < ceiling);
        previous = request;
    }
    assert_eq!(
        saturating_request(step, 0.0, 0.1, 5.0).expect("no capacity"),
        0.0
    );
    for (capacity, half, density) in [
        (-0.1, 0.1, 1.0),
        (0.4, 0.0, 1.0),
        (0.4, -1.0, 1.0),
        (0.4, 0.1, -1.0),
        (f64::NAN, 0.1, 1.0),
        (0.4, 0.1, f64::INFINITY),
    ] {
        assert!(saturating_request(step, capacity, half, density).is_err());
    }

    let world = grid(4, 4, 8.0, 8.0);
    let mut field = zeroed(world);
    put(&mut field, 1, 1, 8.0, &axis(0), 0.0);
    // Cell area is 4, so the center density is 2; halfway to the next
    // (empty) cell it is 1.
    let stencil = ExchangeStencil::at(&world, center(&world, 1, 1)).expect("stencil");
    assert_eq!(stencil.density(&field).expect("density"), 2.0);
    let halfway = ExchangeStencil::at(&world, [4.0, 3.0]).expect("stencil");
    assert_eq!(halfway.density(&field).expect("density"), 1.0);
}

#[test]
fn grants_apply_once_to_the_field_they_were_resolved_for() {
    let world = grid(4, 4, 4.0, 4.0);
    let mut field = zeroed(world);
    put(&mut field, 1, 1, 2.0, &axis(0), 1.0);
    put(&mut field, 2, 1, 2.0, &axis(1), 1.0);
    let demands = [
        demand(1, center(&world, 1, 1), 0.5),
        demand(2, [2.0, 1.5], 0.5),
    ];
    let mut grants = Grants::new();
    resolve_uptake(&field, &demands, &mut grants).expect("resolve");

    // A field changed since resolution is refused, and left as it was.
    let mut changed = field.clone();
    changed.energy_mut()[world.cell_index(2, 1).expect("cell")] = 1.5;
    let snapshot = changed.as_slice().to_vec();
    assert!(matches!(
        apply_grants(&mut changed, &mut grants),
        Err(CoreError::StateConflict { .. })
    ));
    assert_eq!(changed.as_slice(), snapshot.as_slice());
    // So is another grid.
    let mut other = zeroed(grid(4, 4, 8.0, 8.0));
    assert!(matches!(
        apply_grants(&mut other, &mut grants),
        Err(CoreError::StateConflict { .. })
    ));
    // The right field accepts them once.
    apply_grants(&mut field, &mut grants).expect("apply");
    assert!(grants.is_applied());
    let snapshot = field.as_slice().to_vec();
    assert!(matches!(
        apply_grants(&mut field, &mut grants),
        Err(CoreError::StateConflict { .. })
    ));
    assert_eq!(field.as_slice(), snapshot.as_slice());

    // Refused demands leave the grants empty.
    for bad in [
        vec![demand(1, [0.5, 0.5], 1.0), demand(1, [3.5, 3.5], 1.0)],
        vec![demand(1, [0.5, 0.5], -1.0)],
        vec![demand(1, [0.5, 0.5], f64::NAN)],
        vec![demand(1, [f64::INFINITY, 0.5], 1.0)],
    ] {
        assert!(resolve_uptake(&field, &bad, &mut grants).is_err());
        assert!(
            grants.packets().is_empty() && grants.debits().is_empty() && grants.totals().is_empty()
        );
        assert!(
            apply_grants(&mut field, &mut grants).is_err(),
            "empty grants have no grid"
        );
    }
    assert!(matches!(
        resolve_uptake(
            &field,
            &[demand(1, [0.5, 0.5], 1.0), demand(1, [3.5, 3.5], 1.0)],
            &mut grants
        ),
        Err(CoreError::StateConflict { .. })
    ));
}

/// One failure the ledger and grant checks must catch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fault {
    None,
    /// Organisms are credited but the field is never debited.
    CreditWithoutDebit,
    /// The field is debited but organisms are never credited.
    DebitWithoutCredit,
    /// An organism is credited its gain twice (INV-05).
    DoubleCredit,
    /// The inefficiency loss is never recorded.
    UnrecordedLoss,
    /// A gain larger than the grant.
    GainExceedsGrant,
    /// An overspend "repaired" by zeroing the cell while the organism keeps
    /// its whole gain (§5.2 forbids exactly this).
    OverdrawRepairedByClamping,
    /// The grants are applied twice.
    DoubleApply,
    /// The field changes between resolution and application.
    StaleField,
    /// One organism demands twice.
    DuplicateDemand,
    /// Requested rather than paid maintenance is recorded (§8.2).
    UnpaidMaintenanceRecorded,
    /// Field energy changes between ticks with no flow.
    UnrecordedChangeBetweenTicks,
}

const FAULTS: [(Fault, &str); 11] = [
    (Fault::CreditWithoutDebit, "residual"),
    (Fault::DebitWithoutCredit, "residual"),
    (Fault::DoubleCredit, "residual"),
    (Fault::UnrecordedLoss, "residual"),
    (Fault::GainExceedsGrant, "transfer"),
    (Fault::OverdrawRepairedByClamping, "residual"),
    (Fault::DoubleApply, "conflict"),
    (Fault::StaleField, "conflict"),
    (Fault::DuplicateDemand, "conflict"),
    (Fault::UnpaidMaintenanceRecorded, "residual"),
    (Fault::UnrecordedChangeBetweenTicks, "conflict"),
];

struct Organism {
    id: OrganismId,
    position: [f64; 2],
    efficiency: f64,
    energy: f64,
}

/// Two ticks of uptake, chemistry with fixed efficiencies, and maintenance
/// on a 16² field, with one injected `fault`.
fn scenario(fault: Fault) -> Result<LedgerReport, CoreError> {
    let world = FieldGrid::new(16, 16, 16.0, 16.0)?;
    let mut field = FieldState::zeroed(world, &BufferLimits::host())?;
    let mut draw = lcg(19);
    for y in 0..16 {
        for x in 0..16 {
            let energy = if x == 0 && y == 0 {
                0.0
            } else {
                (draw() * 2.0) as f32
            };
            let normals = [(); CHANNELS].map(|()| draw() * 2.0 - 1.0);
            let direction = unit_direction(&normals)?;
            let moments = direction.map(|component| (f64::from(energy) * 0.5 * component) as f32);
            field
                .set_cell(x, y, &Cell { energy, moments })
                .ok_or(CoreError::Overflow {
                    quantity: "test cell",
                })?;
        }
    }
    let hotspot = center(&world, 7, 9);
    let mut organisms: Vec<Organism> = (0..64_u64)
        .map(|id| Organism {
            id: OrganismId::new(id * 3 + 2),
            position: if id < 16 {
                hotspot
            } else {
                [draw() * 16.0, draw() * 16.0]
            },
            efficiency: draw() * 0.8,
            energy: 0.5 + draw(),
        })
        .collect();
    // A starving organism on the empty cell: maintenance exhausts it.
    organisms.push(Organism {
        id: OrganismId::new(1),
        position: center(&world, 0, 0),
        efficiency: 0.5,
        energy: 1.0e-6,
    });
    let step = TimeStep::new(1, 128)?;
    let maintenance = 0.0128 * step.seconds_per_tick();
    let budget = ResidualBudget::new(0.0, 1.0e-12, 1.0e-12)?;
    let energies =
        |organisms: &[Organism]| -> Vec<f64> { organisms.iter().map(|o| o.energy).collect() };
    let mut ledger = EnergyLedger::new(holdings(&field, &energies(&organisms)), budget);
    let mut grants = Grants::new();
    for number in 0..2_u64 {
        if number == 1 && fault == Fault::UnrecordedChangeBetweenTicks {
            field.energy_mut()[17] += 0.5;
        }
        let mut tick = ledger.open(Tick::new(number), holdings(&field, &energies(&organisms)))?;
        let mut demands = Vec::with_capacity(organisms.len() + 1);
        for organism in &organisms {
            let density = ExchangeStencil::at(&world, organism.position)?.density(&field)?;
            // Twice the Appendix A capacity, so the hotspot is contended.
            let amount = saturating_request(step, 0.8, 0.1, density)?;
            demands.push(demand(organism.id.get(), organism.position, amount * 20.0));
        }
        if fault == Fault::DuplicateDemand {
            demands.push(demands[3]);
        }
        resolve_uptake(&field, &demands, &mut grants)?;
        if number == 0 {
            assert!(
                grants.summary().contended_cells > 0,
                "the hotspot is contended"
            );
        }
        if fault == Fault::StaleField {
            let cell = grants.debits()[0].cell;
            field.energy_mut()[cell] *= 0.5;
        }
        if fault != Fault::CreditWithoutDebit {
            apply_grants(&mut field, &mut grants)?;
        }
        if fault == Fault::DoubleApply {
            apply_grants(&mut field, &mut grants)?;
        }
        let totals = grants.totals().to_vec();
        for (index, (organism, &granted)) in organisms.iter_mut().zip(&totals).enumerate() {
            let mut account = EnergyBudget::new(organism.energy)?;
            let efficiency = if fault == Fault::GainExceedsGrant && index == 5 {
                1.25
            } else {
                organism.efficiency
            };
            let mut gain = granted * efficiency;
            if fault == Fault::OverdrawRepairedByClamping && index == 0 {
                // The organism is handed more than its cell holds. The cell
                // is clamped to zero to "pay" for it, and the ledger is told
                // only what the field lost.
                let cell = grants
                    .packets()
                    .iter()
                    .find(|packet| packet.demand == 0)
                    .map_or(0, |packet| packet.cell);
                let left = f64::from(field.energy()[cell]);
                field.energy_mut()[cell] = 0.0;
                tick.transfer(Transfer::Uptake, granted + left, gain + left)?;
                gain += left + 1.0e-3;
            } else if fault != Fault::UnrecordedLoss {
                tick.transfer(Transfer::Uptake, granted, gain)?;
            }
            if fault != Fault::DebitWithoutCredit {
                account.credit(gain)?;
            }
            if fault == Fault::DoubleCredit && index == 7 {
                account.credit(gain)?;
            }
            let payment = account.pay(maintenance)?;
            let recorded = if fault == Fault::UnpaidMaintenanceRecorded {
                payment.requested
            } else {
                payment.paid
            };
            tick.record(Flow::Maintenance, recorded)?;
            organism.energy = account.remaining();
        }
        // The exhausted organism dies with nothing left; it is not carried
        // into the next tick (FL-012 owns the full death transaction).
        organisms.retain(|organism| organism.energy > 0.0);
        let closure = tick.close(holdings(&field, &energies(&organisms)))?;
        ledger.commit(&closure)?;
    }
    Ok(ledger.report())
}

/// The rust_scriptbots fault-injection lane, made a default test: every
/// injected accounting fault must turn the gate red, through the check
/// that is meant to catch it.
#[test]
fn every_injected_fault_turns_the_gate_red() {
    let clean = scenario(Fault::None).expect("the clean world closes");
    assert_eq!(clean.ticks, 2);
    assert!(clean.dissipated > 0.0);
    assert!(clean.normalized.expect("turnover") < 1.0e-14, "{clean:?}");
    for (fault, gate) in FAULTS {
        let outcome = scenario(fault);
        let caught = matches!(
            (&outcome, gate),
            (
                Err(CoreError::NumericalInvalid {
                    quantity: "energy ledger residual (INV-03)",
                    ..
                }),
                "residual",
            ) | (
                Err(CoreError::NumericalInvalid {
                    quantity: "uptake",
                    ..
                }),
                "transfer",
            ) | (Err(CoreError::StateConflict { .. }), "conflict")
        );
        assert!(
            caught,
            "{fault:?} must fail the {gate} gate, got {outcome:?}"
        );
    }
}

/// One simulated second of the Appendix A world: transport, decay and
/// sources, then 4,096 static organisms taking up energy, converting it
/// with fixed efficiencies and paying maintenance. Every tick is committed
/// through the ledger under §21.4's proposed F32 budget.
#[test]
fn uptake_and_sources_close_the_ledger_for_a_simulated_second() {
    let config = appendix_a();
    let world = FieldGrid::from_config(&config).expect("grid");
    let plan = TransportPlan::from_config(&config).expect("plan");
    let kernel = SourceKernel::from_config(&config).expect("kernel");
    let step = config.report().timestep;
    let chemistry = &config.config().chemistry;
    let limits = BufferLimits::host();
    let mut pair = FieldPair::zeroed(world, &limits).expect("pair");
    let (initial, _) = FieldState::initial(&config, &limits).expect("initial field");
    pair.current_mut()
        .copy_from(&initial)
        .expect("initial field");
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
    let population = usize::try_from(config.config().population.initial_count).expect("count");
    let mut organisms: Vec<Organism> = (0..population)
        .map(|index| Organism {
            id: OrganismId::new(u64::try_from(index).expect("small") + 1),
            position: [draw() * 128.0, draw() * 128.0],
            efficiency: draw() * chemistry.maximum_efficiency,
            energy: config.config().population.initial_energy,
        })
        .collect();
    // The starter's minimum mandatory cost per second (§19.4).
    let maintenance = 0.008_556_928 * step.seconds_per_tick();
    let budget = ResidualBudget::new(f64::from(f32::MIN_POSITIVE), 1.0e-5, 1.0e-5).expect("budget");
    let energies =
        |organisms: &[Organism]| -> Vec<f64> { organisms.iter().map(|o| o.energy).collect() };
    let mut ledger = EnergyLedger::new(holdings(pair.current(), &energies(&organisms)), budget);
    let mut grants = Grants::new();
    let mut demands = Vec::with_capacity(population);
    let (mut granted, mut contended) = (0.0, 0);
    for number in 0..128_u64 {
        let mut tick = ledger
            .open(
                Tick::new(number),
                holdings(pair.current(), &energies(&organisms)),
            )
            .expect("opens where the last tick closed");
        let field_step = advance_field(
            &mut pair,
            &plan,
            &kernel,
            &sources,
            step,
            &mut scratch,
            &mut support,
        )
        .expect("phase 2");
        tick.record(Flow::SourcePower, field_step.injected_declared)
            .expect("sources");
        tick.record(Flow::Decay, field_step.transport.decay_removed)
            .expect("decay");
        let (_, candidate) = pair.stage();
        demands.clear();
        for organism in &organisms {
            let density = ExchangeStencil::at(&world, organism.position)
                .and_then(|stencil| stencil.density(candidate))
                .expect("density");
            let amount = saturating_request(
                step,
                chemistry.uptake_capacity_per_second,
                chemistry.uptake_half_saturation_density,
                density,
            )
            .expect("request");
            demands.push(demand(organism.id.get(), organism.position, amount));
        }
        let summary = resolve_uptake(candidate, &demands, &mut grants).expect("resolve");
        apply_grants(candidate, &mut grants).expect("apply");
        granted += summary.granted;
        contended += summary.contended_cells;
        for (organism, &total) in organisms.iter_mut().zip(grants.totals()) {
            let gain = total * organism.efficiency;
            tick.transfer(Transfer::Uptake, total, gain)
                .expect("uptake");
            let mut account = EnergyBudget::new(organism.energy).expect("budget");
            account.credit(gain).expect("gain");
            let payment = account.pay(maintenance).expect("maintenance");
            assert!(payment.is_full(), "nobody starves in one second");
            tick.record(Flow::Maintenance, payment.paid)
                .expect("maintenance");
            organism.energy = account.remaining();
        }
        pair.commit();
        let closure = tick
            .close(holdings(pair.current(), &energies(&organisms)))
            .expect("INV-03 per tick");
        ledger.commit(&closure).expect("INV-03 cumulative");
    }
    let report = ledger.report();
    eprintln!("{report:?}; granted {granted}, contended cells {contended}");
    assert_eq!(report.ticks, 128);
    assert_eq!(report.injected, 640.0, "8 sources × 80 for one second");
    assert!(granted > 1.0, "organisms took up energy");
    let normalized = report.normalized.expect("turnover");
    assert!(normalized < 1.0e-5, "§21.4's proposed F32 gate");
    let tolerance = ConeTolerance::new(f64::from(f32::MIN_POSITIVE), 1.0e-5).expect("tolerance");
    pair.current()
        .check_invariants(tolerance)
        .expect("INV-01, INV-02");
}

#[test]
fn tick_ledgers_need_no_run_ledger_for_single_fixtures() {
    let opening = Holdings::new(1.0, 0.0);
    assert!(opening.is_ok_and(|opening| {
        TickLedger::open(Tick::new(0), opening, ResidualBudget::EXACT)
            .close(opening)
            .is_ok_and(|closure| closure.residual() == 0.0)
    }));
}
