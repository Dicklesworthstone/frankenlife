//! The global energy ledger (plan §4.4, §8.2, §21.2 INV-03 and INV-05,
//! §21.4).
//!
//! Spendable energy is `𝓔 = Σ_i E_i + Σ_c e_c`, organisms plus field. Every
//! committed tick must satisfy `𝓔_{n+1} − 𝓔_n = I_n − O_n − D_n + ε_n`:
//! declared injection, minus declared removal, minus dissipation, plus a
//! numerical residual that a declared [`ResidualBudget`] bounds.
//!
//! - A [`Flow`] is one named external or dissipative term, and belongs to
//!   exactly one [`FlowClass`].
//! - A [`Transfer`] moves energy inside the world: uptake, emission, a
//!   child's endowment or recycling. It debits one holding, credits
//!   another, and charges the difference to its named loss flow. It never
//!   credits more than it debits, so no internal transfer creates energy.
//! - A [`TickLedger`] opens from measured holdings, receives the tick's
//!   flows and transfers, and closes against measured holdings. A residual
//!   outside the budget is an error in every build profile, never a
//!   warning, and the closure records the budget it was checked against.
//! - The run's [`EnergyLedger`] requires each tick to open exactly where the
//!   previous one closed, so a change made between ticks without a flow is
//!   caught too. It checks the cumulative residual and reports it against
//!   initial energy plus turnover, against initial and current energy alone,
//!   and as a drift slope (§21.4).
//! - An [`EnergyBudget`] is one organism's spendable energy within a tick.
//!   It never pays out more than it holds (INV-05), and it keeps the
//!   requested and paid amounts apart (§8.2).
//!
//! Sums are compensated (Neumaier) and run in a fixed order: flows in
//! declaration order, amounts in the order they were recorded.

use crate::clock::Tick;
use crate::error::CoreError;

/// The class of a [`Flow`] (§4.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FlowClass {
    /// Declared external injection, `I`.
    Injection,
    /// Declared removal from the modeled world, `O`.
    Removal,
    /// Dissipation, `D`.
    Dissipation,
}

/// A named external or dissipative energy flow (§4.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Flow {
    /// Scheduled external sources (phase 2).
    SourcePower,
    /// The initial population's endowment.
    InitialEndowment,
    /// Energy added by a recorded command, such as an energy brush.
    CommandInjection,
    /// Energy removed from the world by a recorded command.
    CommandRemoval,
    /// Field decay.
    Decay,
    /// Granted energy an organism did not convert: `U − G`.
    UptakeInefficiency,
    /// Internal energy destroyed by toxicity.
    Toxicity,
    /// Paid maintenance.
    Maintenance,
    /// Paid controller cost.
    ControllerCost,
    /// Paid sensor cost.
    SensorCost,
    /// Paid motion cost.
    MotionCost,
    /// Paid emission that did not reach the field.
    EmissionLoss,
    /// Reproduction energy the parent paid that did not reach the child.
    ReproductionLoss,
    /// A dead organism's energy that was not recycled into the field.
    UnrecycledDeath,
    /// A reservoir cleared by the tiny-energy policy.
    TinyReservoir,
}

/// The number of [`Flow`] kinds.
pub const FLOW_KINDS: usize = Flow::ALL.len();

impl Flow {
    /// Every flow, in the fixed reporting and summation order.
    pub const ALL: [Self; 15] = [
        Self::SourcePower,
        Self::InitialEndowment,
        Self::CommandInjection,
        Self::CommandRemoval,
        Self::Decay,
        Self::UptakeInefficiency,
        Self::Toxicity,
        Self::Maintenance,
        Self::ControllerCost,
        Self::SensorCost,
        Self::MotionCost,
        Self::EmissionLoss,
        Self::ReproductionLoss,
        Self::UnrecycledDeath,
        Self::TinyReservoir,
    ];

    /// The class this flow belongs to.
    #[must_use]
    pub const fn class(self) -> FlowClass {
        match self {
            Self::SourcePower | Self::InitialEndowment | Self::CommandInjection => {
                FlowClass::Injection
            }
            Self::CommandRemoval => FlowClass::Removal,
            Self::Decay
            | Self::UptakeInefficiency
            | Self::Toxicity
            | Self::Maintenance
            | Self::ControllerCost
            | Self::SensorCost
            | Self::MotionCost
            | Self::EmissionLoss
            | Self::ReproductionLoss
            | Self::UnrecycledDeath
            | Self::TinyReservoir => FlowClass::Dissipation,
        }
    }

