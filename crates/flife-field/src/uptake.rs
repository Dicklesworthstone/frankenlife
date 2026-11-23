//! Simultaneous uptake without overspending (plan §5.2, §11.3, INV-04,
//! Appendix C.1).
//!
//! Uptake is a two-pass demand-and-grant protocol over one immutable field
//! snapshot.
//!
//! 1. Organism `i` requests `d_i ≥ 0`. The request is spread over its
//!    exchange footprint as packets `q_ic = d_i w_ic`.
//! 2. Packets are grouped by cell, in organism identity order within each
//!    cell, by [`Bins`]' counting sort (FL-027): `O(cells + packets)` and
//!    bounded scratch, however the organisms cluster. Each cell sums its
//!    demand `A_c` in that order and grants `u_ic = α_c q_ic`, where
//!    `α_c = min(1, e_c/A_c)` and `α_c = 0` when `A_c = 0`.
//!
//! Finite precision needs a defined reconciliation. This module implements
//! `cell-quantum-largest-remainder-v1`:
//! - A cell's F32 energy is a whole number `n_c` of quanta `Q_c`, where
//!   `Q_c` is its F32 unit in the last place, a power of two. Grants are
//!   whole quanta.
//! - A contended cell (`A_c > e_c`) grants exactly `n_c` quanta and is
//!   drained.
//! - An uncontended cell grants `round(A_c/Q_c)` quanta, with ties to even
//!   and never more than `n_c`.
//! - The quanta are apportioned by largest remainder. Each packet first gets
//!   the floor of its ideal share `α_c q_ic / Q_c`. Leftover quanta go to
//!   the largest fractional parts, with ties going to the lower identity.
//!
//! Therefore:
//! - Each grant is nonnegative and within one quantum of its ideal value.
//! - A cell's grants sum to exactly its debit, and the debit never exceeds
//!   its energy (INV-04).
//! - The new cell energy `(n_c − K_c) Q_c` is exactly representable in F32.
//!   The field loses exactly what organisms are granted, so uptake adds no
//!   residual to the ledger.
//! - The result depends on organism identities, never on storage slots or
//!   input order.
//!
//! An uncontended request below half a quantum is granted nothing. The F32
//! field cannot represent that change, and granting it anyway would create
//! energy.
//!
//! FL-031: [`UptakeBackend`] is the port for a device phase 5. A backend
//! computes each organism's density and request ([`demands_on`]) and each
//! cell's demand and grants ([`resolve_uptake_on`]); the host keeps the
//! footprints, the packets' staging and grouping, the checks and the
//! records, through this module's own code, so both paths give the same
//! demands and grants bit for bit.
//!
//! Applying grants debits each cell's energy and scales its moment by
//! `e'_c/e_c`, which removes the same proportion of composition. Grants
//! carry the pre-debit cell energies. Applying them to a field that no
//! longer holds those values, or applying them twice, is refused before
//! anything is written.

use flife_core::ids::OrganismId;
use flife_core::ledger::{CompensatedSum, add_down};
use flife_core::memory::{bytes_for, capacity_bytes, total};
use flife_core::model::MOMENT_CHANNELS;
use flife_core::{CoreError, TimeStep};

use crate::bins::Bins;
use crate::exchange::{
    ExchangeKernel, ExchangeScratch, ExchangeStencil, check_request, saturating_request,
};
use crate::footprint::BufferLimits;
use crate::grid::FieldGrid;
use crate::state::{CHANNELS, FieldState};

/// The most packets one organism's request becomes: its exchange footprint
/// has at most four cells.
const PACKETS_PER_DEMAND: u64 = 4;

/// The finite-precision reconciliation this module implements.
pub const CELL_QUANTUM_LARGEST_REMAINDER_V1: &str = "cell-quantum-largest-remainder-v1";

/// One organism's uptake request for this tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Demand {
    /// The requesting organism. Each organism may demand once per tick.
    pub organism: OrganismId,
    /// Its position. The exchange footprint is taken here.
    pub position: [f64; 2],
    /// `d_i`, the total energy requested this tick.
    pub amount: f64,
}

/// One organism's grant from one cell.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GrantPacket {
    /// The cell.
    pub cell: usize,
    /// The organism.
    pub organism: OrganismId,
    /// The index of the organism's [`Demand`] in the resolved slice.
    pub demand: usize,
    /// `q_ic = d_i w_ic`.
    pub requested: f64,
    /// `u_ic`: whole quanta of the cell, exact in F64.
    pub granted: f64,
}

/// One cell's debit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellDebit {
    /// The cell.
    pub cell: usize,
    /// Its energy when the grants were resolved.
    pub energy_before: f32,
    /// Its energy after the debit: exactly `energy_before − granted`.
    pub energy_after: f32,
    /// `A_c`, the cell's total demand.
    pub demanded: f64,
    /// `Σ_i u_ic`, exact.
    pub granted: f64,
    /// Whether `A_c > e_c`.
    pub contended: bool,
}

/// Totals for one resolution.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct UptakeSummary {
    /// `Σ_c A_c`.
    pub requested: f64,
    /// `Σ_c Σ_i u_ic`.
    pub granted: f64,
    /// Cells with demand.
    pub cells: usize,
    /// Cells whose demand exceeded their energy.
    pub contended_cells: usize,
    /// Cells left with no energy.
    pub drained_cells: usize,
    /// The most packets competing for one cell.
    pub largest_cell_packets: usize,
}

/// The grants of one resolution, reusable from tick to tick so that
/// steady-state ticks do not allocate (§9.4).
#[derive(Debug, Clone)]
pub struct Grants {
    grid: Option<FieldGrid>,
    packets: Vec<GrantPacket>,
    debits: Vec<CellDebit>,
    totals: Vec<f64>,
    summary: UptakeSummary,
    applied: bool,
    /// The most demands one resolution admits.
    organisms: usize,
    identities: Vec<(OrganismId, usize)>,
    /// Packets in organism identity order, before grouping by cell.
    staged: Vec<GrantPacket>,
    bins: Bins,
    shares: Vec<Share>,
    rank: Vec<usize>,
    exchange: ExchangeScratch,
}

impl Default for Grants {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy)]
struct Share {
    quanta: u64,
    remainder: f64,
}

impl Grants {
    /// Bytes its packets, debits, totals and resolution scratch hold
    /// (FL-043).
    #[must_use]
    pub fn allocated_bytes(&self) -> u64 {
        total([
            capacity_bytes(&self.packets),
            capacity_bytes(&self.debits),
            capacity_bytes(&self.totals),
            capacity_bytes(&self.identities),
            capacity_bytes(&self.staged),
            self.bins.allocated_bytes(),
            capacity_bytes(&self.shares),
            capacity_bytes(&self.rank),
            self.exchange.allocated_bytes(),
        ])
    }

