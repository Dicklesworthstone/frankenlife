//! FL-032: phase 7's reproduction proposal on Metal is flife-engine's, bit
//! for bit. The device's Philox4x64-10 is checked against FrankenNumPy's
//! NumPy-exact `PhiloxRng` on random keys and counters, every counter carry
//! included, and against flife-core's `SemanticRng` on semantic addresses;
//! its half-open uniform against flife-core's on every edge of the leading
//! 52 bits. The kernel is checked against `Reproduction::propose` on
//! thousands of organisms under four rule sets: exhausted and cooling-down
//! parents, probabilities of zero and one and of the draw's own uniform,
//! balances at the attempt minimum, every unviable split and viable ones.
//! Every input the reference refuses is refused, and what it never reads is
//! not checked.

use flife_backend_metal::{
    MetalProposal, PROPOSAL_OPS_WORDS, ProposalConstants, ProposalOrganism, ProposalOutcome,
};
use flife_core::CoreError;
use flife_core::clock::{Tick, TickSpan};
use flife_core::ids::{Generation, LineageId, OrganismId};
use flife_core::ledger::EnergyBudget;
use flife_core::rng::{Address, Lane, LaneKind, SemanticRng, Stream, half_open_unit};
use flife_engine::{ParentState, Reproduction, ReproductionProposal, ReproductionRules, Unviable};
use flife_neural::DecodedActions;
use fnp_random::PhiloxRng;

