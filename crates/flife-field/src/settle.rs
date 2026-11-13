//! FL-032's port for a device phase 6 (plan §5.3–§5.6, §8.2, §10.7, §18.2):
//! [`SettleBackend`] and the plain data it takes and gives. flife-engine
//! owns phase 6's rules; a backend cannot depend on the engine, so the port
//! names the rules' constants and each organism's state as numbers.
//!
//! A backend computes three stretches of each organism's phase 6, in order:
//! - [`SettleBackend::chemistry`]: every granted packet's composition and
//!   efficiency, the compensated sums, the gain credited and the toxicity
//!   paid;
//! - [`SettleBackend::motion`]: the exhaustion check after chemistry, the
//!   maintenance bill and its payment, the check after it, the motion
//!   response, its payment and the displacement, and the check after
//!   motion;
//! - [`SettleBackend::emission`]: for an organism alive after motion, its
//!   payment and offer, and its deposit's footprint split;
//! - [`SettleBackend::proposal`]: for an organism alive after emission,
//!   phase 7's reproduction proposal: the cooldown, the attempt's draw and
//!   the energy split.
//!
//! The engine replays every payment on its own budgets with the reference's
//! operations and refuses a backend whose amounts or balances differ from
//! them, records the ledger flows and stages the deposits in identity order,
//! and builds each birth intent from the proposal's split. Any error hands
//! the tick's phases 6 and 7 to the reference.

use flife_core::CoreError;

use crate::grid::FieldGrid;
use crate::state::{CHANNELS, FieldState};

/// The constants of `bounded-affinity-v1` the settlement uses (flife-engine's
/// `Chemistry`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChemistryConstants {
    /// `η_max ∈ (0, 1]`.
    pub maximum_efficiency: f64,
    /// The affinity gain `k ≥ 0`.
    pub gain: f64,
    /// The discrimination threshold `b > 0`.
    pub threshold: f64,
    /// The toxicity sensitivity `κ > 0`.
    pub toxicity_sensitivity: f64,
    /// The affinity norm limit, in `(0, 1]`.
    pub affinity_norm_limit: f64,
}

/// One organism's chemistry: its affinity, its budget at the start of the
/// tick, and its granted packets, `packets` of the call's packet list from
/// `first`, in the order the reference settles them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChemistryOrganism {
    /// The heritable affinity direction `a`.
    pub affinity: [f64; CHANNELS],
    /// The budget's balance.
    pub remaining: f64,
    /// Whether the budget is exhausted.
    pub exhausted: bool,
    /// The index of the organism's first packet.
    pub first: usize,
    /// How many packets it was granted.
    pub packets: usize,
}

/// One granted packet: its cell in the observed field and `u_c`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChemistryPacket {
    /// The cell, row-major as the field numbers them.
    pub cell: usize,
    /// The granted energy.
    pub granted: f64,
}

/// One organism's settlement: flife-engine's `UptakeSettlement` and the
/// budget after it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChemistryOutcome {
    /// `U = Σ_c u_c`.
    pub absorbed: f64,
    /// `G = Σ_c max(η_c, 0) u_c`.
    pub gain: f64,
    /// The requested toxicity `κ Σ_c max(−η_c, 0) u_c`.
    pub requested_toxicity: f64,
    /// The toxicity paid.
    pub paid_toxicity: f64,
    /// The budget's balance after the gain and the toxicity.
    pub remaining: f64,
    /// Whether the budget is exhausted after them.
    pub exhausted: bool,
}

/// A motion response's terms: energy first, then the channels
/// (flife-engine's `RESPONSE_TERMS`).
pub const MOTION_TERMS: usize = 1 + CHANNELS;

/// The run's constants maintenance and motion use: flife-engine's
/// `MaintenanceCosts`, `Motion` and `uptake_ratio` arguments, and the
/// torus.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MotionConstants {
    /// `Δt`, the timestep's seconds per tick.
    pub step_seconds: f64,
    /// `costs.basal_energy_per_second`.
    pub basal: f64,
    /// `costs.sensor_energy_per_second_at_unit_scale`.
    pub sensor: f64,
    /// `costs.sensor_unit_scale_world_units`.
    pub sensor_unit_scale: f64,
    /// `costs.neural_energy_per_parameter_evaluation`.
    pub neural: f64,
    /// `Motion::decay`, `e^(−γΔt)`.
    pub decay: f64,
    /// `Motion::force_coefficient`.
    pub force_coefficient: f64,
    /// `chemistry.maximum_speed`.
    pub maximum_speed: f64,
    /// `chemistry.maximum_acceleration`.
    pub maximum_acceleration: f64,
    /// `costs.motion_energy_per_speed_squared_second`.
    pub cost: f64,
    /// `chemistry.motion_response_norm_limit`.
    pub response_norm_limit: f64,
    /// `chemistry.uptake_capacity_per_second`.
    pub uptake_capacity: f64,
    /// The torus's width.
    pub world_width: f64,
    /// The torus's height.
    pub world_height: f64,
}

