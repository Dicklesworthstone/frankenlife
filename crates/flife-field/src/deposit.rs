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
//! - Packets are sorted by (cell, organism identity). Each cell sums its
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
use flife_core::model::MOMENT_CHANNELS;

use crate::exchange::ExchangeStencil;
use crate::grid::FieldGrid;
use crate::state::{CHANNELS, FieldState};

/// One organism's staged deposit.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Entry {
    organism: OrganismId,
    paid: f64,
    offered: f64,
    direction: [f64; CHANNELS],
}

/// One cell's share of an entry.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Packet {
    cell: usize,
    organism: OrganismId,
    entry: usize,
    amount: f64,
}

/// One cell's reduced deposit, before it is written.
#[derive(Debug, Clone, Copy, PartialEq)]
struct CellDeposit {
    cell: usize,
    after: f32,
    moment: [f64; CHANNELS],
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
#[derive(Debug, Clone, Default)]
pub struct Deposits {
    grid: Option<FieldGrid>,
    entries: Vec<Entry>,
    packets: Vec<Packet>,
    cells: Vec<CellDeposit>,
    identities: Vec<OrganismId>,
    applied: bool,
}

impl Deposits {
    /// No deposits.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget every staged deposit, keeping the buffers.
    pub fn clear(&mut self) {
        self.grid = None;
        self.entries.clear();
        self.packets.clear();
        self.cells.clear();
        self.identities.clear();
        self.applied = false;
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

    /// Stage `organism`'s deposit. It paid `paid` and offers `offered ≤ paid`
    /// to the cells of its footprint at `position` (wrapped onto the torus),
    /// with chemical `direction`, `||direction||₂ ≤ 1`.
    ///
    /// # Errors
    /// - [`CoreError::StateConflict`] when the deposits were already
    ///   applied, or were staged for another grid.
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
        let stencil = ExchangeStencil::at(grid, position)?;
        let entry = self.entries.len();
        let shares = split(&stencil, offered);
        self.grid = Some(*grid);
        self.entries.push(Entry {
            organism,
            paid: paid + 0.0,
            offered: offered + 0.0,
            direction: *direction,
        });
        for (&cell, &amount) in stencil.cells().iter().zip(&shares) {
            if amount > 0.0 {
                self.packets.push(Packet {
                    cell,
                    organism,
                    entry,
                    amount,
                });
            }
        }
        Ok(())
    }
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

/// `a + b`, rounded so that it is never below the exact sum.
fn add_up(a: f64, b: f64) -> f64 {
    -add_down(-a, -b)
}

/// The largest F32 value at or below `value`, for finite `value ≥ 0`.
fn round_down_f32(value: f64) -> f32 {
    let nearest = value as f32;
    if f64::from(nearest) > value {
        nearest.next_down()
    } else {
        nearest
    }
}

/// Reduce `deposits` into `field`, the next state, and return what was paid,
/// offered and applied. The deposits are marked applied.
///
/// # Errors
/// [`CoreError::StateConflict`] when the deposits were already applied,
/// were staged for another grid, name one organism twice, or the field
/// lacks a moment plane; [`CoreError::LayoutMismatch`] if a cell falls
/// outside `field`. Nothing is written on error.
pub fn apply_deposits(
    field: &mut FieldState,
    deposits: &mut Deposits,
) -> Result<DepositSummary, CoreError> {
    let conflict = |detail: String| CoreError::StateConflict {
        operation: "apply deposits",
        detail,
    };
    if deposits.applied {
        return Err(conflict("these deposits were already applied".into()));
    }
    if deposits.grid.is_some_and(|grid| grid != *field.grid()) {
        return Err(conflict(
            "these deposits were staged for another grid".into(),
        ));
    }
    deposits.identities.clear();
    deposits
        .identities
        .extend(deposits.entries.iter().map(|entry| entry.organism));
    deposits.identities.sort_unstable();
    if let Some(pair) = deposits
        .identities
        .windows(2)
        .find(|pair| pair[0] == pair[1])
    {
        return Err(conflict(format!(
            "{} staged two deposits in one reduction",
            pair[0]
        )));
    }
    if (0..MOMENT_CHANNELS).any(|channel| field.moment(channel).is_none()) {
        return Err(conflict("a moment plane is missing".into()));
    }

    let mut summary = DepositSummary {
        organisms: deposits.entries.len(),
        ..DepositSummary::default()
    };
    let mut paid = CompensatedSum::ZERO;
    let mut offered = CompensatedSum::ZERO;
    for entry in &deposits.entries {
        paid.add(entry.paid);
        offered.add(entry.offered);
    }
    summary.paid = paid.value();
    summary.offered = offered.value();

    deposits
        .packets
        .sort_unstable_by_key(|packet| (packet.cell, packet.organism));
    deposits.cells.clear();
    let energy = field.energy();
    let mut applied = CompensatedSum::ZERO;
    for group in deposits
        .packets
        .chunk_by(|left, right| left.cell == right.cell)
    {
        let Some(&Packet { cell, .. }) = group.first() else {
            continue;
        };
        let before = energy
            .get(cell)
            .copied()
            .ok_or_else(|| CoreError::LayoutMismatch {
                buffer: crate::layout::BUFFER,
                detail: format!("deposit cell {cell} is not in this field"),
            })?;
        let mut total = 0.0;
        let mut moment = [0.0; CHANNELS];
        for packet in group {
            total = add_down(total, packet.amount);
            let direction = deposits
                .entries
                .get(packet.entry)
                .map_or(&[0.0; CHANNELS], |entry| &entry.direction);
            for (slot, &component) in moment.iter_mut().zip(direction) {
                *slot += packet.amount * component;
            }
        }
        let after = round_down_f32(add_down(f64::from(before), total));
        let received = f64::from(after) - f64::from(before);
        let factor = if total > 0.0 { received / total } else { 0.0 };
        for slot in &mut moment {
            *slot *= factor;
        }
        applied.add(received);
        deposits.cells.push(CellDeposit {
            cell,
            after,
            moment,
        });
    }
    summary.cells = deposits.cells.len();
    summary.applied = applied.value();

    let energy = field.energy_mut();
    for deposit in &deposits.cells {
        energy[deposit.cell] = deposit.after;
    }
    for channel in 0..MOMENT_CHANNELS {
        let plane = field
            .moment_mut(channel)
            .ok_or_else(|| conflict(format!("moment plane {channel} is missing")))?;
        let index = usize::try_from(channel).unwrap_or(usize::MAX);
        for deposit in &deposits.cells {
            let added = deposit.moment.get(index).copied().unwrap_or(0.0);
            if added != 0.0 {
                let moment = &mut plane[deposit.cell];
                *moment = (f64::from(*moment) + added) as f32;
            }
        }
    }
    deposits.applied = true;
    Ok(summary)
}