    /// The flow's canonical name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::SourcePower => "source-power",
            Self::InitialEndowment => "initial-endowment",
            Self::CommandInjection => "command-injection",
            Self::CommandRemoval => "command-removal",
            Self::Decay => "decay",
            Self::UptakeInefficiency => "uptake-inefficiency",
            Self::Toxicity => "toxicity",
            Self::Maintenance => "maintenance",
            Self::ControllerCost => "controller-cost",
            Self::SensorCost => "sensor-cost",
            Self::MotionCost => "motion-cost",
            Self::EmissionLoss => "emission-loss",
            Self::ReproductionLoss => "reproduction-loss",
            Self::UnrecycledDeath => "unrecycled-death",
            Self::TinyReservoir => "tiny-reservoir",
        }
    }

    const fn index(self) -> usize {
        self as usize
    }
}

/// An internal transfer (§4.4). It debits one holding, credits another and
/// charges the difference to [`Transfer::loss`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Transfer {
    /// Field to organism: granted uptake `U` credited as gain `G`.
    Uptake,
    /// Organism to field: paid emission credited as a deposit.
    Emission,
    /// Parent to child at a birth.
    ChildEndowment,
    /// A dead organism's energy returned to the field.
    Recycling,
}

/// The number of [`Transfer`] kinds.
pub const TRANSFER_KINDS: usize = Transfer::ALL.len();

impl Transfer {
    /// Every transfer, in the fixed reporting and summation order.
    pub const ALL: [Self; 4] = [
        Self::Uptake,
        Self::Emission,
        Self::ChildEndowment,
        Self::Recycling,
    ];

    /// The dissipation flow this transfer's loss is charged to.
    #[must_use]
    pub const fn loss(self) -> Flow {
        match self {
            Self::Uptake => Flow::UptakeInefficiency,
            Self::Emission => Flow::EmissionLoss,
            Self::ChildEndowment => Flow::ReproductionLoss,
            Self::Recycling => Flow::UnrecycledDeath,
        }
    }

    /// The transfer's canonical name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Uptake => "uptake",
            Self::Emission => "emission",
            Self::ChildEndowment => "child-endowment",
            Self::Recycling => "recycling",
        }
    }

    const fn index(self) -> usize {
        self as usize
    }
}

/// Measured spendable energy: field plus organisms.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Holdings {
    field: f64,
    organisms: f64,
}

impl Holdings {
    /// Holdings of `field` energy in the field and `organisms` energy in
    /// live organisms. `-0.0` is stored as `0.0`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] unless both are finite and
    /// nonnegative.
    pub fn new(field: f64, organisms: f64) -> Result<Self, CoreError> {
        Ok(Self {
            field: energy("field holdings", field)?,
            organisms: energy("organism holdings", organisms)?,
        })
    }

    /// Energy held by the field.
    #[must_use]
    pub const fn field(&self) -> f64 {
        self.field
    }

    /// Energy held by live organisms.
    #[must_use]
    pub const fn organisms(&self) -> f64 {
        self.organisms
    }

    /// `𝓔 = field + organisms`.
    #[must_use]
    pub fn total(&self) -> f64 {
        self.field + self.organisms
    }

    fn same(&self, other: &Self) -> bool {
        self.field.to_bits() == other.field.to_bits()
            && self.organisms.to_bits() == other.organisms.to_bits()
    }
}

/// A Neumaier-compensated running sum. Adding the same values in the same
/// order gives the same bits on every IEEE-754 host.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CompensatedSum {
    sum: f64,
    compensation: f64,
}

impl CompensatedSum {
    /// A sum restored from its running `sum` and `compensation`, as
    /// [`CompensatedSum::parts`] gives them.
    #[must_use]
    pub const fn from_parts([sum, compensation]: [f64; 2]) -> Self {
        Self { sum, compensation }
    }

    /// The running sum and its compensation.
    #[must_use]
    pub const fn parts(&self) -> [f64; 2] {
        [self.sum, self.compensation]
    }

    /// The empty sum.
    pub const ZERO: Self = Self {
        sum: 0.0,
        compensation: 0.0,
    };

    /// Add `value`.
    pub fn add(&mut self, value: f64) {
        let total = self.sum + value;
        if self.sum.abs() >= value.abs() {
            self.compensation += (self.sum - total) + value;
        } else {
            self.compensation += (value - total) + self.sum;
        }
        self.sum = total;
    }

