//! FL-006 evidence (plan §23.1): per-packet chemistry, toxicity,
//! maintenance, velocity and energy fixtures for the organism phase (§5.3,
//! §5.4, §5.6, §8.2, Appendix C.2, C.3, C.9, C.11, C.12).
//!
//! The fixed-number fixtures use dyadic values wherever the plan allows, so
//! their oracles are exact. The integrated fixture runs real FL-005 grants
//! through chemistry, maintenance and motion and closes the ledger on every
//! tick.

use flife_core::ids::OrganismId;
use flife_core::ledger::CompensatedSum;
use flife_core::{
    CoreError, EnergyBudget, EnergyLedger, Flow, Holdings, LedgerReport, ResidualBudget, Tick,
    TickLedger, TimeStep, Transfer, ValidatedConfig,
};
use flife_engine::{
    Affinity, ChemicalPacket, Chemistry, MaintenanceCosts, Motion, MotionResponse, RESPONSE_TERMS,
    composition, settle_efficiencies, uptake_ratio,
};
use flife_field::state::CHANNELS;
use flife_field::{
    BufferLimits, Cell, Demand, ExchangeStencil, FieldGrid, FieldState, Grants, apply_grants,
    resolve_uptake, saturating_request,
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
fn step(numerator: u32, denominator: u32) -> TimeStep {
    TimeStep::new(numerator, denominator).expect("Δt")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn holdings(field: f64, organisms: f64) -> Holdings {
    Holdings::new(field, organisms).expect("holdings")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn budget(energy: f64) -> EnergyBudget {
    EnergyBudget::new(energy).expect("budget")
}

/// The C.9 constants: `η_max = 0.8`, `k = 4`, `b = 0.15`, `κ = 0.2`.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn c9_chemistry() -> Chemistry {
    Chemistry::new(0.8, 4.0, 0.15, 0.2, 1.0).expect("C.9 chemistry")
}

/// Motion with `Δt = 1 s`, no drag, limits of 4 and cost `c_v`.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn unit_step_motion(cost: f64) -> Motion {
    Motion::new(step(1, 1), 0.0, 4.0, 4.0, cost, 1.0).expect("motion")
}

fn unit(channel: usize) -> [f64; CHANNELS] {
    let mut direction = [0.0; CHANNELS];
    if let Some(slot) = direction.get_mut(channel) {
        *slot = 1.0;
    }
    direction
}

fn squared(v: [f64; 2]) -> f64 {
    v[0] * v[0] + v[1] * v[1]
}

/// splitmix64, for reproducible arguments.
fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// A uniform value in `[−scale, scale)` with 53 random bits.
fn symmetric(state: &mut u64, scale: f64) -> f64 {
    let unit = (splitmix(state) >> 11) as f64 / (1_u64 << 53) as f64;
    (2.0 * unit - 1.0) * scale
}

#[test]
fn c2_toxicity_is_clipped_to_what_the_organism_holds() {
    // E = 2 absorbs 3 units with no useful gain; κ = 2 makes the requested
    // toxicity 2(2 × 1 + 1 × 0.5) = 5.
    let mut organism = budget(2.0);
    let settlement =
        settle_efficiencies(2.0, &[(2.0, -1.0), (1.0, -0.5)], &mut organism).expect("settle");
    assert_eq!(settlement.absorbed, 3.0);
    assert_eq!(settlement.gain, 0.0);
    assert_eq!(settlement.toxicity.requested, 5.0);
    assert_eq!(settlement.toxicity.paid, 2.0);
    assert_eq!(settlement.toxicity.shortfall(), 3.0);
    assert_eq!(settlement.net(), -2.0);
    assert_eq!(settlement.dissipated(), 5.0);
    assert!(organism.is_exhausted());
    assert_eq!(organism.remaining(), 0.0);

    // The field loses 3 and the organism 2: dissipation is 5, exactly.
    let mut ledger = TickLedger::open(Tick::new(0), holdings(3.0, 2.0), ResidualBudget::EXACT);
    settlement.record(&mut ledger).expect("record");
    let closure = ledger.close(holdings(0.0, 0.0)).expect("closes exactly");
    assert_eq!(closure.flow(Flow::UptakeInefficiency), 3.0);
    assert_eq!(closure.flow(Flow::Toxicity), 2.0);
    assert_eq!(closure.dissipated(), 5.0);
    assert_eq!(closure.residual(), 0.0);

    // Recording the requested toxicity would dissipate 8 and cannot close.
    let mut wrong = TickLedger::open(Tick::new(0), holdings(3.0, 2.0), ResidualBudget::EXACT);
    wrong.transfer(Transfer::Uptake, 3.0, 0.0).expect("uptake");
    wrong
        .record(Flow::Toxicity, settlement.toxicity.requested)
        .expect("record");
    assert!(matches!(
        wrong.close(holdings(0.0, 0.0)),
        Err(CoreError::NumericalInvalid { .. })
    ));

    // Gain is absorbed first, so toxicity is bounded by E + G: here
    // G = 0.5 and T_req = 1.5 × 2 = 3, but only 1 + 0.5 is there to pay.
    let mut organism = budget(1.0);
    let settlement =
        settle_efficiencies(1.5, &[(1.0, 0.5), (2.0, -1.0)], &mut organism).expect("settle");
    assert_eq!(settlement.gain, 0.5);
    assert_eq!(settlement.toxicity.requested, 3.0);
    assert_eq!(settlement.toxicity.paid, 1.5);
    assert_eq!(settlement.net(), -1.0);
    assert_eq!(settlement.dissipated(), 4.0);
    assert!(organism.is_exhausted());
    let mut ledger = TickLedger::open(Tick::new(1), holdings(3.0, 1.0), ResidualBudget::EXACT);
    settlement.record(&mut ledger).expect("record");
    let closure = ledger.close(holdings(0.0, 0.0)).expect("closes exactly");
    assert_eq!(closure.credited(Transfer::Uptake), 0.5);
    assert_eq!(closure.dissipated(), 4.0);
}

#[test]
fn settlements_refuse_invalid_packets_and_dead_organisms() {
    let invalid = |packets: &[(f64, f64)], kappa: f64| {
        matches!(
            settle_efficiencies(kappa, packets, &mut budget(1.0)),
            Err(CoreError::NumericalInvalid { .. })
        )
    };
    assert!(invalid(&[(-1.0, 0.5)], 0.2));
    assert!(invalid(&[(f64::NAN, 0.5)], 0.2));
    assert!(invalid(&[(1.0, 1.5)], 0.2));
    assert!(invalid(&[(1.0, f64::NAN)], 0.2));
    assert!(invalid(&[(1.0, 0.5)], 0.0));
    assert!(invalid(&[(1.0, 0.5)], f64::INFINITY));

    // An exhausted organism cannot absorb a gain, and harmful packets cost
    // it nothing more.
    let mut dead = budget(0.0);
    assert!(dead.is_exhausted());
    assert!(matches!(
        settle_efficiencies(0.2, &[(1.0, 0.5)], &mut dead),
        Err(CoreError::StateConflict { .. })
    ));
    let settlement = settle_efficiencies(0.2, &[(1.0, -0.5)], &mut dead).expect("settle");
    assert_eq!(settlement.toxicity.requested, 0.1);
    assert_eq!(settlement.toxicity.paid, 0.0);
    assert_eq!(dead.remaining(), 0.0);

    // Constants outside their ranges.
    for constants in [
        [0.0, 4.0, 0.15, 0.2, 1.0],
        [1.1, 4.0, 0.15, 0.2, 1.0],
        [0.8, -1.0, 0.15, 0.2, 1.0],
        [0.8, 4.0, 0.0, 0.2, 1.0],
        [0.8, 4.0, 0.15, 0.0, 1.0],
        [0.8, 4.0, 0.15, 0.2, 0.0],
        [0.8, 4.0, 0.15, 0.2, 1.5],
        [0.8, f64::NAN, 0.15, 0.2, 1.0],
    ] {
        let [eta, gain, threshold, kappa, limit] = constants;
        assert!(
            matches!(
                Chemistry::new(eta, gain, threshold, kappa, limit),
                Err(CoreError::NumericalInvalid { .. })
            ),
            "{constants:?}"
        );
    }

    // Affinity norms are checked against the run's limit in F64.
    let chemistry = c9_chemistry();
    let mut quarter = [0.0; CHANNELS];
    quarter[..4].copy_from_slice(&[0.5; 4]);
    assert!(Affinity::new(quarter, &chemistry).is_ok());
    let mut over = quarter;
    over[4] = 0.5;
    assert!(matches!(
        Affinity::new(over, &chemistry),
        Err(CoreError::NumericalInvalid { .. })
    ));
    let mut nan = quarter;
    nan[7] = f64::NAN;
    assert!(Affinity::new(nan, &chemistry).is_err());
    let narrow = Chemistry::new(0.8, 4.0, 0.15, 0.2, 0.5).expect("chemistry");
    assert!(Affinity::new(unit(0), &narrow).is_err());
    assert!(Affinity::new(unit(0).map(|c| c * 0.5), &narrow).is_ok());
}

#[test]
fn c9_chemistry_is_evaluated_per_packet_not_on_the_mixture() {
    let chemistry = c9_chemistry();
    let a = unit(3);
    let affinity = Affinity::new(a, &chemistry).expect("affinity");
    let packets = [
        ChemicalPacket {
            granted: 1.0,
            composition: a,
        },
        ChemicalPacket {
            granted: 1.0,
            composition: a.map(|c| -c),
        },
    ];
    let mut organism = budget(10.0);
    let per_packet = chemistry
        .settle(&affinity, &packets, &mut organism)
        .expect("settle");
    assert_eq!(per_packet.absorbed, 2.0);
    assert!(
        (per_packet.net() - 0.639_355_310_069).abs() < 1.0e-12,
        "{}",
        per_packet.net()
    );
    // η(+a) = 0.8 tanh(3.85) and η(−a) = 0.8 tanh(−4.15), separately.
    let good = chemistry.efficiency(&affinity, &a).expect("η");
    let bad = chemistry.efficiency(&affinity, &a.map(|c| -c)).expect("η");
    assert_eq!(per_packet.gain, good);
    assert_eq!(per_packet.toxicity.requested, 0.2 * -bad);
    assert!(per_packet.toxicity.is_full());
    assert!((organism.remaining() - (10.0 + per_packet.net())).abs() <= 2.0e-15);

    // The same rule applied once to the zero mean composition is wrong.
    let mixture = [ChemicalPacket {
        granted: 2.0,
        composition: [0.0; CHANNELS],
    }];
    let mixed = chemistry
        .settle(&affinity, &mixture, &mut budget(10.0))
        .expect("settle");
    assert!(
        (mixed.net() - -0.047_643_210_759).abs() < 1.0e-12,
        "{}",
        mixed.net()
    );

    // More packets than the stack buffer holds take the same arithmetic.
    let split: Vec<ChemicalPacket> = [0.25, 0.25, 0.25, 0.25]
        .into_iter()
        .map(|granted| ChemicalPacket {
            granted,
            composition: a,
        })
        .chain([0.5, 0.5].into_iter().map(|granted| ChemicalPacket {
            granted,
            composition: a.map(|c| -c),
        }))
        .collect();
    let six = chemistry
        .settle(&affinity, &split, &mut budget(10.0))
        .expect("settle");
    assert_eq!(six.absorbed, 2.0);
    assert!((six.net() - per_packet.net()).abs() <= 2.0 * f64::EPSILON);
}

#[test]
fn c3_composition_is_read_from_the_snapshot_and_neutral_cells_are_mildly_harmful() {
    let grid = FieldGrid::new(4, 4, 4.0, 4.0).expect("grid");
    let mut field = FieldState::zeroed(grid, &BufferLimits::host()).expect("field");
    // Packets (3, 3a) and (5, −5a) make e = 8 and m = −2a.
    let a = unit(2);
    let deposit = |energy: f32, direction: f64| Cell {
        energy,
        moments: a.map(|c| (f64::from(energy) * direction * c) as f32),
    };
    let first = deposit(3.0, 1.0);
    let second = deposit(5.0, -1.0);
    let mut combined = Cell::EMPTY;
    combined.energy = first.energy + second.energy;
    for ((slot, x), y) in combined
        .moments
        .iter_mut()
        .zip(first.moments)
        .zip(second.moments)
    {
        *slot = x + y;
    }
    field.set_cell(1, 1, &combined).expect("cell");
    // Two equal opposite packets: m = 0 while e > 0.
    let mut cancelled = Cell::EMPTY;
    cancelled.energy = 6.0;
    field.set_cell(2, 2, &cancelled).expect("cell");

    let index = |x, y| grid.cell_index(x, y).unwrap_or(usize::MAX);
    let z = composition(&field, index(1, 1)).expect("z");
    assert_eq!(z, a.map(|c| -0.25 * c));
    assert_eq!(
        composition(&field, index(2, 2)).expect("z"),
        [0.0; CHANNELS]
    );
    // An empty cell is neutral too, not a division by zero.
    assert_eq!(
        composition(&field, index(3, 3)).expect("z"),
        [0.0; CHANNELS]
    );
    assert!(matches!(
        composition(&field, 16),
        Err(CoreError::LayoutMismatch { .. })
    ));

    // With b > 0 the neutral composition is mildly harmful, by the rule
    // itself: η(0) = η_max tanh(−b).
    let chemistry = c9_chemistry();
    let affinity = Affinity::new(a, &chemistry).expect("affinity");
    let neutral = chemistry
        .efficiency(&affinity, &[0.0; CHANNELS])
        .expect("η");
    assert!(neutral < 0.0 && neutral > -0.12, "{neutral}");
    // −2a against affinity a: k a·z − b = 4(−0.25) − 0.15.
    let opposed = chemistry.efficiency(&affinity, &z).expect("η");
    assert!((opposed - 0.8 * (-1.15_f64).tanh()).abs() <= 4.0 * f64::EPSILON);
}

#[test]
fn c11_drag_uses_a_stable_expm1_with_an_exact_zero_rate_limit() {
    let dt = step(1, 128);
    let motion = Motion::new(dt, 1.0e-6, 4.0, 4.0, 0.002, 1.0).expect("motion");
    assert!(
        (motion.force_coefficient() - 0.007_812_499_969_48).abs() < 1.0e-14,
        "{}",
        motion.force_coefficient()
    );
    assert!(motion.decay() < 1.0);
    assert!((1.0 - motion.decay() - 7.812_5e-9).abs() < 2.5e-16);

    // γ = 0 is the exact limit, and a vanishing γ reaches it continuously.
    for drag in [0.0, 1.0e-300] {
        let motion = Motion::new(dt, drag, 4.0, 4.0, 0.002, 1.0).expect("motion");
        assert_eq!(motion.force_coefficient(), 1.0 / 128.0, "γ = {drag}");
        assert_eq!(motion.decay(), 1.0, "γ = {drag}");
    }

    // The coefficient falls as drag rises and never exceeds Δt.
    let mut previous = f64::INFINITY;
    for drag in [0.0, 1.0e-9, 1.0e-6, 1.0e-3, 0.5, 2.0, 64.0, 1.0e4] {
        let motion = Motion::new(dt, drag, 4.0, 4.0, 0.002, 1.0).expect("motion");
        let coefficient = motion.force_coefficient();
        assert!(
            coefficient <= previous && coefficient <= 1.0 / 128.0,
            "γ = {drag}"
        );
        previous = coefficient;
    }

    // Appendix A: γ = 2 s⁻¹ at Δt = 1/128 s.
    let motion = Motion::from_config(&appendix_a()).expect("motion");
    let x = 2.0_f64 / 128.0;
    assert!((motion.decay() - (-x).exp()).abs() <= 2.0 * f64::EPSILON);
    let reference = -(-x).exp_m1() / 2.0;
    assert!((motion.force_coefficient() - reference).abs() <= 4.0 * f64::EPSILON * reference);

    for (drag, speed, acceleration, cost, limit) in [
        (-1.0, 4.0, 4.0, 0.002, 1.0),
        (2.0, 0.0, 4.0, 0.002, 1.0),
        (2.0, 4.0, 0.0, 0.002, 1.0),
        (2.0, 4.0, 4.0, -0.002, 1.0),
        (2.0, 4.0, 4.0, 0.002, 0.0),
        (f64::NAN, 4.0, 4.0, 0.002, 1.0),
    ] {
        assert!(
            matches!(
                Motion::new(dt, drag, speed, acceleration, cost, limit),
                Err(CoreError::NumericalInvalid { .. })
            ),
            "{drag} {speed} {acceleration} {cost} {limit}"
        );
    }
}

#[test]
fn c12_motion_spends_the_last_unit_and_still_moves() {
    // E = 1, speed 1, c_v = 1, Δt = 1; cells wide enough for the step.
    let motion = unit_step_motion(1.0);
    let grid = FieldGrid::new(4, 4, 64.0, 64.0).expect("grid");
    let mut organism = budget(1.0);
    let proposed = motion.propose([1.0, 0.0], [0.0, 0.0]).expect("propose");
    assert_eq!(proposed, [1.0, 0.0]);
    let settlement = motion.settle(proposed, &mut organism).expect("settle");
    assert_eq!(settlement.cost.requested, 1.0);
    assert_eq!(settlement.cost.paid, 1.0);
    assert_eq!(settlement.velocity, [1.0, 0.0]);
    assert!(!settlement.scaled);
    assert!(organism.is_exhausted());
    // It moves by what it paid for.
    assert_eq!(
        motion
            .displace(&grid, [10.0, 20.0], settlement.velocity)
            .expect("displace"),
        [11.0, 20.0]
    );

    // The ledger records exactly one dissipated unit.
    let mut ledger = TickLedger::open(Tick::new(0), holdings(0.0, 1.0), ResidualBudget::EXACT);
    ledger
        .record(Flow::MotionCost, settlement.cost.paid)
        .expect("record");
    let closure = ledger.close(holdings(0.0, 0.0)).expect("closes exactly");
    assert_eq!(closure.dissipated(), 1.0);
    assert_eq!(closure.flow(Flow::MotionCost), 1.0);

    // Every later stage finds it exhausted: no emission, no split, no
    // absorption, and no further motion.
    assert_eq!(organism.pay(0.05).expect("pay").paid, 0.0);
    assert!(!organism.spend(0.05).expect("spend"));
    assert!(matches!(
        organism.credit(0.1),
        Err(CoreError::StateConflict { .. })
    ));
    let again = motion.settle([1.0, 0.0], &mut organism).expect("settle");
    assert_eq!(again.velocity, [0.0, 0.0]);
    assert_eq!(again.cost.paid, 0.0);
    assert!(again.scaled);
}

#[test]
fn unaffordable_motion_scales_the_whole_velocity() {
    let motion = unit_step_motion(1.0);
    // An opposing force shrinks the proposal from 3 to 2, which still costs
    // 4. Even with no force at all the residual velocity would cost 9, so
    // scaling only the active force could never make it affordable.
    let proposed = motion.propose([3.0, 0.0], [-1.0, 0.0]).expect("propose");
    assert_eq!(proposed, [2.0, 0.0]);
    let residual_only = motion
        .settle([3.0, 0.0], &mut budget(10.0))
        .expect("settle");
    assert_eq!(residual_only.cost.requested, 9.0);
    let mut organism = budget(1.0);
    let settlement = motion.settle(proposed, &mut organism).expect("settle");
    assert!(settlement.scaled);
    assert_eq!(settlement.proposed, [2.0, 0.0]);
    assert_eq!(settlement.velocity, [1.0, 0.0]);
    assert_eq!(settlement.cost.requested, 4.0);
    assert_eq!(settlement.cost.paid, 1.0);
    assert!(organism.is_exhausted());

    // Affordable motion is paid in full and left alone.
    let mut rich = budget(10.0);
    let settlement = motion.settle(proposed, &mut rich).expect("settle");
    assert!(!settlement.scaled && settlement.cost.is_full());
    assert_eq!(settlement.velocity, proposed);
    assert_eq!(rich.remaining(), 6.0);

    // A non-dyadic case: the applied velocity's cost matches what was paid.
    let mut poor = budget(0.3);
    let settlement = motion.settle([1.5, 2.0], &mut poor).expect("settle");
    assert!(settlement.scaled);
    assert_eq!(settlement.cost.paid, 0.3);
    assert!((squared(settlement.velocity) - 0.3).abs() <= 4.0 * f64::EPSILON);
    let direction = settlement.velocity[0] / settlement.velocity[1];
    assert!((direction - 0.75).abs() <= 2.0 * f64::EPSILON);

    assert!(motion.settle([f64::NAN, 0.0], &mut budget(1.0)).is_err());
    assert!(motion.propose([f64::INFINITY, 0.0], [0.0, 0.0]).is_err());
}

#[test]
fn speed_and_acceleration_bounds_hold_as_computed() {
    let motion = Motion::new(step(1, 128), 2.0, 4.0, 4.0, 0.002, 1.0).expect("motion");
    let mut coefficients = [0.0; RESPONSE_TERMS];
    coefficients[..4].copy_from_slice(&[0.5; 4]);
    let response = MotionResponse::new(coefficients, &motion).expect("response");
    assert_eq!(response.coefficients(), &coefficients);

    let mut gradients = [[0.0; 2]; RESPONSE_TERMS];
    gradients[0] = [600.0, 800.0];
    // No uptake, no chemotaxis.
    assert_eq!(
        motion
            .acceleration(0.0, &response, &gradients)
            .expect("acceleration"),
        [0.0, 0.0]
    );
    let limited = motion
        .acceleration(1.0, &response, &gradients)
        .expect("acceleration");
    assert!(squared(limited) <= 16.0 && squared(limited) >= 16.0 * (1.0 - 8.0 * f64::EPSILON));
    // A small response is not clamped: 0.5 × 0.5 × 1 in x.
    let mut gentle = [[0.0; 2]; RESPONSE_TERMS];
    gentle[0] = [1.0, 0.0];
    assert_eq!(
        motion
            .acceleration(0.5, &response, &gentle)
            .expect("acceleration"),
        [0.25, 0.0]
    );

    // Random directions and magnitudes never escape either bound.
    let mut state = 0x00f1_0006_u64;
    for _ in 0..20_000 {
        let mut gradients = [[0.0; 2]; RESPONSE_TERMS];
        for gradient in &mut gradients {
            *gradient = [symmetric(&mut state, 40.0), symmetric(&mut state, 40.0)];
        }
        let ratio = (splitmix(&mut state) >> 11) as f64 / (1_u64 << 53) as f64;
        let acceleration = motion
            .acceleration(ratio, &response, &gradients)
            .expect("acceleration");
        assert!(squared(acceleration) <= 16.0, "{acceleration:?}");
        let velocity = [symmetric(&mut state, 6.0), symmetric(&mut state, 6.0)];
        let proposed = motion.propose(velocity, acceleration).expect("propose");
        assert!(squared(proposed) <= 16.0, "{proposed:?}");
    }

    let mut nonfinite = [[0.0; 2]; RESPONSE_TERMS];
    nonfinite[9] = [f64::NAN, 0.0];
    assert!(motion.acceleration(1.0, &response, &nonfinite).is_err());
    assert!(motion.acceleration(1.5, &response, &gradients).is_err());
    assert!(motion.acceleration(-0.0001, &response, &gradients).is_err());

    // The response shares one norm budget.
    let mut over = coefficients;
    over[4] = 0.5;
    assert!(matches!(
        MotionResponse::new(over, &motion),
        Err(CoreError::NumericalInvalid { .. })
    ));
    over[4] = f64::NAN;
    assert!(MotionResponse::new(over, &motion).is_err());
}

#[test]
fn uptake_ratio_and_displacement_follow_the_ordered_rule() {
    let dt = step(1, 128);
    // 0.4/256 of a 0.4/128 capacity is exactly half.
    assert_eq!(uptake_ratio(0.4 / 256.0, dt, 0.4).expect("ratio"), 0.5);
    // Whole-quantum grants can round past capacity; the ratio is clamped.
    assert_eq!(uptake_ratio(1.0, dt, 0.4).expect("ratio"), 1.0);
    assert_eq!(uptake_ratio(0.0, dt, 0.4).expect("ratio"), 0.0);
    assert_eq!(uptake_ratio(1.0, dt, 0.0).expect("ratio"), 0.0);
    assert!(uptake_ratio(-1.0, dt, 0.4).is_err());
    assert!(uptake_ratio(1.0, dt, f64::NAN).is_err());

    let motion = unit_step_motion(0.0);
    let grid = FieldGrid::new(16, 16, 16.0, 16.0).expect("grid");
    // x + Δt v wraps on the torus.
    assert_eq!(
        motion
            .displace(&grid, [15.5, 0.25], [1.0, -0.5])
            .expect("displace"),
        [0.5, 15.75]
    );
    // Free motion costs nothing.
    let mut organism = budget(1.0);
    let settlement = motion.settle([3.0, 1.0], &mut organism).expect("settle");
    assert_eq!(settlement.cost.paid, 0.0);
    assert_eq!(organism.remaining(), 1.0);
}

#[test]
fn maintenance_charges_canonical_model_work_only() {
    let config = appendix_a();
    let parameters = u32::try_from(config.report().default_parameter_count).expect("count");
    assert_eq!(parameters, 9_988);
    let costs = MaintenanceCosts::from_config(&config).expect("costs");
    let dt = config.report().timestep;
    // §5.6: 0.005 + 0.001 + 9988 × 128 × 2e−9 per simulation-second at
    // unit sensing scale, evaluated every tick.
    let per_tick = costs.mandatory(dt, 1.0, parameters, true).expect("cost");
    assert!(
        (per_tick * 128.0 - 0.008_556_928).abs() < 1.0e-15,
        "{per_tick}"
    );
    // A tick without an evaluation pays only basal and sensor costs.
    let idle = costs.mandatory(dt, 1.0, parameters, false).expect("cost");
    assert!((idle * 128.0 - 0.006).abs() < 1.0e-17, "{idle}");
    assert_eq!(costs.mandatory(dt, 1.0, 0, true).expect("cost"), idle);
    // The sensor cost is linear in the sensing scale (α = 1).
    let wide = costs.mandatory(dt, 3.0, parameters, false).expect("cost");
    assert!((wide * 128.0 - 0.008).abs() < 1.0e-17, "{wide}");

    for scale in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert!(matches!(
            costs.mandatory(dt, scale, parameters, true),
            Err(CoreError::NumericalInvalid { .. })
        ));
    }
    assert!(matches!(
        MaintenanceCosts::new(0.005, 0.001, 1.0, 1.5, 2.0e-9),
        Err(CoreError::MissingCapability { .. })
    ));
    for [basal, sensor, unit_scale, neural] in [
        [-0.005, 0.001, 1.0, 2.0e-9],
        [0.005, -0.001, 1.0, 2.0e-9],
        [0.005, 0.001, 0.0, 2.0e-9],
        [0.005, 0.001, 1.0, -2.0e-9],
        [f64::NAN, 0.001, 1.0, 2.0e-9],
    ] {
        assert!(matches!(
            MaintenanceCosts::new(basal, sensor, unit_scale, 1.0, neural),
            Err(CoreError::NumericalInvalid { .. })
        ));
    }

    // The Appendix A chemistry matches the constructor.
    let chemistry = Chemistry::from_config(&config).expect("chemistry");
    assert_eq!(
        chemistry,
        Chemistry::new(0.8, 4.0, 0.15, 0.2, 1.0).expect("chemistry")
    );
}

