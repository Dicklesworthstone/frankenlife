//! FL-032 (`flife-kf8`), second stage: phase 6's maintenance and motion on
//! Metal (plan §5.4, §5.6, §8.2, §10.7). [`MetalMotion::settle`] runs, one thread
//! per organism, the stretch of flife-engine's reference that follows
//! chemistry:
//! - the exhaustion check after chemistry;
//! - the maintenance bill (basal, sensor and controller costs), its
//!   payment and its split, then the exhaustion check after it;
//! - the uptake ratio, the motion-response norm check and the clamped
//!   acceleration;
//! - the drag-decayed, speed-clamped proposal and its payment, scaled down
//!   when unaffordable;
//! - the displacement on the torus, then the exhaustion check after motion.
//!
//! Every value is the reference's, bit for bit (`tests/motion.rs` compares
//! them with flife-engine's `MaintenanceCosts`, `Vitality`, `uptake_ratio`,
//! `MotionResponse` and `Motion`).
//!
//! As in [`crate::chemistry`], a thread that meets an input the reference
//! would refuse, or a value softfloat64 cannot carry (an infinity, which it
//! turns into a NaN, or a clamp that needs more than 64 shrinks), reports a
//! status instead of an outcome, and the call is refused: the caller then
//! settles on the reference.
//!
//! [`crate::settle`] serves this stage to the engine. Each call copies every
//! organism's state to the device and the outcomes back.

use flife_core::CoreError;
use flife_field::settle::{
    ExhaustedAt, MOTION_TERMS, MaintenanceOutcome, MotionConstants, MotionOrganism, MotionOutcome,
    MovementOutcome,
};
use ft_kernel_metal::compute::{Grid, MathMode, Pipeline};

use crate::chemistry::{Slot, put, value, word};
use crate::owner::{Binding, DeviceOwner, Kernel, Limits, Purpose, metal};

/// The motion kernel source, with the software binary64 ahead of it.
pub const MOTION_MSL: &str = concat!(
    include_str!("softfloat64.metal"),
    include_str!("motion.metal")
);

/// `motion.metal`'s layouts, in words.
const PARAMS: usize = 29;
const ORGANISM: usize = 17 + 6 * MOTION_TERMS;
const RESPONSE: usize = 17;
const GRADIENTS: usize = RESPONSE + 2 * MOTION_TERMS;
const OUTCOME: usize = 32;
/// Words per row of [`MetalMotion::motion_ops`]'s results.
pub const MOTION_OPS_WORDS: usize = 8;

/// Threads per threadgroup.
const THREADS: usize = 64;

fn overflow() -> CoreError {
    CoreError::Overflow {
        quantity: "metal motion size",
    }
}

/// What a nonzero status in `motion.metal` means.
fn status_name(status: u32) -> &'static str {
    match status {
        1 => "a sensing scale that is not a positive length",
        2 => "a bill share that is not a finite nonnegative energy",
        3 => "an absorbed uptake or uptake ratio the device cannot form",
        4 => "a non-finite motion response or one past the norm limit",
        5 => "a non-finite sensor gradient",
        6 => "a non-finite velocity",
        7 => "a clamp the device cannot carry",
        8 => "a motion cost that is not finite",
        9 => "a displaced position that is not finite",
        _ => "an unknown status",
    }
}

/// Phase 6's maintenance and motion on Metal; see the module documentation.
pub struct MetalMotion {
    owner: DeviceOwner,
    settle: Pipeline,
    ops: Pipeline,
    params: Slot,
    organisms: Slot,
    out: Slot,
    host: Vec<u32>,
}

impl std::fmt::Debug for MetalMotion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MetalMotion")
            .field("device", &self.owner.device_name())
            .finish_non_exhaustive()
    }
}

impl MetalMotion {
    /// Open the default Metal device and compile the motion kernels.
    ///
    /// # Errors
    /// [`CoreError::MissingCapability`] without a Metal device (always, off
    /// macOS); [`CoreError::StateConflict`] if the kernels do not compile.
    pub fn open() -> Result<Self, CoreError> {
        Self::with_owner(DeviceOwner::open(Limits::UNBOUNDED)?)
    }

