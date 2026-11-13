//! FL-032 (`flife-kf8`): phase 7's reproduction proposal on Metal (plan §7.4,
//! §8.1 phase 7, A.3 item 10). [`MetalProposal::settle`] runs, one thread per
//! organism alive after emission, what flife-engine's `Reproduction::propose`
//! runs: the exhaustion and cooldown checks, the attempt's Bernoulli draw
//! (word 0 of `(birth, parent, tick, lane (attempt, 0), 0)` under
//! `fnp-philox-semantic-v1`, Philox4x64-10 on the device, `philox.metal`),
//! and the energy split. Every proposal and every value of its split is the
//! reference's, bit for bit (`tests/proposal.rs`).
//!
//! The caller builds each birth intent from the split. As in
//! [`crate::chemistry`], a thread that meets an input the reference would
//! refuse reports a status instead of an outcome, and the call is refused,
//! so the reference decides.

use flife_core::CoreError;
use flife_core::rng::{Lane, LaneKind, Stream};
use flife_field::settle::{ProposalConstants, ProposalOrganism, ProposalOutcome};
use ft_kernel_metal::compute::{Grid, MathMode, Pipeline};

use crate::chemistry::{Slot, put, value, word};
use crate::owner::{Binding, DeviceOwner, Kernel, Limits, Purpose, metal};

/// The proposal kernel source, with the software binary64 and Philox ahead
/// of it.
pub const PROPOSAL_MSL: &str = concat!(
    include_str!("softfloat64.metal"),
    include_str!("philox.metal"),
    include_str!("proposal.metal")
);

/// `proposal.metal`'s layouts, in words.
const PARAMS: usize = 15;
const ORGANISM: usize = 13;
const OUTCOME: usize = 9;
const ADDRESS: usize = 14;
/// Words per row of [`MetalProposal::proposal_ops`]'s results.
pub const PROPOSAL_OPS_WORDS: usize = 18;

/// Threads per threadgroup.
const THREADS: usize = 64;

fn overflow() -> CoreError {
    CoreError::Overflow {
        quantity: "metal proposal size",
    }
}

/// `value`'s `(low, high)` word pair at `target[at..at + 2]`.
fn put_word(target: &mut [u32], at: usize, value: u64) {
    target[at] = value as u32;
    target[at + 1] = (value >> 32) as u32;
}

/// What a nonzero status in `proposal.metal` means.
fn status_name(status: u32) -> &'static str {
    match status {
        1 => "an attempt probability outside [0, 1]",
        2 => "a balance that is not finite and nonnegative",
        3 => "a child fraction outside (0, 1)",
        _ => "an unknown status",
    }
}

/// Phase 7's proposal on Metal; see the module documentation.
pub struct MetalProposal {
    owner: DeviceOwner,
    settle: Pipeline,
    ops: Pipeline,
    params: Slot,
    organisms: Slot,
    out: Slot,
    host: Vec<u32>,
}

impl std::fmt::Debug for MetalProposal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MetalProposal")
            .field("device", &self.owner.device_name())
            .finish_non_exhaustive()
    }
}

impl MetalProposal {
    /// Open the default Metal device and compile the proposal kernels.
    ///
    /// # Errors
    /// [`CoreError::MissingCapability`] without a Metal device (always, off
    /// macOS); [`CoreError::StateConflict`] if the kernels do not compile.
    pub fn open() -> Result<Self, CoreError> {
        Self::with_owner(DeviceOwner::open(Limits::UNBOUNDED)?)
    }

    /// The proposal kernels on `owner`'s device; their buffers are
    /// `owner`'s.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] if the kernels do not compile.
    pub fn with_owner(owner: DeviceOwner) -> Result<Self, CoreError> {
        let library = owner.library(PROPOSAL_MSL, MathMode::Safe)?;
        let pipeline = |name: &str| library.pipeline(name).map_err(metal);
        Ok(Self {
            settle: pipeline("proposal_settle")?,
            ops: pipeline("proposal_ops")?,
            owner,
            params: Slot::new(),
            organisms: Slot::new(),
            out: Slot::new(),
            host: Vec::new(),
        })
    }

    /// The device's name.
    #[must_use]
    pub fn device_name(&self) -> String {
        self.owner.device_name()
    }

    /// The owner of every buffer these kernels use.
    #[must_use]
    pub const fn owner(&self) -> &DeviceOwner {
        &self.owner
    }