#[test]
fn budget_stages_follow_section_8_2() {
    // Gain is absorbed before maintenance: E = 0.001 plus G = 0.01 pays a
    // maintenance of 0.005 in full. Paid first, the same bill would have
    // killed it.
    let mut organism = budget(0.001);
    let gain = settle_efficiencies(0.2, &[(1.0, 0.01)], &mut organism).expect("settle");
    assert_eq!(gain.gain, 0.01);
    let maintenance = organism.pay(0.005).expect("pay");
    assert!(maintenance.is_full() && !organism.is_exhausted());
    let mut backwards = budget(0.001);
    assert!(!backwards.pay(0.005).expect("pay").is_full());
    assert!(backwards.is_exhausted());

    // Unpaid maintenance is not dissipated energy.
    let mut organism = budget(0.002);
    let maintenance = organism.pay(0.005).expect("pay");
    assert_eq!(maintenance.paid, 0.002);
    assert!((maintenance.shortfall() - 0.003).abs() < 1.0e-18);
    assert!(organism.is_exhausted());
    let mut ledger = TickLedger::open(Tick::new(0), holdings(0.0, 0.002), ResidualBudget::EXACT);
    ledger
        .record(Flow::Maintenance, maintenance.paid)
        .expect("record");
    ledger.close(holdings(0.0, 0.0)).expect("paid cost closes");
    let mut wrong = TickLedger::open(Tick::new(0), holdings(0.0, 0.002), ResidualBudget::EXACT);
    wrong
        .record(Flow::Maintenance, maintenance.requested)
        .expect("record");
    assert!(wrong.close(holdings(0.0, 0.0)).is_err());

    // Death at maintenance prohibits motion: nothing is left to pay for it.
    let motion = unit_step_motion(1.0);
    let settlement = motion.settle([1.0, 1.0], &mut organism).expect("settle");
    assert_eq!(settlement.velocity, [0.0, 0.0]);
    assert_eq!(settlement.cost.paid, 0.0);
}