    /// Empty grants with no declared capacity: their scratch grows on demand
    /// within the `u32` index range.
    #[must_use]
    pub fn new() -> Self {
        Self {
            grid: None,
            packets: Vec::new(),
            debits: Vec::new(),
            totals: Vec::new(),
            summary: UptakeSummary::default(),
            applied: false,
            organisms: usize::try_from(u32::MAX).unwrap_or(usize::MAX) / 4,
            identities: Vec::new(),
            staged: Vec::new(),
            bins: Bins::new(UPTAKE_PACKETS),
            shares: Vec::new(),
            rank: Vec::new(),
            exchange: ExchangeScratch::new(),
        }
    }

    /// Empty grants declared for at most `organisms` demands a resolution on
    /// `grid` under `grid-cic-v1` (§9.4). Their scratch is admitted against
    /// `limits` and allocated now, so a resolution within the declaration
    /// never allocates, and one beyond it is refused.
    ///
    /// # Errors
    /// [`CoreError::BudgetExhausted`] when a scratch buffer exceeds
    /// `limits` or the `u32` index range; [`CoreError::Overflow`] if a byte
    /// count overflows.
    pub fn with_capacity(
        grid: &FieldGrid,
        organisms: u64,
        limits: &BufferLimits,
    ) -> Result<Self, CoreError> {
        Self::with_capacity_for(grid, organisms, ExchangeKernel::GridCicV1, limits)
    }

    /// Empty grants declared for at most `organisms` demands a resolution on
    /// `grid` under `kernel` (FL-053, §9.4, FL-043).
    ///
    /// # Errors
    /// As [`Grants::with_capacity`], plus [`CoreError::AllocationFailed`] if
    /// footprint scratch cannot be reserved.
    pub fn with_capacity_for(
        grid: &FieldGrid,
        organisms: u64,
        kernel: ExchangeKernel,
        limits: &BufferLimits,
    ) -> Result<Self, CoreError> {
        let overflow = || CoreError::Overflow {
            quantity: UPTAKE_PACKETS,
        };
        let packets_per_demand = u64::try_from(kernel.max_cells(grid)).map_err(|_| overflow())?;
        let packets = organisms
            .checked_mul(packets_per_demand)
            .ok_or_else(overflow)?;
        let packet_bytes = packets
            .checked_mul(std::mem::size_of::<GrantPacket>() as u64)
            .ok_or_else(overflow)?;
        limits.admit(UPTAKE_PACKETS, packet_bytes)?;
        let bins = Bins::with_capacity(UPTAKE_PACKETS, grid.cell_count(), packets, limits)?;
        let exchange = ExchangeScratch::for_kernel(kernel, grid)?;
        let organisms = usize::try_from(organisms).map_err(|_| overflow())?;
        let packets = usize::try_from(packets).map_err(|_| overflow())?;
        let cells = usize::try_from(grid.cell_count()).map_err(|_| overflow())?;
        Ok(Self {
            packets: Vec::with_capacity(packets),
            debits: Vec::with_capacity(packets.min(cells)),
            totals: Vec::with_capacity(organisms),
            organisms,
            identities: Vec::with_capacity(organisms),
            staged: Vec::with_capacity(packets),
            bins,
            // One cell may hold every packet: the hotspot.
            shares: Vec::with_capacity(packets),
            rank: Vec::with_capacity(packets),
            exchange,
            ..Self::new()
        })
    }

    /// The most demands one resolution admits.
    #[must_use]
    pub const fn organisms(&self) -> usize {
        self.organisms
    }

    /// The cell bins of the last resolution: every packet's index in
    /// [`Grants::packets`] order, grouped by cell (FL-027).
    #[must_use]
    pub const fn bins(&self) -> &Bins {
        &self.bins
    }

    /// The allocated capacity of every scratch buffer, in entries: equal
    /// before and after a resolution within the declaration.
    #[must_use]
    pub fn allocated(&self) -> [usize; 9] {
        let (offsets, order) = self.bins.allocated();
        [
            self.packets.capacity(),
            self.staged.capacity(),
            self.debits.capacity(),
            self.totals.capacity(),
            self.identities.capacity(),
            offsets,
            order,
            self.shares.capacity(),
            self.rank.capacity(),
        ]
    }

    /// Every packet, sorted by (cell, organism).
    #[must_use]
    pub fn packets(&self) -> &[GrantPacket] {
        &self.packets
    }

    /// Every cell with demand, in ascending cell order.
    #[must_use]
    pub fn debits(&self) -> &[CellDebit] {
        &self.debits
    }

    /// These grants checked against `field` for a debit on another device
    /// (FL-032), which writes it as [`apply_grants_on`] does: each debited
    /// cell's energy becomes its debit's `energy_after`, and each of its
    /// moments is scaled by `energy_after / energy_before` in F64 (exactly
    /// zero when the cell is drained). Only debits that granted energy
    /// write anything.
    ///
    /// # Errors
    /// As [`apply_grants_on`]. Nothing is applied either way until
    /// [`GrantDebit::finish`].
    pub fn debit(&mut self, field: &FieldState) -> Result<GrantDebit<'_>, CoreError> {
        self.check_debit(field)?;
        Ok(GrantDebit { grants: self })
    }

    /// [`apply_grants_on`]'s checks of these grants against `field`, which
    /// they would debit: not applied, resolved for its grid, every debited
    /// cell still holding the energy they were resolved against, and every
    /// plane whole. Returns the grid's cell count.
    fn check_debit(&self, field: &FieldState) -> Result<usize, CoreError> {
        let conflict = |detail: String| CoreError::StateConflict {
            operation: "uptake grants",
            detail,
        };
        if self.applied {
            return Err(conflict("these grants were already applied".into()));
        }
        if self.grid != Some(*field.grid()) {
            return Err(conflict(
                "these grants were resolved for another grid".into(),
            ));
        }
        let energy = field.energy();
        for debit in &self.debits {
            let current = energy.get(debit.cell).copied();
            if current.map(f32::to_bits) != Some(debit.energy_before.to_bits()) {
                return Err(conflict(format!(
                    "cell {} holds {current:?}, but the grants were resolved against {}",
                    debit.cell, debit.energy_before
                )));
            }
        }
        if (0..MOMENT_CHANNELS).any(|channel| field.moment(channel).is_none()) {
            return Err(conflict("a moment plane is missing".into()));
        }
        let cells =
            usize::try_from(field.grid().cell_count()).map_err(|_| CoreError::Overflow {
                quantity: "field cells",
            })?;
        if field.as_slice().len().checked_sub(cells) != Some(cells.saturating_mul(CHANNELS)) {
            return Err(conflict("the moment planes are not whole".into()));
        }
        Ok(cells)
    }

    /// `U_i = Σ_c u_ic` for each demand, in the order the demands were
    /// given. Each total is summed in cell order and rounded down, so it
    /// never exceeds the energy its cells lost.
    #[must_use]
    pub fn totals(&self) -> &[f64] {
        &self.totals
    }

    /// The resolution's totals.
    #[must_use]
    pub const fn summary(&self) -> UptakeSummary {
        self.summary
    }

    /// Whether these grants have been applied.
    #[must_use]
    pub const fn is_applied(&self) -> bool {
        self.applied
    }

    fn reset(&mut self) {
        self.grid = None;
        self.packets.clear();
        self.debits.clear();
        self.totals.clear();
        self.summary = UptakeSummary::default();
        self.applied = false;
        self.identities.clear();
        self.staged.clear();
        self.bins.clear();
    }
}