    /// Every organism's proposal under `constants`: `out[i]` is
    /// `organisms[i]`'s, as the reference computes it.
    ///
    /// # Errors
    /// - [`CoreError::NumericalInvalid`] for a constant that is not finite.
    /// - [`CoreError::StateConflict`], after the command, when any organism
    ///   meets an input the reference refuses: the caller settles that tick
    ///   on the reference. Also if the dispatch fails.
    /// - [`CoreError::Overflow`] for a call too large to describe in `u32`.
    ///
    /// On error `out` is left empty.
    pub fn settle(
        &mut self,
        constants: &ProposalConstants,
        organisms: &[ProposalOrganism],
        out: &mut Vec<ProposalOutcome>,
    ) -> Result<(), CoreError> {
        out.clear();
        let mut params = [0_u32; PARAMS];
        for (at, quantity, constant) in [
            (
                7,
                "reproduction.minimum_parent_energy_before_attempt",
                constants.minimum_attempt_energy,
            ),
            (9, "costs.birth_energy", constants.birth_cost),
            (
                11,
                "reproduction.minimum_child_energy",
                constants.minimum_child_energy,
            ),
            (
                13,
                "reproduction.minimum_parent_energy_after_split",
                constants.minimum_parent_energy,
            ),
        ] {
            if !constant.is_finite() {
                return Err(CoreError::NumericalInvalid {
                    quantity,
                    detail: format!("{constant} is not finite"),
                });
            }
            put(&mut params, at, constant);
        }
        if organisms.is_empty() {
            return Ok(());
        }
        params[0] = word(organisms.len())?;
        put_word(&mut params, 1, constants.seed);
        put_word(&mut params, 3, Stream::Birth.code());
        put_word(&mut params, 5, Lane::new(0, LaneKind::Attempt, 0)?.get());

        let host = &mut self.host;
        host.clear();
        host.try_reserve(organisms.len().saturating_mul(ORGANISM))
            .map_err(|_| overflow())?;
        for organism in organisms {
            let mut row = [0_u32; ORGANISM];
            put_word(&mut row, 0, organism.entity);
            put_word(&mut row, 2, organism.tick);
            put_word(&mut row, 4, organism.cooldown_deadline);
            put(&mut row, 6, organism.remaining);
            put(&mut row, 8, organism.attempt_probability);
            put(&mut row, 10, organism.child_fraction);
            row[12] = u32::from(organism.exhausted);
            host.extend_from_slice(&row);
        }

        let Self {
            owner,
            settle,
            params: params_slot,
            organisms: organisms_slot,
            out: out_slot,
            host,
            ..
        } = self;
        let params_buffer = params_slot.ensure(owner, PARAMS)?.buffer();
        let organisms_buffer = organisms_slot.ensure(owner, host.len())?.buffer();
        let rows = organisms.len().checked_mul(OUTCOME).ok_or_else(overflow)?;
        let out_buffer = out_slot.ensure(owner, rows)?.buffer();
        owner.write(params_buffer)?.write(0, &params)?;
        owner.write(organisms_buffer)?.write(0, host)?;
        let bindings: [Binding; 3] = [
            params_buffer.reads(),
            organisms_buffer.reads(),
            out_buffer.writes(),
        ];
        owner.submit_batch(&[Kernel {
            pipeline: settle,
            bindings: &bindings,
            grid: Grid::linear(organisms.len().div_ceil(THREADS), THREADS),
        }])?;

        host.clear();
        host.resize(rows, 0);
        owner.read(out_buffer, Purpose::Readback)?.read(host)?;
        out.try_reserve(organisms.len()).map_err(|_| overflow())?;
        for (index, row) in host.as_chunks::<OUTCOME>().0.iter().enumerate() {
            let outcome = match (row[0], row[1], row[2]) {
                (0, 0, _) => Some(ProposalOutcome::Exhausted),
                (0, 1, _) => Some(ProposalOutcome::CoolingDown),
                (0, 2, _) => Some(ProposalOutcome::NoAttempt),
                (0, 3, 0) => Some(ProposalOutcome::BelowAttemptEnergy {
                    available: value(&row[3..5]),
                }),
                (0, 3, 1) => Some(ProposalOutcome::BirthCostUnaffordable {
                    available: value(&row[3..5]),
                }),
                (0, 3, 2) => Some(ProposalOutcome::ChildBelowMinimum {
                    child: value(&row[5..7]),
                }),
                (0, 3, 3) => Some(ProposalOutcome::ParentBelowMinimum {
                    parent: value(&row[7..9]),
                }),
                (0, 4, _) => Some(ProposalOutcome::Viable {
                    available: value(&row[3..5]),
                    child: value(&row[5..7]),
                    parent: value(&row[7..9]),
                }),
                _ => None,
            };
            let Some(outcome) = outcome else {
                out.clear();
                return Err(CoreError::StateConflict {
                    operation: "metal proposal",
                    detail: format!(
                        "organism {index} of the call met {} (proposal {}, reason {}); the \
                         reference settles this tick",
                        status_name(row[0]),
                        row[1],
                        row[2]
                    ),
                });
            };
            out.push(outcome);
        }
        Ok(())
    }

    /// The kernel's randomness for each row `[seed, stream, entity, ordinal,
    /// lane, draw, word]`: each result row holds, as `(low, high)` word
    /// pairs, the four words of Philox4x64-10 of the counter plus one under
    /// the key, each one's half-open uniform, and `word`'s.
    /// `addresses.len()` must be a positive multiple of 64.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] for another length;
    /// [`CoreError::StateConflict`] if the dispatch fails.
    pub fn proposal_ops(
        &self,
        addresses: &[[u64; 7]],
        out: &mut Vec<[u32; PROPOSAL_OPS_WORDS]>,
    ) -> Result<(), CoreError> {
        if addresses.is_empty() || !addresses.len().is_multiple_of(64) {
            return Err(CoreError::LayoutMismatch {
                buffer: "proposal addresses",
                detail: format!("{} addresses", addresses.len()),
            });
        }
        let mut words = vec![0_u32; addresses.len() * ADDRESS];
        for (row, address) in words.as_chunks_mut::<ADDRESS>().0.iter_mut().zip(addresses) {
            for (k, &part) in address.iter().enumerate() {
                put_word(row, 2 * k, part);
            }
        }
        let input = self.owner.upload_owned(&words)?;
        let results = self
            .owner
            .zeroed_owned::<u32>(addresses.len() * PROPOSAL_OPS_WORDS)?;
        self.owner.submit(
            &self.ops,
            &[input.buffer().reads(), results.buffer().writes()],
            Grid::linear(addresses.len() / 64, 64),
        )?;
        let mut flat = vec![0_u32; addresses.len() * PROPOSAL_OPS_WORDS];
        self.owner.read_back(results.buffer(), &mut flat)?;
        out.clear();
        out.extend(flat.as_chunks::<PROPOSAL_OPS_WORDS>().0.iter().copied());
        Ok(())
    }
}
