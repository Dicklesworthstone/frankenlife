//! Bounded, semantically addressed mutation proposals (plan §7.3, A.3
//! item 4, Appendix C.16).
//!
//! A birth attempt by parent organism `p` at tick `t` tries candidates
//! `c = 0, 1, …` up to `maximum_candidate_proposals_per_birth` (at most
//! eight, §7.3). Every word a candidate uses is addressed
//! `(mutation, p, t, lane (c, kind, index), draw)`, so an operational retry
//! of the same tick repeats its proposals exactly, a later tick draws fresh
//! ones, and different candidates never share a word. Within a candidate,
//! in A.3 item 4's order:
//!
//! 1. **Topology** (`uniform-other-catalog-member`). Lane `(c, topology, 0)`:
//!    the change happens when word 0 passes `probability_per_birth`, and
//!    word 1 chooses uniformly among the other admitted members. The parent
//!    is then mapped onto the target ([`crate::transition`]); the map's
//!    added weights take word 0 of lane `(c, map weight, neuron)`.
//! 2. **Parameters**, named by their neuron in the child's topology. Lane
//!    `(c, weight or bias, neuron)`, draw 0: when word 0 passes
//!    `replacement_probability`, the parameter is replaced by
//!    `fan-in-scaled-symmetric-v1` of word 1 with its layer's fan-in;
//!    otherwise, when word 2 passes `weight_probability`, it is perturbed by
//!    `weight_standard_deviation` times component 0 of the Box–Muller pair of
//!    draw 1. Arithmetic is F64, rounded once to F32 ([`perturb`]). Biases
//!    follow the same rule.
//! 3. **Traits.** The affinity direction adds `chemistry_standard_deviation`
//!    times 32 normals (lanes `(c, affinity, j)`) and normalizes back to the
//!    sphere of radius `min(1, affinity_norm_limit)`. The motion response
//!    adds the same deviation times 33 normals (lanes
//!    `(c, motion response, j)`) and projects radially into its norm ball
//!    when outside it. The sensing scale adds
//!    `sensing_log_scale_standard_deviation` times component 0 of lane
//!    `(c, sensing scale, 0)` to its natural logarithm and reflects at the
//!    log bounds ([`reflect_log_scale`]). An operator whose deviation is
//!    zero leaves its trait bit for bit.
//!
//! A candidate whose values are nonfinite, out of bound or of zero norm is
//! rejected, never reinterpreted (any [`CoreError::NumericalInvalid`] while
//! building or sealing it). With `reject_effective_noop`, a candidate equal
//! to its parent after canonical rounding, bit for bit in topology,
//! parameters and traits, is rejected too. When every candidate is
//! rejected, the birth is rejected (`reject-birth-no-charge`): every buffer
//! has been abandoned, so nothing stays charged, and energy and cooldown
//! are the birth transaction's to preserve (FL-011). A quota or allocation
//! refusal is not a mutation outcome and is returned as an error at once,
//! with nothing charged (§7.5).

use flife_core::ids::{GenomeId, OrganismId};
use flife_core::numerics::{exp, ln};
use flife_core::rng::{
    Address, Block, Lane, LaneKind, SemanticRng, Stream, bernoulli, box_muller, choose,
};
use flife_core::{CoreError, Tick, ValidatedConfig};
use flife_neural::DenseTopology;

use crate::catalog::{TopologyCatalog, TopologyId};
use crate::draws::normals;
use crate::genome::{
    AFFINITY_TERMS, Genome, MOTION_RESPONSE_TERMS, TraitLimits, Traits, rescale, squared_norm,
};
use crate::init::fan_in_weight;
use crate::quota::{GenomeQuota, ParameterBuffer};
use crate::transition::{Transition, map_parameters};

/// The target selection this module implements.
pub const UNIFORM_OTHER_CATALOG_MEMBER: &str = "uniform-other-catalog-member";

/// The exhausted-proposals policy this module implements.
pub const REJECT_BIRTH_NO_CHARGE: &str = "reject-birth-no-charge";