/// The name grants' scratch refusals carry.
const UPTAKE_PACKETS: &str = "uptake packets";

/// Check one demand's amount and stage its packets under `kernel`.
fn stage_demand(
    grid: &FieldGrid,
    kernel: ExchangeKernel,
    index: usize,
    demand: &Demand,
    exchange: &mut ExchangeScratch,
    staged: &mut Vec<GrantPacket>,
) -> Result<(), CoreError> {
    if !demand.amount.is_finite() || demand.amount < 0.0 {
        return Err(CoreError::NumericalInvalid {
            quantity: "uptake demand",
            detail: format!(
                "organism {} requests {}",
                demand.organism.get(),
                demand.amount
            ),
        });
    }
    match kernel {
        ExchangeKernel::GridCicV1 => {
            let stencil = ExchangeStencil::at(grid, demand.position)?;
            for (&cell, &weight) in stencil.cells().iter().zip(stencil.weights()) {
                let requested = demand.amount * weight;
                if requested > 0.0 {
                    staged.push(GrantPacket {
                        cell,
                        organism: demand.organism,
                        demand: index,
                        requested,
                        granted: 0.0,
                    });
                }
            }
        }
        ExchangeKernel::FixedWorldGaussianV1 { .. } => {
            kernel.footprint_into(grid, demand.position, exchange)?;
            for &(cell, weight) in exchange.cells() {
                let requested = demand.amount * weight;
                if requested > 0.0 {
                    staged.push(GrantPacket {
                        cell,
                        organism: demand.organism,
                        demand: index,
                        requested,
                        granted: 0.0,
                    });
                }
            }
        }
    }
    Ok(())
}

/// Check one demand under `kernel` without staging any packet (for reporting
/// the first invalid demand in input order).
fn check_demand(
    grid: &FieldGrid,
    kernel: ExchangeKernel,
    demand: &Demand,
    exchange: &mut ExchangeScratch,
) -> Result<(), CoreError> {
    if !demand.amount.is_finite() || demand.amount < 0.0 {
        return Err(CoreError::NumericalInvalid {
            quantity: "uptake demand",
            detail: format!(
                "organism {} requests {}",
                demand.organism.get(),
                demand.amount
            ),
        });
    }
    match kernel {
        ExchangeKernel::GridCicV1 => {
            let _ = ExchangeStencil::at(grid, demand.position)?;
        }
        ExchangeKernel::FixedWorldGaussianV1 { .. } => {
            kernel.footprint_into(grid, demand.position, exchange)?;
        }
    }
    Ok(())
}

/// Resolve `demands` against the immutable `field` snapshot into `grants`
/// under `grid-cic-v1`. The field is not changed; [`apply_grants`] debits it.
///
/// # Errors
/// - [`CoreError::BudgetExhausted`] for more demands than `grants` were
///   declared for ([`Grants::with_capacity`]), before anything is staged.
/// - [`CoreError::StateConflict`] when an organism demands twice.
/// - [`CoreError::NumericalInvalid`] for a negative or nonfinite amount, a
///   nonfinite position, or an invalid cell energy. Of several invalid
///   demands, the first in `demands` is reported.
///
/// On error `grants` is left empty.
pub fn resolve_uptake(
    field: &FieldState,
    demands: &[Demand],
    grants: &mut Grants,
) -> Result<UptakeSummary, CoreError> {
    resolve_uptake_on_workers(field, demands, grants, 1)
}

/// [`resolve_uptake`] under an explicit [`ExchangeKernel`] (FL-053).
///
/// # Errors
/// As [`resolve_uptake`].
pub fn resolve_uptake_with(
    field: &FieldState,
    kernel: ExchangeKernel,
    demands: &[Demand],
    grants: &mut Grants,
) -> Result<UptakeSummary, CoreError> {
    resolve_uptake_with_on_workers(field, kernel, demands, grants, 1)
}

/// The fewest packets one thread of [`resolve_uptake_on_workers`] resolves.
const UPTAKE_PACKETS_PER_RUN: usize = 2_048;

/// [`resolve_uptake`] with per-cell grant resolution split across up to
/// `workers` threads (this one and scoped workers joined before returning).
/// Cells are independent once packets are grouped by cell; the host sums
/// [`Tally`] in ascending cell order afterwards, so every grant, debit and
/// summary bit is identical at any worker count, and zero steady-state
/// allocations occur for non-hotspot cells (`<= 32` packets).
///
/// # Errors
/// As [`resolve_uptake`], or [`CoreError::StateConflict`] if a worker panicked.
pub fn resolve_uptake_on_workers(
    field: &FieldState,
    demands: &[Demand],
    grants: &mut Grants,
    workers: usize,
) -> Result<UptakeSummary, CoreError> {
    resolve_uptake_with_on_workers(field, ExchangeKernel::GridCicV1, demands, grants, workers)
}

/// [`resolve_uptake_on_workers`] under an explicit [`ExchangeKernel`]
/// (FL-053).
///
/// # Errors
/// As [`resolve_uptake_on_workers`].
pub fn resolve_uptake_with_on_workers(
    field: &FieldState,
    kernel: ExchangeKernel,
    demands: &[Demand],
    grants: &mut Grants,
    workers: usize,
) -> Result<UptakeSummary, CoreError> {
    grants.reset();
    let result = resolve_into(field, kernel, demands, grants, workers);
    if result.is_err() {
        grants.reset();
    }
    result
}

