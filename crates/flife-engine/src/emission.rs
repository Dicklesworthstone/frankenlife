//! Paid emission (plan §5.5, §8.1 phase 7, §8.2, A.1, Appendix C.4).
//!
//! The action decoder proposes an emission (`flife_neural::Emission`) only
//! when the gate is open and the direction valid. Otherwise, under
//! `no-emission-no-charge`, the organism emits nothing and pays nothing. A
//! dead organism, one whose budget is exhausted, also emits nothing.
//!
//! An open emission spends `w = f E`. Here `E` is what the organism holds
//! at the emission stage, which comes after motion (§8.1), and `f` is the
//! decoded fraction `1 − e^(−r_max σ(o) Δt)`. A held controller output is
//! decoded against the current tick's budget, never a cached one. The
//! organism pays `w` from its budget (INV-05) and offers `ρ w` to the field
//! at its post-motion position (`post-motion`). The offer is staged in a
//! `flife_field::Deposits` and reaches the next field in phase 9
//! (`next-tick`), so no organism senses or absorbs it within the tick it
//! was made, the emitter included.
//!
//! The ledger records the transfer when the deposits are applied. It debits
//! `Σ w`, credits what the cells received, and charges the rest to
//! `emission-loss`. Because `ρ ≤ 1` and the field never receives more than
//! was offered, no emission loop can amplify spendable energy.
//!
//! An emission that spends the last unit of energy exhausts the emitter,
//! which then dies. Its deposit stands, because already-paid deposits keep
//! their stated effect (§8.2).

use flife_core::ids::OrganismId;
use flife_core::ledger::{EnergyBudget, Payment};
use flife_core::{CoreError, ValidatedConfig};
use flife_field::{Deposits, FieldGrid};
use flife_neural::Emission;

/// The emission position this module implements.
pub const POST_MOTION: &str = "post-motion";

/// The deposit visibility this module implements: a deposit is first
/// sensed on the next tick.
pub const NEXT_TICK_VISIBILITY: &str = "next-tick";

/// The `[emission]` transfer rule.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EmissionPolicy {
    efficiency: f64,
}

impl EmissionPolicy {
    /// A transfer efficiency `ρ ∈ [0, 1]`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] outside `[0, 1]`.
    pub fn new(efficiency: f64) -> Result<Self, CoreError> {
        if !(0.0..=1.0).contains(&efficiency) {
            return Err(CoreError::NumericalInvalid {
                quantity: "emission.transfer_efficiency",
                detail: format!("{efficiency} is outside [0, 1]"),
            });
        }
        Ok(Self {
            efficiency: efficiency + 0.0,
        })
    }

    /// The policy of a validated configuration.
    ///
    /// # Errors
    /// [`CoreError::UnknownAlgorithm`] for another position or visibility;
    /// otherwise as [`EmissionPolicy::new`].
    pub fn from_config(config: &ValidatedConfig) -> Result<Self, CoreError> {
        let emission = &config.config().emission;
        for (role, name, implemented) in [
            ("emission.position", &emission.position, POST_MOTION),
            (
                "emission.visibility",
                &emission.visibility,
                NEXT_TICK_VISIBILITY,
            ),
        ] {
            if name != implemented {
                return Err(CoreError::UnknownAlgorithm {
                    role,
                    name: name.clone(),
                });
            }
        }
        Self::new(emission.transfer_efficiency)
    }

    /// `ρ`.
    #[must_use]
    pub const fn efficiency(&self) -> f64 {
        self.efficiency
    }

    /// Settle `organism`'s `emission` at the emission stage: pay `f E` from
    /// `budget` and stage `ρ f E` at `position`, its post-motion position,
    /// in `deposits`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a spending fraction outside
    /// `[0, 1]` or an invalid direction or position. A refused emission
    /// changes neither the budget nor the deposits.
    pub fn emit(
        &self,
        grid: &FieldGrid,
        organism: OrganismId,
        position: [f64; 2],
        emission: Option<&Emission>,
        budget: &mut EnergyBudget,
        deposits: &mut Deposits,
    ) -> Result<EmissionSettlement, CoreError> {
        let available = budget.remaining();
        let nothing = EmissionSettlement {
            available,
            cost: Payment {
                requested: 0.0,
                paid: 0.0,
            },
            offered: 0.0,
        };
        let Some(emission) = emission else {
            return Ok(nothing);
        };
        let fraction = emission.spend_fraction;
        if !(0.0..=1.0).contains(&fraction) {
            return Err(CoreError::NumericalInvalid {
                quantity: "emission spending fraction",
                detail: format!("{fraction} is outside [0, 1]"),
            });
        }
        if budget.is_exhausted() {
            return Ok(nothing);
        }
        // `f ≤ 1`, so the request never exceeds the balance: emission is
        // always affordable and pays in full.
        let requested = (fraction * available).min(available);
        if requested == 0.0 {
            return Ok(nothing);
        }
        let offered = self.efficiency * requested;
        deposits.push(
            grid,
            organism,
            position,
            requested,
            offered,
            &emission.direction,
        )?;
        let cost = budget.pay(requested)?;
        Ok(EmissionSettlement {
            available,
            cost,
            offered,
        })
    }
}

/// What one organism's emission stage paid and offered.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EmissionSettlement {
    /// The budget at the emission stage, after motion.
    pub available: f64,
    /// `w = f E`, requested and paid; emission always pays in full.
    pub cost: Payment,
    /// `ρ w`, offered to the field.
    pub offered: f64,
}

impl EmissionSettlement {
    /// Whether anything was emitted.
    #[must_use]
    pub fn emitted(&self) -> bool {
        self.cost.paid > 0.0
    }
}
