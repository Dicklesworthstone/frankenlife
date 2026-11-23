//! FL-032: phase 9 of the tick on Metal (plan §8.1, §8.3, §11.1): the
//! uptake grants' debit, then the emission and recycling deposits reduced
//! into the tick's candidate field, bit-identical to
//! [`flife_field::reduce_field_on`]. [`MetalField`](crate::MetalField) runs
//! it as its [`FieldBackend::reduce`](flife_field::FieldBackend::reduce).
//!
//! The work is split by what each side does best:
//! - the host runs flife-field's own checks and grouping
//!   ([`Grants::debit`], [`Deposits::reduction`]) and the summaries' sums in
//!   cell order ([`DepositReduction::summarize`]);
//! - the device runs every cell's arithmetic, in `reduce.metal` on
//!   `softfloat64.metal`'s binary64: each debited cell's energy and
//!   moments, then each set's cells, one thread per cell, its packets in
//!   (cell, identity) order.
//!
//! One reduction is one command, on the device's copy of the field. The
//! host's field is written only once the command, its fault word and every
//! sum have succeeded, and the grants and deposits are marked applied only
//! then: a failure leaves all of them as they were (plan §8.3), and the
//! world reduces the tick on the reference.

use flife_core::CoreError;
use flife_field::grid::{CarrierLayout, DUAL_CARRIER_PLANES, FIELD_PLANES};
use flife_field::{DepositReduction, DepositSummary, Deposits, FieldState, Grants};
use ft_kernel_metal::compute::{Grid, MathMode, Pipeline};

use crate::owner::{Binding, DeviceOwner, Kernel, Owned, Purpose, metal};

/// The phase 9 kernel source, with the software binary64 ahead of it.
pub const REDUCE_MSL: &str = concat!(
    include_str!("softfloat64.metal"),
    include_str!("reduce.metal")
);

/// `reduce.metal`'s layouts, in words.
const DEBIT: usize = 3;
const DEBIT_PARAMS: usize = 3;
const PACKET: usize = 3;
const DIRECTION: usize = 64;
const DEPOSIT_PARAMS: usize = 5;

/// Threads per threadgroup.
const THREADS: usize = 64;

fn overflow() -> CoreError {
    CoreError::Overflow {
        quantity: "metal reduction size",
    }
}

fn word(value: usize) -> Result<u32, CoreError> {
    u32::try_from(value).map_err(|_| overflow())
}

fn words(value: f64) -> [u32; 2] {
    let bits = value.to_bits();
    [(bits & 0xFFFF_FFFF) as u32, (bits >> 32) as u32]
}

/// A buffer of words that grows to what a reduction needs.
#[derive(Debug)]
struct Growing {
    buffer: Owned<u32>,
    len: usize,
}

impl Growing {
    fn new(owner: &DeviceOwner) -> Result<Self, CoreError> {
        Ok(Self {
            buffer: owner.zeroed_owned(1)?,
            len: 1,
        })
    }

    /// At least `needed` words (one when none): replaced by a buffer twice
    /// as large, or as large as needed, when it is short. The old one
    /// retires when dropped.
    fn room(&mut self, owner: &DeviceOwner, needed: usize) -> Result<(), CoreError> {
        let needed = needed.max(1);
        if self.len < needed {
            let len = needed.max(self.len.saturating_mul(2));
            self.buffer = owner.zeroed_owned(len)?;
            self.len = len;
        }
        Ok(())
    }
}

/// One deposit set's device buffers.
#[derive(Debug)]
struct Set {
    offsets: Owned<u32>,
    packets: Growing,
    directions: Growing,
    params: Owned<u32>,
    received: Owned<u32>,
}

impl Set {
    fn new(owner: &DeviceOwner, cells: usize) -> Result<Self, CoreError> {
        Ok(Self {
            offsets: owner.zeroed_owned(cells.checked_add(1).ok_or_else(overflow)?)?,
            packets: Growing::new(owner)?,
            directions: Growing::new(owner)?,
            params: owner.zeroed_owned(DEPOSIT_PARAMS)?,
            received: owner.zeroed_owned(cells.checked_mul(2).ok_or_else(overflow)?)?,
        })
    }
}

