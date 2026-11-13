//! FL-032: phase 6 on a [`SettleBackend`] (plan §5.3–§5.6, §8.2, §10.7).
//!
//! The backend computes each organism's chemistry, maintenance, motion and
//! emission (`flife_field::settle`); the host composes them with the
//! reference's own [`settle_stages`], so the order of stages, the
//! exhaustion checks, the ledger flows, the watched organism's record and
//! the reproduction proposal are the reference's code, not a copy. At each
//! stage the host replays the backend's payment on its own budget with the
//! reference's operations ([`Device`]) and refuses a settlement whose
//! amounts or balances differ by a bit; it recomputes emission's request
//! and offer, and stages each deposit's split through
//! [`Deposits::push_split`]'s checks. The replay runs on the lanes, each
//! lane taking the run the reference would, so each lane's ledger flows and
//! staged emissions are what the reference's lane holds.
//!
//! What the host does not recompute it takes from the backend: what
//! chemistry absorbed and gained, the requested toxicity and maintenance
//! bill, the motion request, the velocity and position (only checked to lie
//! on the torus), and each deposit's split within `push_split`'s bounds.
//! That those are the reference's is shown by racing the worlds, not by
//! this replay (`flife-backend-metal`'s `tests/settle_world.rs`).
//!
//! No row is written until every organism has settled: [`settle_stages`]
//! returns each organism's [`Settled`], and the rows take them only after
//! the whole replay succeeds. Any error, from the backend or the replay,
//! leaves the rows as they were, and the tick's phase 6 runs on the
//! reference ([`settle_with`]).

use flife_core::CoreError;
use flife_core::clock::Tick;
use flife_core::ids::OrganismId;
use flife_core::ledger::{EnergyBudget, Payment, sub_down};
use flife_core::memory::{capacity_bytes, total};
use flife_field::settle::{
    ChemistryOrganism, ChemistryOutcome, ChemistryPacket, EmissionAction, EmissionOrganism,
    EmissionOutcome, ExhaustedAt, MotionConstants, MotionOrganism, MotionOutcome, SettleBackend,
};
use flife_field::state::CHANNELS;
use flife_field::{Bins, Deposits, FieldState, GrantPacket};

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

/// Phases 6 and 7 with `device`'s chemistry, maintenance, motion and
/// emission; see the module documentation. On error no row is written.
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
        settled,
    } = work;
    chemistry.clear();
    packets.clear();
    motion.clear();
    emission.clear();
    emitters.clear();
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

    // The replay, on the lanes as the reference runs them.
    for lane in lanes.iter_mut() {
        lane.flows.clear();
        lane.emissions.clear();
        lane.transactions = None;
    }
    settled.extend(working.iter().map(Settled::of));
    {
        let rows: &[Working] = working;
        let (chemistry_out, motion_out, emission_out, emitters) =
            (&*chemistry_out, &*motion_out, &*emission_out, &*emitters);
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
}
