//! Phase 2 of the tick (plan §8.1): transport the committed field, decay
//! it, then apply this tick's external sources. The result is the next
//! field's observation state. The step writes only the candidate buffer and
//! never commits. The engine validates the candidate (phase 10) and then
//! commits or discards it.

use flife_core::{CoreError, TimeStep};

use crate::sources::{DepositReport, Source, SourceKernel, SourceSupport, deposit};
use crate::state::FieldPair;
use crate::transport::{TransportPlan, TransportReport, TransportScratch, sum, transport};

/// The field's ledger terms for one tick (§4.4), in F64.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldStepReport {
    /// Transport and decay.
    pub transport: TransportReport,
    /// One record per source, in the order given.
    pub deposits: Vec<DepositReport>,
    /// `Σ_j P_j Δt`: declared external injection `I`.
    pub injected_declared: f64,
    /// Energy the F32 field actually gained from sources.
    pub injected_applied: f64,
    /// `Σ e` of the candidate field.
    pub energy_after: f64,
    /// `ε = (after − before) − (I − D_decay)`: the numerical residual of the
    /// whole step. It is zero in exact arithmetic.
    pub residual: f64,
}

/// Advance the field part of one tick from `pair.current()` into the
/// candidate `pair`'s next buffer.
///
/// # Errors
/// As [`transport`] and [`deposit`]. On error the candidate is incomplete
/// and must be discarded; the committed field is untouched.
pub fn advance_field(
    pair: &mut FieldPair,
    plan: &TransportPlan,
    kernel: &SourceKernel,
    sources: &[Source],
    step: TimeStep,
    scratch: &mut TransportScratch,
    support: &mut SourceSupport,
) -> Result<FieldStepReport, CoreError> {
    let (current, next) = pair.stage();
    let transported = transport(plan, current, next, scratch)?;
    let mut deposits = Vec::with_capacity(sources.len());
    let (mut injected_declared, mut injected_applied) = (0.0, 0.0);
    for source in sources {
        let report = deposit(next, kernel, source, step, support)?;
        injected_declared += report.declared;
        injected_applied += report.applied;
        deposits.push(report);
    }
    let energy_after = sum(next.energy());
    let residual = (energy_after - transported.energy_before)
        - (injected_declared - transported.decay_removed);
    Ok(FieldStepReport {
        transport: transported,
        deposits,
        injected_declared,
        injected_applied,
        energy_after,
        residual,
    })
}
