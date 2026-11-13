//! FL-011: paid emission and the reproduction transaction (plan §5.5, §7.4,
//! §7.5, §8.1 phases 7–9, §8.2, INV-03, INV-05, INV-06, A.1, A.3 item 10,
//! Appendix C.4, C.5).
//!
//! Exact split and deposit accounting against the ledger, deposits that are
//! first sensed on the next tick, viability on the post-emission budget,
//! early fixed logical quotas, and all-or-nothing rollback. The pinned
//! attempt and offset come from NumPy 2.5.4's `Philox` and 80-digit Python
//! `Decimal`, in an oracle that imports no FrankenLife code.

use std::collections::BTreeMap;

use flife_core::ids::{Generation, GenomeId, IdSequence, LineageId, OrganismId};
use flife_core::ledger::{CompensatedSum, Transfer};
use flife_core::rng::SemanticRng;
use flife_core::{
    CoreError, EnergyBudget, EnergyLedger, Flow, Holdings, ResidualBudget, Tick, TickSpan,
    ValidatedConfig,
};
use flife_engine::{
    BirthContext, BirthIntent, BirthRequest, BirthResolution, Births, Capacity, EmissionPolicy,
    Identities, Motion, Offspring, ParentState, PopulationQuota, Reproduction,
    ReproductionProposal, ReproductionRules, Unviable, Viability, resolve_births,
};
use flife_field::sources::unit_direction;
use flife_field::state::CHANNELS;
use flife_field::{
    ConeTolerance, Demand, Deposits, ExchangeStencil, FieldGrid, FieldState, Grants,
    apply_deposits, apply_grants, resolve_uptake,
};
use flife_genome::{
    Genome, GenomeQuota, MAXIMUM_CANDIDATES, MutationParameters, Mutator, RejectionReason,
    TopologyCatalog, TraitLimits, founder, initial_scale,
};
use flife_neural::{ACTION_WIDTH, ActionDecoding, DecodedActions, Emission, decode};

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