fn resolve_into(
    field: &FieldState,
    kernel: ExchangeKernel,
    demands: &[Demand],
    grants: &mut Grants,
    workers: usize,
) -> Result<UptakeSummary, CoreError> {
    let grid = *field.grid();
    stage(&grid, kernel, demands, grants)?;
    group_packets(&grid, grants, workers)?;

    let energy = field.energy();
    let Grants {
        bins,
        packets,
        debits,
        shares,
        rank,
        ..
    } = grants;
    let items = packets.len();
    let buckets = bins.buckets();
    let offsets = bins.offsets();
    let runs = workers.min(items / UPTAKE_PACKETS_PER_RUN).max(1);
    let mut tally = Tally::default();

    if runs == 1 {
        for (cell, range) in bins.occupied() {
            let value = cell_energy(energy, cell)?;
            let length = range.len();
            let Some(group) = packets.get_mut(range) else {
                continue;
            };
            let debit = resolve_cell(group, cell, value, shares, rank)?;
            tally.record(&debit, length);
            debits.push(debit);
        }
    } else {
        // Partition cells into `runs` contiguous ranges of roughly equal
        // packet count, count occupied cells per range, and pre-size `debits`
        // (already reserved to capacity in `Grants::with_capacity`).
        let mut cell_bounds = Vec::with_capacity(runs + 1);
        for run in 0..=runs {
            if run == runs {
                cell_bounds.push(buckets);
            } else {
                let target = items / runs * run + items % runs * run / runs;
                let cell = offsets
                    .partition_point(|&at| (at as usize) < target)
                    .min(buckets);
                cell_bounds.push(cell);
            }
        }
        let mut occ_counts = Vec::with_capacity(runs);
        let mut total_occupied = 0_usize;
        for pair in cell_bounds.windows(2) {
            let mut count = 0_usize;
            for cell in pair[0]..pair[1] {
                if offsets[cell] < offsets[cell + 1] {
                    count += 1;
                }
            }
            occ_counts.push(count);
            total_occupied += count;
        }
        debits.resize(
            total_occupied,
            CellDebit {
                cell: 0,
                energy_before: 0.0,
                energy_after: 0.0,
                demanded: 0.0,
                granted: 0.0,
                contended: false,
            },
        );
        // Split `packets` and `debits` into disjoint per-worker slices.
        struct Slice<'a> {
            cells: std::ops::Range<usize>,
            pkt_base: usize,
            packets: &'a mut [GrantPacket],
            debits: &'a mut [CellDebit],
        }
        let mut slices = Vec::with_capacity(runs);
        let mut rem_pkts = packets.as_mut_slice();
        let mut rem_debits = debits.as_mut_slice();
        for (pair, &occ) in cell_bounds.windows(2).zip(&occ_counts) {
            let pkt_base = offsets.get(pair[0]).map_or(items, |&v| v as usize);
            let pkt_end = offsets.get(pair[1]).map_or(items, |&v| v as usize);
            let pkt_len = pkt_end.saturating_sub(pkt_base);
            let (pkt_head, pkt_tail) = rem_pkts.split_at_mut(pkt_len);
            rem_pkts = pkt_tail;
            let (deb_head, deb_tail) = rem_debits.split_at_mut(occ);
            rem_debits = deb_tail;
            slices.push(Slice {
                cells: pair[0]..pair[1],
                pkt_base,
                packets: pkt_head,
                debits: deb_head,
            });
        }

        let work = |slice: &mut Slice<'_>| -> Result<(), CoreError> {
            let mut local_shares = Vec::new();
            let mut local_rank = Vec::new();
            let mut out_idx = 0_usize;
            for cell in slice.cells.clone() {
                let first = offsets[cell] as usize;
                let end = offsets[cell + 1] as usize;
                if first >= end {
                    continue;
                }
                let value = cell_energy(energy, cell)?;
                let local_start = first - slice.pkt_base;
                let local_end = end - slice.pkt_base;
                let group = &mut slice.packets[local_start..local_end];
                let debit = resolve_cell(group, cell, value, &mut local_shares, &mut local_rank)?;
                slice.debits[out_idx] = debit;
                out_idx += 1;
            }
            Ok(())
        };
        let work = &work;
        let mut iter = slices.iter_mut();
        let head = iter.next();
        std::thread::scope(|scope| {
            let handles: Vec<_> = iter.map(|s| scope.spawn(move || work(s))).collect();
            let mut first_err = head.map_or(Ok(()), work);
            for handle in handles {
                let joined = handle.join().unwrap_or_else(|_| {
                    Err(CoreError::StateConflict {
                        operation: "resolve uptake",
                        detail: "an uptake worker panicked".to_owned(),
                    })
                });
                if first_err.is_ok() {
                    first_err = joined;
                }
            }
            first_err
        })?;

        for debit in debits.iter() {
            let len = (offsets[debit.cell + 1] - offsets[debit.cell]) as usize;
            tally.record(debit, len);
        }
    }
    Ok(finish(grid, demands.len(), grants, tally))
}

/// Check `demands` and stage their packets in identity order.
fn stage(
    grid: &FieldGrid,
    kernel: ExchangeKernel,
    demands: &[Demand],
    grants: &mut Grants,
) -> Result<(), CoreError> {
    if demands.len() > grants.organisms {
        return Err(CoreError::BudgetExhausted {
            budget: UPTAKE_PACKETS,
            required: u64::try_from(demands.len()).unwrap_or(u64::MAX),
            available: u64::try_from(grants.organisms).unwrap_or(u64::MAX),
        });
    }
    let strictly_ascending = demands
        .windows(2)
        .all(|pair| pair[0].organism < pair[1].organism);
    if strictly_ascending {
        for (index, demand) in demands.iter().enumerate() {
            stage_demand(
                grid,
                kernel,
                index,
                demand,
                &mut grants.exchange,
                &mut grants.staged,
            )?;
        }
        return Ok(());
    }
    grants.identities.extend(
        demands
            .iter()
            .enumerate()
            .map(|(index, demand)| (demand.organism, index)),
    );
    grants.identities.sort_unstable();
    if let Some(pair) = grants
        .identities
        .windows(2)
        .find(|pair| pair[0].0 == pair[1].0)
    {
        return Err(CoreError::StateConflict {
            operation: "uptake demand",
            detail: format!(
                "organism {} demands twice (entries {} and {}); an organism cannot be granted the same tick twice",
                pair[0].0.get(),
                pair[0].1,
                pair[1].1
            ),
        });
    }
    // Stage packets in identity order. An organism's footprint names each
    // cell at most once, so grouping them by cell gives (cell, identity)
    // order.
    for &(_, index) in &grants.identities {
        let Some(demand) = demands.get(index) else {
            continue;
        };
        if let Err(error) = stage_demand(
            grid,
            kernel,
            index,
            demand,
            &mut grants.exchange,
            &mut grants.staged,
        ) {
            // Report the first invalid demand in the order given.
            return Err(demands
                .iter()
                .find_map(|item| check_demand(grid, kernel, item, &mut grants.exchange).err())
                .unwrap_or(error));
        }
    }
    Ok(())
}

