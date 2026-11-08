//! FL-005 ledger evidence (plan §4.4, §8.2, §21.2 INV-03 and INV-05,
//! §21.4).
//!
//! The rounding-direction and no-double-spend properties are checked
//! against exact integer arithmetic: each test value is a dyadic rational
//! whose bits fit an `i128` scaled by `2^100`.

use flife_core::ledger::{CompensatedSum, FLOW_KINDS, TRANSFER_KINDS, add_down, sub_down};
use flife_core::{
    CoreError, EnergyBudget, EnergyLedger, Flow, FlowClass, Holdings, ResidualBudget, Tick,
    TickLedger, Transfer,
};

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn holdings(field: f64, organisms: f64) -> Holdings {
    Holdings::new(field, organisms).expect("holdings")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn budget(absolute: f64, per_tick: f64, cumulative: f64) -> ResidualBudget {
    ResidualBudget::new(absolute, per_tick, cumulative).expect("budget")
}

/// `splitmix64`, a fixed generator for reproducible test values.
fn splitmix(seed: u64) -> impl FnMut() -> u64 {
    let mut state = seed;
    move || {
        state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

/// A random float `m × 2^(e − 52)` with a full 53-bit significand and
/// `e ∈ [low, high]`.
fn dyadic(next: &mut impl FnMut() -> u64, low: i32, high: i32) -> f64 {
    let significand = (next() >> 11) | (1 << 52);
    let span = u64::try_from(high - low + 1).unwrap_or(1);
    let exponent = low + i32::try_from(next() % span).unwrap_or(0);
    // `significand < 2^53` converts exactly; scaling by a power of two is exact.
    (significand as f64) * 2.0_f64.powi(exponent - 52)
}

const SCALE_BITS: i32 = 100;

/// `value × 2^100` as an exact integer.
fn exact(value: f64) -> i128 {
    let scaled = value * 2.0_f64.powi(SCALE_BITS);
    assert_eq!(
        scaled.fract(),
        0.0,
        "{value} has bits below 2^-{SCALE_BITS}"
    );
    assert!(scaled.abs() < 2.0_f64.powi(126), "{value} is too large");
    // Integral and below 2^126, so the conversion is exact.
    scaled as i128
}

#[test]
fn flows_have_one_class_and_transfers_charge_named_losses() {
    assert_eq!(FLOW_KINDS, 15);
    assert_eq!(TRANSFER_KINDS, 4);
    let mut names: Vec<&str> = Flow::ALL.iter().map(|flow| flow.name()).collect();
    names.extend(Transfer::ALL.iter().map(|transfer| transfer.name()));
    let count = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), count, "names are unique");
    let injections: Vec<Flow> = Flow::ALL
        .into_iter()
        .filter(|flow| flow.class() == FlowClass::Injection)
        .collect();
    assert_eq!(
        injections,
        [
            Flow::SourcePower,
            Flow::InitialEndowment,
            Flow::CommandInjection
        ]
    );
    assert_eq!(Flow::CommandRemoval.class(), FlowClass::Removal);
    for transfer in Transfer::ALL {
        assert_eq!(transfer.loss().class(), FlowClass::Dissipation);
    }
    assert_eq!(Transfer::Uptake.loss(), Flow::UptakeInefficiency);
    assert_eq!(Transfer::Emission.loss(), Flow::EmissionLoss);
    assert_eq!(Transfer::ChildEndowment.loss(), Flow::ReproductionLoss);
    assert_eq!(Transfer::Recycling.loss(), Flow::UnrecycledDeath);
}

/// Appendix C.1's accounting: grants 4 and 6 with efficiencies 0.5 and
/// 0.25 dissipate exactly 6.5, and the ledger closes with `ε = 0`.
#[test]
fn c1_accounting_closes_exactly() {
    let mut tick = TickLedger::open(Tick::new(0), holdings(10.0, 2.0), ResidualBudget::EXACT);
    assert_eq!(
        tick.transfer(Transfer::Uptake, 4.0, 2.0).expect("uptake"),
        2.0
    );
    assert_eq!(
        tick.transfer(Transfer::Uptake, 6.0, 1.5).expect("uptake"),
        4.5
    );
    let closure = tick.close(holdings(0.0, 5.5)).expect("closes");
    assert_eq!(closure.residual(), 0.0);
    assert_eq!(closure.dissipated(), 6.5);
    assert_eq!(closure.flow(Flow::UptakeInefficiency), 6.5);
    assert_eq!(closure.debited(Transfer::Uptake), 10.0);
    assert_eq!(closure.credited(Transfer::Uptake), 3.5);
    assert_eq!(closure.moved(), 3.5);
    assert_eq!(closure.turnover(), 10.0);
    assert_eq!(closure.opening().total() - closure.closing().total(), 6.5);
    assert_eq!(closure.budget(), ResidualBudget::EXACT);
    assert_eq!(closure.allowance(), 0.0);
}

#[test]
fn transfers_cannot_create_energy() {
    let mut tick = TickLedger::open(Tick::new(3), holdings(1.0, 1.0), ResidualBudget::EXACT);
    for (debited, credited) in [
        (1.0, 1.0 + f64::EPSILON),
        (-1.0, 0.0),
        (1.0, -0.5),
        (f64::NAN, 0.0),
        (f64::INFINITY, 1.0),
    ] {
        assert!(
            matches!(
                tick.transfer(Transfer::Emission, debited, credited),
                Err(CoreError::NumericalInvalid { .. })
            ),
            "({debited}, {credited}) is refused"
        );
    }
    for amount in [-1.0e-300, f64::NAN, f64::NEG_INFINITY] {
        assert!(tick.record(Flow::Maintenance, amount).is_err());
    }
    assert_eq!(
        tick.transfer(Transfer::Recycling, 0.25, 0.25)
            .expect("lossless"),
        0.0
    );
    // Nothing refused was recorded: the unchanged world still closes
    // exactly, with only the lossless transfer in the turnover.
    let closure = tick.close(holdings(1.0, 1.0)).expect("closes");
    assert_eq!(closure.residual(), 0.0);
    assert_eq!(closure.dissipated(), 0.0);
    assert_eq!(closure.turnover(), 0.25);
    assert!(Holdings::new(-1.0, 0.0).is_err());
    assert!(Holdings::new(0.0, f64::NAN).is_err());
    assert_eq!(holdings(-0.0, 0.0).field().to_bits(), 0.0_f64.to_bits());
}

#[test]
fn residuals_outside_the_budget_are_errors() {
    let unrecorded = TickLedger::open(Tick::new(7), holdings(10.0, 0.0), ResidualBudget::EXACT);
    let error = unrecorded
        .close(holdings(10.0 + 1.0e-9, 0.0))
        .expect_err("energy appeared");
    assert!(matches!(
        error,
        CoreError::NumericalInvalid {
            quantity: "energy ledger residual (INV-03)",
            ..
        }
    ));
    assert!(error.to_string().contains("tick 7"));

    // A declared budget admits the same residual and records itself.
    let loose = budget(0.0, 1.0e-9, 1.0e-9);
    let mut tick = TickLedger::open(Tick::new(7), holdings(10.0, 0.0), loose);
    tick.record(Flow::Decay, 1.0).expect("decay");
    let closure = tick
        .close(holdings(9.0 + 1.0e-9, 0.0))
        .expect("inside the budget");
    assert!(closure.residual() > 0.0);
    assert_eq!(closure.scale(), 11.0);
    assert_eq!(closure.allowance(), 1.0e-9 * 11.0);
    assert_eq!(closure.budget(), loose);

    // An absolute floor admits a residual on an empty world.
    let floor = budget(1.0e-30, 0.0, 0.0);
    let empty = TickLedger::open(Tick::new(0), holdings(0.0, 0.0), floor);
    assert!(empty.close(holdings(1.0e-31, 0.0)).is_ok());
    for (absolute, relative) in [(-1.0, 0.0), (0.0, f64::NAN), (f64::INFINITY, 0.0)] {
        assert!(ResidualBudget::new(absolute, relative, relative).is_err());
    }
}

#[test]
fn ticks_must_open_where_the_last_one_closed() {
    let mut ledger = EnergyLedger::new(holdings(4.0, 2.0), ResidualBudget::EXACT);
    let mut first = ledger
        .open(Tick::new(10), holdings(4.0, 2.0))
        .expect("opens");
    first.transfer(Transfer::Uptake, 1.0, 0.5).expect("uptake");
    let closure = first.close(holdings(3.0, 2.5)).expect("closes");
    let report = ledger.commit(&closure).expect("commits");
    assert_eq!(report.ticks, 1);
    assert_eq!(report.dissipated, 0.5);
    assert_eq!(ledger.holdings(), holdings(3.0, 2.5));

    // Energy that changed between ticks, even by one ulp, is refused.
    let drifted = holdings(3.0, 2.5_f64.next_up());
    assert!(matches!(
        ledger.open(Tick::new(11), drifted),
        Err(CoreError::StateConflict { .. })
    ));
    // So is a skipped or repeated tick, and a closure replayed.
    for tick in [10, 12] {
        assert!(matches!(
            ledger.open(Tick::new(tick), holdings(3.0, 2.5)),
            Err(CoreError::StateConflict { .. })
        ));
    }
    assert!(matches!(
        ledger.commit(&closure),
        Err(CoreError::StateConflict { .. })
    ));
    // And a closure checked against another budget.
    let other = TickLedger::open(Tick::new(11), holdings(3.0, 2.5), budget(1.0, 0.0, 0.0))
        .close(holdings(3.0, 2.5))
        .expect("closes");
    assert!(matches!(
        ledger.commit(&other),
        Err(CoreError::StateConflict { .. })
    ));
    // None of the refusals changed the ledger.
    assert_eq!(ledger.report(), report);
    let next = ledger
        .open(Tick::new(11), holdings(3.0, 2.5))
        .expect("opens")
        .close(holdings(3.0, 2.5))
        .expect("closes");
    assert_eq!(ledger.commit(&next).expect("commits").ticks, 2);
}

#[test]
fn cumulative_residuals_are_gated_and_reported() {
    // Each tick gains a residual of exactly 2^-30 out of 1024: inside the
    // per-tick allowance, but the cumulative bound is tighter.
    let step = 2.0_f64.powi(-30);
    let budget = budget(0.0, 1.0e-9, 4.0e-11);
    let mut ledger = EnergyLedger::new(holdings(1024.0, 0.0), budget);
    let mut field = 1024.0;
    let mut committed = Vec::new();
    for tick in 0..64_u64 {
        let opening = holdings(field, 0.0);
        let mut ledger_tick = ledger.open(Tick::new(tick), opening).expect("opens");
        ledger_tick.record(Flow::SourcePower, 1.0).expect("source");
        ledger_tick.record(Flow::Decay, 1.0).expect("decay");
        let closing = holdings(field + step, 0.0);
        let closure = ledger_tick.close(closing).expect("per-tick check");
        assert_eq!(closure.residual(), step);
        match ledger.commit(&closure) {
            Ok(report) => {
                field += step;
                committed.push(report);
            }
            Err(error) => {
                assert!(matches!(
                    error,
                    CoreError::NumericalInvalid {
                        quantity: "cumulative energy ledger residual (INV-03)",
                        ..
                    }
                ));
                break;
            }
        }
    }
    let last = *committed.last().expect("some ticks commit");
    let ticks = last.ticks;
    assert!(ticks > 2 && ticks < 64, "the cumulative gate trips");
    assert_eq!(ledger.report(), last, "the refused tick is not committed");
    // Exact arithmetic: every term below is a small dyadic rational.
    let n = f64::from(u32::try_from(ticks).expect("small"));
    assert_eq!(last.residual, n * step);
    assert_eq!(last.injected, n);
    assert_eq!(last.dissipated, n);
    assert_eq!(last.turnover, 2.0 * n);
    assert_eq!(last.normalized, Some(n * step / (1024.0 + 2.0 * n)));
    assert_eq!(last.versus_initial, Some(n * step / 1024.0));
    assert_eq!(last.versus_current, Some(n * step / (1024.0 + n * step)));
    assert_eq!(last.largest_tick_residual, step);
    let drift = last.drift_per_tick.expect("two or more ticks");
    assert!((drift - step).abs() <= 1.0e-12 * step, "slope {drift}");
    assert!(committed[0].drift_per_tick.is_none());
}

#[test]
fn compensated_sums_keep_what_naive_sums_lose() {
    let values = [1.0e16, 1.0, -1.0e16, 3.0];
    let naive: f64 = values.iter().sum();
    let mut compensated = CompensatedSum::ZERO;
    for value in values {
        compensated.add(value);
    }
    assert_eq!(naive, 3.0);
    assert_eq!(compensated.value(), 4.0);
}

#[test]
fn rounding_down_never_exceeds_the_exact_result() {
    let mut next = splitmix(17);
    let mut stepped = 0;
    for _ in 0..20_000 {
        let a = dyadic(&mut next, -30, 10);
        let b = dyadic(&mut next, -30, 10);
        let sum = add_down(a, b);
        let truth = exact(a) + exact(b);
        assert!(exact(sum) <= truth, "{a} + {b} rounded up to {sum}");
        assert!(exact(sum.next_up()) > truth, "{a} + {b} is within one ulp");
        stepped += usize::from(sum != a + b);
        let (high, low) = if a >= b { (a, b) } else { (b, a) };
        let difference = sub_down(high, low);
        let truth = exact(high) - exact(low);
        assert!(difference >= 0.0);
        if difference > 0.0 {
            assert!(exact(difference) <= truth, "{high} − {low} rounded up");
        } else {
            // Test values are multiples of 2^-82, so a zero result means
            // the operands were equal.
            assert_eq!(truth, 0, "{high} − {low} rounded down to zero");
        }
    }
    assert!(
        stepped > 1_000,
        "rounding down actually happens ({stepped})"
    );
    assert_eq!(add_down(0.1, 0.2), (0.1_f64 + 0.2).next_down());
    assert_eq!(add_down(0.5, 0.25), 0.75);
}

#[test]
fn budgets_never_pay_more_than_they_hold() {
    let mut next = splitmix(29);
    for trial in 0..200 {
        let available = dyadic(&mut next, 0, 6);
        let mut budget = EnergyBudget::new(available).expect("budget");
        let mut held = exact(available);
        let mut paid_total = 0_i128;
        for step in 0..200 {
            if next().is_multiple_of(3) {
                let amount = dyadic(&mut next, -20, 2);
                budget.credit(amount).expect("credit");
                held += exact(amount);
            } else {
                // Mostly affordable requests, sometimes more than the balance.
                let requested = if step % 50 == 49 {
                    budget.remaining() * 2.0 + 1.0
                } else {
                    budget.remaining() * f64::from(u32::try_from(next() % 1000).unwrap_or(0))
                        / 2048.0
                };
                let payment = budget.pay(requested).expect("pay");
                assert_eq!(payment.requested, requested);
                assert!(payment.paid <= payment.requested);
                paid_total += exact(payment.paid);
                if budget.is_exhausted() {
                    assert!(payment.paid < payment.requested);
                    break;
                }
                assert!(payment.is_full(), "affordable requests are paid in full");
            }
            let remaining = budget.remaining();
            assert!(remaining >= 0.0);
            // INV-05: in exact arithmetic, paid + remaining never exceeds
            // what was available plus what was credited.
            assert!(
                paid_total + exact(remaining) <= held,
                "trial {trial} step {step} spent energy twice"
            );
        }
    }
}

#[test]
fn exhausted_budgets_pay_nothing_and_absorb_nothing() {
    let mut budget = EnergyBudget::new(1.0).expect("budget");
    budget.credit(0.5).expect("gain before maintenance");
    let first = budget.pay(0.25).expect("pay");
    assert!(first.is_full());
    assert_eq!(budget.remaining(), 1.25);
    assert!(
        !budget.spend(2.0).expect("spend"),
        "unaffordable is not paid"
    );
    assert_eq!(
        budget.remaining(),
        1.25,
        "an unaffordable spend pays nothing"
    );
    let partial = budget.pay(2.0).expect("pay");
    assert_eq!(partial.paid, 1.25);
    assert_eq!(partial.shortfall(), 0.75);
    assert!(budget.is_exhausted());
    assert_eq!(budget.pay(1.0).expect("pay").paid, 0.0);
    assert!(matches!(
        budget.credit(1.0),
        Err(CoreError::StateConflict { .. })
    ));
    assert!(EnergyBudget::new(0.0).expect("empty").is_exhausted());
    for value in [-1.0, f64::NAN, f64::INFINITY] {
        assert!(EnergyBudget::new(value).is_err());
        assert!(EnergyBudget::new(1.0).expect("budget").pay(value).is_err());
    }
    let mut exact_spend = EnergyBudget::new(0.75).expect("budget");
    assert!(exact_spend.spend(0.75).expect("spend"));
    assert!(exact_spend.is_exhausted());
    assert_eq!(exact_spend.remaining(), 0.0);
}

#[test]
fn budgets_retain_exactly_the_share_a_parent_keeps() {
    // FL-011: a parent keeps its split share bit for bit, whatever the
    // rounding of what it paid out.
    let mut next = splitmix(0x0f11_0011);
    for _ in 0..2_000 {
        let available = dyadic(&mut next, -2, 6);
        let kept = dyadic(&mut next, -8, 6).min(available);
        let mut budget = EnergyBudget::new(available).expect("budget");
        let payment = budget.retain(kept).expect("retain");
        assert_eq!(budget.remaining().to_bits(), kept.to_bits());
        assert!(!budget.is_exhausted());
        assert!(payment.is_full());
        assert_eq!(payment.paid, available - kept);
    }
    // C.5: a parent with 10 keeps 6.
    let mut parent = EnergyBudget::new(10.0).expect("budget");
    assert_eq!(parent.retain(6.0).expect("retain").paid, 4.0);
    assert_eq!(parent.remaining(), 6.0);
    // Keeping everything pays nothing; keeping nothing exhausts.
    assert_eq!(parent.retain(6.0).expect("retain").paid, 0.0);
    assert_eq!(parent.retain(0.0).expect("retain").paid, 6.0);
    assert!(parent.is_exhausted());
    assert!(matches!(
        parent.retain(0.0),
        Err(CoreError::StateConflict { .. })
    ));
    let mut budget = EnergyBudget::new(1.0).expect("budget");
    for kept in [1.0_f64.next_up(), -0.5, f64::NAN, f64::INFINITY] {
        assert!(matches!(
            budget.retain(kept),
            Err(CoreError::NumericalInvalid { .. })
        ));
        assert_eq!(budget.remaining(), 1.0, "a refused retain changes nothing");
    }
}