/// One organism in the integrated fixture.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Organism {
    id: OrganismId,
    position: [f64; 2],
    velocity: [f64; 2],
    affinity: Affinity,
    budget: EnergyBudget,
}

/// What the integrated fixture observed.
#[derive(Debug, Clone, PartialEq)]
struct Outcome {
    organisms: Vec<Organism>,
    field: Vec<u32>,
    report: LedgerReport,
    contended_cells: usize,
    deaths: Vec<(OrganismId, u64)>,
    granted_after_death: usize,
    largest_speed_squared: f64,
}

fn organism_total(organisms: &[Organism]) -> f64 {
    let mut total = CompensatedSum::ZERO;
    for organism in organisms {
        total.add(organism.budget.remaining());
    }
    total.value()
}

/// Five organisms on a 16 × 16 field for `ticks` ticks of Appendix A
/// constants. `order` is the order in which they are presented; the result
/// must not depend on it.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn integrated(order: &[usize], ticks: u64) -> Outcome {
    let config = appendix_a();
    let dt = config.report().timestep;
    let chemistry = Chemistry::from_config(&config).expect("chemistry");
    let costs = MaintenanceCosts::from_config(&config).expect("costs");
    let motion = Motion::from_config(&config).expect("motion");
    let capacity = config.config().chemistry.uptake_capacity_per_second;
    let half_saturation = config.config().chemistry.uptake_half_saturation_density;
    let parameters = u32::try_from(config.report().default_parameter_count).expect("count");

    let grid = FieldGrid::new(16, 16, 16.0, 16.0).expect("grid");
    let mut field = FieldState::zeroed(grid, &BufferLimits::host()).expect("field");
    for y in 0..16_u32 {
        for x in 0..16_u32 {
            // Sparse nearly empty cells make contention; the rest alternate
            // between a +channel-0 and a channel-1 composition.
            let energy: f32 = if (5 * x + 3 * y) % 7 == 0 {
                0.0004
            } else {
                0.05 + 0.01 * ((7 * x + 3 * y) % 5) as f32
            };
            let (channel, purity) = if (x + y) % 2 == 0 { (0, 0.9) } else { (1, 0.5) };
            let moments = unit(channel).map(|c| (f64::from(energy) * purity * c) as f32);
            field
                .set_cell(x, y, &Cell { energy, moments })
                .expect("cell");
        }
    }

    let start: [(u64, [f64; 2], usize, f64, f64); 5] = [
        (17, [4.3, 4.6], 0, 1.0, 0.02),
        (3, [4.8, 4.2], 1, 1.0, 0.5),
        // Opposed to the dominant composition: harmed until it dies.
        (42, [5.1, 5.4], 0, -1.0, 0.004),
        (8, [9.7, 11.2], 0, 1.0, 0.1),
        (25, [4.5, 5.0], 1, 0.7, 0.03),
    ];
    let mut organisms: Vec<Organism> = start
        .iter()
        .map(|&(id, position, channel, sign, energy)| Organism {
            id: OrganismId::new(id),
            position,
            velocity: [0.0, 0.0],
            affinity: Affinity::new(unit(channel).map(|c| c * sign), &chemistry).expect("affinity"),
            budget: budget(energy),
        })
        .collect();
    let mut coefficients = [0.0; RESPONSE_TERMS];
    coefficients[..4].copy_from_slice(&[0.5; 4]);
    let response = MotionResponse::new(coefficients, &motion).expect("response");
    let mut gradients = [[0.0; 2]; RESPONSE_TERMS];
    gradients[0] = [6.0, -3.0];
    gradients[1] = [2.0, 4.0];
    gradients[2] = [-1.0, 1.0];

    let initial = holdings(field.energy_total(), organism_total(&organisms));
    let mut run = EnergyLedger::new(
        initial,
        ResidualBudget::new(0.0, 1.0e-12, 1.0e-12).expect("budget"),
    );
    let mut grants = Grants::new();
    let mut demands = Vec::new();
    let mut absorbed = Vec::new();
    let mut packets = Vec::new();
    let mut outcome = Outcome {
        organisms: Vec::new(),
        field: Vec::new(),
        report: run.report(),
        contended_cells: 0,
        deaths: Vec::new(),
        granted_after_death: 0,
        largest_speed_squared: 0.0,
    };

    for tick in 0..ticks {
        let opening = holdings(field.energy_total(), organism_total(&organisms));
        let mut ledger = run.open(Tick::new(tick), opening).expect("open");

        // Phase 5: requests from the observation snapshot, resolved per cell.
        demands.clear();
        for &index in order {
            let organism = organisms.get(index).expect("organism");
            if organism.budget.is_exhausted() {
                continue;
            }
            let density = ExchangeStencil::at(&grid, organism.position)
                .and_then(|stencil| stencil.density(&field))
                .expect("density");
            demands.push(Demand {
                organism: organism.id,
                position: organism.position,
                amount: saturating_request(dt, capacity, half_saturation, density)
                    .expect("request"),
            });
        }
        let summary = resolve_uptake(&field, &demands, &mut grants).expect("resolve");
        outcome.contended_cells += summary.contended_cells;
        for (id, _) in &outcome.deaths {
            outcome.granted_after_death += grants
                .packets()
                .iter()
                .filter(|packet| packet.organism == *id)
                .count();
        }

        // Phase 6: per-packet chemistry on the snapshot, then maintenance.
        absorbed.clear();
        for (index, demand) in demands.iter().enumerate() {
            let organism = organisms
                .iter_mut()
                .find(|organism| organism.id == demand.organism)
                .expect("demanding organism");
            packets.clear();
            for packet in grants.packets().iter().filter(|p| p.demand == index) {
                packets.push(ChemicalPacket {
                    granted: packet.granted,
                    composition: composition(&field, packet.cell).expect("z"),
                });
            }
            let settlement = chemistry
                .settle(&organism.affinity, &packets, &mut organism.budget)
                .expect("settle");
            settlement.record(&mut ledger).expect("record");
            let bill = costs
                .mandatory(dt, 1.0, parameters, true)
                .expect("maintenance");
            let paid = organism.budget.pay(bill).expect("pay");
            ledger.record(Flow::Maintenance, paid.paid).expect("record");
            absorbed.push((organism.id, settlement.absorbed));
        }

        // Phase 7: affordable motion for the survivors.
        for &(id, uptake) in &absorbed {
            let organism = organisms
                .iter_mut()
                .find(|organism| organism.id == id)
                .expect("organism");
            if organism.budget.is_exhausted() {
                outcome.deaths.push((id, tick));
                continue;
            }
            let ratio = uptake_ratio(uptake, dt, capacity).expect("ratio");
            let acceleration = motion
                .acceleration(ratio, &response, &gradients)
                .expect("acceleration");
            let proposed = motion
                .propose(organism.velocity, acceleration)
                .expect("propose");
            let settlement = motion
                .settle(proposed, &mut organism.budget)
                .expect("settle");
            ledger
                .record(Flow::MotionCost, settlement.cost.paid)
                .expect("record");
            organism.velocity = settlement.velocity;
            organism.position = motion
                .displace(&grid, organism.position, settlement.velocity)
                .expect("displace");
            outcome.largest_speed_squared = outcome
                .largest_speed_squared
                .max(squared(settlement.velocity));
            if organism.budget.is_exhausted() {
                outcome.deaths.push((id, tick));
            }
        }

        // Phase 9 for uptake: the debits reach the next field.
        apply_grants(&mut field, &mut grants).expect("apply");
        let closing = holdings(field.energy_total(), organism_total(&organisms));
        let closure = ledger.close(closing).expect("tick closes");
        outcome.report = run.commit(&closure).expect("run closes");
    }
    organisms.sort_by_key(|organism| organism.id);
    outcome.deaths.sort_unstable();
    outcome.organisms = organisms;
    outcome.field = field.energy().iter().map(|e| e.to_bits()).collect();
    outcome
}