/// The device buffers for one grid and plane count (`33` single-carrier or
/// `34` dual-carrier). `trail_set` is lazily allocated only for 34-plane
/// dual-carrier reductions (0 extra VRAM on 33-plane).
#[derive(Debug)]
struct Buffers {
    cells: usize,
    planes: usize,
    field: Owned<f32>,
    debits: Growing,
    debit_params: Owned<u32>,
    sets: [Set; 2],
    trail_set: Option<Set>,
    fault: Owned<u32>,
}

/// One set's words for the device, and what its cells received.
#[derive(Debug, Default)]
struct HostSet {
    packets: Vec<u32>,
    directions: Vec<u32>,
    received: Vec<f64>,
}

/// What the host prepares and reads back each reduction.
#[derive(Debug, Default)]
struct Host {
    debits: Vec<u32>,
    sets: [HostSet; 2],
    trail_set: HostSet,
    words: Vec<u32>,
}

/// Phase 9's kernels and their buffers on one owner's device.
pub(crate) struct Reduce {
    debit: Pipeline,
    deposits: Pipeline,
    buffers: Option<Buffers>,
    host: Host,
}

impl std::fmt::Debug for Reduce {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Reduce")
            .field("buffers", &self.buffers)
            .finish_non_exhaustive()
    }
}

impl Reduce {
    /// The kernels compiled on `owner`'s device.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] if they do not compile.
    pub(crate) fn new(owner: &DeviceOwner) -> Result<Self, CoreError> {
        let library = owner.library(REDUCE_MSL, MathMode::Safe)?;
        let pipeline = |name: &str| library.pipeline(name).map_err(metal);
        Ok(Self {
            debit: pipeline("grants_debit")?,
            deposits: pipeline("deposits_reduce")?,
            buffers: None,
            host: Host::default(),
        })
    }

    /// Buffers for `cells` cells and `planes` planes (`33` or `34`), allocated
    /// on first use and again when either changes.
    fn ensure_buffers(
        &mut self,
        owner: &DeviceOwner,
        cells: usize,
        planes: usize,
        need_trail_set: bool,
    ) -> Result<(), CoreError> {
        let current = self
            .buffers
            .as_ref()
            .is_some_and(|buffers| buffers.cells == cells && buffers.planes == planes);
        if !current {
            self.buffers = None;
            let values = cells.checked_mul(planes).ok_or_else(overflow)?;
            self.buffers = Some(Buffers {
                cells,
                planes,
                field: owner.zeroed_owned(values)?,
                debits: Growing::new(owner)?,
                debit_params: owner.zeroed_owned(DEBIT_PARAMS)?,
                sets: [Set::new(owner, cells)?, Set::new(owner, cells)?],
                trail_set: None,
                fault: owner.zeroed_owned(1)?,
            });
        }
        if need_trail_set
            && let Some(buffers) = self.buffers.as_mut()
            && buffers.trail_set.is_none()
        {
            buffers.trail_set = Some(Set::new(owner, cells)?);
        }
        Ok(())
    }

