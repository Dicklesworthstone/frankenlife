//! FL-032's engine port: the real small-world preset with phase 6's
//! chemistry, maintenance, motion and emission and phase 7's reproduction
//! proposal on Metal is the reference world, digest for digest and the
//! watched organism's transactions record for record, at every tick, alone
//! and together with the Metal field, sensors, uptake and controllers.
//! Changes the host's replay cannot see (one organism moved by one ulp, two
//! packets of one deposit swapped, a viable attempt reported as none)
//! change the world (the planted negatives). A backend that fails, or gives
//! a settlement the replay refuses (one per host check), hands that tick to
//! the reference and is dropped, leaving the run the reference's: no
//! organism keeps a stage the refused tick settled.

use flife_backend_metal::{
    ChemistryConstants, ChemistryOrganism, ChemistryOutcome, ChemistryPacket, EmissionOrganism,
    EmissionOutcome, ExhaustedAt, MetalBackend, MetalField, MetalSensors, MetalSettle, MetalUptake,
    MotionConstants, MotionOrganism, MotionOutcome, ProposalConstants, ProposalOrganism,
    ProposalOutcome,
};
use flife_core::{CoreError, ValidatedConfig};
use flife_engine::{Arrangement, BudgetStage, DeathCause, NumericalProfile, Stepped, World};
use flife_field::{FieldGrid, FieldState, SettleBackend};
use std::num::NonZeroUsize;

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

fn found() -> World {
    found_from(PRESET)
}