    /// The compensated total.
    #[must_use]
    pub fn value(&self) -> f64 {
        self.sum + self.compensation
    }
}

/// `a + b` rounded so that it never exceeds the exact sum: the rounded sum
/// when it is exact or below, otherwise the next float below it. For finite
/// arguments whose sum does not overflow.
#[must_use]
pub fn add_down(a: f64, b: f64) -> f64 {
    let sum = a + b;
    // TwoSum: `error` is exactly `(a + b) − sum`.
    let b_part = sum - a;
    let a_part = sum - b_part;
    let error = (a - a_part) + (b - b_part);
    if error < 0.0 { sum.next_down() } else { sum }
}

/// `a − b`, rounded so that it never exceeds the exact difference.
#[must_use]
pub fn sub_down(a: f64, b: f64) -> f64 {
    add_down(a, -b)
}

/// A declared residual budget (§21.4). A tick's residual may be at most
/// `absolute + per_tick × scale`, where the scale is the tick's opening
/// energy plus its turnover. The run's cumulative residual may be at most
/// `absolute + cumulative × scale`, where the scale is the initial energy
/// plus the cumulative turnover.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResidualBudget {
    absolute: f64,
    per_tick: f64,
    cumulative: f64,
}

impl ResidualBudget {
    /// No residual at all, for exact fixtures.
    pub const EXACT: Self = Self {
        absolute: 0.0,
        per_tick: 0.0,
        cumulative: 0.0,
    };

    /// A budget with an `absolute` floor in ecological-energy and relative
    /// per-tick and cumulative bounds.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] unless all three are finite and
    /// nonnegative.
    pub fn new(absolute: f64, per_tick: f64, cumulative: f64) -> Result<Self, CoreError> {
        Ok(Self {
            absolute: energy("absolute residual budget", absolute)?,
            per_tick: energy("per-tick relative residual budget", per_tick)?,
            cumulative: energy("cumulative relative residual budget", cumulative)?,
        })
    }

    /// The absolute floor.
    #[must_use]
    pub const fn absolute(&self) -> f64 {
        self.absolute
    }

    /// The per-tick relative bound.
    #[must_use]
    pub const fn per_tick(&self) -> f64 {
        self.per_tick
    }

    /// The cumulative relative bound.
    #[must_use]
    pub const fn cumulative(&self) -> f64 {
        self.cumulative
    }

    fn tick_allowance(&self, scale: f64) -> f64 {
        self.absolute + self.per_tick * scale
    }

    fn cumulative_allowance(&self, scale: f64) -> f64 {
        self.absolute + self.cumulative * scale
    }
}

/// One tick's flows, between measured opening and closing holdings.
#[derive(Debug, Clone)]
pub struct TickLedger {
    tick: Tick,
    opening: Holdings,
    budget: ResidualBudget,
    flows: [CompensatedSum; FLOW_KINDS],
    debited: [CompensatedSum; TRANSFER_KINDS],
    credited: [CompensatedSum; TRANSFER_KINDS],
}

impl TickLedger {
    /// Open `tick` at the measured `opening` holdings. The closure will be
    /// checked against `budget`.
    #[must_use]
    pub fn open(tick: Tick, opening: Holdings, budget: ResidualBudget) -> Self {
        Self {
            tick,
            opening,
            budget,
            flows: [CompensatedSum::ZERO; FLOW_KINDS],
            debited: [CompensatedSum::ZERO; TRANSFER_KINDS],
            credited: [CompensatedSum::ZERO; TRANSFER_KINDS],
        }
    }

    /// The tick being accounted.
    #[must_use]
    pub const fn tick(&self) -> Tick {
        self.tick
    }

    /// Record `amount` of an external or dissipative `flow`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] unless `amount` is finite and
    /// nonnegative. Nothing is recorded.
    pub fn record(&mut self, flow: Flow, amount: f64) -> Result<(), CoreError> {
        let amount = energy(flow.name(), amount)?;
        self.flows[flow.index()].add(amount);
        Ok(())
    }

