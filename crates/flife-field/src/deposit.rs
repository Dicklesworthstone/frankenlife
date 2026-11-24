//! Paid deposits reduced into the next field (plan §5.5, §8.1 phase 9,
//! INV-03, INV-05, Appendix C.4).
//!
//! An organism that paid `w` for an emission offers `ρ w` to the field at
//! its post-motion position. The offer is spread over the `grid-cic-v1`
//! footprint as one packet per cell, and energy and every moment component
//! use that same packet: cell `c` receives `(q_c, q_c a)` for chemical
//! direction `a`. Every cell except the one with the largest weight (the
//! lowest index on a tie) receives `ρ w w_c`, rounded to nearest. The
//! largest-weight cell receives the remainder, rounded down. So
//! `Σ_c q_c ≤ ρ w` holds exactly, and no packet is negative, because the
//! largest weight is at least a quarter.
//!
//! The reduction:
//! - Packets are grouped by cell, in organism identity order within each
//!   cell, by [`Bins`]' counting sort (FL-027): `O(cells + packets)` and
//!   bounded scratch, however the organisms cluster. Each cell sums its
//!   packets in that order, rounding every partial sum down, so its total
//!   `S_c` never exceeds the exact sum.
//! - The cell's new energy `e'_c` is the largest F32 value at or below
//!   `e_c + S_c`. It is never above the exact sum, so a deposit never
//!   creates energy. The part the F32 cell cannot hold is dissipated, the
//!   same rule as an uptake request below half a quantum.
//! - The cell received `δ_c = e'_c − e_c`. This difference of two F32 values
//!   is exact in F64 whenever the cell was empty or held at least `2^-28`
//!   of its new energy; otherwise it is rounded to nearest.
//! - The moment gains `(δ_c / S_c) Σ q a` per channel, accumulated in
//!   packet order. The composition shrinks with the energy it accompanies,
//!   so `||Δm_c||₂ ≤ δ_c` holds in exact arithmetic for directions in the
//!   unit ball. Each component is rounded once to F32, to nearest.
//! - The ledger records one internal transfer per reduction: debited
//!   `Σ w`, what the organisms paid, and credited `Σ δ_c`, what the cells
//!   received. Its loss, `(1 − ρ) Σ w` plus whatever the cells could not
//!   hold, is the transfer's named dissipation flow.
//!
//! The result depends only on organism identities, never on push order or
//! storage slot. Deposits are written to the next state after that tick's
//! uptake debits. `crate::uptake::apply_grants` refuses a cell that no
//! longer holds its snapshot energy, so the order cannot be inverted. The
//! tick's observation epoch is never written: a deposit is first sensed,
//! and first absorbable, on the next tick, by every organism, its emitter
//! included.

use std::ops::Range;

use flife_core::CoreError;
use flife_core::ids::OrganismId;
use flife_core::ledger::{CompensatedSum, TickLedger, Transfer, add_down, sub_down};
use flife_core::memory::{capacity_bytes, total};

use crate::bins::Bins;
use crate::exchange::{ExchangeKernel, ExchangeScratch, ExchangeStencil};
use crate::footprint::BufferLimits;
use crate::grid::{CARRIER0_MOMENT_CHANNELS, CARRIER1_MOMENT_CHANNELS, CarrierLayout, FieldGrid};
use crate::state::{CHANNELS, FieldState};

/// The name deposits' scratch refusals carry.
const DEPOSIT_PACKETS: &str = "deposit packets";

/// The most packets one deposit becomes: its footprint has at most four
/// cells.
const PACKETS_PER_DEPOSIT: u64 = 4;

/// One organism's staged deposit.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Entry {
    organism: OrganismId,
    paid: f64,
    offered: f64,
    direction: [f64; CHANNELS],
    /// Its packets: `packets[first..first + count]`.
    first: usize,
    count: usize,
}

/// One cell's share of an entry.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Packet {
    cell: usize,
    organism: OrganismId,
    entry: usize,
    amount: f64,
}

/// What one reduction paid, offered and applied.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DepositSummary {
    /// Organisms with a staged deposit, including any whose offer was zero.
    pub organisms: usize,
    /// Distinct cells that received a packet.
    pub cells: usize,
    /// `Σ w`: what the organisms paid.
    pub paid: f64,
    /// `Σ ρ w`: what they offered to the field.
    pub offered: f64,
    /// `Σ δ_c`: what the cells received.
    pub applied: f64,
}

impl DepositSummary {
    /// Record the reduction as one `transfer`, debiting what the organisms
    /// paid and crediting what the cells received. Returns the loss charged
    /// to the transfer's dissipation flow.
    ///
    /// In exact arithmetic `applied ≤ offered ≤ paid`. The two sums are
    /// compensated but summed over different sets of terms, so they can
    /// round past each other by one unit in the last place when nothing was
    /// lost at all. In that case the credit is recorded as `paid`. The
    /// ledger closure measures holdings, so this one-ulp difference shows up
    /// in its residual and is not hidden.
    ///
    /// # Errors
    /// As [`TickLedger::transfer`].
    pub fn record(&self, ledger: &mut TickLedger, transfer: Transfer) -> Result<f64, CoreError> {
        ledger.transfer(transfer, self.paid, self.applied.min(self.paid))
    }
}

/// Deposits staged during one tick, reusable from tick to tick so that
/// steady-state ticks do not allocate (§9.4).
#[derive(Debug, Clone)]
pub struct Deposits {
    grid: Option<FieldGrid>,
    entries: Vec<Entry>,
    packets: Vec<Packet>,
    /// What each occupied cell of the last reduction received: a slot a
    /// packet, so that each run of cells has its own.
    received: Vec<f64>,
    /// Each entry's organism and index, sorted by organism.
    identities: Vec<(OrganismId, usize)>,
    /// Packet indices in organism identity order, before grouping by cell.
    sequence: Vec<u32>,
    bins: Bins,
    /// The most deposits one reduction admits.
    organisms: usize,
    applied: bool,
    exchange: ExchangeScratch,
    shares: Vec<f64>,
}

impl Default for Deposits {
    fn default() -> Self {
        Self::new()
    }
}