fn appendix_a_toml() -> String {
    let appendix = PLAN.find("## Appendix A").map_or("", |at| &PLAN[at..]);
    let start = appendix
        .find("```toml\n")
        .map_or(0, |at| at + "```toml\n".len());
    let length = appendix[start..].find("```").unwrap_or(0);
    appendix[start..start + length].to_owned()
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn holdings(field: f64, organisms: f64) -> Holdings {
    Holdings::new(field, organisms).expect("holdings")
}

fn total(budgets: &[EnergyBudget]) -> f64 {
    let mut sum = CompensatedSum::ZERO;
    for budget in budgets {
        sum.add(budget.remaining());
    }
    sum.value()
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn budget(energy: f64) -> EnergyBudget {
    EnergyBudget::new(energy).expect("budget")
}

/// An `n × n` torus of unit cells, every cell holding `energy` and no
/// composition.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn field(n: u32, energy: f32) -> FieldState {
    let grid = FieldGrid::new(n, n, f64::from(n), f64::from(n)).expect("grid");
    let mut field = FieldState::zeroed(grid, &flife_field::BufferLimits::host()).expect("field");
    field.energy_mut().fill(energy);
    field
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn direction(components: &[f64]) -> [f64; CHANNELS] {
    let mut normals = [0.0; CHANNELS];
    normals[..components.len()].copy_from_slice(components);
    unit_direction(&normals).expect("a direction")
}

fn emission(components: &[f64], spend_fraction: f64) -> Emission {
    Emission {
        direction: direction(components),
        spend_fraction,
    }
}

fn moment(field: &FieldState, channel: u32, cell: usize) -> f32 {
    field
        .moment(channel)
        .and_then(|plane| plane.get(cell))
        .copied()
        .unwrap_or(f32::NAN)
}

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

fn ulps(a: f64, b: f64) -> u64 {
    let ordinal = |value: f64| {
        let bits = value.to_bits();
        let magnitude = i64::try_from(bits & 0x7fff_ffff_ffff_ffff).unwrap_or(i64::MAX);
        if bits >> 63 == 0 {
            magnitude
        } else {
            -magnitude
        }
    };
    ordinal(a).abs_diff(ordinal(b))
}

const SCALE_BITS: i32 = 100;

/// `value × 2^100` as an exact integer.
fn exact(value: f64) -> i128 {
    let scaled = value * 2.0_f64.powi(SCALE_BITS);
    assert_eq!(scaled.fract(), 0.0, "{value} has bits below 2^-100");
    assert!(scaled.abs() < 2.0_f64.powi(126), "{value} is too large");
    scaled as i128
}

// ---------------------------------------------------------------------------
// Paid emission and deposits.

#[test]
fn c4_paid_emission_charges_the_emitter_and_splits_between_field_and_dissipation() {
    // C.4: 4 units at efficiency 0.75: the field receives 3, dissipation 1,
    // and the organism's energy falls by 4. At a cell corner the four
    // bilinear weights are 1/4 each.
    let mut world = field(8, 1.0);
    let grid = *world.grid();
    let policy = EmissionPolicy::new(0.75).expect("policy");
    let mut organism = budget(10.0);
    let mut run = EnergyLedger::new(
        holdings(world.energy_total(), organism.remaining()),
        ResidualBudget::EXACT,
    );
    let mut ledger = run
        .open(
            Tick::new(0),
            holdings(world.energy_total(), organism.remaining()),
        )
        .expect("open");
    let mut deposits = Deposits::new();
    let a = direction(&[0.6, 0.8]);
    let settlement = policy
        .emit(
            &grid,
            OrganismId::new(1),
            [3.0, 5.0],
            Some(&emission(&[0.6, 0.8], 0.4)),
            &mut organism,
            &mut deposits,
        )
        .expect("emit");
    assert_eq!(settlement.available, 10.0);
    assert_eq!(
        (settlement.cost.requested, settlement.cost.paid),
        (4.0, 4.0)
    );
    assert_eq!(settlement.offered, 3.0);
    assert!(settlement.emitted());
    assert_eq!(organism.remaining(), 6.0);

    let summary = apply_deposits(&mut world, &mut deposits).expect("apply");
    assert_eq!(
        (
            summary.organisms,
            summary.cells,
            summary.paid,
            summary.offered,
            summary.applied
        ),
        (1, 4, 4.0, 3.0, 3.0)
    );
    assert_eq!(
        summary
            .record(&mut ledger, Transfer::Emission)
            .expect("record"),
        1.0
    );
    let closure = ledger
        .close(holdings(world.energy_total(), organism.remaining()))
        .expect("C.4 closes exactly");
    assert_eq!(closure.flow(Flow::EmissionLoss), 1.0);
    assert_eq!(closure.dissipated(), 1.0);
    assert_eq!(closure.debited(Transfer::Emission), 4.0);
    assert_eq!(closure.credited(Transfer::Emission), 3.0);
    run.commit(&closure).expect("commit");

    let stencil = ExchangeStencil::at(&grid, [3.0, 5.0]).expect("stencil");
    assert_eq!(stencil.weights(), [0.25; 4]);
    for &cell in stencil.cells() {
        assert_eq!(world.energy()[cell], 1.75);
        // Energy and every moment component use the same weight.
        for channel in 0..32_u32 {
            let expected = (0.75 * a[channel as usize]) as f32;
            assert_eq!(moment(&world, channel, cell), expected, "channel {channel}");
        }
    }
    let touched: usize = world.energy().iter().filter(|&&e| e != 1.0).count();
    assert_eq!(touched, 4);
}

#[test]
fn deposits_share_one_bilinear_weight_per_cell_for_energy_and_moment() {
    let mut world = field(8, 0.0);
    let grid = *world.grid();
    let policy = EmissionPolicy::new(0.75).expect("policy");
    let position = [3.3, 5.7];
    let stencil = ExchangeStencil::at(&grid, position).expect("stencil");
    assert_eq!(stencil.cells().len(), 4);
    let sum: f64 = stencil.weights().iter().sum();
    assert!(stencil.weights().iter().all(|&w| w > 0.0));
    assert!((sum - 1.0).abs() <= 2.0 * f64::EPSILON);

    let a = direction(&[0.3, -0.5, 0.2, 0.7, -0.1]);
    let mut organism = budget(10.0);
    let mut deposits = Deposits::new();
    policy
        .emit(
            &grid,
            OrganismId::new(4),
            position,
            Some(&Emission {
                direction: a,
                spend_fraction: 0.4,
            }),
            &mut organism,
            &mut deposits,
        )
        .expect("emit");
    let summary = apply_deposits(&mut world, &mut deposits).expect("apply");
    assert!(summary.applied <= summary.offered && summary.offered <= summary.paid);
    for (&cell, &weight) in stencil.cells().iter().zip(stencil.weights()) {
        let received = f64::from(world.energy()[cell]);
        // Each cell holds its weight of the offer, to F32 rounding.
        assert!((received - 3.0 * weight).abs() <= 2.0 * 3.0 * weight * f64::from(f32::EPSILON));
        assert!(received <= 3.0 * weight * (1.0 + 4.0 * f64::EPSILON));
        for (channel, &component) in (0_u32..).zip(&a) {
            let stored = f64::from(moment(&world, channel, cell));
            let ideal = received * component;
            assert!(
                (stored - ideal).abs() <= f64::from(f32::EPSILON) * ideal.abs() + f64::MIN_POSITIVE,
                "cell {cell} channel {channel}: {stored} vs {ideal}"
            );
        }
    }
}

#[test]
fn emission_spends_the_decoded_fraction_of_the_post_motion_budget() {
    let config = appendix_a();
    let step = config.report().timestep;
    let decoding = ActionDecoding::from_config(&config).expect("decoding");
    let mut raw = [0.0; ACTION_WIDTH];
    raw[2] = 4.0; // gate open
    raw[3] = 1.5;
    raw[4] = -2.0;
    raw[35] = 3.0; // spending
    let actions = decode(&raw, step, &decoding).expect("decode");
    let Some(proposal) = actions.emission else {
        panic!("an open gate and a valid direction propose an emission");
    };
    let fraction = proposal.spend_fraction;
    assert!(fraction > 0.0 && fraction < 0.01, "{fraction}");

    // Motion is paid first; emission spends its fraction of what is left.
    let motion = Motion::from_config(&config).expect("motion");
    let grid = *field(16, 0.5).grid();
    let policy = EmissionPolicy::from_config(&config).expect("policy");
    let mut organism = budget(3.0);
    let moved = motion.settle([2.0, -1.0], &mut organism).expect("motion");
    assert!(moved.cost.paid > 0.0);
    let after_motion = organism.remaining();
    let mut deposits = Deposits::new();
    let settlement = policy
        .emit(
            &grid,
            OrganismId::new(3),
            [4.5, 4.5],
            Some(&proposal),
            &mut organism,
            &mut deposits,
        )
        .expect("emit");
    assert_eq!(settlement.available, after_motion);
    assert_eq!(settlement.cost.paid, fraction * after_motion);
    assert_eq!(settlement.offered, 0.9 * settlement.cost.paid);

    // A held output is decoded against each tick's own budget.
    for energy in [0.5, 2.0, 7.25] {
        let mut held = budget(energy);
        let paid = policy
            .emit(
                &grid,
                OrganismId::new(5),
                [1.0, 1.0],
                Some(&proposal),
                &mut held,
                &mut Deposits::new(),
            )
            .expect("emit")
            .cost
            .paid;
        assert_eq!(paid, fraction * energy);
    }
}

#[test]
fn no_emission_no_charge_for_closed_gates_zero_directions_and_the_dead() {
    let config = appendix_a();
    let step = config.report().timestep;
    let decoding = ActionDecoding::from_config(&config).expect("decoding");
    let policy = EmissionPolicy::from_config(&config).expect("policy");
    let grid = *field(8, 0.5).grid();

    let mut closed = [0.0; ACTION_WIDTH];
    closed[2] = -3.0;
    closed[3] = 1.0;
    let mut zero = [0.0; ACTION_WIDTH];
    zero[2] = 3.0;
    for raw in [closed, zero] {
        let actions = decode(&raw, step, &decoding).expect("decode");
        assert!(actions.emission.is_none());
        let mut organism = budget(2.0);
        let mut deposits = Deposits::new();
        let settlement = policy
            .emit(
                &grid,
                OrganismId::new(1),
                [2.0, 2.0],
                actions.emission.as_ref(),
                &mut organism,
                &mut deposits,
            )
            .expect("emit");
        assert!(!settlement.emitted());
        assert_eq!(settlement.cost.requested, 0.0);
        assert_eq!(organism.remaining(), 2.0);
        assert!(deposits.is_empty());
    }

    // The dead emit nothing; a zero fraction stages nothing.
    let valid = emission(&[1.0], 0.5);
    for (mut organism, proposal) in [
        (budget(0.0), emission(&[1.0], 0.5)),
        (budget(2.0), emission(&[1.0], 0.0)),
    ] {
        let mut deposits = Deposits::new();
        let settlement = policy
            .emit(
                &grid,
                OrganismId::new(2),
                [2.0, 2.0],
                Some(&proposal),
                &mut organism,
                &mut deposits,
            )
            .expect("emit");
        assert!(!settlement.emitted());
        assert!(deposits.is_empty());
    }

    // A malformed proposal is refused and changes nothing.
    for bad in [1.5, -0.1, f64::NAN] {
        let mut organism = budget(2.0);
        let mut deposits = Deposits::new();
        let proposal = Emission {
            spend_fraction: bad,
            ..valid
        };
        assert!(
            policy
                .emit(
                    &grid,
                    OrganismId::new(3),
                    [2.0, 2.0],
                    Some(&proposal),
                    &mut organism,
                    &mut deposits
                )
                .is_err()
        );
        assert_eq!(organism.remaining(), 2.0);
        assert!(deposits.is_empty());
    }
    let mut outside = valid;
    outside.direction[0] = 1.5;
    let mut organism = budget(2.0);
    let mut deposits = Deposits::new();
    assert!(
        policy
            .emit(
                &grid,
                OrganismId::new(3),
                [2.0, 2.0],
                Some(&outside),
                &mut organism,
                &mut deposits
            )
            .is_err()
    );
    assert_eq!(organism.remaining(), 2.0);
    assert!(deposits.is_empty());
}

#[test]
fn an_emission_that_spends_everything_exhausts_the_emitter_and_its_deposit_stands() {
    let mut world = field(8, 0.0);
    let grid = *world.grid();
    let policy = EmissionPolicy::new(0.5).expect("policy");
    let mut organism = budget(1.25);
    let mut deposits = Deposits::new();
    let settlement = policy
        .emit(
            &grid,
            OrganismId::new(9),
            [4.5, 4.5],
            Some(&emission(&[0.0, 1.0], 1.0)),
            &mut organism,
            &mut deposits,
        )
        .expect("emit");
    assert_eq!(settlement.cost.paid, 1.25);
    assert!(organism.is_exhausted(), "the emitter dies after this stage");
    let summary = apply_deposits(&mut world, &mut deposits).expect("apply");
    assert_eq!(summary.applied, 0.625);
    assert_eq!(world.energy_total(), 0.625);
}

/// Organism 1 emits at `at`, while organisms 1 and 2 both demand uptake
/// there. Returns the grants they received and the next tick's density.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn emit_and_feed(emit: bool) -> (Vec<(OrganismId, f64)>, f64, f64) {
    let mut world = field(8, 0.5);
    let grid = *world.grid();
    let at = [3.2, 4.6];
    let snapshot = world.clone();
    let policy = EmissionPolicy::new(0.9).expect("policy");
    let mut emitter = budget(5.0);
    let mut deposits = Deposits::new();
    if emit {
        policy
            .emit(
                &grid,
                OrganismId::new(1),
                at,
                Some(&emission(&[1.0], 0.5)),
                &mut emitter,
                &mut deposits,
            )
            .expect("emit");
    }
    let demands = [
        Demand {
            organism: OrganismId::new(1),
            position: at,
            amount: 0.2,
        },
        Demand {
            organism: OrganismId::new(2),
            position: at,
            amount: 0.3,
        },
    ];
    let mut grants = Grants::new();
    resolve_uptake(&snapshot, &demands, &mut grants).expect("resolve");
    let mut received: BTreeMap<OrganismId, f64> = BTreeMap::new();
    for packet in grants.packets() {
        let organism = demands
            .get(packet.demand)
            .map(|d| d.organism)
            .expect("demand");
        *received.entry(organism).or_default() += packet.granted;
    }
    apply_grants(&mut world, &mut grants).expect("grants first");
    apply_deposits(&mut world, &mut deposits).expect("then deposits");
    let density = ExchangeStencil::at(&grid, at)
        .and_then(|stencil| stencil.density(&world))
        .expect("density");
    let snapshot_density = ExchangeStencil::at(&grid, at)
        .and_then(|stencil| stencil.density(&snapshot))
        .expect("density");
    (received.into_iter().collect(), density, snapshot_density)
}

#[test]
fn deposits_are_first_sensed_and_absorbed_on_the_next_tick() {
    let (quiet, quiet_next, quiet_snapshot) = emit_and_feed(false);
    let (loud, loud_next, loud_snapshot) = emit_and_feed(true);
    // This tick's observation and grants are blind to the new deposit, the
    // emitter's own included.
    assert_eq!(quiet_snapshot.to_bits(), loud_snapshot.to_bits());
    assert_eq!(quiet.len(), 2);
    for ((id, a), (other, b)) in quiet.iter().zip(&loud) {
        assert_eq!(id, other);
        assert_eq!(a.to_bits(), b.to_bits(), "{id}");
    }
    // The next tick sees it.
    assert!(loud_next > quiet_next, "{loud_next} vs {quiet_next}");

    // Deposits cannot be reduced before this tick's uptake debits.
    let mut world = field(8, 0.5);
    let grid = *world.grid();
    let snapshot = world.clone();
    let mut deposits = Deposits::new();
    deposits
        .push(
            &grid,
            OrganismId::new(1),
            [3.2, 4.6],
            1.0,
            0.9,
            &direction(&[1.0]),
        )
        .expect("push");
    let mut grants = Grants::new();
    resolve_uptake(
        &snapshot,
        &[Demand {
            organism: OrganismId::new(2),
            position: [3.2, 4.6],
            amount: 0.3,
        }],
        &mut grants,
    )
    .expect("resolve");
    apply_deposits(&mut world, &mut deposits).expect("deposits");
    assert!(matches!(
        apply_grants(&mut world, &mut grants),
        Err(CoreError::StateConflict { .. })
    ));
}

/// 96 deposits from 64 organisms at seeded positions (several sharing
/// cells), pushed in `order`, onto a seeded field.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn reduce(order: &[usize], efficiency: f64, base: f32) -> (Vec<u32>, Vec<u32>, f64, f64, f64) {
    let mut world = field(16, base);
    let grid = *world.grid();
    let mut state = 0x0f11_0f11;
    for cell in world.energy_mut() {
        *cell = base + (uniform(&mut state) as f32) * base;
    }
    let mut specs = Vec::new();
    for organism in 0..64_u64 {
        let position = if organism % 5 == 0 {
            [7.5, 7.5]
        } else {
            [16.0 * uniform(&mut state), 16.0 * uniform(&mut state)]
        };
        let mut normals = [0.0; CHANNELS];
        for normal in &mut normals {
            *normal = uniform(&mut state) - 0.5;
        }
        let a = unit_direction(&normals).expect("direction");
        let paid = 0.01 + uniform(&mut state);
        specs.push((OrganismId::new(1_000 - organism), position, paid, a));
    }
    let mut budgets: Vec<EnergyBudget> = specs.iter().map(|_| budget(2.0)).collect();
    let mut run = EnergyLedger::new(
        holdings(world.energy_total(), total(&budgets)),
        ResidualBudget::new(0.0, 1.0e-15, 1.0e-15).expect("budget"),
    );
    let mut ledger = run
        .open(
            Tick::new(0),
            holdings(world.energy_total(), total(&budgets)),
        )
        .expect("open");
    let mut deposits = Deposits::new();
    for &index in order {
        let (organism, position, paid, a) = specs[index];
        deposits
            .push(&grid, organism, position, paid, efficiency * paid, &a)
            .expect("push");
        budgets[index].pay(paid).expect("pay");
    }
    let summary = apply_deposits(&mut world, &mut deposits).expect("apply");
    summary
        .record(&mut ledger, Transfer::Emission)
        .expect("a deposit never credits more than was paid");
    let closure = ledger
        .close(holdings(world.energy_total(), total(&budgets)))
        .expect("the reduction closes to rounding");
    run.commit(&closure).expect("commit");
    world
        .check_invariants(ConeTolerance::new(0.0, 2.0 * f64::from(f32::EPSILON)).expect("tol"))
        .expect("deposits keep the cone");
    let moments = (0..32_u32)
        .flat_map(|channel| world.moment(channel).unwrap_or(&[]).to_vec())
        .map(f32::to_bits)
        .collect();
    (
        world.energy().iter().map(|e| e.to_bits()).collect(),
        moments,
        summary.paid,
        summary.offered,
        summary.applied,
    )
}