/// Group the staged packets by cell, in identity order within each cell,
/// into [`Grants::packets`].
fn group_packets(grid: &FieldGrid, grants: &mut Grants, workers: usize) -> Result<(), CoreError> {
    let Grants {
        staged,
        bins,
        packets,
        ..
    } = grants;
    let cells = usize::try_from(grid.cell_count()).map_err(|_| CoreError::Overflow {
        quantity: UPTAKE_PACKETS,
    })?;
    bins.group(cells, staged.len(), |at| {
        staged.get(at).map_or(usize::MAX, |packet| packet.cell)
    })?;
    let order = bins.order();
    let items = order.len();
    let runs = workers.min(items / UPTAKE_PACKETS_PER_RUN).max(1);
    if runs == 1 {
        packets.extend(
            order
                .iter()
                .filter_map(|&at| staged.get(at as usize).copied()),
        );
    } else {
        packets.resize(
            items,
            GrantPacket {
                cell: 0,
                organism: OrganismId::new(0),
                demand: 0,
                requested: 0.0,
                granted: 0.0,
            },
        );
        let staged_ref: &[GrantPacket] = staged;
        let chunk = items.div_ceil(runs);
        let gather = |ord_chunk: &[u32], dst_chunk: &mut [GrantPacket]| {
            for (&at, dst) in ord_chunk.iter().zip(dst_chunk) {
                if let Some(&pkt) = staged_ref.get(at as usize) {
                    *dst = pkt;
                }
            }
        };
        let gather = &gather;
        let mut ord_iter = order.chunks(chunk);
        let mut dst_iter = packets.chunks_mut(chunk);
        let head = ord_iter.next().zip(dst_iter.next());
        let panicked = std::thread::scope(|scope| {
            let handles: Vec<_> = ord_iter
                .zip(dst_iter)
                .map(|(o, d)| scope.spawn(move || gather(o, d)))
                .collect();
            if let Some((o, d)) = head {
                gather(o, d);
            }
            let mut panicked = false;
            for h in handles {
                panicked |= h.join().is_err();
            }
            panicked
        });
        if panicked {
            return Err(CoreError::StateConflict {
                operation: "group uptake packets",
                detail: "a packet gather worker panicked".to_owned(),
            });
        }
    }
    Ok(())
}

/// A footprint cell's energy in `energy`.
fn cell_energy(energy: &[f32], cell: usize) -> Result<f32, CoreError> {
    energy
        .get(cell)
        .copied()
        .ok_or_else(|| CoreError::LayoutMismatch {
            buffer: "field",
            detail: format!("exchange cell {cell} is not in this field"),
        })
}

/// The resolution's totals, cell by cell in ascending order.
#[derive(Debug, Default)]
struct Tally {
    summary: UptakeSummary,
    requested: CompensatedSum,
    granted: CompensatedSum,
}

impl Tally {
    fn record(&mut self, debit: &CellDebit, length: usize) {
        let summary = &mut self.summary;
        self.requested.add(debit.demanded);
        self.granted.add(debit.granted);
        summary.cells += 1;
        summary.contended_cells += usize::from(debit.contended);
        summary.drained_cells += usize::from(debit.granted > 0.0 && debit.energy_after == 0.0);
        summary.largest_cell_packets = summary.largest_cell_packets.max(length);
    }
}

/// Each demand's total and the summary, once every cell is resolved.
fn finish(grid: FieldGrid, demands: usize, grants: &mut Grants, tally: Tally) -> UptakeSummary {
    let mut summary = tally.summary;
    summary.requested = tally.requested.value();
    summary.granted = tally.granted.value();
    grants.totals.resize(demands, 0.0);
    for packet in &grants.packets {
        let total = &mut grants.totals[packet.demand];
        *total = add_down(*total, packet.granted);
    }
    grants.summary = summary;
    grants.grid = Some(grid);
    summary
}

/// A cell's grant under `cell-quantum-largest-remainder-v1`, from its
/// energy and demand.
#[derive(Debug, Clone, Copy)]
struct CellTarget {
    /// `Q_c`.
    quantum: f64,
    /// `n_c`.
    quanta: u64,
    /// `A_c > e_c`.
    contended: bool,
    /// `K_c`: every quantum when contended, else the demand in quanta
    /// rounded to even and at most `n_c`.
    target: u64,
    /// What a packet's request is multiplied by for its ideal share in
    /// quanta.
    scale: f64,
}

fn cell_target(energy: f32, demanded: f64) -> CellTarget {
    let contended = demanded > f64::from(energy);
    let (quantum, quanta) = quanta_of(energy);
    if quanta == 0 {
        return CellTarget {
            quantum,
            quanta,
            contended,
            target: 0,
            scale: 0.0,
        };
    }
    // `quanta < 2^24`, so these conversions are exact.
    let whole = quanta as f64;
    let (target, scale) = if contended {
        (quanta, whole / demanded)
    } else {
        // Uncontended: `A_c ≤ e_c`, so the rounded quotient is at most
        // `n_c`; `min` guards the edge.
        let ideal = (demanded / quantum).round_ties_even().min(whole);
        (ideal as u64, 1.0 / quantum)
    };
    CellTarget {
        quantum,
        quanta,
        contended,
        target,
        scale,
    }
}

/// The debit of a cell that grants its target.
fn debit_of(cell: usize, energy: f32, demanded: f64, at: &CellTarget) -> CellDebit {
    if at.quanta == 0 {
        return CellDebit {
            cell,
            energy_before: energy,
            energy_after: energy,
            demanded,
            granted: 0.0,
            contended: at.contended,
        };
    }
    CellDebit {
        cell,
        energy_before: energy,
        // `(n_c − K_c) Q_c` is an integer below 2^24 times a power of two
        // at least 2^-149, so it is exact in F32.
        energy_after: ((at.quanta - at.target) as f64 * at.quantum) as f32,
        demanded,
        granted: at.target as f64 * at.quantum,
        contended: at.contended,
    }
}

/// A cell's energy as the resolver admits it.
fn check_energy(cell: usize, energy: f32) -> Result<(), CoreError> {
    if !energy.is_finite() || energy < 0.0 {
        return Err(CoreError::NumericalInvalid {
            quantity: "field energy",
            detail: format!("cell {cell} holds {energy}"),
        });
    }
    Ok(())
}

/// Cells with at most this many competing packets resolve on the stack with
/// zero heap allocation.
const STACK_CELL_PACKETS: usize = 32;