    /// Record an internal transfer that debited `debited` from one holding
    /// and credited `credited` to another. The difference is charged to
    /// [`Transfer::loss`] and returned.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] unless both are finite and
    /// nonnegative and `credited ≤ debited`: a transfer that would create
    /// energy is refused. Nothing is recorded.
    pub fn transfer(
        &mut self,
        transfer: Transfer,
        debited: f64,
        credited: f64,
    ) -> Result<f64, CoreError> {
        let debited = energy(transfer.name(), debited)?;
        let credited = energy(transfer.name(), credited)?;
        if credited > debited {
            return Err(CoreError::NumericalInvalid {
                quantity: transfer.name(),
                detail: format!(
                    "credits {credited} but debits only {debited}; an internal transfer cannot create energy"
                ),
            });
        }
        let loss = debited - credited;
        self.debited[transfer.index()].add(debited);
        self.credited[transfer.index()].add(credited);
        self.flows[transfer.loss().index()].add(loss);
        Ok(loss)
    }

    /// Close the tick against the measured `closing` holdings.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] when `|ε|` exceeds the budget's
    /// per-tick allowance (INV-03). The error names the residual, the
    /// allowance, the scale and the budget.
    pub fn close(self, closing: Holdings) -> Result<TickClosure, CoreError> {
        let flows = self.flows.map(|sum| sum.value());
        let mut class_sums = [CompensatedSum::ZERO; 3];
        for flow in Flow::ALL {
            let class = match flow.class() {
                FlowClass::Injection => 0,
                FlowClass::Removal => 1,
                FlowClass::Dissipation => 2,
            };
            class_sums[class].add(flows[flow.index()]);
        }
        let [injected, removed, dissipated] = class_sums.map(|sum| sum.value());
        let debited = self.debited.map(|sum| sum.value());
        let credited = self.credited.map(|sum| sum.value());
        let mut moved = CompensatedSum::ZERO;
        for amount in credited {
            moved.add(amount);
        }
        let moved = moved.value();
        let mut turnover = CompensatedSum::ZERO;
        for amount in [injected, removed, dissipated, moved] {
            turnover.add(amount);
        }
        let turnover = turnover.value();
        let residual =
            (closing.total() - self.opening.total()) - ((injected - removed) - dissipated);
        let scale = self.opening.total() + turnover;
        let allowance = self.budget.tick_allowance(scale);
        if residual.abs() > allowance || !residual.is_finite() {
            return Err(CoreError::NumericalInvalid {
                quantity: "energy ledger residual (INV-03)",
                detail: format!(
                    "tick {}: ε = {residual:e} exceeds the allowance {allowance:e} (scale {scale:e}; budget {:?})",
                    self.tick.get(),
                    self.budget
                ),
            });
        }
        Ok(TickClosure {
            tick: self.tick,
            opening: self.opening,
            closing,
            budget: self.budget,
            flows,
            debited,
            credited,
            injected,
            removed,
            dissipated,
            moved,
            turnover,
            residual,
            scale,
            allowance,
        })
    }
}

/// A closed tick. Only [`TickLedger::close`] makes one, so a closure
/// always passed its per-tick check.
#[derive(Debug, Clone, PartialEq)]
pub struct TickClosure {
    tick: Tick,
    opening: Holdings,
    closing: Holdings,
    budget: ResidualBudget,
    flows: [f64; FLOW_KINDS],
    debited: [f64; TRANSFER_KINDS],
    credited: [f64; TRANSFER_KINDS],
    injected: f64,
    removed: f64,
    dissipated: f64,
    moved: f64,
    turnover: f64,
    residual: f64,
    scale: f64,
    allowance: f64,
}

impl TickClosure {
    /// The tick.
    #[must_use]
    pub const fn tick(&self) -> Tick {
        self.tick
    }

    /// Measured holdings when the tick opened.
    #[must_use]
    pub const fn opening(&self) -> Holdings {
        self.opening
    }

    /// Measured holdings when the tick closed.
    #[must_use]
    pub const fn closing(&self) -> Holdings {
        self.closing
    }

    /// The budget the residual was checked against.
    #[must_use]
    pub const fn budget(&self) -> ResidualBudget {
        self.budget
    }

    /// The total recorded for `flow`, including transfer losses charged to
    /// it.
    #[must_use]
    pub const fn flow(&self, flow: Flow) -> f64 {
        self.flows[flow.index()]
    }

    /// The total debited by `transfer`.
    #[must_use]
    pub const fn debited(&self, transfer: Transfer) -> f64 {
        self.debited[transfer.index()]
    }

    /// The total credited by `transfer`.
    #[must_use]
    pub const fn credited(&self, transfer: Transfer) -> f64 {
        self.credited[transfer.index()]
    }