    /// [`flife_field::reduce_field_on`] on the device (33-plane single-carrier
    /// or 34-plane Carrier 0, leaving Carrier 1 untouched); see the module
    /// documentation.
    ///
    /// # Errors
    /// As [`flife_field::reduce_field_on`]'s checks;
    /// [`CoreError::StateConflict`] for debits not in strictly ascending
    /// cell order; [`CoreError::NumericalInvalid`] for a non-finite value a
    /// kernel met or would write; the device's errors. Nothing is written
    /// or marked applied on error.
    #[expect(
        clippy::too_many_lines,
        reason = "one reduction's checks, words, command and read-back, in order"
    )]
    pub(crate) fn run(
        &mut self,
        owner: &DeviceOwner,
        field: &mut FieldState,
        grants: &mut Grants,
        deposits: [&mut Deposits; 2],
    ) -> Result<[DepositSummary; 2], CoreError> {
        let plane_count = field.planes();
        if plane_count != FIELD_PLANES && plane_count != DUAL_CARRIER_PLANES {
            return Err(CoreError::LayoutMismatch {
                buffer: "metal field reduction",
                detail: format!(
                    "unsupported plane count {plane_count} (expected {FIELD_PLANES} or {DUAL_CARRIER_PLANES})"
                ),
            });
        }
        let planes = usize::try_from(plane_count).map_err(|_| overflow())?;
        let carrier0 = field.carrier0();

        // flife-field's own checks and grouping, nothing applied yet.
        let debit = grants.debit(field)?;
        let [emissions, recycling] = deposits;
        let reductions = [
            emissions.reduction_for_carrier(field, carrier0)?,
            recycling.reduction_for_carrier(field, carrier0)?,
        ];
        let cells = usize::try_from(field.grid().cell_count()).map_err(|_| overflow())?;
        let values = field.as_slice().len();

        // The words: each debit that granted energy, then each set's
        // grouped packets and its deposits' directions.
        let host = &mut self.host;
        pack_debits(debit.debits(), cells, &mut host.debits)?;
        for (reduction, set) in reductions.iter().zip(&mut host.sets) {
            set_words(reduction, set, cells)?;
        }
        let debits = host.debits.len() / DEBIT;
        let items = |set: &HostSet| set.packets.len() / PACKET;

        self.ensure_buffers(owner, cells, planes, false)?;
        let Self {
            debit: debit_pipeline,
            deposits: deposits_pipeline,
            buffers,
            host,
        } = self;
        let buffers = buffers.as_mut().ok_or_else(overflow)?;
        buffers.debits.room(owner, host.debits.len())?;
        for (set, words) in buffers.sets.iter_mut().zip(&host.sets) {
            set.packets.room(owner, words.packets.len())?;
            set.directions.room(owner, words.directions.len())?;
        }

        // The device's copy of the field and every word, then one command.
        let mut kernels = Vec::with_capacity(3);
        let debit_bindings = [
            buffers.field.buffer().writes(),
            buffers.debits.buffer.buffer().reads(),
            buffers.debit_params.buffer().reads(),
            buffers.fault.buffer().writes(),
        ];
        let set_bindings = buffers.sets.each_ref().map(|set| {
            [
                buffers.field.buffer().writes(),
                set.offsets.buffer().reads(),
                set.packets.buffer.buffer().reads(),
                set.directions.buffer.buffer().reads(),
                set.params.buffer().reads(),
                set.received.buffer().writes(),
                buffers.fault.buffer().writes(),
            ]
        });
        if debits > 0 {
            kernels.push(Kernel {
                pipeline: debit_pipeline,
                bindings: &debit_bindings,
                grid: Grid::linear(debits.div_ceil(THREADS), THREADS),
            });
        }
        for (words, bindings) in host.sets.iter().zip(&set_bindings) {
            if items(words) > 0 {
                kernels.push(Kernel {
                    pipeline: deposits_pipeline,
                    bindings,
                    grid: Grid::linear(cells.div_ceil(THREADS), THREADS),
                });
            }
        }
        if !kernels.is_empty() {
            owner
                .write(buffers.field.buffer())?
                .write(0, field.as_slice())?;
            if debits > 0 {
                owner
                    .write(buffers.debits.buffer.buffer())?
                    .write(0, &host.debits)?;
                owner
                    .write(buffers.debit_params.buffer())?
                    .write(0, &[word(debits)?, word(cells)?, carrier0.moment_count])?;
            }
            for ((words, set), reduction) in host.sets.iter().zip(&buffers.sets).zip(&reductions) {
                if items(words) == 0 {
                    continue;
                }
                upload_set(owner, set, words, reduction, cells, carrier0)?;
            }
            owner.write(buffers.fault.buffer())?.write(0, &[0])?;
            owner.submit_batch(&kernels)?;
            check_reduce_fault(owner, &buffers.fault)?;
        }

        // What each cell received, then the summaries, before anything is
        // written: the host's sums in cell order.
        for (words, set) in host.sets.iter_mut().zip(&buffers.sets) {
            read_received(owner, set, words, cells, &mut host.words)?;
        }
        let [first, second] = reductions;
        let summaries = [
            first.summarize(&host.sets[0].received)?,
            second.summarize(&host.sets[1].received)?,
        ];

        // The field, in one copy once its length is known to fit; then the
        // grants and deposits are applied.
        if !kernels.is_empty() {
            let lease = owner.read(buffers.field.buffer(), Purpose::Readback)?;
            if lease.elements() < values {
                return Err(overflow());
            }
            lease.read(field.as_mut_slice())?;
        }
        debit.finish();
        first.finish();
        second.finish();
        Ok(summaries)
    }

    /// [`flife_field::reduce_dual_carrier_field_on`] on the device (`V2-15`
    /// 34-plane dual-carrier Phase 9): debits `grants` strictly from Carrier 0
    /// `(0, 1, 24)` (leaving Carrier 1 bit-for-bit untouched), reduces
    /// `carrier0_deposits` (`[emissions_0, recycling]`) onto Carrier 0
    /// `(0, 1, 24)`, and reduces `trail_deposits` onto Carrier 1 `(25, 26, 8)`
    /// in one Metal command buffer.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] if `field` is not 34-plane dual-carrier;
    /// otherwise as [`Reduce::run`]. Nothing is written or marked applied on
    /// error.
    #[expect(
        clippy::too_many_lines,
        reason = "atomic 34-plane dual-carrier reduction: grants + 3 deposit sets in one command"
    )]
    pub(crate) fn run_dual_carrier(
        &mut self,
        owner: &DeviceOwner,
        field: &mut FieldState,
        grants: &mut Grants,
        carrier0_deposits: [&mut Deposits; 2],
        trail_deposits: &mut Deposits,
    ) -> Result<([DepositSummary; 2], DepositSummary), CoreError> {
        let Some(carrier1) = field.carrier1() else {
            return Err(CoreError::LayoutMismatch {
                buffer: "metal dual-carrier field reduction",
                detail: "reduce_dual_carrier requires a 34-plane dual-carrier field".into(),
            });
        };
        let carrier0 = field.carrier0();
        let planes = usize::try_from(field.planes()).map_err(|_| overflow())?;

        let debit = grants.debit(field)?;
        let [emissions_0, recycling] = carrier0_deposits;
        let red0 = emissions_0.reduction_for_carrier(field, carrier0)?;
        let red1 = recycling.reduction_for_carrier(field, carrier0)?;
        let red_trail = trail_deposits.reduction_for_carrier(field, carrier1)?;
        let cells = usize::try_from(field.grid().cell_count()).map_err(|_| overflow())?;
        let values = field.as_slice().len();

        let host = &mut self.host;
        pack_debits(debit.debits(), cells, &mut host.debits)?;
        set_words(&red0, &mut host.sets[0], cells)?;
        set_words(&red1, &mut host.sets[1], cells)?;
        set_words(&red_trail, &mut host.trail_set, cells)?;
        let debits = host.debits.len() / DEBIT;
        let items = |set: &HostSet| set.packets.len() / PACKET;

        self.ensure_buffers(owner, cells, planes, true)?;
        let Self {
            debit: debit_pipeline,
            deposits: deposits_pipeline,
            buffers,
            host,
        } = self;
        let buffers = buffers.as_mut().ok_or_else(overflow)?;
        buffers.debits.room(owner, host.debits.len())?;
        for (set, words) in buffers.sets.iter_mut().zip(&host.sets) {
            set.packets.room(owner, words.packets.len())?;
            set.directions.room(owner, words.directions.len())?;
        }
        let trail_buf = buffers.trail_set.as_mut().ok_or_else(overflow)?;
        trail_buf
            .packets
            .room(owner, host.trail_set.packets.len())?;
        trail_buf
            .directions
            .room(owner, host.trail_set.directions.len())?;

        let trail_buf = buffers.trail_set.as_ref().ok_or_else(overflow)?;
        let debit_bindings: [Binding; 4] = [
            buffers.field.buffer().writes(),
            buffers.debits.buffer.buffer().reads(),
            buffers.debit_params.buffer().reads(),
            buffers.fault.buffer().writes(),
        ];
        let set_bindings: [[Binding; 7]; 2] = buffers.sets.each_ref().map(|set| {
            [
                buffers.field.buffer().writes(),
                set.offsets.buffer().reads(),
                set.packets.buffer.buffer().reads(),
                set.directions.buffer.buffer().reads(),
                set.params.buffer().reads(),
                set.received.buffer().writes(),
                buffers.fault.buffer().writes(),
            ]
        });
        let trail_bindings: [Binding; 7] = [
            buffers.field.buffer().writes(),
            trail_buf.offsets.buffer().reads(),
            trail_buf.packets.buffer.buffer().reads(),
            trail_buf.directions.buffer.buffer().reads(),
            trail_buf.params.buffer().reads(),
            trail_buf.received.buffer().writes(),
            buffers.fault.buffer().writes(),
        ];

        let mut kernels = Vec::with_capacity(4);
        if debits > 0 {
            kernels.push(Kernel {
                pipeline: debit_pipeline,
                bindings: &debit_bindings,
                grid: Grid::linear(debits.div_ceil(THREADS), THREADS),
            });
        }
        for (words, bindings) in host.sets.iter().zip(&set_bindings) {
            if items(words) > 0 {
                kernels.push(Kernel {
                    pipeline: deposits_pipeline,
                    bindings,
                    grid: Grid::linear(cells.div_ceil(THREADS), THREADS),
                });
            }
        }
        if items(&host.trail_set) > 0 {
            kernels.push(Kernel {
                pipeline: deposits_pipeline,
                bindings: &trail_bindings,
                grid: Grid::linear(cells.div_ceil(THREADS), THREADS),
            });
        }

        if !kernels.is_empty() {
            owner
                .write(buffers.field.buffer())?
                .write(0, field.as_slice())?;
            if debits > 0 {
                owner
                    .write(buffers.debits.buffer.buffer())?
                    .write(0, &host.debits)?;
                owner
                    .write(buffers.debit_params.buffer())?
                    .write(0, &[word(debits)?, word(cells)?, carrier0.moment_count])?;
            }
            if items(&host.sets[0]) > 0 {
                upload_set(
                    owner,
                    &buffers.sets[0],
                    &host.sets[0],
                    &red0,
                    cells,
                    carrier0,
                )?;
            }
            if items(&host.sets[1]) > 0 {
                upload_set(
                    owner,
                    &buffers.sets[1],
                    &host.sets[1],
                    &red1,
                    cells,
                    carrier0,
                )?;
            }
            if items(&host.trail_set) > 0 {
                upload_set(
                    owner,
                    trail_buf,
                    &host.trail_set,
                    &red_trail,
                    cells,
                    carrier1,
                )?;
            }
            owner.write(buffers.fault.buffer())?.write(0, &[0])?;
            owner.submit_batch(&kernels)?;
            check_reduce_fault(owner, &buffers.fault)?;
        }

        read_received(
            owner,
            &buffers.sets[0],
            &mut host.sets[0],
            cells,
            &mut host.words,
        )?;
        read_received(
            owner,
            &buffers.sets[1],
            &mut host.sets[1],
            cells,
            &mut host.words,
        )?;
        read_received(
            owner,
            trail_buf,
            &mut host.trail_set,
            cells,
            &mut host.words,
        )?;

        let c0_summaries = [
            red0.summarize(&host.sets[0].received)?,
            red1.summarize(&host.sets[1].received)?,
        ];
        let trail_summary = red_trail.summarize(&host.trail_set.received)?;

        if !kernels.is_empty() {
            let lease = owner.read(buffers.field.buffer(), Purpose::Readback)?;
            if lease.elements() < values {
                return Err(overflow());
            }
            lease.read(field.as_mut_slice())?;
        }
        debit.finish();
        red0.finish();
        red1.finish();
        red_trail.finish();
        Ok((c0_summaries, trail_summary))
    }
}

