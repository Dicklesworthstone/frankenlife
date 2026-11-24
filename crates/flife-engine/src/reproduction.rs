//! Reproduction proposals and the birth transaction (plan §7.4, §7.5,
//! §8.1 phases 7 and 8, §8.2, INV-05, INV-06, A.3 item 10, Appendix C.5).
//!
//! **Phase 7.** [`Reproduction::propose`] reads the post-emission budget and
//! changes nothing:
//! - A dead parent, one whose budget is exhausted, cannot reproduce.
//! - A parent is eligible only once the attempt tick `n`, the committed
//!   input tick of the advance, has reached its cooldown deadline.
//! - An eligible parent makes one Bernoulli attempt per tick. The
//!   probability is `p = 1 − e^(−λΔt)`, decoded by `flife_neural::decode`.
//!   The draw is word 0 of `(birth, parent, n, lane (attempt, 0), 0)`, so
//!   it depends on the parent's stable identity and the tick only.
//! - An attempt must pass the minimum attempt energy, checked against the
//!   post-emission budget `E`. It must then afford the birth cost `c_b`
//!   and leave both a viable child and a viable parent. The split is
//!   `R = E − c_b`, rounded down, the child's endowment `β R`, rounded to
//!   nearest, and the parent's share `R − β R`, rounded down. So
//!   `child + parent ≤ E − c_b` holds exactly, and both minimums are
//!   checked on the values that will be committed. C.5: `E = 10`, `c_b = 1`
//!   and `β = 1/3` give exactly 3 and 6, with 1 dissipated.
//! - A failed attempt never resets the cooldown.
//!
//! **Phase 8.** [`resolve_births`] stages the tick's viable attempts as one
//! transaction, in ascending parent identity whatever order they arrive
//! in. For each:
//! 1. One live-population slot is reserved against the fixed logical
//!    population quota. This happens before any genome work, so the quota
//!    is enforced early.
//! 2. The mutated child is constructed and validated by
//!    `flife_genome::Mutator::propose` (under
//!    `field-ecology-v1-fresh-offspring`, FL-041, the child's genome is
//!    instead a founder's drawn at its own identity, [`Offspring::Fresh`]).
//!    Through [`resolve_births_with`], a birth's first candidate can come
//!    with its parameters already mutated elsewhere (FL-032's device), for
//!    `Mutator::propose_prepared`. The fixed logical genome quota is
//!    charged before any parameter buffer is allocated. A birth whose
//!    candidates are all rejected is itself rejected and charges nothing
//!    (`reject-birth-no-charge`). It takes no slot and no identity.
//! 3. The child receives the next organism and genome identities, in that
//!    order. Its position is the parent's post-motion position plus an
//!    offset uniform by area in the configured disk, `R√u₀` at angle
//!    `2πu₁`, from words 0 and 1 of `(birth, parent, n, lane (position, 0),
//!    0)`, wrapped onto the torus.
//!
//! When either fixed logical quota cannot cover a birth, under
//! `pause-before-commit` (research mode, §7.5) the whole stage is rolled
//! back and reported as a [`CapacityShortfall`]. Every staged genome's
//! charge is returned, and no identity, slot or energy is committed.
//! Retrying the unchanged candidate reproduces the same proposals,
//! randomness and identities.
//!
//! Under `refuse-excess-births-v1`, §7.5's exhibition-mode birth-rejection
//! policy, the same quotas are enforced by refusing births instead:
//! - When the tick's viable attempts outnumber the free population slots,
//!   the slots go to the attempts with the smallest admission keys, word 0
//!   of `(birth, parent, n, lane (attempt, 1), 0)`. The key depends on the
//!   parent's stable identity and the tick only, so the choice is
//!   independent of arrival order, observation and backend, and it does
//!   not favour old (low-identity) parents the way ascending identity
//!   would. Ties, which need equal 64-bit words, fall to the lower
//!   identity.
//! - An admitted birth the logical genome quota cannot cover is refused in
//!   turn, in ascending parent identity, and later births are still tried.
//! - A refused birth ([`RefusedBirth`]) takes no slot, identity or genome
//!   charge. Its parent keeps its energy, and its cooldown is unchanged, as
//!   for any failed attempt, so it may attempt again next tick.
//!
//! The policy is part of the experiment's identity, and every refusal is
//! reported, so the capacity pressure it imposes stays visible (§7.5).
//!
//! Any other failure, such as a physical allocation failure, rolls
//! everything back under either policy. It is returned as an error that
//! pauses, retries or fails the run, and never becomes a birth-selection
//! rule (§7.5).
//!
//! **Commit.** A [`Birth`] carries the child's identity, genome, slot, and
//! energy together (INV-06). [`Births::settle`] checks every parent's budget
//! against its proposal before changing any, then returns the budgets the
//! parents keep and records one `child-endowment` transfer per birth. Each
//! transfer debits `E − parent`, credits the child, and charges the
//! difference, the birth cost, to `reproduction-loss`. [`Births::abandon`]
//! returns every staged charge when the candidate tick is discarded (§8.3).
//!
//! Newborns (`next-tick`) first exist in the committed state of tick
//! `n + 1`, which is their birth tick. They start with zero velocity, zero
//! energy-change history, an invalid held controller output that forces
//! their first evaluation, and their own stable phase. They inherit their
//! parent's lineage at the next generation. A birth sets both the child's
//! and the parent's cooldown deadline to `n + 1 + ⌈cooldown/Δt⌉`.

use flife_core::clock::{Tick, TickSpan, stable_phase};
use flife_core::ids::{Generation, GenomeId, IdSequence, LineageId, OrganismId};
use flife_core::ledger::{EnergyBudget, TickLedger, Transfer, sub_down};
use flife_core::numerics::{normalize_turns, sigmoid, sin_cos_turns};
use flife_core::rng::{
    Address, Block, Lane, LaneKind, SemanticRng, Stream, bernoulli, box_muller, half_open_unit,
};
use flife_core::{CoreError, ValidatedConfig};
use flife_field::{FieldGrid, ProposalConstants};
use flife_genome::{
    BirthAttempt, CandidateRejection, DetachedProposal, Genome, GenomeQuota, LOGICAL_GENOME_BYTES,
    MutationRecord, Mutator, Prepared, Proposal, TopologyCatalog, TraitLimits, founder_v2,
};
use flife_neural::{DecodedActions, MAXIMUM_RECURRENT_STATE_WIDTH};

use crate::population::{DEFAULT_POLARITY, HeldOutput};
use crate::world::{DifferentiationRules, StateInheritanceMode};

/// Fewest admitted inherited births before spawning parallel proposal workers
/// (`flife-s0t`). Below this, proposing on the main thread avoids sync cost.
const MIN_PARALLEL_BIRTHS: usize = 4;

/// Maximum in-flight detached genome scratch per parallel wave (`flife-s0t`,
/// FL-043): keeps parallel birth proposals strictly within staging memory.
const PARALLEL_PROPOSAL_BUDGET_BYTES: u64 = 2 << 20;

/// Most requests proposed in one parallel wave before committing serially.
const MAX_PARALLEL_WAVE: usize = 128;

/// The newborn activation this module implements: newborns first take part
/// on the tick after their birth.
pub const NEXT_TICK_ACTIVATION: &str = "next-tick";

/// The constants of `[reproduction]`, `costs.birth_energy` and the
/// controller interval.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReproductionRules {
    /// `c_b`, dissipated by every birth.
    pub birth_cost: f64,
    /// The minimum post-emission energy for an attempt.
    pub minimum_attempt_energy: f64,
    /// The minimum child endowment, `> 0`.
    pub minimum_child_energy: f64,
    /// The minimum energy the parent keeps, `> 0`.
    pub minimum_parent_energy: f64,
    /// `⌈cooldown_seconds / Δt⌉`.
    pub cooldown: TickSpan,
    /// The child offset disk's radius, in world units.
    pub offset_radius: f64,
    /// The controller evaluation interval, for the newborn's stable phase.
    pub controller_interval: u32,
}

/// The validated birth rules.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reproduction {
    rules: ReproductionRules,
}

impl Reproduction {
    /// Validated rules: a finite nonnegative birth cost, attempt minimum
    /// and offset radius, strictly positive child and parent minimums, and
    /// an interval of at least one tick.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a value out of range.
    pub fn new(rules: ReproductionRules) -> Result<Self, CoreError> {
        for (quantity, value, positive) in [
            ("costs.birth_energy", rules.birth_cost, false),
            (
                "reproduction.minimum_parent_energy_before_attempt",
                rules.minimum_attempt_energy,
                false,
            ),
            (
                "reproduction.minimum_child_energy",
                rules.minimum_child_energy,
                true,
            ),
            (
                "reproduction.minimum_parent_energy_after_split",
                rules.minimum_parent_energy,
                true,
            ),
            (
                "reproduction.child_position_offset_radius",
                rules.offset_radius,
                false,
            ),
        ] {
            let admissible = value.is_finite() && if positive { value > 0.0 } else { value >= 0.0 };
            if !admissible {
                return Err(CoreError::NumericalInvalid {
                    quantity,
                    detail: format!("{value} is out of range"),
                });
            }
        }
        if rules.controller_interval == 0 {
            return Err(CoreError::NumericalInvalid {
                quantity: "controller.evaluation_interval_ticks",
                detail: "0 ticks; the interval must be at least 1".into(),
            });
        }
        Ok(Self { rules })
    }

    /// The rules of a validated configuration.
    ///
    /// # Errors
    /// [`CoreError::UnknownAlgorithm`] for another newborn activation;
    /// otherwise as [`Reproduction::new`].
    pub fn from_config(config: &ValidatedConfig) -> Result<Self, CoreError> {
        let model = config.config();
        let reproduction = &model.reproduction;
        if reproduction.newborn_activation != NEXT_TICK_ACTIVATION {
            return Err(CoreError::UnknownAlgorithm {
                role: "reproduction.newborn_activation",
                name: reproduction.newborn_activation.clone(),
            });
        }
        Self::new(ReproductionRules {
            birth_cost: model.costs.birth_energy,
            minimum_attempt_energy: reproduction.minimum_parent_energy_before_attempt,
            minimum_child_energy: reproduction.minimum_child_energy,
            minimum_parent_energy: reproduction.minimum_parent_energy_after_split,
            cooldown: config.report().cooldown_ticks,
            offset_radius: reproduction.child_position_offset_radius,
            controller_interval: model.controller.evaluation_interval_ticks,
        })
    }

