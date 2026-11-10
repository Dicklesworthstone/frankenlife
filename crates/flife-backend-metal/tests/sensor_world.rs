//! FL-031's sensor wiring: the real small-world preset with phase 3's
//! sampling on Metal is the reference world, digest for digest, at every
//! tick, alone and together with the Metal field and controllers. Flipped
//! bits in one tick's samples change the world (the planted negative), and
//! a backend that fails mid-run hands that tick to the reference and is
//! dropped, leaving the run the reference's.

use flife_backend_metal::{MetalBackend, MetalField, MetalSensors};
use flife_core::{CoreError, ValidatedConfig};
use flife_engine::{Arrangement, NumericalProfile, Stepped, World};
use flife_field::{FieldState, RawSample, SensorBackend, SensorPyramid, SensorQuery};
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
fn sensors() -> Option<MetalSensors> {
    match MetalSensors::open() {
        Ok(sensors) => Some(sensors),
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
fn the_preset_with_its_sensors_on_metal_is_the_reference_world_at_every_tick() {
    let Some(device) = sensors() else { return };
    let description = device.describe();
    let owner = device.owner().clone();
    let mut reference = found();
    let mut metal = found();
    metal.set_sensor_backend(Some(Box::new(device)));
    let (difference, births) = race(&mut reference, &mut metal, 400);
    let report = metal.sensor_report().clone();
    println!("births {births}: {report:?}");
    assert_eq!(difference, None, "the worlds diverged");
    assert_eq!(report.backend.as_deref(), Some(description.as_str()));
    assert_eq!((report.ticks, report.reference_ticks), (400, 0));
    assert_eq!(report.failure, None);
    assert!(births > 0);
    // FL-028: the field, the coarse levels, one parameter buffer per coarse
    // level, the sample parameters, queries and samples, with no lease,
    // command or retiring buffer left over.
    let config = ValidatedConfig::from_toml(PRESET).expect("the preset validates");
    let levels = config.report().pyramid_levels as usize;
    let usage = owner.usage();
    assert_eq!(
        (
            usage.buffers,
            usage.retiring,
            usage.read_leases,
            usage.write_leases,
            usage.running
        ),
        (5 + levels, 0, [0; 3], 0, 0),
        "{usage:?}"
    );
}

#[test]
fn the_preset_with_sensors_field_and_controllers_on_one_metal_device_is_the_reference_world() {
    let Some(field) = field() else { return };
    let owner = field.owner().clone();
    let device = MetalSensors::with_owner(owner).expect("the sensor kernels compile");
    let mut reference = found();
    let mut metal = found();
    metal.set_field_backend(Some(Box::new(field)));
    metal.set_sensor_backend(Some(Box::new(device)));
    metal.set_controller_backend(Some(Box::new(
        MetalBackend::open().expect("a Metal device has the controllers too"),
    )));
    let (difference, _) = race(&mut reference, &mut metal, 200);
    assert_eq!(difference, None, "the worlds diverged");
    assert_eq!(metal.field_report().steps, 200);
    assert_eq!(metal.sensor_report().ticks, 200);
    assert!(metal.controller_report().batched > 0);
}

/// The Metal sensors with one change at one tick: a bit of every sample's
/// energy density flipped, or a failure after its samples.
#[derive(Debug)]
struct Altered {
    inner: MetalSensors,
    ticks: u64,
    at: u64,
    fail: bool,
}

impl SensorBackend for Altered {
    fn describe(&self) -> String {
        self.inner.describe()
    }

    fn sample(
        &mut self,
        pyramid: &SensorPyramid,
        field: &FieldState,
        queries: &[SensorQuery],
        samples: &mut Vec<RawSample>,
    ) -> Result<(), CoreError> {
        self.inner.sample(pyramid, field, queries, samples)?;
        self.ticks += 1;
        if self.ticks == self.at {
            if self.fail {
                // Leave damaged samples behind, as a device failing mid-tick
                // could.
                for sample in samples.iter_mut() {
                    sample.values[0] = -1.0;
                }
                return Err(CoreError::StateConflict {
                    operation: "planted sensor failure",
                    detail: "the test's backend fails here".to_owned(),
                });
            }
            for sample in samples.iter_mut() {
                sample.values[0] = f64::from_bits(sample.values[0].to_bits() ^ (1 << 51));
            }
        }
        Ok(())
    }
}

#[test]
fn flipped_sample_bits_change_the_world() {
    let Some(inner) = sensors() else { return };
    let mut reference = found();
    let mut metal = found();
    metal.set_sensor_backend(Some(Box::new(Altered {
        inner,
        ticks: 0,
        at: 7,
        fail: false,
    })));
    let (difference, _) = race(&mut reference, &mut metal, 10);
    assert_eq!(difference, Some(7));
}

#[test]
fn a_failing_sensor_backend_hands_its_tick_to_the_reference_and_is_dropped() {
    let Some(inner) = sensors() else { return };
    let mut reference = found();
    let mut metal = found();
    metal.set_sensor_backend(Some(Box::new(Altered {
        inner,
        ticks: 0,
        at: 5,
        fail: true,
    })));
    let (difference, _) = race(&mut reference, &mut metal, 12);
    assert_eq!(difference, None, "the fallback must be the reference");
    let report = metal.sensor_report();
    assert_eq!((report.ticks, report.reference_ticks), (4, 1), "{report:?}");
    assert!(
        report
            .failure
            .as_deref()
            .is_some_and(|failure| failure.contains("planted sensor failure")),
        "{report:?}"
    );
}