fn pack_debits(
    debits: &[flife_field::CellDebit],
    cells: usize,
    out: &mut Vec<u32>,
) -> Result<(), CoreError> {
    out.clear();
    let mut last = None;
    for cell_debit in debits.iter().filter(|debit| debit.granted > 0.0) {
        if cell_debit.cell >= cells || last.is_some_and(|last| cell_debit.cell <= last) {
            return Err(CoreError::StateConflict {
                operation: "metal grants",
                detail: format!(
                    "debit of cell {} after cell {last:?} of {cells}",
                    cell_debit.cell
                ),
            });
        }
        last = Some(cell_debit.cell);
        out.extend_from_slice(&[
            word(cell_debit.cell)?,
            cell_debit.energy_before.to_bits(),
            cell_debit.energy_after.to_bits(),
        ]);
    }
    Ok(())
}

fn upload_set(
    owner: &DeviceOwner,
    set: &Set,
    words: &HostSet,
    reduction: &DepositReduction<'_>,
    cells: usize,
    carrier: CarrierLayout,
) -> Result<(), CoreError> {
    owner
        .write(set.offsets.buffer())?
        .write(0, reduction.offsets())?;
    owner
        .write(set.packets.buffer.buffer())?
        .write(0, &words.packets)?;
    owner
        .write(set.directions.buffer.buffer())?
        .write(0, &words.directions)?;
    owner.write(set.params.buffer())?.write(
        0,
        &[
            word(cells)?,
            word(reduction.deposits())?,
            carrier.energy_plane,
            carrier.first_moment_plane,
            carrier.moment_count,
        ],
    )?;
    Ok(())
}