#[test]
fn deposit_reduction_depends_on_identities_never_on_push_order() {
    let forward: Vec<usize> = (0..64).collect();
    let reverse: Vec<usize> = (0..64).rev().collect();
    let interleaved: Vec<usize> = (0..64)
        .filter(|i| i % 2 == 1)
        .chain((0..64).filter(|i| i % 2 == 0))
        .collect();
    let first = reduce(&forward, 0.9, 0.25);
    assert_eq!(reduce(&reverse, 0.9, 0.25), first);
    assert_eq!(reduce(&interleaved, 0.9, 0.25), first);
    let (_, _, paid, offered, applied) = first;
    assert!(applied <= offered && offered <= paid);
    assert!((offered - 0.9 * paid).abs() <= 1.0e-14 * paid);

    // An adversarial cell where the F64 summation order decides an F32
    // boundary. Summed in identity order (1, 2, 3), the packets add up to
    // exactly 1 + 2^-23, which F32 holds. Summed as 3, 1, 2 they round down
    // to just below it, and F32 would keep only 1.
    let tiny = 2.0_f64.powi(-70);
    let packets = [
        (OrganismId::new(3), 1.0),
        (OrganismId::new(1), 2.0_f64.powi(-23) - tiny),
        (OrganismId::new(2), tiny),
    ];
    let expected = 1.0_f32.next_up();
    for order in [[0, 1, 2], [1, 2, 0], [2, 0, 1], [0, 2, 1]] {
        let mut world = field(4, 0.0);
        let grid = *world.grid();
        let mut deposits = Deposits::new();
        for index in order {
            let (organism, amount) = packets[index];
            deposits
                .push(
                    &grid,
                    organism,
                    [2.5, 2.5],
                    amount,
                    amount,
                    &direction(&[1.0]),
                )
                .expect("push");
        }
        let summary = apply_deposits(&mut world, &mut deposits).expect("apply");
        assert_eq!(world.energy()[10], expected, "push order {order:?}");
        assert_eq!(summary.applied, f64::from(expected));
    }
}

#[test]
fn deposits_never_create_energy_even_at_full_efficiency() {
    // Cells near 1,000 hold only multiples of 2^-14: much of each small
    // packet is lost to rounding, and every cell rounds down.
    let order: Vec<usize> = (0..64).collect();
    let (energies, _, paid, offered, applied) = reduce(&order, 1.0, 1_000.0);
    assert_eq!(offered, paid);
    assert!(applied < offered, "{applied} vs {offered}");
    assert!(
        energies
            .iter()
            .all(|&bits| f32::from_bits(bits).is_finite())
    );

    // A packet below the cell's last place is dissipated entirely.
    let mut world = field(4, 1_000.0);
    let grid = *world.grid();
    let before = world.energy().to_vec();
    let mut deposits = Deposits::new();
    deposits
        .push(
            &grid,
            OrganismId::new(1),
            [1.5, 1.5],
            1.0e-6,
            1.0e-6,
            &direction(&[1.0]),
        )
        .expect("push");
    let summary = apply_deposits(&mut world, &mut deposits).expect("apply");
    assert_eq!(summary.applied, 0.0);
    assert_eq!(world.energy(), before.as_slice());
    assert_eq!(moment(&world, 0, 5), 0.0);

    // Into empty cells at full efficiency, an exact offer lands exactly.
    let mut world = field(4, 0.0);
    let mut deposits = Deposits::new();
    deposits
        .push(
            &grid,
            OrganismId::new(1),
            [1.0, 1.0],
            2.0,
            2.0,
            &direction(&[1.0]),
        )
        .expect("push");
    let summary = apply_deposits(&mut world, &mut deposits).expect("apply");
    assert_eq!((summary.offered, summary.applied), (2.0, 2.0));
    assert_eq!(world.energy_total(), 2.0);
}

