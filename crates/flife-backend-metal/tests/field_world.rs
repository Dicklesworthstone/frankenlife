//! FL-031's engine wiring: the real small-world preset with phase 2 on the
//! Metal field backend is the reference world, digest for digest, at every
//! tick, alone and together with the Metal controllers. A flipped bit in
//! one field value changes the world (the planted negative), and a backend
//! that fails mid-run hands that step to the reference and is dropped,
//! leaving the run the reference's.

use flife_backend_metal::{MetalBackend, MetalField};
use flife_core::{CoreError, TimeStep, ValidatedConfig};
use flife_engine::{Arrangement, NumericalProfile, Stepped, World};
use flife_field::{FieldBackend, FieldPair, FieldStepReport, Source, SourceKernel, TransportPlan};
use std::num::NonZeroUsize;

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure is a test failure"
)]
fn found() -> World {
    let config = ValidatedConfig::from_toml(PRESET).expect("the preset validates");
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
fn field() -> Option<MetalField> {
    match MetalField::open() {
        Ok(field) => Some(field),
        Err(CoreError::MissingCapability { .. }) if !cfg!(target_os = "macos") => None,
        Err(error) => panic!("this host should have Metal: {error}"),
    }
}

/// Step both worlds; the first tick whose digests differ, and the births.
#[expect(clippy::expect_used, clippy::panic, reason = "test helper")]
fn race(reference: &mut World, other: &mut World, ticks: u64) -> (Option<u64>, usize) {
    let (mut difference, mut births) = (None, 0);
    for _ in 0..ticks {
        let expected = reference.step().expect("the reference steps");
        let got = other.step().expect("the other world steps");
        match (expected, got) {
            (Stepped::Published(_), Stepped::Published(report)) => births += report.births.len(),
            (expected, got) => panic!("unexpected outcomes {expected:?} and {got:?}"),
        }
        if difference.is_none() && reference.digest() != other.digest() {
            difference = Some(other.tick().get());
        }
    }
    (difference, births)
}

#[test]
fn the_preset_with_its_field_on_metal_is_the_reference_world_at_every_tick() {
    let Some(device) = field() else { return };
    let description = device.describe();
    let owner = device.owner().clone();
    let mut reference = found();
    let mut metal = found();
    metal.set_field_backend(Some(Box::new(device)));
    let (difference, births) = race(&mut reference, &mut metal, 400);
    let report = metal.field_report().clone();
    println!("births {births}: {report:?}");
    assert_eq!(difference, None, "the worlds diverged");
    assert_eq!(report.backend.as_deref(), Some(description.as_str()));
    assert_eq!((report.steps, report.reference_steps), (400, 0));
    assert_eq!(report.failure, None);
    assert!(births > 0);
    // FL-028: the backend's twelve buffers (three field copies, the moments,
    // weights, fault word and six of parameters and candidates), with no
    // lease, command or retiring buffer left over.
    let usage = owner.usage();
    assert_eq!(
        (
            usage.buffers,
            usage.retiring,
            usage.read_leases,
            usage.write_leases,
            usage.running
        ),
        (12, 0, [0; 3], 0, 0),
        "{usage:?}"
    );
}

#[test]
fn the_preset_with_field_and_controllers_on_metal_is_the_reference_world() {
    let Some(device) = field() else { return };
    let mut reference = found();
    let mut metal = found();
    metal.set_field_backend(Some(Box::new(device)));
    metal.set_controller_backend(Some(Box::new(
        MetalBackend::open().expect("a Metal device has the controllers too"),
    )));
    let (difference, _) = race(&mut reference, &mut metal, 200);
    assert_eq!(difference, None, "the worlds diverged");
    assert_eq!(metal.field_report().steps, 200);
    assert!(metal.controller_report().batched > 0);
}

/// The Metal field with one change at one step: a bit of one candidate
/// value flipped, or a failure after its step.
#[derive(Debug)]
struct Altered {
    inner: MetalField,
    steps: u64,
    at: u64,
    fail: bool,
}

impl FieldBackend for Altered {
    fn describe(&self) -> String {
        self.inner.describe()
    }

    fn advance(
        &mut self,
        pair: &mut FieldPair,
        plan: &TransportPlan,
        kernel: &SourceKernel,
        sources: &[Source],
        step: TimeStep,
    ) -> Result<FieldStepReport, CoreError> {
        let report = self.inner.advance(pair, plan, kernel, sources, step)?;
        self.steps += 1;
        if self.steps == self.at {
            if self.fail {
                // Leave a damaged candidate behind, as a device failing
                // mid-step could.
                pair.stage().1.energy_mut()[100] = -1.0;
                return Err(CoreError::StateConflict {
                    operation: "planted field failure",
                    detail: "the test's backend fails here".to_owned(),
                });
            }
            let moment = &mut pair.stage().1.moments_mut()[5_000];
            *moment = f32::from_bits(moment.to_bits() ^ 1);
        }
        Ok(report)
    }
}

#[test]
fn one_flipped_field_bit_changes_the_world() {
    let Some(inner) = field() else { return };
    let mut reference = found();
    let mut metal = found();
    metal.set_field_backend(Some(Box::new(Altered {
        inner,
        steps: 0,
        at: 7,
        fail: false,
    })));
    let (difference, _) = race(&mut reference, &mut metal, 10);
    assert_eq!(difference, Some(7));
}

#[test]
fn a_failing_field_backend_hands_its_step_to_the_reference_and_is_dropped() {
    let Some(inner) = field() else { return };
    let mut reference = found();
    let mut metal = found();
    metal.set_field_backend(Some(Box::new(Altered {
        inner,
        steps: 0,
        at: 5,
        fail: true,
    })));
    let (difference, _) = race(&mut reference, &mut metal, 12);
    assert_eq!(difference, None, "the fallback must be the reference");
    let report = metal.field_report();
    assert_eq!((report.steps, report.reference_steps), (4, 1), "{report:?}");
    assert!(
        report
            .failure
            .as_deref()
            .is_some_and(|failure| failure.contains("planted field failure")),
        "{report:?}"
    );
}