impl Deposits {
    /// Bytes its entries, packets, received amounts and reduction scratch hold
    /// (FL-043).
    #[must_use]
    pub fn allocated_bytes(&self) -> u64 {
        total([
            capacity_bytes(&self.entries),
            capacity_bytes(&self.packets),
            capacity_bytes(&self.received),
            capacity_bytes(&self.identities),
            capacity_bytes(&self.sequence),
            self.bins.allocated_bytes(),
            self.exchange.allocated_bytes(),
            capacity_bytes(&self.shares),
        ])
    }

    /// No deposits, with no declared capacity: their scratch grows on demand
    /// within the `u32` index range.
    #[must_use]
    pub fn new() -> Self {
        Self {
            grid: None,
            entries: Vec::new(),
            packets: Vec::new(),
            received: Vec::new(),
            identities: Vec::new(),
            sequence: Vec::new(),
            bins: Bins::new(DEPOSIT_PACKETS),
            organisms: usize::try_from(u32::MAX).unwrap_or(usize::MAX) / 4,
            applied: false,
            exchange: ExchangeScratch::new(),
            shares: Vec::new(),
        }
    }

    /// No deposits, declared for at most `organisms` deposits a reduction on
    /// `grid` under `grid-cic-v1` (§9.4). Their scratch is admitted against
    /// `limits` and allocated now, so staging and reducing within the
    /// declaration never allocates, and staging beyond it is refused.
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

    /// No deposits, declared for at most `organisms` deposits a reduction on
    /// `grid` under `kernel` (FL-053, §9.4, FL-043).
    ///
    /// # Errors
    /// As [`Deposits::with_capacity`], plus [`CoreError::AllocationFailed`] if
    /// footprint scratch cannot be reserved.
    pub fn with_capacity_for(
        grid: &FieldGrid,
        organisms: u64,
        kernel: ExchangeKernel,
        limits: &BufferLimits,
    ) -> Result<Self, CoreError> {
        let overflow = || CoreError::Overflow {
            quantity: DEPOSIT_PACKETS,
        };
        let max_cells = kernel.max_cells(grid);
        let packets_per_deposit = u64::try_from(max_cells).map_err(|_| overflow())?;
        let packets = organisms
            .checked_mul(packets_per_deposit)
            .ok_or_else(overflow)?;
        let entry_bytes = organisms
            .checked_mul(std::mem::size_of::<Entry>() as u64)
            .ok_or_else(overflow)?;
        let received_bytes = packets
            .checked_mul(std::mem::size_of::<f64>() as u64)
            .ok_or_else(overflow)?;
        limits.admit(DEPOSIT_PACKETS, entry_bytes)?;
        limits.admit(DEPOSIT_PACKETS, received_bytes)?;
        let bins = Bins::with_capacity(DEPOSIT_PACKETS, grid.cell_count(), packets, limits)?;
        let exchange = ExchangeScratch::for_kernel(kernel, grid)?;
        let shares = if kernel.is_grid_cic() {
            Vec::new()
        } else {
            Vec::with_capacity(max_cells)
        };
        let organisms = usize::try_from(organisms).map_err(|_| overflow())?;
        let packets = usize::try_from(packets).map_err(|_| overflow())?;
        Ok(Self {
            entries: Vec::with_capacity(organisms),
            packets: Vec::with_capacity(packets),
            received: Vec::with_capacity(packets),
            identities: Vec::with_capacity(organisms),
            sequence: Vec::with_capacity(packets),
            bins,
            organisms,
            exchange,
            shares,
            ..Self::new()
        })
    }

    /// Forget every staged deposit, keeping the buffers.
    pub fn clear(&mut self) {
        self.grid = None;
        self.entries.clear();
        self.packets.clear();
        self.received.clear();
        self.identities.clear();
        self.sequence.clear();
        self.bins.clear();
        self.applied = false;
    }

    /// The most deposits one reduction admits.
    #[must_use]
    pub const fn organisms(&self) -> usize {
        self.organisms
    }

    /// The cell bins of the last reduction: positions in the reduction's
    /// identity-ordered packet sequence, grouped by cell (FL-027).
    #[must_use]
    pub const fn bins(&self) -> &Bins {
        &self.bins
    }

    /// The allocated capacity of every scratch buffer, in entries: equal
    /// before and after staging and reducing within the declaration.
    #[must_use]
    pub fn allocated(&self) -> [usize; 7] {
        let (offsets, order) = self.bins.allocated();
        [
            self.entries.capacity(),
            self.packets.capacity(),
            self.received.capacity(),
            self.identities.capacity(),
            self.sequence.capacity(),
            offsets,
            order,
        ]
    }

    /// Staged deposits.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is staged.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether these deposits were applied.
    #[must_use]
    pub const fn is_applied(&self) -> bool {
        self.applied
    }

    /// Staged deposit `index`'s organism, payment and offer, in staging
    /// order. A device's emission is compared with these (FL-032).
    #[must_use]
    pub fn entry(&self, index: usize) -> Option<(OrganismId, f64, f64)> {
        self.entries
            .get(index)
            .map(|entry| (entry.organism, entry.paid, entry.offered))
    }

