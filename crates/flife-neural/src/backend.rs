//! The controller backend port (plan §10.2, §10.7, §15): how the engine
//! hands a tick's due evaluations to an accelerator in one batch. The
//! composition root (the CLI, the Studio) chooses an implementation and
//! gives it to the world; the engine depends only on this trait, and no
//! backend depends on the engine.
//!
//! A backend is an exact replacement for [`ReferenceController::evaluate`],
//! not an approximation: every output it returns is the reference's, bit for
//! bit, and it returns `None` for any evaluation it cannot reproduce. The
//! engine evaluates those on the reference, which also reproduces the
//! reference's refusals. So no result of a run depends on the backend.

use std::fmt::Debug;

use flife_core::CoreError;
use flife_core::ids::GenomeId;

use crate::{ArenaSlot, RawOutputs, ReferenceController};

/// One due organism's controller evaluation.
#[derive(Debug, Clone, Copy)]
pub struct ControllerJob<'a> {
    /// The organism's genome.
    pub genome: GenomeId,
    /// The genome's arena slot. While a backend serves one world, a slot
    /// and generation name one immutable parameter set for as long as that
    /// genome lives, and a later occupant of the slot has a higher
    /// generation. So a backend keeps packed parameters by slot, and reuses
    /// them only for the same generation and genome; anything else it
    /// packs afresh into the same place.
    pub slot: ArenaSlot,
    /// The organism's controller, admitted under the run's limits.
    pub controller: ReferenceController<'a>,
    /// Its controller inputs (`inputs.len() == controller.topology().input_width()`,
    /// up to [`crate::MAX_SENSOR_WIDTH`] = `128`). For recurrent topologies,
    /// the caller appends the current internal state `h[0..S]` at the tail.
    pub inputs: &'a [f64],
}

/// One due organism's controller metadata for
/// [`ControllerBackend::evaluate_flat_f32`], whose F32 inputs live in the
/// caller's contiguous row-major `inputs_f32` buffer.
#[derive(Debug, Clone, Copy)]
pub struct FlatControllerJob<'a> {
    /// The organism's genome.
    pub genome: GenomeId,
    /// The genome's arena slot and generation.
    pub slot: ArenaSlot,
    /// The organism's controller, admitted under the run's limits.
    pub controller: ReferenceController<'a>,
}

/// Contiguous row-major F32 controller outputs returned by
/// [`ControllerBackend::evaluate_flat_f32`], avoiding per-job
/// `Option<RawOutputs>` unpack copies on large populations.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FlatOutputs {
    /// Stride (elements per job) in [`Self::values`]: the batch's
    /// `raw_output_width` (`<= crate::MAX_CONTROLLER_OUTPUT_WIDTH`).
    pub output_width: usize,
    /// Row-major raw linear outputs of length `jobs.len() * output_width`.
    /// Row `i` is meaningful only when `valid[i] != 0`.
    pub values: Vec<f32>,
    /// Per-job validity flags (`1` = evaluated and written into `values`;
    /// `0` = deferred to the scalar reference). Length `jobs.len()`.
    pub valid: Vec<u8>,
}

impl FlatOutputs {
    /// Clear output metadata and lengths while retaining allocated buffer
    /// capacity for reuse across ticks.
    pub fn clear(&mut self) {
        self.output_width = 0;
        self.values.clear();
        self.valid.clear();
    }