/// At most this many candidates per attempted birth (§7.3).
pub const MAXIMUM_CANDIDATES: u8 = 8;

/// Reflections tried before a log scale is refused. A deviation of `0.02`
/// in log scale never needs a second one.
const REFLECTIONS: u32 = 64;

/// The run's mutation rates and limits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MutationParameters {
    /// Probability that a candidate changes topology; zero when
    /// architecture mutation is disabled.
    pub topology_change_probability: f64,
    /// Per-parameter perturbation probability.
    pub weight_probability: f64,
    /// Standard deviation of a parameter perturbation.
    pub weight_standard_deviation: f64,
    /// Per-parameter replacement probability.
    pub replacement_probability: f64,
    /// Standard deviation of affinity and motion-response noise.
    pub chemistry_standard_deviation: f64,
    /// Standard deviation of sensing-scale noise, in natural log scale.
    pub sensing_log_scale_standard_deviation: f64,
    /// Reject candidates that equal their parent after canonical rounding.
    pub reject_effective_noop: bool,
    /// Candidates per attempted birth, `1..=8`.
    pub maximum_candidates: u8,
}

impl MutationParameters {
    /// The parameters of a validated configuration.
    ///
    /// # Errors
    /// [`CoreError::UnknownAlgorithm`] for another target selection or
    /// exhausted-proposals policy; otherwise as
    /// [`MutationParameters::validate`].
    pub fn from_config(config: &ValidatedConfig) -> Result<Self, CoreError> {
        let model = config.config();
        let architecture = &model.architecture_mutation;
        let mutation = &model.mutation;
        for (role, name, expected) in [
            (
                "architecture_mutation.target_selection",
                &architecture.target_selection,
                UNIFORM_OTHER_CATALOG_MEMBER,
            ),
            (
                "mutation.exhausted_proposals_policy",
                &mutation.exhausted_proposals_policy,
                REJECT_BIRTH_NO_CHARGE,
            ),
        ] {
            if name != expected {
                return Err(CoreError::UnknownAlgorithm {
                    role,
                    name: name.clone(),
                });
            }
        }
        let parameters = Self {
            topology_change_probability: if architecture.enabled {
                architecture.probability_per_birth
            } else {
                0.0
            },
            weight_probability: mutation.weight_probability,
            weight_standard_deviation: mutation.weight_standard_deviation,
            replacement_probability: mutation.replacement_probability,
            chemistry_standard_deviation: mutation.chemistry_standard_deviation,
            sensing_log_scale_standard_deviation: mutation.sensing_log_scale_standard_deviation,
            reject_effective_noop: mutation.reject_effective_noop,
            maximum_candidates: u8::try_from(mutation.maximum_candidate_proposals_per_birth)
                .unwrap_or(u8::MAX),
        };
        parameters.validate()?;
        Ok(parameters)
    }

    /// Check the rates and limits.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a probability outside `[0, 1]`, a
    /// negative or nonfinite deviation, a candidate count outside `1..=8`,
    /// or a strict no-op rule with every operator disabled (§7.3).
    pub fn validate(&self) -> Result<(), CoreError> {
        for (quantity, value) in [
            (
                "architecture_mutation.probability_per_birth",
                self.topology_change_probability,
            ),
            ("mutation.weight_probability", self.weight_probability),
            (
                "mutation.replacement_probability",
                self.replacement_probability,
            ),
        ] {
            if !(0.0..=1.0).contains(&value) {
                return Err(CoreError::NumericalInvalid {
                    quantity,
                    detail: format!("{value} is not a probability"),
                });
            }
        }
        for (quantity, value) in [
            (
                "mutation.weight_standard_deviation",
                self.weight_standard_deviation,
            ),
            (
                "mutation.chemistry_standard_deviation",
                self.chemistry_standard_deviation,
            ),
            (
                "mutation.sensing_log_scale_standard_deviation",
                self.sensing_log_scale_standard_deviation,
            ),
        ] {
            if !value.is_finite() || value < 0.0 {
                return Err(CoreError::NumericalInvalid {
                    quantity,
                    detail: format!("{value} is not a standard deviation"),
                });
            }
        }
        if !(1..=MAXIMUM_CANDIDATES).contains(&self.maximum_candidates) {
            return Err(CoreError::NumericalInvalid {
                quantity: "mutation.maximum_candidate_proposals_per_birth",
                detail: format!(
                    "{} is outside 1..={MAXIMUM_CANDIDATES}",
                    self.maximum_candidates
                ),
            });
        }
        let any_operator = self.topology_change_probability > 0.0
            || self.replacement_probability > 0.0
            || (self.weight_probability > 0.0 && self.weight_standard_deviation > 0.0)
            || self.chemistry_standard_deviation > 0.0
            || self.sensing_log_scale_standard_deviation > 0.0;
        if self.reject_effective_noop && !any_operator {
            return Err(CoreError::NumericalInvalid {
                quantity: "mutation.reject_effective_noop",
                detail: "no enabled operator can change a genome, so every birth would fail".into(),
            });
        }
        Ok(())
    }
}