    /// Staged deposit `index`'s packets, `(cell, amount)`, in staging order;
    /// none for an index past the staged deposits.
    pub fn entry_packets(&self, index: usize) -> impl Iterator<Item = (usize, f64)> + '_ {
        let packets = self
            .entries
            .get(index)
            .and_then(|entry| self.packets.get(entry.first..entry.first + entry.count))
            .unwrap_or_default();
        packets.iter().map(|packet| (packet.cell, packet.amount))
    }

    /// Stage `organism`'s deposit. It paid `paid` and offers `offered ≤ paid`
    /// to the cells of its footprint at `position` (wrapped onto the torus),
    /// with chemical `direction`, `||direction||₂ ≤ 1`.
    ///
    /// # Errors
    /// - [`CoreError::StateConflict`] when the deposits were already
    ///   applied, or were staged for another grid.
    /// - [`CoreError::BudgetExhausted`] for more deposits than these were
    ///   declared for ([`Deposits::with_capacity`]).
    /// - [`CoreError::NumericalInvalid`] for a nonfinite or negative amount,
    ///   an offer above the payment, a nonfinite direction or one outside
    ///   the unit ball (squares summed in channel order), or a nonfinite
    ///   position.
    ///
    /// Nothing is staged on error.
    pub fn push(
        &mut self,
        grid: &FieldGrid,
        organism: OrganismId,
        position: [f64; 2],
        paid: f64,
        offered: f64,
        direction: &[f64; CHANNELS],
    ) -> Result<(), CoreError> {
        self.push_with(
            grid,
            ExchangeKernel::GridCicV1,
            organism,
            position,
            paid,
            offered,
            direction,
        )
    }

    /// Stage `organism`'s deposit under `kernel` (FL-053).
    ///
    /// # Errors
    /// As [`Deposits::push`]. Nothing is staged on error.
    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors Deposits::push with the explicit exchange kernel"
    )]
    pub fn push_with(
        &mut self,
        grid: &FieldGrid,
        kernel: ExchangeKernel,
        organism: OrganismId,
        position: [f64; 2],
        paid: f64,
        offered: f64,
        direction: &[f64; CHANNELS],
    ) -> Result<(), CoreError> {
        self.check_entry(grid, paid, offered, direction)?;
        match kernel {
            ExchangeKernel::GridCicV1 => {
                let stencil = ExchangeStencil::at(grid, position)?;
                let shares = split(&stencil, offered);
                self.stage(
                    grid,
                    organism,
                    paid,
                    offered,
                    direction,
                    stencil.cells().iter().copied().zip(shares),
                );
            }
            ExchangeKernel::FixedWorldGaussianV1 { .. } => {
                kernel.footprint_into(grid, position, &mut self.exchange)?;
                split_slice(self.exchange.cells(), offered, &mut self.shares);
                let entry = self.entries.len();
                self.grid = Some(*grid);
                let first = self.packets.len();
                for (&(cell, _), &amount) in self.exchange.cells().iter().zip(&self.shares) {
                    if amount > 0.0 {
                        self.packets.push(Packet {
                            cell,
                            organism,
                            entry,
                            amount,
                        });
                    }
                }
                self.entries.push(Entry {
                    organism,
                    paid: paid + 0.0,
                    offered: offered + 0.0,
                    direction: *direction,
                    first,
                    count: self.packets.len() - first,
                });
            }
        }
        Ok(())
    }

    /// Stage `organism`'s deposit as [`Deposits::push`] would, with the
    /// footprint split a backend computed: `packets`, `(cell, amount)` in
    /// increasing cell order, are the cells of the split that receive a
    /// positive amount (FL-032). The host cannot redo the split without
    /// the position, so it checks what the split guarantees: at most four
    /// cells of `grid`, each amount finite and positive, and their sum,
    /// rounded down, at most the offer.
    ///
    /// # Errors
    /// As [`Deposits::push`], and [`CoreError::StateConflict`] for packets
    /// the split cannot give.
    ///
    /// Nothing is staged on error.
    pub fn push_split(
        &mut self,
        grid: &FieldGrid,
        organism: OrganismId,
        paid: f64,
        offered: f64,
        direction: &[f64; CHANNELS],
        packets: &[(usize, f64)],
    ) -> Result<(), CoreError> {
        self.check_entry(grid, paid, offered, direction)?;
        let refuse = |detail: String| CoreError::StateConflict {
            operation: "stage a backend's deposit split",
            detail,
        };
        if packets.len() > 4 {
            return Err(refuse(format!("{} packets", packets.len())));
        }
        let mut sum = 0.0;
        let mut previous = None;
        for &(cell, amount) in packets {
            if u64::try_from(cell).map_or(true, |cell| cell >= grid.cell_count())
                || previous.is_some_and(|previous| cell <= previous)
            {
                return Err(refuse(format!(
                    "cell {cell} out of order or outside the grid in {packets:?}"
                )));
            }
            if !(amount.is_finite() && amount > 0.0) {
                return Err(refuse(format!(
                    "amount {amount:e} for an offer of {offered:e}"
                )));
            }
            sum = add_down(sum, amount);
            previous = Some(cell);
        }
        if sum > offered {
            return Err(refuse(format!(
                "packets summing to {sum:e} for an offer of {offered:e}"
            )));
        }
        self.stage(
            grid,
            organism,
            paid,
            offered,
            direction,
            packets.iter().copied(),
        );
        Ok(())
    }

    /// [`Deposits::push`]'s checks of everything but the position.
    fn check_entry(
        &self,
        grid: &FieldGrid,
        paid: f64,
        offered: f64,
        direction: &[f64; CHANNELS],
    ) -> Result<(), CoreError> {
        if self.applied {
            return Err(CoreError::StateConflict {
                operation: "stage deposit",
                detail: "these deposits were already applied; clear them first".into(),
            });
        }
        if self.grid.is_some_and(|staged| staged != *grid) {
            return Err(CoreError::StateConflict {
                operation: "stage deposit",
                detail: "these deposits were staged for another grid".into(),
            });
        }
        if self.entries.len() >= self.organisms {
            return Err(CoreError::BudgetExhausted {
                budget: DEPOSIT_PACKETS,
                required: u64::try_from(self.entries.len() + 1).unwrap_or(u64::MAX),
                available: u64::try_from(self.organisms).unwrap_or(u64::MAX),
            });
        }
        for (quantity, value) in [("deposit payment", paid), ("deposit offer", offered)] {
            if !(value.is_finite() && value >= 0.0) {
                return Err(CoreError::NumericalInvalid {
                    quantity,
                    detail: format!("{value} is not a finite nonnegative energy"),
                });
            }
        }
        if offered > paid {
            return Err(CoreError::NumericalInvalid {
                quantity: "deposit offer",
                detail: format!("offers {offered} but paid only {paid}"),
            });
        }
        let squared = direction
            .iter()
            .fold(0.0, |total, &component| total + component * component);
        if !(squared.is_finite() && squared <= 1.0) {
            return Err(CoreError::NumericalInvalid {
                quantity: "deposit direction",
                detail: format!("||a||² = {squared} is outside the unit ball"),
            });
        }
        Ok(())
    }

    /// Stage a checked deposit whose cells receive `shares`, keeping the
    /// positive ones.
    fn stage(
        &mut self,
        grid: &FieldGrid,
        organism: OrganismId,
        paid: f64,
        offered: f64,
        direction: &[f64; CHANNELS],
        shares: impl Iterator<Item = (usize, f64)>,
    ) {
        let entry = self.entries.len();
        self.grid = Some(*grid);
        let first = self.packets.len();
        for (cell, amount) in shares {
            if amount > 0.0 {
                self.packets.push(Packet {
                    cell,
                    organism,
                    entry,
                    amount,
                });
            }
        }
        self.entries.push(Entry {
            organism,
            paid: paid + 0.0,
            offered: offered + 0.0,
            direction: *direction,
            first,
            count: self.packets.len() - first,
        });
    }

    /// Forget every staged deposit and give these deposits room to stage
    /// exactly `organisms` deposits and their packets, and no more room than
    /// that: deposits that only stage, and are then appended to deposits
    /// declared for the whole reduction ([`Deposits::append`]). Their own
    /// declared limit is unchanged.
    ///
    /// # Errors
    /// [`CoreError::AllocationFailed`] if the room cannot be reserved;
    /// [`CoreError::Overflow`] if its size overflows.
    pub fn fit_staging(&mut self, organisms: usize) -> Result<(), CoreError> {
        self.fit_staging_for(organisms, PACKETS_PER_DEPOSIT as usize)
    }

    /// [`Deposits::fit_staging`] sized for `kernel` on `grid`, reserving
    /// footprint and share scratch as well as entries and packets so steady-state
    /// staging allocates nothing (FL-053, FL-043).
    ///
    /// # Errors
    /// As [`Deposits::fit_staging`].
    pub fn fit_staging_for_kernel(
        &mut self,
        grid: &FieldGrid,
        organisms: usize,
        kernel: ExchangeKernel,
    ) -> Result<(), CoreError> {
        self.fit_staging_for(organisms, kernel.max_cells(grid))?;
        self.exchange = ExchangeScratch::for_kernel(kernel, grid)?;
        Ok(())
    }

    /// [`Deposits::fit_staging`] sized for at most `max_packets_per_deposit`
    /// cells per deposit (FL-053, FL-043).
    ///
    /// # Errors
    /// As [`Deposits::fit_staging`].
    pub fn fit_staging_for(
        &mut self,
        organisms: usize,
        max_packets_per_deposit: usize,
    ) -> Result<(), CoreError> {
        self.clear();
        let packets =
            organisms
                .checked_mul(max_packets_per_deposit)
                .ok_or(CoreError::Overflow {
                    quantity: DEPOSIT_PACKETS,
                })?;
        fit(&mut self.entries, organisms)?;
        fit(&mut self.packets, packets)?;
        if max_packets_per_deposit > PACKETS_PER_DEPOSIT as usize {
            fit(&mut self.shares, max_packets_per_deposit)?;
        } else {
            self.shares = Vec::new();
        }
        Ok(())
    }

    /// Move `other`'s staged deposits after these, in the order `other`
    /// staged them: the same entries and packets these deposits would hold
    /// had each been pushed here instead, so reducing them gives the same
    /// field and the same summary. `other` is left empty, with its room.
    ///
    /// # Errors
    /// - [`CoreError::StateConflict`] when either was applied, or they were
    ///   staged for different grids.
    /// - [`CoreError::BudgetExhausted`] when these deposits were declared for
    ///   fewer than both hold together, naming the first that does not fit,
    ///   as [`Deposits::push`] would.
    ///
    /// Nothing moves on error.
    pub fn append(&mut self, other: &mut Self) -> Result<(), CoreError> {
        if self.applied || other.applied {
            return Err(CoreError::StateConflict {
                operation: "append deposits",
                detail: "these deposits were already applied; clear them first".into(),
            });
        }
        if let (Some(staged), Some(appended)) = (self.grid, other.grid)
            && staged != appended
        {
            return Err(CoreError::StateConflict {
                operation: "append deposits",
                detail: "these deposits were staged for another grid".into(),
            });
        }
        if other.entries.len() > self.organisms.saturating_sub(self.entries.len()) {
            return Err(CoreError::BudgetExhausted {
                budget: DEPOSIT_PACKETS,
                required: u64::try_from(self.organisms.saturating_add(1)).unwrap_or(u64::MAX),
                available: u64::try_from(self.organisms).unwrap_or(u64::MAX),
            });
        }
        let entries = self.entries.len();
        let packets = self.packets.len();
        self.packets
            .extend(other.packets.iter().map(|packet| Packet {
                entry: packet.entry + entries,
                ..*packet
            }));
        self.entries.extend(other.entries.iter().map(|entry| Entry {
            first: entry.first + packets,
            ..*entry
        }));
        if other.grid.is_some() {
            self.grid = other.grid;
        }
        other.clear();
        Ok(())
    }
}