    /// Size buffers for `jobs` rows of width `output_width`, resetting all
    /// `valid` flags to `0` (`deferred`) while preserving existing capacity.
    ///
    /// # Errors
    /// [`CoreError::Overflow`] if `jobs * output_width` overflows `usize`;
    /// [`CoreError::AllocationFailed`] if backing capacity cannot be reserved.
    pub fn prepare(&mut self, jobs: usize, output_width: usize) -> Result<(), CoreError> {
        let total = jobs.checked_mul(output_width).ok_or(CoreError::Overflow {
            quantity: "flat controller output elements",
        })?;
        if self.values.capacity() < total {
            let additional = total - self.values.len();
            self.values
                .try_reserve(additional)
                .map_err(|_| CoreError::AllocationFailed {
                    buffer: "flat controller outputs",
                    bytes: u64::try_from(total.saturating_mul(size_of::<f32>()))
                        .unwrap_or(u64::MAX),
                })?;
        }
        if self.valid.capacity() < jobs {
            let additional = jobs - self.valid.len();
            self.valid
                .try_reserve(additional)
                .map_err(|_| CoreError::AllocationFailed {
                    buffer: "flat controller valid flags",
                    bytes: u64::try_from(jobs).unwrap_or(u64::MAX),
                })?;
        }
        self.output_width = output_width;
        self.values.resize(total, 0.0);
        self.valid.clear();
        self.valid.resize(jobs, 0);
        Ok(())
    }

    /// The raw linear outputs for job `index`, or `None` when that job was
    /// deferred (`valid[index] == 0`) or out of range.
    #[must_use]
    pub fn row(&self, index: usize) -> Option<&[f32]> {
        if self.valid.get(index).copied().unwrap_or(0) == 0 || self.output_width == 0 {
            return None;
        }
        let start = index.checked_mul(self.output_width)?;
        let end = start.checked_add(self.output_width)?;
        self.values.get(start..end)
    }
}

/// The memory a controller backend may hold for one world (FL-043, plan
/// §19.4): the preflight's backend term, given to a backend when it is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackendBudget {
    /// Bytes it may hold at once, host and device: resident parameters,
    /// staging and scratch. A backend makes no parameter shard that would
    /// leave too little of it for its staging of a batch of `organisms`
    /// jobs; it defers that shard's jobs to the reference instead.
    pub bytes: u64,
    /// The most jobs one batch holds: the world's population capacity.
    pub organisms: u64,
}

impl BackendBudget {
    /// No limit but the host's and the device's.
    pub const UNBOUNDED: Self = Self {
        bytes: u64::MAX,
        organisms: u64::MAX,
    };
}

/// A batch evaluator of the reference controller.
pub trait ControllerBackend: Debug + Send {
    /// The backend and its device, for run records.
    fn describe(&self) -> String;

    /// The bytes it holds now, host and device: resident parameters,
    /// staging and scratch (FL-043). Within its [`BackendBudget`]'s bytes.
    fn resident_bytes(&self) -> u64;

    /// Drop everything kept resident; the next batch uploads afresh. The
    /// world calls this when the backend starts serving it, since another
    /// world's slots name other genomes.
    fn forget(&mut self);

    /// Notify the backend that `slots` were retired by deaths at tick commit
    /// (FL-043). Backends holding resident parameter shards mark matching
    /// `(slot, generation)` entries unoccupied and reclaim any shard with no
    /// live occupants remaining.
    fn release_slots(&mut self, _slots: &[ArenaSlot]) {}

    /// The device's time on the commands of the last
    /// [`ControllerBackend::evaluate`], in nanoseconds (FL-029): zero for a
    /// backend that runs on the host, and for a batch that dispatched
    /// nothing.
    fn last_device_nanoseconds(&self) -> u64 {
        0
    }

