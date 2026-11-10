//! FL-031 (`flife-8le`), last part: phase 5's demand and grants on Metal
//! (plan §5.2, §8.1, §10.7). [`MetalUptake`] is an [`UptakeBackend`]: every
//! organism's density and request, and every cell's demand and grants under
//! `cell-quantum-largest-remainder-v1`, bit-identical to flife-field's
//! [`flife_field::demands`] and [`flife_field::resolve_uptake`].
//!
//! The work is split by what each side does best:
//! - the device computes each footprint's density and saturating request,
//!   one thread per organism, and each cell's compensated demand, quantum,
//!   target, floor shares and remainders, one thread per cell. It ranks the
//!   packets of every cell whose floors fall short of its target, one thread
//!   per packet, and applies the deficit in closed form. All of it is in
//!   `uptake.metal`, on `softfloat64.metal`'s binary64. A cell whose floors
//!   exceed its target, which no valid input makes (`uptake.metal` says
//!   why), keeps them, and the host's sum check refuses it.
//! - the host takes each footprint, and stages and groups the packets in
//!   identity order through flife-field's own code
//!   ([`flife_field::demands_on`], [`flife_field::resolve_uptake_on`]). It
//!   checks every value and refuses quanta that do not sum to the cell's
//!   grant.
//!
//! A tick is two commands: the requests, then the grants (the host groups
//! the packets in between). The energy plane and the packets are copied to
//! the device and the results back every tick.
//!
//! softfloat64 gives a quiet NaN where IEEE would give an infinity or a
//! zero from an infinite operand. Any non-finite density, request or demand
//! is therefore refused, so the world computes that tick's phase 5 on the
//! reference instead of using a value the reference would not give.

use flife_core::CoreError;
use flife_field::{ExchangeStencil, FieldState, RequestRule, UptakeBackend, UptakeCell};
use ft_kernel_metal::compute::{Grid, MathMode, Pipeline};

use crate::owner::{Binding, Buffer, DeviceOwner, Element, Kernel, Limits, Owned, Purpose, metal};

/// The uptake kernel source, with the software binary64 ahead of it.
pub const UPTAKE_MSL: &str = concat!(
    include_str!("softfloat64.metal"),
    include_str!("uptake.metal")
);

/// `uptake.metal`'s layouts, in words.
const STENCIL: usize = 13;
const REQUEST_PARAMS: usize = 10;
const REQUEST_OUT: usize = 4;
const CELL: usize = 3;
const RESOLVED: usize = 5;
const RESOLVE_PARAMS: usize = 2;

/// Threads per threadgroup.
const THREADS: usize = 256;

fn words(value: f64) -> [u32; 2] {
    let bits = value.to_bits();
    [(bits & 0xFFFF_FFFF) as u32, (bits >> 32) as u32]
}

fn put(target: &mut [u32], at: usize, value: f64) {
    target[at..at + 2].copy_from_slice(&words(value));
}

fn value(pair: &[u32]) -> f64 {
    f64::from_bits(u64::from(pair[0]) | (u64::from(pair[1]) << 32))
}

fn overflow() -> CoreError {
    CoreError::Overflow {
        quantity: "metal uptake size",
    }
}

fn word(value: usize) -> Result<u32, CoreError> {
    u32::try_from(value).map_err(|_| overflow())
}

fn refuse(detail: String) -> CoreError {
    CoreError::StateConflict {
        operation: "metal uptake",
        detail,
    }
}

/// A device buffer with room for at least `capacity` elements, replaced by
/// one of the next power of two when a tick outgrows it; the old one retires
/// when dropped.
#[derive(Debug)]
struct Slot<T: Element> {
    owned: Option<Owned<T>>,
    capacity: usize,
}

impl<T: Element> Slot<T> {
    const fn new() -> Self {
        Self {
            owned: None,
            capacity: 0,
        }
    }

    fn ensure(&mut self, owner: &DeviceOwner, len: usize) -> Result<Buffer<T>, CoreError> {
        if self.owned.is_none() || self.capacity < len {
            self.owned = None;
            let capacity = len.max(1).next_power_of_two();
            self.owned = Some(owner.zeroed_owned(capacity)?);
            self.capacity = capacity;
        }
        self.owned.as_ref().map(Owned::buffer).ok_or_else(overflow)
    }
}

/// The device buffers, each grown as a tick needs.
#[derive(Debug)]
struct Buffers {
    energy: Slot<f32>,
    stencils: Slot<u32>,
    request_params: Slot<u32>,
    request_out: Slot<u32>,
    cells: Slot<u32>,
    requested: Slot<u32>,
    resolve_params: Slot<u32>,
    resolved: Slot<u32>,
    cell_of: Slot<u32>,
    remainders: Slot<u32>,
    quanta: Slot<u32>,
}