/// Empty `vector` and give it room for exactly `count` values.
fn fit<T>(vector: &mut Vec<T>, count: usize) -> Result<(), CoreError> {
    vector.clear();
    if vector.capacity() > count {
        vector.shrink_to(count);
    }
    vector
        .try_reserve_exact(count)
        .map_err(|_| CoreError::AllocationFailed {
            buffer: DEPOSIT_PACKETS,
            bytes: u64::try_from(count.saturating_mul(std::mem::size_of::<T>()))
                .unwrap_or(u64::MAX),
        })
}

/// `offered` over `stencil`'s cells: every cell but the largest-weight one
/// gets `offered w_c`, and that one gets the remainder, rounded down.
fn split(stencil: &ExchangeStencil, offered: f64) -> [f64; 4] {
    let weights = stencil.weights();
    let mut largest = 0;
    for (index, &weight) in weights.iter().enumerate() {
        if weight > weights[largest] {
            largest = index;
        }
    }
    let mut shares = [0.0; 4];
    let mut others = 0.0;
    for (index, (&weight, share)) in weights.iter().zip(&mut shares).enumerate() {
        if index != largest {
            *share = offered * weight;
            others = add_up(others, *share);
        }
    }
    if let Some(share) = shares.get_mut(largest) {
        *share = sub_down(offered, others).max(0.0);
    }
    shares
}