#[test]
fn deposits_refuse_malformed_input_duplicates_and_double_application() {
    let mut world = field(4, 0.5);
    let grid = *world.grid();
    let a = direction(&[1.0]);
    let mut deposits = Deposits::new();
    for (paid, offered) in [
        (1.0, 1.5),
        (-1.0, 0.0),
        (1.0, -0.5),
        (f64::NAN, 0.0),
        (f64::INFINITY, 1.0),
    ] {
        assert!(matches!(
            deposits.push(&grid, OrganismId::new(1), [1.0, 1.0], paid, offered, &a),
            Err(CoreError::NumericalInvalid { .. })
        ));
    }
    let mut outside = a;
    outside[1] = 0.1;
    let mut infinite = a;
    infinite[2] = f64::INFINITY;
    for bad in [outside, infinite] {
        assert!(
            deposits
                .push(&grid, OrganismId::new(1), [1.0, 1.0], 1.0, 0.5, &bad)
                .is_err()
        );
    }
    assert!(
        deposits
            .push(&grid, OrganismId::new(1), [f64::NAN, 1.0], 1.0, 0.5, &a)
            .is_err()
    );
    assert!(deposits.is_empty(), "a refused push stages nothing");

    let other = FieldGrid::new(8, 8, 8.0, 8.0).expect("grid");
    deposits
        .push(&grid, OrganismId::new(1), [1.0, 1.0], 1.0, 0.5, &a)
        .expect("push");
    assert!(matches!(
        deposits.push(&other, OrganismId::new(2), [1.0, 1.0], 1.0, 0.5, &a),
        Err(CoreError::StateConflict { .. })
    ));
    deposits
        .push(&grid, OrganismId::new(1), [2.5, 2.5], 1.0, 0.5, &a)
        .expect("push");
    let before = world.as_slice().to_vec();
    assert!(matches!(
        apply_deposits(&mut world, &mut deposits),
        Err(CoreError::StateConflict { .. })
    ));
    assert_eq!(world.as_slice(), before.as_slice(), "nothing is written");

    deposits.clear();
    deposits
        .push(&grid, OrganismId::new(1), [1.0, 1.0], 1.0, 0.5, &a)
        .expect("push");
    apply_deposits(&mut world, &mut deposits).expect("apply");
    assert!(deposits.is_applied());
    assert!(matches!(
        apply_deposits(&mut world, &mut deposits),
        Err(CoreError::StateConflict { .. })
    ));
    assert!(matches!(
        deposits.push(&grid, OrganismId::new(3), [1.0, 1.0], 1.0, 0.5, &a),
        Err(CoreError::StateConflict { .. })
    ));
    let mut elsewhere = field(8, 0.5);
    deposits.clear();
    deposits
        .push(&grid, OrganismId::new(1), [1.0, 1.0], 1.0, 0.5, &a)
        .expect("push");
    assert!(matches!(
        apply_deposits(&mut elsewhere, &mut deposits),
        Err(CoreError::StateConflict { .. })
    ));
}

// ---------------------------------------------------------------------------
// Reproduction.

const SEED: u64 = 42;
const COOLDOWN: u64 = 128;

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn catalog() -> TopologyCatalog {
    TopologyCatalog::from_config(&appendix_a()).expect("Appendix A catalog")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn limits() -> TraitLimits {
    TraitLimits::from_config(&appendix_a()).expect("Appendix A limits")
}

fn rules(birth_cost: f64) -> ReproductionRules {
    ReproductionRules {
        birth_cost,
        minimum_attempt_energy: 4.0,
        minimum_child_energy: 0.3,
        minimum_parent_energy: 0.3,
        cooldown: TickSpan::new(COOLDOWN),
        offset_radius: 0.25,
        controller_interval: 4,
    }
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn reproduction(birth_cost: f64) -> Reproduction {
    Reproduction::new(rules(birth_cost)).expect("valid rules")
}

/// Every operator off: a child copies its parent exactly.
fn quiet() -> MutationParameters {
    MutationParameters {
        topology_change_probability: 0.0,
        weight_probability: 0.0,
        weight_standard_deviation: 0.0,
        replacement_probability: 0.0,
        chemistry_standard_deviation: 0.0,
        sensing_log_scale_standard_deviation: 0.0,
        reject_effective_noop: false,
        maximum_candidates: MAXIMUM_CANDIDATES,
    }
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn mutator(parameters: MutationParameters, catalog: &TopologyCatalog) -> Mutator<'_> {
    Mutator::new(SemanticRng::new(SEED), parameters, catalog, limits()).expect("valid rates")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn founder_genome(organism: u64, catalog: &TopologyCatalog, quota: &mut GenomeQuota) -> Genome {
    founder(
        &SemanticRng::new(SEED),
        OrganismId::new(organism),
        GenomeId::new(organism),
        catalog,
        quota,
        &limits(),
        initial_scale(&appendix_a()),
    )
    .expect("a founder")
}

/// A default-topology parent whose every parameter is `value`.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn uniform_parent(
    organism: u64,
    value: f32,
    catalog: &TopologyCatalog,
    quota: &mut GenomeQuota,
) -> Genome {
    let traits = *founder_genome(organism, catalog, &mut quota.clone()).traits();
    let mut buffer = quota
        .allocate(catalog, catalog.default_topology())
        .expect("room for the parent");
    buffer.f32_mut().expect("an F32 buffer").fill(value);
    Genome::seal(
        buffer,
        GenomeId::new(organism),
        catalog,
        traits,
        None,
        &limits(),
    )
    .expect("an in-bound parent")
}

fn parent(id: u64, position: [f64; 2], deadline: u64) -> ParentState {
    ParentState {
        id: OrganismId::new(id),
        lineage: LineageId::new(id % 3),
        generation: Generation::new(2),
        position,
        cooldown_deadline: Tick::new(deadline),
    }
}

fn actions(attempt_probability: f64, child_fraction: f64) -> DecodedActions {
    DecodedActions {
        attempt_rate: 1.0,
        attempt_probability,
        child_fraction,
        emission_gate: 0.0,
        emission: None,
    }
}

/// A sure attempt's intent.
#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "test helper: a failure here is a test failure"
)]
fn intent(
    reproduction: &Reproduction,
    state: &ParentState,
    tick: u64,
    energy: f64,
    child_fraction: f64,
) -> BirthIntent {
    match reproduction
        .propose(
            &SemanticRng::new(SEED),
            state,
            Tick::new(tick),
            &actions(1.0, child_fraction),
            &budget(energy),
        )
        .expect("propose")
    {
        ReproductionProposal::Intent(intent) => intent,
        other => panic!("expected a viable intent, got {other:?}"),
    }
}

fn identities(organism: u64, genome: u64) -> Identities {
    Identities {
        organisms: IdSequence::starting_at(organism),
        genomes: IdSequence::starting_at(genome),
    }
}

#[expect(
    clippy::panic,
    reason = "test helper: a failure here is a test failure"
)]
fn staged(resolution: BirthResolution) -> Births {
    match resolution {
        BirthResolution::Staged(births) => births,
        BirthResolution::Paused(shortfall) => panic!("unexpected shortfall {shortfall:?}"),
    }
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn grid() -> FieldGrid {
    FieldGrid::from_config(&appendix_a()).expect("Appendix A grid")
}

#[test]
fn c5_birth_splits_energy_and_dissipates_the_birth_cost() {
    // C.5: energy 10, birth cost 1, share 1/3: child 3, parent 6, and 1
    // dissipated.
    let reproduction = reproduction(1.0);
    let split = match reproduction.split(10.0, 1.0 / 3.0).expect("split") {
        Viability::Viable(split) => split,
        Viability::Unviable(reason) => panic!("{reason:?}"),
    };
    assert_eq!(
        (split.available, split.child, split.parent),
        (10.0, 3.0, 6.0)
    );
    assert_eq!((split.debited(), split.dissipated()), (4.0, 1.0));

    // Through the whole transaction, against an exact ledger.
    let catalog = catalog();
    let mut quota = GenomeQuota::new(1 << 32, 1 << 32).expect("quota");
    let genome = founder_genome(1, &catalog, &mut quota);
    let mutator = mutator(quiet(), &catalog);
    let world = grid();
    let rng = SemanticRng::new(SEED);
    let context = BirthContext {
        rng: &rng,
        grid: &world,
        reproduction: &reproduction,
        mutator: &mutator,
        offspring: Offspring::Inherited,
    };
    let state = parent(1, [10.0, 20.0], 0);
    let request = BirthRequest {
        intent: intent(&reproduction, &state, 500, 10.0, 1.0 / 3.0),
        genome: &genome,
    };
    let mut ids = identities(100, 200);
    let births = staged(
        resolve_births(
            &context,
            &[request],
            PopulationQuota::new(16, 1).expect("population"),
            &mut ids,
            &mut quota,
        )
        .expect("resolve"),
    );
    let [birth] = births.born() else {
        panic!("one birth");
    };
    assert_eq!(birth.newborn.energy, 3.0);

    let parent_budget = budget(10.0);
    let mut run = EnergyLedger::new(holdings(0.0, 10.0), ResidualBudget::EXACT);
    let mut ledger = run.open(Tick::new(500), holdings(0.0, 10.0)).expect("open");
    let kept = births
        .settle(&mut ledger, |id| (id == state.id).then_some(parent_budget))
        .expect("settle");
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].0, state.id);
    assert_eq!(kept[0].1.remaining(), 6.0);
    let closure = ledger
        .close(holdings(0.0, kept[0].1.remaining() + birth.newborn.energy))
        .expect("C.5 closes exactly");
    assert_eq!(closure.flow(Flow::ReproductionLoss), 1.0);
    assert_eq!(closure.debited(Transfer::ChildEndowment), 4.0);
    assert_eq!(closure.credited(Transfer::ChildEndowment), 3.0);
    assert_eq!(closure.dissipated(), 1.0);
    run.commit(&closure).expect("commit");
}