/// `None` off macOS, where `open` must refuse with its typed error.
#[expect(clippy::panic, reason = "test helper")]
fn metal() -> Option<MetalProposal> {
    match MetalProposal::open() {
        Ok(proposal) => Some(proposal),
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

    /// Uniform in `0..n`.
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn word(row: &[u32], at: usize) -> u64 {
    u64::from(row[at]) | (u64::from(row[at + 1]) << 32)
}

/// NumPy's first block for `key` and `counter`.
fn numpy(key: [u64; 2], counter: [u64; 4]) -> [u64; 4] {
    let mut philox = PhiloxRng::new(key, counter);
    [
        philox.next_u64(),
        philox.next_u64(),
        philox.next_u64(),
        philox.next_u64(),
    ]
}

#[test]
fn the_random_blocks_are_numpys_and_flife_cores() {
    let Some(proposal) = metal() else { return };
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    const MAX: u64 = u64::MAX;
    // The counter's carries, each word wrapping in turn, and the whole
    // counter wrapping to zero.
    let carries = [
        [MAX, 0, 0, 0],
        [MAX, MAX, 0, 0],
        [MAX, MAX, MAX, 0],
        [MAX, MAX, MAX, MAX],
        [MAX - 1, MAX, MAX, MAX],
        [MAX, 7, 0, 0],
        [MAX, MAX, 9, 0],
        [0, 0, 0, 0],
    ];
    // Leading-52-bit edges for the uniform: zero, one, the largest, and
    // each bit alone.
    let mut edges = vec![0, (1 << 12) - 1, 1 << 12, MAX, MAX << 12, 1 << 63];
    edges.extend((12..64).map(|bit| 1_u64 << bit));
    let mut rows: Vec<[u64; 7]> = Vec::new();
    for (k, counter) in carries.iter().enumerate() {
        rows.push([
            rng.next(),
            rng.next(),
            counter[0],
            counter[1],
            counter[2],
            counter[3],
            edges[k],
        ]);
    }
    // flife-core's semantic addresses, every stream and lane kind.
    let kinds = [
        LaneKind::Topology,
        LaneKind::Weight,
        LaneKind::Bias,
        LaneKind::MapWeight,
        LaneKind::Affinity,
        LaneKind::MotionResponse,
        LaneKind::SensingScale,
        LaneKind::Direction,
        LaneKind::Position,
        LaneKind::Attempt,
    ];
    let mut semantic = Vec::new();
    for stream in Stream::ALL {
        for kind in kinds {
            let lane = Lane::new(
                u8::try_from(rng.below(256)).expect("a byte"),
                kind,
                rng.below(1 << 48),
            )
            .expect("an index below 2^48");
            let address = Address {
                stream,
                entity: if rng.below(4) == 0 {
                    MAX
                } else {
                    rng.below(1 << 20)
                },
                ordinal: rng.below(1 << 20),
                lane,
                draw: rng.below(4),
            };
            let seed = rng.next();
            semantic.push((rows.len(), seed, address));
            let at = rows.len() % edges.len();
            rows.push([
                seed,
                stream.code(),
                address.entity,
                address.ordinal,
                lane.get(),
                address.draw,
                edges[at],
            ]);
        }
    }
    while !rows.len().is_multiple_of(64) || rows.len() < 4096 {
        let at = rows.len() % edges.len();
        let extra = if at < edges.len() / 2 {
            edges[at]
        } else {
            rng.next()
        };
        rows.push([
            rng.next(),
            rng.next(),
            rng.next(),
            rng.next(),
            rng.next(),
            rng.next(),
            extra,
        ]);
    }
    let mut out: Vec<[u32; PROPOSAL_OPS_WORDS]> = Vec::new();
    proposal
        .proposal_ops(&rows, &mut out)
        .expect("the device computes the blocks");
    assert_eq!(out.len(), rows.len());
    for (row, got) in rows.iter().zip(&out) {
        let expected = numpy([row[0], row[1]], [row[2], row[3], row[4], row[5]]);
        for (k, &word_k) in expected.iter().enumerate() {
            assert_eq!(word(got, 2 * k), word_k, "word {k} of {row:x?}");
            assert_eq!(
                word(got, 8 + 2 * k),
                half_open_unit(word_k).to_bits(),
                "uniform {k} of {row:x?}"
            );
        }
        assert_eq!(
            word(got, 16),
            half_open_unit(row[6]).to_bits(),
            "the uniform of {:#x}",
            row[6]
        );
    }
    for (at, seed, address) in semantic {
        let block = SemanticRng::new(seed).block(address);
        for (k, &word_k) in block.iter().enumerate() {
            assert_eq!(word(&out[at], 2 * k), word_k, "{address:?}");
        }
    }
}

/// The reference's proposal as the port carries it.
fn ported(proposal: ReproductionProposal) -> ProposalOutcome {
    match proposal {
        ReproductionProposal::Exhausted => ProposalOutcome::Exhausted,
        ReproductionProposal::CoolingDown { .. } => ProposalOutcome::CoolingDown,
        ReproductionProposal::NoAttempt => ProposalOutcome::NoAttempt,
        ReproductionProposal::Unviable(Unviable::BelowAttemptEnergy { available, .. }) => {
            ProposalOutcome::BelowAttemptEnergy { available }
        }
        ReproductionProposal::Unviable(Unviable::BirthCostUnaffordable { available, .. }) => {
            ProposalOutcome::BirthCostUnaffordable { available }
        }
        ReproductionProposal::Unviable(Unviable::ChildBelowMinimum { child, .. }) => {
            ProposalOutcome::ChildBelowMinimum { child }
        }
        ReproductionProposal::Unviable(Unviable::ParentBelowMinimum { parent, .. }) => {
            ProposalOutcome::ParentBelowMinimum { parent }
        }
        ReproductionProposal::Intent(intent) => ProposalOutcome::Viable {
            available: intent.split.available,
            child: intent.split.child,
            parent: intent.split.parent,
        },
    }
}

/// An outcome's variant and values as bits, for exact comparison.
fn bits(outcome: ProposalOutcome) -> (usize, [u64; 3]) {
    match outcome {
        ProposalOutcome::Exhausted => (0, [0; 3]),
        ProposalOutcome::CoolingDown => (1, [0; 3]),
        ProposalOutcome::NoAttempt => (2, [0; 3]),
        ProposalOutcome::BelowAttemptEnergy { available } => (3, [available.to_bits(), 0, 0]),
        ProposalOutcome::BirthCostUnaffordable { available } => (4, [available.to_bits(), 0, 0]),
        ProposalOutcome::ChildBelowMinimum { child } => (5, [0, child.to_bits(), 0]),
        ProposalOutcome::ParentBelowMinimum { parent } => (6, [0, 0, parent.to_bits()]),
        ProposalOutcome::Viable {
            available,
            child,
            parent,
        } => (7, [available.to_bits(), child.to_bits(), parent.to_bits()]),
    }
}

const fn rules(birth_cost: f64, attempt: f64, child: f64, parent: f64) -> ReproductionRules {
    ReproductionRules {
        birth_cost,
        minimum_attempt_energy: attempt,
        minimum_child_energy: child,
        minimum_parent_energy: parent,
        cooldown: TickSpan::new(10),
        offset_radius: 0.25,
        controller_interval: 1,
    }
}

/// One generated parent: its port row and the reference's arguments.
struct Case {
    organism: ProposalOrganism,
    parent: ParentState,
    actions: DecodedActions,
    budget: EnergyBudget,
}

#[expect(clippy::expect_used, reason = "test helper")]
fn case(rng: &mut Rng, rules: &ReproductionRules, source: &SemanticRng) -> Case {
    let exhausted = rng.below(20) == 0;
    let remaining = match rng.below(10) {
        0 => 0.0,
        1 => rules.minimum_attempt_energy * rng.unit(),
        2 => rules.birth_cost * (0.5 + rng.unit()),
        3 => f64::from_bits(rules.birth_cost.to_bits() + rng.below(3)),
        4 => rules.birth_cost + rules.minimum_child_energy * 4.0 * rng.unit(),
        5 => 1.0e300 * rng.unit(),
        6 => rules.minimum_attempt_energy,
        7 => rules.minimum_attempt_energy.next_down().max(0.0),
        _ => 100.0 * rng.unit(),
    };
    let budget = if exhausted {
        let mut budget = EnergyBudget::new(1.0).expect("a budget");
        budget.pay(1.0).expect("the payment");
        budget
    } else {
        EnergyBudget::new(remaining).expect("a budget")
    };
    let tick = rng.below(1_000);
    let deadline = match rng.below(6) {
        0 => tick + 1,
        1 => tick,
        2 => tick.saturating_sub(1),
        3 => 0,
        _ => tick.saturating_sub(rng.below(50)),
    };
    let entity = if rng.below(16) == 0 {
        u64::MAX - rng.below(4)
    } else {
        rng.below(1 << 24)
    };
    // The attempt's own uniform `u`: a probability of `u` makes no attempt
    // and one ulp above it does.
    let [word, ..] = source.block(Address {
        stream: Stream::Birth,
        entity,
        ordinal: tick,
        lane: Lane::new(0, LaneKind::Attempt, 0).expect("the attempt lane"),
        draw: 0,
    });
    let drawn = half_open_unit(word);
    let attempt_probability = match rng.below(12) {
        0 => 0.0,
        1 => -0.0,
        2 => 1.0,
        3 => f64::from_bits(1),
        4 => 1.0 - f64::EPSILON / 2.0,
        5 => drawn,
        6 => drawn.next_up(),
        _ => rng.unit(),
    };
    let child_fraction = match rng.below(10) {
        0 => f64::from_bits(1),
        1 => 1.0 - f64::EPSILON / 2.0,
        2 => 0.5,
        _ => 0.05 + 0.9 * rng.unit(),
    };
    Case {
        organism: ProposalOrganism {
            entity,
            tick,
            cooldown_deadline: deadline,
            remaining: budget.remaining(),
            exhausted: budget.is_exhausted(),
            attempt_probability,
            child_fraction,
        },
        parent: ParentState {
            id: OrganismId::new(entity),
            lineage: LineageId::new(rng.below(64)),
            generation: Generation::new(u32::try_from(rng.below(1_000)).expect("small")),
            position: [rng.unit(), rng.unit()],
            cooldown_deadline: Tick::new(deadline),
        },
        actions: DecodedActions {
            attempt_rate: 0.0,
            attempt_probability,
            child_fraction,
            emission_gate: 0.0,
            emission: None,
        },
        budget,
    }
}

#[test]
fn proposals_on_the_device_are_the_references() {
    let Some(mut proposal) = metal() else { return };
    let mut rng = Rng(0x0123_4567_89AB_CDEF);
    let mut seen = [0_usize; 8];
    // The preset's rules; no birth cost or attempt minimum with the
    // smallest positive minimums; a birth cost above most balances; and a
    // child's minimum unlike the parent's. The constants are the engine's
    // (Reproduction::proposal_constants).
    for (seed, rules) in [
        (42, rules(0.05, 3.0, 0.3, 0.3)),
        (
            rng.next(),
            rules(0.0, 0.0, f64::from_bits(1), f64::from_bits(1)),
        ),
        (rng.next(), rules(60.0, 0.0, 0.3, 0.3)),
        (rng.next(), rules(0.05, 0.0, 0.4, 0.2)),
    ] {
        let reproduction = Reproduction::new(rules).expect("valid rules");
        let source = SemanticRng::new(seed);
        let cases: Vec<Case> = (0..4_096)
            .map(|_| case(&mut rng, &rules, &source))
            .collect();
        let organisms: Vec<ProposalOrganism> = cases.iter().map(|case| case.organism).collect();
        let mut out = Vec::new();
        proposal
            .settle(&reproduction.proposal_constants(seed), &organisms, &mut out)
            .expect("the device proposes");
        assert_eq!(out.len(), cases.len());
        for (case, &got) in cases.iter().zip(&out) {
            let expected = reproduction
                .propose(
                    &source,
                    &case.parent,
                    Tick::new(case.organism.tick),
                    &case.actions,
                    &case.budget,
                )
                .expect("the reference proposes");
            let expected = bits(ported(expected));
            assert_eq!(bits(got), expected, "{:?} under {rules:?}", case.organism);
            seen[expected.0] += 1;
        }
    }
    println!("proposals by kind: {seen:?}");
    assert!(seen.iter().all(|&count| count > 0), "{seen:?}");
}

#[test]
fn proposals_the_reference_refuses_are_refused() {
    let Some(mut proposal) = metal() else { return };
    let rules = rules(0.05, 3.0, 0.3, 0.3);
    let reproduction = Reproduction::new(rules).expect("valid rules");
    let source = SemanticRng::new(7);
    let valid = ProposalOrganism {
        entity: 3,
        tick: 20,
        cooldown_deadline: 5,
        remaining: 10.0,
        exhausted: false,
        attempt_probability: 1.0,
        child_fraction: 0.5,
    };
    let with = |edit: &dyn Fn(&mut ProposalOrganism)| {
        let mut organism = valid;
        edit(&mut organism);
        organism
    };
    // The reference's proposal for a row whose balance a budget holds.
    let reference = |organism: &ProposalOrganism| -> Result<ProposalOutcome, CoreError> {
        let parent = ParentState {
            id: OrganismId::new(organism.entity),
            lineage: LineageId::new(0),
            generation: Generation::FOUNDER,
            position: [0.0, 0.0],
            cooldown_deadline: Tick::new(organism.cooldown_deadline),
        };
        let actions = DecodedActions {
            attempt_rate: 0.0,
            attempt_probability: organism.attempt_probability,
            child_fraction: organism.child_fraction,
            emission_gate: 0.0,
            emission: None,
        };
        let budget = EnergyBudget::new(organism.remaining)?;
        reproduction
            .propose(
                &source,
                &parent,
                Tick::new(organism.tick),
                &actions,
                &budget,
            )
            .map(ported)
    };
    // Whether the reference refuses a row: through `propose` where a budget
    // holds the balance, and through `split`, which `propose` calls on an
    // attempt, where none can.
    let refuses = |organism: &ProposalOrganism| -> bool {
        if EnergyBudget::new(organism.remaining).is_ok() {
            reference(organism).is_err()
        } else {
            reproduction
                .split(organism.remaining, organism.child_fraction)
                .is_err()
        }
    };
    let refused = [
        with(&|o| o.attempt_probability = f64::NAN),
        with(&|o| o.attempt_probability = -f64::from_bits(1)),
        with(&|o| o.attempt_probability = 1.0_f64.next_up()),
        with(&|o| o.attempt_probability = f64::INFINITY),
        with(&|o| o.remaining = -1.0),
        with(&|o| o.remaining = f64::NAN),
        with(&|o| o.remaining = f64::INFINITY),
        with(&|o| o.child_fraction = 0.0),
        with(&|o| o.child_fraction = 1.0),
        with(&|o| o.child_fraction = f64::NAN),
        with(&|o| o.child_fraction = -0.5),
    ];
    let mut out = Vec::new();
    for organism in refused {
        assert!(refuses(&organism), "{organism:?}");
        let result = proposal.settle(
            &reproduction.proposal_constants(7),
            &[valid, organism, valid],
            &mut out,
        );
        assert!(
            matches!(result, Err(CoreError::StateConflict { .. })),
            "{organism:?}: {result:?}"
        );
        assert!(out.is_empty());
    }
    // Inputs the reference never reads: a cooling-down or exhausted parent
    // with NaN everything else, and a parent that makes no attempt with a
    // NaN child fraction.
    let unread = [
        with(&|o| {
            o.cooldown_deadline = 21;
            o.attempt_probability = f64::NAN;
            o.child_fraction = f64::NAN;
        }),
        with(&|o| {
            o.remaining = 0.0;
            o.exhausted = true;
            o.attempt_probability = f64::NAN;
            o.child_fraction = f64::NAN;
        }),
        with(&|o| {
            o.attempt_probability = 0.0;
            o.child_fraction = f64::NAN;
        }),
    ];
    let expected = [
        ProposalOutcome::CoolingDown,
        ProposalOutcome::Exhausted,
        ProposalOutcome::NoAttempt,
    ];
    proposal
        .settle(&reproduction.proposal_constants(7), &unread, &mut out)
        .expect("nothing the reference reads is refused");
    for ((organism, &got), &want) in unread.iter().zip(&out).zip(&expected) {
        assert_eq!(bits(got), bits(want), "{organism:?}");
        if !organism.exhausted {
            let reference = reference(organism).expect("the reference proposes");
            assert_eq!(bits(got), bits(reference), "{organism:?}");
        }
    }
    // Constants out of range, before any command.
    for edit in [
        |c: &mut ProposalConstants| c.birth_cost = f64::NAN,
        |c: &mut ProposalConstants| c.minimum_attempt_energy = f64::INFINITY,
        |c: &mut ProposalConstants| c.minimum_child_energy = f64::NAN,
        |c: &mut ProposalConstants| c.minimum_parent_energy = f64::NEG_INFINITY,
    ] {
        let mut bad = reproduction.proposal_constants(7);
        edit(&mut bad);
        let result = proposal.settle(&bad, &[valid], &mut out);
        assert!(
            matches!(result, Err(CoreError::NumericalInvalid { .. })),
            "{bad:?}: {result:?}"
        );
    }
}