/// The birth attempt a proposal belongs to: the parent's stable identity
/// and the event tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BirthAttempt {
    /// The parent organism.
    pub parent: OrganismId,
    /// The tick of the attempt.
    pub tick: Tick,
}

/// A candidate's topology change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TopologyChange {
    /// The parent's member.
    pub from: TopologyId,
    /// The child's member.
    pub to: TopologyId,
    /// The map that took one to the other.
    pub transition: Transition,
}

/// Why a candidate was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectionReason {
    /// Equal to the parent after canonical rounding.
    EffectiveNoop,
    /// Nonfinite, out of bound or of zero norm.
    Invalid(CoreError),
}

/// One rejected candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateRejection {
    /// The candidate index.
    pub candidate: u8,
    /// Why.
    pub reason: RejectionReason,
}

/// The mutation record of an accepted child (§7.1 provenance).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationRecord {
    /// The accepted candidate.
    pub candidate: u8,
    /// Its topology change, if any.
    pub topology: Option<TopologyChange>,
    /// Parameters replaced.
    pub replaced: u32,
    /// Parameters perturbed.
    pub perturbed: u32,
    /// The candidates rejected before it, in order.
    pub rejected: Vec<CandidateRejection>,
}

/// What a birth attempt's mutation produced.
#[derive(Debug)]
pub enum Proposal {
    /// A sealed child genome, charged to the quota.
    Child {
        /// The child, boxed: a genome carries its 65 trait values inline.
        genome: Box<Genome>,
        /// How it was made.
        record: MutationRecord,
    },
    /// Every candidate was rejected; nothing is charged.
    Rejected {
        /// Every candidate, in order.
        candidates: Vec<CandidateRejection>,
    },
}

/// A parameter perturbed in F64 and rounded once to canonical F32.
#[must_use]
pub fn perturb(value: f32, delta: f64) -> f32 {
    (f64::from(value) + delta) as f32
}

/// `exp(ln scale + delta)`, reflected at `ln minimum` and `ln maximum` until
/// inside, then kept within `[minimum, maximum]` against the last bit of
/// `exp`. A range of one scale admits only that scale.
///
/// # Errors
/// [`CoreError::NumericalInvalid`] for a nonpositive or nonfinite scale or
/// bound, or a step that cannot be reflected inside.
pub fn reflect_log_scale(scale: f64, delta: f64, range: [f64; 2]) -> Result<f64, CoreError> {
    let [minimum, maximum] = range;
    let invalid = |detail: String| CoreError::NumericalInvalid {
        quantity: "sensing scale",
        detail,
    };
    let logarithm =
        |value: f64| ln(value).ok_or_else(|| invalid(format!("ln {value} is undefined")));
    let (low, high) = (logarithm(minimum)?, logarithm(maximum)?);
    if low >= high {
        return Ok(minimum);
    }
    let mut log = logarithm(scale)? + delta;
    if !log.is_finite() {
        return Err(invalid(format!("log scale {log} is not finite")));
    }
    for _ in 0..REFLECTIONS {
        if log > high {
            log = 2.0 * high - log;
        } else if log < low {
            log = 2.0 * low - log;
        } else {
            let value = exp(log).ok_or_else(|| invalid(format!("exp {log} overflows")))?;
            return Ok(value.max(minimum).min(maximum));
        }
    }
    Err(invalid(format!(
        "{scale} with log step {delta} does not reflect into [{minimum}, {maximum}]"
    )))
}