#[test]
fn split_shares_are_exact_bounded_and_checked_on_committed_values() {
    let mut state = 0x0c05_0c05_u64;
    for _ in 0..4_000 {
        let birth_cost = (uniform(&mut state) * 1024.0).floor() / 1024.0;
        let reproduction = reproduction(birth_cost);
        let available = 4.0 + 96.0 * uniform(&mut state);
        let share = 0.1 + 0.8 * uniform(&mut state);
        let Viability::Viable(split) = reproduction.split(available, share).expect("split") else {
            continue;
        };
        // child + parent ≤ E − c_b exactly, and the birth costs at least c_b.
        let (e, c, p, b) = (
            exact(split.available),
            exact(split.child),
            exact(split.parent),
            exact(birth_cost),
        );
        assert!(c + p <= e - b, "{split:?}");
        assert!(
            e - b - c - p <= exact(4.0 * available * f64::EPSILON),
            "{split:?}"
        );
        assert!(split.debited() >= split.child);
        assert!(split.child >= 0.3 && split.parent >= 0.3);
        assert!(
            ulps(split.child, share * (available - birth_cost)) <= 4,
            "{split:?}"
        );
    }
}

#[test]
fn viability_is_checked_against_the_post_emission_budget() {
    let reproduction = reproduction(0.05);
    // Viable before emission; emission leaves it below the attempt minimum.
    assert!(matches!(
        reproduction.split(5.0, 0.5).expect("split"),
        Viability::Viable(_)
    ));
    let mut organism = budget(5.0);
    let policy = EmissionPolicy::new(0.9).expect("policy");
    policy
        .emit(
            &grid(),
            OrganismId::new(1),
            [1.0, 1.0],
            Some(&emission(&[1.0], 0.3)),
            &mut organism,
            &mut Deposits::new(),
        )
        .expect("emit");
    let proposal = reproduction
        .propose(
            &SemanticRng::new(SEED),
            &parent(1, [1.0, 1.0], 0),
            Tick::new(10),
            &actions(1.0, 0.5),
            &organism,
        )
        .expect("propose");
    assert_eq!(
        proposal,
        ReproductionProposal::Unviable(Unviable::BelowAttemptEnergy {
            available: 3.5,
            minimum: 4.0,
        })
    );
    // A share too small for the child, or too large for the parent.
    assert!(matches!(
        reproduction.split(4.0, 0.05).expect("split"),
        Viability::Unviable(Unviable::ChildBelowMinimum { .. })
    ));
    assert!(matches!(
        reproduction.split(4.0, 0.95).expect("split"),
        Viability::Unviable(Unviable::ParentBelowMinimum { .. })
    ));
    // A birth cost the budget cannot pay.
    let costly = Reproduction::new(ReproductionRules {
        birth_cost: 6.0,
        ..rules(0.0)
    })
    .expect("rules");
    assert!(matches!(
        costly.split(5.0, 0.5).expect("split"),
        Viability::Unviable(Unviable::BirthCostUnaffordable { .. })
    ));
    for share in [0.0, 1.0, f64::NAN] {
        assert!(reproduction.split(5.0, share).is_err());
    }
    assert!(reproduction.split(f64::INFINITY, 0.5).is_err());
}

#[test]
fn attempts_are_one_bernoulli_draw_per_parent_and_tick() {
    let reproduction = reproduction(0.05);
    let rng = SemanticRng::new(SEED);
    // NumPy's word 0 for parent 7 at tick 1234 is 0x3f4f719432ff9508, the
    // half-open uniform 0.24730596416611505539… (the oracle's exact value).
    let u = (0x3f4f_7194_32ff_9508_u64 >> 12) as f64 / 4_503_599_627_370_496.0;
    assert!((u - 0.247_305_964_166_115).abs() < 1.0e-15);
    let at = |p| {
        reproduction
            .attempts(&rng, OrganismId::new(7), Tick::new(1234), p)
            .expect("attempt")
    };
    assert!(!at(u), "an attempt needs u < p");
    assert!(at(u.next_up()));

    let p = 0.3;
    let mut successes = 0_u32;
    let mut trials = 0_u32;
    for parent in 0..100_u64 {
        for tick in 0..100_u64 {
            trials += 1;
            let first = reproduction
                .attempts(&rng, OrganismId::new(parent), Tick::new(tick), p)
                .expect("attempt");
            let again = reproduction
                .attempts(&rng, OrganismId::new(parent), Tick::new(tick), p)
                .expect("attempt");
            assert_eq!(first, again);
            successes += u32::from(first);
            assert!(
                !reproduction
                    .attempts(&rng, OrganismId::new(parent), Tick::new(tick), 0.0)
                    .expect("attempt")
            );
            assert!(
                reproduction
                    .attempts(&rng, OrganismId::new(parent), Tick::new(tick), 1.0)
                    .expect("attempt")
            );
        }
    }
    let n = f64::from(trials);
    let sigma = (n * p * (1.0 - p)).sqrt();
    assert!(
        (f64::from(successes) - n * p).abs() < 5.0 * sigma,
        "{successes}"
    );
    let reseeded = (0..64_u64).any(|parent| {
        reproduction
            .attempts(&rng, OrganismId::new(parent), Tick::new(3), 0.5)
            .ok()
            != reproduction
                .attempts(
                    &SemanticRng::new(43),
                    OrganismId::new(parent),
                    Tick::new(3),
                    0.5,
                )
                .ok()
    });
    assert!(reseeded);
    for bad in [-0.1, 1.5, f64::NAN] {
        assert!(
            reproduction
                .attempts(&rng, OrganismId::new(1), Tick::new(1), bad)
                .is_err()
        );
    }
}

#[test]
fn child_offsets_are_uniform_by_area_inside_the_disk_and_wrap() {
    let reproduction = reproduction(0.05);
    let rng = SemanticRng::new(SEED);
    // Pinned by the oracle for parent 7 at tick 1234, radius 1/4.
    let [x, y] = reproduction
        .child_offset(&rng, OrganismId::new(7), Tick::new(1234))
        .expect("offset");
    assert!(ulps(x, f64::from_bits(0xbfc7_1d97_eb47_982d)) <= 4, "{x}");
    assert!(ulps(y, f64::from_bits(0x3fb4_bbe9_0b45_5583)) <= 4, "{y}");

    let radius = 0.25;
    let count = 8_192_u32;
    let (mut area, mut cos, mut sin) = (0.0, 0.0, 0.0);
    for parent in 0..u64::from(count) {
        let [dx, dy] = reproduction
            .child_offset(&rng, OrganismId::new(parent), Tick::new(parent % 17))
            .expect("offset");
        let r2 = dx * dx + dy * dy;
        assert!(r2 <= radius * radius * (1.0 + 4.0 * f64::EPSILON));
        area += r2 / (radius * radius);
        cos += dx / radius;
        sin += dy / radius;
    }
    let n = f64::from(count);
    // Uniform by area: r²/R² is uniform on [0, 1).
    assert!((area / n - 0.5).abs() < 5.0 * (1.0 / 12.0 / n).sqrt());
    // r cos θ and r sin θ have mean 0 and variance R²/4.
    for sum in [cos, sin] {
        assert!((sum / n).abs() < 5.0 * (0.25 / n).sqrt(), "{sum}");
    }

    // A child near the seam is wrapped onto the torus.
    let catalog = catalog();
    let mut quota = GenomeQuota::new(1 << 32, 1 << 32).expect("quota");
    let genome = founder_genome(1, &catalog, &mut quota);
    let mutator = mutator(quiet(), &catalog);
    let world = grid();
    let context = BirthContext {
        rng: &rng,
        grid: &world,
        reproduction: &reproduction,
        mutator: &mutator,
        offspring: Offspring::Inherited,
    };
    for (id, position) in (1..).zip([[0.01, 127.99], [127.99, 0.0], [0.0, 0.0]]) {
        let births = staged(
            resolve_births(
                &context,
                &[BirthRequest {
                    intent: intent(&reproduction, &parent(id, position, 0), 9, 8.0, 0.5),
                    genome: &genome,
                }],
                PopulationQuota::new(16, 1).expect("population"),
                &mut identities(500, 500),
                &mut quota,
            )
            .expect("resolve"),
        );
        let newborn = births.born()[0].newborn;
        let [cx, cy] = newborn.position;
        assert!((0.0..128.0).contains(&cx) && (0.0..128.0).contains(&cy));
        let expected = world
            .wrap_position([
                position[0] + newborn.offset[0],
                position[1] + newborn.offset[1],
            ])
            .expect("wrap");
        assert_eq!(newborn.position, expected);
        births.abandon(&mut quota).expect("abandon");
    }
}

