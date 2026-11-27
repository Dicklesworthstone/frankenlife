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

/// One organism's staged deposit metadata (kept compact at 40 B so
/// `prepare_for_carrier` scans metadata without touching the 256 B per-entry
/// chemical directions stored in `Deposits::directions`).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Entry {
    organism: OrganismId,
    paid: f64,
    offered: f64,
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
    /// Chemical direction for `entries[i]`, stored in a parallel array so
    /// metadata scans (`prepare_for_carrier`) touch 40 B/entry instead of
    /// 296 B/entry, and reduction reads 256 B-aligned direction rows directly.
    directions: Vec<[f64; CHANNELS]>,
    packets: Vec<Packet>,
    /// What each occupied cell of the last reduction received: a slot a
    /// packet, so that each run of cells has its own.
    received: Vec<f64>,
    /// Each entry's organism and index, sorted by organism.
    identities: Vec<(OrganismId, usize)>,
    /// Packet indices in organism identity order, before grouping by cell.
    sequence: Vec<u32>,
    /// True when the last `prepare_for_carrier` proved `packets` is already in
    /// strictly ascending identity order (`0..packets.len()`), allowing
    /// grouping and reduction to skip `sequence` indirection completely.
    identity_sequence: bool,
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
            capacity_bytes(&self.directions),
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
            directions: Vec::new(),
            packets: Vec::new(),
            received: Vec::new(),
            identities: Vec::new(),
            sequence: Vec::new(),
            identity_sequence: false,
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
        let per_entry_bytes =
            (std::mem::size_of::<Entry>() + std::mem::size_of::<[f64; CHANNELS]>()) as u64;
        let entry_bytes = organisms
            .checked_mul(per_entry_bytes)
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
            directions: Vec::with_capacity(organisms),
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
        self.directions.clear();
        self.packets.clear();
        self.received.clear();
        self.identities.clear();
        self.sequence.clear();
        self.identity_sequence = false;
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
                    first,
                    count: self.packets.len() - first,
                });
                self.directions.push(*direction);
            }
        }
        Ok(())
    }

    /// Stage `organism`'s deposit using a precomputed `grid-cic-v1`
    /// [`ExchangeStencil`], avoiding redundant stencil computation when
    /// multiple deposit streams share the exact same position.
    ///
    /// # Errors
    /// As [`Deposits::push`]. Nothing is staged on error.
    pub fn push_stencil(
        &mut self,
        grid: &FieldGrid,
        organism: OrganismId,
        paid: f64,
        offered: f64,
        direction: &[f64; CHANNELS],
        stencil: &ExchangeStencil,
    ) -> Result<(), CoreError> {
        self.check_entry(grid, paid, offered, direction)?;
        let shares = split(stencil, offered);
        self.stage(
            grid,
            organism,
            paid,
            offered,
            direction,
            stencil.cells().iter().copied().zip(shares),
        );
        Ok(())
    }

    /// Stage `organism`'s deposit using a precomputed [`ExchangeKernel`]
    /// footprint (`(cell_index, weight)` pairs from
    /// [`ExchangeKernel::footprint_into`] in ascending cell order), avoiding
    /// redundant Gaussian evaluations when multiple streams share a position.
    ///
    /// # Errors
    /// As [`Deposits::push`], [`CoreError::StateConflict`] for cells outside
    /// `grid` or not in strictly ascending order, and
    /// [`CoreError::NumericalInvalid`] for a nonfinite or negative weight.
    /// Nothing is staged on error.
    pub fn push_footprint(
        &mut self,
        grid: &FieldGrid,
        organism: OrganismId,
        paid: f64,
        offered: f64,
        direction: &[f64; CHANNELS],
        footprint: &[(usize, f64)],
    ) -> Result<(), CoreError> {
        self.check_entry(grid, paid, offered, direction)?;
        let mut previous = None;
        for &(cell, weight) in footprint {
            if u64::try_from(cell).map_or(true, |cell| cell >= grid.cell_count())
                || previous.is_some_and(|previous| cell <= previous)
            {
                return Err(CoreError::StateConflict {
                    operation: "stage a deposit footprint",
                    detail: format!("cell {cell} out of order or outside the grid"),
                });
            }
            if !weight.is_finite() || weight < 0.0 {
                return Err(CoreError::NumericalInvalid {
                    quantity: "deposit footprint weight",
                    detail: format!("weight {weight:?} must be finite and nonnegative"),
                });
            }
            previous = Some(cell);
        }
        split_slice(footprint, offered, &mut self.shares);
        let entry = self.entries.len();
        self.grid = Some(*grid);
        let first = self.packets.len();
        for (&(cell, _), &amount) in footprint.iter().zip(&self.shares) {
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
            first,
            count: self.packets.len() - first,
        });
        self.directions.push(*direction);
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
            first,
            count: self.packets.len() - first,
        });
        self.directions.push(*direction);
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
        fit(&mut self.directions, organisms)?;
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
        if !other.packets.is_empty() {
            self.packets.extend_from_slice(&other.packets);
            if entries > 0 {
                for packet in &mut self.packets[packets..] {
                    packet.entry += entries;
                }
            }
        }
        if !other.entries.is_empty() {
            self.entries.extend_from_slice(&other.entries);
            self.directions.extend_from_slice(&other.directions);
            if packets > 0 {
                for entry in &mut self.entries[entries..] {
                    entry.first += packets;
                }
            }
        }
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
pub(crate) fn split(stencil: &ExchangeStencil, offered: f64) -> [f64; 4] {
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
    if cells[largest].1 <= 0.0 {
        return;
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
#[inline]
pub(crate) fn round_down_f32(value: f64) -> f32 {
    if value >= f64::from(f32::MAX) {
        return f32::MAX;
    }
    let nearest = value as f32;
    if value < 0.0 {
        if f64::from(nearest) > value {
            nearest.next_down()
        } else {
            nearest
        }
    } else {
        // Branchless `if f64::from(nearest) > value { nearest.next_down() } else { nearest }`
        // for `0 <= value < f32::MAX`: `nearest > 0` whenever `nearest > value >= 0`,
        // so subtracting 1 from its IEEE-754 bit pattern is exact across normals
        // and subnormals down to `+0.0`.
        let dec = u32::from(f64::from(nearest) > value);
        f32::from_bits(nearest.to_bits().wrapping_sub(dec))
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

/// Reduce multiple [`Deposits`] streams sequentially into `carrier` of `field`
/// in fixed slice order (`streams[0]`, `streams[1]`, ...) inside a single
/// worker-pool dispatch so each worker processes every active stream over its
/// cell partition while those cells stay hot in L1/L2 cache.
///
/// Bit-identical to calling [`apply_deposits_to_carrier_on`] on each stream in
/// slice order.
///
/// # Errors
/// [`CoreError::LayoutMismatch`] if `streams.len() != out.len()`, or as
/// [`apply_deposits_to_carrier_on`]. On validation error nothing is written to
/// `field`.
#[expect(
    clippy::too_many_lines,
    reason = "single-dispatch multi-stream carrier deposit reduction preserving bit-exact per-stream F32 rounding and compensated sums"
)]
pub fn apply_deposit_streams_to_carrier_on(
    field: &mut FieldState,
    streams: &mut [&mut Deposits],
    carrier: CarrierLayout,
    workers: usize,
    out: &mut [Option<DepositSummary>],
) -> Result<(), CoreError> {
    if streams.len() != out.len() {
        return Err(CoreError::LayoutMismatch {
            buffer: "deposit summaries",
            detail: format!(
                "{} deposit streams for {} summary slots",
                streams.len(),
                out.len()
            ),
        });
    }
    if streams.is_empty() {
        return Ok(());
    }
    if streams.len() == 1 {
        out[0] = Some(apply_deposits_to_carrier_on(
            field, streams[0], carrier, workers,
        )?);
        return Ok(());
    }

    // Validate and group every stream before mutating `field`.
    let mut prepared: Vec<(DepositSummary, usize)> = Vec::with_capacity(streams.len());
    let mut active_indices: Vec<usize> = Vec::with_capacity(streams.len());
    let mut total_items = 0_usize;
    let mut heaviest_stream = 0_usize;
    let mut heaviest_items = 0_usize;
    for (idx, stream) in streams.iter_mut().enumerate() {
        let (summary, buckets) = stream.prepare_for_carrier(field, carrier, workers)?;
        let items = stream.bins.order().len();
        let occ = stream.bins.occupied_buckets().len();
        if items > 0 && occ > 0 {
            active_indices.push(idx);
            total_items = total_items.saturating_add(items);
            if items > heaviest_items {
                heaviest_items = items;
                heaviest_stream = idx;
            }
        }
        prepared.push((summary, buckets));
    }
    if active_indices.is_empty() {
        for ((stream, (summary, _)), slot) in streams.iter_mut().zip(prepared).zip(out.iter_mut()) {
            stream.received.clear();
            stream.applied = true;
            *slot = Some(summary);
        }
        return Ok(());
    }
    let buckets = prepared[0].1;
    let values = field.as_mut_slice();
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

    let base_runs = workers.min(total_items / PACKETS_PER_RUN).max(1);
    let runs = if base_runs <= 1 {
        1
    } else {
        workers
            .saturating_mul(4)
            .min(64)
            .min(total_items / (PACKETS_PER_RUN / 2).max(1))
            .max(base_runs)
    };
    if runs == 1 {
        let mut field_moments: [&mut [f32]; CHANNELS] =
            std::array::from_fn(|_| <&mut [f32]>::default());
        if moment_count > 0 {
            for (channel, plane) in moments.chunks_exact_mut(buckets.max(1)).enumerate() {
                if let Some(slot) = field_moments.get_mut(channel) {
                    *slot = plane;
                }
            }
        }
        for ((stream, (mut summary, _)), slot) in
            streams.iter_mut().zip(prepared).zip(out.iter_mut())
        {
            let occ = stream.bins.occupied_buckets();
            stream.received.clear();
            if stream.bins.order().is_empty() || occ.is_empty() {
                stream.applied = true;
                *slot = Some(summary);
                continue;
            }
            stream.received.resize(occ.len(), 0.0);
            let seq = (!stream.identity_sequence).then_some(stream.sequence.as_slice());
            let count = {
                let mut run = Run {
                    cell_base: 0,
                    occupied_cells: occ,
                    energy: &mut *energy,
                    moment_count,
                    moments: field_moments.each_mut().map(|m| &mut **m),
                    received: stream.received.as_mut_slice(),
                };
                reduce_run(
                    &mut run,
                    stream.bins.offsets(),
                    stream.bins.order(),
                    seq,
                    &stream.packets,
                    &stream.directions,
                )
            };
            let mut applied = CompensatedSum::ZERO;
            for &value in stream.received.get(..count).unwrap_or(&[]) {
                applied.add(value);
            }
            summary.cells = count;
            summary.applied = applied.value();
            stream.applied = true;
            *slot = Some(summary);
        }
        return Ok(());
    }

    // Partition cells into `runs` ranges balanced by the heaviest active stream.
    let heavy_offsets = streams[heaviest_stream].bins.offsets();
    let heavy_items = streams[heaviest_stream].bins.order().len();
    let bounds: Vec<usize> = (0..=runs)
        .map(|run| {
            if run == runs {
                buckets
            } else {
                let target = heavy_items / runs * run + heavy_items % runs * run / runs;
                heavy_offsets
                    .partition_point(|&at| (at as usize) < target)
                    .min(buckets)
            }
        })
        .collect();

    struct FieldSlice<'a> {
        cell_base: usize,
        energy: &'a mut [f32],
        moments: [&'a mut [f32]; CHANNELS],
    }
    let mut field_lanes = Vec::with_capacity(runs);
    for pair in bounds.windows(2) {
        let cell_start = pair[0];
        let cell_end = pair[1];
        let (head, tail) = cut(std::mem::take(&mut energy), cell_end - cell_start)?;
        energy = tail;
        field_lanes.push(FieldSlice {
            cell_base: cell_start,
            energy: head,
            moments: std::array::from_fn(|_| <&mut [f32]>::default()),
        });
    }
    if moment_count > 0 {
        for (channel, plane) in moments.chunks_exact_mut(buckets.max(1)).enumerate() {
            let mut plane = plane;
            for lane in &mut field_lanes {
                let (head, tail) = cut(std::mem::take(&mut plane), lane.energy.len())?;
                plane = tail;
                if let Some(slot) = lane.moments.get_mut(channel) {
                    *slot = head;
                }
            }
        }
    }

    // Prepare per-stream read-only views and per-lane mutable `received` slices.
    struct StreamRead<'a> {
        offsets: &'a [u32],
        order: &'a [u32],
        sequence: Option<&'a [u32]>,
        packets: &'a [Packet],
        directions: &'a [[f64; CHANNELS]],
    }
    struct StreamLane<'a> {
        occupied_cells: &'a [u32],
        received: &'a mut [f64],
        count: usize,
    }
    let stream_count = streams.len();
    let mut stream_reads: Vec<StreamRead<'_>> = Vec::with_capacity(stream_count);
    let mut lane_streams: Vec<Vec<StreamLane<'_>>> = (0..runs)
        .map(|_| Vec::with_capacity(stream_count))
        .collect();
    for stream in streams.iter_mut() {
        let occ_len = stream.bins.occupied_buckets().len();
        stream.received.clear();
        stream.received.resize(occ_len, 0.0);
        let Deposits {
            directions,
            packets,
            received,
            sequence,
            identity_sequence,
            bins,
            ..
        } = &mut **stream;
        let occupied_buckets = bins.occupied_buckets();
        stream_reads.push(StreamRead {
            offsets: bins.offsets(),
            order: bins.order(),
            sequence: (!*identity_sequence).then_some(sequence.as_slice()),
            packets: packets.as_slice(),
            directions: directions.as_slice(),
        });
        let mut rem_received = received.as_mut_slice();
        let mut occ_cursor = 0_usize;
        for (run_idx, pair) in bounds.windows(2).enumerate() {
            let cell_end = pair[1];
            let occ_end = if run_idx + 1 == runs {
                occupied_buckets.len()
            } else {
                occ_cursor
                    + occupied_buckets[occ_cursor..].partition_point(|&c| (c as usize) < cell_end)
            };
            let lane_occ = &occupied_buckets[occ_cursor..occ_end];
            occ_cursor = occ_end;
            let (owned, rest) = cut(std::mem::take(&mut rem_received), lane_occ.len())?;
            rem_received = rest;
            lane_streams[run_idx].push(StreamLane {
                occupied_cells: lane_occ,
                received: owned,
                count: 0,
            });
        }
    }

    let reads: &[StreamRead<'_>] = &stream_reads;
    let process_lane = |field_lane: &mut FieldSlice<'_>, lane_work: &mut [StreamLane<'_>]| {
        for (s_read, s_lane) in reads.iter().zip(lane_work.iter_mut()) {
            if s_lane.occupied_cells.is_empty() {
                s_lane.count = 0;
                continue;
            }
            let mut run = Run {
                cell_base: field_lane.cell_base,
                occupied_cells: s_lane.occupied_cells,
                energy: &mut *field_lane.energy,
                moment_count,
                moments: field_lane.moments.each_mut().map(|m| &mut **m),
                received: &mut *s_lane.received,
            };
            s_lane.count = reduce_run(
                &mut run,
                s_read.offsets,
                s_read.order,
                s_read.sequence,
                s_read.packets,
                s_read.directions,
            );
        }
    };
    let mut work_tiles: Vec<(&mut FieldSlice<'_>, &mut Vec<StreamLane<'_>>)> = field_lanes
        .iter_mut()
        .zip(lane_streams.iter_mut())
        .collect();
    flife_core::pool::parallel_tasks(
        workers,
        "apply deposits",
        &mut work_tiles,
        |_, (f_lane, s_lanes)| process_lane(f_lane, s_lanes),
    )?;

    for (s_idx, (mut summary, _)) in prepared.into_iter().enumerate() {
        let mut applied = CompensatedSum::ZERO;
        for lane in &lane_streams {
            let s_lane = &lane[s_idx];
            for &value in s_lane.received.get(..s_lane.count).unwrap_or(&[]) {
                applied.add(value);
            }
            summary.cells += s_lane.count;
        }
        summary.applied = applied.value();
        out[s_idx] = Some(summary);
    }
    drop(lane_streams);
    drop(stream_reads);
    for stream in streams.iter_mut() {
        stream.applied = true;
    }
    Ok(())
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
    let (mut summary, buckets) = deposits.prepare_for_carrier(field, carrier, workers)?;
    let Deposits {
        directions,
        packets,
        received,
        sequence,
        identity_sequence,
        bins,
        ..
    } = deposits;
    let offsets = bins.offsets();
    let order = bins.order();
    let occupied_buckets = bins.occupied_buckets();
    let items = order.len();
    received.clear();
    if items == 0 || occupied_buckets.is_empty() {
        deposits.applied = true;
        return Ok(summary);
    }

    let values = field.as_mut_slice();
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

    let base_runs = workers.min(items / PACKETS_PER_RUN).max(1);
    let runs = if base_runs <= 1 {
        1
    } else {
        workers
            .saturating_mul(4)
            .min(64)
            .min(items / (PACKETS_PER_RUN / 2).max(1))
            .max(base_runs)
    };
    received.resize(occupied_buckets.len(), 0.0);
    let seq = (!*identity_sequence).then_some(sequence.as_slice());
    let (packets, directions) = (&**packets, &**directions);

    if runs == 1 {
        let mut run = Run {
            cell_base: 0,
            occupied_cells: occupied_buckets,
            energy,
            moment_count,
            moments: std::array::from_fn(|_| <&mut [f32]>::default()),
            received,
        };
        if moment_count > 0 {
            for (channel, plane) in moments.chunks_exact_mut(buckets.max(1)).enumerate() {
                if let Some(slot) = run.moments.get_mut(channel) {
                    *slot = plane;
                }
            }
        }
        let count = reduce_run(&mut run, offsets, order, seq, packets, directions);
        let mut applied = CompensatedSum::ZERO;
        for &value in run.received.get(..count).unwrap_or(&[]) {
            applied.add(value);
        }
        summary.cells = count;
        summary.applied = applied.value();
        deposits.applied = true;
        return Ok(summary);
    }

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

    let mut slots: &mut [f64] = received;
    let mut lanes = Vec::with_capacity(runs);
    let mut occ_cursor = 0_usize;
    for (run_idx, pair) in bounds.windows(2).enumerate() {
        let cell_start = pair[0];
        let cell_end = pair[1];
        let (head, tail) = cut(std::mem::take(&mut energy), cell_end - cell_start)?;
        energy = tail;
        let occ_end = if run_idx + 1 == runs {
            occupied_buckets.len()
        } else {
            occ_cursor
                + occupied_buckets[occ_cursor..].partition_point(|&c| (c as usize) < cell_end)
        };
        let lane_occupied = &occupied_buckets[occ_cursor..occ_end];
        occ_cursor = occ_end;
        let (owned, rest) = cut(std::mem::take(&mut slots), lane_occupied.len())?;
        slots = rest;
        lanes.push(Run {
            cell_base: cell_start,
            occupied_cells: lane_occupied,
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
                let (head, tail) = cut(std::mem::take(&mut plane), lane.energy.len())?;
                plane = tail;
                if let Some(slot) = lane.moments.get_mut(channel) {
                    *slot = head;
                }
            }
        }
    }

    let reduce = |run: &mut Run<'_>| reduce_run(run, offsets, order, seq, packets, directions);
    let mut occupied = vec![0_usize; lanes.len()];
    let mut work_tiles: Vec<(&mut Run<'_>, &mut usize)> =
        lanes.iter_mut().zip(occupied.iter_mut()).collect();
    flife_core::pool::parallel_tasks(
        workers,
        "apply deposits",
        &mut work_tiles,
        |_, (run, slot)| {
            **slot = reduce(run);
        },
    )?;

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
        workers: usize,
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
        // Single pass over compact `self.entries` (40 B/entry; 256 B
        // chemical directions live in `self.directions`) checks ascending
        // identity order, contiguous packet spans, and compensated paid/offered
        // totals simultaneously (7.4× less serial memory traffic, bit-identical).
        let mut strictly_ascending = true;
        let mut contiguous_spans = true;
        let mut prev_org: Option<OrganismId> = None;
        let mut cursor = 0_usize;
        let mut paid = CompensatedSum::ZERO;
        let mut offered = CompensatedSum::ZERO;
        for entry in &self.entries {
            if let Some(prev) = prev_org
                && !(prev < entry.organism)
            {
                strictly_ascending = false;
            }
            prev_org = Some(entry.organism);
            if entry.first != cursor {
                contiguous_spans = false;
            }
            cursor = cursor.saturating_add(entry.count);
            paid.add(entry.paid);
            offered.add(entry.offered);
        }
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
            identity_sequence,
            bins,
            ..
        } = self;
        if u32::try_from(packets.len()).is_err() {
            return Err(CoreError::Overflow {
                quantity: DEPOSIT_PACKETS,
            });
        }
        let buckets =
            usize::try_from(field.grid().cell_count()).map_err(|_| CoreError::Overflow {
                quantity: DEPOSIT_PACKETS,
            })?;
        let contiguous = strictly_ascending && contiguous_spans && cursor == packets.len();
        sequence.clear();
        if contiguous {
            *identity_sequence = true;
            bins.group_on(buckets, packets.len(), workers, |at| packets[at].cell)?;
        } else {
            *identity_sequence = false;
            if strictly_ascending {
                for entry in entries.iter() {
                    sequence.extend((entry.first..entry.first + entry.count).map(|at| at as u32));
                }
            } else {
                for &(_, index) in identities.iter() {
                    if let Some(entry) = entries.get(index) {
                        sequence
                            .extend((entry.first..entry.first + entry.count).map(|at| at as u32));
                    }
                }
            }
            let seq_slice = sequence.as_slice();
            let pkt_slice = packets.as_slice();
            bins.group_on(buckets, seq_slice.len(), workers, |at| {
                seq_slice
                    .get(at)
                    .and_then(|&packet| pkt_slice.get(packet as usize))
                    .map_or(usize::MAX, |packet| packet.cell)
            })?;
        }

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
        let (summary, cells) = self.prepare_for_carrier(field, carrier, 1)?;
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
        let packet_idx = if deposits.identity_sequence {
            index as usize
        } else {
            *deposits.sequence.get(index as usize)? as usize
        };
        let packet = deposits.packets.get(packet_idx)?;
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
        self.deposits.directions.get(index)
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
        for &cell_u32 in self.deposits.bins.occupied_buckets() {
            let cell = cell_u32 as usize;
            if let Some(&value) = received.get(cell) {
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

/// One thread's share of a reduction: a contiguous range of cells starting at
/// `cell_base`, the compact ascending list of occupied cells in that range,
/// their energy and their values in every moment plane, and a slot for what
/// each of its occupied cells received.
struct Run<'a> {
    cell_base: usize,
    occupied_cells: &'a [u32],
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

/// Flush `count` pre-scaled cell updates into `moments`, one plane at a time.
/// Classifies contiguous 4-cell quads once before the plane loop so dense
/// runs emit contiguous 16-byte NEON `ldr q` / `fcvtl` / `fadd.2d` / `fcvtn`
/// / `str q` updates while preserving exact `-0.0` semantics (`added != 0.0`).
#[inline(never)]
fn flush_moment_batch_const<const M: usize>(
    moments: &mut [&mut [f32]],
    count: usize,
    cell_at: &[usize; REDUCE_BATCH],
    accum: &[[f64; M]; REDUCE_BATCH],
) {
    let quads = count / 4;
    let tail_start = quads * 4;
    // Classify each 4-cell quad once across all `M` planes:
    // `2` = 4 contiguous cells (`i0..i0+4`), `1` = strictly ascending distinct, `0` = general.
    let mut quad_kind = [0_u8; REDUCE_BATCH / 4];
    for (q, kind) in quad_kind.iter_mut().enumerate().take(quads) {
        let b = q * 4;
        let i0 = cell_at[b];
        let i1 = cell_at[b + 1];
        let i2 = cell_at[b + 2];
        let i3 = cell_at[b + 3];
        if i0 + 1 == i1 && i1 + 1 == i2 && i2 + 1 == i3 {
            *kind = 2;
        } else if i0 < i1 && i1 < i2 && i2 < i3 {
            *kind = 1;
        }
    }
    for (channel, plane_slot) in moments.iter_mut().take(M).enumerate() {
        let plane: &mut [f32] = plane_slot;
        let plane_len = plane.len();
        for (q, &kind) in quad_kind.iter().enumerate().take(quads) {
            let b = q * 4;
            let a0 = accum[b][channel];
            let a1 = accum[b + 1][channel];
            let a2 = accum[b + 2][channel];
            let a3 = accum[b + 3][channel];
            let all_nonzero = (a0 != 0.0) & (a1 != 0.0) & (a2 != 0.0) & (a3 != 0.0);
            if kind == 2 && all_nonzero {
                let i0 = cell_at[b];
                if let Some(chunk) = plane.get_mut(i0..i0 + 4) {
                    let s0 = [f64::from(chunk[0]) + a0, f64::from(chunk[1]) + a1];
                    let s1 = [f64::from(chunk[2]) + a2, f64::from(chunk[3]) + a3];
                    chunk[0] = s0[0] as f32;
                    chunk[1] = s0[1] as f32;
                    chunk[2] = s1[0] as f32;
                    chunk[3] = s1[1] as f32;
                    continue;
                }
            } else if kind == 1 && all_nonzero {
                let i0 = cell_at[b];
                let i1 = cell_at[b + 1];
                let i2 = cell_at[b + 2];
                let i3 = cell_at[b + 3];
                if (i0 < i1) & (i1 < i2) & (i2 < i3) & (i3 < plane_len) {
                    let v01 = [f64::from(plane[i0]) + a0, f64::from(plane[i1]) + a1];
                    let v23 = [f64::from(plane[i2]) + a2, f64::from(plane[i3]) + a3];
                    plane[i0] = v01[0] as f32;
                    plane[i1] = v01[1] as f32;
                    plane[i2] = v23[0] as f32;
                    plane[i3] = v23[1] as f32;
                    continue;
                }
            }
            for k in 0..4 {
                let added = accum[b + k][channel];
                if added != 0.0
                    && let Some(slot) = plane.get_mut(cell_at[b + k])
                {
                    *slot = (f64::from(*slot) + added) as f32;
                }
            }
        }
        for b in tail_start..count {
            let added = accum[b][channel];
            if added != 0.0
                && let Some(slot) = plane.get_mut(cell_at[b])
            {
                *slot = (f64::from(*slot) + added) as f32;
            }
        }
    }
}

/// Fixed-channel specialization of [`reduce_run`] iterating only occupied cells
/// with compile-time channel count `M` (`32` single-carrier, `24` Carrier 0,
/// `8` Carrier 1) and compile-time `HAS_SEQ` specialization (hoisting the
/// identity-sequence branch out of the per-cell loop) so LLVM auto-vectorizes
/// the inner moment accumulation loop cleanly while preserving exact IEEE-754
/// order (`(amount * component) * factor`).
#[inline(never)]
fn reduce_run_const_inner<const M: usize, const HAS_SEQ: bool>(
    run: &mut Run<'_>,
    offsets: &[u32],
    order: &[u32],
    sequence: &[u32],
    packets: &[Packet],
    directions: &[[f64; CHANNELS]],
) -> usize {
    debug_assert!(M <= CHANNELS);
    let mut occupied = 0_usize;
    let mut batch_len = 0_usize;
    let mut cell_at = [0_usize; REDUCE_BATCH];
    let mut accum = [[0.0_f64; M]; REDUCE_BATCH];
    let cell_base = run.cell_base;
    let energy_len = run.energy.len();
    let mut received_iter = run.received.iter_mut();

    for &cell_u32 in run.occupied_cells {
        let cell = cell_u32 as usize;
        let at = cell.wrapping_sub(cell_base);
        if at >= energy_len {
            continue;
        }
        let Some(&[first_u, end_u]) = offsets
            .get(cell..cell + 2)
            .and_then(|pair| pair.first_chunk::<2>())
        else {
            continue;
        };
        let first = first_u as usize;
        let end = end_u as usize;
        if first >= end {
            continue;
        }
        let slice = &order[first..end];
        let before = run.energy[at];

        if slice.len() == 1 {
            // Fast path for single-packet cells: compute `after`, `received`,
            // and `factor` before touching `accum[batch_len]`, then fuse
            // `(amount * component) * factor` in registers in one pass
            // (preserving exact IEEE-754 order and skipping the 32-ch loop
            // entirely when `factor == 0.0`).
            let ord = slice[0] as usize;
            let pkt_idx = if HAS_SEQ { sequence[ord] as usize } else { ord };
            let packet = &packets[pkt_idx];
            let total = packet.amount;
            let after = round_down_f32(add_down(f64::from(before), total));
            let received = f64::from(after) - f64::from(before);
            let factor = if total > 0.0 && total.is_finite() {
                received / total
            } else {
                0.0
            };
            run.energy[at] = after;
            if let Some(rec_slot) = received_iter.next() {
                *rec_slot = received;
            }
            occupied += 1;

            if factor != 0.0 {
                let moment = &mut accum[batch_len];
                let dir = &directions[packet.entry][..M];
                for (slot, &component) in moment.iter_mut().zip(dir) {
                    *slot = (total * component) * factor;
                }
                cell_at[batch_len] = at;
                batch_len += 1;
                if batch_len == REDUCE_BATCH {
                    flush_moment_batch_const::<M>(
                        &mut run.moments[..M],
                        batch_len,
                        &cell_at,
                        &accum,
                    );
                    batch_len = 0;
                }
            }
            continue;
        }

        let moment = &mut accum[batch_len];
        let mut total = 0.0_f64;
        moment.fill(0.0);
        for &ord_u in slice {
            let ord = ord_u as usize;
            let pkt_idx = if HAS_SEQ { sequence[ord] as usize } else { ord };
            let packet = &packets[pkt_idx];
            let amount = packet.amount;
            total = add_down(total, amount);
            let dir = &directions[packet.entry][..M];
            for (slot, &component) in moment.iter_mut().zip(dir) {
                *slot += amount * component;
            }
        }
        let after = round_down_f32(add_down(f64::from(before), total));
        let received = f64::from(after) - f64::from(before);
        let factor = if total > 0.0 && total.is_finite() {
            received / total
        } else {
            0.0
        };
        run.energy[at] = after;
        if let Some(rec_slot) = received_iter.next() {
            *rec_slot = received;
        }
        occupied += 1;

        if factor != 0.0 {
            for slot in moment.iter_mut() {
                *slot *= factor;
            }
            cell_at[batch_len] = at;
            batch_len += 1;
            if batch_len == REDUCE_BATCH {
                flush_moment_batch_const::<M>(&mut run.moments[..M], batch_len, &cell_at, &accum);
                batch_len = 0;
            }
        }
    }
    if batch_len > 0 {
        flush_moment_batch_const::<M>(&mut run.moments[..M], batch_len, &cell_at, &accum);
    }
    occupied
}

/// Fixed-channel specialization of [`reduce_run`] dispatching once on
/// `sequence.is_some()` outside the occupied-cell loop.
fn reduce_run_const<const M: usize>(
    run: &mut Run<'_>,
    offsets: &[u32],
    order: &[u32],
    sequence: Option<&[u32]>,
    packets: &[Packet],
    directions: &[[f64; CHANNELS]],
) -> usize {
    match sequence {
        None => reduce_run_const_inner::<M, false>(run, offsets, order, &[], packets, directions),
        Some(seq) => {
            reduce_run_const_inner::<M, true>(run, offsets, order, seq, packets, directions)
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
    sequence: Option<&[u32]>,
    packets: &[Packet],
    directions: &[[f64; CHANNELS]],
) -> usize {
    let moment_count = run.moment_count.min(CHANNELS);
    match moment_count {
        CHANNELS => {
            reduce_run_const::<CHANNELS>(run, offsets, order, sequence, packets, directions)
        }
        24 => reduce_run_const::<24>(run, offsets, order, sequence, packets, directions),
        8 => reduce_run_const::<8>(run, offsets, order, sequence, packets, directions),
        _ => {
            let mut occupied = 0_usize;
            let mut batch_len = 0_usize;
            let mut cell_at = [0_usize; REDUCE_BATCH];
            let mut accum = [[0.0_f64; CHANNELS]; REDUCE_BATCH];
            let cell_base = run.cell_base;

            for &cell_u32 in run.occupied_cells {
                let cell = cell_u32 as usize;
                let Some(at) = cell.checked_sub(cell_base) else {
                    continue;
                };
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
                let mut total = 0.0_f64;
                let mut moment = [0.0_f64; CHANNELS];
                for &ord in slice {
                    let pkt_idx = match sequence {
                        None => ord as usize,
                        Some(seq_slice) => match seq_slice.get(ord as usize) {
                            Some(&seq) => seq as usize,
                            None => continue,
                        },
                    };
                    let Some(packet) = packets.get(pkt_idx) else {
                        continue;
                    };
                    total = add_down(total, packet.amount);
                    if let Some(dir) = directions.get(packet.entry) {
                        let amount = packet.amount;
                        for (slot, &component) in moment.iter_mut().zip(dir).take(moment_count) {
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
                    for slot in moment.iter_mut().take(moment_count) {
                        *slot *= factor;
                    }
                    cell_at[batch_len] = at;
                    accum[batch_len] = moment;
                    batch_len += 1;
                    if batch_len == REDUCE_BATCH {
                        flush_moment_batch_const::<CHANNELS>(
                            &mut run.moments[..moment_count],
                            batch_len,
                            &cell_at,
                            &accum,
                        );
                        batch_len = 0;
                    }
                }
            }
            if batch_len > 0 {
                flush_moment_batch_const::<CHANNELS>(
                    &mut run.moments[..moment_count],
                    batch_len,
                    &cell_at,
                    &accum,
                );
            }
            occupied
        }
    }
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