/// The preset with another motion cost and maximum emission rate.
fn variant(motion: &str, rate: &str) -> String {
    const MOTION: &str = "motion_energy_per_speed_squared_second = 0.002";
    const RATE: &str = "maximum_fractional_rate_per_second = 0.25";
    assert_eq!(PRESET.matches(MOTION).count(), 1);
    assert_eq!(PRESET.matches(RATE).count(), 1);
    PRESET
        .replace(
            MOTION,
            &format!("motion_energy_per_speed_squared_second = {motion}"),
        )
        .replace(
            RATE,
            &format!("maximum_fractional_rate_per_second = {rate}"),
        )
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure is a test failure"
)]
fn found_from(toml: &str) -> World {
    let config = ValidatedConfig::from_toml(toml).expect("the configuration validates");
    let mut world = World::found(
        &config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("the world founds");
    let workers = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
    world
        .set_cpu_workers(NonZeroUsize::new(workers.min(16)).expect("nonzero"))
        .expect("workers");
    world
}

/// `None` off macOS, where `open` must refuse with its typed error.
#[expect(clippy::panic, reason = "test helper")]
fn settle() -> Option<MetalSettle> {
    match MetalSettle::open() {
        Ok(settle) => Some(settle),
        Err(CoreError::MissingCapability { .. }) if !cfg!(target_os = "macos") => None,
        Err(error) => panic!("this host should have Metal: {error}"),
    }
}

/// What a race saw: the first tick whose digests or watched transactions
/// differ, the emitters and births, and the deaths by stage (chemistry,
/// maintenance, motion, emission).
#[derive(Debug, Default)]
struct Raced {
    difference: Option<u64>,
    emitted: usize,
    births: usize,
    deaths: [usize; 4],
}

/// Step both worlds, each watching the same organism.
#[expect(clippy::expect_used, clippy::panic, reason = "test helper")]
fn race(reference: &mut World, other: &mut World, ticks: u64) -> Raced {
    let watched = reference.population().ids().next();
    reference.watch(watched);
    other.watch(watched);
    let mut raced = Raced::default();
    for _ in 0..ticks {
        let expected = reference.step().expect("the reference steps");
        let got = other.step().expect("the other world steps");
        match (expected, got) {
            (Stepped::Published(_), Stepped::Published(report)) => {
                raced.emitted += report.emitted;
                raced.births += report.births.len();
                for death in &report.deaths {
                    let stage = match death.cause {
                        DeathCause::Exhausted(BudgetStage::Chemistry) => 0,
                        DeathCause::Exhausted(BudgetStage::Maintenance) => 1,
                        DeathCause::Exhausted(BudgetStage::Motion) => 2,
                        DeathCause::Exhausted(BudgetStage::Emission) => 3,
                        _ => continue,
                    };
                    raced.deaths[stage] += 1;
                }
            }
            (expected, got) => panic!("unexpected outcomes {expected:?} and {got:?}"),
        }
        let same = reference.digest() == other.digest()
            && reference
                .watched_transactions()
                .eq(other.watched_transactions());
        if raced.difference.is_none() && !same {
            raced.difference = Some(other.tick().get());
        }
    }
    raced
}

#[test]
fn the_preset_with_phase_6_on_metal_is_the_reference_world_at_every_tick() {
    let Some(device) = settle() else { return };
    let description = device.describe();
    let owner = device.owner().clone();
    let mut reference = found();
    let mut metal = found();
    let before = metal.allocated().scratch;
    metal.set_settle_backend(Some(Box::new(device)));
    let installed = metal.allocated().scratch;
    let raced = race(&mut reference, &mut metal, 300);
    let report = metal.settle_report().clone();
    println!("{raced:?}: {report:?}; scratch {before} -> {installed}");
    assert_eq!(raced.difference, None, "the worlds diverged");
    assert_eq!(report.backend.as_deref(), Some(description.as_str()));
    assert_eq!((report.ticks, report.reference_ticks), (300, 0));
    assert_eq!(report.failure, None);
    assert!(raced.emitted > 0 && raced.births > 0, "{raced:?}");
    assert!(raced.deaths[0] > 0 && raced.deaths[1] > 0, "{raced:?}");
    // FL-043: installing reserves the host's lists for the capacity, and
    // stepping never grows them.
    assert!(installed > before, "{before} {installed}");
    assert_eq!(metal.allocated().scratch, installed);
    assert_eq!(metal.footprint().scratch, installed);
    // FL-028: no lease, command or retiring buffer left over.
    let usage = owner.usage();
    assert_eq!(
        (
            usage.retiring,
            usage.read_leases,
            usage.write_leases,
            usage.running
        ),
        (0, [0; 3], 0, 0),
        "{usage:?}"
    );

    // Removing it releases the lists.
    metal.set_settle_backend(None);
    assert_eq!(metal.allocated().scratch, before);
    assert_eq!(metal.settle_report().backend, None);
}

/// Two variants of the preset. In one, motion is a thousand times as
/// costly and emission drains budgets four thousand times as fast:
/// organisms run low, and unaffordable motion scales down and exhausts. In
/// the other, emission is so fast that a spend fraction rounds to one
/// (`1 − e^(−r σ Δt)` with `r Δt = 10⁷/128`): emission spends everything.
/// Between them organisms die at every stage phase 6 settles.
#[test]
fn worlds_whose_organisms_die_at_every_stage_are_the_reference_worlds() {
    let mut deaths = [0; 4];
    for (motion, rate) in [("2.0", "1000.0"), ("0.002", "1.0e7")] {
        let Some(device) = settle() else { return };
        let changed = format!("motion cost {motion}, emission rate {rate}");
        let toml = variant(motion, rate);
        let mut reference = found_from(&toml);
        let mut metal = found_from(&toml);
        metal.set_settle_backend(Some(Box::new(device)));
        let raced = race(&mut reference, &mut metal, 60);
        let report = metal.settle_report();
        println!("{changed}: {raced:?}: {report:?}");
        assert_eq!(raced.difference, None, "{changed}: the worlds diverged");
        assert_eq!(
            (report.ticks, report.reference_ticks),
            (60, 0),
            "{changed}: {report:?}"
        );
        for (total, count) in deaths.iter_mut().zip(raced.deaths) {
            *total += count;
        }
    }
    assert!(deaths.iter().all(|&count| count > 0), "{deaths:?}");
}

#[test]
fn the_preset_with_every_metal_backend_on_one_device_is_the_reference_world() {
    let Some(device) = settle() else { return };
    let owner = device.owner().clone();
    let mut reference = found();
    let mut metal = found();
    metal.set_field_backend(Some(Box::new(
        MetalField::with_owner(owner.clone()).expect("the field kernels compile"),
    )));
    metal.set_sensor_backend(Some(Box::new(
        MetalSensors::with_owner(owner.clone()).expect("the sensor kernels compile"),
    )));
    metal.set_uptake_backend(Some(Box::new(
        MetalUptake::with_owner(owner).expect("the uptake kernels compile"),
    )));
    metal.set_controller_backend(Some(Box::new(
        MetalBackend::open().expect("a Metal device has the controllers too"),
    )));
    metal.set_settle_backend(Some(Box::new(device)));
    let raced = race(&mut reference, &mut metal, 120);
    assert_eq!(raced.difference, None, "the worlds diverged");
    assert_eq!(metal.field_report().steps, 120);
    assert_eq!(metal.sensor_report().ticks, 120);
    assert_eq!(metal.uptake_report().ticks, 120);
    assert!(metal.controller_report().batched > 0);
    assert_eq!(
        (
            metal.settle_report().ticks,
            metal.settle_report().reference_ticks
        ),
        (120, 0)
    );
}

/// What the altered backend does at its tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    /// Move the first organism that moves one ulp further along x.
    Nudge,
    /// Swap the amounts of the first deposit's first two unequal packets.
    Swap,
    /// Fail after emission, leaving its outcomes behind.
    Fail,
    /// Pay one ulp more toxicity for the last organism that pays any.
    Toxicity,
    /// Report a death at motion for an organism the host finds alive.
    Death,
    /// Give a deposit's first packet twice its offer.
    Packet,
    /// Leave one ulp less after chemistry for the last organism with a
    /// gain.
    Balance,
    /// Pay one ulp more of the first maintenance bill to the controller.
    Split,
    /// Flip whether the first motion was scaled.
    Scaled,
    /// Offer one ulp less for the first deposit.
    Offer,
    /// Swap the first two packets of the first deposit with two.
    Order,
    /// Move the first organism that moves to the torus's edge, outside it.
    Torus,
    /// Report the first organism chemistry exhausted as dying at
    /// maintenance. (One reported alive reaches the emission kernel, which
    /// refuses an empty balance itself.)
    Later,
    /// Report a loss for the first organism that gained.
    Gain,
    /// Pay one ulp more of the first maintenance bill.
    Bill,
    /// Pay one ulp less for the first motion that pays.
    Fare,
    /// Pay one ulp less for the first deposit.
    Spend,
    /// Stage a deposit for the first organism that does not emit.
    Stage,
    /// Give the first deposit with two packets its first cell twice.
    Repeat,
    /// Put the first deposit's last packet one cell past the grid.
    Outside,
    /// Give the first deposit's first packet nothing.
    Zero,
    /// Report the first viable attempt as no attempt.
    Abstain,
    /// Report the first organism that made no attempt as cooling down.
    Cool,
    /// Report the first organism cooling down as having made no attempt.
    Warm,
    /// Give the first viable split one ulp less for the child.
    Child,
    /// Report the first attempt below the attempt energy as one that cannot
    /// afford the birth cost.
    Reason,
    /// Give the first attempt below the attempt energy one ulp less.
    Short,
}