/// Mutation for one run: its semantic source, rates, catalog and bounds.
#[derive(Debug, Clone)]
pub struct Mutator<'a> {
    rng: SemanticRng,
    parameters: MutationParameters,
    catalog: &'a TopologyCatalog,
    limits: TraitLimits,
}

/// A candidate's fate before it is recorded.
enum Outcome {
    Accepted(Box<Genome>, Option<TopologyChange>, Counts),
    Rejected(RejectionReason),
}

#[derive(Debug, Clone, Copy, Default)]
struct Counts {
    replaced: u32,
    perturbed: u32,
}

impl<'a> Mutator<'a> {
    /// Mutation under `parameters`.
    ///
    /// # Errors
    /// As [`MutationParameters::validate`].
    pub fn new(
        rng: SemanticRng,
        parameters: MutationParameters,
        catalog: &'a TopologyCatalog,
        limits: TraitLimits,
    ) -> Result<Self, CoreError> {
        parameters.validate()?;
        Ok(Self {
            rng,
            parameters,
            catalog,
            limits,
        })
    }

    /// The rates.
    #[must_use]
    pub const fn parameters(&self) -> &MutationParameters {
        &self.parameters
    }

    /// Propose a child of `parent` for `attempt`, sealed as `child` and
    /// charged to `quota`, or reject the birth with every candidate's reason.
    ///
    /// # Errors
    /// [`CoreError::BudgetExhausted`] or [`CoreError::AllocationFailed`]
    /// when the quota or host refuses a candidate's buffer, with nothing
    /// charged; [`CoreError::LayoutMismatch`] or [`CoreError::Overflow`] for
    /// a parent that does not fit the catalog.
    pub fn propose(
        &self,
        parent: &Genome,
        attempt: BirthAttempt,
        child: GenomeId,
        quota: &mut GenomeQuota,
    ) -> Result<Proposal, CoreError> {
        let mut rejected = Vec::new();
        for candidate in 0..self.parameters.maximum_candidates {
            match self.candidate(parent, attempt, candidate, child, quota)? {
                Outcome::Accepted(genome, topology, counts) => {
                    return Ok(Proposal::Child {
                        genome,
                        record: MutationRecord {
                            candidate,
                            topology,
                            replaced: counts.replaced,
                            perturbed: counts.perturbed,
                            rejected,
                        },
                    });
                }
                Outcome::Rejected(reason) => {
                    rejected.push(CandidateRejection { candidate, reason });
                }
            }
        }
        Ok(Proposal::Rejected {
            candidates: rejected,
        })
    }

    fn candidate(
        &self,
        parent: &Genome,
        attempt: BirthAttempt,
        candidate: u8,
        child: GenomeId,
        quota: &mut GenomeQuota,
    ) -> Result<Outcome, CoreError> {
        let from = parent.topology();
        let to = self.target(from, attempt, candidate)?;
        let mut buffer = quota.allocate(self.catalog, to)?;
        let built = self
            .parameters_of(parent, from, to, attempt, candidate, &mut buffer)
            .and_then(|(transition, counts)| {
                let traits = self.traits_of(parent.traits(), attempt, candidate)?;
                Ok((transition, counts, traits))
            });
        let (transition, counts, traits) = match built {
            Ok(built) => built,
            Err(error) => {
                quota.abandon(buffer)?;
                return rejection(error);
            }
        };
        if self.parameters.reject_effective_noop
            && to == from
            && same_f32(buffer.values(), parent.parameters())
            && same_traits(&traits, parent.traits())
        {
            quota.abandon(buffer)?;
            return Ok(Outcome::Rejected(RejectionReason::EffectiveNoop));
        }
        let topology = transition.map(|transition| TopologyChange {
            from,
            to,
            transition,
        });
        match Genome::seal(
            buffer,
            child,
            self.catalog,
            traits,
            Some(parent.id()),
            &self.limits,
        ) {
            Ok(genome) => Ok(Outcome::Accepted(Box::new(genome), topology, counts)),
            Err(unsealed) => {
                quota.abandon(unsealed.buffer)?;
                rejection(unsealed.error)
            }
        }
    }