    /// The rules.
    #[must_use]
    pub const fn rules(&self) -> &ReproductionRules {
        &self.rules
    }

    /// The constants a settle backend's proposal takes for these rules in a
    /// world of seed `seed` (FL-032).
    #[must_use]
    pub const fn proposal_constants(&self, seed: u64) -> ProposalConstants {
        ProposalConstants {
            seed,
            minimum_attempt_energy: self.rules.minimum_attempt_energy,
            birth_cost: self.rules.birth_cost,
            minimum_child_energy: self.rules.minimum_child_energy,
            minimum_parent_energy: self.rules.minimum_parent_energy,
        }
    }

    /// Whether `parent` attempts a birth at tick `tick` with probability
    /// `probability`: word 0 of `(birth, parent, tick, lane (attempt, 0),
    /// 0)` against `p`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] unless `0 ≤ p ≤ 1`.
    pub fn attempts(
        &self,
        rng: &SemanticRng,
        parent: OrganismId,
        tick: Tick,
        probability: f64,
    ) -> Result<bool, CoreError> {
        check_probability(probability)?;
        let [word, ..] = birth_block(rng, parent, tick, LaneKind::Attempt)?;
        Ok(bernoulli(word, probability))
    }

    /// The child's offset from its parent, uniform by area in the disk of
    /// radius `R`: `R√u₀` at angle `2πu₁`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] if a draw leaves the unit interval,
    /// which does not happen.
    pub fn child_offset(
        &self,
        rng: &SemanticRng,
        parent: OrganismId,
        tick: Tick,
    ) -> Result<[f64; 2], CoreError> {
        self.child_offset_with_radius(rng, parent, tick, self.rules.offset_radius)
    }

    /// Uniform-by-area disk offset of radius `radius` using the canonical
    /// `(Stream::Birth, parent, tick, LaneKind::Position)` draws.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] if `radius` is negative or nonfinite or
    /// if a draw leaves the unit interval.
    pub fn child_offset_with_radius(
        &self,
        rng: &SemanticRng,
        parent: OrganismId,
        tick: Tick,
        radius: f64,
    ) -> Result<[f64; 2], CoreError> {
        if !radius.is_finite() || radius < 0.0 {
            return Err(CoreError::NumericalInvalid {
                quantity: "child offset radius",
                detail: format!("{radius} is not finite and nonnegative"),
            });
        }
        let [first, second, ..] = birth_block(rng, parent, tick, LaneKind::Position)?;
        let r = radius * half_open_unit(first).sqrt();
        let (sin, cos) =
            sin_cos_turns(half_open_unit(second)).ok_or_else(|| CoreError::NumericalInvalid {
                quantity: "child offset angle",
                detail: format!("word {second:#018x} left the unit interval"),
            })?;
        Ok([r * cos, r * sin])
    }

