//! FL-032: phases 6 and 7 on a [`SettleBackend`] (plan §5.3–§5.6, §7.4,
//! §8.2, §10.7).
//!
//! The backend computes each organism's chemistry, maintenance, motion,
//! emission and reproduction proposal (`flife_field::settle`); the host
//! composes them with the reference's own [`settle_stages`], so the order of
//! stages, the exhaustion checks, the ledger flows and the watched
//! organism's record are the reference's code, not a copy. At each stage the
//! host replays the backend's payment on its own budget with the reference's
//! operations ([`Device`]) and refuses a settlement whose amounts or
//! balances differ by a bit; it recomputes emission's request and offer, and
//! stages each deposit's split through [`Deposits::push_split`]'s checks.
//! For the proposal it checks the cooldown and the attempt probability, and
//! for an attempt recomputes the split with the reference's
//! `Reproduction::attempted`, which also builds the birth intent. The replay
//! runs on the lanes, each lane taking the run the reference would, so each
//! lane's ledger flows and staged emissions are what the reference's lane
//! holds.
//!
//! What the host does not recompute it takes from the backend: what
//! chemistry absorbed and gained, the requested toxicity and maintenance
//! bill, the motion request, the velocity and position (only checked to lie
//! on the torus), each deposit's split within `push_split`'s bounds, and
//! whether the attempt's draw made an attempt. That those are the
//! reference's is shown by racing the worlds, not by this replay
//! (`flife-backend-metal`'s `tests/settle_world.rs`).
//!
//! No row is written until every organism has settled: [`settle_stages`]
//! returns each organism's [`Settled`], and the rows take them only after
//! the whole replay succeeds. Any error, from the backend or the replay,
//! leaves the rows as they were, and the tick's phases 6 and 7 run on the
//! reference ([`settle_with`]).

use flife_core::CoreError;
use flife_core::clock::Tick;
use flife_core::ids::OrganismId;
use flife_core::ledger::{EnergyBudget, Payment, sub_down};
use flife_core::memory::{capacity_bytes, total};
use std::ops::Range;

use flife_field::settle::{
    ChemistryOrganism, ChemistryOutcome, ChemistryPacket, EmissionAction, EmissionOrganism,
    EmissionOutcome, ExhaustedAt, MUTATION_LAYERS, MotionConstants, MotionOrganism, MotionOutcome,
    MutationBirth, MutationConstants, MutationCounts, MutationLayer, MutationValues,
    ProposalOrganism, ProposalOutcome, SettleBackend,
};
use flife_field::state::CHANNELS;
use flife_field::{Bins, Deposits, FieldState, GrantPacket};
use flife_genome::{BirthAttempt, Mutator, ParameterValues, Prepared, TopologyId};
use flife_neural::{DenseTopology, WeightDtype, Weights};

use super::{
    DecisionLane, PACKETS_PER_ORGANISM, Rules, SettleReport, Settled, Stages, Working,
    compositions_into, fit, genome_of, in_runs_mut, parameter_count, settle_all, settle_stages,
};
use crate::chemistry::UptakeSettlement;
use crate::costs::{MaintenanceBill, MaintenanceSettlement};
use crate::emission::EmissionSettlement;
use crate::lifecycle::{BudgetStage, Vitality};
use crate::motion::MotionSettlement;
use crate::population::Population;
use crate::reproduction::{
    BirthRequest, ParentState, ReproductionProposal, Unviable, check_probability,
};

/// The world's phase 6 backend and the host's lists for it.
#[derive(Debug, Default)]
pub(super) struct Settle {
    pub(super) backend: Option<Box<dyn SettleBackend>>,
    pub(super) report: SettleReport,
    pub(super) work: SettleWork,
}

/// The host's lists for a backend's phase 6, reusable from tick to tick.
#[derive(Debug, Default)]
pub(super) struct SettleWork {
    chemistry: Vec<ChemistryOrganism>,
    packets: Vec<ChemistryPacket>,
    chemistry_out: Vec<ChemistryOutcome>,
    motion: Vec<MotionOrganism>,
    motion_out: Vec<MotionOutcome>,
    emission: Vec<EmissionOrganism>,
    emission_out: Vec<EmissionOutcome>,
    /// For each organism, the index of its emission outcome, or
    /// `usize::MAX` for one that does not reach emission.
    emitters: Vec<usize>,
    proposal: Vec<ProposalOrganism>,
    proposal_out: Vec<ProposalOutcome>,
    /// For each organism, the index of its proposal, or `usize::MAX` for
    /// one that does not reach phase 7.
    proposers: Vec<usize>,
    settled: Vec<Settled>,
}