    /// `I`.
    #[must_use]
    pub const fn injected(&self) -> f64 {
        self.injected
    }

    /// `O`.
    #[must_use]
    pub const fn removed(&self) -> f64 {
        self.removed
    }

    /// `D`, including every transfer loss.
    #[must_use]
    pub const fn dissipated(&self) -> f64 {
        self.dissipated
    }

    /// Energy moved between holdings without loss: the sum of transfer
    /// credits.
    #[must_use]
    pub const fn moved(&self) -> f64 {
        self.moved
    }

    /// `I + O + D + moved`: every joule that changed hands once.
    #[must_use]
    pub const fn turnover(&self) -> f64 {
        self.turnover
    }

    /// `ε = (𝓔_close − 𝓔_open) − (I − O − D)`.
    #[must_use]
    pub const fn residual(&self) -> f64 {
        self.residual
    }

    /// The opening energy plus turnover that the allowance scales with.
    #[must_use]
    pub const fn scale(&self) -> f64 {
        self.scale
    }

    /// The largest `|ε|` the budget allowed this tick.
    #[must_use]
    pub const fn allowance(&self) -> f64 {
        self.allowance
    }
}

/// Everything an [`EnergyLedger`] carries between ticks: its holdings,
/// tick count and every running sum, bit for bit. Each tick's
/// [`LedgerReport`] and INV-03 check read all of it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LedgerState {
    /// The holdings the run started at.
    pub initial: Holdings,
    /// The last committed closing holdings.
    pub last: Holdings,
    /// The next tick the ledger accepts; `None` before the first.
    pub next_tick: Option<Tick>,
    /// Committed ticks.
    pub ticks: u64,
    /// Energy injected.
    pub injected: CompensatedSum,
    /// Energy removed.
    pub removed: CompensatedSum,
    /// Energy dissipated.
    pub dissipated: CompensatedSum,
    /// Energy turned over.
    pub turnover: CompensatedSum,
    /// The cumulative residual `Σ ε`.
    pub residual: CompensatedSum,
    /// The largest single-tick `|ε|`.
    pub largest_tick_residual: f64,
    /// The residual drift fit: count, mean x, mean y, co-moment and x
    /// moment.
    pub drift: [f64; 5],
}

/// The run ledger: every committed tick, in order, with no gaps between
/// them.
#[derive(Debug, Clone)]
pub struct EnergyLedger {
    budget: ResidualBudget,
    initial: Holdings,
    last: Holdings,
    next_tick: Option<Tick>,
    ticks: u64,
    injected: CompensatedSum,
    removed: CompensatedSum,
    dissipated: CompensatedSum,
    turnover: CompensatedSum,
    residual: CompensatedSum,
    largest_tick_residual: f64,
    drift: DriftFit,
}

impl EnergyLedger {
    /// A run that starts at the measured `initial` holdings and is checked
    /// against `budget`.
    #[must_use]
    pub fn new(initial: Holdings, budget: ResidualBudget) -> Self {
        Self {
            budget,
            initial,
            last: initial,
            next_tick: None,
            ticks: 0,
            injected: CompensatedSum::ZERO,
            removed: CompensatedSum::ZERO,
            dissipated: CompensatedSum::ZERO,
            turnover: CompensatedSum::ZERO,
            residual: CompensatedSum::ZERO,
            largest_tick_residual: 0.0,
            drift: DriftFit::default(),
        }
    }

    /// The run's budget.
    #[must_use]
    pub const fn budget(&self) -> ResidualBudget {
        self.budget
    }

    /// Everything the ledger carries between ticks, for checkpoints.
    #[must_use]
    pub const fn state(&self) -> LedgerState {
        LedgerState {
            initial: self.initial,
            last: self.last,
            next_tick: self.next_tick,
            ticks: self.ticks,
            injected: self.injected,
            removed: self.removed,
            dissipated: self.dissipated,
            turnover: self.turnover,
            residual: self.residual,
            largest_tick_residual: self.largest_tick_residual,
            drift: [
                self.drift.count,
                self.drift.mean_x,
                self.drift.mean_y,
                self.drift.co_moment,
                self.drift.x_moment,
            ],
        }
    }