/// Conservative multi-cell split for arbitrary footprints (FL-053): every
/// share is nonnegative and `Σ_c share_c ≤ offered` holds in exact
/// arithmetic, so a deposit never creates energy.
fn split_slice(cells: &[(usize, f64)], offered: f64, shares: &mut Vec<f64>) {
    shares.clear();
    shares.resize(cells.len(), 0.0);
    if cells.is_empty() || offered <= 0.0 {
        return;
    }
    let mut largest = 0;
    for (index, &(_, weight)) in cells.iter().enumerate() {
        if weight > cells[largest].1 {
            largest = index;
        }
    }
    let mut others = 0.0;
    for (index, (&(_, weight), share)) in cells.iter().zip(shares.iter_mut()).enumerate() {
        if index != largest {
            *share = offered * weight;
            others = add_up(others, *share);
        }
    }
    if others <= offered {
        if let Some(share) = shares.get_mut(largest) {
            *share = sub_down(offered, others).max(0.0);
        }
        return;
    }
    // Extreme edge case (e.g., subnormal offer or huge cell count where the
    // upper-bounded sum of non-largest shares passed `offered`): clamp
    // monotonically so `add_up` running sum never exceeds `offered`.
    let mut running = 0.0;
    for (&(_, weight), share) in cells.iter().zip(shares.iter_mut()) {
        let mut amount = offered * weight;
        if add_up(running, amount) > offered {
            amount = sub_down(offered, running).max(0.0);
            while amount > 0.0 && add_up(running, amount) > offered {
                amount = amount.next_down();
            }
        }
        *share = amount;
        running = add_up(running, amount);
    }
}

/// `a + b`, rounded so that it is never below the exact sum.
fn add_up(a: f64, b: f64) -> f64 {
    -add_down(-a, -b)
}

/// The largest F32 value at or below `value`, for `value ≥ 0`.
fn round_down_f32(value: f64) -> f32 {
    if value >= f64::from(f32::MAX) {
        return f32::MAX;
    }
    let nearest = value as f32;
    if f64::from(nearest) > value {
        nearest.next_down()
    } else {
        nearest
    }
}

/// The fewest packets one thread of [`apply_deposits_on`] reduces: a smaller
/// run costs more to hand to a thread than it saves.
const PACKETS_PER_RUN: usize = 1_024;

/// Result of splitting one organism's paid emission across Carrier 0 (`24`
/// metabolic channels) and Carrier 1 (`8` non-destructive trail channels)
/// under `V2-15` (`dual-carrier-trails`).
///
/// Guarantees `paid_0 + paid_1 <= paid`, `offered_k <= paid_k`, and sub-vector
/// unit-ball norms `‖direction_0‖₂ ≤ 1`, `‖direction_1‖₂ ≤ 1` without
/// renormalization (zero `0/0` NaN).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DualCarrierEmissionSplit {
    /// Paid share allocated to Carrier 0.
    pub paid_0: f64,
    /// Offered energy for Carrier 0 (`<= paid_0`).
    pub offered_0: f64,
    /// Carrier 0 direction (`direction[0..24]` followed by 8 zeros).
    pub direction_0: [f64; CHANNELS],
    /// Paid share allocated to Carrier 1 (trail carrier).
    pub paid_1: f64,
    /// Offered energy for Carrier 1 (`<= paid_1`).
    pub offered_1: f64,
    /// Carrier 1 direction (`direction[24..32]` packed into `[0..8]`, zeros after).
    pub direction_1: [f64; CHANNELS],
}

/// Split one paid emission `(paid, efficiency, direction)` between Carrier 0
/// (`24` channels) and Carrier 1 (`8` trail channels) using configured
/// `trail_share ∈ [0, 1]` (`V2-15`).
///
/// Exact F64 arithmetic:
/// - `paid_1 = (trail_share * paid).min(paid)`
/// - `paid_0 = sub_down(paid, paid_1).max(0.0)`
/// - `offered_0 = (efficiency * paid_0).min(paid_0)`
/// - `offered_1 = (efficiency * paid_1).min(paid_1)`
///
/// # Errors
/// [`CoreError::NumericalInvalid`] for negative/nonfinite `paid`, `efficiency`
/// or `trail_share` outside `[0, 1]`, or a direction outside the unit ball.
pub fn split_dual_carrier_emission(
    paid: f64,
    efficiency: f64,
    trail_share: f64,
    direction: &[f64; CHANNELS],
) -> Result<DualCarrierEmissionSplit, CoreError> {
    if !paid.is_finite() || paid < 0.0 {
        return Err(CoreError::NumericalInvalid {
            quantity: "emission payment",
            detail: format!("{paid:?} is not finite and nonnegative"),
        });
    }
    if !efficiency.is_finite() || !(0.0..=1.0).contains(&efficiency) {
        return Err(CoreError::NumericalInvalid {
            quantity: "emission efficiency",
            detail: format!("{efficiency:?} is outside [0, 1]"),
        });
    }
    if !trail_share.is_finite() || !(0.0..=1.0).contains(&trail_share) {
        return Err(CoreError::NumericalInvalid {
            quantity: "v2.carriers.trail_emission_share",
            detail: format!("{trail_share:?} is outside [0, 1]"),
        });
    }
    let mut norm_sq = 0.0_f64;
    for &c in direction {
        if !c.is_finite() {
            return Err(CoreError::NumericalInvalid {
                quantity: "emission direction",
                detail: format!("nonfinite component {c:?}"),
            });
        }
        norm_sq += c * c;
    }
    if !norm_sq.is_finite() || norm_sq > 1.0 {
        return Err(CoreError::NumericalInvalid {
            quantity: "emission direction",
            detail: format!("squared norm {norm_sq:?} exceeds 1"),
        });
    }
    let paid_1 = (trail_share * paid).min(paid);
    let paid_0 = sub_down(paid, paid_1).max(0.0);
    let offered_0 = (efficiency * paid_0).min(paid_0);
    let offered_1 = (efficiency * paid_1).min(paid_1);
    let mut direction_0 = [0.0_f64; CHANNELS];
    let c0 = CARRIER0_MOMENT_CHANNELS as usize;
    direction_0[..c0].copy_from_slice(&direction[..c0]);
    let mut direction_1 = [0.0_f64; CHANNELS];
    let c1 = CARRIER1_MOMENT_CHANNELS as usize;
    direction_1[..c1].copy_from_slice(&direction[c0..c0 + c1]);
    Ok(DualCarrierEmissionSplit {
        paid_0,
        offered_0,
        direction_0,
        paid_1,
        offered_1,
        direction_1,
    })
}