#[test]
fn cooldown_gates_attempts_and_only_a_birth_resets_it() {
    let reproduction = reproduction(0.05);
    let rng = SemanticRng::new(SEED);
    let state = parent(5, [3.0, 3.0], 300);
    let sure = actions(1.0, 0.5);
    for tick in [0, 299] {
        assert_eq!(
            reproduction
                .propose(&rng, &state, Tick::new(tick), &sure, &budget(10.0))
                .expect("propose"),
            ReproductionProposal::CoolingDown {
                deadline: Tick::new(300)
            }
        );
    }
    assert!(matches!(
        reproduction
            .propose(&rng, &state, Tick::new(300), &sure, &budget(10.0))
            .expect("propose"),
        ReproductionProposal::Intent(_)
    ));
    // The dead never attempt; a zero rate makes no attempt.
    let mut dead = budget(1.0);
    dead.pay(1.0).expect("pay");
    assert_eq!(
        reproduction
            .propose(&rng, &state, Tick::new(400), &sure, &dead)
            .expect("propose"),
        ReproductionProposal::Exhausted
    );
    assert_eq!(
        reproduction
            .propose(
                &rng,
                &state,
                Tick::new(400),
                &actions(0.0, 0.5),
                &budget(10.0)
            )
            .expect("propose"),
        ReproductionProposal::NoAttempt
    );

    // A committed birth at attempt tick n sets both deadlines to
    // n + 1 + cooldown; a rejected one carries no deadline at all.
    let catalog = catalog();
    let mut quota = GenomeQuota::new(1 << 32, 1 << 32).expect("quota");
    let genome = founder_genome(5, &catalog, &mut quota);
    let flat = uniform_parent(6, 0.5, &catalog, &mut quota);
    let strict = mutator(
        MutationParameters {
            weight_probability: 1.0,
            weight_standard_deviation: 1.0e-12,
            reject_effective_noop: true,
            ..quiet()
        },
        &catalog,
    );
    let world = grid();
    let context = BirthContext {
        rng: &rng,
        grid: &world,
        reproduction: &reproduction,
        mutator: &strict,
        offspring: Offspring::Inherited,
    };
    let requests = [
        BirthRequest {
            intent: intent(&reproduction, &state, 300, 10.0, 0.5),
            genome: &genome,
        },
        BirthRequest {
            intent: intent(&reproduction, &parent(6, [1.0, 1.0], 0), 300, 10.0, 0.5),
            genome: &flat,
        },
    ];
    let births = staged(
        resolve_births(
            &context,
            &requests,
            PopulationQuota::new(16, 2).expect("population"),
            &mut identities(10, 10),
            &mut quota,
        )
        .expect("resolve"),
    );
    let [birth] = births.born() else {
        panic!("one birth: {:?}", births.rejected());
    };
    assert_eq!(birth.parent, OrganismId::new(5));
    assert_eq!(
        birth.parent_cooldown_deadline,
        Tick::new(300 + 1 + COOLDOWN)
    );
    assert_eq!(
        birth.newborn.cooldown_deadline,
        birth.parent_cooldown_deadline
    );
    let [rejected] = births.rejected() else {
        panic!("one rejected birth");
    };
    assert_eq!(rejected.parent, OrganismId::new(6));
    births.abandon(&mut quota).expect("abandon");
}

#[test]
fn newborns_start_inert_and_join_on_the_next_tick() {
    let reproduction = reproduction(0.05);
    let rng = SemanticRng::new(SEED);
    let catalog = catalog();
    let mut quota = GenomeQuota::new(1 << 32, 1 << 32).expect("quota");
    let genome = founder_genome(21, &catalog, &mut quota);
    let mutator = mutator(quiet(), &catalog);
    let world = grid();
    let context = BirthContext {
        rng: &rng,
        grid: &world,
        reproduction: &reproduction,
        mutator: &mutator,
        offspring: Offspring::Inherited,
    };
    let state = parent(21, [64.0, 32.0], 0);
    let request = BirthRequest {
        intent: intent(&reproduction, &state, 77, 6.0, 0.25),
        genome: &genome,
    };
    let mut ids = identities(1_001, 2_001);
    let births = staged(
        resolve_births(
            &context,
            &[request],
            PopulationQuota::new(16, 1).expect("population"),
            &mut ids,
            &mut quota,
        )
        .expect("resolve"),
    );
    let birth = &births.born()[0];
    let newborn = birth.newborn;
    assert_eq!(newborn.id, OrganismId::new(1_001));
    assert_eq!(newborn.genome, GenomeId::new(2_001));
    assert_eq!(newborn.genome, birth.genome.id());
    assert_eq!(birth.genome.parent(), Some(genome.id()));
    assert_eq!(newborn.parent, state.id);
    assert_eq!(newborn.lineage, state.lineage);
    assert_eq!(newborn.generation, Generation::new(3));
    assert_eq!(newborn.birth_tick, Tick::new(78));
    assert_eq!(newborn.velocity, [0.0, 0.0]);
    assert_eq!(newborn.energy_change_rate, 0.0);
    assert!(
        !newborn.held_output_valid,
        "the first advance forces evaluation"
    );
    assert_eq!(newborn.phase, 1_001 % 4);
    assert_eq!(newborn.energy, request.intent.split.child);
    assert_eq!(
        newborn.offset,
        reproduction
            .child_offset(&rng, state.id, Tick::new(77))
            .expect("offset")
    );
    assert_eq!(ids.organisms.next_raw(), Some(1_002));
    assert_eq!(ids.genomes.next_raw(), Some(2_002));
}

/// Six parents' births presented in `order`.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn six_births(order: &[usize]) -> (Vec<String>, GenomeQuota, Identities, u64) {
    let reproduction = reproduction(0.05);
    let rng = SemanticRng::new(SEED);
    let catalog = catalog();
    let mut quota = GenomeQuota::new(1 << 32, 1 << 32).expect("quota");
    let parents: Vec<(ParentState, Genome)> = [40_u64, 3, 17, 8, 25, 11]
        .iter()
        .map(|&id| {
            (
                parent(id, [id as f64, 2.0 * id as f64], 0),
                founder_genome(id, &catalog, &mut quota),
            )
        })
        .collect();
    let charged = quota.logical_bytes();
    let mutator = mutator(
        MutationParameters {
            weight_probability: 0.05,
            weight_standard_deviation: 0.02,
            chemistry_standard_deviation: 0.01,
            ..quiet()
        },
        &catalog,
    );
    let world = grid();
    let context = BirthContext {
        rng: &rng,
        grid: &world,
        reproduction: &reproduction,
        mutator: &mutator,
        offspring: Offspring::Inherited,
    };
    let requests: Vec<BirthRequest<'_>> = order
        .iter()
        .map(|&index| {
            let (state, genome) = &parents[index];
            BirthRequest {
                intent: intent(&reproduction, state, 640, 5.0 + state.id.get() as f64, 0.4),
                genome,
            }
        })
        .collect();
    let mut ids = identities(1_000, 5_000);
    let births = staged(
        resolve_births(
            &context,
            &requests,
            PopulationQuota::new(64, 6).expect("population"),
            &mut ids,
            &mut quota,
        )
        .expect("resolve"),
    );
    let summary = births
        .born()
        .iter()
        .map(|birth| {
            format!(
                "{} {} {} {:?} {:x} {:x} {:x}",
                birth.parent,
                birth.newborn.id,
                birth.newborn.genome,
                birth.newborn.position.map(f64::to_bits),
                birth.newborn.energy.to_bits(),
                birth.split.parent.to_bits(),
                birth
                    .genome
                    .parameters()
                    .iter()
                    .fold(0_u64, |hash, value| hash.rotate_left(5)
                        ^ u64::from(value.to_bits())),
            )
        })
        .collect();

    // Settle all parents and close the ledger on the resulting holdings.
    let budgets: BTreeMap<OrganismId, EnergyBudget> = parents
        .iter()
        .map(|(state, _)| (state.id, budget(5.0 + state.id.get() as f64)))
        .collect();
    let opening: Vec<EnergyBudget> = budgets.values().copied().collect();
    let mut run = EnergyLedger::new(
        holdings(0.0, total(&opening)),
        ResidualBudget::new(0.0, 1.0e-15, 1.0e-15).expect("budget"),
    );
    let mut ledger = run
        .open(Tick::new(640), holdings(0.0, total(&opening)))
        .expect("open");
    let kept = births
        .settle(&mut ledger, |id| budgets.get(&id).copied())
        .expect("settle");
    let mut closing: Vec<EnergyBudget> = kept.iter().map(|(_, budget)| *budget).collect();
    closing.extend(
        births
            .born()
            .iter()
            .map(|birth| budget(birth.newborn.energy)),
    );
    let closure = ledger
        .close(holdings(0.0, total(&closing)))
        .expect("the births close");
    run.commit(&closure).expect("commit");
    assert!((closure.flow(Flow::ReproductionLoss) - 6.0 * 0.05).abs() < 1.0e-14);

    let staged_bytes = quota.logical_bytes() - charged;
    births.abandon(&mut quota).expect("abandon");
    assert_eq!(
        quota.logical_bytes(),
        charged,
        "abandoning returns every charge"
    );
    (summary, quota, ids, staged_bytes)
}