    /// Compute the `V2-06` (`so2-budding`) child placement offset, composing
    /// cleanly with `V2-08` (`pcp-polarity` lateral mitosis):
    ///
    /// - Basis `e_1 = ∇q / ‖∇q‖₂` if `‖∇q‖₂ > 1e-12`, else `v / ‖v‖₂` if
    ///   `‖v‖₂ > 1e-12`, else deterministic Philox unit angle from word 2 of
    ///   `(Stream::Birth, parent, tick, LaneKind::Position)`; `e_2 = [-e_1.y, e_1.x]`.
    /// - `u_world = u_local.radial * e_1 + u_local.tangential * e_2`.
    /// - `Δx_base = (1 - λ_epi) * (r_base * ξ_disk) + λ_epi * lateral_offset`,
    ///   where `r_base = r_local` when `so2-budding` is active else `R_max`.
    /// - `Δx_child = Δx_base + (1 - λ_epi) * (R_max - r_local) * u_world`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for nonfinite inputs or invalid radii.
    #[expect(
        clippy::too_many_arguments,
        reason = "V2-06 SO(2) budding and V2-08 lateral mitosis composition inputs"
    )]
    pub fn budding_child_offset(
        &self,
        rng: &SemanticRng,
        parent: OrganismId,
        tick: Tick,
        local_offset_radius: Option<f64>,
        steering: Option<flife_neural::BuddingSteering>,
        energy_gradient: [f64; 2],
        velocity: [f64; 2],
        lambda_epi: f64,
        lateral_offset: [f64; 2],
    ) -> Result<[f64; 2], CoreError> {
        if !lambda_epi.is_finite() || !(0.0..=1.0).contains(&lambda_epi) {
            return Err(CoreError::NumericalInvalid {
                quantity: "epithelial lock lambda_epi",
                detail: format!("{lambda_epi} is outside [0, 1]"),
            });
        }
        if !energy_gradient[0].is_finite()
            || !energy_gradient[1].is_finite()
            || !velocity[0].is_finite()
            || !velocity[1].is_finite()
            || !lateral_offset[0].is_finite()
            || !lateral_offset[1].is_finite()
        {
            return Err(CoreError::NumericalInvalid {
                quantity: "budding kinematics",
                detail: "gradient, velocity, or lateral offset is not finite".to_owned(),
            });
        }
        let r_max = self.rules.offset_radius;
        let r_base = match local_offset_radius {
            Some(r_local) => {
                if !r_local.is_finite() || r_local < 0.0 || r_local > r_max {
                    return Err(CoreError::NumericalInvalid {
                        quantity: "v2.budding.local_offset_radius",
                        detail: format!("{r_local} is outside [0, {r_max}]"),
                    });
                }
                r_local
            }
            None => r_max,
        };
        let disk = self.child_offset_with_radius(rng, parent, tick, r_base)?;
        let free = 1.0 - lambda_epi;
        let base = [
            free * disk[0] + lambda_epi * lateral_offset[0],
            free * disk[1] + lambda_epi * lateral_offset[1],
        ];
        let Some(r_local) = local_offset_radius else {
            return Ok(base);
        };
        let steer = steering.unwrap_or(flife_neural::BuddingSteering::ZERO);
        if steer.is_zero() || free == 0.0 || r_max <= r_local {
            return Ok(base);
        }
        const AXIS_EPS: f64 = 1.0e-12;
        let g_norm = energy_gradient[0].hypot(energy_gradient[1]);
        let e1 = if g_norm > AXIS_EPS {
            [energy_gradient[0] / g_norm, energy_gradient[1] / g_norm]
        } else {
            let v_norm = velocity[0].hypot(velocity[1]);
            if v_norm > AXIS_EPS {
                [velocity[0] / v_norm, velocity[1] / v_norm]
            } else {
                let [_, _, third, _] = birth_block(rng, parent, tick, LaneKind::Position)?;
                let (sin, cos) = sin_cos_turns(half_open_unit(third)).ok_or_else(|| {
                    CoreError::NumericalInvalid {
                        quantity: "budding fallback angle",
                        detail: format!("word {third:#018x} left the unit interval"),
                    }
                })?;
                [cos, sin]
            }
        };
        let e2 = [-e1[1], e1[0]];
        let u_world = [
            steer.radial * e1[0] + steer.tangential * e2[0],
            steer.radial * e1[1] + steer.tangential * e2[1],
        ];
        let reach = free * (r_max - r_local);
        Ok([base[0] + reach * u_world[0], base[1] + reach * u_world[1]])
    }

    /// `V2-08` (`pcp-polarity`) lateral mitotic offset: places the daughter at
    /// `1.05 * diameter` along `s * p_i^⊥` (`s ∈ {-1, +1}` from `SemanticRng`)
    /// perturbed by Gaussian angular jitter `σ_θ = π (1 - 0.9 α_pol)` through
    /// host-independent [`sin_cos_turns`] + [`normalize_turns`].
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for invalid inputs or nonfinite RNG
    /// draws; [`CoreError::Overflow`] for lane construction.
    pub fn lateral_mitosis_offset(
        rng: &SemanticRng,
        parent: OrganismId,
        tick: Tick,
        parent_polarity: [f64; 2],
        diameter: f64,
        polarity_strength: f64,
    ) -> Result<[f64; 2], CoreError> {
        if !(diameter.is_finite() && diameter > 0.0) {
            return Err(CoreError::NumericalInvalid {
                quantity: "contact.diameter",
                detail: format!("{diameter} must be finite and positive"),
            });
        }
        if !(polarity_strength.is_finite() && (0.0..=1.0).contains(&polarity_strength)) {
            return Err(CoreError::NumericalInvalid {
                quantity: "polarity_strength",
                detail: format!("{polarity_strength} is outside [0, 1]"),
            });
        }
        let norm = parent_polarity[0].hypot(parent_polarity[1]);
        if !norm.is_finite() || norm <= 0.0 {
            return Err(CoreError::NumericalInvalid {
                quantity: "parent polarity",
                detail: format!("{parent_polarity:?} has zero norm"),
            });
        }
        let px = parent_polarity[0] / norm;
        let py = parent_polarity[1] / norm;
        let [w_side, w_u1, w_u2, _] = rng.block(Address {
            stream: Stream::Birth,
            entity: parent.get(),
            ordinal: tick.get(),
            lane: Lane::new(0, LaneKind::Position, 2)?,
            draw: 0,
        });
        let side = if w_side & 1 == 0 { 1.0 } else { -1.0 };
        let base_x = -py * side;
        let base_y = px * side;
        let [z0, _] = box_muller(w_u1, w_u2)?;
        let sigma_theta = core::f64::consts::PI * (1.0 - 0.9 * polarity_strength);
        let dtheta = (sigma_theta * z0).clamp(-core::f64::consts::PI, core::f64::consts::PI);
        let turns = normalize_turns(dtheta / core::f64::consts::TAU).ok_or_else(|| {
            CoreError::NumericalInvalid {
                quantity: "lateral mitosis jitter",
                detail: format!("dtheta {dtheta:?} failed turn normalization"),
            }
        })?;
        let (sin_d, cos_d) = sin_cos_turns(turns).ok_or_else(|| CoreError::NumericalInvalid {
            quantity: "lateral mitosis jitter",
            detail: format!("turns {turns:?} left [0, 1)"),
        })?;
        let rx = cos_d * base_x - sin_d * base_y;
        let ry = sin_d * base_x + cos_d * base_y;
        let dist = 1.05 * diameter;
        Ok([dist * rx, dist * ry])
    }

    /// `V2-08` (`pcp-polarity`) newborn polarity inheritance: starts from
    /// `parent_polarity` rotated by small semantic-Philox angular jitter
    /// (`σ = 0.05 rad`, clamped to `±0.25 rad`) and renormalized to unit
    /// length (`INV-06`, `INV-11`).
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for an invalid parent polarity or draw;
    /// [`CoreError::Overflow`] for lane construction.
    pub fn inherit_polarity(
        rng: &SemanticRng,
        parent: OrganismId,
        tick: Tick,
        parent_polarity: [f64; 2],
    ) -> Result<[f64; 2], CoreError> {
        let [w1, w2, ..] = rng.block(Address {
            stream: Stream::Birth,
            entity: parent.get(),
            ordinal: tick.get(),
            lane: Lane::new(0, LaneKind::Direction, 0x0802)?,
            draw: 0,
        });
        let [z0, _] = box_muller(w1, w2)?;
        let jitter = (0.05 * z0).clamp(-0.25, 0.25);
        crate::contact::rotate_unit_polarity(parent_polarity, jitter)
    }

    /// `V2-12` (`epigenetic-recurrence`) newborn epigenetic recurrent state
    /// inheritance from a parent's post-Phase-4 state `parent_state[0..S]`:
    ///
    /// - [`StateInheritanceMode::Zero`]: returns `Ok(None)` (`HeldOutput::INVALID`).
    /// - [`StateInheritanceMode::SymmetricEpigenetic`]:
    ///   `h_child[k] = clamp(α_epi * h_p[k] + σ_epi * ξ_k, -1, 1)` for `k ∈ 0..S`.
    /// - [`StateInheritanceMode::AsymmetricStem`]:
    ///   `c_stem = sigmoid(3.0 * h_p[0])`,
    ///   `h_child[0] = clamp(α_epi * h_p[0] + c_stem * β_asym, -1, 1)`, and
    ///   symmetric inheritance on `k ∈ 1..S`.
    ///
    /// Every jitter draw `ξ_k` is addressed semantically by
    /// `(Stream::Birth, parent, tick, Lane(0, Direction, 0x0C01), draw = k)`.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] if `parent_state.len() != rules.state_width`
    /// or `rules.state_width` lies outside `1..=MAXIMUM_RECURRENT_STATE_WIDTH`;
    /// [`CoreError::NumericalInvalid`] for nonfinite or out-of-range state/rules;
    /// [`CoreError::Overflow`] for lane construction.
    pub fn inherit_epigenetic_state(
        rng: &SemanticRng,
        parent: OrganismId,
        tick: Tick,
        parent_state: &[f32],
        rules: DifferentiationRules,
    ) -> Result<Option<InheritedState>, CoreError> {
        if rules.state_inheritance == StateInheritanceMode::Zero {
            return Ok(None);
        }
        let width = rules.state_width;
        if width == 0 || width > MAXIMUM_RECURRENT_STATE_WIDTH || parent_state.len() != width {
            return Err(CoreError::LayoutMismatch {
                buffer: "parent recurrent state",
                detail: format!(
                    "expected parent_state len {width} (1..={MAXIMUM_RECURRENT_STATE_WIDTH}), got {}",
                    parent_state.len()
                ),
            });
        }
        for (index, &value) in parent_state.iter().enumerate() {
            if !value.is_finite() || !(-1.0_f32..=1.0_f32).contains(&value) {
                return Err(CoreError::NumericalInvalid {
                    quantity: "parent recurrent state",
                    detail: format!("parent_state[{index}] = {value} lies outside [-1, 1]"),
                });
            }
        }
        let alpha = rules.inheritance_retention;
        let sigma = rules.inheritance_jitter;
        let lane = Lane::new(0, LaneKind::Direction, 0x0C01)?;
        let mut child = [0.0_f32; MAXIMUM_RECURRENT_STATE_WIDTH];
        let start_sym = match rules.state_inheritance {
            StateInheritanceMode::Zero => return Ok(None),
            StateInheritanceMode::SymmetricEpigenetic => 0,
            StateInheritanceMode::AsymmetricStem => {
                let hp0 = f64::from(parent_state[0]);
                let c_stem = sigmoid(3.0 * hp0).ok_or_else(|| CoreError::NumericalInvalid {
                    quantity: "asymmetric stem sigmoid",
                    detail: format!("sigmoid(3.0 * {hp0}) overflowed"),
                })?;
                let raw = alpha * hp0 + c_stem * rules.stem_asymmetry;
                if !raw.is_finite() {
                    return Err(CoreError::NumericalInvalid {
                        quantity: "inherited epigenetic state",
                        detail: format!("asymmetric h_child[0] = {raw} is not finite"),
                    });
                }
                let val = (raw.clamp(-1.0, 1.0) as f32).clamp(-1.0_f32, 1.0_f32);
                child[0] = if val == 0.0 { 0.0 } else { val };
                1
            }
        };
        for k in start_sym..width {
            let jitter = if sigma > 0.0 {
                let [w1, w2, ..] = rng.block(Address {
                    stream: Stream::Birth,
                    entity: parent.get(),
                    ordinal: tick.get(),
                    lane,
                    draw: u64::try_from(k).unwrap_or(u64::MAX),
                });
                let [z0, _] = box_muller(w1, w2)?;
                sigma * z0
            } else {
                0.0
            };
            let raw = alpha * f64::from(parent_state[k]) + jitter;
            if !raw.is_finite() {
                return Err(CoreError::NumericalInvalid {
                    quantity: "inherited epigenetic state",
                    detail: format!("h_child[{k}] = {raw} is not finite"),
                });
            }
            let val = (raw.clamp(-1.0, 1.0) as f32).clamp(-1.0_f32, 1.0_f32);
            child[k] = if val == 0.0 { 0.0 } else { val };
        }
        InheritedState::from_slice(&child[..width]).map(Some)
    }

    /// The split of `available`, the post-emission budget, at child share
    /// `β`, or why no viable birth fits it.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] unless `available` is finite and
    /// nonnegative and `0 < β < 1`.
    pub fn split(&self, available: f64, child_fraction: f64) -> Result<Viability, CoreError> {
        if !(available.is_finite() && available >= 0.0) {
            return Err(CoreError::NumericalInvalid {
                quantity: "parent energy",
                detail: format!("{available} is not a finite nonnegative energy"),
            });
        }
        if !(child_fraction > 0.0 && child_fraction < 1.0) {
            return Err(CoreError::NumericalInvalid {
                quantity: "child fraction",
                detail: format!("{child_fraction} is outside (0, 1)"),
            });
        }
        let rules = &self.rules;
        if available < rules.minimum_attempt_energy {
            return Ok(Viability::Unviable(Unviable::BelowAttemptEnergy {
                available,
                minimum: rules.minimum_attempt_energy,
            }));
        }
        let remainder = sub_down(available, rules.birth_cost);
        if remainder <= 0.0 {
            return Ok(Viability::Unviable(Unviable::BirthCostUnaffordable {
                available,
                cost: rules.birth_cost,
            }));
        }
        // `β < 1`, so `β R ≤ R` after rounding and the parent's share is
        // nonnegative.
        let child = child_fraction * remainder;
        let parent = sub_down(remainder, child);
        if child < rules.minimum_child_energy {
            return Ok(Viability::Unviable(Unviable::ChildBelowMinimum {
                child,
                minimum: rules.minimum_child_energy,
            }));
        }
        if parent < rules.minimum_parent_energy {
            return Ok(Viability::Unviable(Unviable::ParentBelowMinimum {
                parent,
                minimum: rules.minimum_parent_energy,
            }));
        }
        Ok(Viability::Viable(Split {
            available: available + 0.0,
            child,
            parent,
        }))
    }

    /// Phase 7's reproduction proposal for `parent`, read from its
    /// post-emission `budget` and this tick's decoded `actions`, at attempt
    /// tick `tick`. Nothing changes.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for an invalid decoded probability or
    /// share, and [`CoreError::Overflow`] for a child generation past
    /// `u32::MAX`.
    pub fn propose(
        &self,
        rng: &SemanticRng,
        parent: &ParentState,
        tick: Tick,
        actions: &DecodedActions,
        budget: &EnergyBudget,
    ) -> Result<ReproductionProposal, CoreError> {
        self.propose_with_probability(
            rng,
            parent,
            tick,
            actions.attempt_probability,
            actions.child_fraction,
            budget,
        )
    }

    /// Phase 7's reproduction proposal for `parent` using an explicit
    /// `attempt_probability` and `child_fraction` (e.g., Hippo-YAP crowding
    /// gated `p_eff` under `V2-07`). Nothing changes.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for an invalid probability or share,
    /// and [`CoreError::Overflow`] for a child generation past `u32::MAX`.
    pub fn propose_with_probability(
        &self,
        rng: &SemanticRng,
        parent: &ParentState,
        tick: Tick,
        attempt_probability: f64,
        child_fraction: f64,
        budget: &EnergyBudget,
    ) -> Result<ReproductionProposal, CoreError> {
        if budget.is_exhausted() {
            return Ok(ReproductionProposal::Exhausted);
        }
        if tick < parent.cooldown_deadline {
            return Ok(ReproductionProposal::CoolingDown {
                deadline: parent.cooldown_deadline,
            });
        }
        if !self.attempts(rng, parent.id, tick, attempt_probability)? {
            return Ok(ReproductionProposal::NoAttempt);
        }
        self.attempted(parent, tick, child_fraction, budget)
    }

    /// The proposal of an attempt that the draw made: the split of the
    /// budget at child share `child_fraction`, and the intent a viable one
    /// gives.
    ///
    /// # Errors
    /// As [`Self::split`]; [`CoreError::Overflow`] for a child generation
    /// past `u32::MAX`.
    pub(crate) fn attempted(
        &self,
        parent: &ParentState,
        tick: Tick,
        child_fraction: f64,
        budget: &EnergyBudget,
    ) -> Result<ReproductionProposal, CoreError> {
        match self.split(budget.remaining(), child_fraction)? {
            Viability::Unviable(reason) => Ok(ReproductionProposal::Unviable(reason)),
            Viability::Viable(split) => Ok(ReproductionProposal::Intent(BirthIntent {
                parent: parent.id,
                lineage: parent.lineage,
                generation: parent.generation.child()?,
                tick,
                position: parent.position,
                split,
                budding_offset: None,
                newborn_polarity: None,
                newborn_state: None,
            })),
        }
    }
}