/// The world each change is planted in: the preset, unless the change
/// needs an early death or an early proposal. The preset's founders cool
/// down for 64 ticks (0.5 s at Δt = 1/128 s); with a cooldown of 0.01 s
/// they propose from tick 2, below the attempt energy, and with an attempt
/// minimum of 1.0 below their 2.0 they can propose viable births.
fn world_for(change: Change) -> String {
    const COOLDOWN: (&str, &str) = ("cooldown_seconds = 0.5", "cooldown_seconds = 0.01");
    const ATTEMPT: (&str, &str) = (
        "minimum_parent_energy_before_attempt = 3.0",
        "minimum_parent_energy_before_attempt = 1.0",
    );
    match change {
        Change::Later => variant("2.0", "1000.0"),
        Change::Cool | Change::Reason | Change::Short => with(&[COOLDOWN]),
        Change::Abstain | Change::Child => with(&[COOLDOWN, ATTEMPT]),
        _ => PRESET.to_owned(),
    }
}

/// The preset with each line replaced, each found once.
fn with(lines: &[(&str, &str)]) -> String {
    lines.iter().fold(PRESET.to_owned(), |toml, (from, to)| {
        assert_eq!(toml.matches(from).count(), 1, "{from}");
        toml.replace(from, to)
    })
}