#[test]
fn births_commit_identity_genome_slot_and_energy_together() {
    let (forward, _, ids, bytes) = six_births(&[0, 1, 2, 3, 4, 5]);
    assert_eq!(six_births(&[5, 4, 3, 2, 1, 0]).0, forward);
    assert_eq!(six_births(&[2, 5, 0, 3, 1, 4]).0, forward);
    // Children take consecutive identities in ascending parent identity.
    let parents: Vec<&str> = forward
        .iter()
        .map(|line| &line[..line.find(' ').unwrap_or(0)])
        .collect();
    assert_eq!(
        parents,
        [
            "organism:3",
            "organism:8",
            "organism:11",
            "organism:17",
            "organism:25",
            "organism:40"
        ]
    );
    for (index, line) in forward.iter().enumerate() {
        assert!(
            line.contains(&format!(
                "organism:{} genome:{}",
                1_000 + index,
                5_000 + index
            )),
            "{line}"
        );
    }
    assert_eq!(ids.organisms.next_raw(), Some(1_006));
    assert_eq!(ids.genomes.next_raw(), Some(5_006));
    let per_child = appendix_a().report().default_parameter_count * 4;
    assert_eq!(
        bytes,
        6 * per_child,
        "every staged child is charged in full"
    );
}

#[test]
fn settling_refuses_a_parent_whose_budget_moved_and_records_nothing() {
    let reproduction = reproduction(0.05);
    let rng = SemanticRng::new(SEED);
    let catalog = catalog();
    let mut quota = GenomeQuota::new(1 << 32, 1 << 32).expect("quota");
    let genome = founder_genome(2, &catalog, &mut quota);
    let mutator = mutator(quiet(), &catalog);
    let world = grid();
    let context = BirthContext {
        rng: &rng,
        grid: &world,
        reproduction: &reproduction,
        mutator: &mutator,
        offspring: Offspring::Inherited,
    };
    let births = staged(
        resolve_births(
            &context,
            &[BirthRequest {
                intent: intent(&reproduction, &parent(2, [5.0, 5.0], 0), 3, 9.0, 0.5),
                genome: &genome,
            }],
            PopulationQuota::new(16, 1).expect("population"),
            &mut identities(10, 10),
            &mut quota,
        )
        .expect("resolve"),
    );
    let run = EnergyLedger::new(holdings(0.0, 9.0), ResidualBudget::EXACT);
    let mut ledger = run.open(Tick::new(3), holdings(0.0, 9.0)).expect("open");
    for stale in [Some(budget(8.5)), None] {
        assert!(matches!(
            births.settle(&mut ledger, |_| stale),
            Err(CoreError::StateConflict { .. })
        ));
    }
    let mut dead = budget(9.0);
    dead.pay(9.0).expect("pay");
    assert!(births.settle(&mut ledger, |_| Some(dead)).is_err());
    // Nothing was recorded: the unchanged holdings close exactly.
    ledger.close(holdings(0.0, 9.0)).expect("nothing recorded");
    births.abandon(&mut quota).expect("abandon");
}

#[test]
fn c5_a_rejected_allocation_commits_no_child_no_transfer_and_no_cost() {
    let reproduction = reproduction(1.0);
    let rng = SemanticRng::new(SEED);
    let catalog = catalog();
    let per_child = appendix_a().report().default_parameter_count * 4;
    // Room for the two parents and exactly one child.
    let mut quota = GenomeQuota::new(3 * per_child, 4 * per_child).expect("quota");
    let first = founder_genome(1, &catalog, &mut quota);
    let second = founder_genome(2, &catalog, &mut quota);
    let before = quota.clone();
    let mutator = mutator(quiet(), &catalog);
    let world = grid();
    let context = BirthContext {
        rng: &rng,
        grid: &world,
        reproduction: &reproduction,
        mutator: &mutator,
        offspring: Offspring::Inherited,
    };
    let requests = [
        BirthRequest {
            intent: intent(
                &reproduction,
                &parent(2, [5.0, 5.0], 0),
                50,
                10.0,
                1.0 / 3.0,
            ),
            genome: &second,
        },
        BirthRequest {
            intent: intent(
                &reproduction,
                &parent(1, [6.0, 5.0], 0),
                50,
                10.0,
                1.0 / 3.0,
            ),
            genome: &first,
        },
    ];
    let mut ids = identities(10, 10);
    let shortfall = match resolve_births(
        &context,
        &requests,
        PopulationQuota::new(16, 2).expect("population"),
        &mut ids,
        &mut quota,
    )
    .expect("resolve")
    {
        BirthResolution::Paused(shortfall) => shortfall,
        BirthResolution::Staged(births) => {
            panic!("the quota cannot cover {:?}", births.born().len())
        }
    };
    assert_eq!(shortfall.resource, Capacity::LogicalGenomeBytes);
    assert_eq!(shortfall.parent, OrganismId::new(2));
    assert_eq!((shortfall.required, shortfall.available), (per_child, 0));
    assert_eq!(shortfall.proposed, [OrganismId::new(1), OrganismId::new(2)]);
    assert_eq!(shortfall.covered, 1);
    // No child exists, no identity was issued and nothing stays charged;
    // the parents' budgets were never touched.
    assert_eq!(quota, before);
    assert_eq!(ids, identities(10, 10));
    // Retrying the unchanged candidate reproduces the same report.
    let again = resolve_births(
        &context,
        &requests,
        PopulationQuota::new(16, 2).expect("population"),
        &mut ids,
        &mut quota,
    )
    .expect("resolve");
    assert!(matches!(again, BirthResolution::Paused(ref s) if *s == shortfall));
    assert_eq!(quota, before);

    // An error mid-transaction rolls back too: the second child's identity
    // overflows after the first child was staged.
    let mut roomy = GenomeQuota::new(1 << 32, 1 << 32).expect("quota");
    let first = founder_genome(1, &catalog, &mut roomy);
    let second = founder_genome(2, &catalog, &mut roomy);
    let before = roomy.clone();
    let mut last = Identities {
        organisms: IdSequence::resume(Some(u64::MAX)),
        genomes: IdSequence::starting_at(10),
    };
    let snapshot = last.clone();
    let requests = [
        BirthRequest {
            intent: intent(&reproduction, &parent(1, [5.0, 5.0], 0), 50, 10.0, 0.5),
            genome: &first,
        },
        BirthRequest {
            intent: intent(&reproduction, &parent(2, [6.0, 5.0], 0), 50, 10.0, 0.5),
            genome: &second,
        },
    ];
    assert!(matches!(
        resolve_births(
            &context,
            &requests,
            PopulationQuota::new(16, 2).expect("population"),
            &mut last,
            &mut roomy,
        ),
        Err(CoreError::Overflow { .. })
    ));
    assert_eq!(roomy, before);
    assert_eq!(last, snapshot);
}