impl SettleWork {
    /// Room for `rows` organisms, so that a tick within them allocates
    /// nothing on the host (FL-043).
    ///
    /// # Errors
    /// [`CoreError::AllocationFailed`] when the host refuses the room.
    pub(super) fn reserve(&mut self, rows: usize) -> Result<(), CoreError> {
        fit(&mut self.chemistry, rows, "phase 6 chemistry rows")?;
        fit(
            &mut self.packets,
            rows.saturating_mul(PACKETS_PER_ORGANISM),
            "phase 6 packets",
        )?;
        fit(&mut self.chemistry_out, rows, "phase 6 chemistry outcomes")?;
        fit(&mut self.motion, rows, "phase 6 motion rows")?;
        fit(&mut self.motion_out, rows, "phase 6 motion outcomes")?;
        fit(&mut self.emission, rows, "phase 6 emission rows")?;
        fit(&mut self.emission_out, rows, "phase 6 emission outcomes")?;
        fit(&mut self.emitters, rows, "phase 6 emitters")?;
        fit(&mut self.proposal, rows, "phase 7 proposal rows")?;
        fit(&mut self.proposal_out, rows, "phase 7 proposals")?;
        fit(&mut self.proposers, rows, "phase 7 proposers")?;
        fit(&mut self.settled, rows, "phase 6 settlements")
    }

    /// The bytes the lists hold.
    pub(super) fn allocated_bytes(&self) -> u64 {
        total([
            capacity_bytes(&self.chemistry),
            capacity_bytes(&self.packets),
            capacity_bytes(&self.chemistry_out),
            capacity_bytes(&self.motion),
            capacity_bytes(&self.motion_out),
            capacity_bytes(&self.emission),
            capacity_bytes(&self.emission_out),
            capacity_bytes(&self.emitters),
            capacity_bytes(&self.proposal),
            capacity_bytes(&self.proposal_out),
            capacity_bytes(&self.proposers),
            capacity_bytes(&self.settled),
        ])
    }
}

/// Phases 6 and 7 for every working organism: on the world's settle
/// backend if it has one ([`settle_on`]); otherwise, or when the backend
/// or the host's replay of it meets an error, on the reference
/// ([`settle_all`]) with the packets' compositions computed first. A
/// backend that fails where the reference does not is dropped, and its
/// report says why.
#[expect(
    clippy::too_many_arguments,
    reason = "private stage runner of World::prepare; the arguments are the tick's working state"
)]
pub(super) fn settle_with(
    rules: &Rules,
    population: &Population,
    observed: &FieldState,
    grants: (&Bins, &[GrantPacket]),
    compositions: &mut Vec<[f64; CHANNELS]>,
    from: Tick,
    watched: Option<OrganismId>,
    working: &mut [Working],
    lanes: &mut [DecisionLane],
    settle: &mut Settle,
) -> Result<(), CoreError> {
    let (by_demand, granted) = grants;
    let Settle {
        backend,
        report,
        work,
    } = settle;
    let mut failure = None;
    if let Some(device) = backend.as_deref_mut() {
        match settle_on(
            device, rules, population, observed, grants, from, watched, working, lanes, work,
        ) {
            Ok(()) => {
                report.ticks = report.ticks.saturating_add(1);
                return Ok(());
            }
            Err(error) => {
                report.reference_ticks = report.reference_ticks.saturating_add(1);
                failure = Some(error.to_string());
            }
        }
    }
    compositions_into(observed, granted, lanes.len(), compositions)?;
    settle_all(
        rules,
        population,
        (by_demand, granted, compositions),
        from,
        watched,
        working,
        lanes,
    )?;
    if failure.is_some() {
        report.failure = failure;
        *backend = None;
    }
    Ok(())
}