/// The Metal phase 6 with one change at one tick.
#[derive(Debug)]
struct Altered {
    inner: MetalSettle,
    ticks: u64,
    at: u64,
    change: Change,
}

impl Altered {
    fn now(&self, change: Change) -> bool {
        self.ticks == self.at && self.change == change
    }
}

impl SettleBackend for Altered {
    fn describe(&self) -> String {
        self.inner.describe()
    }

    fn chemistry(
        &mut self,
        observed: &FieldState,
        constants: &ChemistryConstants,
        organisms: &[ChemistryOrganism],
        packets: &[ChemistryPacket],
        out: &mut Vec<ChemistryOutcome>,
    ) -> Result<(), CoreError> {
        self.ticks += 1;
        self.inner
            .chemistry(observed, constants, organisms, packets, out)?;
        if self.now(Change::Toxicity)
            && let Some(outcome) = out
                .iter_mut()
                .rev()
                .find(|outcome| outcome.paid_toxicity > 0.0)
        {
            outcome.paid_toxicity = outcome.paid_toxicity.next_up();
        }
        if self.now(Change::Balance)
            && let Some(outcome) = out.iter_mut().rev().find(|outcome| outcome.gain > 0.0)
        {
            outcome.remaining = outcome.remaining.next_down();
        }
        if self.now(Change::Gain)
            && let Some(outcome) = out.iter_mut().find(|outcome| outcome.gain > 0.0)
        {
            outcome.gain = -outcome.gain;
        }
        Ok(())
    }

    fn motion(
        &mut self,
        constants: &MotionConstants,
        organisms: &[MotionOrganism],
        out: &mut Vec<MotionOutcome>,
    ) -> Result<(), CoreError> {
        self.inner.motion(constants, organisms, out)?;
        let alive = |outcome: &&mut MotionOutcome| {
            outcome.died.is_none() && outcome.movement.is_some_and(|m| m.velocity[0] != 0.0)
        };
        if self.now(Change::Nudge)
            && let Some(outcome) = out.iter_mut().find(|outcome| {
                alive(outcome) && outcome.position[0].next_up() < constants.world_width
            })
        {
            outcome.position[0] = outcome.position[0].next_up();
        }
        if self.now(Change::Death)
            && let Some(outcome) = out.iter_mut().find(alive)
        {
            outcome.died = Some(ExhaustedAt::Motion);
        }
        if self.now(Change::Split)
            && let Some(bill) = out
                .iter_mut()
                .find_map(|outcome| outcome.maintenance.as_mut())
        {
            bill.controller = bill.controller.next_up();
        }
        if self.now(Change::Scaled)
            && let Some(movement) = out.iter_mut().find_map(|outcome| outcome.movement.as_mut())
        {
            movement.scaled = !movement.scaled;
        }
        if self.now(Change::Torus)
            && let Some(outcome) = out.iter_mut().find(alive)
        {
            outcome.position[0] = constants.world_width;
        }
        if self.now(Change::Later)
            && let Some(outcome) = out
                .iter_mut()
                .find(|outcome| outcome.died == Some(ExhaustedAt::Chemistry))
        {
            outcome.died = Some(ExhaustedAt::Maintenance);
        }
        if self.now(Change::Bill)
            && let Some(bill) = out
                .iter_mut()
                .find_map(|outcome| outcome.maintenance.as_mut())
        {
            bill.paid = bill.paid.next_up();
        }
        if self.now(Change::Fare)
            && let Some(movement) = out
                .iter_mut()
                .filter_map(|outcome| outcome.movement.as_mut())
                .find(|movement| movement.paid > 0.0)
        {
            movement.paid = movement.paid.next_down();
        }
        Ok(())
    }