/// FNV-1a over everything the fixture produced, as a cross-host guard.
fn digest(outcome: &Outcome) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut eat = |word: u64| {
        for byte in word.to_le_bytes() {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
    };
    for organism in &outcome.organisms {
        eat(organism.id.get());
        eat(organism.budget.remaining().to_bits());
        for value in organism.position.iter().chain(&organism.velocity) {
            eat(value.to_bits());
        }
    }
    for &bits in &outcome.field {
        eat(u64::from(bits));
    }
    hash
}

#[test]
fn organisms_settle_contended_uptake_and_close_the_ledger_every_tick() {
    let ticks = 128;
    let outcome = integrated(&[0, 1, 2, 3, 4], ticks);
    let report = outcome.report;
    assert_eq!(report.ticks, ticks);
    assert!(outcome.contended_cells > 0, "no contention was exercised");
    // A simulated second of uptake, chemistry, maintenance and motion closes
    // to rounding: the field side is exact, the organism side rounds down.
    let normalized = report.normalized.expect("turnover");
    assert!(normalized.abs() < 1.0e-13, "{report:?}");
    assert!(report.dissipated > 0.0 && report.injected == 0.0);
    assert!(report.current < report.initial);

    // The opposed organism is harmed until it dies, and once dead it never
    // demands or absorbs again.
    let deaths: Vec<OrganismId> = outcome.deaths.iter().map(|&(id, _)| id).collect();
    assert_eq!(deaths, [OrganismId::new(42)], "{:?}", outcome.deaths);
    assert_eq!(outcome.granted_after_death, 0);
    let dead = outcome
        .organisms
        .iter()
        .find(|o| o.id == OrganismId::new(42))
        .expect("organism 42");
    assert_eq!(dead.budget.remaining(), 0.0);
    // The well-matched organisms gain.
    for (id, initial) in [(17, 0.02), (3, 0.5), (8, 0.1)] {
        let organism = outcome
            .organisms
            .iter()
            .find(|o| o.id == OrganismId::new(id))
            .expect("organism");
        assert!(organism.budget.remaining() > initial, "{organism:?}");
    }
    assert!(outcome.largest_speed_squared <= 16.0 && outcome.largest_speed_squared > 0.0);
    for organism in &outcome.organisms {
        let [x, y] = organism.position;
        assert!((0.0..16.0).contains(&x) && (0.0..16.0).contains(&y));
    }
}

#[test]
fn the_organism_phase_does_not_depend_on_presentation_order() {
    let ticks = 48;
    let forward = integrated(&[0, 1, 2, 3, 4], ticks);
    let reversed = integrated(&[4, 3, 2, 1, 0], ticks);
    let shuffled = integrated(&[2, 0, 4, 1, 3], ticks);
    // Organisms and field agree bit for bit. The ledger's flow sums may
    // round differently in their last bits, but each run closes.
    for other in [&reversed, &shuffled] {
        assert_eq!(forward.organisms, other.organisms);
        assert_eq!(forward.field, other.field);
        assert_eq!(forward.deaths, other.deaths);
        assert_eq!(digest(&forward), digest(other));
    }
    // Cross-host guard: pinned on macOS arm64 and checked again on Linux
    // x86_64. Every operator here uses only IEEE-754 arithmetic and the
    // host-independent `flife_core::numerics`, so the bits must agree.
    assert_eq!(digest(&forward), 0x770d_6440_241c_d51e);
    assert_eq!(forward.deaths, [(OrganismId::new(42), 29)]);
}