/// [`Reproduction::attempts`]' check of an attempt probability.
///
/// # Errors
/// [`CoreError::NumericalInvalid`] unless `0 ≤ p ≤ 1`.
pub(crate) fn check_probability(probability: f64) -> Result<(), CoreError> {
    if (0.0..=1.0).contains(&probability) {
        Ok(())
    } else {
        Err(CoreError::NumericalInvalid {
            quantity: "reproduction attempt probability",
            detail: format!("{probability} is outside [0, 1]"),
        })
    }
}

/// Smooth Hippo-YAP contact-inhibition-of-proliferation gate (`V2-07`):
/// `g_hippo = clamp(1 - local_neighbors / arrest_threshold, 0, 1)`.
#[must_use]
pub fn hippo_gate(local_neighbors: u32, arrest_threshold: f64) -> f64 {
    if !arrest_threshold.is_finite() || arrest_threshold <= 0.0 {
        return 0.0;
    }
    (1.0 - f64::from(local_neighbors) / arrest_threshold).clamp(0.0, 1.0)
}

/// Effective reproduction attempt probability under Hippo-YAP local crowding
/// arrest (`V2-07`): `p_eff = 1 - exp(-(attempt_rate * g_hippo) * Δt)`, exact
/// `0.0` when `g_hippo == 0.0`.
///
/// # Errors
/// [`CoreError::NumericalInvalid`] if `attempt_rate` is negative or nonfinite,
/// `arrest_threshold` is nonfinite or `< 2.0`, or `expm1_neg` rejects the
/// scaled product.
pub fn hippo_attempt_probability(
    attempt_rate: f64,
    step: flife_core::TimeStep,
    local_neighbors: u32,
    arrest_threshold: f64,
) -> Result<f64, CoreError> {
    if !attempt_rate.is_finite() || attempt_rate < 0.0 {
        return Err(CoreError::NumericalInvalid {
            quantity: "reproduction attempt rate",
            detail: format!("{attempt_rate} is not finite and nonnegative"),
        });
    }
    if !arrest_threshold.is_finite() || arrest_threshold < 2.0 {
        return Err(CoreError::NumericalInvalid {
            quantity: "v2.crowding.arrest_threshold",
            detail: format!("{arrest_threshold} must be finite and >= 2.0"),
        });
    }
    let gate = hippo_gate(local_neighbors, arrest_threshold);
    if gate == 0.0 || attempt_rate == 0.0 {
        return Ok(0.0);
    }
    flife_core::numerics::expm1_neg(attempt_rate * gate * step.seconds_per_tick()).ok_or_else(
        || CoreError::NumericalInvalid {
            quantity: "hippo attempt probability",
            detail: format!(
                "attempt_rate={attempt_rate}, gate={gate}, dt={} out of range",
                step.seconds_per_tick()
            ),
        },
    )
}

/// Optional Hippo-YAP crowding sensor value `c_den = N_loc / (N_loc + 4)`
/// (`V2-07`), bounded in `[0, 1)`.
#[must_use]
pub fn hippo_crowding_sensor(local_neighbors: u32) -> f64 {
    let n = f64::from(local_neighbors);
    n / (n + 4.0)
}

fn birth_block(
    rng: &SemanticRng,
    parent: OrganismId,
    tick: Tick,
    kind: LaneKind,
) -> Result<Block, CoreError> {
    Ok(rng.block(Address {
        stream: Stream::Birth,
        entity: parent.get(),
        ordinal: tick.get(),
        lane: Lane::new(0, kind, 0)?,
        draw: 0,
    }))
}

/// What phase 7 reads of a parent besides its budget.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParentState {
    /// The parent's stable identity.
    pub id: OrganismId,
    /// Its founder lineage.
    pub lineage: LineageId,
    /// Its generation.
    pub generation: Generation,
    /// Its post-motion position.
    pub position: [f64; 2],
    /// The first attempt tick at which it is eligible.
    pub cooldown_deadline: Tick,
}

/// An energy split that leaves a viable child and parent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Split {
    /// `E`, the parent's post-emission budget.
    pub available: f64,
    /// The child's endowment, `β (E − c_b)`.
    pub child: f64,
    /// What the parent keeps, `(1 − β)(E − c_b)`.
    pub parent: f64,
}

impl Split {
    /// What the parent pays, `E − parent`, rounded to nearest: the child's
    /// endowment plus the birth cost.
    #[must_use]
    pub fn debited(&self) -> f64 {
        self.available - self.parent
    }

    /// What the birth dissipates, `E − parent − child`: the birth cost, plus
    /// at most a few units in the last place of the split's rounding.
    #[must_use]
    pub fn dissipated(&self) -> f64 {
        self.debited() - self.child
    }
}

/// Whether a split is viable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Viability {
    /// Both shares meet their minimums.
    Viable(Split),
    /// No viable birth fits the budget.
    Unviable(Unviable),
}

/// Why an attempted birth is not viable (§7.4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Unviable {
    /// The post-emission budget is below the attempt minimum.
    BelowAttemptEnergy {
        /// The budget.
        available: f64,
        /// The minimum.
        minimum: f64,
    },
    /// Nothing is left after the birth cost.
    BirthCostUnaffordable {
        /// The budget.
        available: f64,
        /// The birth cost.
        cost: f64,
    },
    /// The child's endowment is below its minimum.
    ChildBelowMinimum {
        /// The endowment.
        child: f64,
        /// The minimum.
        minimum: f64,
    },
    /// The parent's share is below its minimum.
    ParentBelowMinimum {
        /// The share.
        parent: f64,
        /// The minimum.
        minimum: f64,
    },
}

/// Phase 7's reproduction proposal for one organism.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ReproductionProposal {
    /// The organism is dead.
    Exhausted,
    /// The attempt tick is before its cooldown deadline.
    CoolingDown {
        /// The deadline.
        deadline: Tick,
    },
    /// Eligible, but this tick's draw made no attempt.
    NoAttempt,
    /// Attempted, but no viable birth fits the post-emission budget.
    Unviable(Unviable),
    /// A viable attempt for phase 8.
    Intent(BirthIntent),
}

/// Compact bounded newborn epigenetic recurrent state `h_child ∈ [-1, 1]^S`
/// carried from Phase 7 [`BirthIntent`] to Phase 8 [`Newborn`] (`V2-12`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InheritedState {
    len: u8,
    values: [f32; MAXIMUM_RECURRENT_STATE_WIDTH],
}

impl InheritedState {
    /// Construct a validated inherited epigenetic state from `slice`.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] if `slice.len() > MAXIMUM_RECURRENT_STATE_WIDTH`;
    /// [`CoreError::NumericalInvalid`] if any entry is nonfinite or outside `[-1, 1]`.
    pub fn from_slice(slice: &[f32]) -> Result<Self, CoreError> {
        if slice.len() > MAXIMUM_RECURRENT_STATE_WIDTH {
            return Err(CoreError::LayoutMismatch {
                buffer: "inherited epigenetic state",
                detail: format!(
                    "expected at most {MAXIMUM_RECURRENT_STATE_WIDTH} entries, got {}",
                    slice.len()
                ),
            });
        }
        let mut values = [0.0_f32; MAXIMUM_RECURRENT_STATE_WIDTH];
        for (index, (dst, &value)) in values.iter_mut().zip(slice).enumerate() {
            if !value.is_finite() || !(-1.0_f32..=1.0_f32).contains(&value) {
                return Err(CoreError::NumericalInvalid {
                    quantity: "inherited epigenetic state",
                    detail: format!("state[{index}] = {value} lies outside [-1, 1]"),
                });
            }
            *dst = value;
        }
        let len = u8::try_from(slice.len()).map_err(|_| CoreError::Overflow {
            quantity: "inherited epigenetic state width",
        })?;
        Ok(Self { len, values })
    }

    /// The inherited state lanes `0..S`.
    #[must_use]
    pub fn as_slice(&self) -> &[f32] {
        &self.values[..usize::from(self.len)]
    }

    /// Convert into a newborn [`HeldOutput`] with `evaluated_at == None`
    /// (preserving Appendix C.13 forced first evaluation).
    ///
    /// # Errors
    /// As [`HeldOutput::newborn_with_state`].
    pub fn into_held_output(self) -> Result<HeldOutput, CoreError> {
        HeldOutput::newborn_with_state(self.as_slice())
    }
}

/// A viable attempt: what phase 8 needs to construct the birth.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BirthIntent {
    /// The parent.
    pub parent: OrganismId,
    /// The parent's lineage, which the child inherits.
    pub lineage: LineageId,
    /// The child's generation.
    pub generation: Generation,
    /// The attempt tick `n`, the advance's committed input tick.
    pub tick: Tick,
    /// The parent's post-motion position.
    pub position: [f64; 2],
    /// The energy split.
    pub split: Split,
    /// Optional pre-resolved child offset for `V2-06` (`so2-budding`) / `V2-08`
    /// (`pcp-polarity` lateral mitosis). `None` uses baseline uniform disk
    /// placement [`Reproduction::child_offset`] bit-identically.
    pub budding_offset: Option<[f64; 2]>,
    /// Optional inherited newborn apical-basal polarity `p_child ∈ S^1` for
    /// `V2-08` (`pcp-polarity`). `None` uses [`DEFAULT_POLARITY`] when
    /// `pcp-polarity` is disabled.
    pub newborn_polarity: Option<[f64; 2]>,
    /// Optional inherited newborn epigenetic recurrent state `h_child ∈ [-1, 1]^S`
    /// for `V2-12` (`epigenetic-recurrence`). `None` leaves newborn `HeldOutput::INVALID`.
    pub newborn_state: Option<InheritedState>,
}

