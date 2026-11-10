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
//! Applying grants debits each cell's energy and scales its moment by
//! `e'_c/e_c`, which removes the same proportion of composition. Grants
//! carry the pre-debit cell energies. Applying them to a field that no
//! longer holds those values, or applying them twice, is refused before
//! anything is written.

use flife_core::CoreError;
use flife_core::ids::OrganismId;
use flife_core::ledger::{CompensatedSum, add_down};
use flife_core::memory::{capacity_bytes, total};
use flife_core::model::MOMENT_CHANNELS;

use crate::bins::Bins;
use crate::exchange::ExchangeStencil;
use crate::footprint::BufferLimits;
use crate::grid::FieldGrid;
use crate::state::FieldState;

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
        }
    }

    /// Empty grants declared for at most `organisms` demands a resolution on
    /// `grid` (§9.4). Their scratch is admitted against `limits` and
    /// allocated now, so a resolution within the declaration never
    /// allocates, and one beyond it is refused.
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
        let overflow = || CoreError::Overflow {
            quantity: UPTAKE_PACKETS,
        };
        let packets = organisms
            .checked_mul(PACKETS_PER_DEMAND)
            .ok_or_else(overflow)?;
        let packet_bytes = packets
            .checked_mul(std::mem::size_of::<GrantPacket>() as u64)
            .ok_or_else(overflow)?;
        limits.admit(UPTAKE_PACKETS, packet_bytes)?;
        let bins = Bins::with_capacity(UPTAKE_PACKETS, grid.cell_count(), packets, limits)?;
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

/// Check one demand and take its exchange footprint.
fn footprint(grid: &FieldGrid, demand: &Demand) -> Result<ExchangeStencil, CoreError> {
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
    ExchangeStencil::at(grid, demand.position)
}

/// Resolve `demands` against the immutable `field` snapshot into `grants`.
/// The field is not changed; [`apply_grants`] debits it.
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
    grants.reset();
    let result = resolve_into(field, demands, grants);
    if result.is_err() {
        grants.reset();
    }
    result
}

fn resolve_into(
    field: &FieldState,
    demands: &[Demand],
    grants: &mut Grants,
) -> Result<UptakeSummary, CoreError> {
    let grid = *field.grid();
    if demands.len() > grants.organisms {
        return Err(CoreError::BudgetExhausted {
            budget: UPTAKE_PACKETS,
            required: u64::try_from(demands.len()).unwrap_or(u64::MAX),
            available: u64::try_from(grants.organisms).unwrap_or(u64::MAX),
        });
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
        let stencil = match footprint(&grid, demand) {
            Ok(stencil) => stencil,
            // Report the first invalid demand in the order given.
            Err(error) => {
                return Err(demands
                    .iter()
                    .find_map(|demand| footprint(&grid, demand).err())
                    .unwrap_or(error));
            }
        };
        for (&cell, &weight) in stencil.cells().iter().zip(stencil.weights()) {
            let requested = demand.amount * weight;
            if requested > 0.0 {
                grants.staged.push(GrantPacket {
                    cell,
                    organism: demand.organism,
                    demand: index,
                    requested,
                    granted: 0.0,
                });
            }
        }
    }
    let Grants {
        staged,
        bins,
        packets,
        debits,
        shares,
        rank,
        ..
    } = grants;
    let cells = usize::try_from(grid.cell_count()).map_err(|_| CoreError::Overflow {
        quantity: UPTAKE_PACKETS,
    })?;
    bins.group(cells, staged.len(), |at| {
        staged.get(at).map_or(usize::MAX, |packet| packet.cell)
    })?;
    packets.extend(
        bins.order()
            .iter()
            .filter_map(|&at| staged.get(at as usize).copied()),
    );

    let energy = field.energy();
    let mut summary = UptakeSummary::default();
    let (mut requested, mut granted) = (CompensatedSum::ZERO, CompensatedSum::ZERO);
    for (cell, range) in bins.occupied() {
        let value = energy
            .get(cell)
            .copied()
            .ok_or_else(|| CoreError::LayoutMismatch {
                buffer: "field",
                detail: format!("exchange cell {cell} is not in this field"),
            })?;
        let length = range.len();
        let Some(group) = packets.get_mut(range) else {
            continue;
        };
        let debit = resolve_cell(group, cell, value, shares, rank)?;
        requested.add(debit.demanded);
        granted.add(debit.granted);
        summary.cells += 1;
        summary.contended_cells += usize::from(debit.contended);
        summary.drained_cells += usize::from(debit.granted > 0.0 && debit.energy_after == 0.0);
        summary.largest_cell_packets = summary.largest_cell_packets.max(length);
        debits.push(debit);
    }
    summary.requested = requested.value();
    summary.granted = granted.value();

    grants.totals.resize(demands.len(), 0.0);
    for packet in &grants.packets {
        let total = &mut grants.totals[packet.demand];
        *total = add_down(*total, packet.granted);
    }
    grants.summary = summary;
    grants.grid = Some(grid);
    Ok(summary)
}