    /// The ledger [`EnergyLedger::state`] gave `state`, checked against
    /// `budget`.
    ///
    /// # Errors
    /// [`CoreError::IncompatibleCheckpoint`] for a nonfinite value, a tick
    /// count that disagrees with the next tick, or a cumulative residual
    /// beyond the budget's allowance (INV-03).
    pub fn restore(state: LedgerState, budget: ResidualBudget) -> Result<Self, CoreError> {
        let sums = [
            state.injected,
            state.removed,
            state.dissipated,
            state.turnover,
            state.residual,
        ];
        let finite = sums
            .iter()
            .flat_map(CompensatedSum::parts)
            .chain(state.drift)
            .chain([state.largest_tick_residual])
            .all(f64::is_finite);
        if !finite {
            return Err(CoreError::IncompatibleCheckpoint {
                reason: "ledger state holds a nonfinite value".into(),
            });
        }
        if (state.ticks == 0) != state.next_tick.is_none() {
            return Err(CoreError::IncompatibleCheckpoint {
                reason: format!(
                    "ledger counts {} ticks but its next tick is {:?}",
                    state.ticks, state.next_tick
                ),
            });
        }
        let [count, mean_x, mean_y, co_moment, x_moment] = state.drift;
        let ledger = Self {
            budget,
            initial: state.initial,
            last: state.last,
            next_tick: state.next_tick,
            ticks: state.ticks,
            injected: state.injected,
            removed: state.removed,
            dissipated: state.dissipated,
            turnover: state.turnover,
            residual: state.residual,
            largest_tick_residual: state.largest_tick_residual,
            drift: DriftFit {
                count,
                mean_x,
                mean_y,
                co_moment,
                x_moment,
            },
        };
        let report = ledger.report();
        if report.residual.abs() > report.allowance {
            return Err(CoreError::IncompatibleCheckpoint {
                reason: format!(
                    "ledger residual {:e} exceeds the allowance {:e} (INV-03)",
                    report.residual, report.allowance
                ),
            });
        }
        Ok(ledger)
    }

    /// The holdings the next tick must open at.
    #[must_use]
    pub const fn holdings(&self) -> Holdings {
        self.last
    }

    /// Open `tick` at the measured `opening` holdings.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] when `tick` is not the next tick, or
    /// `opening` differs from the last committed closing holdings in any
    /// bit: energy changed between ticks without a recorded flow.
    pub fn open(&self, tick: Tick, opening: Holdings) -> Result<TickLedger, CoreError> {
        self.continues(tick, opening)?;
        Ok(TickLedger::open(tick, opening, self.budget))
    }

    /// Commit a closed tick.
    ///
    /// # Errors
    /// - [`CoreError::StateConflict`] as for [`EnergyLedger::open`], or when
    ///   the closure was checked against a different budget.
    /// - [`CoreError::NumericalInvalid`] when the cumulative residual
    ///   exceeds the cumulative allowance (INV-03). Nothing is committed.
    pub fn commit(&mut self, closure: &TickClosure) -> Result<LedgerReport, CoreError> {
        self.continues(closure.tick, closure.opening)?;
        if closure.budget != self.budget {
            return Err(CoreError::StateConflict {
                operation: "energy ledger commit",
                detail: format!(
                    "tick {} was checked against {:?}, not the run's {:?}",
                    closure.tick.get(),
                    closure.budget,
                    self.budget
                ),
            });
        }
        let mut next = self.clone();
        next.injected.add(closure.injected);
        next.removed.add(closure.removed);
        next.dissipated.add(closure.dissipated);
        next.turnover.add(closure.turnover);
        next.residual.add(closure.residual);
        next.largest_tick_residual = next.largest_tick_residual.max(closure.residual.abs());
        next.ticks = self.ticks.checked_add(1).ok_or(CoreError::Overflow {
            quantity: "ledger tick count",
        })?;
        // Ticks counted since the run began; exact below 2^53.
        next.drift.add(next.ticks as f64, next.residual.value());
        next.last = closure.closing;
        next.next_tick = Some(closure.tick.next()?);
        let report = next.report();
        if report.residual.abs() > report.allowance || !report.residual.is_finite() {
            return Err(CoreError::NumericalInvalid {
                quantity: "cumulative energy ledger residual (INV-03)",
                detail: format!(
                    "after tick {}: Σε = {:e} exceeds the allowance {:e} (initial energy plus turnover {:e}; budget {:?})",
                    closure.tick.get(),
                    report.residual,
                    report.allowance,
                    report.initial + report.turnover,
                    self.budget
                ),
            });
        }
        *self = next;
        Ok(report)
    }