    fn emission(
        &mut self,
        grid: &FieldGrid,
        efficiency: f64,
        organisms: &[EmissionOrganism],
        out: &mut Vec<EmissionOutcome>,
    ) -> Result<(), CoreError> {
        self.inner.emission(grid, efficiency, organisms, out)?;
        if self.now(Change::Swap)
            && let Some(outcome) = out.iter_mut().find(|outcome| {
                outcome.packet_count >= 2 && outcome.packets[0].1 != outcome.packets[1].1
            })
        {
            let (first, second) = (outcome.packets[0].1, outcome.packets[1].1);
            outcome.packets[0].1 = second;
            outcome.packets[1].1 = first;
        }
        if self.now(Change::Packet)
            && let Some(outcome) = out.iter_mut().find(|outcome| outcome.packet_count >= 1)
        {
            outcome.packets[0].1 = 2.0 * outcome.offered;
        }
        if self.now(Change::Offer)
            && let Some(outcome) = out.iter_mut().find(|outcome| outcome.staged)
        {
            outcome.offered = outcome.offered.next_down();
        }
        if self.now(Change::Order)
            && let Some(outcome) = out.iter_mut().find(|outcome| outcome.packet_count >= 2)
        {
            outcome.packets.swap(0, 1);
        }
        if self.now(Change::Spend)
            && let Some(outcome) = out.iter_mut().find(|outcome| outcome.staged)
        {
            outcome.paid = outcome.paid.next_down();
        }
        if self.now(Change::Stage)
            && let Some((_, outcome)) = organisms
                .iter()
                .zip(out.iter_mut())
                .find(|(organism, _)| organism.emission.is_none())
        {
            outcome.staged = true;
        }
        if self.now(Change::Repeat)
            && let Some(outcome) = out.iter_mut().find(|outcome| outcome.packet_count >= 2)
        {
            outcome.packets[1].0 = outcome.packets[0].0;
        }
        if self.now(Change::Outside)
            && let Some(outcome) = out.iter_mut().find(|outcome| outcome.packet_count >= 1)
        {
            let cells = usize::try_from(grid.cell_count()).unwrap_or(usize::MAX);
            outcome.packets[outcome.packet_count - 1].0 = cells;
        }
        if self.now(Change::Zero)
            && let Some(outcome) = out.iter_mut().find(|outcome| outcome.packet_count >= 1)
        {
            outcome.packets[0].1 = 0.0;
        }
        if self.now(Change::Fail) {
            return Err(CoreError::StateConflict {
                operation: "planted settle failure",
                detail: "the test's backend fails here".to_owned(),
            });
        }
        Ok(())
    }

    fn proposal(
        &mut self,
        constants: &ProposalConstants,
        organisms: &[ProposalOrganism],
        out: &mut Vec<ProposalOutcome>,
    ) -> Result<(), CoreError> {
        self.inner.proposal(constants, organisms, out)?;
        let viable =
            |outcome: &&mut ProposalOutcome| matches!(outcome, ProposalOutcome::Viable { .. });
        if self.now(Change::Abstain)
            && let Some(outcome) = out.iter_mut().find(viable)
        {
            *outcome = ProposalOutcome::NoAttempt;
        }
        if self.now(Change::Cool)
            && let Some(outcome) = out
                .iter_mut()
                .find(|outcome| **outcome == ProposalOutcome::NoAttempt)
        {
            *outcome = ProposalOutcome::CoolingDown;
        }
        if self.now(Change::Warm)
            && let Some(outcome) = out
                .iter_mut()
                .find(|outcome| **outcome == ProposalOutcome::CoolingDown)
        {
            *outcome = ProposalOutcome::NoAttempt;
        }
        if self.now(Change::Child)
            && let Some(ProposalOutcome::Viable { child, .. }) = out.iter_mut().find(viable)
        {
            *child = child.next_down();
        }
        if self.now(Change::Reason)
            && let Some(outcome) = out
                .iter_mut()
                .find(|outcome| matches!(outcome, ProposalOutcome::BelowAttemptEnergy { .. }))
            && let ProposalOutcome::BelowAttemptEnergy { available } = *outcome
        {
            *outcome = ProposalOutcome::BirthCostUnaffordable { available };
        }
        if self.now(Change::Short)
            && let Some(ProposalOutcome::BelowAttemptEnergy { available }) = out
                .iter_mut()
                .find(|outcome| matches!(outcome, ProposalOutcome::BelowAttemptEnergy { .. }))
        {
            *available = available.next_down();
        }
        Ok(())
    }
}

