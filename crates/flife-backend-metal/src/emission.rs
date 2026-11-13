//! FL-032 (`flife-kf8`), third stage: phase 6's emission on Metal (plan
//! §5.5, §8.2, §10.7). [`MetalEmission::settle`] runs, one thread per
//! organism that reaches emission, what flife-engine's reference runs there:
//! `EmissionPolicy::emit` with flife-field's `Deposits::push` (the payment
//! and offer checks, the direction's norm, `ExchangeStencil::at` and the
//! deposit split) and the budget's payment, then the exhaustion check after
//! emission. Every value, and every packet's cell and amount, is the
//! reference's, bit for bit (`tests/emission.rs` compares them with
//! `EmissionPolicy::emit` and what it staged).
//!
//! The caller stages each deposit's packets in identity order, as the
//! reference's `Deposits` does. As in [`crate::chemistry`], a thread that
//! meets an input the reference would refuse reports a status instead of an
//! outcome, and the call is refused, so the reference decides.
//!
//! Not yet here: the engine port. Each call copies every organism's state
//! to the device and the outcomes back.

use flife_core::CoreError;
use flife_field::FieldGrid;
use flife_field::state::CHANNELS;
use flife_neural::Emission;
use ft_kernel_metal::compute::{Grid, MathMode, Pipeline};

use crate::chemistry::{Slot, put, value, word};
use crate::owner::{Binding, DeviceOwner, Kernel, Limits, Purpose, metal};

/// The emission kernel source, with the software binary64 ahead of it.
pub const EMISSION_MSL: &str = concat!(
    include_str!("softfloat64.metal"),
    include_str!("emission.metal")
);

/// `emission.metal`'s layouts, in words.
const PARAMS: usize = 13;
const ORGANISM: usize = 9 + 2 * CHANNELS;
const DIRECTION: usize = 9;
const OUTCOME: usize = 27;
const PACKETS: usize = 15;
/// Words per row of [`MetalEmission::emission_ops`]'s results.
pub const EMISSION_OPS_WORDS: usize = 6;

/// Threads per threadgroup.
const THREADS: usize = 64;

/// One organism that reaches emission: alive after motion, so its budget
/// holds energy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EmissionOrganism {
    /// The budget's balance after motion, finite and positive.
    pub remaining: f64,
    /// Its position after motion.
    pub position: [f64; 2],
    /// Its decoded emission, if it emits.
    pub emission: Option<Emission>,
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
        &self.packets[..self.packet_count.min(4)]
    }
}

fn overflow() -> CoreError {
    CoreError::Overflow {
        quantity: "metal emission size",
    }
}

/// What a nonzero status in `emission.metal` means.
fn status_name(status: u32) -> &'static str {
    match status {
        1 => "a spend fraction outside [0, 1]",
        2 => "a payment or offer the deposit refuses",
        3 => "a direction outside the unit ball",
        4 => "a position that is not finite",
        _ => "an unknown status",
    }
}

/// Phase 6's emission on Metal; see the module documentation.
pub struct MetalEmission {
    owner: DeviceOwner,
    settle: Pipeline,
    ops: Pipeline,
    params: Slot,
    organisms: Slot,
    out: Slot,
    host: Vec<u32>,
}

impl std::fmt::Debug for MetalEmission {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MetalEmission")
            .field("device", &self.owner.device_name())
            .finish_non_exhaustive()
    }
}

impl MetalEmission {
    /// Open the default Metal device and compile the emission kernels.
    ///
    /// # Errors
    /// [`CoreError::MissingCapability`] without a Metal device (always, off
    /// macOS); [`CoreError::StateConflict`] if the kernels do not compile.
    pub fn open() -> Result<Self, CoreError> {
        Self::with_owner(DeviceOwner::open(Limits::UNBOUNDED)?)
    }