/// One birth for phase 8: an intent and its parent's genome.
#[derive(Debug, Clone, Copy)]
pub struct BirthRequest<'g> {
    /// The viable attempt.
    pub intent: BirthIntent,
    /// The parent's genome.
    pub genome: &'g Genome,
}

/// The fixed logical live-population quota for one tick's births (§7.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PopulationQuota {
    maximum: u64,
    live: u64,
}

impl PopulationQuota {
    /// `live` organisms will be live at publication before this tick's
    /// births, of at most `maximum`.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] when `live > maximum`.
    pub fn new(maximum: u64, live: u64) -> Result<Self, CoreError> {
        if live > maximum {
            return Err(CoreError::StateConflict {
                operation: "population quota",
                detail: format!("{live} live organisms exceed the quota {maximum}"),
            });
        }
        Ok(Self { maximum, live })
    }

    /// `population.maximum_count`, with `live` organisms live.
    ///
    /// # Errors
    /// As [`PopulationQuota::new`].
    pub fn from_config(config: &ValidatedConfig, live: u64) -> Result<Self, CoreError> {
        Self::new(u64::from(config.config().population.maximum_count), live)
    }

    /// Slots this tick's births may fill.
    #[must_use]
    pub const fn available(&self) -> u64 {
        self.maximum - self.live
    }
}

/// What phase 8 constructs from, besides the requests.
#[derive(Debug, Clone, Copy)]
pub struct BirthContext<'a> {
    /// The world's semantic randomness.
    pub rng: &'a SemanticRng,
    /// The torus children are placed on.
    pub grid: &'a FieldGrid,
    /// The birth rules.
    pub reproduction: &'a Reproduction,
    /// The mutation operator.
    pub mutator: &'a Mutator<'a>,
    /// Where a newborn's genome comes from.
    pub offspring: Offspring<'a>,
    /// What a birth the fixed logical quotas cannot cover does.
    pub capacity: CapacityPolicy,
}

/// `population.capacity_policy`: what a birth the fixed logical quotas
/// cannot cover does (§7.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapacityPolicy {
    /// `pause-before-commit`, research mode: the tick pauses before commit
    /// with nothing staged.
    PauseBeforeCommit,
    /// `refuse-excess-births-v1`, exhibition mode: births are admitted in
    /// admission-key order until a quota is full, and the rest are refused
    /// without charge (see the [module documentation](self)).
    RefuseExcessBirths,
}

/// The research-mode capacity policy.
pub const PAUSE_BEFORE_COMMIT: &str = "pause-before-commit";

/// The exhibition-mode capacity policy.
pub const REFUSE_EXCESS_BIRTHS: &str = "refuse-excess-births-v1";

impl CapacityPolicy {
    /// The policy named `name`.
    ///
    /// # Errors
    /// [`CoreError::UnknownAlgorithm`] for any other name.
    pub fn from_name(name: &str) -> Result<Self, CoreError> {
        match name {
            PAUSE_BEFORE_COMMIT => Ok(Self::PauseBeforeCommit),
            REFUSE_EXCESS_BIRTHS => Ok(Self::RefuseExcessBirths),
            _ => Err(CoreError::UnknownAlgorithm {
                role: "population.capacity_policy",
                name: name.to_owned(),
            }),
        }
    }

    /// The registered name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::PauseBeforeCommit => PAUSE_BEFORE_COMMIT,
            Self::RefuseExcessBirths => REFUSE_EXCESS_BIRTHS,
        }
    }
}

/// Where a newborn's genome comes from.
#[derive(Debug, Clone, Copy)]
pub enum Offspring<'a> {
    /// Its parent's genome through [`Mutator::propose`]
    /// (`field-ecology-v1`).
    Inherited,
    /// A founder's genome drawn at the newborn's own organism identity
    /// (`field-ecology-v1-fresh-offspring`, FL-041): the default topology,
    /// founder weights and zero biases, founder traits. Nothing of the
    /// parent's genome passes to it, and no mutation is proposed.
    Fresh {
        /// The topology catalog.
        catalog: &'a TopologyCatalog,
        /// The trait limits.
        limits: &'a TraitLimits,
        /// The configured initial sensing scale.
        initial_scale: f64,
    },
}

/// The run's identity sequences: the next organism and genome identities
/// to issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identities {
    /// Organism identities.
    pub organisms: IdSequence<OrganismId>,
    /// Genome identities.
    pub genomes: IdSequence<GenomeId>,
}

/// A fixed logical quota a birth needs (§7.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capacity {
    /// `population.maximum_count`.
    PopulationSlots,
    /// `execution.maximum_logical_genome_bytes`.
    LogicalGenomeBytes,
}

/// A birth the fixed logical quotas cannot cover. Under
/// `pause-before-commit` the tick pauses before commit with nothing staged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapacityShortfall {
    /// The quota.
    pub resource: Capacity,
    /// The parent of the first birth it could not cover.
    pub parent: OrganismId,
    /// What that birth needed: one slot, or its genome's logical bytes.
    pub required: u64,
    /// What was left.
    pub available: u64,
    /// Every proposed birth's parent, in the stable order they were staged.
    pub proposed: Vec<OrganismId>,
    /// Births covered before the refused one.
    pub covered: usize,
}

/// A viable attempt `refuse-excess-births-v1` refused because a fixed
/// logical quota could not cover it: no child, no charge, and the parent's
/// energy and cooldown unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefusedBirth {
    /// The parent.
    pub parent: OrganismId,
    /// The quota that could not cover it.
    pub resource: Capacity,
}

/// A birth whose every candidate was rejected: no child, no charge, and the
/// parent's energy and cooldown unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedBirth {
    /// The parent.
    pub parent: OrganismId,
    /// Every candidate's rejection, in order.
    pub candidates: Vec<CandidateRejection>,
}

/// A newborn's initial state (§6.6, §7.4, A.3 item 10).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Newborn {
    /// Its stable identity.
    pub id: OrganismId,
    /// Its genome's identity.
    pub genome: GenomeId,
    /// Its parent.
    pub parent: OrganismId,
    /// Its founder lineage, inherited.
    pub lineage: LineageId,
    /// Its generation, one past its parent's.
    pub generation: Generation,
    /// `n + 1`: the first committed tick it exists in.
    pub birth_tick: Tick,
    /// Its wrapped position.
    pub position: [f64; 2],
    /// Its offset from the parent's post-motion position.
    pub offset: [f64; 2],
    /// Zero.
    pub velocity: [f64; 2],
    /// Its endowment.
    pub energy: f64,
    /// Zero: newborns start with no energy-change history.
    pub energy_change_rate: f64,
    /// False: its first advance forces a controller evaluation.
    pub held_output_valid: bool,
    /// Its initial held controller state (`evaluated_at == None`, carrying
    /// inherited epigenetic state under `V2-12` when configured; `HeldOutput::INVALID`
    /// otherwise).
    pub held: HeldOutput,
    /// Its stable controller phase, identity modulo the interval.
    pub phase: u32,
    /// `n + 1 + ⌈cooldown/Δt⌉`.
    pub cooldown_deadline: Tick,
    /// Its initial unit apical-basal polarity (`V2-08`, [`DEFAULT_POLARITY`]
    /// when disabled).
    pub polarity: [f64; 2],
}

/// One staged birth: identity, genome, slot and energy together (INV-06).
#[derive(Debug)]
pub struct Birth {
    /// The parent.
    pub parent: OrganismId,
    /// The energy split.
    pub split: Split,
    /// The parent's new cooldown deadline, the same as the child's.
    pub parent_cooldown_deadline: Tick,
    /// The child.
    pub newborn: Newborn,
    /// The child's sealed genome, charged to the genome quota.
    pub genome: Box<Genome>,
    /// How the genome was made.
    pub record: MutationRecord,
}

impl Birth {
    /// What the parent keeps: `budget` with exactly the split's parent
    /// share.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] unless `budget` still holds exactly the
    /// energy the split was proposed against.
    pub fn settle_parent(&self, budget: &EnergyBudget) -> Result<EnergyBudget, CoreError> {
        if budget.is_exhausted() || budget.remaining().to_bits() != self.split.available.to_bits() {
            return Err(CoreError::StateConflict {
                operation: "settle birth",
                detail: format!(
                    "{} holds {}, but its birth was proposed against {}",
                    self.parent,
                    budget.remaining(),
                    self.split.available
                ),
            });
        }
        let mut kept = *budget;
        kept.retain(self.split.parent)?;
        Ok(kept)
    }
}

/// A tick's staged births.
#[derive(Debug)]
pub struct Births {
    born: Vec<Birth>,
    rejected: Vec<RejectedBirth>,
    refused: Vec<RefusedBirth>,
}

impl Births {
    /// The staged births, in ascending parent identity, which is also
    /// ascending child identity.
    #[must_use]
    pub fn born(&self) -> &[Birth] {
        &self.born
    }

    /// Births whose every candidate was rejected.
    #[must_use]
    pub fn rejected(&self) -> &[RejectedBirth] {
        &self.rejected
    }

    /// Births `refuse-excess-births-v1` refused, in ascending parent
    /// identity.
    #[must_use]
    pub fn refused(&self) -> &[RefusedBirth] {
        &self.refused
    }

    /// Settle every parent and record the endowments, all or nothing.
    /// `budget_of` gives each parent's budget at the reproduction stage.
    /// Returns, in birth order, the budget each parent keeps, for the
    /// caller to commit with the newborns.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] when a parent is missing or its budget
    /// no longer matches its proposal. Nothing is recorded on error.
    pub fn settle(
        &self,
        ledger: &mut TickLedger,
        budget_of: impl Fn(OrganismId) -> Option<EnergyBudget>,
    ) -> Result<Vec<(OrganismId, EnergyBudget)>, CoreError> {
        let mut kept = Vec::with_capacity(self.born.len());
        for birth in &self.born {
            let budget = budget_of(birth.parent).ok_or_else(|| CoreError::StateConflict {
                operation: "settle birth",
                detail: format!("{} has no budget", birth.parent),
            })?;
            kept.push((birth.parent, birth.settle_parent(&budget)?));
        }
        for birth in &self.born {
            ledger.transfer(
                Transfer::ChildEndowment,
                birth.split.debited(),
                birth.split.child,
            )?;
        }
        Ok(kept)
    }