/// Resolve one cell's packets, which are in identity order.
fn resolve_cell(
    packets: &mut [GrantPacket],
    cell: usize,
    energy: f32,
    shares_buf: &mut Vec<Share>,
    rank_buf: &mut Vec<usize>,
) -> Result<CellDebit, CoreError> {
    check_energy(cell, energy)?;
    let mut demand = CompensatedSum::ZERO;
    for packet in packets.iter() {
        demand.add(packet.requested);
    }
    let demanded = demand.value();
    if !demanded.is_finite() || demanded < 0.0 {
        return Err(CoreError::NumericalInvalid {
            quantity: "cell uptake demand",
            detail: format!("cell {cell} demand sum {demanded} is not finite and nonnegative"),
        });
    }
    let at = cell_target(energy, demanded);
    if at.quanta == 0 {
        return Ok(debit_of(cell, energy, demanded, &at));
    }
    let CellTarget {
        quantum,
        target,
        scale,
        ..
    } = at;

    let len = packets.len();
    let mut stack_shares = [Share {
        quanta: 0,
        remainder: 0.0,
    }; STACK_CELL_PACKETS];
    let mut stack_rank = [0_usize; STACK_CELL_PACKETS];
    let (shares, rank): (&mut [Share], &mut [usize]) = if len <= STACK_CELL_PACKETS {
        (&mut stack_shares[..len], &mut stack_rank[..len])
    } else {
        shares_buf.clear();
        shares_buf.resize(
            len,
            Share {
                quanta: 0,
                remainder: 0.0,
            },
        );
        rank_buf.clear();
        rank_buf.resize(len, 0);
        (&mut shares_buf[..len], &mut rank_buf[..len])
    };

    let mut floors = 0_u64;
    for (i, packet) in packets.iter().enumerate() {
        let ideal = packet.requested * scale;
        let floor = ideal.floor();
        // `ideal ≤ n_c (1 + rounding) < 2^25`.
        let share = floor as u64;
        floors += share;
        shares[i] = Share {
            quanta: share,
            remainder: ideal - floor,
        };
    }
    if floors != target {
        for (i, slot) in rank.iter_mut().enumerate() {
            *slot = i;
        }
        if floors < target {
            // Largest remainder first; ties to the lower identity.
            rank.sort_unstable_by(|&a, &b| {
                shares[b]
                    .remainder
                    .total_cmp(&shares[a].remainder)
                    .then(a.cmp(&b))
            });
            let mut deficit = target - floors;
            while deficit > 0 {
                for &index in rank.iter() {
                    if deficit == 0 {
                        break;
                    }
                    shares[index].quanta += 1;
                    deficit -= 1;
                }
            }
        } else {
            // Smallest remainder first; ties to the higher identity.
            rank.sort_unstable_by(|&a, &b| {
                shares[a]
                    .remainder
                    .total_cmp(&shares[b].remainder)
                    .then(b.cmp(&a))
            });
            let mut surplus = floors - target;
            while surplus > 0 {
                for &index in rank.iter() {
                    if surplus == 0 {
                        break;
                    }
                    if shares[index].quanta > 0 {
                        shares[index].quanta -= 1;
                        surplus -= 1;
                    }
                }
            }
        }
    }
    for (packet, share) in packets.iter_mut().zip(shares.iter()) {
        packet.granted = share.quanta as f64 * quantum;
    }
    Ok(debit_of(cell, energy, demanded, &at))
}

/// A nonnegative F32 energy as `(Q, n)` with `energy = n Q` exactly: `Q` is
/// its unit in the last place and `n < 2^24`.
fn quanta_of(energy: f32) -> (f64, u64) {
    let bits = energy.to_bits();
    let exponent = (bits >> 23) & 0xff;
    let fraction = u64::from(bits & 0x7f_ffff);
    let quanta = if exponent == 0 {
        fraction
    } else {
        fraction | 0x80_0000
    };
    // `Q = 2^(max(exponent, 1) − 150)`: biased F64 exponent
    // `max(exponent, 1) + 873`.
    let quantum = f64::from_bits(u64::from(exponent.max(1) + 873) << 52);
    (quantum, quanta)
}

/// Debit `field` by `grants`: each cell's energy becomes its debit's
/// `energy_after`, and its moment is scaled by `e'_c/e_c` (exactly zero when
/// the cell is drained). Returns the energy debited. This is
/// [`apply_grants_on`] on one thread.
///
/// # Errors
/// As [`apply_grants_on`].
pub fn apply_grants(field: &mut FieldState, grants: &mut Grants) -> Result<f64, CoreError> {
    apply_grants_on(field, grants, 1)
}

/// [`apply_grants`] on at most `workers` threads: this one and scoped
/// workers it joins before returning. Every check runs first, on this
/// thread, and the energy plane is written here. The moment planes are then
/// split into contiguous runs of whole planes, one run per thread, and each
/// plane is written exactly as one thread writes it. A cell's new values
/// depend only on its debit and its own old values, so the field is the
/// same at any worker count.
///
/// # Errors
/// [`CoreError::StateConflict`] when the grants were already applied, were
/// resolved for another grid, or any debited cell no longer holds the
/// energy they were resolved against. Nothing is written.
pub fn apply_grants_on(
    field: &mut FieldState,
    grants: &mut Grants,
    workers: usize,
) -> Result<f64, CoreError> {
    let cells = grants.check_debit(field)?;
    let conflict = |detail: String| CoreError::StateConflict {
        operation: "uptake grants",
        detail,
    };
    let energy = field.energy_mut();
    for debit in grants.debits.iter().filter(|debit| debit.granted > 0.0) {
        energy[debit.cell] = debit.energy_after;
    }
    let debits: &[CellDebit] = &grants.debits;
    let scale = |planes: &mut [f32]| {
        const CHUNK: usize = 64;
        let mut batch = [(0_usize, 0.0_f64); CHUNK];
        let mut batch_len = 0_usize;
        let flush = |planes: &mut [f32], batch: &[(usize, f64)]| {
            for plane in planes.chunks_exact_mut(cells.max(1)) {
                for &(cell, ratio) in batch {
                    let moment = &mut plane[cell];
                    *moment = if ratio == 0.0 {
                        0.0
                    } else {
                        (f64::from(*moment) * ratio) as f32
                    };
                }
            }
        };
        for debit in debits.iter().filter(|debit| debit.granted > 0.0) {
            let ratio = if debit.energy_after == 0.0 {
                0.0
            } else {
                f64::from(debit.energy_after) / f64::from(debit.energy_before)
            };
            batch[batch_len] = (debit.cell, ratio);
            batch_len += 1;
            if batch_len == CHUNK {
                flush(planes, &batch[..batch_len]);
                batch_len = 0;
            }
        }
        if batch_len > 0 {
            flush(planes, &batch[..batch_len]);
        }
    };
    let used = workers.clamp(1, CHANNELS);
    let per = CHANNELS.div_ceil(used);
    let mut runs = field.moments_mut().chunks_mut(per * cells.max(1));
    let head = runs.next().unwrap_or_default();
    let scale = &scale;
    let panicked = std::thread::scope(|scope| {
        let workers: Vec<_> = runs
            .map(|planes| scope.spawn(move || scale(planes)))
            .collect();
        scale(head);
        // Join every worker, so that none is left for the scope to find
        // panicked.
        let mut panicked = false;
        for worker in workers {
            panicked |= worker.join().is_err();
        }
        panicked
    });
    if panicked {
        return Err(conflict("a grant worker panicked".into()));
    }
    grants.applied = true;
    Ok(grants.summary.granted)
}