impl Buffers {
    const fn new() -> Self {
        Self {
            energy: Slot::new(),
            stencils: Slot::new(),
            request_params: Slot::new(),
            request_out: Slot::new(),
            cells: Slot::new(),
            requested: Slot::new(),
            resolve_params: Slot::new(),
            resolved: Slot::new(),
            cell_of: Slot::new(),
            remainders: Slot::new(),
            quanta: Slot::new(),
        }
    }
}

/// What the host prepares and reads back each tick.
#[derive(Debug, Default)]
struct Host {
    stencils: Vec<u32>,
    out: Vec<u32>,
    cells: Vec<u32>,
    requested: Vec<u32>,
    resolved: Vec<u32>,
}

/// Phase 5's demand and grants on Metal; see the module documentation.
pub struct MetalUptake {
    owner: DeviceOwner,
    request: Pipeline,
    cells: Pipeline,
    rank: Pipeline,
    buffers: Buffers,
    host: Host,
}

impl std::fmt::Debug for MetalUptake {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MetalUptake")
            .field("device", &self.owner.device_name())
            .field("buffers", &self.buffers)
            .finish_non_exhaustive()
    }
}

impl MetalUptake {
    /// Open the default Metal device and compile the uptake kernels.
    ///
    /// # Errors
    /// [`CoreError::MissingCapability`] without a Metal device (always, off
    /// macOS); [`CoreError::StateConflict`] if the kernels do not compile.
    pub fn open() -> Result<Self, CoreError> {
        Self::with_owner(DeviceOwner::open(Limits::UNBOUNDED)?)
    }

