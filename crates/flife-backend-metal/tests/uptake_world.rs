//! FL-031's uptake wiring: the real small-world preset with phase 5's
//! demands and grants on Metal is the reference world, digest for digest,
//! at every tick, alone and together with the Metal field, sensors and
//! controllers. One quantum moved between two packets of one cell, a change
//! the host's sum check cannot see, changes the world (the planted
//! negative). A backend that fails mid-run, or grants a cell one quantum too
//! many, hands that tick to the reference and is dropped, leaving the run
//! the reference's.

use flife_backend_metal::{MetalBackend, MetalField, MetalSensors, MetalUptake};
use flife_core::{CoreError, ValidatedConfig};
use flife_engine::{Arrangement, NumericalProfile, Stepped, World};
use flife_field::{ExchangeStencil, FieldState, RequestRule, UptakeBackend, UptakeCell};
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
fn uptake() -> Option<MetalUptake> {
    match MetalUptake::open() {
        Ok(uptake) => Some(uptake),
        Err(CoreError::MissingCapability { .. }) if !cfg!(target_os = "macos") => None,
        Err(error) => panic!("this host should have Metal: {error}"),
    }
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
fn the_preset_with_its_uptake_on_metal_is_the_reference_world_at_every_tick() {
    let Some(device) = uptake() else { return };
    let description = device.describe();
    let owner = device.owner().clone();
    let mut reference = found();
    let mut metal = found();
    let before = metal.allocated().scratch;
    metal.set_uptake_backend(Some(Box::new(device)));
    let installed = metal.allocated().scratch;
    let (difference, births) = race(&mut reference, &mut metal, 400);
    let report = metal.uptake_report().clone();
    println!("births {births}: {report:?}; scratch {before} -> {installed}");
    assert_eq!(difference, None, "the worlds diverged");
    assert_eq!(report.backend.as_deref(), Some(description.as_str()));
    assert_eq!((report.ticks, report.reference_ticks), (400, 0));
    assert_eq!(report.failure, None);
    assert!(births > 0);
    // FL-043: installing reserves the host's lists for the capacity, and
    // stepping never grows them.
    assert!(installed > before, "{before} {installed}");
    assert_eq!(metal.allocated().scratch, installed);
    assert_eq!(metal.footprint().scratch, installed);
    // FL-028: eleven buffers, with no lease, command or retiring buffer
    // left over.
    let usage = owner.usage();
    assert_eq!(
        (
            usage.buffers,
            usage.retiring,
            usage.read_leases,
            usage.write_leases,
            usage.running
        ),
        (11, 0, [0; 3], 0, 0),
        "{usage:?}"
    );

    // Removing it releases the lists.
    metal.set_uptake_backend(None);
    assert_eq!(metal.allocated().scratch, before);
    assert_eq!(metal.uptake_report().backend, None);
}

#[test]
fn the_preset_with_field_sensors_uptake_and_controllers_on_one_metal_device_is_the_reference_world()
{
    let Some(field) = field() else { return };
    let owner = field.owner().clone();
    let sensors = MetalSensors::with_owner(owner.clone()).expect("the sensor kernels compile");
    let device = MetalUptake::with_owner(owner).expect("the uptake kernels compile");
    let mut reference = found();
    let mut metal = found();
    metal.set_field_backend(Some(Box::new(field)));
    metal.set_sensor_backend(Some(Box::new(sensors)));
    metal.set_uptake_backend(Some(Box::new(device)));
    metal.set_controller_backend(Some(Box::new(
        MetalBackend::open().expect("a Metal device has the controllers too"),
    )));
    let (difference, _) = race(&mut reference, &mut metal, 200);
    assert_eq!(difference, None, "the worlds diverged");
    assert_eq!(metal.field_report().steps, 200);
    assert_eq!(metal.sensor_report().ticks, 200);
    assert_eq!(metal.uptake_report().ticks, 200);
    assert!(metal.controller_report().batched > 0);
}

/// What the altered backend does at its tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    /// Move one quantum between two packets of one cell.
    Move,
    /// Grant one packet one quantum more.
    Add,
    /// Fail after the grants, leaving damaged quanta behind.
    Fail,
}

/// The Metal uptake with one change at one tick.
#[derive(Debug)]
struct Altered {
    inner: MetalUptake,
    ticks: u64,
    at: u64,
    change: Change,
}

impl UptakeBackend for Altered {
    fn describe(&self) -> String {
        self.inner.describe()
    }

    fn request(
        &mut self,
        field: &FieldState,
        rule: &RequestRule,
        stencils: &[ExchangeStencil],
        densities: &mut Vec<f64>,
        requests: &mut Vec<f64>,
    ) -> Result<(), CoreError> {
        self.inner
            .request(field, rule, stencils, densities, requests)
    }

    fn resolve(
        &mut self,
        cells: &[UptakeCell],
        requested: &[f64],
        demanded: &mut Vec<f64>,
        quanta: &mut Vec<u32>,
    ) -> Result<(), CoreError> {
        self.inner.resolve(cells, requested, demanded, quanta)?;
        self.ticks += 1;
        if self.ticks != self.at {
            return Ok(());
        }
        match self.change {
            Change::Move => {
                // The first cell with two packets, one of them granted.
                for cell in cells.iter().filter(|cell| cell.len > 1) {
                    let group = &mut quanta[cell.start..cell.start + cell.len];
                    if let Some(from) = group.iter().position(|&share| share > 0) {
                        group[from] -= 1;
                        group[(from + 1) % cell.len] += 1;
                        break;
                    }
                }
                Ok(())
            }
            Change::Add => {
                if let Some(share) = quanta.first_mut() {
                    *share += 1;
                }
                Ok(())
            }
            Change::Fail => {
                quanta.iter_mut().for_each(|share| *share = u32::MAX);
                Err(CoreError::StateConflict {
                    operation: "planted uptake failure",
                    detail: "the test's backend fails here".to_owned(),
                })
            }
        }
    }
}

fn altered(inner: MetalUptake, at: u64, change: Change) -> Altered {
    Altered {
        inner,
        ticks: 0,
        at,
        change,
    }
}

#[test]
fn one_quantum_moved_between_two_packets_changes_the_world() {
    let Some(inner) = uptake() else { return };
    let mut reference = found();
    let mut metal = found();
    metal.set_uptake_backend(Some(Box::new(altered(inner, 7, Change::Move))));
    let (difference, _) = race(&mut reference, &mut metal, 10);
    assert_eq!(difference, Some(7));
    // The host's checks cannot see it: the backend served every tick.
    let report = metal.uptake_report();
    assert_eq!(
        (report.ticks, report.reference_ticks),
        (10, 0),
        "{report:?}"
    );
}

#[test]
fn a_failing_uptake_backend_hands_its_tick_to_the_reference_and_is_dropped() {
    for (change, at, cause) in [
        (Change::Fail, 5, "planted uptake failure"),
        (Change::Add, 6, "quanta, not its"),
    ] {
        let Some(inner) = uptake() else { return };
        let mut reference = found();
        let mut metal = found();
        metal.set_uptake_backend(Some(Box::new(altered(inner, at, change))));
        let (difference, _) = race(&mut reference, &mut metal, 12);
        assert_eq!(
            difference, None,
            "{change:?}: the fallback must be the reference"
        );
        let report = metal.uptake_report();
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