/// Reduce `deposits` into `field`'s Carrier 0, the next state, and return what
/// was paid, offered and applied. The deposits are marked applied.
///
/// # Errors
/// [`CoreError::StateConflict`] when the deposits were already applied,
/// were staged for another grid, name one organism twice, or the field
/// lacks a moment plane; [`CoreError::LayoutMismatch`] if the field does
/// not hold every cell of its grid in every plane. Nothing is written on
/// error.
pub fn apply_deposits(
    field: &mut FieldState,
    deposits: &mut Deposits,
) -> Result<DepositSummary, CoreError> {
    apply_deposits_on(field, deposits, 1)
}

/// Reduce `deposits` into `carrier` of `field` on one thread (`V2-15`).
///
/// # Errors
/// As [`apply_deposits_to_carrier_on`].
pub fn apply_deposits_to_carrier(
    field: &mut FieldState,
    deposits: &mut Deposits,
    carrier: CarrierLayout,
) -> Result<DepositSummary, CoreError> {
    apply_deposits_to_carrier_on(field, deposits, carrier, 1)
}

/// [`apply_deposits`] on up to `workers` threads targeting `field.carrier0()`.
///
/// # Errors
/// As [`apply_deposits_to_carrier_on`].
pub fn apply_deposits_on(
    field: &mut FieldState,
    deposits: &mut Deposits,
    workers: usize,
) -> Result<DepositSummary, CoreError> {
    let carrier = field.carrier0();
    apply_deposits_to_carrier_on(field, deposits, carrier, workers)
}

/// [`apply_deposits_on`] targeting a specific [`CarrierLayout`] (`V2-15`):
/// `(0, 1, 32)` on 33-plane single-carrier fields, `(0, 1, 24)` for Carrier 0
/// or `(25, 26, 8)` for Carrier 1 on 34-plane dual-carrier fields. Leaves all
/// planes outside `carrier` untouched.
///
/// # Errors
/// As [`apply_deposits`]; [`CoreError::LayoutMismatch`] if `carrier` lies
/// outside `field`'s planes or overlaps its own energy plane;
/// [`CoreError::StateConflict`] for a worker that panicked.
pub fn apply_deposits_to_carrier_on(
    field: &mut FieldState,
    deposits: &mut Deposits,
    carrier: CarrierLayout,
    workers: usize,
) -> Result<DepositSummary, CoreError> {
    let conflict = |detail: String| CoreError::StateConflict {
        operation: "apply deposits",
        detail,
    };
    let (mut summary, buckets) = deposits.prepare_for_carrier(field, carrier)?;
    let Deposits {
        entries,
        packets,
        received,
        sequence,
        bins,
        ..
    } = deposits;
    let values = field.as_mut_slice();
    let offsets = bins.offsets();
    let order = bins.order();
    let items = order.len();
    let start = |cell: usize| offsets.get(cell).map_or(items, |&at| at as usize);
    let runs = workers.min(items / PACKETS_PER_RUN).max(1);
    let bounds: Vec<usize> = (0..=runs)
        .map(|run| {
            if run == runs {
                buckets
            } else {
                let target = items / runs * run + items % runs * run / runs;
                offsets
                    .partition_point(|&at| (at as usize) < target)
                    .min(buckets)
            }
        })
        .collect();
    received.clear();
    received.resize(items, 0.0);

    let energy_start = (carrier.energy_plane as usize) * buckets;
    let moments_start = (carrier.first_moment_plane as usize) * buckets;
    let moment_count = carrier.moment_count as usize;
    let moments_len = moment_count * buckets;
    let (mut energy, moments): (&mut [f32], &mut [f32]) = if energy_start < moments_start {
        let (_, tail) = cut(values, energy_start)?;
        let (e_slice, after_e) = cut(tail, buckets)?;
        let gap = moments_start - (energy_start + buckets);
        let (_, m_tail) = cut(after_e, gap)?;
        let (m_slice, _) = cut(m_tail, moments_len)?;
        (e_slice, m_slice)
    } else {
        let (_, tail) = cut(values, moments_start)?;
        let (m_slice, after_m) = cut(tail, moments_len)?;
        let gap = energy_start - (moments_start + moments_len);
        let (_, e_tail) = cut(after_m, gap)?;
        let (e_slice, _) = cut(e_tail, buckets)?;
        (e_slice, m_slice)
    };

    let mut slots: &mut [f64] = received;
    let mut lanes = Vec::with_capacity(runs);
    for pair in bounds.windows(2) {
        let cells = pair[0]..pair[1];
        let (head, tail) = cut(std::mem::take(&mut energy), cells.len())?;
        energy = tail;
        let (owned, rest) = cut(
            std::mem::take(&mut slots),
            start(cells.end).saturating_sub(start(cells.start)),
        )?;
        slots = rest;
        lanes.push(Run {
            cells,
            energy: head,
            moment_count,
            moments: std::array::from_fn(|_| <&mut [f32]>::default()),
            received: owned,
        });
    }
    if moment_count > 0 {
        for (channel, plane) in moments.chunks_exact_mut(buckets.max(1)).enumerate() {
            let mut plane = plane;
            for lane in &mut lanes {
                let (head, tail) = cut(std::mem::take(&mut plane), lane.cells.len())?;
                plane = tail;
                if let Some(slot) = lane.moments.get_mut(channel) {
                    *slot = head;
                }
            }
        }
    }

    let (sequence, packets, entries) = (&**sequence, &**packets, &**entries);
    let reduce = |run: &mut Run<'_>| reduce_run(run, offsets, order, sequence, packets, entries);
    let reduce = &reduce;
    let mut rest = lanes.iter_mut();
    let first = rest.next();
    let occupied = std::thread::scope(|scope| {
        let workers: Vec<_> = rest.map(|run| scope.spawn(move || reduce(run))).collect();
        let mut occupied = Vec::with_capacity(runs);
        occupied.push(first.map_or(0, reduce));
        for worker in workers {
            occupied.push(
                worker
                    .join()
                    .map_err(|_| conflict("a deposit worker panicked".into()))?,
            );
        }
        Ok::<_, CoreError>(occupied)
    })?;

    let mut applied = CompensatedSum::ZERO;
    for (lane, &count) in lanes.iter().zip(&occupied) {
        for &value in lane.received.get(..count).unwrap_or(&[]) {
            applied.add(value);
        }
        summary.cells += count;
    }
    summary.applied = applied.value();
    deposits.applied = true;
    Ok(summary)
}