/// Resolve one cell's packets, which are in identity order.
fn resolve_cell(
    packets: &mut [GrantPacket],
    cell: usize,
    energy: f32,
    shares: &mut Vec<Share>,
    rank: &mut Vec<usize>,
) -> Result<CellDebit, CoreError> {
    if !energy.is_finite() || energy < 0.0 {
        return Err(CoreError::NumericalInvalid {
            quantity: "field energy",
            detail: format!("cell {cell} holds {energy}"),
        });
    }
    let mut demand = CompensatedSum::ZERO;
    for packet in packets.iter() {
        demand.add(packet.requested);
    }
    let demanded = demand.value();
    let contended = demanded > f64::from(energy);
    let (quantum, quanta) = quanta_of(energy);
    if quanta == 0 {
        return Ok(CellDebit {
            cell,
            energy_before: energy,
            energy_after: energy,
            demanded,
            granted: 0.0,
            contended,
        });
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

    shares.clear();
    let mut floors = 0_u64;
    for packet in packets.iter() {
        let ideal = packet.requested * scale;
        let floor = ideal.floor();
        // `ideal ≤ n_c (1 + rounding) < 2^25`.
        let share = floor as u64;
        floors += share;
        shares.push(Share {
            quanta: share,
            remainder: ideal - floor,
        });
    }
    rank.clear();
    rank.extend(0..packets.len());
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
    } else if floors > target {
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
    for (packet, share) in packets.iter_mut().zip(shares.iter()) {
        packet.granted = share.quanta as f64 * quantum;
    }
    // `(n_c − K_c) Q_c` is an integer below 2^24 times a power of two at
    // least 2^-149, so it is exact in F32.
    let energy_after = ((quanta - target) as f64 * quantum) as f32;
    Ok(CellDebit {
        cell,
        energy_before: energy,
        energy_after,
        demanded,
        granted: target as f64 * quantum,
        contended,
    })
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
/// the cell is drained). Returns the energy debited.
///
/// # Errors
/// [`CoreError::StateConflict`] when the grants were already applied, were
/// resolved for another grid, or any debited cell no longer holds the
/// energy they were resolved against. Nothing is written.
pub fn apply_grants(field: &mut FieldState, grants: &mut Grants) -> Result<f64, CoreError> {
    let conflict = |detail: String| CoreError::StateConflict {
        operation: "uptake grants",
        detail,
    };
    if grants.applied {
        return Err(conflict("these grants were already applied".into()));
    }
    if grants.grid != Some(*field.grid()) {
        return Err(conflict(
            "these grants were resolved for another grid".into(),
        ));
    }
    let energy = field.energy();
    for debit in &grants.debits {
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
    let energy = field.energy_mut();
    for debit in grants.debits.iter().filter(|debit| debit.granted > 0.0) {
        energy[debit.cell] = debit.energy_after;
    }
    for channel in 0..MOMENT_CHANNELS {
        let plane = field
            .moment_mut(channel)
            .ok_or_else(|| conflict(format!("moment plane {channel} is missing")))?;
        for debit in grants.debits.iter().filter(|debit| debit.granted > 0.0) {
            let moment = &mut plane[debit.cell];
            *moment = if debit.energy_after == 0.0 {
                0.0
            } else {
                let ratio = f64::from(debit.energy_after) / f64::from(debit.energy_before);
                (f64::from(*moment) * ratio) as f32
            };
        }
    }
    grants.applied = true;
    Ok(grants.summary.granted)
}
