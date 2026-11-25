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
use flife_core::model::FIELD_ECOLOGY_V1_UNGATED_MOTION;
use std::ops::Range;

use flife_field::settle::{
    ChemistryOrganism, ChemistryOutcome, ChemistryPacket, EmissionAction, EmissionOrganism,
    EmissionOutcome, ExhaustedAt, MUTATION_LAYERS, MotionConstants, MotionOrganism, MotionOutcome,
    MotionV2Extras, MotorSteeringInputs, MutationBirth, MutationConstants, MutationCounts,
    MutationLayer, MutationValues, ProposalOrganism, ProposalOutcome, SettleBackend,
};
use flife_field::state::CHANNELS;
use flife_field::{
    Bins, Deposits, ExchangeScratch, FieldState, GrantPacket, split_dual_carrier_emission,
};
use flife_genome::{BirthAttempt, Mutator, ParameterValues, Prepared, TopologyId};
use flife_neural::{DenseTopology, WeightDtype, Weights};

use super::{
    DecisionLane, ModelVariant, PACKETS_PER_ORGANISM, Rules, SettleReport, SettleTileBuf, Settled,
    Stages, Working, compositions_into, fit, in_runs_mut, parameter_count, settle_all,
    settle_stages,
};
use crate::chemistry::{UptakeSettlement, effective_affinity};
use crate::costs::{MaintenanceBill, MaintenanceSettlement};
use crate::emission::{CachedFootprint, EmissionSettlement};
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
    motion_extras: Vec<MotionV2Extras>,
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
    pub(super) fn reserve(&mut self, rows: usize, needs_motion_v2: bool) -> Result<(), CoreError> {
        fit(&mut self.chemistry, rows, "phase 6 chemistry rows")?;
        fit(
            &mut self.packets,
            rows.saturating_mul(PACKETS_PER_ORGANISM),
            "phase 6 packets",
        )?;
        fit(&mut self.chemistry_out, rows, "phase 6 chemistry outcomes")?;
        fit(&mut self.motion, rows, "phase 6 motion rows")?;
        if needs_motion_v2 {
            fit(&mut self.motion_extras, rows, "phase 6 motion v2 extras")?;
        } else {
            self.motion_extras = Vec::new();
        }
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
            capacity_bytes(&self.motion_extras),
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
///
/// Returns the number of populated `settle_tiles` when the large-world
/// work-stealing tile path ran (`> 0`), or `0` when results live in `lanes`.
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
    settle_tiles: &mut Vec<SettleTileBuf>,
    settle: &mut Settle,
) -> Result<usize, CoreError> {
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
                return Ok(0);
            }
            Err(error) => {
                report.reference_ticks = report.reference_ticks.saturating_add(1);
                failure = Some(error.to_string());
            }
        }
    }
    compositions_into(observed, granted, lanes.len(), compositions)?;
    let active_tiles = settle_all(
        rules,
        population,
        (by_demand, granted, compositions),
        from,
        watched,
        working,
        lanes,
        settle_tiles,
    )?;
    if failure.is_some() {
        report.failure = failure;
        *backend = None;
    }
    Ok(active_tiles)
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
    // Before any dispatch: a model or exchange kernel the port does not
    // implement is refused here, not after its chemistry has run.
    if !rules.exchange.is_grid_cic() {
        return Err(CoreError::MissingCapability {
            capability: format!(
                "a settle backend's emission under exchange kernel `{}`",
                rules.exchange.name()
            ),
        });
    }
    let motion_constants = motion_constants(rules)?;
    let needs_motion_v2 = rules.v2.needs_motion_v2();
    let motor_constants = rules.v2.motor();
    let (by_demand, granted) = grants;
    let SettleWork {
        chemistry,
        packets,
        chemistry_out,
        motion,
        motion_extras,
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
    motion_extras.clear();
    emission.clear();
    emitters.clear();
    proposal.clear();
    proposers.clear();
    settled.clear();

    // Each organism's packets, in cell order as the reference settles them,
    // and what its genome gives maintenance and motion.
    let mut live = population.ascending();
    for (index, organism) in working.iter().enumerate() {
        let genome =
            live.next(organism.committed.id)
                .0
                .ok_or_else(|| CoreError::StateConflict {
                    operation: "read genome",
                    detail: format!("{} has no genome", organism.committed.id),
                })?;
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
        let post_phase4_h0 = organism.held.recurrent_state_array()[0];
        let affinity = effective_affinity(
            traits,
            post_phase4_h0,
            &rules.v2,
            rules.chemistry.affinity_norm_limit(),
        )?;
        chemistry.push(ChemistryOrganism {
            affinity,
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
        if needs_motion_v2 {
            motion_extras.push(MotionV2Extras {
                basal_multiplier: organism.basal_multiplier,
                motor: organism
                    .actions
                    .motor
                    .map_or_else(MotorSteeringInputs::default, MotorSteeringInputs::from),
            });
        }
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
    if needs_motion_v2 {
        device.motion_v2(
            &motion_constants,
            motor_constants.as_ref(),
            motion,
            motion_extras,
            motion_out,
        )?;
    } else {
        device.motion(&motion_constants, motion, motion_out)?;
    }
    expect_outcomes("motion", motion_out.len(), working.len())?;
    let emission_v2_active = rules.v2.emission().any_enabled();
    for (organism, outcome) in working.iter().zip(motion_out.iter()) {
        if outcome.died.is_some() {
            emitters.push(usize::MAX);
        } else {
            emitters.push(emission.len());
            let (deposit_position, effective_emission) = if emission_v2_active {
                let ctx =
                    rules.emission_context(organism, outcome.position, outcome.velocity, from)?;
                let plan = rules.v2.emission().plan_with_recovery_alpha(
                    &rules.grid,
                    rules.step,
                    outcome.remaining,
                    false,
                    organism.actions.emission.as_ref(),
                    &ctx,
                    rules.v2.emission_recovery_alpha(),
                )?;
                (
                    plan.deposit_position,
                    plan.effective_emission.map(EmissionAction::from),
                )
            } else {
                (
                    outcome.position,
                    organism.actions.emission.map(EmissionAction::from),
                )
            };
            emission.push(EmissionOrganism {
                remaining: outcome.remaining,
                position: deposit_position,
                emission: effective_emission,
            });
        }
    }
    let energy_floor = rules.v2.emission().energy_floor();
    if energy_floor > 0.0 {
        device.emission_with_floor(
            &rules.grid,
            rules.emission.efficiency(),
            energy_floor,
            emission,
            emission_out,
        )?;
    } else {
        device.emission(
            &rules.grid,
            rules.emission.efficiency(),
            emission,
            emission_out,
        )?;
    }
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
                    attempt_probability: rules.effective_attempt_probability(organism)?,
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
        if let Some(trail) = lane.trail_emissions.as_mut() {
            trail.clear();
        }
        if let Some(byproducts) = lane.byproducts.as_mut() {
            byproducts.clear();
        }
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
                    exchange,
                    flows,
                    emissions,
                    trail_emissions,
                    byproducts,
                    transactions,
                    ..
                } = lane;
                let mut live = rows
                    .get(start)
                    .map(|first| population.ascending_from(first.committed.id));
                for (offset, slot) in run.iter_mut().enumerate() {
                    let index = start + offset;
                    let missing = || differ(format!("no outcome for row {index}"));
                    let organism = rows.get(index).ok_or_else(missing)?;
                    let traits = live
                        .as_mut()
                        .and_then(|walk| walk.next(organism.committed.id).0)
                        .ok_or_else(|| CoreError::StateConflict {
                            operation: "read genome",
                            detail: format!("{} has no genome", organism.committed.id),
                        })?
                        .traits();
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
                        traits,
                        &mut stages,
                        from,
                        flows,
                        emissions,
                        trail_emissions.as_mut(),
                        byproducts.as_mut(),
                        exchange,
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
///
/// The port's motion scales the response by the uptake ratio (§5.4), and the
/// host takes its velocity and position without recomputing them, so a model
/// whose gain is not that ratio is refused here and the tick runs on the
/// reference.
fn motion_constants(rules: &Rules) -> Result<MotionConstants, CoreError> {
    if rules.variant == ModelVariant::UngatedMotion {
        return Err(CoreError::MissingCapability {
            capability: format!(
                "a settle backend's motion under {FIELD_ECOLOGY_V1_UNGATED_MOTION}"
            ),
        });
    }
    let (basal, sensor, sensor_unit_scale, sensor_scale_exponent, neural) =
        rules.costs.coefficients();
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
        sensor_scale_exponent,
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
        _organism: &Working,
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
        position: [f64; 2],
        velocity: [f64; 2],
        from: Tick,
        budget: &mut EnergyBudget,
        emissions: &mut Deposits,
        trail_emissions: Option<&mut Deposits>,
        exchange_scratch: &mut ExchangeScratch,
        footprint: &mut CachedFootprint,
    ) -> Result<(EmissionSettlement, f32, Tick), CoreError> {
        // EmissionPolicy::emit, its request and offer recomputed here, its
        // footprint split the backend's (at the position the backend moved
        // the organism to, which `displace` checked, or the V2-04 offset
        // position computed identically on host).
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
        let (
            deposit_position,
            effective_emission,
            energy_floor,
            next_readiness,
            next_cooldown_deadline,
        ) = if rules.v2.emission().any_enabled() {
            let ctx = rules.emission_context(organism, position, velocity, from)?;
            let plan = rules.v2.emission().plan_with_recovery_alpha(
                &rules.grid,
                rules.step,
                available,
                budget.is_exhausted(),
                organism.actions.emission.as_ref(),
                &ctx,
                rules.v2.emission_recovery_alpha(),
            )?;
            (
                plan.deposit_position,
                plan.effective_emission,
                plan.energy_floor,
                plan.next_readiness,
                plan.next_cooldown_deadline,
            )
        } else {
            (
                position,
                organism.actions.emission,
                0.0,
                organism.secretory_readiness,
                organism.emission_cooldown_deadline,
            )
        };
        let Some(action) = effective_emission.as_ref() else {
            return if outcome.staged {
                Err(differ(
                    "emission staged for an organism that does not emit".into(),
                ))
            } else {
                Ok((nothing, next_readiness, next_cooldown_deadline))
            };
        };
        let fraction = action.spend_fraction;
        if !(0.0..=1.0).contains(&fraction) {
            return Err(CoreError::NumericalInvalid {
                quantity: "emission spending fraction",
                detail: format!("{fraction} is outside [0, 1]"),
            });
        }
        let surplus = if energy_floor == 0.0 {
            available
        } else {
            (available - energy_floor).max(0.0)
        };
        let requested = if budget.is_exhausted() {
            0.0
        } else {
            (fraction * surplus).min(available)
        };
        if requested == 0.0 {
            return if outcome.staged {
                Err(differ("emission staged a zero request".into()))
            } else {
                Ok((nothing, next_readiness, next_cooldown_deadline))
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
        let settlement_offered =
            if let (Some(dc), Some(trail_deposits)) = (rules.v2.dual_carrier(), trail_emissions) {
                let split = split_dual_carrier_emission(
                    requested,
                    rules.emission.efficiency(),
                    dc.trail_emission_share(),
                    &action.direction,
                )?;
                footprint.push_deposit(
                    emissions,
                    &rules.grid,
                    rules.exchange,
                    organism.committed.id,
                    deposit_position,
                    split.paid_0,
                    split.offered_0,
                    &split.direction_0,
                    exchange_scratch,
                )?;
                footprint.push_deposit(
                    trail_deposits,
                    &rules.grid,
                    rules.exchange,
                    organism.committed.id,
                    deposit_position,
                    split.paid_1,
                    split.offered_1,
                    &split.direction_1,
                    exchange_scratch,
                )?;
                split.offered_0 + split.offered_1
            } else {
                emissions.push_split(
                    &rules.grid,
                    organism.committed.id,
                    requested,
                    offered,
                    &action.direction,
                    outcome.packets(),
                )?;
                offered
            };
        let cost = budget.pay(requested)?;
        check_paid("emission", cost.paid, outcome.paid)?;
        check_balance("emission", budget, outcome.remaining, outcome.exhausted)?;
        Ok((
            EmissionSettlement {
                available,
                cost,
                offered: settlement_offered,
            },
            next_readiness,
            next_cooldown_deadline,
        ))
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
        check_probability(rules.effective_attempt_probability(organism)?)?;
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
/// backend: for [`resolve_births_with_wave_preparer`](crate::reproduction::resolve_births_with_wave_preparer).
#[derive(Debug, Default)]
pub(super) struct FirstCandidates {
    /// Each served birth's parent, member, and its values' range.
    births: Vec<(OrganismId, TopologyId, Range<usize>, MutationCounts)>,
    values: Option<ParameterValues>,
}

impl FirstCandidates {
    /// Clear the wave's entries while keeping its bounded staging capacity.
    fn clear(&mut self) {
        self.births.clear();
        if let Some(values) = &mut self.values {
            match values {
                ParameterValues::F32(values) => values.clear(),
                ParameterValues::F16(values) => values.clear(),
            }
        }
    }

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

/// Bounded-wave mutator for phase 8 on the world's settle backend (FL-032,
/// FL-043): reuses a single wave-sized parameter staging buffer (at most
/// `PARALLEL_PROPOSAL_BUDGET_BYTES`) instead of materializing every birth in
/// the tick at once.
pub(super) struct WaveMutator<'a> {
    settle: &'a mut Settle,
    rules: &'a Rules,
    mutator: &'a Mutator<'a>,
    dtype: WeightDtype,
    enabled: bool,
    scratch: FirstCandidates,
    rows: Vec<MutationBirth>,
    counts: Vec<MutationCounts>,
}

impl<'a> WaveMutator<'a> {
    pub(super) fn new(
        settle: &'a mut Settle,
        rules: &'a Rules,
        mutator: &'a Mutator<'a>,
        dtype: WeightDtype,
        inherited: bool,
    ) -> Self {
        let rates = &rules.mutation;
        let enabled =
            inherited && (rates.replacement_probability > 0.0 || rates.weight_probability > 0.0);
        Self {
            settle,
            rules,
            mutator,
            dtype,
            enabled,
            scratch: FirstCandidates::default(),
            rows: Vec::new(),
            counts: Vec::new(),
        }
    }
}

impl crate::reproduction::FirstCandidateWave for WaveMutator<'_> {
    fn active(&self) -> bool {
        self.enabled && self.settle.backend.is_some()
    }

    fn prepare_wave(&mut self, requests: &[&BirthRequest<'_>]) {
        self.scratch.clear();
        self.rows.clear();
        self.counts.clear();
        if !self.active() || requests.is_empty() {
            return;
        }
        let Settle {
            backend, report, ..
        } = self.settle;
        let Some(device) = backend.as_deref_mut() else {
            return;
        };
        let values = self.scratch.values.get_or_insert_with(|| match self.dtype {
            WeightDtype::F32 => ParameterValues::F32(Vec::new()),
            WeightDtype::F16 => ParameterValues::F16(Vec::new()),
        });
        for request in requests {
            let attempt = BirthAttempt {
                parent: request.intent.parent,
                tick: request.intent.tick,
            };
            let start = values.weights().len();
            let Ok(member) = self.mutator.unmutated(request.genome, attempt, values) else {
                continue;
            };
            let end = values.weights().len();
            match self
                .rules
                .catalog
                .topology(member)
                .and_then(|topology| mutation_birth(attempt, topology))
            {
                Some(row) => {
                    self.rows.push(row);
                    self.scratch.births.push((
                        attempt.parent,
                        member,
                        start..end,
                        MutationCounts::default(),
                    ));
                }
                _ => truncate(values, start),
            }
        }
        if self.rows.is_empty() {
            return;
        }
        let rates = &self.rules.mutation;
        let constants = MutationConstants {
            seed: self.rules.rng.seed(),
            replacement_probability: rates.replacement_probability,
            weight_probability: rates.weight_probability,
            weight_standard_deviation: rates.weight_standard_deviation,
        };
        let served = match values {
            ParameterValues::F32(values) => device.mutation(
                &constants,
                &self.rows,
                MutationValues::F32(values),
                &mut self.counts,
            ),
            ParameterValues::F16(values) => device.mutation(
                &constants,
                &self.rows,
                MutationValues::F16(values),
                &mut self.counts,
            ),
        }
        .and_then(|()| check_counts(&self.rows, &self.counts));
        match served {
            Ok(()) => {
                for (birth, served) in self.scratch.births.iter_mut().zip(&self.counts) {
                    birth.3 = *served;
                }
                report.mutated = report
                    .mutated
                    .saturating_add(u64::try_from(self.scratch.births.len()).unwrap_or(u64::MAX));
            }
            Err(error) => {
                report.reference_ticks = report.reference_ticks.saturating_add(1);
                report.failure = Some(error.to_string());
                *backend = None;
                self.scratch.clear();
            }
        }
    }

    fn prepared(&self) -> Vec<(OrganismId, Prepared<'_>)> {
        self.scratch.prepared()
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

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use flife_core::ValidatedConfig;
    use flife_core::ids::{GenomeId, IdSequence, LineageId, OrganismId};
    use flife_field::settle::{
        ChemistryConstants, ChemistryOrganism, ChemistryOutcome, ChemistryPacket, EmissionOrganism,
        EmissionOutcome, MotionConstants, MotionOrganism, MotionOutcome, MutationBirth,
        MutationConstants, MutationCounts, MutationValues, ProposalConstants, ProposalOrganism,
        ProposalOutcome, SettleBackend,
    };
    use flife_field::{FieldGrid, FieldState};
    use flife_genome::{
        Genome, GenomeQuota, Mutator, TopologyCatalog, TraitLimits, WeightDtype, founder,
        initial_scale,
    };

    use super::*;
    use crate::reproduction::{
        BirthContext, BirthRequest, BirthResolution, CapacityPolicy, Identities, Offspring,
        ParentState, PopulationQuota, Reproduction, resolve_births_with_wave_preparer,
        resolve_births_with_workers,
    };

    const PRESET: &str = include_str!("../../../../presets/reference-small-world.toml");

    #[derive(Debug, Default)]
    struct WaveLog {
        wave_rows: Vec<usize>,
        wave_bytes: Vec<usize>,
    }

    #[derive(Debug)]
    struct RecordingSettle {
        log: Arc<Mutex<WaveLog>>,
        fail_on_call: Option<usize>,
        calls: usize,
    }

    impl SettleBackend for RecordingSettle {
        fn describe(&self) -> String {
            "recording-settle".into()
        }

        fn chemistry(
            &mut self,
            _observed: &FieldState,
            _constants: &ChemistryConstants,
            _organisms: &[ChemistryOrganism],
            _packets: &[ChemistryPacket],
            _out: &mut Vec<ChemistryOutcome>,
        ) -> Result<(), CoreError> {
            Err(CoreError::MissingCapability {
                capability: "test stub".into(),
            })
        }

        fn motion(
            &mut self,
            _constants: &MotionConstants,
            _organisms: &[MotionOrganism],
            _out: &mut Vec<MotionOutcome>,
        ) -> Result<(), CoreError> {
            Err(CoreError::MissingCapability {
                capability: "test stub".into(),
            })
        }

        fn emission(
            &mut self,
            _grid: &FieldGrid,
            _efficiency: f64,
            _organisms: &[EmissionOrganism],
            _out: &mut Vec<EmissionOutcome>,
        ) -> Result<(), CoreError> {
            Err(CoreError::MissingCapability {
                capability: "test stub".into(),
            })
        }

        fn proposal(
            &mut self,
            _constants: &ProposalConstants,
            _organisms: &[ProposalOrganism],
            _out: &mut Vec<ProposalOutcome>,
        ) -> Result<(), CoreError> {
            Err(CoreError::MissingCapability {
                capability: "test stub".into(),
            })
        }

        fn mutation(
            &mut self,
            _constants: &MutationConstants,
            births: &[MutationBirth],
            values: MutationValues<'_>,
            out: &mut Vec<MutationCounts>,
        ) -> Result<(), CoreError> {
            self.calls += 1;
            let bytes = match &values {
                MutationValues::F32(slice) => std::mem::size_of_val(*slice),
                MutationValues::F16(slice) => std::mem::size_of_val(*slice),
            };
            {
                let mut log = self.log.lock().expect("lock");
                log.wave_rows.push(births.len());
                log.wave_bytes.push(bytes);
            }
            if self.fail_on_call == Some(self.calls) {
                return Err(CoreError::StateConflict {
                    operation: "recording mutation",
                    detail: format!("planted failure on wave {}", self.calls),
                });
            }
            out.clear();
            // Perturb one parameter per birth cleanly so candidates are not no-ops.
            let mut offset = 0usize;
            for birth in births {
                match &values {
                    MutationValues::F32(_) => {}
                    MutationValues::F16(_) => {}
                }
                let count = birth.parameters;
                out.push(MutationCounts {
                    replaced: 0,
                    perturbed: 1,
                });
                offset += count;
            }
            let _ = offset;
            if let MutationValues::F32(slice) = values {
                let mut at = 0usize;
                for birth in births {
                    if let Some(first) = slice.get_mut(at) {
                        *first += 0.01;
                    }
                    at += birth.parameters;
                }
            }
            Ok(())
        }
    }

    #[test]
    fn wave_mutator_bounds_staging_per_wave_and_falls_back_cleanly_on_error() {
        let config = ValidatedConfig::from_toml(PRESET).expect("preset");
        let rules = Rules::from_config(
            &config,
            crate::NumericalProfile::reference_f32().expect("profile"),
        )
        .expect("rules");
        let catalog = TopologyCatalog::from_config(&config).expect("catalog");
        let limits = TraitLimits::from_config(&config).expect("limits");
        let reproduction = Reproduction::from_config(&config).expect("reproduction");
        let mutator = Mutator::new(rules.rng, rules.mutation, &catalog, limits).expect("mutator");
        let mut quota = GenomeQuota::new(1 << 30, 1 << 30).expect("quota");
        let parents: Vec<Genome> = (1_u64..=120)
            .map(|id| {
                founder(
                    &rules.rng,
                    OrganismId::new(id),
                    GenomeId::new(id),
                    &catalog,
                    &mut quota,
                    &limits,
                    initial_scale(&config),
                )
                .expect("founder")
            })
            .collect();
        let budget = EnergyBudget::new(9.0).expect("budget");
        let requests: Vec<BirthRequest<'_>> = parents
            .iter()
            .zip(1_u64..)
            .map(|(genome, id)| {
                let parent = ParentState {
                    id: OrganismId::new(id),
                    lineage: LineageId::new(id),
                    generation: flife_core::ids::Generation::FOUNDER,
                    position: [10.0, 10.0],
                    cooldown_deadline: Tick::ZERO,
                };
                let intent = match reproduction
                    .attempted(&parent, Tick::new(5), 0.5, &budget)
                    .expect("viable")
                {
                    ReproductionProposal::Intent(intent) => intent,
                    other => panic!("expected viable intent, got {other:?}"),
                };
                BirthRequest { intent, genome }
            })
            .collect();
        let context = BirthContext {
            rng: &rules.rng,
            grid: &rules.grid,
            reproduction: &reproduction,
            mutator: &mutator,
            offspring: Offspring::Inherited,
            capacity: CapacityPolicy::PauseBeforeCommit,
        };
        let ids0 = Identities {
            organisms: IdSequence::resume(Some(1000)),
            genomes: IdSequence::resume(Some(1000)),
        };

        // 1. All waves succeed: 120 F32 births (~4.6 MiB total) must split into
        //    >= 3 bounded waves, each holding <= 2 MiB of parameter staging.
        let log = Arc::new(Mutex::new(WaveLog::default()));
        let mut settle = Settle {
            backend: Some(Box::new(RecordingSettle {
                log: Arc::clone(&log),
                fail_on_call: None,
                calls: 0,
            })),
            ..Settle::default()
        };
        let mut ids = ids0.clone();
        let mut q = quota.clone();
        let mut wave = WaveMutator::new(&mut settle, &rules, &mutator, WeightDtype::F32, true);
        let staged = resolve_births_with_wave_preparer(
            &context,
            &mut wave,
            &requests,
            PopulationQuota::new(1024, 120).expect("pop"),
            &mut ids,
            &mut q,
            1,
        )
        .expect("staged");
        assert!(matches!(staged, BirthResolution::Staged(b) if b.born().len() == 120));
        let recorded = log.lock().expect("lock");
        assert!(
            recorded.wave_rows.len() >= 3,
            "120 F32 births should split across >=3 waves, got {:?}",
            recorded.wave_rows
        );
        assert!(
            recorded.wave_bytes.iter().all(|&bytes| bytes <= (2 << 20)),
            "every wave must stay within 2 MiB staging cap: {:?}",
            recorded.wave_bytes
        );
        assert_eq!(settle.report.mutated, 120);
        assert_eq!(settle.report.reference_ticks, 0);

        // 2. Planted failure on wave 1 drops the backend cleanly and reproduces
        //    the reference resolution bit for bit.
        let fail_log = Arc::new(Mutex::new(WaveLog::default()));
        let mut failing_settle = Settle {
            backend: Some(Box::new(RecordingSettle {
                log: Arc::clone(&fail_log),
                fail_on_call: Some(1),
                calls: 0,
            })),
            ..Settle::default()
        };
        let mut fail_ids = ids0.clone();
        let mut fail_q = quota.clone();
        let mut fail_wave = WaveMutator::new(
            &mut failing_settle,
            &rules,
            &mutator,
            WeightDtype::F32,
            true,
        );
        let fallback = resolve_births_with_wave_preparer(
            &context,
            &mut fail_wave,
            &requests,
            PopulationQuota::new(1024, 120).expect("pop"),
            &mut fail_ids,
            &mut fail_q,
            1,
        )
        .expect("fallback");
        assert!(failing_settle.backend.is_none());
        assert_eq!(failing_settle.report.reference_ticks, 1);
        assert_eq!(failing_settle.report.mutated, 0);

        let mut ref_ids = ids0;
        let mut ref_q = quota;
        let reference = resolve_births_with_workers(
            &context,
            &[],
            &requests,
            PopulationQuota::new(1024, 120).expect("pop"),
            &mut ref_ids,
            &mut ref_q,
            1,
        )
        .expect("reference");
        let (BirthResolution::Staged(got), BirthResolution::Staged(want)) = (fallback, reference)
        else {
            panic!("expected both staged");
        };
        assert_eq!(got.born().len(), want.born().len());
        for (a, b) in got.born().iter().zip(want.born()) {
            assert_eq!(a.newborn, b.newborn);
            assert!(a.genome.parameters().same_bits(&b.genome.parameters()));
        }
    }
}