#[test]
fn population_slots_are_reserved_before_any_genome_work() {
    let reproduction = reproduction(0.05);
    let rng = SemanticRng::new(SEED);
    let catalog = catalog();
    let mut quota = GenomeQuota::new(1 << 32, 1 << 32).expect("quota");
    let genomes: Vec<Genome> = (1..=3)
        .map(|id| founder_genome(id, &catalog, &mut quota))
        .collect();
    let before = quota.clone();
    let mutator = mutator(quiet(), &catalog);
    let world = grid();
    let context = BirthContext {
        rng: &rng,
        grid: &world,
        reproduction: &reproduction,
        mutator: &mutator,
        offspring: Offspring::Inherited,
    };
    let requests: Vec<BirthRequest<'_>> = genomes
        .iter()
        .zip(1_u64..)
        .map(|(genome, id)| BirthRequest {
            intent: intent(&reproduction, &parent(id, [id as f64, 1.0], 0), 9, 8.0, 0.5),
            genome,
        })
        .collect();
    // A full world: nothing is attempted, so nothing was ever allocated.
    let full = PopulationQuota::from_config(&appendix_a(), 16_384).expect("population");
    assert_eq!(full.available(), 0);
    let mut ids = identities(10, 10);
    let BirthResolution::Paused(shortfall) =
        resolve_births(&context, &requests, full, &mut ids, &mut quota).expect("resolve")
    else {
        panic!("a full world pauses");
    };
    assert_eq!(
        (
            shortfall.resource,
            shortfall.parent,
            shortfall.required,
            shortfall.available,
            shortfall.covered
        ),
        (Capacity::PopulationSlots, OrganismId::new(1), 1, 0, 0)
    );
    assert_eq!(quota, before);

    // Two free slots for three births: the third, in identity order, is
    // refused and the first two are rolled back.
    let BirthResolution::Paused(shortfall) = resolve_births(
        &context,
        &requests,
        PopulationQuota::new(10, 8).expect("population"),
        &mut ids,
        &mut quota,
    )
    .expect("resolve") else {
        panic!("two slots cannot hold three births");
    };
    assert_eq!(
        (shortfall.parent, shortfall.covered),
        (OrganismId::new(3), 2)
    );
    assert_eq!(quota, before);
    assert_eq!(ids, identities(10, 10));
    // Three slots hold them all.
    let births = staged(
        resolve_births(
            &context,
            &requests,
            PopulationQuota::new(10, 7).expect("population"),
            &mut ids,
            &mut quota,
        )
        .expect("resolve"),
    );
    assert_eq!(births.born().len(), 3);
    births.abandon(&mut quota).expect("abandon");
    assert!(PopulationQuota::new(4, 5).is_err());
}

#[test]
fn rejected_births_charge_nothing_and_take_no_identity() {
    // C.16 at the transaction: a parent whose every candidate is an
    // effective no-op is rejected without charge; the next birth takes the
    // identities it would have used.
    let reproduction = reproduction(0.05);
    let rng = SemanticRng::new(SEED);
    let catalog = catalog();
    let mut quota = GenomeQuota::new(1 << 32, 1 << 32).expect("quota");
    let flat = uniform_parent(3, 0.5, &catalog, &mut quota);
    let founder = founder_genome(9, &catalog, &mut quota);
    let before = quota.clone();
    let strict = mutator(
        MutationParameters {
            weight_probability: 1.0,
            weight_standard_deviation: 1.0e-12,
            reject_effective_noop: true,
            ..quiet()
        },
        &catalog,
    );
    let world = grid();
    let context = BirthContext {
        rng: &rng,
        grid: &world,
        reproduction: &reproduction,
        mutator: &strict,
        offspring: Offspring::Inherited,
    };
    let requests = [
        BirthRequest {
            intent: intent(&reproduction, &parent(3, [1.0, 1.0], 0), 4, 9.0, 0.5),
            genome: &flat,
        },
        BirthRequest {
            intent: intent(&reproduction, &parent(9, [2.0, 1.0], 0), 4, 9.0, 0.5),
            genome: &founder,
        },
    ];
    let mut ids = identities(70, 80);
    let births = staged(
        resolve_births(
            &context,
            &requests,
            PopulationQuota::new(3, 2).expect("population"),
            &mut ids,
            &mut quota,
        )
        .expect("resolve"),
    );
    let [rejected] = births.rejected() else {
        panic!("one rejection");
    };
    assert_eq!(rejected.parent, OrganismId::new(3));
    assert_eq!(rejected.candidates.len(), usize::from(MAXIMUM_CANDIDATES));
    assert!(
        rejected
            .candidates
            .iter()
            .all(|c| c.reason == RejectionReason::EffectiveNoop)
    );
    let [birth] = births.born() else {
        panic!("one birth");
    };
    assert_eq!(birth.parent, OrganismId::new(9));
    assert_eq!(
        (birth.newborn.id, birth.newborn.genome),
        (OrganismId::new(70), GenomeId::new(80))
    );
    assert_eq!(quota.live(), before.live() + 1);
    assert_eq!(ids, identities(71, 81));
    births.abandon(&mut quota).expect("abandon");
    assert_eq!(quota, before);
}

#[test]
fn malformed_birth_requests_are_refused_before_anything_is_staged() {
    let reproduction = reproduction(0.05);
    let rng = SemanticRng::new(SEED);
    let catalog = catalog();
    let mut quota = GenomeQuota::new(1 << 32, 1 << 32).expect("quota");
    let genome = founder_genome(1, &catalog, &mut quota);
    let before = quota.clone();
    let mutator = mutator(quiet(), &catalog);
    let world = grid();
    let context = BirthContext {
        rng: &rng,
        grid: &world,
        reproduction: &reproduction,
        mutator: &mutator,
        offspring: Offspring::Inherited,
    };
    let at = |tick| BirthRequest {
        intent: intent(&reproduction, &parent(1, [1.0, 1.0], 0), tick, 9.0, 0.5),
        genome: &genome,
    };
    let other = BirthRequest {
        intent: intent(&reproduction, &parent(2, [1.0, 1.0], 0), 6, 9.0, 0.5),
        genome: &genome,
    };
    for requests in [vec![at(5), at(5)], vec![at(5), other]] {
        assert!(matches!(
            resolve_births(
                &context,
                &requests,
                PopulationQuota::new(16, 1).expect("population"),
                &mut identities(1, 1),
                &mut quota,
            ),
            Err(CoreError::StateConflict { .. })
        ));
        assert_eq!(quota, before);
    }
    let empty = staged(
        resolve_births(
            &context,
            &[],
            PopulationQuota::new(16, 16).expect("population"),
            &mut identities(1, 1),
            &mut quota,
        )
        .expect("resolve"),
    );
    assert!(empty.born().is_empty() && empty.rejected().is_empty());
}

#[test]
fn emission_and_reproduction_rules_come_from_the_configuration() {
    let config = appendix_a();
    let policy = EmissionPolicy::from_config(&config).expect("policy");
    assert_eq!(policy.efficiency(), 0.9);
    let reproduction = Reproduction::from_config(&config).expect("rules");
    let got = reproduction.rules();
    assert_eq!(got.birth_cost, 0.05);
    assert_eq!(got.minimum_attempt_energy, 4.0);
    assert_eq!(got.minimum_child_energy, 0.3);
    assert_eq!(got.minimum_parent_energy, 0.3);
    assert_eq!(got.cooldown, TickSpan::new(COOLDOWN));
    assert_eq!(got.offset_radius, 0.25);
    assert_eq!(
        got.controller_interval,
        config.config().controller.evaluation_interval_ticks
    );
    assert_eq!(
        PopulationQuota::from_config(&config, 4_096)
            .expect("population")
            .available(),
        16_384 - 4_096
    );

    // Other positions, visibilities and activations are not implemented.
    let toml = appendix_a_toml();
    for (from, to) in [
        ("position = \"post-motion\"", "position = \"pre-motion\""),
        ("visibility = \"next-tick\"", "visibility = \"same-tick\""),
        (
            "newborn_activation = \"next-tick\"",
            "newborn_activation = \"same-tick\"",
        ),
    ] {
        assert!(toml.contains(from), "{from}");
        assert!(
            ValidatedConfig::from_toml(&toml.replacen(from, to, 1)).is_err(),
            "{to}"
        );
    }
    for efficiency in [-0.1, 1.1, f64::NAN] {
        assert!(EmissionPolicy::new(efficiency).is_err());
    }
    for broken in [
        ReproductionRules {
            minimum_child_energy: 0.0,
            ..rules(0.05)
        },
        ReproductionRules {
            minimum_parent_energy: -1.0,
            ..rules(0.05)
        },
        ReproductionRules {
            birth_cost: f64::NAN,
            ..rules(0.05)
        },
        ReproductionRules {
            offset_radius: f64::INFINITY,
            ..rules(0.05)
        },
        ReproductionRules {
            controller_interval: 0,
            ..rules(0.05)
        },
    ] {
        assert!(Reproduction::new(broken).is_err(), "{broken:?}");
    }
}