/// One organism as chemistry left it: its budget, what maintenance bills,
/// and what motion reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MotionOrganism {
    /// The budget's balance after chemistry.
    pub remaining: f64,
    /// Whether the budget is exhausted after chemistry.
    pub exhausted: bool,
    /// Whether its controller was evaluated this tick.
    pub evaluated: bool,
    /// Its controller's parameter count.
    pub parameters: u32,
    /// Its heritable sensing scale.
    pub sensing_scale: f64,
    /// The uptake chemistry absorbed, `U`.
    pub absorbed: f64,
    /// Its velocity at the start of the tick.
    pub velocity: [f64; 2],
    /// Its position at the start of the tick.
    pub position: [f64; 2],
    /// Its heritable motion response `(χ_e, χ_1, …, χ_32)`.
    pub response: [f64; MOTION_TERMS],
    /// Its sensor gradients, in the response's order.
    pub gradients: [[f64; 2]; MOTION_TERMS],
}

/// The stage whose exhaustion check found the budget exhausted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExhaustedAt {
    /// Chemistry, before maintenance or motion ran.
    Chemistry,
    /// Maintenance; motion did not run.
    Maintenance,
    /// Motion.
    Motion,
}

/// Maintenance's settlement: flife-engine's `MaintenanceSettlement`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MaintenanceOutcome {
    /// The bill's total, as requested.
    pub requested: f64,
    /// The total paid.
    pub paid: f64,
    /// The basal share paid.
    pub basal: f64,
    /// The sensor share paid.
    pub sensor: f64,
    /// The controller share paid.
    pub controller: f64,
}

/// Motion's settlement: flife-engine's `MotionSettlement`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MovementOutcome {
    /// The proposed velocity after the speed limit.
    pub proposed: [f64; 2],
    /// The velocity applied.
    pub velocity: [f64; 2],
    /// The motion cost requested.
    pub requested: f64,
    /// The motion cost paid.
    pub paid: f64,
    /// Whether the proposal was unaffordable and scaled down.
    pub scaled: bool,
}

/// One organism after maintenance and motion.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MotionOutcome {
    /// Maintenance's settlement, if it ran.
    pub maintenance: Option<MaintenanceOutcome>,
    /// Motion's settlement, if it ran.
    pub movement: Option<MovementOutcome>,
    /// The velocity after the stages.
    pub velocity: [f64; 2],
    /// The position after the stages.
    pub position: [f64; 2],
    /// The budget's balance after the stages.
    pub remaining: f64,
    /// Whether the budget is exhausted after the stages.
    pub exhausted: bool,
    /// The first exhaustion check that found the budget exhausted.
    pub died: Option<ExhaustedAt>,
}

/// An organism's decoded emission, as plain data: flife-neural's
/// `Emission`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EmissionAction {
    /// The chemical direction, `||d||₂ ≤ 1` as computed.
    pub direction: [f64; CHANNELS],
    /// The fraction of the energy available at the emission stage to spend.
    pub spend_fraction: f64,
}

/// One organism that reaches emission: alive after motion, so its budget
/// holds energy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EmissionOrganism {
    /// The budget's balance after motion, finite and positive.
    pub remaining: f64,
    /// Its position after motion.
    pub position: [f64; 2],
    /// Its decoded emission, if it emits.
    pub emission: Option<EmissionAction>,
}

/// One organism's emission: flife-engine's `EmissionSettlement`, the
/// deposit it staged, and the budget after it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EmissionOutcome {
    /// The energy available at the stage.
    pub available: f64,
    /// The payment requested.
    pub requested: f64,
    /// The payment made.
    pub paid: f64,
    /// The offer to the field.
    pub offered: f64,
    /// Whether a deposit was staged (a positive request); its offer may
    /// still be zero.
    pub staged: bool,
    /// The deposit's packets, `(cell, amount)`, in stencil order: the
    /// first [`EmissionOutcome::packet_count`] entries.
    pub packets: [(usize, f64); 4],
    /// How many packets the deposit has.
    pub packet_count: usize,
    /// The budget's balance after the stage.
    pub remaining: f64,
    /// Whether the payment exhausted the budget: the organism dies at
    /// emission.
    pub exhausted: bool,
}