    /// The cumulative report.
    #[must_use]
    pub fn report(&self) -> LedgerReport {
        let initial = self.initial.total();
        let current = self.last.total();
        let turnover = self.turnover.value();
        let residual = self.residual.value();
        let magnitude = residual.abs();
        let ratio = |denominator: f64| (denominator > 0.0).then(|| magnitude / denominator);
        LedgerReport {
            ticks: self.ticks,
            initial,
            current,
            injected: self.injected.value(),
            removed: self.removed.value(),
            dissipated: self.dissipated.value(),
            turnover,
            residual,
            normalized: ratio(initial + turnover),
            versus_initial: ratio(initial),
            versus_current: ratio(current),
            largest_tick_residual: self.largest_tick_residual,
            drift_per_tick: self.drift.slope(),
            allowance: self.budget.cumulative_allowance(initial + turnover),
        }
    }

    fn continues(&self, tick: Tick, opening: Holdings) -> Result<(), CoreError> {
        if let Some(expected) = self.next_tick
            && tick != expected
        {
            return Err(CoreError::StateConflict {
                operation: "energy ledger tick",
                detail: format!(
                    "tick {} offered where tick {} is next",
                    tick.get(),
                    expected.get()
                ),
            });
        }
        if !opening.same(&self.last) {
            return Err(CoreError::StateConflict {
                operation: "energy ledger tick",
                detail: format!(
                    "tick {} opens at {:?} but the ledger closed at {:?}; energy changed without a recorded flow",
                    tick.get(),
                    opening,
                    self.last
                ),
            });
        }
        Ok(())
    }
}

/// The run ledger's cumulative report (§21.4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LedgerReport {
    /// Committed ticks.
    pub ticks: u64,
    /// `𝓔` at the start of the run.
    pub initial: f64,
    /// `𝓔` after the last committed tick.
    pub current: f64,
    /// Cumulative `I`.
    pub injected: f64,
    /// Cumulative `O`.
    pub removed: f64,
    /// Cumulative `D`.
    pub dissipated: f64,
    /// Cumulative turnover.
    pub turnover: f64,
    /// Cumulative `Σ ε`.
    pub residual: f64,
    /// `|Σ ε| / (𝓔_initial + turnover)`, the gated measure. `None` when
    /// the denominator is zero.
    pub normalized: Option<f64>,
    /// `|Σ ε| / 𝓔_initial`. A large recycled turnover cannot hide a
    /// residual from this one.
    pub versus_initial: Option<f64>,
    /// `|Σ ε| / 𝓔_current`.
    pub versus_current: Option<f64>,
    /// The largest `|ε|` of any single tick.
    pub largest_tick_residual: f64,
    /// Least-squares slope of `Σ ε` against committed ticks, in energy per
    /// tick. `None` before two ticks.
    pub drift_per_tick: Option<f64>,
    /// The largest `|Σ ε|` the budget allows now.
    pub allowance: f64,
}

/// Online least-squares slope (Welford's co-moment form).
#[derive(Debug, Clone, Copy, Default)]
struct DriftFit {
    count: f64,
    mean_x: f64,
    mean_y: f64,
    co_moment: f64,
    x_moment: f64,
}

impl DriftFit {
    fn add(&mut self, x: f64, y: f64) {
        self.count += 1.0;
        let dx = x - self.mean_x;
        self.mean_x += dx / self.count;
        self.mean_y += (y - self.mean_y) / self.count;
        self.co_moment += dx * (y - self.mean_y);
        self.x_moment += dx * (x - self.mean_x);
    }

    fn slope(&self) -> Option<f64> {
        (self.x_moment > 0.0).then(|| self.co_moment / self.x_moment)
    }
}

/// What a payment asked for and what it got (§8.2: requested and paid
/// costs are distinct telemetry values).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Payment {
    /// The amount asked for.
    pub requested: f64,
    /// The amount actually paid.
    pub paid: f64,
}

impl Payment {
    /// `requested − paid`: the unpaid part, which is never dissipated.
    #[must_use]
    pub fn shortfall(&self) -> f64 {
        self.requested - self.paid
    }

    /// Whether the whole request was paid.
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.paid == self.requested
    }
}

/// One organism's spendable energy within a tick (INV-05).
///
/// The remaining balance is rounded down after every credit and payment, so
/// in exact arithmetic `Σ paid ≤ available + Σ credited`: no energy is
/// spent twice. Rounding down can lose at most one ulp of the balance per
/// operation, which the ledger sees as a negative residual. Once a payment
/// takes the balance to zero the budget is exhausted for good: later
/// payments get nothing and credits are refused, because an exhausted
/// organism dies and cannot absorb energy (§8.2).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnergyBudget {
    remaining: f64,
    exhausted: bool,
}