    /// Step 1: the candidate's member.
    fn target(
        &self,
        from: TopologyId,
        attempt: BirthAttempt,
        candidate: u8,
    ) -> Result<TopologyId, CoreError> {
        let [decide, pick, ..] =
            self.block(attempt, Lane::new(candidate, LaneKind::Topology, 0)?, 0);
        if !bernoulli(decide, self.parameters.topology_change_probability) {
            return Ok(from);
        }
        let targets: Vec<TopologyId> = self.catalog.transition_targets(from).collect();
        Ok(choose(pick, targets.len())
            .and_then(|index| targets.get(index).copied())
            .unwrap_or(from))
    }

    /// Step 2: map the parent onto `to` and mutate every parameter.
    fn parameters_of(
        &self,
        parent: &Genome,
        from: TopologyId,
        to: TopologyId,
        attempt: BirthAttempt,
        candidate: u8,
        buffer: &mut ParameterBuffer,
    ) -> Result<(Option<Transition>, Counts), CoreError> {
        let source = self.member(from)?;
        let target = self.member(to)?;
        let child = buffer.values_mut();
        let transition = if from == to {
            if child.len() != parent.parameters().len() {
                return Err(CoreError::LayoutMismatch {
                    buffer: "genome parameters",
                    detail: "the parent does not have its member's length".into(),
                });
            }
            child.copy_from_slice(parent.parameters());
            None
        } else {
            let mut lane_error = None;
            let transition = map_parameters(source, parent.parameters(), target, child, |at| {
                match Lane::neuron_index(at.layer, at.output, at.input)
                    .and_then(|index| Lane::new(candidate, LaneKind::MapWeight, index))
                {
                    Ok(lane) => self.block(attempt, lane, 0)[0],
                    Err(error) => {
                        lane_error.get_or_insert(error);
                        0
                    }
                }
            })?;
            if let Some(error) = lane_error {
                return Err(error);
            }
            Some(transition)
        };
        let mut counts = Counts::default();
        if self.parameters.replacement_probability > 0.0 || self.parameters.weight_probability > 0.0
        {
            self.mutate_parameters(target, child, attempt, candidate, &mut counts)?;
        }
        Ok((transition, counts))
    }

    fn mutate_parameters(
        &self,
        topology: &DenseTopology,
        child: &mut [f32],
        attempt: BirthAttempt,
        candidate: u8,
        counts: &mut Counts,
    ) -> Result<(), CoreError> {
        for (layer, shape) in topology.layers().iter().enumerate() {
            for output in 0..shape.outputs {
                for input in 0..shape.inputs {
                    self.mutate_parameter(
                        child,
                        shape.weight(output, input),
                        Lane::new(
                            candidate,
                            LaneKind::Weight,
                            Lane::neuron_index(layer, output, input)?,
                        )?,
                        shape.inputs,
                        attempt,
                        counts,
                    )?;
                }
                self.mutate_parameter(
                    child,
                    shape.bias(output),
                    Lane::new(
                        candidate,
                        LaneKind::Bias,
                        Lane::neuron_index(layer, output, 0)?,
                    )?,
                    shape.inputs,
                    attempt,
                    counts,
                )?;
            }
        }
        Ok(())
    }