    /// Discard the staged births, returning every genome's charge, as when
    /// the candidate tick is cancelled before publication (§8.3).
    ///
    /// # Errors
    /// As [`GenomeQuota::retire`].
    pub fn abandon(self, quota: &mut GenomeQuota) -> Result<(), CoreError> {
        roll_back(self.born, quota)
    }

    /// The births, the rejected births and the refused births.
    #[must_use]
    pub fn into_parts(self) -> (Vec<Birth>, Vec<RejectedBirth>, Vec<RefusedBirth>) {
        (self.born, self.rejected, self.refused)
    }
}

/// The outcome of phase 8's birth stage.
#[derive(Debug)]
pub enum BirthResolution {
    /// Every viable attempt was staged, rejected or refused; the
    /// identities advanced.
    Staged(Births),
    /// Under `pause-before-commit`, a fixed logical quota could not cover a
    /// birth. Nothing is staged or charged and the identities are
    /// unchanged.
    Paused(CapacityShortfall),
}

/// Stage `requests`, the tick's viable attempts, as one transaction (see the
/// [module documentation](self)).
///
/// # Errors
/// [`CoreError::StateConflict`] for two requests from one parent or for
/// requests of different ticks, [`CoreError::Overflow`] for an exhausted
/// identity sequence or tick, and any error of mutation other than a
/// logical genome quota refusal, such as a physical allocation failure.
/// Everything is rolled back first: no genome stays charged and the
/// identities are unchanged.
pub fn resolve_births(
    context: &BirthContext<'_>,
    requests: &[BirthRequest<'_>],
    population: PopulationQuota,
    identities: &mut Identities,
    quota: &mut GenomeQuota,
) -> Result<BirthResolution, CoreError> {
    resolve_births_with(context, &[], requests, population, identities, quota)
}

/// [`resolve_births`], where an inherited birth whose parent has an entry
/// in `first` (sorted by parent) proposes its first candidate from it,
/// through `Mutator::propose_prepared`: its member and its parameters
/// already mutated (FL-032).
///
/// # Errors
/// As [`resolve_births`], and as `Mutator::propose_prepared` for a
/// prepared candidate that is not its birth's.
pub fn resolve_births_with(
    context: &BirthContext<'_>,
    first: &[(OrganismId, Prepared<'_>)],
    requests: &[BirthRequest<'_>],
    population: PopulationQuota,
    identities: &mut Identities,
    quota: &mut GenomeQuota,
) -> Result<BirthResolution, CoreError> {
    resolve_births_with_workers(context, first, requests, population, identities, quota, 1)
}

/// Bounded-wave preparer for first-candidate parameter mutations on a settle
/// backend (FL-032, FL-043): mutates at most one wave within
/// [`PARALLEL_PROPOSAL_BUDGET_BYTES`] at a time so host staging stays bounded.
pub(crate) trait FirstCandidateWave {
    fn active(&self) -> bool;
    fn prepare_wave(&mut self, wave: &[&BirthRequest<'_>]);
    fn prepared(&self) -> Vec<(OrganismId, Prepared<'_>)>;
}

struct NoWavePreparer;

impl FirstCandidateWave for NoWavePreparer {
    fn active(&self) -> bool {
        false
    }

    fn prepare_wave(&mut self, _wave: &[&BirthRequest<'_>]) {}

    fn prepared(&self) -> Vec<(OrganismId, Prepared<'_>)> {
        Vec::new()
    }
}

/// [`resolve_births_with`], proposing admitted inherited newborns' genomes in
/// bounded parallel waves across up to `workers` threads before committing
/// them serially in ascending parent identity (`flife-s0t`). Results are bit
/// for bit identical at any worker count.
///
/// # Errors
/// As [`resolve_births_with`], or [`CoreError::StateConflict`] if a worker
/// panicked.
pub fn resolve_births_with_workers(
    context: &BirthContext<'_>,
    first: &[(OrganismId, Prepared<'_>)],
    requests: &[BirthRequest<'_>],
    population: PopulationQuota,
    identities: &mut Identities,
    quota: &mut GenomeQuota,
    workers: usize,
) -> Result<BirthResolution, CoreError> {
    let mut none = NoWavePreparer;
    resolve_births_inner(
        context, first, &mut none, requests, population, identities, quota, workers,
    )
}

/// [`resolve_births_with_workers`] with an optional bounded-wave settle
/// preparer (FL-032, FL-043).
pub(crate) fn resolve_births_with_wave_preparer(
    context: &BirthContext<'_>,
    preparer: &mut impl FirstCandidateWave,
    requests: &[BirthRequest<'_>],
    population: PopulationQuota,
    identities: &mut Identities,
    quota: &mut GenomeQuota,
    workers: usize,
) -> Result<BirthResolution, CoreError> {
    resolve_births_inner(
        context,
        &[],
        preparer,
        requests,
        population,
        identities,
        quota,
        workers,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "private shared driver for resolve_births_with_workers and wave-bounded settle"
)]
fn resolve_births_inner(
    context: &BirthContext<'_>,
    first: &[(OrganismId, Prepared<'_>)],
    preparer: &mut impl FirstCandidateWave,
    requests: &[BirthRequest<'_>],
    population: PopulationQuota,
    identities: &mut Identities,
    quota: &mut GenomeQuota,
    workers: usize,
) -> Result<BirthResolution, CoreError> {
    let mut order: Vec<&BirthRequest<'_>> = requests.iter().collect();
    order.sort_unstable_by_key(|request| request.intent.parent);
    if let Some(pair) = order
        .windows(2)
        .find(|pair| pair[0].intent.parent == pair[1].intent.parent)
    {
        return Err(CoreError::StateConflict {
            operation: "resolve births",
            detail: format!("{} proposed two births", pair[0].intent.parent),
        });
    }
    let Some(tick) = order.first().map(|request| request.intent.tick) else {
        return Ok(BirthResolution::Staged(Births {
            born: Vec::new(),
            rejected: Vec::new(),
            refused: Vec::new(),
        }));
    };
    if order.iter().any(|request| request.intent.tick != tick) {
        return Err(CoreError::StateConflict {
            operation: "resolve births",
            detail: "the requests belong to different ticks".into(),
        });
    }
    let birth_tick = tick.next()?;
    let deadline = birth_tick.checked_add(context.reproduction.rules.cooldown)?;

    let mut working = identities.clone();
    // Each request ends in one birth, one rejected birth or one refused
    // birth: room for every request in each list, so none grows (FL-043).
    let mut born: Vec<Birth> = Vec::new();
    let mut rejected: Vec<RejectedBirth> = Vec::new();
    let mut refused: Vec<RefusedBirth> = Vec::new();
    reserve_births(&mut born, order.len(), "births")?;
    reserve_births(&mut rejected, order.len(), "rejected births")?;
    let refusing = context.capacity == CapacityPolicy::RefuseExcessBirths;
    if refusing {
        reserve_births(&mut refused, order.len(), "refused births")?;
        admit(context.rng, tick, population, &mut order, &mut refused)?;
    }

    let max_genome_bytes = context.mutator.maximum_genome_bytes(quota.dtype());
    let wave_size = usize::try_from(PARALLEL_PROPOSAL_BUDGET_BYTES / max_genome_bytes)
        .unwrap_or(1)
        .clamp(1, MAX_PARALLEL_WAVE);
    let parallel_workers = workers.min(wave_size).min(order.len());
    let use_wave_preparer = first.is_empty() && preparer.active();
    let use_parallel = !use_wave_preparer
        && parallel_workers > 1
        && order.len() >= MIN_PARALLEL_BIRTHS
        && matches!(context.offspring, Offspring::Inherited)
        && first.is_empty();

    if use_wave_preparer {
        for wave in order.chunks(wave_size) {
            if preparer.active() {
                preparer.prepare_wave(wave);
            }
            let wave_prepared = if preparer.active() {
                preparer.prepared()
            } else {
                Vec::new()
            };
            for request in wave {
                let prepared = wave_prepared
                    .binary_search_by_key(&request.intent.parent, |(parent, _)| *parent)
                    .ok()
                    .and_then(|at| wave_prepared.get(at))
                    .map(|(_, prepared)| *prepared);
                match stage(
                    context,
                    prepared,
                    request,
                    birth_tick,
                    deadline,
                    population,
                    &mut working,
                    quota,
                    born.len(),
                ) {
                    Ok(Staged::Born(birth)) => born.push(*birth),
                    Ok(Staged::Rejected(birth)) => rejected.push(birth),
                    Ok(Staged::Short { resource, .. }) if refusing => {
                        refused.push(RefusedBirth {
                            parent: request.intent.parent,
                            resource,
                        });
                    }
                    Ok(Staged::Short {
                        resource,
                        required,
                        available,
                    }) => {
                        let covered = born.len();
                        roll_back(born, quota)?;
                        return Ok(BirthResolution::Paused(CapacityShortfall {
                            resource,
                            parent: request.intent.parent,
                            required,
                            available,
                            proposed: order.iter().map(|request| request.intent.parent).collect(),
                            covered,
                        }));
                    }
                    Err(error) => {
                        roll_back(born, quota)?;
                        return Err(error);
                    }
                }
            }
        }
    } else if use_parallel {
        let shortfall = stage_parallel_waves(
            context,
            &order,
            birth_tick,
            deadline,
            population,
            &mut working,
            quota,
            refusing,
            workers,
            wave_size,
            &mut born,
            &mut rejected,
            &mut refused,
        )?;
        if let Some(shortfall) = shortfall {
            return Ok(BirthResolution::Paused(shortfall));
        }
    } else {
        for request in &order {
            let prepared = first
                .binary_search_by_key(&request.intent.parent, |(parent, _)| *parent)
                .ok()
                .and_then(|at| first.get(at))
                .map(|(_, prepared)| *prepared);
            match stage(
                context,
                prepared,
                request,
                birth_tick,
                deadline,
                population,
                &mut working,
                quota,
                born.len(),
            ) {
                Ok(Staged::Born(birth)) => born.push(*birth),
                Ok(Staged::Rejected(birth)) => rejected.push(birth),
                Ok(Staged::Short { resource, .. }) if refusing => {
                    refused.push(RefusedBirth {
                        parent: request.intent.parent,
                        resource,
                    });
                }
                Ok(Staged::Short {
                    resource,
                    required,
                    available,
                }) => {
                    let covered = born.len();
                    roll_back(born, quota)?;
                    return Ok(BirthResolution::Paused(CapacityShortfall {
                        resource,
                        parent: request.intent.parent,
                        required,
                        available,
                        proposed: order.iter().map(|request| request.intent.parent).collect(),
                        covered,
                    }));
                }
                Err(error) => {
                    roll_back(born, quota)?;
                    return Err(error);
                }
            }
        }
    }
    *identities = working;
    refused.sort_unstable_by_key(|birth| birth.parent);
    Ok(BirthResolution::Staged(Births {
        born,
        rejected,
        refused,
    }))
}

fn propose_safely(
    propose_one: &(impl Fn(&BirthRequest<'_>) -> Result<DetachedProposal, CoreError> + Sync),
    request: &BirthRequest<'_>,
) -> Result<DetachedProposal, CoreError> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| propose_one(request))) {
        Ok(result) => result,
        Err(_) => Err(CoreError::StateConflict {
            operation: "propose births",
            detail: "a birth proposal worker panicked".into(),
        }),
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "private wave coordinator for resolve_births_with_workers"
)]
fn stage_parallel_waves(
    context: &BirthContext<'_>,
    order: &[&BirthRequest<'_>],
    birth_tick: Tick,
    deadline: Tick,
    population: PopulationQuota,
    working: &mut Identities,
    quota: &mut GenomeQuota,
    refusing: bool,
    workers: usize,
    wave_size: usize,
    born: &mut Vec<Birth>,
    rejected: &mut Vec<RejectedBirth>,
    refused: &mut Vec<RefusedBirth>,
) -> Result<Option<CapacityShortfall>, CoreError> {
    let dtype = quota.dtype();
    stage_parallel_waves_with(
        context,
        order,
        birth_tick,
        deadline,
        population,
        working,
        quota,
        refusing,
        workers,
        wave_size,
        born,
        rejected,
        refused,
        &|request| {
            let attempt = BirthAttempt {
                parent: request.intent.parent,
                tick: request.intent.tick,
            };
            context
                .mutator
                .propose_detached(request.genome, attempt, dtype)
        },
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "private wave coordinator for resolve_births_with_workers"
)]
fn stage_parallel_waves_with(
    context: &BirthContext<'_>,
    order: &[&BirthRequest<'_>],
    birth_tick: Tick,
    deadline: Tick,
    population: PopulationQuota,
    working: &mut Identities,
    quota: &mut GenomeQuota,
    refusing: bool,
    workers: usize,
    wave_size: usize,
    born: &mut Vec<Birth>,
    rejected: &mut Vec<RejectedBirth>,
    refused: &mut Vec<RefusedBirth>,
    propose_one: &(impl Fn(&BirthRequest<'_>) -> Result<DetachedProposal, CoreError> + Sync),
) -> Result<Option<CapacityShortfall>, CoreError> {
    let mut wave_slots: Vec<Option<Result<DetachedProposal, CoreError>>> = Vec::new();
    reserve_births(&mut wave_slots, wave_size, "birth proposal wave")?;

    let mut outcome: Result<Option<CapacityShortfall>, CoreError> = Ok(None);
    let mut wave_start = 0;
    'waves: while wave_start < order.len() {
        let end = (wave_start + wave_size).min(order.len());
        let wave_len = end - wave_start;
        let wave_requests = &order[wave_start..end];
        wave_slots.clear();
        wave_slots.resize_with(wave_len, || None);

        if let Err(error) = flife_core::pool::parallel_chunks_mut(
            workers,
            "propose births",
            &mut wave_slots,
            1,
            |chunk_start, chunk| {
                for (local, slot) in chunk.iter_mut().enumerate() {
                    if let Some(request) = wave_requests.get(chunk_start + local) {
                        *slot = Some(propose_safely(propose_one, request));
                    }
                }
            },
        ) {
            wave_slots.clear();
            outcome = Err(error);
            break 'waves;
        }

        for offset in 0..wave_len {
            let Some(request) = wave_requests.get(offset) else {
                break;
            };
            let Some(proposed) = wave_slots.get_mut(offset).and_then(Option::take) else {
                outcome = Err(CoreError::StateConflict {
                    operation: "propose births",
                    detail: "a birth proposal worker did not return a result".into(),
                });
                break 'waves;
            };
            match stage_detached(
                context,
                request,
                proposed,
                birth_tick,
                deadline,
                population,
                working,
                quota,
                born.len(),
            ) {
                Ok(Staged::Born(birth)) => born.push(*birth),
                Ok(Staged::Rejected(birth)) => rejected.push(birth),
                Ok(Staged::Short { resource, .. }) if refusing => {
                    refused.push(RefusedBirth {
                        parent: request.intent.parent,
                        resource,
                    });
                }
                Ok(Staged::Short {
                    resource,
                    required,
                    available,
                }) => {
                    let covered = born.len();
                    wave_slots.clear();
                    outcome = Ok(Some(CapacityShortfall {
                        resource,
                        parent: request.intent.parent,
                        required,
                        available,
                        proposed: order.iter().map(|req| req.intent.parent).collect(),
                        covered,
                    }));
                    break 'waves;
                }
                Err(error) => {
                    wave_slots.clear();
                    outcome = Err(error);
                    break 'waves;
                }
            }
        }
        wave_start = end;
    }

    match outcome {
        Ok(None) => Ok(None),
        Ok(Some(shortfall)) => {
            let drained = std::mem::take(born);
            roll_back(drained, quota)?;
            Ok(Some(shortfall))
        }
        Err(error) => {
            let drained = std::mem::take(born);
            roll_back(drained, quota)?;
            Err(error)
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "private commit step matching stage() for a pre-proposed detached genome"
)]
fn stage_detached(
    context: &BirthContext<'_>,
    request: &BirthRequest<'_>,
    proposed: Result<DetachedProposal, CoreError>,
    birth_tick: Tick,
    deadline: Tick,
    population: PopulationQuota,
    identities: &mut Identities,
    quota: &mut GenomeQuota,
    staged: usize,
) -> Result<Staged, CoreError> {
    let intent = &request.intent;
    let staged = u64::try_from(staged).unwrap_or(u64::MAX);
    if staged >= population.available() {
        return Ok(Staged::Short {
            resource: Capacity::PopulationSlots,
            required: 1,
            available: population.available().saturating_sub(staged),
        });
    }
    let offset = match intent.budding_offset {
        Some(offset) => offset,
        None => context
            .reproduction
            .child_offset(context.rng, intent.parent, intent.tick)?,
    };
    let position = context.grid.wrap_position([
        intent.position[0] + offset[0],
        intent.position[1] + offset[1],
    ])?;
    let mut issued = identities.clone();
    let child = issued.organisms.issue()?;
    let genome_id = issued.genomes.issue()?;
    let phase = stable_phase(child, context.reproduction.rules.controller_interval)?;
    let attempt = BirthAttempt {
        parent: intent.parent,
        tick: intent.tick,
    };
    let committed = match proposed {
        Ok(detached) => {
            context
                .mutator
                .commit_detached(detached, request.genome, attempt, genome_id, quota)
        }
        Err(error) => Err(error),
    };
    let proposal = match committed {
        Ok(proposal) => proposal,
        Err(CoreError::BudgetExhausted {
            budget,
            required,
            available,
        }) if budget == LOGICAL_GENOME_BYTES => {
            return Ok(Staged::Short {
                resource: Capacity::LogicalGenomeBytes,
                required,
                available,
            });
        }
        Err(error) => return Err(error),
    };
    match proposal {
        Proposal::Rejected { candidates } => Ok(Staged::Rejected(RejectedBirth {
            parent: intent.parent,
            candidates,
        })),
        Proposal::Child { genome, record } => {
            let held = match intent.newborn_state {
                Some(state) => state.into_held_output()?,
                None => HeldOutput::INVALID,
            };
            *identities = issued;
            Ok(Staged::Born(Box::new(Birth {
                parent: intent.parent,
                split: intent.split,
                parent_cooldown_deadline: deadline,
                newborn: Newborn {
                    id: child,
                    genome: genome.id(),
                    parent: intent.parent,
                    lineage: intent.lineage,
                    generation: intent.generation,
                    birth_tick,
                    position,
                    offset,
                    velocity: [0.0, 0.0],
                    energy: intent.split.child,
                    energy_change_rate: 0.0,
                    held_output_valid: false,
                    held,
                    phase,
                    cooldown_deadline: deadline,
                    polarity: intent.newborn_polarity.unwrap_or(DEFAULT_POLARITY),
                },
                genome,
                record,
            })))
        }
    }
}

/// `refuse-excess-births-v1`'s slot admission: when `order` (ascending
/// parent identity) holds more births than `population` has free slots,
/// keep the births with the smallest admission keys, still in ascending
/// parent identity, and move the rest to `refused`.
///
/// # Errors
/// [`CoreError::AllocationFailed`] if the keys cannot be held.
fn admit(
    rng: &SemanticRng,
    tick: Tick,
    population: PopulationQuota,
    order: &mut Vec<&BirthRequest<'_>>,
    refused: &mut Vec<RefusedBirth>,
) -> Result<(), CoreError> {
    let slots = usize::try_from(population.available()).unwrap_or(usize::MAX);
    if order.len() <= slots {
        return Ok(());
    }
    let mut keys: Vec<(u64, OrganismId)> = Vec::new();
    reserve_births(&mut keys, order.len(), "admission keys")?;
    for request in order.iter() {
        let parent = request.intent.parent;
        keys.push((admission_key(rng, parent, tick)?, parent));
    }
    // The first `slots` keys are admitted; they are sorted back by parent
    // for the membership test.
    keys.sort_unstable();
    let admitted = keys.get_mut(..slots).unwrap_or_default();
    admitted.sort_unstable_by_key(|&(_, parent)| parent);
    order.retain(|request| {
        let parent = request.intent.parent;
        let keep = admitted
            .binary_search_by_key(&parent, |&(_, admitted)| admitted)
            .is_ok();
        if !keep {
            refused.push(RefusedBirth {
                parent,
                resource: Capacity::PopulationSlots,
            });
        }
        keep
    });
    Ok(())
}

/// A parent's admission key at attempt tick `tick`: word 0 of
/// `(birth, parent, n, lane (attempt, 1), 0)`.
///
/// # Errors
/// As `Lane::new`, which does not fail for index 1.
pub fn admission_key(rng: &SemanticRng, parent: OrganismId, tick: Tick) -> Result<u64, CoreError> {
    let [word, ..] = rng.block(Address {
        stream: Stream::Birth,
        entity: parent.get(),
        ordinal: tick.get(),
        lane: Lane::new(0, LaneKind::Attempt, 1)?,
        draw: 0,
    });
    Ok(word)
}

/// One request's staging outcome.
enum Staged {
    Born(Box<Birth>),
    Rejected(RejectedBirth),
    Short {
        resource: Capacity,
        required: u64,
        available: u64,
    },
}

#[expect(
    clippy::too_many_arguments,
    reason = "private staging step of resolve_births; the arguments are its working state"
)]
fn stage(
    context: &BirthContext<'_>,
    prepared: Option<Prepared<'_>>,
    request: &BirthRequest<'_>,
    birth_tick: Tick,
    deadline: Tick,
    population: PopulationQuota,
    identities: &mut Identities,
    quota: &mut GenomeQuota,
    staged: usize,
) -> Result<Staged, CoreError> {
    let intent = &request.intent;
    let staged = u64::try_from(staged).unwrap_or(u64::MAX);
    if staged >= population.available() {
        return Ok(Staged::Short {
            resource: Capacity::PopulationSlots,
            required: 1,
            available: population.available().saturating_sub(staged),
        });
    }
    let offset = match intent.budding_offset {
        Some(offset) => offset,
        None => context
            .reproduction
            .child_offset(context.rng, intent.parent, intent.tick)?,
    };
    let position = context.grid.wrap_position([
        intent.position[0] + offset[0],
        intent.position[1] + offset[1],
    ])?;
    let mut issued = identities.clone();
    let child = issued.organisms.issue()?;
    let genome_id = issued.genomes.issue()?;
    let phase = stable_phase(child, context.reproduction.rules.controller_interval)?;
    let attempt = BirthAttempt {
        parent: intent.parent,
        tick: intent.tick,
    };
    let proposed = match context.offspring {
        Offspring::Inherited => match prepared {
            Some(first) => {
                context
                    .mutator
                    .propose_prepared(request.genome, attempt, genome_id, quota, first)
            }
            None => context
                .mutator
                .propose(request.genome, attempt, genome_id, quota),
        },
        Offspring::Fresh {
            catalog,
            limits,
            initial_scale,
        } => founder_v2(
            context.rng,
            child,
            genome_id,
            catalog,
            quota,
            limits,
            initial_scale,
        )
        .map(|genome| Proposal::Child {
            genome: Box::new(genome),
            record: MutationRecord {
                candidate: 0,
                topology: None,
                replaced: 0,
                perturbed: 0,
                rejected: Vec::new(),
            },
        }),
    };
    let proposal = match proposed {
        Ok(proposal) => proposal,
        Err(CoreError::BudgetExhausted {
            budget,
            required,
            available,
        }) if budget == LOGICAL_GENOME_BYTES => {
            return Ok(Staged::Short {
                resource: Capacity::LogicalGenomeBytes,
                required,
                available,
            });
        }
        Err(error) => return Err(error),
    };
    match proposal {
        Proposal::Rejected { candidates } => Ok(Staged::Rejected(RejectedBirth {
            parent: intent.parent,
            candidates,
        })),
        Proposal::Child { genome, record } => {
            let held = match intent.newborn_state {
                Some(state) => state.into_held_output()?,
                None => HeldOutput::INVALID,
            };
            *identities = issued;
            Ok(Staged::Born(Box::new(Birth {
                parent: intent.parent,
                split: intent.split,
                parent_cooldown_deadline: deadline,
                newborn: Newborn {
                    id: child,
                    genome: genome.id(),
                    parent: intent.parent,
                    lineage: intent.lineage,
                    generation: intent.generation,
                    birth_tick,
                    position,
                    offset,
                    velocity: [0.0, 0.0],
                    energy: intent.split.child,
                    energy_change_rate: 0.0,
                    held_output_valid: false,
                    held,
                    phase,
                    cooldown_deadline: deadline,
                    polarity: intent.newborn_polarity.unwrap_or(DEFAULT_POLARITY),
                },
                genome,
                record,
            })))
        }
    }
}

/// Return every staged genome's charge, newest first.
/// Room for exactly `count` more in `list`.
fn reserve_births<T>(
    list: &mut Vec<T>,
    count: usize,
    buffer: &'static str,
) -> Result<(), CoreError> {
    list.try_reserve_exact(count)
        .map_err(|_| CoreError::AllocationFailed {
            buffer,
            bytes: flife_core::memory::bytes_for::<T>(count),
        })
}

fn roll_back(born: Vec<Birth>, quota: &mut GenomeQuota) -> Result<(), CoreError> {
    for birth in born.into_iter().rev() {
        quota.retire(*birth.genome)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flife_genome::{MutationParameters, founder};

    #[test]
    fn parallel_proposal_panic_returns_state_conflict_without_deadlock_and_rolls_back() {
        let plan = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");
        let appendix = &plan[plan.find("## Appendix A").unwrap()..];
        let start = appendix.find("```toml\n").unwrap() + "```toml\n".len();
        let length = appendix[start..].find("```").unwrap();
        let validated = ValidatedConfig::from_toml(&appendix[start..start + length]).unwrap();
        let grid = FieldGrid::from_config(&validated).unwrap();
        let reproduction = Reproduction::from_config(&validated).unwrap();
        let catalog = TopologyCatalog::from_config(&validated).unwrap();
        let limits = TraitLimits::from_config(&validated).unwrap();
        let mut quota = GenomeQuota::from_config(&validated).unwrap();
        let rng = SemanticRng::new(99);
        let mutator = Mutator::new(
            rng,
            MutationParameters::from_config(&validated).unwrap(),
            &catalog,
            limits,
        )
        .unwrap();

        let mut identities = Identities {
            organisms: IdSequence::starting_at(1),
            genomes: IdSequence::starting_at(1),
        };
        let mut parents = Vec::new();
        for i in 0..8u64 {
            let id = identities.organisms.issue().unwrap();
            let gid = identities.genomes.issue().unwrap();
            let genome = founder(&rng, id, gid, &catalog, &mut quota, &limits, 1.0).unwrap();
            parents.push((id, i, genome));
        }

        let Viability::Viable(split) = reproduction.split(10.0, 1.0 / 3.0).unwrap() else {
            panic!("expected viable split");
        };
        let intents: Vec<BirthIntent> = parents
            .iter()
            .map(|(id, i, _)| BirthIntent {
                parent: *id,
                lineage: LineageId::new(1),
                generation: Generation::new(1),
                tick: Tick::new(5),
                position: [1.0 + *i as f64, 2.0],
                split,
                budding_offset: None,
                newborn_polarity: None,
                newborn_state: None,
            })
            .collect();
        let requests: Vec<BirthRequest<'_>> = intents
            .iter()
            .zip(&parents)
            .map(|(intent, (_, _, genome))| BirthRequest {
                intent: *intent,
                genome,
            })
            .collect();
        let order: Vec<&BirthRequest<'_>> = requests.iter().collect();
        let context = BirthContext {
            grid: &grid,
            reproduction: &reproduction,
            mutator: &mutator,
            rng: &rng,
            offspring: Offspring::Inherited,
            capacity: CapacityPolicy::PauseBeforeCommit,
        };

        let bytes_before = quota.allocated_bytes();
        let logical_before = quota.logical_bytes();
        let live_before = quota.live();
        let dtype = quota.dtype();

        // Panic on wave 0 (index 0), wave 0 (index 1), and wave 2 (index 5 after waves 0-1 staged births).
        for panic_parent in [parents[0].0, parents[1].0, parents[5].0] {
            let mut working = identities.clone();
            let mut born = Vec::new();
            let mut rejected = Vec::new();
            let mut refused = Vec::new();
            let result = stage_parallel_waves_with(
                &context,
                &order,
                Tick::new(6),
                Tick::new(20),
                PopulationQuota::new(32, 8).unwrap(),
                &mut working,
                &mut quota,
                false,
                4,
                2,
                &mut born,
                &mut rejected,
                &mut refused,
                &|request| {
                    if request.intent.parent == panic_parent {
                        panic!("injected proposal panic for parent {}", panic_parent.get());
                    }
                    let attempt = BirthAttempt {
                        parent: request.intent.parent,
                        tick: request.intent.tick,
                    };
                    context
                        .mutator
                        .propose_detached(request.genome, attempt, dtype)
                },
            );
            assert!(
                matches!(
                    result,
                    Err(CoreError::StateConflict {
                        operation: "propose births",
                        ..
                    })
                ),
                "expected StateConflict on panic for parent {}, got {result:?}",
                panic_parent.get()
            );
            assert!(born.is_empty(), "staged births must be drained on rollback");
            assert_eq!(
                quota.allocated_bytes(),
                bytes_before,
                "genome quota bytes must roll back after panic"
            );
            assert_eq!(
                quota.logical_bytes(),
                logical_before,
                "logical genome bytes must roll back after panic"
            );
            assert_eq!(
                quota.live(),
                live_before,
                "live genome count must roll back after panic"
            );
        }
    }
}