/// Grants checked for a debit on another device (FL-032); see
/// [`Grants::debit`].
#[derive(Debug)]
pub struct GrantDebit<'a> {
    grants: &'a mut Grants,
}

impl GrantDebit<'_> {
    /// Every cell with demand, in ascending cell order.
    #[must_use]
    pub fn debits(&self) -> &[CellDebit] {
        &self.grants.debits
    }

    /// Mark the grants applied, once the other device has written their
    /// debit into the field.
    pub fn finish(self) {
        self.grants.applied = true;
    }
}

/// A.1's saturating request rule with its run's constants: what phase 5's
/// demand is computed by, on the reference ([`demands`]) or a backend
/// ([`demands_on`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RequestRule {
    /// The tick's step.
    pub step: TimeStep,
    /// `U_max`, ecological-energy per simulation-second.
    pub capacity_per_second: f64,
    /// `K`, the half-saturation density.
    pub half_saturation_density: f64,
}

impl RequestRule {
    /// [`saturating_request`] at `density`.
    ///
    /// # Errors
    /// As [`saturating_request`].
    pub fn request(&self, density: f64) -> Result<f64, CoreError> {
        saturating_request(
            self.step,
            self.capacity_per_second,
            self.half_saturation_density,
            density,
        )
    }
}

/// Phase 5's demands on the reference under `grid-cic-v1`: for each
/// `(organism, position)`, in order, its [`demand`].
///
/// # Errors
/// The first organism's footprint, density or request refusal, in order.
pub fn demands(
    field: &FieldState,
    rule: &RequestRule,
    requesters: impl IntoIterator<Item = (OrganismId, [f64; 2])>,
    demands: &mut Vec<Demand>,
) -> Result<(), CoreError> {
    demands_with(field, rule, ExchangeKernel::GridCicV1, requesters, demands)
}

/// Phase 5's demands on the reference under `kernel` (FL-053).
///
/// # Errors
/// The first organism's footprint, density or request refusal, in order.
pub fn demands_with(
    field: &FieldState,
    rule: &RequestRule,
    kernel: ExchangeKernel,
    requesters: impl IntoIterator<Item = (OrganismId, [f64; 2])>,
    demands: &mut Vec<Demand>,
) -> Result<(), CoreError> {
    demands.clear();
    let mut scratch = ExchangeScratch::for_kernel(kernel, field.grid())?;
    for (organism, position) in requesters {
        demands.push(demand_with_scratch(
            field,
            rule,
            kernel,
            organism,
            position,
            &mut scratch,
        )?);
    }
    Ok(())
}

/// One organism's phase 5 [`Demand`] on the reference under `grid-cic-v1`, at
/// its footprint's density:
/// `rule.request(ExchangeStencil::at(grid, position)?.density(field)?)`. It
/// reads only `field`, so callers may compute many at once, each into its own
/// slot.
///
/// # Errors
/// The footprint's, the density's or the request's refusal.
pub fn demand(
    field: &FieldState,
    rule: &RequestRule,
    organism: OrganismId,
    position: [f64; 2],
) -> Result<Demand, CoreError> {
    demand_with(field, rule, ExchangeKernel::GridCicV1, organism, position)
}

/// One organism's phase 5 [`Demand`] under `kernel` (FL-053).
///
/// # Errors
/// As [`demand`].
pub fn demand_with(
    field: &FieldState,
    rule: &RequestRule,
    kernel: ExchangeKernel,
    organism: OrganismId,
    position: [f64; 2],
) -> Result<Demand, CoreError> {
    let mut scratch = ExchangeScratch::new();
    demand_with_scratch(field, rule, kernel, organism, position, &mut scratch)
}

/// One organism's phase 5 [`Demand`] under `kernel`, reusing `scratch`
/// (FL-053).
///
/// # Errors
/// As [`demand`].
pub fn demand_with_scratch(
    field: &FieldState,
    rule: &RequestRule,
    kernel: ExchangeKernel,
    organism: OrganismId,
    position: [f64; 2],
    scratch: &mut ExchangeScratch,
) -> Result<Demand, CoreError> {
    let density = kernel.density_with_scratch(field, position, scratch)?;
    Ok(Demand {
        organism,
        position,
        amount: rule.request(density)?,
    })
}

/// One occupied cell for [`UptakeBackend::resolve`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UptakeCell {
    /// Its first packet in the requests the backend is given.
    pub start: usize,
    /// Its packets, in identity order: at least one.
    pub len: usize,
    /// Its energy, finite and nonnegative.
    pub energy: f32,
}

/// FL-031's port for a device phase 5. Each method must give the
/// reference's values bit for bit; the host keeps everything else (see the
/// module documentation).
pub trait UptakeBackend: std::fmt::Debug + Send {
    /// The backend and its device, for run records.
    fn describe(&self) -> String;

    /// For each of `stencils` (an organism's footprint on `field`), its
    /// density, [`ExchangeStencil::density`], and the request
    /// `rule.request` makes at that density: `densities[i]` and
    /// `requests[i]`. The caller checks the densities and the rule.
    ///
    /// # Errors
    /// Any. The world then computes the tick's phase 5 on the reference.
    fn request(
        &mut self,
        field: &FieldState,
        rule: &RequestRule,
        stencils: &[ExchangeStencil],
        densities: &mut Vec<f64>,
        requests: &mut Vec<f64>,
    ) -> Result<(), CoreError>;

    /// For each of `cells`, whose packets request
    /// `requested[start..start + len]`, its demand `A_c` (their
    /// [`CompensatedSum`] in order) in `demanded`, and each packet's whole
    /// quanta of its cell under `cell-quantum-largest-remainder-v1` in
    /// `quanta`, one per request.
    ///
    /// # Errors
    /// Any. The world then computes the tick's phase 5 on the reference.
    fn resolve(
        &mut self,
        cells: &[UptakeCell],
        requested: &[f64],
        demanded: &mut Vec<f64>,
        quanta: &mut Vec<u32>,
    ) -> Result<(), CoreError>;
}

/// The host's lists for a backend's phase 5, reusable from tick to tick.
#[derive(Debug, Default)]
pub struct UptakeWork {
    stencils: Vec<ExchangeStencil>,
    densities: Vec<f64>,
    requests: Vec<f64>,
    cells: Vec<UptakeCell>,
    requested: Vec<f64>,
    demanded: Vec<f64>,
    quanta: Vec<u32>,
}