fn check_reduce_fault(owner: &DeviceOwner, fault_buf: &Owned<u32>) -> Result<(), CoreError> {
    let mut fault = [0_u32];
    owner
        .read(fault_buf.buffer(), Purpose::Readback)?
        .read(&mut fault)?;
    if fault[0] != 0 {
        return Err(CoreError::NumericalInvalid {
            quantity: "metal reduction operand",
            detail: "a field value or amount the device met or would write is not finite"
                .to_owned(),
        });
    }
    Ok(())
}

fn read_received(
    owner: &DeviceOwner,
    set: &Set,
    words: &mut HostSet,
    cells: usize,
    scratch: &mut Vec<u32>,
) -> Result<(), CoreError> {
    words.received.clear();
    words.received.resize(cells, 0.0);
    if words.packets.len() / PACKET == 0 {
        return Ok(());
    }
    scratch.clear();
    scratch.resize(2 * cells, 0);
    owner
        .read(set.received.buffer(), Purpose::Readback)?
        .read(scratch)?;
    for (value, pair) in words.received.iter_mut().zip(scratch.as_chunks::<2>().0) {
        *value = f64::from_bits(u64::from(pair[0]) | (u64::from(pair[1]) << 32));
    }
    Ok(())
}

/// `reduction`'s grouped packets (amount, then deposit index) and its
/// deposits' directions as words.
fn set_words(
    reduction: &DepositReduction<'_>,
    set: &mut HostSet,
    cells: usize,
) -> Result<(), CoreError> {
    let offsets = reduction.offsets();
    if offsets.len() != cells + 1 {
        return Err(overflow());
    }
    let items = offsets.last().map_or(0, |&end| end as usize);
    set.packets.clear();
    set.packets
        .reserve(items.checked_mul(PACKET).ok_or_else(overflow)?);
    for at in 0..items {
        let (amount, deposit) = reduction.packet(at).ok_or_else(overflow)?;
        set.packets.extend_from_slice(&words(amount));
        set.packets.push(word(deposit)?);
    }
    set.directions.clear();
    if items > 0 {
        set.directions.reserve(
            reduction
                .deposits()
                .checked_mul(DIRECTION)
                .ok_or_else(overflow)?,
        );
        for index in 0..reduction.deposits() {
            let direction = reduction.direction(index).ok_or_else(overflow)?;
            for &component in direction {
                set.directions.extend_from_slice(&words(component));
            }
        }
    }
    Ok(())
}