/// Phases 6 and 7 with `device`'s chemistry, maintenance, motion, emission
/// and proposal; see the module documentation. On error no row is written.
#[expect(
    clippy::too_many_arguments,
    reason = "private stage runner of World::prepare; the arguments are the tick's working state"
)]
fn settle_on(
    device: &mut dyn SettleBackend,
    rules: &Rules,
    population: &Population,
    observed: &FieldState,
    grants: (&Bins, &[GrantPacket]),
    from: Tick,
    watched: Option<OrganismId>,
    working: &mut [Working],
    lanes: &mut [DecisionLane],
    work: &mut SettleWork,
) -> Result<(), CoreError> {
    let (by_demand, granted) = grants;
    let SettleWork {
        chemistry,
        packets,
        chemistry_out,
        motion,
        motion_out,
        emission,
        emission_out,
        emitters,
        proposal,
        proposal_out,
        proposers,
        settled,
    } = work;
    chemistry.clear();
    packets.clear();
    motion.clear();
    emission.clear();
    emitters.clear();
    proposal.clear();
    proposers.clear();
    settled.clear();

    // Each organism's packets, in cell order as the reference settles them,
    // and what its genome gives maintenance and motion.
    for (index, organism) in working.iter().enumerate() {
        let genome = genome_of(population, organism.committed.id)?;
        let traits = genome.traits();
        let first = packets.len();
        for packet in by_demand
            .bucket(index)
            .iter()
            .filter_map(|&at| granted.get(at as usize))
        {
            packets.push(ChemistryPacket {
                cell: packet.cell,
                granted: packet.granted,
            });
        }
        chemistry.push(ChemistryOrganism {
            affinity: *traits.affinity(),
            remaining: organism.budget.remaining(),
            exhausted: organism.budget.is_exhausted(),
            first,
            packets: packets.len() - first,
        });
        motion.push(MotionOrganism {
            remaining: 0.0,
            exhausted: false,
            evaluated: organism.evaluated,
            parameters: parameter_count(&rules.catalog, genome)?,
            sensing_scale: traits.sensing_scale(),
            absorbed: 0.0,
            velocity: organism.velocity,
            position: organism.position,
            response: *traits.motion_response(),
            gradients: organism.gradients,
        });
    }

    device.chemistry(
        observed,
        &rules.chemistry.settle_constants(),
        chemistry,
        packets,
        chemistry_out,
    )?;
    expect_outcomes("chemistry", chemistry_out.len(), working.len())?;
    for (row, outcome) in motion.iter_mut().zip(chemistry_out.iter()) {
        row.remaining = outcome.remaining;
        row.exhausted = outcome.exhausted;
        row.absorbed = outcome.absorbed;
    }
    device.motion(&motion_constants(rules)?, motion, motion_out)?;
    expect_outcomes("motion", motion_out.len(), working.len())?;
    for (organism, outcome) in working.iter().zip(motion_out.iter()) {
        if outcome.died.is_some() {
            emitters.push(usize::MAX);
        } else {
            emitters.push(emission.len());
            emission.push(EmissionOrganism {
                remaining: outcome.remaining,
                position: outcome.position,
                emission: organism.actions.emission.map(EmissionAction::from),
            });
        }
    }
    device.emission(
        &rules.grid,
        rules.emission.efficiency(),
        emission,
        emission_out,
    )?;
    expect_outcomes("emission", emission_out.len(), emission.len())?;
    for (organism, &at) in working.iter().zip(emitters.iter()) {
        match emission_out.get(at) {
            Some(outcome) if !outcome.exhausted => {
                proposers.push(proposal.len());
                proposal.push(ProposalOrganism {
                    entity: organism.committed.id.get(),
                    tick: from.get(),
                    cooldown_deadline: organism.cooldown_deadline.get(),
                    remaining: outcome.remaining,
                    exhausted: false,
                    attempt_probability: organism.actions.attempt_probability,
                    child_fraction: organism.actions.child_fraction,
                });
            }
            _ => proposers.push(usize::MAX),
        }
    }
    device.proposal(
        &rules.reproduction.proposal_constants(rules.rng.seed()),
        proposal,
        proposal_out,
    )?;
    expect_outcomes("proposal", proposal_out.len(), proposal.len())?;

    // The replay, on the lanes as the reference runs them.
    for lane in lanes.iter_mut() {
        lane.flows.clear();
        lane.emissions.clear();
        lane.transactions = None;
    }
    settled.extend(working.iter().map(Settled::of));
    {
        let rows: &[Working] = working;
        let (chemistry_out, motion_out, emission_out, emitters, proposal_out, proposers) = (
            &*chemistry_out,
            &*motion_out,
            &*emission_out,
            &*emitters,
            &*proposal_out,
            &*proposers,
        );
        in_runs_mut(
            settled,
            lanes,
            "settle on a backend",
            &|start, run, lane| {
                let DecisionLane {
                    flows,
                    emissions,
                    transactions,
                    ..
                } = lane;
                for (offset, slot) in run.iter_mut().enumerate() {
                    let index = start + offset;
                    let missing = || differ(format!("no outcome for row {index}"));
                    let organism = rows.get(index).ok_or_else(missing)?;
                    let mut stages = Device {
                        chemistry: chemistry_out.get(index).ok_or_else(missing)?,
                        motion: motion_out.get(index).ok_or_else(missing)?,
                        emission: emitters.get(index).and_then(|&at| emission_out.get(at)),
                        proposal: proposers.get(index).and_then(|&at| proposal_out.get(at)),
                    };
                    let record =
                        (watched == Some(organism.committed.id)).then_some(&mut *transactions);
                    *slot = settle_stages(
                        rules,
                        organism,
                        &mut stages,
                        from,
                        flows,
                        emissions,
                        record,
                    )?;
                    stages.check_fate(organism.committed.id, &slot.vitality)?;
                }
                Ok(())
            },
        )?;
    }
    for (organism, settled) in working.iter_mut().zip(settled.iter()) {
        settled.apply(organism);
    }
    Ok(())
}