    fn mutate_parameter(
        &self,
        child: &mut [f32],
        offset: Option<usize>,
        lane: Lane,
        fan_in: usize,
        attempt: BirthAttempt,
        counts: &mut Counts,
    ) -> Result<(), CoreError> {
        let slot = offset
            .and_then(|offset| child.get_mut(offset))
            .ok_or_else(|| CoreError::LayoutMismatch {
                buffer: "genome parameters",
                detail: "a parameter offset is out of range".into(),
            })?;
        let [replace, replacement, perturbation, _] = self.block(attempt, lane, 0);
        if bernoulli(replace, self.parameters.replacement_probability) {
            *slot = fan_in_weight(replacement, fan_in)?;
            counts.replaced += 1;
        } else if bernoulli(perturbation, self.parameters.weight_probability) {
            let [first, second, ..] = self.block(attempt, lane, 1);
            let [normal, _] = box_muller(first, second)?;
            *slot = perturb(*slot, self.parameters.weight_standard_deviation * normal);
            counts.perturbed += 1;
        }
        Ok(())
    }

    /// Step 3: the heritable trait vectors and the sensing scale.
    fn traits_of(
        &self,
        traits: &Traits,
        attempt: BirthAttempt,
        candidate: u8,
    ) -> Result<Traits, CoreError> {
        let mut affinity = *traits.affinity();
        let mut motion_response = *traits.motion_response();
        let mut scale = traits.sensing_scale();
        let at = |lane| self.address(attempt, lane, 0);
        let deviation = self.parameters.chemistry_standard_deviation;
        if deviation > 0.0 {
            let mut noise = [0.0; AFFINITY_TERMS];
            normals(&self.rng, at, candidate, LaneKind::Affinity, &mut noise)?;
            for (value, normal) in affinity.iter_mut().zip(noise) {
                *value += deviation * normal;
            }
            rescale(
                "affinity direction",
                &mut affinity,
                self.limits.affinity_norm().min(1.0),
            )?;
            let mut noise = [0.0; MOTION_RESPONSE_TERMS];
            normals(
                &self.rng,
                at,
                candidate,
                LaneKind::MotionResponse,
                &mut noise,
            )?;
            for (value, normal) in motion_response.iter_mut().zip(noise) {
                *value += deviation * normal;
            }
            let limit = self.limits.motion_response_norm();
            if squared_norm(&motion_response) > limit * limit {
                rescale("motion response", &mut motion_response, limit)?;
            }
        }
        let deviation = self.parameters.sensing_log_scale_standard_deviation;
        if deviation > 0.0 {
            let [first, second, ..] =
                self.block(attempt, Lane::new(candidate, LaneKind::SensingScale, 0)?, 0);
            let [normal, _] = box_muller(first, second)?;
            scale = reflect_log_scale(scale, deviation * normal, self.limits.scale_range())?;
        }
        Traits::new(affinity, motion_response, scale, &self.limits)
    }

    fn member(&self, id: TopologyId) -> Result<&'a DenseTopology, CoreError> {
        self.catalog
            .topology(id)
            .ok_or_else(|| CoreError::LayoutMismatch {
                buffer: "genome parameters",
                detail: format!("member {} is not in the catalog", id.index()),
            })
    }

    fn address(&self, attempt: BirthAttempt, lane: Lane, draw: u64) -> Address {
        Address {
            stream: Stream::Mutation,
            entity: attempt.parent.get(),
            ordinal: attempt.tick.get(),
            lane,
            draw,
        }
    }

    fn block(&self, attempt: BirthAttempt, lane: Lane, draw: u64) -> Block {
        self.rng.block(self.address(attempt, lane, draw))
    }
}

/// A numerical failure rejects the candidate; anything else is the run's.
fn rejection(error: CoreError) -> Result<Outcome, CoreError> {
    match error {
        CoreError::NumericalInvalid { .. } => {
            Ok(Outcome::Rejected(RejectionReason::Invalid(error)))
        }
        other => Err(other),
    }
}

fn same_f32(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits())
}

fn same_f64(a: &[f64], b: &[f64]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits())
}

fn same_traits(a: &Traits, b: &Traits) -> bool {
    same_f64(a.affinity(), b.affinity())
        && same_f64(a.motion_response(), b.motion_response())
        && a.sensing_scale().to_bits() == b.sensing_scale().to_bits()
}