    /// The motion kernels on `owner`'s device; their buffers are `owner`'s.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] if the kernels do not compile.
    pub fn with_owner(owner: DeviceOwner) -> Result<Self, CoreError> {
        let library = owner.library(MOTION_MSL, MathMode::Safe)?;
        let pipeline = |name: &str| library.pipeline(name).map_err(metal);
        Ok(Self {
            settle: pipeline("motion_settle")?,
            ops: pipeline("motion_ops")?,
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

    /// Settle every organism's maintenance and motion: `out[i]` is
    /// `organisms[i]`'s, as the reference computes it.
    ///
    /// # Errors
    /// - [`CoreError::NumericalInvalid`] for constants outside their ranges.
    /// - [`CoreError::StateConflict`] when any organism meets an input the
    ///   reference refuses or a value the device cannot carry: the caller
    ///   settles that tick on the reference. Also if the dispatch fails.
    /// - [`CoreError::Overflow`] for a call too large to describe in `u32`.
    ///
    /// On error `out` is left empty.
    pub fn settle(
        &mut self,
        constants: &MotionConstants,
        organisms: &[MotionOrganism],
        out: &mut Vec<MotionOutcome>,
    ) -> Result<(), CoreError> {
        out.clear();
        check_constants(constants)?;
        if organisms.is_empty() {
            return Ok(());
        }
        let mut params = [0_u32; PARAMS];
        params[0] = word(organisms.len())?;
        for (at, constant) in [
            (1, constants.step_seconds),
            (3, constants.basal),
            (5, constants.sensor),
            (7, constants.sensor_unit_scale),
            (9, constants.neural),
            (11, constants.decay),
            (13, constants.force_coefficient),
            (15, constants.maximum_speed),
            (17, constants.maximum_acceleration),
            (19, constants.cost),
            (21, constants.response_norm_limit),
            (23, constants.uptake_capacity),
            (25, constants.world_width),
            (27, constants.world_height),
        ] {
            put(&mut params, at, constant);
        }

        let host = &mut self.host;
        host.clear();
        host.try_reserve(organisms.len().saturating_mul(ORGANISM))
            .map_err(|_| overflow())?;
        for organism in organisms {
            let mut row = [0_u32; ORGANISM];
            put(&mut row, 0, organism.remaining);
            row[2] = u32::from(organism.exhausted);
            row[3] = u32::from(organism.evaluated);
            row[4] = organism.parameters;
            put(&mut row, 5, organism.sensing_scale);
            put(&mut row, 7, organism.absorbed);
            put(&mut row, 9, organism.velocity[0]);
            put(&mut row, 11, organism.velocity[1]);
            put(&mut row, 13, organism.position[0]);
            put(&mut row, 15, organism.position[1]);
            for (term, (&coefficient, gradient)) in organism
                .response
                .iter()
                .zip(&organism.gradients)
                .enumerate()
            {
                put(&mut row, RESPONSE + 2 * term, coefficient);
                put(&mut row, GRADIENTS + 4 * term, gradient[0]);
                put(&mut row, GRADIENTS + 4 * term + 2, gradient[1]);
            }
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
        for (index, (row, organism)) in host
            .as_chunks::<OUTCOME>()
            .0
            .iter()
            .zip(organisms)
            .enumerate()
        {
            let died = match (row[0], row[1]) {
                (0, 0) => None,
                (0, 1) => Some(ExhaustedAt::Chemistry),
                (0, 2) => Some(ExhaustedAt::Maintenance),
                (0, 3) => Some(ExhaustedAt::Motion),
                (status, died) => {
                    out.clear();
                    return Err(CoreError::StateConflict {
                        operation: "metal motion",
                        detail: format!(
                            "organism {index} of the call met {} (stage code {died}); the \
                             reference settles this tick",
                            status_name(status)
                        ),
                    });
                }
            };
            let maintenance = (died != Some(ExhaustedAt::Chemistry)).then(|| MaintenanceOutcome {
                requested: value(&row[2..4]),
                paid: value(&row[4..6]),
                basal: value(&row[6..8]),
                sensor: value(&row[8..10]),
                controller: value(&row[10..12]),
            });
            let movement =
                matches!(died, None | Some(ExhaustedAt::Motion)).then(|| MovementOutcome {
                    proposed: [value(&row[12..14]), value(&row[14..16])],
                    velocity: [value(&row[16..18]), value(&row[18..20])],
                    requested: value(&row[20..22]),
                    paid: value(&row[22..24]),
                    scaled: row[24] != 0,
                });
            out.push(MotionOutcome {
                maintenance,
                movement,
                velocity: movement.map_or(organism.velocity, |moved| moved.velocity),
                position: [value(&row[25..27]), value(&row[27..29])],
                remaining: value(&row[29..31]),
                exhausted: row[31] != 0,
                died,
            });
        }
        Ok(())
    }

    /// The kernel's arithmetic on operand pairs `(a, b)` as bit patterns.
    /// Each result row holds, as `(low, high)` word pairs: `sqrt(a)`,
    /// `a % b`, `a.rem_euclid(b)`, and flife-field's `wrap_coordinate(a,
    /// |b|)` (a NaN where it refuses: a non-finite `a` or `b`, or a zero
    /// `b`). `pairs.len()` must be a positive multiple of 64.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] for another length;
    /// [`CoreError::StateConflict`] if the dispatch fails.
    pub fn motion_ops(
        &self,
        pairs: &[[u64; 2]],
        out: &mut Vec<[u32; MOTION_OPS_WORDS]>,
    ) -> Result<(), CoreError> {
        if pairs.is_empty() || !pairs.len().is_multiple_of(64) {
            return Err(CoreError::LayoutMismatch {
                buffer: "motion operands",
                detail: format!("{} pairs", pairs.len()),
            });
        }
        let words: Vec<u32> = pairs
            .iter()
            .flat_map(|pair| pair.iter().flat_map(|&v| [v as u32, (v >> 32) as u32]))
            .collect();
        let operands = self.owner.upload_owned(&words)?;
        let results = self
            .owner
            .zeroed_owned::<u32>(pairs.len() * MOTION_OPS_WORDS)?;
        self.owner.submit(
            &self.ops,
            &[operands.buffer().reads(), results.buffer().writes()],
            Grid::linear(pairs.len() / 64, 64),
        )?;
        let mut flat = vec![0_u32; pairs.len() * MOTION_OPS_WORDS];
        self.owner.read_back(results.buffer(), &mut flat)?;
        out.clear();
        out.extend(flat.as_chunks::<MOTION_OPS_WORDS>().0.iter().copied());
        Ok(())
    }
}

/// The ranges flife-engine's `MaintenanceCosts::new`, `Motion::new`,
/// `uptake_ratio` and `FieldGrid::new` admit, which the kernel takes as
/// given.
fn check_constants(constants: &MotionConstants) -> Result<(), CoreError> {
    let MotionConstants {
        step_seconds,
        basal,
        sensor,
        sensor_unit_scale,
        neural,
        decay,
        force_coefficient,
        maximum_speed,
        maximum_acceleration,
        cost,
        response_norm_limit,
        uptake_capacity,
        world_width,
        world_height,
    } = *constants;
    let checks = [
        ("timestep seconds", step_seconds, step_seconds > 0.0),
        ("costs.basal_energy_per_second", basal, basal >= 0.0),
        (
            "costs.sensor_energy_per_second_at_unit_scale",
            sensor,
            sensor >= 0.0,
        ),
        (
            "costs.sensor_unit_scale_world_units",
            sensor_unit_scale,
            sensor_unit_scale > 0.0,
        ),
        (
            "costs.neural_energy_per_parameter_evaluation",
            neural,
            neural >= 0.0,
        ),
        ("drag decay", decay, (0.0..=1.0).contains(&decay)),
        (
            "drag force coefficient",
            force_coefficient,
            force_coefficient >= 0.0,
        ),
        (
            "chemistry.maximum_speed",
            maximum_speed,
            maximum_speed > 0.0,
        ),
        (
            "chemistry.maximum_acceleration",
            maximum_acceleration,
            maximum_acceleration > 0.0,
        ),
        (
            "costs.motion_energy_per_speed_squared_second",
            cost,
            cost >= 0.0,
        ),
        (
            "chemistry.motion_response_norm_limit",
            response_norm_limit,
            response_norm_limit > 0.0,
        ),
        (
            "chemistry.uptake_capacity_per_second",
            uptake_capacity,
            uptake_capacity >= 0.0,
        ),
        ("world width", world_width, world_width > 0.0),
        ("world height", world_height, world_height > 0.0),
    ];
    for (quantity, value, admissible) in checks {
        if !value.is_finite() || !admissible {
            return Err(CoreError::NumericalInvalid {
                quantity,
                detail: format!("{value} is out of range"),
            });
        }
    }
    Ok(())
}