    /// Evaluate `jobs` in order. `outputs` becomes one entry per job: its
    /// raw linear outputs (length `job.controller.topology().raw_output_width()`,
    /// up to [`crate::MAX_CONTROLLER_OUTPUT_WIDTH`] = `80`), bit-identical to
    /// `job.controller.evaluate_raw(job.inputs, _)`, or `None` where the
    /// backend defers that job to the reference. Every job the reference
    /// refuses must be deferred.
    ///
    /// # Errors
    /// A device or allocation failure. The world then evaluates the tick on
    /// the reference and stops using the backend.
    fn evaluate(
        &mut self,
        jobs: &[ControllerJob<'_>],
        outputs: &mut Vec<Option<RawOutputs>>,
    ) -> Result<(), CoreError>;

    /// Whether this backend implements [`Self::evaluate_flat_f32`] natively
    /// without widening back through [`Self::evaluate`].
    fn supports_flat_f32(&self) -> bool {
        false
    }

    /// Evaluate `jobs` from a contiguous row-major F32 input buffer
    /// (`inputs_f32.len() == jobs.len() * input_width`) with per-job finiteness
    /// flags (`inputs_finite.len() == jobs.len()`), writing contiguous row-major
    /// F32 outputs and validity flags into `outputs`.
    ///
    /// Every job with `inputs_finite[i] == 0`, mismatched topology width, or
    /// any reference refusal must be deferred (`outputs.valid[i] = 0`).
    ///
    /// The default implementation widens F32 inputs losslessly to F64 and
    /// delegates to [`Self::evaluate`] so all existing backends remain
    /// compatible.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] or [`CoreError::Overflow`] for malformed
    /// slice lengths, or the backend's evaluation/allocation error (clearing
    /// `outputs` on failure).
    fn evaluate_flat_f32(
        &mut self,
        jobs: &[FlatControllerJob<'_>],
        input_width: usize,
        inputs_f32: &[f32],
        inputs_finite: &[u8],
        outputs: &mut FlatOutputs,
    ) -> Result<(), CoreError> {
        let expected_inputs = jobs
            .len()
            .checked_mul(input_width)
            .ok_or(CoreError::Overflow {
                quantity: "flat controller inputs",
            })?;
        if inputs_f32.len() != expected_inputs {
            outputs.clear();
            return Err(CoreError::LayoutMismatch {
                buffer: "flat controller inputs",
                detail: format!(
                    "expected {expected_inputs} f32 inputs ({} jobs x {input_width}), got {}",
                    jobs.len(),
                    inputs_f32.len()
                ),
            });
        }
        if inputs_finite.len() != jobs.len() {
            outputs.clear();
            return Err(CoreError::LayoutMismatch {
                buffer: "flat controller input flags",
                detail: format!(
                    "expected {} finite flags, got {}",
                    jobs.len(),
                    inputs_finite.len()
                ),
            });
        }
        let Some(first) = jobs.first() else {
            outputs.clear();
            return Ok(());
        };
        let output_width = first.controller.topology().raw_output_width();
        outputs.prepare(jobs.len(), output_width)?;

        let mut wide_inputs = Vec::with_capacity(expected_inputs);
        for &value in inputs_f32 {
            wide_inputs.push(f64::from(value));
        }
        let mut legacy_jobs = Vec::with_capacity(jobs.len());
        let mut legacy_indices = Vec::with_capacity(jobs.len());
        for (index, (job, &finite)) in jobs.iter().zip(inputs_finite.iter()).enumerate() {
            if finite == 0
                || job.controller.topology().input_width() != input_width
                || job.controller.topology().raw_output_width() != output_width
            {
                continue;
            }
            let start = index * input_width;
            legacy_jobs.push(ControllerJob {
                genome: job.genome,
                slot: job.slot,
                controller: job.controller,
                inputs: &wide_inputs[start..start + input_width],
            });
            legacy_indices.push(index);
        }
        if legacy_jobs.is_empty() {
            return Ok(());
        }
        let mut legacy_outputs = Vec::new();
        if let Err(error) = self.evaluate(&legacy_jobs, &mut legacy_outputs) {
            outputs.clear();
            return Err(error);
        }
        for (orig_index, maybe_out) in legacy_indices.into_iter().zip(legacy_outputs) {
            let Some(raw) = maybe_out else {
                continue;
            };
            let slice = raw.as_slice();
            if slice.len() != output_width {
                continue;
            }
            let start = orig_index * output_width;
            outputs.values[start..start + output_width].copy_from_slice(slice);
            outputs.valid[orig_index] = 1;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::{ControllerScratch, DenseTopology};
    use crate::weights::Weights;

    #[derive(Debug)]
    struct ScalarStubBackend;

    impl ControllerBackend for ScalarStubBackend {
        fn describe(&self) -> String {
            "scalar-stub".to_owned()
        }

        fn resident_bytes(&self) -> u64 {
            0
        }

        fn forget(&mut self) {}

        fn evaluate(
            &mut self,
            jobs: &[ControllerJob<'_>],
            outputs: &mut Vec<Option<RawOutputs>>,
        ) -> Result<(), CoreError> {
            outputs.clear();
            let mut scratch = ControllerScratch::default();
            for job in jobs {
                match job.controller.evaluate_raw(job.inputs, &mut scratch) {
                    Ok(raw) => outputs.push(Some(raw)),
                    Err(_) => outputs.push(None),
                }
            }
            Ok(())
        }
    }

    #[test]
    fn default_evaluate_flat_f32_matches_evaluate_and_defers_nonfinite() {
        let topology = DenseTopology::new(&[4]).expect("topology");
        let in_w = topology.input_width();
        let out_w = topology.raw_output_width();
        let params = vec![0.05_f32; topology.parameters()];
        let controller =
            ReferenceController::new(&topology, Weights::F32(&params), 4.0).expect("controller");

        let mut backend = ScalarStubBackend;
        assert!(!backend.supports_flat_f32());

        let jobs = [
            FlatControllerJob {
                genome: GenomeId::new(1),
                slot: ArenaSlot::new(0, 0, 1),
                controller,
            },
            FlatControllerJob {
                genome: GenomeId::new(2),
                slot: ArenaSlot::new(0, 1, 1),
                controller,
            },
            FlatControllerJob {
                genome: GenomeId::new(3),
                slot: ArenaSlot::new(0, 2, 1),
                controller,
            },
        ];
        let mut inputs_f32 = vec![0.0_f32; 3 * in_w];
        for k in 0..in_w {
            inputs_f32[k] = ((k as f32) * 0.01) - 0.5;
            inputs_f32[2 * in_w + k] = 0.5 - ((k as f32) * 0.01);
        }
        let inputs_finite = [1_u8, 0, 1];
        let mut flat = FlatOutputs::default();
        backend
            .evaluate_flat_f32(&jobs, in_w, &inputs_f32, &inputs_finite, &mut flat)
            .expect("flat eval");
        assert_eq!(flat.output_width, out_w);
        assert_eq!(flat.valid, [1, 0, 1]);
        assert!(flat.row(1).is_none());

        // Compare valid rows bit-for-bit against scalar reference.
        let mut scratch = ControllerScratch::default();
        for idx in [0_usize, 2] {
            let start = idx * in_w;
            let wide: Vec<f64> = inputs_f32[start..start + in_w]
                .iter()
                .map(|&v| f64::from(v))
                .collect();
            let expected = controller
                .evaluate_raw(&wide, &mut scratch)
                .expect("reference");
            let got = flat.row(idx).expect("valid row");
            assert_eq!(got.len(), expected.as_slice().len());
            for (g, e) in got.iter().zip(expected.as_slice()) {
                assert_eq!(g.to_bits(), e.to_bits());
            }
        }

        // Empty batch clears outputs cleanly; mismatched lengths return LayoutMismatch.
        backend
            .evaluate_flat_f32(&[], in_w, &[], &[], &mut flat)
            .expect("empty");
        assert_eq!(flat.output_width, 0);
        assert!(flat.values.is_empty());
        assert!(flat.valid.is_empty());

        let mismatch =
            backend.evaluate_flat_f32(&jobs, in_w, &inputs_f32[..in_w], &inputs_finite, &mut flat);
        assert!(matches!(mismatch, Err(CoreError::LayoutMismatch { .. })));
        assert!(flat.valid.is_empty());
    }
}