/// The constants maintenance and motion use, as the port takes them.
fn motion_constants(rules: &Rules) -> Result<MotionConstants, CoreError> {
    let (basal, sensor, sensor_unit_scale, neural) = rules.costs.coefficients();
    let (step_seconds, maximum_acceleration, cost) = rules.motion.step_limit_and_cost();
    if step_seconds.to_bits() != rules.step.seconds_per_tick().to_bits() {
        return Err(differ(format!(
            "motion's timestep {step_seconds:e} is not the world's"
        )));
    }
    Ok(MotionConstants {
        step_seconds,
        basal,
        sensor,
        sensor_unit_scale,
        neural,
        decay: rules.motion.decay(),
        force_coefficient: rules.motion.force_coefficient(),
        maximum_speed: rules.motion.maximum_speed(),
        maximum_acceleration,
        cost,
        response_norm_limit: rules.motion.response_norm_limit(),
        uptake_capacity: rules.uptake_capacity,
        world_width: rules.grid.world_width(),
        world_height: rules.grid.world_height(),
    })
}

fn differ(detail: String) -> CoreError {
    CoreError::StateConflict {
        operation: "replay a settle backend",
        detail,
    }
}

fn expect_outcomes(stage: &str, got: usize, expected: usize) -> Result<(), CoreError> {
    if got == expected {
        Ok(())
    } else {
        Err(differ(format!(
            "{got} {stage} outcomes for {expected} organisms"
        )))
    }
}

/// Whether two values are the same binary64, bit for bit.
fn same(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits()
}

/// One organism's settlements from a backend, which the host replays on its
/// own budget: each stage pays what the reference's operator would pay with
/// the backend's amounts, and the backend's payments and balances must be
/// the host's, bit for bit.
struct Device<'a> {
    chemistry: &'a ChemistryOutcome,
    motion: &'a MotionOutcome,
    /// `None` for an organism the backend found dead before emission.
    emission: Option<&'a EmissionOutcome>,
    /// `None` for an organism the backend found dead before phase 7.
    proposal: Option<&'a ProposalOutcome>,
}

impl Device<'_> {
    /// The backend's first exhaustion must be the host's.
    fn check_fate(&self, organism: OrganismId, vitality: &Vitality) -> Result<(), CoreError> {
        let device = match self.motion.died {
            Some(ExhaustedAt::Chemistry) => Some(BudgetStage::Chemistry),
            Some(ExhaustedAt::Maintenance) => Some(BudgetStage::Maintenance),
            Some(ExhaustedAt::Motion) => Some(BudgetStage::Motion),
            None => self
                .emission
                .filter(|outcome| outcome.exhausted)
                .map(|_| BudgetStage::Emission),
        };
        if device == vitality.died_at() {
            Ok(())
        } else {
            Err(differ(format!(
                "{organism} died at {device:?} on the backend and at {:?} on the host",
                vitality.died_at()
            )))
        }
    }
}