impl EnergyBudget {
    /// A budget holding `available` energy. A budget of zero starts
    /// exhausted.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] unless `available` is finite and
    /// nonnegative.
    pub fn new(available: f64) -> Result<Self, CoreError> {
        let remaining = energy("energy budget", available)?;
        Ok(Self {
            remaining,
            exhausted: remaining == 0.0,
        })
    }

    /// The balance.
    #[must_use]
    pub const fn remaining(&self) -> f64 {
        self.remaining
    }

    /// Whether a payment has taken the balance to zero.
    #[must_use]
    pub const fn is_exhausted(&self) -> bool {
        self.exhausted
    }

    /// Add `amount`, such as an uptake gain.
    ///
    /// # Errors
    /// - [`CoreError::NumericalInvalid`] unless `amount` is finite and
    ///   nonnegative.
    /// - [`CoreError::StateConflict`] when the budget is exhausted.
    pub fn credit(&mut self, amount: f64) -> Result<(), CoreError> {
        let amount = energy("energy credit", amount)?;
        if self.exhausted {
            return Err(CoreError::StateConflict {
                operation: "energy credit",
                detail: "the budget is exhausted; a dead organism cannot absorb energy".into(),
            });
        }
        let remaining = add_down(self.remaining, amount);
        if !remaining.is_finite() {
            return Err(CoreError::NumericalInvalid {
                quantity: "energy credit",
                detail: format!("{} + {amount} overflows", self.remaining),
            });
        }
        self.remaining = remaining;
        Ok(())
    }

    /// Pay as much of `requested` as the balance allows.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] unless `requested` is finite and
    /// nonnegative.
    pub fn pay(&mut self, requested: f64) -> Result<Payment, CoreError> {
        let requested = energy("energy payment", requested)?;
        let paid = requested.min(self.remaining);
        self.remaining = if paid == self.remaining {
            0.0
        } else {
            sub_down(self.remaining, paid)
        };
        if self.remaining == 0.0 && paid > 0.0 {
            self.exhausted = true;
        }
        Ok(Payment { requested, paid })
    }

    /// Pay all of `amount` if the balance covers it, otherwise nothing.
    /// Returns whether it was paid.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] unless `amount` is finite and
    /// nonnegative.
    pub fn spend(&mut self, amount: f64) -> Result<bool, CoreError> {
        let amount = energy("energy payment", amount)?;
        if amount > self.remaining {
            return Ok(false);
        }
        self.pay(amount)?;
        Ok(true)
    }

    /// Keep exactly `kept` and pay out the rest, as a parent does when it
    /// endows a child (§7.4). The balance becomes `kept` bit for bit; the
    /// returned payment is `remaining − kept` rounded to nearest, which is
    /// telemetry: the ledger is told what moved by the transaction that
    /// computed `kept`. Keeping zero exhausts the budget.
    ///
    /// # Errors
    /// - [`CoreError::NumericalInvalid`] unless `kept` is finite,
    ///   nonnegative and at most the balance.
    /// - [`CoreError::StateConflict`] when the budget is exhausted.
    pub fn retain(&mut self, kept: f64) -> Result<Payment, CoreError> {
        let kept = energy("retained energy", kept)?;
        if self.exhausted {
            return Err(CoreError::StateConflict {
                operation: "retain energy",
                detail: "the budget is exhausted; a dead organism cannot reproduce".into(),
            });
        }
        if kept > self.remaining {
            return Err(CoreError::NumericalInvalid {
                quantity: "retained energy",
                detail: format!("{kept} exceeds the balance {}", self.remaining),
            });
        }
        let paid = self.remaining - kept;
        self.remaining = kept;
        if kept == 0.0 && paid > 0.0 {
            self.exhausted = true;
        }
        Ok(Payment {
            requested: paid,
            paid,
        })
    }
}

/// `value` as a finite nonnegative energy, with `-0.0` folded to `0.0`.
fn energy(quantity: &'static str, value: f64) -> Result<f64, CoreError> {
    if value.is_finite() && value >= 0.0 {
        Ok(value + 0.0)
    } else {
        Err(CoreError::NumericalInvalid {
            quantity,
            detail: format!("{value} is not a finite nonnegative energy"),
        })
    }
}