impl UptakeWork {
    /// Empty lists.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Room for `organisms` demands on a grid of `cells` cells, so that a
    /// tick within them allocates nothing (FL-043).
    ///
    /// # Errors
    /// [`CoreError::AllocationFailed`] when the host refuses the room.
    pub fn reserve(&mut self, organisms: usize, cells: usize) -> Result<(), CoreError> {
        let packets = organisms.saturating_mul(PACKETS_PER_DEMAND as usize);
        let occupied = packets.min(cells);
        fit(&mut self.stencils, organisms, "uptake footprints")?;
        fit(&mut self.densities, organisms, "uptake densities")?;
        fit(&mut self.requests, organisms, "uptake requests")?;
        fit(&mut self.cells, occupied, "uptake cells")?;
        fit(&mut self.requested, packets, "uptake packet requests")?;
        fit(&mut self.demanded, occupied, "uptake cell demands")?;
        fit(&mut self.quanta, packets, "uptake packet quanta")
    }

    /// Bytes its lists hold (FL-043).
    #[must_use]
    pub fn allocated_bytes(&self) -> u64 {
        total([
            capacity_bytes(&self.stencils),
            capacity_bytes(&self.densities),
            capacity_bytes(&self.requests),
            capacity_bytes(&self.cells),
            capacity_bytes(&self.requested),
            capacity_bytes(&self.demanded),
            capacity_bytes(&self.quanta),
        ])
    }
}

/// `list` emptied and holding room for exactly `count`.
fn fit<T>(list: &mut Vec<T>, count: usize, buffer: &'static str) -> Result<(), CoreError> {
    list.clear();
    list.shrink_to(count);
    list.try_reserve_exact(count)
        .map_err(|_| CoreError::AllocationFailed {
            buffer,
            bytes: bytes_for::<T>(count),
        })
}

fn backend_conflict(detail: String) -> CoreError {
    CoreError::StateConflict {
        operation: "uptake backend",
        detail,
    }
}

/// [`demands`] with each density and request computed on `backend`, the
/// footprints and checks on the host: the same demands, bit for bit.
///
/// # Errors
/// A footprint, density or rule refusal, or anything the backend meets. The
/// order may differ from [`demands`]'s, so the caller recomputes on the
/// reference to report the reference's own error.
pub fn demands_on(
    backend: &mut dyn UptakeBackend,
    field: &FieldState,
    rule: &RequestRule,
    requesters: impl IntoIterator<Item = (OrganismId, [f64; 2])>,
    work: &mut UptakeWork,
    demands: &mut Vec<Demand>,
) -> Result<(), CoreError> {
    demands.clear();
    work.stencils.clear();
    for (organism, position) in requesters {
        work.stencils
            .push(ExchangeStencil::at(field.grid(), position)?);
        demands.push(Demand {
            organism,
            position,
            amount: 0.0,
        });
    }
    let UptakeWork {
        stencils,
        densities,
        requests,
        ..
    } = work;
    backend.request(field, rule, stencils, densities, requests)?;
    if densities.len() != demands.len() || requests.len() != demands.len() {
        return Err(backend_conflict(format!(
            "{} densities and {} requests for {} organisms",
            densities.len(),
            requests.len(),
            demands.len()
        )));
    }
    for ((demand, &density), &request) in demands
        .iter_mut()
        .zip(densities.iter())
        .zip(requests.iter())
    {
        check_request(
            rule.capacity_per_second,
            rule.half_saturation_density,
            density,
        )?;
        demand.amount = request;
    }
    Ok(())
}

/// [`resolve_uptake`] with each cell's demand and grants computed on
/// `backend`: the same grants, bit for bit. The host stages and groups the
/// packets, checks every cell's energy, and records each debit from the
/// backend's demand and quanta, refusing quanta that do not sum to the
/// cell's grant (INV-04).
///
/// # Errors
/// As [`resolve_uptake`], or anything the backend meets. On error `grants`
/// is left empty.
pub fn resolve_uptake_on(
    backend: &mut dyn UptakeBackend,
    field: &FieldState,
    demands: &[Demand],
    work: &mut UptakeWork,
    grants: &mut Grants,
) -> Result<UptakeSummary, CoreError> {
    grants.reset();
    let result = resolve_on(backend, field, demands, work, grants);
    if result.is_err() {
        grants.reset();
    }
    result
}

fn resolve_on(
    backend: &mut dyn UptakeBackend,
    field: &FieldState,
    demands: &[Demand],
    work: &mut UptakeWork,
    grants: &mut Grants,
) -> Result<UptakeSummary, CoreError> {
    let grid = *field.grid();
    stage(&grid, ExchangeKernel::GridCicV1, demands, grants)?;
    group_packets(&grid, grants, 1)?;
    let energy = field.energy();
    let UptakeWork {
        cells,
        requested,
        demanded,
        quanta,
        ..
    } = work;
    cells.clear();
    for (cell, range) in grants.bins.occupied() {
        let value = cell_energy(energy, cell)?;
        check_energy(cell, value)?;
        cells.push(UptakeCell {
            start: range.start,
            len: range.len(),
            energy: value,
        });
    }
    requested.clear();
    requested.extend(grants.packets.iter().map(|packet| packet.requested));
    backend.resolve(cells, requested, demanded, quanta)?;
    if demanded.len() != cells.len() || quanta.len() != requested.len() {
        return Err(backend_conflict(format!(
            "{} demands and {} quanta for {} cells and {} packets",
            demanded.len(),
            quanta.len(),
            cells.len(),
            requested.len()
        )));
    }
    let mut tally = Tally::default();
    let Grants {
        bins,
        packets,
        debits,
        ..
    } = grants;
    for (((cell, range), occupied), &demand) in
        bins.occupied().zip(cells.iter()).zip(demanded.iter())
    {
        let at = cell_target(occupied.energy, demand);
        let (Some(shares), Some(group)) = (quanta.get(range.clone()), packets.get_mut(range))
        else {
            return Err(backend_conflict(format!(
                "cell {cell}'s packets are out of range"
            )));
        };
        let granted: u64 = shares.iter().map(|&share| u64::from(share)).sum();
        if granted != at.target {
            return Err(backend_conflict(format!(
                "cell {cell} granted {granted} quanta, not its {}",
                at.target
            )));
        }
        if at.quanta > 0 {
            for (packet, &share) in group.iter_mut().zip(shares) {
                packet.granted = f64::from(share) * at.quantum;
            }
        }
        let debit = debit_of(cell, occupied.energy, demand, &at);
        tally.record(&debit, group.len());
        debits.push(debit);
    }
    Ok(finish(grid, demands.len(), grants, tally))
}