/// The backend's `balance` and exhaustion after `stage` must be `budget`'s.
fn check_balance(
    stage: &str,
    budget: &EnergyBudget,
    balance: f64,
    exhausted: bool,
) -> Result<(), CoreError> {
    if same(budget.remaining(), balance) && budget.is_exhausted() == exhausted {
        Ok(())
    } else {
        Err(differ(format!(
            "{stage} left {balance:e} (exhausted {exhausted}) on the backend and {:e} \
             (exhausted {}) on the host",
            budget.remaining(),
            budget.is_exhausted()
        )))
    }
}

/// The backend's payment must be the host's.
fn check_paid(stage: &str, host: f64, backend: f64) -> Result<(), CoreError> {
    if same(host, backend) {
        Ok(())
    } else {
        Err(differ(format!(
            "{stage} paid {backend:e} on the backend and {host:e} on the host"
        )))
    }
}

impl Stages for Device<'_> {
    fn uptake(
        &mut self,
        _rules: &Rules,
        budget: &mut EnergyBudget,
    ) -> Result<UptakeSettlement, CoreError> {
        // settle_efficiencies' payments: the gain credited first, then the
        // requested toxicity paid.
        let outcome = self.chemistry;
        for (quantity, value) in [("absorbed", outcome.absorbed), ("gain", outcome.gain)] {
            if !(value.is_finite() && value >= 0.0) {
                return Err(differ(format!("chemistry's {quantity} is {value:e}")));
            }
        }
        if outcome.gain > 0.0 {
            budget.credit(outcome.gain)?;
        }
        let toxicity = budget.pay(outcome.requested_toxicity)?;
        check_paid("chemistry", toxicity.paid, outcome.paid_toxicity)?;
        check_balance("chemistry", budget, outcome.remaining, outcome.exhausted)?;
        Ok(UptakeSettlement {
            absorbed: outcome.absorbed,
            gain: outcome.gain,
            toxicity,
        })
    }

    fn maintenance(
        &mut self,
        _rules: &Rules,
        _organism: &Working,
        budget: &mut EnergyBudget,
    ) -> Result<MaintenanceSettlement, CoreError> {
        // MaintenanceBill::settle: the total paid, then split basal first,
        // then sensor, the controller taking the rest.
        let outcome = self
            .motion
            .maintenance
            .ok_or_else(|| differ("the backend did not settle maintenance".into()))?;
        let cost = budget.pay(outcome.requested)?;
        check_paid("maintenance", cost.paid, outcome.paid)?;
        let rest = sub_down(cost.paid, outcome.basal);
        if !(outcome.basal <= cost.paid
            && outcome.sensor <= rest
            && same(outcome.controller, sub_down(rest, outcome.sensor)))
        {
            return Err(differ(format!(
                "maintenance split {:e} as {outcome:?}",
                cost.paid
            )));
        }
        if self.motion.movement.is_none() {
            // The stages ended at maintenance on the backend.
            check_balance(
                "maintenance",
                budget,
                self.motion.remaining,
                self.motion.exhausted,
            )?;
        }
        Ok(MaintenanceSettlement {
            cost,
            paid: MaintenanceBill {
                basal: outcome.basal,
                sensor: outcome.sensor,
                controller: outcome.controller,
            },
        })
    }

    fn movement(
        &mut self,
        _rules: &Rules,
        _organism: &Working,
        _absorbed: f64,
        budget: &mut EnergyBudget,
    ) -> Result<MotionSettlement, CoreError> {
        // Motion::settle's payment: the request when affordable, else the
        // whole balance.
        let outcome = self
            .motion
            .movement
            .ok_or_else(|| differ("the backend did not settle motion".into()))?;
        let available = budget.remaining();
        let affordable = outcome.requested <= available;
        if outcome.scaled == affordable {
            return Err(differ(format!(
                "motion requested {:e} of {available:e}, scaled {}",
                outcome.requested, outcome.scaled
            )));
        }
        let cost = if affordable {
            budget.pay(outcome.requested)?
        } else {
            Payment {
                requested: outcome.requested,
                paid: budget.pay(available)?.paid,
            }
        };
        check_paid("motion", cost.paid, outcome.paid)?;
        check_balance(
            "motion",
            budget,
            self.motion.remaining,
            self.motion.exhausted,
        )?;
        Ok(MotionSettlement {
            proposed: outcome.proposed,
            velocity: outcome.velocity,
            cost,
            scaled: outcome.scaled,
        })
    }

    fn displace(
        &mut self,
        rules: &Rules,
        _organism: &Working,
        _velocity: [f64; 2],
    ) -> Result<[f64; 2], CoreError> {
        // The velocity is the backend's own (`movement` returned it), and the
        // host does not recompute the kinematics: only the torus is checked.
        let [x, y] = self.motion.position;
        if (0.0..rules.grid.world_width()).contains(&x)
            && (0.0..rules.grid.world_height()).contains(&y)
        {
            Ok(self.motion.position)
        } else {
            Err(differ(format!(
                "motion moved to {:?}, off the torus",
                self.motion.position
            )))
        }
    }

    fn emission(
        &mut self,
        rules: &Rules,
        organism: &Working,
        _position: [f64; 2],
        budget: &mut EnergyBudget,
        emissions: &mut Deposits,
    ) -> Result<EmissionSettlement, CoreError> {
        // EmissionPolicy::emit, its request and offer recomputed here, its
        // footprint split the backend's (at the position the backend moved
        // the organism to, which `displace` checked).
        let outcome = self
            .emission
            .ok_or_else(|| differ("the backend did not settle emission".into()))?;
        let available = budget.remaining();
        let nothing = EmissionSettlement {
            available,
            cost: Payment {
                requested: 0.0,
                paid: 0.0,
            },
            offered: 0.0,
        };
        if !same(outcome.available, available) {
            return Err(differ(format!(
                "emission found {:e} on the backend and {available:e} on the host",
                outcome.available
            )));
        }
        let Some(action) = organism.actions.emission.as_ref() else {
            return if outcome.staged {
                Err(differ(
                    "emission staged for an organism that does not emit".into(),
                ))
            } else {
                Ok(nothing)
            };
        };
        let fraction = action.spend_fraction;
        if !(0.0..=1.0).contains(&fraction) {
            return Err(CoreError::NumericalInvalid {
                quantity: "emission spending fraction",
                detail: format!("{fraction} is outside [0, 1]"),
            });
        }
        let requested = if budget.is_exhausted() {
            0.0
        } else {
            (fraction * available).min(available)
        };
        if requested == 0.0 {
            return if outcome.staged {
                Err(differ("emission staged a zero request".into()))
            } else {
                Ok(nothing)
            };
        }
        let offered = rules.emission.efficiency() * requested;
        if !(outcome.staged && same(outcome.requested, requested) && same(outcome.offered, offered))
        {
            return Err(differ(format!(
                "emission requested {:e} and offered {:e} on the backend, {requested:e} and \
                 {offered:e} on the host",
                outcome.requested, outcome.offered
            )));
        }
        emissions.push_split(
            &rules.grid,
            organism.committed.id,
            requested,
            offered,
            &action.direction,
            outcome.packets(),
        )?;
        let cost = budget.pay(requested)?;
        check_paid("emission", cost.paid, outcome.paid)?;
        check_balance("emission", budget, outcome.remaining, outcome.exhausted)?;
        Ok(EmissionSettlement {
            available,
            cost,
            offered,
        })
    }

    fn propose(
        &mut self,
        rules: &Rules,
        organism: &Working,
        parent: &ParentState,
        tick: Tick,
        budget: &EnergyBudget,
    ) -> Result<ReproductionProposal, CoreError> {
        // Reproduction::propose: the cooldown and the probability checked
        // here, whether the draw made an attempt taken from the backend, and
        // an attempt's split recomputed. `settle_stages` proposes only for an
        // organism alive after emission, so the budget is not exhausted and a
        // backend's `Exhausted` is refused.
        let outcome = *self
            .proposal
            .ok_or_else(|| differ("the backend did not propose".into()))?;
        let unexpected = || differ(format!("{} proposed {outcome:?} on the backend", parent.id));
        if tick < parent.cooldown_deadline {
            return if outcome == ProposalOutcome::CoolingDown {
                Ok(ReproductionProposal::CoolingDown {
                    deadline: parent.cooldown_deadline,
                })
            } else {
                Err(unexpected())
            };
        }
        check_probability(organism.actions.attempt_probability)?;
        match outcome {
            ProposalOutcome::NoAttempt => Ok(ReproductionProposal::NoAttempt),
            ProposalOutcome::Exhausted | ProposalOutcome::CoolingDown => Err(unexpected()),
            attempted => {
                let proposal = rules.reproduction.attempted(
                    parent,
                    tick,
                    organism.actions.child_fraction,
                    budget,
                )?;
                if agrees(&proposal, attempted) {
                    Ok(proposal)
                } else {
                    Err(differ(format!(
                        "{} split as {attempted:?} on the backend and {proposal:?} on the host",
                        parent.id
                    )))
                }
            }
        }
    }
}