impl EmissionOutcome {
    /// The deposit's packets.
    #[must_use]
    pub fn packets(&self) -> &[(usize, f64)] {
        self.packets
            .get(..self.packet_count)
            .unwrap_or(&self.packets)
    }
}

/// FL-032's port for a device phase 6. Each method must give the
/// reference's values bit for bit (see the module documentation), and on
/// success `out` holds one outcome per organism, in order.
pub trait SettleBackend: std::fmt::Debug + Send {
    /// The backend and its device, for run records.
    fn describe(&self) -> String;

    /// Each organism's chemistry against `observed`, the field its grants
    /// were resolved on.
    ///
    /// # Errors
    /// Any. The world then computes the tick's phase 6 on the reference.
    fn chemistry(
        &mut self,
        observed: &FieldState,
        constants: &ChemistryConstants,
        organisms: &[ChemistryOrganism],
        packets: &[ChemistryPacket],
        out: &mut Vec<ChemistryOutcome>,
    ) -> Result<(), CoreError>;

    /// Each organism's maintenance and motion, with the exhaustion checks
    /// after chemistry, maintenance and motion.
    ///
    /// # Errors
    /// Any. The world then computes the tick's phase 6 on the reference.
    fn motion(
        &mut self,
        constants: &MotionConstants,
        organisms: &[MotionOrganism],
        out: &mut Vec<MotionOutcome>,
    ) -> Result<(), CoreError>;

    /// The emission of each organism alive after motion, on `grid` with
    /// transfer efficiency `efficiency`.
    ///
    /// # Errors
    /// Any. The world then computes the tick's phase 6 on the reference.
    fn emission(
        &mut self,
        grid: &FieldGrid,
        efficiency: f64,
        organisms: &[EmissionOrganism],
        out: &mut Vec<EmissionOutcome>,
    ) -> Result<(), CoreError>;

    /// The reproduction proposal of each organism alive after emission.
    ///
    /// # Errors
    /// Any. The world then computes the tick's phases 6 and 7 on the
    /// reference.
    fn proposal(
        &mut self,
        constants: &ProposalConstants,
        organisms: &[ProposalOrganism],
        out: &mut Vec<ProposalOutcome>,
    ) -> Result<(), CoreError>;
}

/// The constants of phase 7's proposal (flife-engine's `Reproduction`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProposalConstants {
    /// The world seed, the key of every random block.
    pub seed: u64,
    /// The minimum post-emission energy for an attempt.
    pub minimum_attempt_energy: f64,
    /// `c_b`, the birth cost.
    pub birth_cost: f64,
    /// The minimum child endowment.
    pub minimum_child_energy: f64,
    /// The minimum energy the parent keeps.
    pub minimum_parent_energy: f64,
}

/// One organism's state for its proposal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProposalOrganism {
    /// Its stable identity, the draw's entity.
    pub entity: u64,
    /// The attempt tick, the draw's ordinal.
    pub tick: u64,
    /// The first tick at which it may attempt.
    pub cooldown_deadline: u64,
    /// Its post-emission balance.
    pub remaining: f64,
    /// Whether its budget is exhausted.
    pub exhausted: bool,
    /// The decoded probability of one attempt this tick.
    pub attempt_probability: f64,
    /// The decoded child share `β`.
    pub child_fraction: f64,
}

/// One organism's proposal, as flife-engine's `Reproduction::propose`
/// gives it, without the parent's identities the host adds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ProposalOutcome {
    /// Its budget is exhausted.
    Exhausted,
    /// The attempt tick is before its cooldown deadline.
    CoolingDown,
    /// Eligible, and the draw made no attempt.
    NoAttempt,
    /// Attempted, with a balance below the attempt minimum.
    BelowAttemptEnergy {
        /// The balance.
        available: f64,
    },
    /// Attempted, with nothing left after the birth cost.
    BirthCostUnaffordable {
        /// The balance.
        available: f64,
    },
    /// Attempted, and the child's share is below its minimum.
    ChildBelowMinimum {
        /// The child's share.
        child: f64,
    },
    /// Attempted, and the parent's share is below its minimum.
    ParentBelowMinimum {
        /// The parent's share.
        parent: f64,
    },
    /// A viable split.
    Viable {
        /// The balance, `+0` for a zero.
        available: f64,
        /// The child's endowment.
        child: f64,
        /// What the parent keeps.
        parent: f64,
    },
}