impl Deposits {
    /// [`apply_deposits_to_carrier_on`]'s checks and grouping for a reduction
    /// into `carrier` of `field`. Nothing is applied.
    fn prepare_for_carrier(
        &mut self,
        field: &FieldState,
        carrier: CarrierLayout,
    ) -> Result<(DepositSummary, usize), CoreError> {
        let conflict = |detail: String| CoreError::StateConflict {
            operation: "apply deposits",
            detail,
        };
        if self.applied {
            return Err(conflict("these deposits were already applied".into()));
        }
        if self.grid.is_some_and(|grid| grid != *field.grid()) {
            return Err(conflict(
                "these deposits were staged for another grid".into(),
            ));
        }
        let strictly_ascending = self
            .entries
            .windows(2)
            .all(|pair| pair[0].organism < pair[1].organism);
        if !strictly_ascending {
            self.identities.clear();
            self.identities.extend(
                self.entries
                    .iter()
                    .enumerate()
                    .map(|(index, entry)| (entry.organism, index)),
            );
            self.identities.sort_unstable();
            if let Some(pair) = self
                .identities
                .windows(2)
                .find(|pair| pair[0].0 == pair[1].0)
            {
                return Err(conflict(format!(
                    "{} staged two deposits in one reduction",
                    pair[0].0
                )));
            }
        }
        let total_planes = field.planes();
        let moment_end = carrier
            .first_moment_plane
            .checked_add(carrier.moment_count)
            .ok_or(CoreError::Overflow {
                quantity: "carrier moment planes",
            })?;
        let moment_count = carrier.moment_count as usize;
        if carrier.energy_plane >= total_planes
            || moment_end > total_planes
            || moment_count > CHANNELS
            || (carrier.energy_plane >= carrier.first_moment_plane
                && carrier.energy_plane < moment_end)
        {
            return Err(CoreError::LayoutMismatch {
                buffer: crate::layout::BUFFER,
                detail: format!(
                    "carrier layout {carrier:?} is invalid for a {total_planes}-plane field"
                ),
            });
        }
        if field.plane(carrier.energy_plane).is_none()
            || (carrier.first_moment_plane..moment_end).any(|plane| field.plane(plane).is_none())
        {
            return Err(conflict("a moment plane is missing".into()));
        }

        let mut summary = DepositSummary {
            organisms: self.entries.len(),
            ..DepositSummary::default()
        };
        let mut paid = CompensatedSum::ZERO;
        let mut offered = CompensatedSum::ZERO;
        for entry in &self.entries {
            paid.add(entry.paid);
            offered.add(entry.offered);
        }
        summary.paid = paid.value();
        summary.offered = offered.value();
        if !summary.paid.is_finite() || !summary.offered.is_finite() {
            return Err(CoreError::NumericalInvalid {
                quantity: "deposit total",
                detail: format!(
                    "staged deposits overflow F64 (paid={}, offered={})",
                    summary.paid, summary.offered
                ),
            });
        }

        let Self {
            entries,
            packets,
            identities,
            sequence,
            bins,
            ..
        } = self;
        if u32::try_from(packets.len()).is_err() {
            return Err(CoreError::Overflow {
                quantity: DEPOSIT_PACKETS,
            });
        }
        sequence.clear();
        if strictly_ascending {
            for entry in entries.iter() {
                sequence.extend((entry.first..entry.first + entry.count).map(|at| at as u32));
            }
        } else {
            for &(_, index) in identities.iter() {
                if let Some(entry) = entries.get(index) {
                    sequence.extend((entry.first..entry.first + entry.count).map(|at| at as u32));
                }
            }
        }
        let buckets =
            usize::try_from(field.grid().cell_count()).map_err(|_| CoreError::Overflow {
                quantity: DEPOSIT_PACKETS,
            })?;
        bins.group(buckets, sequence.len(), |at| {
            sequence
                .get(at)
                .and_then(|&packet| packets.get(packet as usize))
                .map_or(usize::MAX, |packet| packet.cell)
        })?;

        let values = field.as_slice().len();
        let planes_usize = usize::try_from(total_planes).map_err(|_| CoreError::Overflow {
            quantity: DEPOSIT_PACKETS,
        })?;
        if buckets.checked_mul(planes_usize) != Some(values) {
            return Err(CoreError::LayoutMismatch {
                buffer: crate::layout::BUFFER,
                detail: format!(
                    "the field holds {values} values, not {buckets} cells in {planes_usize} planes"
                ),
            });
        }
        Ok((summary, buckets))
    }

    /// These deposits checked and grouped for a reduction into `field`'s
    /// Carrier 0 on another device (FL-032).
    ///
    /// # Errors
    /// As [`apply_deposits_on`]. Nothing is applied either way until
    /// [`DepositReduction::finish`].
    pub fn reduction(&mut self, field: &FieldState) -> Result<DepositReduction<'_>, CoreError> {
        self.reduction_for_carrier(field, field.carrier0())
    }

    /// These deposits checked and grouped for a reduction into `carrier` of
    /// `field` on another device (`V2-15`).
    ///
    /// # Errors
    /// As [`apply_deposits_to_carrier_on`]. Nothing is applied either way until
    /// [`DepositReduction::finish`].
    pub fn reduction_for_carrier(
        &mut self,
        field: &FieldState,
        carrier: CarrierLayout,
    ) -> Result<DepositReduction<'_>, CoreError> {
        let (summary, cells) = self.prepare_for_carrier(field, carrier)?;
        Ok(DepositReduction {
            deposits: self,
            summary,
            cells,
        })
    }
}

/// Deposits checked and grouped by cell for a reduction on another device
/// (FL-032); see [`Deposits::reduction`].
#[derive(Debug)]
pub struct DepositReduction<'a> {
    deposits: &'a mut Deposits,
    summary: DepositSummary,
    cells: usize,
}