    /// The emission kernels on `owner`'s device; their buffers are
    /// `owner`'s.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] if the kernels do not compile.
    pub fn with_owner(owner: DeviceOwner) -> Result<Self, CoreError> {
        let library = owner.library(EMISSION_MSL, MathMode::Safe)?;
        let pipeline = |name: &str| library.pipeline(name).map_err(metal);
        Ok(Self {
            settle: pipeline("emission_settle")?,
            ops: pipeline("emission_ops")?,
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

    /// Settle every organism's emission on `grid` with transfer
    /// `efficiency`: `out[i]` is `organisms[i]`'s, as the reference computes
    /// it.
    ///
    /// # Errors
    /// - [`CoreError::NumericalInvalid`] for an efficiency outside `[0, 1]`.
    /// - [`CoreError::StateConflict`] for an organism whose balance is not
    ///   finite and positive (one that does not reach emission); and, after
    ///   the command, when any organism meets an input the reference refuses:
    ///   the caller settles that tick on the reference. Also if the dispatch
    ///   fails.
    /// - [`CoreError::Overflow`] for a call too large to describe in `u32`,
    ///   or a grid with more than `u32::MAX` cells.
    ///
    /// On error `out` is left empty.
    pub fn settle(
        &mut self,
        grid: &FieldGrid,
        efficiency: f64,
        organisms: &[EmissionOrganism],
        out: &mut Vec<EmissionOutcome>,
    ) -> Result<(), CoreError> {
        out.clear();
        if !(0.0..=1.0).contains(&efficiency) {
            return Err(CoreError::NumericalInvalid {
                quantity: "emission.transfer_efficiency",
                detail: format!("{efficiency} is outside [0, 1]"),
            });
        }
        if grid.cell_count() > u64::from(u32::MAX) {
            return Err(overflow());
        }
        if organisms.is_empty() {
            return Ok(());
        }
        let mut params = [0_u32; PARAMS];
        params[0] = word(organisms.len())?;
        params[1] = grid.cells_x();
        params[2] = grid.cells_y();
        for (at, constant) in [
            (3, efficiency + 0.0),
            (5, grid.world_width()),
            (7, grid.world_height()),
            (9, grid.cell_width()),
            (11, grid.cell_height()),
        ] {
            put(&mut params, at, constant);
        }

        let host = &mut self.host;
        host.clear();
        host.try_reserve(organisms.len().saturating_mul(ORGANISM))
            .map_err(|_| overflow())?;
        for (index, organism) in organisms.iter().enumerate() {
            if !(organism.remaining.is_finite() && organism.remaining > 0.0) {
                return Err(CoreError::StateConflict {
                    operation: "metal emission",
                    detail: format!(
                        "organism {index} of the call holds {:e}; only an organism alive after \
                         motion reaches emission",
                        organism.remaining
                    ),
                });
            }
            let mut row = [0_u32; ORGANISM];
            put(&mut row, 0, organism.remaining);
            if let Some(emission) = &organism.emission {
                row[2] = 1;
                put(&mut row, 3, emission.spend_fraction);
                for (channel, &component) in emission.direction.iter().enumerate() {
                    put(&mut row, DIRECTION + 2 * channel, component);
                }
            }
            put(&mut row, 5, organism.position[0]);
            put(&mut row, 7, organism.position[1]);
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
            if row[0] != 0 || row[3] > 4 {
                out.clear();
                return Err(CoreError::StateConflict {
                    operation: "metal emission",
                    detail: format!(
                        "organism {index} of the call met {} ({} packets); the reference \
                         settles this tick",
                        status_name(row[0]),
                        row[3]
                    ),
                });
            }
            let mut packets = [(0_usize, 0.0); 4];
            for (k, packet) in packets.iter_mut().enumerate() {
                let at = PACKETS + 3 * k;
                *packet = (row[at] as usize, value(&row[at + 1..at + 3]));
            }
            out.push(EmissionOutcome {
                available: value(&row[4..6]),
                requested: value(&row[6..8]),
                paid: value(&row[8..10]),
                offered: value(&row[10..12]),
                staged: row[2] != 0,
                packets,
                packet_count: row[3] as usize,
                remaining: value(&row[12..14]),
                exhausted: row[14] != 0,
            });
        }
        Ok(())
    }

    /// The kernel's added arithmetic on operands `a` as bit patterns. Each
    /// result row holds, as `(low, high)` word pairs: `floor(a)`, `floor(a)
    /// as i64` (zero unless `a` is finite and its floor below 2^62 in
    /// magnitude), and `add_up(a, a / 3)`. `operands.len()` must be a
    /// positive multiple of 64.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] for another length;
    /// [`CoreError::StateConflict`] if the dispatch fails.
    pub fn emission_ops(
        &self,
        operands: &[u64],
        out: &mut Vec<[u32; EMISSION_OPS_WORDS]>,
    ) -> Result<(), CoreError> {
        if operands.is_empty() || !operands.len().is_multiple_of(64) {
            return Err(CoreError::LayoutMismatch {
                buffer: "emission operands",
                detail: format!("{} operands", operands.len()),
            });
        }
        let words: Vec<u32> = operands
            .iter()
            .flat_map(|&v| [v as u32, (v >> 32) as u32])
            .collect();
        let input = self.owner.upload_owned(&words)?;
        let results = self
            .owner
            .zeroed_owned::<u32>(operands.len() * EMISSION_OPS_WORDS)?;
        self.owner.submit(
            &self.ops,
            &[input.buffer().reads(), results.buffer().writes()],
            Grid::linear(operands.len() / 64, 64),
        )?;
        let mut flat = vec![0_u32; operands.len() * EMISSION_OPS_WORDS];
        self.owner.read_back(results.buffer(), &mut flat)?;
        out.clear();
        out.extend(flat.as_chunks::<EMISSION_OPS_WORDS>().0.iter().copied());
        Ok(())
    }
}