fn altered(at: u64, change: Change) -> Option<Altered> {
    settle().map(|inner| Altered {
        inner,
        ticks: 0,
        at,
        change,
    })
}

#[test]
fn a_change_the_host_cannot_see_changes_the_world() {
    for (change, at) in [(Change::Nudge, 7), (Change::Swap, 9), (Change::Abstain, 3)] {
        let Some(device) = altered(at, change) else {
            return;
        };
        let toml = world_for(change);
        let mut reference = found_from(&toml);
        let mut metal = found_from(&toml);
        metal.set_settle_backend(Some(Box::new(device)));
        let raced = race(&mut reference, &mut metal, 12);
        assert_eq!(raced.difference, Some(at), "{change:?}");
        // The host's replay cannot see it: the backend served every tick.
        let report = metal.settle_report();
        assert_eq!(
            (report.ticks, report.reference_ticks),
            (12, 0),
            "{change:?}: {report:?}"
        );
    }
}

#[test]
fn a_failing_or_refused_settle_backend_hands_its_tick_to_the_reference_and_is_dropped() {
    for (change, at, cause) in [
        (Change::Fail, 5, "planted settle failure"),
        (Change::Toxicity, 6, "chemistry paid"),
        (Change::Death, 4, "did not settle emission"),
        (Change::Packet, 8, "deposit split"),
        (Change::Balance, 3, "chemistry left"),
        (Change::Split, 7, "maintenance split"),
        (Change::Scaled, 5, "motion requested"),
        (Change::Offer, 6, "emission requested"),
        (Change::Order, 2, "out of order"),
        (Change::Torus, 3, "off the torus"),
        (Change::Later, 3, "on the backend and at Some(Chemistry)"),
        (Change::Gain, 4, "chemistry's gain"),
        (Change::Bill, 5, "maintenance paid"),
        (Change::Fare, 6, "motion paid"),
        (Change::Spend, 7, "emission paid"),
        (Change::Stage, 8, "does not emit"),
        (Change::Repeat, 3, "out of order"),
        (Change::Outside, 4, "outside the grid"),
        (Change::Zero, 5, "amount 0e0"),
        (Change::Cool, 4, "proposed CoolingDown"),
        (Change::Warm, 6, "proposed NoAttempt"),
        (Change::Child, 3, "split as Viable"),
        (Change::Reason, 5, "split as BirthCostUnaffordable"),
        (Change::Short, 6, "split as BelowAttemptEnergy"),
    ] {
        let Some(device) = altered(at, change) else {
            return;
        };
        let toml = world_for(change);
        let mut reference = found_from(&toml);
        let mut metal = found_from(&toml);
        metal.set_settle_backend(Some(Box::new(device)));
        let raced = race(&mut reference, &mut metal, 12);
        assert_eq!(
            raced.difference, None,
            "{change:?}: the fallback must be the reference"
        );
        let report = metal.settle_report();
        assert_eq!(
            (report.ticks, report.reference_ticks),
            (at - 1, 1),
            "{change:?}: {report:?}"
        );
        assert!(
            report
                .failure
                .as_deref()
                .is_some_and(|failure| failure.contains(cause)),
            "{change:?}: {report:?}"
        );
    }
}