/// Phase 8's first candidates, their parameters mutated on the settle
/// backend: for [`resolve_births_with`](crate::reproduction::resolve_births_with).
#[derive(Debug, Default)]
pub(super) struct FirstCandidates {
    /// Each served birth's parent, member, and its values' range.
    births: Vec<(OrganismId, TopologyId, Range<usize>, MutationCounts)>,
    values: Option<ParameterValues>,
}

impl FirstCandidates {
    /// Each served birth's first candidate, by parent.
    pub(super) fn prepared(&self) -> Vec<(OrganismId, Prepared<'_>)> {
        let Some(values) = &self.values else {
            return Vec::new();
        };
        let weights = values.weights();
        let mut prepared: Vec<(OrganismId, Prepared<'_>)> = self
            .births
            .iter()
            .filter_map(|(parent, topology, range, counts)| {
                let values = match weights {
                    Weights::F32(values) => Weights::F32(values.get(range.clone())?),
                    Weights::F16(values) => Weights::F16(values.get(range.clone())?),
                };
                Some((
                    *parent,
                    Prepared {
                        topology: *topology,
                        values,
                        replaced: counts.replaced,
                        perturbed: counts.perturbed,
                    },
                ))
            })
            .collect();
        prepared.sort_unstable_by_key(|(parent, _)| *parent);
        prepared
    }
}

/// Phase 8's parameter mutation on the world's settle backend (FL-032):
/// each inherited birth's first candidate, its member chosen and its parent
/// mapped onto it by the reference's `mutator`, mutated in one call. The
/// host takes the mutated values as given: the rest of each child is the
/// mutator's own ([`Mutator::propose_prepared`]). A birth the host cannot
/// prepare is left to the reference; a backend that fails, or does not
/// count one outcome per birth within its parameters, is dropped, and the
/// tick's births are the reference's.
pub(super) fn mutate_first_candidates(
    settle: &mut Settle,
    rules: &Rules,
    mutator: &Mutator<'_>,
    requests: &[BirthRequest<'_>],
    dtype: WeightDtype,
) -> FirstCandidates {
    let Settle {
        backend, report, ..
    } = settle;
    let Some(device) = backend.as_deref_mut() else {
        return FirstCandidates::default();
    };
    let rates = &rules.mutation;
    if requests.is_empty()
        || (rates.replacement_probability <= 0.0 && rates.weight_probability <= 0.0)
    {
        return FirstCandidates::default();
    }
    let mut values = match dtype {
        WeightDtype::F32 => ParameterValues::F32(Vec::new()),
        WeightDtype::F16 => ParameterValues::F16(Vec::new()),
    };
    let mut rows = Vec::new();
    let mut births = Vec::new();
    for request in requests {
        let attempt = BirthAttempt {
            parent: request.intent.parent,
            tick: request.intent.tick,
        };
        let start = values.weights().len();
        let Ok(member) = mutator.unmutated(request.genome, attempt, &mut values) else {
            continue;
        };
        let end = values.weights().len();
        match rules
            .catalog
            .topology(member)
            .and_then(|topology| mutation_birth(attempt, topology))
        {
            Some(row) => {
                rows.push(row);
                births.push((
                    attempt.parent,
                    member,
                    start..end,
                    MutationCounts::default(),
                ));
            }
            _ => truncate(&mut values, start),
        }
    }
    if rows.is_empty() {
        return FirstCandidates::default();
    }
    let constants = MutationConstants {
        seed: rules.rng.seed(),
        replacement_probability: rates.replacement_probability,
        weight_probability: rates.weight_probability,
        weight_standard_deviation: rates.weight_standard_deviation,
    };
    let mut counts = Vec::new();
    let served = match &mut values {
        ParameterValues::F32(values) => {
            device.mutation(&constants, &rows, MutationValues::F32(values), &mut counts)
        }
        ParameterValues::F16(values) => {
            device.mutation(&constants, &rows, MutationValues::F16(values), &mut counts)
        }
    }
    .and_then(|()| check_counts(&rows, &counts));
    match served {
        Ok(()) => {
            for (birth, served) in births.iter_mut().zip(&counts) {
                birth.3 = *served;
            }
            report.mutated = report
                .mutated
                .saturating_add(u64::try_from(births.len()).unwrap_or(u64::MAX));
            FirstCandidates {
                births,
                values: Some(values),
            }
        }
        Err(error) => {
            report.reference_ticks = report.reference_ticks.saturating_add(1);
            report.failure = Some(error.to_string());
            *backend = None;
            FirstCandidates::default()
        }
    }
}

/// `topology`'s first candidate of `attempt` as the port names it, or
/// `None` for a member deeper than the port carries.
fn mutation_birth(attempt: BirthAttempt, topology: &DenseTopology) -> Option<MutationBirth> {
    let shapes = topology.layers();
    if shapes.len() > MUTATION_LAYERS {
        return None;
    }
    let mut layers = [MutationLayer::default(); MUTATION_LAYERS];
    for (slot, shape) in layers.iter_mut().zip(shapes) {
        *slot = MutationLayer {
            inputs: shape.inputs,
            outputs: shape.outputs,
            weights: shape.weights,
            biases: shape.biases,
        };
    }
    Some(MutationBirth {
        parent: attempt.parent.get(),
        tick: attempt.tick.get(),
        candidate: 0,
        layers,
        depth: shapes.len(),
        parameters: topology.parameter_count(),
    })
}

/// `values` back to its first `length`.
fn truncate(values: &mut ParameterValues, length: usize) {
    match values {
        ParameterValues::F32(values) => values.truncate(length),
        ParameterValues::F16(values) => values.truncate(length),
    }
}

/// One count per birth, none past its parameters.
fn check_counts(rows: &[MutationBirth], counts: &[MutationCounts]) -> Result<(), CoreError> {
    if counts.len() != rows.len() {
        return Err(differ(format!(
            "the backend counted {} mutations for {} births",
            counts.len(),
            rows.len()
        )));
    }
    for (row, count) in rows.iter().zip(counts) {
        let mutated = u64::from(count.replaced) + u64::from(count.perturbed);
        if mutated > u64::try_from(row.parameters).unwrap_or(u64::MAX) {
            return Err(differ(format!(
                "the backend mutated {mutated} of {}'s {} parameters",
                row.parent, row.parameters
            )));
        }
    }
    Ok(())
}

/// Whether the host's proposal for an attempt is the backend's, bit for
/// bit.
fn agrees(host: &ReproductionProposal, backend: ProposalOutcome) -> bool {
    match (host, backend) {
        (
            ReproductionProposal::Unviable(Unviable::BelowAttemptEnergy { available, .. }),
            ProposalOutcome::BelowAttemptEnergy { available: theirs },
        )
        | (
            ReproductionProposal::Unviable(Unviable::BirthCostUnaffordable { available, .. }),
            ProposalOutcome::BirthCostUnaffordable { available: theirs },
        )
        | (
            ReproductionProposal::Unviable(Unviable::ChildBelowMinimum {
                child: available, ..
            }),
            ProposalOutcome::ChildBelowMinimum { child: theirs },
        )
        | (
            ReproductionProposal::Unviable(Unviable::ParentBelowMinimum {
                parent: available, ..
            }),
            ProposalOutcome::ParentBelowMinimum { parent: theirs },
        ) => same(*available, theirs),
        (
            ReproductionProposal::Intent(intent),
            ProposalOutcome::Viable {
                available,
                child,
                parent,
            },
        ) => {
            same(intent.split.available, available)
                && same(intent.split.child, child)
                && same(intent.split.parent, parent)
        }
        _ => false,
    }
}