    /// The uptake kernels on `owner`'s device; their buffers are `owner`'s.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] if the kernels do not compile.
    pub fn with_owner(owner: DeviceOwner) -> Result<Self, CoreError> {
        let library = owner.library(UPTAKE_MSL, MathMode::Safe)?;
        let pipeline = |name: &str| library.pipeline(name).map_err(metal);
        Ok(Self {
            request: pipeline("uptake_request")?,
            cells: pipeline("uptake_cells")?,
            rank: pipeline("uptake_rank")?,
            owner,
            buffers: Buffers::new(),
            host: Host::default(),
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
}

fn linear(threads: usize) -> Grid {
    Grid::linear(threads.div_ceil(THREADS), THREADS)
}

impl UptakeBackend for MetalUptake {
    fn describe(&self) -> String {
        format!("metal ({})", self.owner.device_name())
    }

    fn request(
        &mut self,
        field: &FieldState,
        rule: &RequestRule,
        stencils: &[ExchangeStencil],
        densities: &mut Vec<f64>,
        requests: &mut Vec<f64>,
    ) -> Result<(), CoreError> {
        densities.clear();
        requests.clear();
        let count = stencils.len();
        if count == 0 {
            return Ok(());
        }
        let energy = field.energy();
        let host = &mut self.host;
        host.stencils.clear();
        for stencil in stencils {
            let mut descriptor = [0_u32; STENCIL];
            descriptor[0] = word(stencil.cells().len())?;
            for (k, (&cell, &weight)) in stencil.cells().iter().zip(stencil.weights()).enumerate() {
                // The reference's own refusal of a stencil made for another
                // grid; the device must never read past the plane.
                if cell >= energy.len() {
                    return Err(CoreError::LayoutMismatch {
                        buffer: "field",
                        detail: format!("exchange cell {cell} is not in this field"),
                    });
                }
                descriptor[1 + k] = word(cell)?;
                put(&mut descriptor, 5 + 2 * k, weight);
            }
            host.stencils.extend_from_slice(&descriptor);
        }
        let mut params = [0_u32; REQUEST_PARAMS];
        params[0] = word(count)?;
        put(&mut params, 2, field.grid().cell_area());
        put(&mut params, 4, rule.step.seconds_per_tick());
        put(&mut params, 6, rule.capacity_per_second);
        put(&mut params, 8, rule.half_saturation_density);

        let Self {
            owner,
            request,
            buffers,
            ..
        } = self;
        let energy_buffer = buffers.energy.ensure(owner, energy.len())?;
        let stencil_buffer = buffers.stencils.ensure(owner, host.stencils.len())?;
        let params_buffer = buffers.request_params.ensure(owner, REQUEST_PARAMS)?;
        let out_buffer = buffers
            .request_out
            .ensure(owner, count.checked_mul(REQUEST_OUT).ok_or_else(overflow)?)?;
        owner.write(energy_buffer)?.write(0, energy)?;
        owner.write(stencil_buffer)?.write(0, &host.stencils)?;
        owner.write(params_buffer)?.write(0, &params)?;
        let bindings: [Binding; 4] = [
            energy_buffer.reads(),
            stencil_buffer.reads(),
            params_buffer.reads(),
            out_buffer.writes(),
        ];
        owner.submit_batch(&[Kernel {
            pipeline: request,
            bindings: &bindings,
            grid: linear(count),
        }])?;

        host.out.resize(count * REQUEST_OUT, 0);
        owner
            .read(out_buffer, Purpose::Readback)?
            .read(&mut host.out)?;
        for block in host.out.as_chunks::<REQUEST_OUT>().0 {
            let (density, request) = (value(&block[..2]), value(&block[2..]));
            if !density.is_finite() || !request.is_finite() {
                return Err(refuse(format!(
                    "a non-finite density ({density}) or request ({request}); the reference decides this tick"
                )));
            }
            densities.push(density);
            requests.push(request);
        }
        Ok(())
    }

    fn resolve(
        &mut self,
        cells: &[UptakeCell],
        requested: &[f64],
        demanded: &mut Vec<f64>,
        quanta: &mut Vec<u32>,
    ) -> Result<(), CoreError> {
        demanded.clear();
        quanta.clear();
        let packets = requested.len();
        // The cells must partition the packets in order: every packet's cell
        // is written by its cell's thread, and a gap would leave it unknown.
        let host = &mut self.host;
        host.cells.clear();
        let mut next = 0_usize;
        for cell in cells {
            if cell.start != next || cell.len == 0 {
                return Err(refuse(format!(
                    "a cell starts at packet {} with {} packets, after {next}",
                    cell.start, cell.len
                )));
            }
            next = next.checked_add(cell.len).ok_or_else(overflow)?;
            let descriptor: [u32; CELL] =
                [word(cell.start)?, word(cell.len)?, cell.energy.to_bits()];
            host.cells.extend_from_slice(&descriptor);
        }
        if next != packets {
            return Err(refuse(format!(
                "the cells hold {next} packets of {packets}"
            )));
        }
        if cells.is_empty() {
            return Ok(());
        }
        host.requested.clear();
        for &amount in requested {
            host.requested.extend_from_slice(&words(amount));
        }
        let params = [word(cells.len())?, word(packets)?];

        let Self {
            owner,
            cells: cells_pipeline,
            rank,
            buffers,
            ..
        } = self;
        let cells_buffer = buffers.cells.ensure(owner, host.cells.len())?;
        let requested_buffer = buffers.requested.ensure(owner, host.requested.len())?;
        let params_buffer = buffers.resolve_params.ensure(owner, RESOLVE_PARAMS)?;
        let resolved_buffer = buffers.resolved.ensure(
            owner,
            cells.len().checked_mul(RESOLVED).ok_or_else(overflow)?,
        )?;
        let cell_of = buffers.cell_of.ensure(owner, packets)?;
        let remainders = buffers
            .remainders
            .ensure(owner, packets.checked_mul(2).ok_or_else(overflow)?)?;
        let quanta_buffer = buffers.quanta.ensure(owner, packets)?;
        owner.write(cells_buffer)?.write(0, &host.cells)?;
        owner.write(requested_buffer)?.write(0, &host.requested)?;
        owner.write(params_buffer)?.write(0, &params)?;

        // One command: every cell's demand, target and floors; then the
        // ranks of the packets whose cell falls short, with the deficits
        // applied.
        let cell_bindings = [
            cells_buffer.reads(),
            requested_buffer.reads(),
            params_buffer.reads(),
            resolved_buffer.writes(),
            cell_of.writes(),
            remainders.writes(),
            quanta_buffer.writes(),
        ];
        let rank_bindings = [
            cells_buffer.reads(),
            params_buffer.reads(),
            resolved_buffer.reads(),
            cell_of.reads(),
            remainders.reads(),
            quanta_buffer.writes(),
        ];
        owner.submit_batch(&[
            Kernel {
                pipeline: cells_pipeline,
                bindings: &cell_bindings,
                grid: linear(cells.len()),
            },
            Kernel {
                pipeline: rank,
                bindings: &rank_bindings,
                grid: linear(packets),
            },
        ])?;

        host.resolved.resize(cells.len() * RESOLVED, 0);
        owner
            .read(resolved_buffer, Purpose::Readback)?
            .read(&mut host.resolved)?;
        for block in host.resolved.as_chunks::<RESOLVED>().0 {
            let demand = value(&block[..2]);
            if !demand.is_finite() {
                return Err(refuse(format!(
                    "a non-finite cell demand ({demand}); the reference decides this tick"
                )));
            }
            demanded.push(demand);
        }
        quanta.resize(packets, 0);
        owner.read(quanta_buffer, Purpose::Readback)?.read(quanta)?;
        Ok(())
    }
}