impl DepositReduction<'_> {
    /// One offset per cell and one more: cell `c`'s packets are
    /// `offsets()[c]..offsets()[c + 1]` in [`DepositReduction::packet`]'s
    /// numbering, in identity order.
    #[must_use]
    pub fn offsets(&self) -> &[u32] {
        self.deposits.bins.offsets()
    }

    /// Grouped packet `at`: its amount and its deposit's index.
    #[must_use]
    pub fn packet(&self, at: usize) -> Option<(f64, usize)> {
        let deposits = &*self.deposits;
        let &index = deposits.bins.order().get(at)?;
        let &packet = deposits.sequence.get(index as usize)?;
        let packet = deposits.packets.get(packet as usize)?;
        Some((packet.amount, packet.entry))
    }

    /// How many deposits there are: their indices are below it.
    #[must_use]
    pub fn deposits(&self) -> usize {
        self.deposits.entries.len()
    }

    /// Deposit `index`'s chemical direction.
    #[must_use]
    pub fn direction(&self, index: usize) -> Option<&[f64; CHANNELS]> {
        self.deposits
            .entries
            .get(index)
            .map(|entry| &entry.direction)
    }

    /// The reduction's summary, given `received[c]` for every cell `c`, of
    /// which only the cells with packets are read: the summary's cells and
    /// its applied energy, summed in cell order as [`apply_deposits_on`]
    /// sums them. Nothing is applied.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] for fewer values than cells.
    pub fn summarize(&self, received: &[f64]) -> Result<DepositSummary, CoreError> {
        if received.len() < self.cells {
            return Err(CoreError::LayoutMismatch {
                buffer: DEPOSIT_PACKETS,
                detail: format!(
                    "{} received values for {} cells",
                    received.len(),
                    self.cells
                ),
            });
        }
        let mut summary = self.summary;
        let mut applied = CompensatedSum::ZERO;
        for (cell, pair) in self.offsets().windows(2).enumerate() {
            if pair[0] < pair[1]
                && let Some(&value) = received.get(cell)
            {
                applied.add(value);
                summary.cells += 1;
            }
        }
        summary.applied = applied.value();
        Ok(summary)
    }

    /// Mark the deposits applied, once the other device has written the
    /// reduction into the field.
    pub fn finish(self) {
        self.deposits.applied = true;
    }
}

/// One thread's share of a reduction: a contiguous range of cells, their
/// energy and their values in every moment plane, and a slot for what each
/// of its occupied cells received.
struct Run<'a> {
    cells: Range<usize>,
    energy: &'a mut [f32],
    moment_count: usize,
    moments: [&'a mut [f32]; CHANNELS],
    received: &'a mut [f64],
}

/// Occupied cells accumulated before flushing moment writes plane-by-plane
/// in [`reduce_run`]. Flushing 32 cells per plane avoids 32-way L1D
/// cache-set thrashing on power-of-two grids (e.g. 512² where planes are
/// 1 MiB apart) while writing bit-identical F32 values.
const REDUCE_BATCH: usize = 32;

/// Flush `count` buffered cell updates into `moments`, one plane at a time.
fn flush_moment_batch(
    moments: &mut [&mut [f32]],
    count: usize,
    cell_at: &[usize; REDUCE_BATCH],
    factors: &[f64; REDUCE_BATCH],
    accum: &[[f64; CHANNELS]; REDUCE_BATCH],
) {
    for (channel, plane) in moments.iter_mut().enumerate() {
        for b in 0..count {
            let added = accum[b][channel] * factors[b];
            if added != 0.0
                && let Some(value) = plane.get_mut(cell_at[b])
            {
                *value = (f64::from(*value) + added) as f32;
            }
        }
    }
}

/// Reduce and write `run`'s occupied cells in cell order, each as
/// [`apply_deposits`] reduces it, filling `run.received` in the same order.
/// Returns how many cells received a packet.
fn reduce_run(
    run: &mut Run<'_>,
    offsets: &[u32],
    order: &[u32],
    sequence: &[u32],
    packets: &[Packet],
    entries: &[Entry],
) -> usize {
    let mut occupied = 0;
    let mut batch_len = 0_usize;
    let mut cell_at = [0_usize; REDUCE_BATCH];
    let mut factors = [0.0_f64; REDUCE_BATCH];
    let mut accum = [[0.0_f64; CHANNELS]; REDUCE_BATCH];
    let moment_count = run.moment_count.min(CHANNELS);

    for (at, cell) in run.cells.clone().enumerate() {
        let (Some(&first), Some(&end)) = (offsets.get(cell), offsets.get(cell + 1)) else {
            continue;
        };
        if first >= end {
            continue;
        }
        let Some(energy) = run.energy.get_mut(at) else {
            continue;
        };
        let slice = order.get(first as usize..end as usize).unwrap_or(&[]);
        let before = *energy;
        let mut total = 0.0;
        let mut moment = [0.0; CHANNELS];
        for &ord in slice {
            let Some(&seq) = sequence.get(ord as usize) else {
                continue;
            };
            let Some(packet) = packets.get(seq as usize) else {
                continue;
            };
            total = add_down(total, packet.amount);
            if let Some(entry) = entries.get(packet.entry) {
                let amount = packet.amount;
                for (slot, &component) in moment.iter_mut().zip(&entry.direction).take(moment_count)
                {
                    *slot += amount * component;
                }
            }
        }
        let after = round_down_f32(add_down(f64::from(before), total));
        let received = f64::from(after) - f64::from(before);
        let factor = if total > 0.0 && total.is_finite() {
            received / total
        } else {
            0.0
        };
        *energy = after;
        if let Some(slot) = run.received.get_mut(occupied) {
            *slot = received;
        }
        occupied += 1;

        if factor != 0.0 {
            cell_at[batch_len] = at;
            factors[batch_len] = factor;
            accum[batch_len] = moment;
            batch_len += 1;
            if batch_len == REDUCE_BATCH {
                flush_moment_batch(
                    &mut run.moments[..moment_count],
                    batch_len,
                    &cell_at,
                    &factors,
                    &accum,
                );
                batch_len = 0;
            }
        }
    }
    if batch_len > 0 {
        flush_moment_batch(
            &mut run.moments[..moment_count],
            batch_len,
            &cell_at,
            &factors,
            &accum,
        );
    }
    occupied
}

/// `slice` cut after its first `count` values.
fn cut<T>(slice: &mut [T], count: usize) -> Result<(&mut [T], &mut [T]), CoreError> {
    slice
        .split_at_mut_checked(count)
        .ok_or_else(|| CoreError::LayoutMismatch {
            buffer: crate::layout::BUFFER,
            detail: format!("a deposit run of {count} values overruns its buffer"),
        })
}
