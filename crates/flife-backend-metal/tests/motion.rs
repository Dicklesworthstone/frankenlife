//! FL-032, second stage: phase 6's maintenance and motion on Metal are
//! flife-engine's, bit for bit. The device's new arithmetic (a correctly
//! rounded square root, an exact remainder, the Euclidean remainder and the
//! torus wrap) is checked against Rust's and flife-field's on a quarter
//! million operand pairs. The kernel is checked against the reference's
//! own calls (`MaintenanceCosts::bill` and `settle`, `Vitality::check`,
//! `uptake_ratio`, `MotionResponse::new`, and `Motion`'s `acceleration`,
//! `propose`, `settle` and `displace`), in `settle_stages`' order, on
//! thousands of organisms under the preset's constants and under extreme
//! ones: deaths at each stage, unaffordable motion, both clamps, and wraps
//! across both edges. Every input the reference refuses is refused, and
//! what the reference never reads is not checked.

use flife_backend_metal::{
    ExhaustedAt, MOTION_OPS_WORDS, MOTION_TERMS, MaintenanceOutcome, MetalMotion, MotionConstants,
    MotionOrganism, MotionOutcome, MovementOutcome,
};
use flife_core::ledger::EnergyBudget;
use flife_core::{CoreError, TimeStep, ValidatedConfig};
use flife_engine::{BudgetStage, MaintenanceCosts, Motion, MotionResponse, Vitality, uptake_ratio};
use flife_field::FieldGrid;
use flife_field::grid::wrap_coordinate;

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

/// `None` off macOS, where `open` must refuse with its typed error.
#[expect(clippy::panic, reason = "test helper")]
fn metal() -> Option<MetalMotion> {
    match MetalMotion::open() {
        Ok(motion) => Some(motion),
        Err(CoreError::MissingCapability { .. }) if !cfg!(target_os = "macos") => None,
        Err(error) => panic!("this host should have Metal: {error}"),
    }
}

/// xorshift64*.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in `[0, 1)`.
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1_u64 << 53) as f64
    }

    /// Uniform in `[-1, 1)`.
    fn signed(&mut self) -> f64 {
        2.0 * self.unit() - 1.0
    }

    /// A binary64 pattern: full random bits (infinities and NaNs among
    /// them), operands near each other's magnitude or exponents apart,
    /// subnormals, zeros of both signs, and coordinates around a period.
    fn pattern(&mut self, other: u64) -> u64 {
        let bits = self.next();
        match bits % 10 {
            0 => bits & 0x8000_0000_0000_0000,
            1 => other ^ (self.next() & 0x8000_0000_000F_FFFF),
            2 => bits & 0x800F_FFFF_FFFF_FFFF,
            3 => (self.signed() * 300.0).to_bits(),
            4 => (self.signed() * 1.0e-3).to_bits(),
            5 => {
                // Exponents up to 64 apart from `other`.
                let shift = (self.next() % 128) << 52;
                (other & 0x800F_FFFF_FFFF_FFFF) | ((other & 0x7FF0_0000_0000_0000) ^ shift)
            }
            _ => bits,
        }
    }
}

fn same(device: f64, reference: f64) -> bool {
    device.to_bits() == reference.to_bits() || (device.is_nan() && reference.is_nan())
}

fn pair_value(row: &[u32; MOTION_OPS_WORDS], k: usize) -> f64 {
    f64::from_bits(u64::from(row[2 * k]) | (u64::from(row[2 * k + 1]) << 32))
}

/// The square root, the remainders and the wrap are Rust's and
/// flife-field's on every pair, including NaN results, the signs of zeros,
/// subnormals and remainders that round up to the period.
#[test]
fn the_motion_arithmetic_is_the_references() -> Result<(), CoreError> {
    let Some(motion) = metal() else {
        return Ok(());
    };
    let mut rng = Rng(0x0C4E_3157_0000_0072);
    let mut pairs = Vec::new();
    let mut previous = 1.0_f64.to_bits();
    while pairs.len() < 1 << 18 {
        let a = rng.pattern(previous);
        let b = rng.pattern(a);
        previous = b;
        pairs.push([a, b]);
    }
    // Coordinates around periods: inside, a step past either edge, a tiny
    // negative whose Euclidean remainder rounds up to the period, and
    // multiples of it.
    for length in [128.0_f64, 100.5, 1.0, 3.0e5, 0.1, 7.0e-300] {
        for x in [
            0.0,
            -0.0,
            length,
            -length,
            length * 2.0,
            f64::from_bits(length.to_bits() - 1),
            -f64::from_bits(1),
            -1.0e-300,
            -length * 1.0e-17,
            length * 0.5,
            length * 1.000_000_1,
            -length * 0.999_999_9,
            length * 5.0e15,
            1.0e300,
        ] {
            pairs.push([x.to_bits(), length.to_bits()]);
            pairs.push([x.to_bits(), (-length).to_bits()]);
        }
    }
    let edges = [
        0.0,
        -0.0,
        f64::from_bits(1),
        -f64::from_bits(1),
        f64::MIN_POSITIVE,
        f64::from_bits(f64::MIN_POSITIVE.to_bits() - 1),
        f64::MAX,
        f64::MIN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NAN,
        1.0,
        2.0,
        4.0,
        0.25,
        f64::from_bits(2.0_f64.to_bits() - 1),
        f64::from_bits(1.0_f64.to_bits() + 1),
        1.0e-300,
    ];
    for &a in &edges {
        for &b in &edges {
            pairs.push([a.to_bits(), b.to_bits()]);
        }
    }
    while !pairs.len().is_multiple_of(64) {
        pairs.push([0, 0]);
    }
    let mut rows = Vec::new();
    motion.motion_ops(&pairs, &mut rows)?;
    assert_eq!(rows.len(), pairs.len());
    // Square roots of positive values; remainders that divide (|a| > |b|);
    // Euclidean remainders that add the period; wraps.
    let mut counted = [0_usize; 4];
    for (index, ([a, b], row)) in pairs.iter().zip(&rows).enumerate() {
        let (a, b) = (f64::from_bits(*a), f64::from_bits(*b));
        let (got, want) = (pair_value(row, 0), a.sqrt());
        assert!(
            same(got, want),
            "pair {index}: sqrt({a:e}) gave {got:e}, not {want:e}"
        );
        counted[0] += usize::from(a > 0.0 && a.is_finite());
        let (got, want) = (pair_value(row, 1), a % b);
        assert!(
            same(got, want),
            "pair {index}: {a:e} % {b:e} gave {got:e}, not {want:e}"
        );
        counted[1] += usize::from(a.is_finite() && b.is_finite() && a.abs() > b.abs() && b != 0.0);
        // softfloat64 carries no infinities: the one infinite remainder, a
        // negative a on an infinite period, is a NaN on the device. The
        // kernel's periods are finite.
        let (got, want) = (pair_value(row, 2), a.rem_euclid(b));
        if want.is_infinite() {
            assert!(
                got.is_nan() && b.is_infinite(),
                "pair {index}: rem_euclid({a:e}, {b:e}) gave {got:e}"
            );
        } else {
            assert!(
                same(got, want),
                "pair {index}: rem_euclid({a:e}, {b:e}) gave {got:e}, not {want:e}"
            );
        }
        counted[2] += usize::from((a % b) < 0.0);
        let got = pair_value(row, 3);
        match wrap_coordinate(a, b.abs()) {
            Ok(want) => {
                assert!(
                    same(got, want) && !got.is_nan(),
                    "pair {index}: wrap({a:e}, {:e}) gave {got:e}, not {want:e}",
                    b.abs()
                );
                counted[3] += 1;
            }
            Err(_) => assert!(
                got.is_nan(),
                "pair {index}: wrap({a:e}, {b:e}) gave {got:e}"
            ),
        }
    }
    assert!(counted[0] > 100_000, "{} square roots", counted[0]);
    assert!(counted[1] > 50_000, "{} dividing remainders", counted[1]);
    assert!(counted[2] > 20_000, "{} Euclidean corrections", counted[2]);
    assert!(counted[3] > 100_000, "{} wraps", counted[3]);
    // The rounding-up case is among the pairs: the remainder of a tiny
    // negative coordinate is the period itself, which wraps to zero.
    assert!(same((-1.0e-300_f64).rem_euclid(128.0), 128.0));
    assert!(same(wrap_coordinate(-1.0e-300, 128.0)?, 0.0));
    Ok(())
}

/// The reference's rules for the two stages, and the device's constants.
struct Rules {
    step: TimeStep,
    costs: MaintenanceCosts,
    motion: Motion,
    grid: FieldGrid,
    capacity: f64,
    constants: MotionConstants,
}

/// Plain numbers for one rule set.
#[derive(Clone, Copy)]
struct Numbers {
    step: (u32, u32),
    basal: f64,
    sensor: f64,
    sensor_unit_scale: f64,
    neural: f64,
    drag: f64,
    maximum_speed: f64,
    maximum_acceleration: f64,
    cost: f64,
    response_norm_limit: f64,
    capacity: f64,
    world: (u32, u32, f64, f64),
}

#[expect(clippy::expect_used, reason = "test helper")]
fn rules(n: Numbers) -> Rules {
    let step = TimeStep::new(n.step.0, n.step.1).expect("a timestep");
    let costs = MaintenanceCosts::new(n.basal, n.sensor, n.sensor_unit_scale, 1.0, n.neural)
        .expect("valid costs");
    let motion = Motion::new(
        step,
        n.drag,
        n.maximum_speed,
        n.maximum_acceleration,
        n.cost,
        n.response_norm_limit,
    )
    .expect("valid motion");
    let grid = FieldGrid::new(n.world.0, n.world.1, n.world.2, n.world.3).expect("a grid");
    Rules {
        step,
        costs,
        motion,
        grid,
        capacity: n.capacity,
        constants: MotionConstants {
            step_seconds: step.seconds_per_tick(),
            basal: n.basal,
            sensor: n.sensor,
            sensor_unit_scale: n.sensor_unit_scale,
            neural: n.neural,
            decay: motion.decay(),
            force_coefficient: motion.force_coefficient(),
            maximum_speed: n.maximum_speed,
            maximum_acceleration: n.maximum_acceleration,
            cost: n.cost,
            response_norm_limit: n.response_norm_limit,
            uptake_capacity: n.capacity,
            world_width: n.world.2,
            world_height: n.world.3,
        },
    }
}

/// The preset's rules.
#[expect(clippy::expect_used, reason = "test helper")]
fn preset_rules() -> Rules {
    let config = ValidatedConfig::from_toml(PRESET).expect("the preset validates");
    let step = config.report().timestep;
    let chemistry = &config.config().chemistry;
    let costs = &config.config().costs;
    let world = &config.config().world;
    let grid = FieldGrid::from_config(&config).expect("the preset's grid");
    let preset = rules(Numbers {
        step: (step.numerator(), step.denominator()),
        basal: costs.basal_energy_per_second,
        sensor: costs.sensor_energy_per_second_at_unit_scale,
        sensor_unit_scale: costs.sensor_unit_scale_world_units,
        neural: costs.neural_energy_per_parameter_evaluation,
        drag: chemistry.drag_per_second,
        maximum_speed: chemistry.maximum_speed,
        maximum_acceleration: chemistry.maximum_acceleration,
        cost: costs.motion_energy_per_speed_squared_second,
        response_norm_limit: chemistry.motion_response_norm_limit,
        capacity: chemistry.uptake_capacity_per_second,
        world: (
            world.grid_width,
            world.grid_height,
            world.width,
            world.height,
        ),
    });
    // The same objects the engine builds from the configuration.
    assert_eq!(
        preset.costs,
        MaintenanceCosts::from_config(&config).expect("costs")
    );
    assert_eq!(preset.motion, Motion::from_config(&config).expect("motion"));
    assert_eq!(preset.grid, grid);
    preset
}

/// What the reference's `settle_stages` computes for `organism` from the
/// exhaustion check after chemistry to the one after motion, in its order
/// and with its calls.
fn reference(rules: &Rules, organism: &MotionOrganism) -> Result<MotionOutcome, CoreError> {
    let mut budget = EnergyBudget::new(organism.remaining)?;
    assert_eq!(
        budget.is_exhausted(),
        organism.exhausted,
        "a consistent budget"
    );
    let mut vitality = Vitality::new();
    let mut outcome = MotionOutcome {
        maintenance: None,
        movement: None,
        velocity: organism.velocity,
        position: organism.position,
        remaining: 0.0,
        exhausted: false,
        died: None,
    };
    let finish = |mut outcome: MotionOutcome, budget: &EnergyBudget, vitality: &Vitality| {
        outcome.remaining = budget.remaining();
        outcome.exhausted = budget.is_exhausted();
        outcome.died = vitality.died_at().map(|stage| match stage {
            BudgetStage::Chemistry => ExhaustedAt::Chemistry,
            BudgetStage::Maintenance => ExhaustedAt::Maintenance,
            _ => ExhaustedAt::Motion,
        });
        Ok(outcome)
    };
    if !vitality.check(BudgetStage::Chemistry, &budget)? {
        return finish(outcome, &budget, &vitality);
    }
    let maintenance = rules
        .costs
        .bill(
            rules.step,
            organism.sensing_scale,
            organism.parameters,
            organism.evaluated,
        )?
        .settle(&mut budget)?;
    outcome.maintenance = Some(MaintenanceOutcome {
        requested: maintenance.cost.requested,
        paid: maintenance.cost.paid,
        basal: maintenance.paid.basal,
        sensor: maintenance.paid.sensor,
        controller: maintenance.paid.controller,
    });
    if !vitality.check(BudgetStage::Maintenance, &budget)? {
        return finish(outcome, &budget, &vitality);
    }
    let ratio = uptake_ratio(organism.absorbed, rules.step, rules.capacity)?;
    let response = MotionResponse::new(organism.response, &rules.motion)?;
    let acceleration = rules
        .motion
        .acceleration(ratio, &response, &organism.gradients)?;
    let proposed = rules.motion.propose(organism.velocity, acceleration)?;
    let moved = rules.motion.settle(proposed, &mut budget)?;
    outcome.movement = Some(MovementOutcome {
        proposed: moved.proposed,
        velocity: moved.velocity,
        requested: moved.cost.requested,
        paid: moved.cost.paid,
        scaled: moved.scaled,
    });
    outcome.velocity = moved.velocity;
    outcome.position = rules
        .motion
        .displace(&rules.grid, organism.position, moved.velocity)?;
    vitality.check(BudgetStage::Motion, &budget)?;
    finish(outcome, &budget, &vitality)
}

/// A random organism: budgets from zero to plenty, scales and parameter
/// counts over many magnitudes, responses up to the norm limit, gradients
/// from subnormal to saturating, velocities past the speed limit, and
/// positions at and near the torus's edges.
fn organism(rules: &Rules, rng: &mut Rng) -> MotionOrganism {
    let c = &rules.constants;
    let evaluated = rng.next().is_multiple_of(2);
    let parameters = match rng.next() % 6 {
        0 => 0,
        1 => u32::MAX,
        _ => (rng.next() % 200_000) as u32,
    };
    let sensing_scale = match rng.next() % 8 {
        0 => f64::from_bits(rng.next() % 0x000F_FFFF_FFFF_FFFF + 1),
        1 => rng.unit().mul_add(1.0e3, 1.0e-3),
        _ => rng.unit().mul_add(7.5, 0.5),
    };
    // Roughly what maintenance bills and the most motion costs, so that
    // some budgets run out at each stage.
    let bill = c.step_seconds * (c.basal + c.sensor * (sensing_scale / c.sensor_unit_scale))
        + if evaluated {
            c.neural * f64::from(parameters)
        } else {
            0.0
        };
    let motion = c.cost * c.maximum_speed * c.maximum_speed * c.step_seconds;
    let remaining = match rng.next() % 12 {
        0 => 0.0,
        1 => f64::from_bits(rng.next() % 0x000F_FFFF_FFFF_FFFF + 1),
        2 => rng.unit() * 0.05,
        3 => rng.unit() * 1.0e-6,
        4 => rng.unit() * 1.0e4,
        5 | 6 => rng.unit().mul_add(motion, bill),
        _ => rng.unit() * 2.0,
    } + 0.0; // A budget's balance is never -0 (`EnergyBudget` adds +0).
    let absorbed = match rng.next() % 8 {
        0 => 0.0,
        1 => f64::from_bits(rng.next() % 0x000F_FFFF_FFFF_FFFF + 1),
        2 => rng.unit() * 1.0e3,
        _ => rng.unit() * c.step_seconds * c.uptake_capacity.max(1.0),
    };
    let speed = c.maximum_speed * 2.0 * rng.unit();
    let angle = rng.signed() * std::f64::consts::PI;
    let velocity = match rng.next() % 10 {
        0 => [0.0, -0.0],
        1 => [c.maximum_speed, 0.0],
        _ => [speed * angle.cos(), speed * angle.sin()],
    };
    let coordinate = |rng: &mut Rng, length: f64| match rng.next() % 8 {
        0 => 0.0,
        1 => f64::from_bits(length.to_bits() - 1),
        2 => length * rng.unit() * 1.0e-3,
        3 => length * (1.0 - rng.unit() * 1.0e-3),
        _ => length * rng.unit(),
    };
    let position = [
        coordinate(rng, c.world_width),
        coordinate(rng, c.world_height),
    ];
    let mut response = [0.0; MOTION_TERMS];
    for coefficient in &mut response {
        *coefficient = if rng.next().is_multiple_of(4) {
            0.0
        } else {
            rng.signed()
        };
    }
    let fraction = match rng.next() % 6 {
        0 => 0.0,
        1 => 1.0,
        _ => rng.unit(),
    };
    let norm = response.iter().map(|c| c * c).sum::<f64>().sqrt();
    if norm > 0.0 {
        for coefficient in &mut response {
            *coefficient *= fraction * c.response_norm_limit / norm;
        }
    }
    let magnitude = match rng.next() % 6 {
        0 => 0.0,
        1 => 1.0e-310,
        2 => 1.0e3,
        _ => 1.0e-2,
    };
    let mut gradients = [[0.0; 2]; MOTION_TERMS];
    for gradient in &mut gradients {
        *gradient = [rng.signed() * magnitude, rng.signed() * magnitude];
    }
    MotionOrganism {
        remaining,
        exhausted: remaining == 0.0,
        evaluated,
        parameters,
        sensing_scale,
        absorbed,
        velocity,
        position,
        response,
        gradients,
    }
}

fn assert_same_outcomes(device: &[MotionOutcome], reference: &[MotionOutcome]) {
    assert_eq!(device.len(), reference.len());
    for (index, (got, want)) in device.iter().zip(reference).enumerate() {
        assert_eq!(
            got.died, want.died,
            "organism {index}: the stage it died at"
        );
        assert_eq!(
            got.maintenance.is_some(),
            want.maintenance.is_some(),
            "organism {index}: whether maintenance ran"
        );
        assert_eq!(
            got.movement.is_some(),
            want.movement.is_some(),
            "organism {index}: whether motion ran"
        );
        let mut fields = vec![
            ("velocity x", got.velocity[0], want.velocity[0]),
            ("velocity y", got.velocity[1], want.velocity[1]),
            ("position x", got.position[0], want.position[0]),
            ("position y", got.position[1], want.position[1]),
            ("remaining", got.remaining, want.remaining),
        ];
        if let (Some(g), Some(w)) = (got.maintenance, want.maintenance) {
            fields.extend([
                ("maintenance requested", g.requested, w.requested),
                ("maintenance paid", g.paid, w.paid),
                ("basal paid", g.basal, w.basal),
                ("sensor paid", g.sensor, w.sensor),
                ("controller paid", g.controller, w.controller),
            ]);
        }
        if let (Some(g), Some(w)) = (got.movement, want.movement) {
            fields.extend([
                ("proposed x", g.proposed[0], w.proposed[0]),
                ("proposed y", g.proposed[1], w.proposed[1]),
                ("moved x", g.velocity[0], w.velocity[0]),
                ("moved y", g.velocity[1], w.velocity[1]),
                ("motion requested", g.requested, w.requested),
                ("motion paid", g.paid, w.paid),
            ]);
            assert_eq!(g.scaled, w.scaled, "organism {index}: scaled");
        }
        for (name, got_value, want_value) in fields {
            assert!(
                got_value.to_bits() == want_value.to_bits(),
                "organism {index}: {name} {got_value:e} on the device, {want_value:e} on the reference"
            );
        }
        assert_eq!(
            got.exhausted, want.exhausted,
            "organism {index}: exhaustion"
        );
    }
}

/// Whether `v` is past `limit` and its first radial scaling still is, so
/// that clamp_radius's shrink loop runs (coverage only).
fn shrinks(v: [f64; 2], limit: f64) -> bool {
    let squared = v[0] * v[0] + v[1] * v[1];
    let bound = limit * limit;
    if squared <= bound {
        return false;
    }
    let scale = limit / squared.sqrt();
    let clamped = [v[0] * scale, v[1] * scale];
    clamped[0] * clamped[0] + clamped[1] * clamped[1] > bound
}

/// Whether the acceleration clamp and the speed clamp each run their
/// shrink loop for `organism`, which motion settled (coverage only: the
/// unclamped values are recomputed as the reference forms them).
#[expect(clippy::expect_used, reason = "coverage helper")]
fn shrink_loops(rules: &Rules, organism: &MotionOrganism) -> [bool; 2] {
    let c = &rules.constants;
    let ratio = uptake_ratio(organism.absorbed, rules.step, rules.capacity).expect("a ratio");
    let mut sum = [0.0, 0.0];
    for (coefficient, gradient) in organism.response.iter().zip(&organism.gradients) {
        sum[0] += coefficient * gradient[0];
        sum[1] += coefficient * gradient[1];
    }
    let response = MotionResponse::new(organism.response, &rules.motion).expect("a response");
    let acceleration = rules
        .motion
        .acceleration(ratio, &response, &organism.gradients)
        .expect("an acceleration");
    let free = [
        c.decay * organism.velocity[0] + c.force_coefficient * acceleration[0],
        c.decay * organism.velocity[1] + c.force_coefficient * acceleration[1],
    ];
    [
        shrinks([ratio * sum[0], ratio * sum[1]], c.maximum_acceleration),
        shrinks(free, c.maximum_speed),
    ]
}

/// Thousands of organisms under the preset's constants and under extreme
/// ones: every settlement, velocity, position and budget is the
/// reference's, bit for bit, and the draws cover the cases that matter.
#[test]
fn motion_on_the_device_is_the_references() -> Result<(), CoreError> {
    let Some(mut device) = metal() else {
        return Ok(());
    };
    let mut rng = Rng(0x0C4E_3157_0000_0007);
    let sets = [
        preset_rules(),
        // Expensive, fast and cramped: maintenance and motion exhaust many
        // budgets, no drag, a small torus crossed every tick.
        rules(Numbers {
            step: (1, 3),
            basal: 1.5,
            sensor: 0.05,
            sensor_unit_scale: 0.75,
            neural: 3.0e-6,
            drag: 0.0,
            maximum_speed: 3.0,
            maximum_acceleration: 7.0,
            cost: 0.4,
            response_norm_limit: 12.0,
            capacity: 0.5,
            world: (3, 1, 3.7, 1.0),
        }),
        // Free and viscous: every cost a negative zero, no uptake capacity
        // (a zero acceleration), drag that underflows the decay, a wide
        // torus. Nothing dies past chemistry and nothing moves far.
        rules(Numbers {
            step: (1, 60),
            basal: -0.0,
            sensor: -0.0,
            sensor_unit_scale: 2.0,
            neural: -0.0,
            drag: 1.0e5,
            maximum_speed: 0.5,
            maximum_acceleration: 1.0e-3,
            cost: -0.0,
            response_norm_limit: 1.0,
            capacity: 0.0,
            world: (512, 512, 3.0e5, 2.5e5),
        }),
    ];
    let mut shrunk_total = [0_usize; 2];
    for (set, rules) in sets.iter().enumerate() {
        let mut organisms = Vec::new();
        let mut want = Vec::new();
        let mut refused = 0;
        while organisms.len() < 4_096 {
            let organism = organism(rules, &mut rng);
            match reference(rules, &organism) {
                Ok(outcome) => {
                    organisms.push(organism);
                    want.push(outcome);
                }
                Err(_) => refused += 1,
            }
        }
        let mut got = Vec::new();
        device.settle(&rules.constants, &organisms, &mut got)?;
        assert_same_outcomes(&got, &want);

        // Coverage.
        let died = |stage| want.iter().filter(|o| o.died == Some(stage)).count();
        let survived = want.iter().filter(|o| o.died.is_none()).count();
        let moved: Vec<(&MotionOrganism, MovementOutcome)> = organisms
            .iter()
            .zip(&want)
            .filter_map(|(organism, o)| o.movement.map(|m| (organism, m)))
            .collect();
        let scaled = moved.iter().filter(|(_, m)| m.scaled).count();
        let speed = rules.constants.maximum_speed;
        let at_limit = moved
            .iter()
            .filter(|(_, m)| {
                let s = m.proposed[0] * m.proposed[0] + m.proposed[1] * m.proposed[1];
                s > speed * speed * (1.0 - 1.0e-9)
            })
            .count();
        let mut shrunk = [0_usize; 2];
        for (organism, _) in &moved {
            for (count, ran) in shrunk.iter_mut().zip(shrink_loops(rules, organism)) {
                *count += usize::from(ran);
            }
        }
        shrunk_total[0] += shrunk[0];
        shrunk_total[1] += shrunk[1];
        let crossed = moved
            .iter()
            .filter(|(organism, m)| {
                let c = &rules.constants;
                (0..2).any(|axis| {
                    let length = [c.world_width, c.world_height][axis];
                    let x = organism.position[axis] + c.step_seconds * m.velocity[axis];
                    !(0.0..length).contains(&x)
                })
            })
            .count();
        assert!(
            died(ExhaustedAt::Chemistry) > 100,
            "set {set}: {} died at chemistry",
            died(ExhaustedAt::Chemistry)
        );
        eprintln!(
            "set {set}: died {}/{}/{}, survived {survived}, scaled {scaled}, at the limit \
             {at_limit}, shrink loops {shrunk:?}, crossed {crossed}, refused {refused}",
            died(ExhaustedAt::Chemistry),
            died(ExhaustedAt::Maintenance),
            died(ExhaustedAt::Motion),
        );
        assert!(survived > 500, "set {set}: {survived} survived");
        if set != 2 {
            assert!(
                died(ExhaustedAt::Maintenance) > 100 && died(ExhaustedAt::Motion) > 20,
                "set {set}: {} died at maintenance, {} at motion",
                died(ExhaustedAt::Maintenance),
                died(ExhaustedAt::Motion)
            );
            assert!(scaled > 20, "set {set}: {scaled} scaled");
            assert!(at_limit > 100, "set {set}: {at_limit} at the speed limit");
            assert!(crossed > 20, "set {set}: {crossed} crossed an edge");
        }
        // Refusals while drawing (responses rounded past the limit) are
        // the refusal test's.
        assert!(refused < 4_096, "set {set}: {refused} refused");
    }
    // Both clamps' shrink loops ran somewhere.
    assert!(
        shrunk_total[0] > 10 && shrunk_total[1] > 10,
        "shrink loops {shrunk_total:?}"
    );
    Ok(())
}

/// An alive organism the preset's rules settle.
fn plain(rules: &Rules) -> MotionOrganism {
    let mut response = [0.0; MOTION_TERMS];
    response[0] = 1.0;
    response[5] = -0.5;
    let mut gradients = [[0.0; 2]; MOTION_TERMS];
    gradients[0] = [0.25, -0.125];
    gradients[5] = [0.5, 0.75];
    MotionOrganism {
        remaining: 2.0,
        exhausted: false,
        evaluated: true,
        parameters: 9_000,
        sensing_scale: 2.0,
        absorbed: 0.05,
        velocity: [0.5, -0.25],
        position: [
            rules.constants.world_width * 0.25,
            rules.constants.world_height * 0.75,
        ],
        response,
        gradients,
    }
}

fn refused_by_device(result: &Result<(), CoreError>) -> bool {
    matches!(
        result,
        Err(CoreError::StateConflict {
            operation: "metal motion",
            ..
        })
    )
}

/// The device refuses the call wherever the reference refuses an organism,
/// one category at a time, and where it cannot carry a value the reference
/// settles; it does not check what the reference never reads.
#[test]
#[expect(clippy::too_many_lines, reason = "one case per category")]
fn motion_the_reference_refuses_is_refused() -> Result<(), CoreError> {
    let Some(mut device) = metal() else {
        return Ok(());
    };
    let rules = preset_rules();
    let valid = plain(&rules);
    assert!(reference(&rules, &valid).is_ok());
    let edit = |change: &dyn Fn(&mut MotionOrganism)| {
        let mut organism = valid;
        change(&mut organism);
        organism
    };
    let limit = rules.constants.response_norm_limit;
    let cases: Vec<(&str, MotionOrganism)> = vec![
        ("a zero sensing scale", edit(&|o| o.sensing_scale = 0.0)),
        (
            "a negative sensing scale",
            edit(&|o| o.sensing_scale = -1.0),
        ),
        ("a NaN sensing scale", edit(&|o| o.sensing_scale = f64::NAN)),
        (
            "an infinite sensing scale",
            edit(&|o| o.sensing_scale = f64::INFINITY),
        ),
        (
            "a negative absorbed uptake",
            edit(&|o| o.absorbed = -1.0e-300),
        ),
        ("a NaN absorbed uptake", edit(&|o| o.absorbed = f64::NAN)),
        ("a NaN response", edit(&|o| o.response[32] = f64::NAN)),
        (
            "a response past the limit",
            edit(&|o| o.response[1] = limit * 1.000_000_1),
        ),
        (
            "an infinite gradient",
            edit(&|o| o.gradients[17][1] = f64::NEG_INFINITY),
        ),
        ("a NaN velocity", edit(&|o| o.velocity[0] = f64::NAN)),
        (
            "an infinite position",
            edit(&|o| o.position[1] = f64::INFINITY),
        ),
    ];
    for (name, organism) in &cases {
        assert!(
            reference(&rules, organism).is_err(),
            "{name}: the reference should refuse this"
        );
        let mut out = vec![MotionOutcome {
            maintenance: None,
            movement: None,
            velocity: [1.0; 2],
            position: [1.0; 2],
            remaining: 1.0,
            exhausted: false,
            died: None,
        }];
        let result = device.settle(&rules.constants, &[valid, *organism], &mut out);
        assert!(
            refused_by_device(&result),
            "{name}: the device gave {result:?}"
        );
        assert!(out.is_empty(), "{name}: outcomes were left");
    }

    // A sensor share that overflows, under a sensor cost large enough for
    // one.
    let mut numbers = Numbers {
        step: (1, 30),
        basal: 0.8,
        sensor: 4.0,
        sensor_unit_scale: 1.0,
        neural: 2.0e-9,
        drag: 2.0,
        maximum_speed: rules.constants.maximum_speed,
        maximum_acceleration: rules.constants.maximum_acceleration,
        cost: rules.constants.cost,
        response_norm_limit: limit,
        capacity: rules.constants.uptake_capacity,
        world: (128, 128, 128.0, 128.0),
    };
    let sensitive = self::rules(numbers);
    let overflowing = edit(&|o| o.sensing_scale = f64::MAX);
    assert!(reference(&sensitive, &overflowing).is_err());
    assert!(reference(&sensitive, &valid).is_ok());
    let result = device.settle(&sensitive.constants, &[valid, overflowing], &mut Vec::new());
    assert!(
        refused_by_device(&result),
        "a sensor share that overflows: {result:?}"
    );
    // Under the preset's sensor cost the same scale bills a huge, finite
    // share, which the device settles as the reference does.
    numbers.sensor = 0.001;
    let tame = self::rules(numbers);
    let mut out = Vec::new();
    device.settle(&tame.constants, &[overflowing], &mut out)?;
    assert_same_outcomes(&out, &[reference(&tame, &overflowing)?]);

    // Values the reference settles and the device cannot carry: a velocity
    // whose square overflows (the reference clamps it to zero) and a motion
    // cost that overflows (the reference scales the motion to nothing).
    let huge = edit(&|o| o.velocity = [1.0e200, 1.0e200]);
    assert!(reference(&rules, &huge).is_ok());
    let result = device.settle(&rules.constants, &[huge], &mut Vec::new());
    assert!(refused_by_device(&result), "a huge velocity: {result:?}");
    let costly = MotionConstants {
        cost: f64::MAX,
        ..rules.constants
    };
    let costly_rules = Rules {
        motion: Motion::new(
            rules.step,
            2.0,
            rules.constants.maximum_speed,
            rules.constants.maximum_acceleration,
            f64::MAX,
            limit,
        )?,
        constants: costly,
        ..preset_rules()
    };
    // A proposal below unit speed costs a huge, finite amount, which the
    // device settles as the reference does; one above it overflows.
    let mut out = Vec::new();
    device.settle(&costly, &[valid], &mut out)?;
    assert_same_outcomes(&out, &[reference(&costly_rules, &valid)?]);
    assert!(
        out[0]
            .movement
            .is_some_and(|m| m.scaled && m.requested.is_finite())
    );
    let fast = edit(&|o| o.velocity = [2.0, 2.0]);
    let settled = reference(&costly_rules, &fast)?;
    assert!(
        settled
            .movement
            .is_some_and(|m| m.scaled && m.requested.is_infinite())
    );
    let result = device.settle(&costly, &[fast], &mut Vec::new());
    assert!(
        refused_by_device(&result),
        "an overflowing cost: {result:?}"
    );

    // What the reference never reads is not checked: an organism exhausted
    // by chemistry with nothing else valid, and one that maintenance
    // exhausts with an invalid response and gradients.
    let dead = MotionOrganism {
        remaining: 0.0,
        exhausted: true,
        sensing_scale: f64::NAN,
        absorbed: f64::NAN,
        velocity: [f64::NAN; 2],
        position: [f64::INFINITY; 2],
        response: [f64::NAN; MOTION_TERMS],
        gradients: [[f64::NAN; 2]; MOTION_TERMS],
        ..valid
    };
    let starved = MotionOrganism {
        remaining: 1.0e-12,
        absorbed: f64::NAN,
        response: [f64::NAN; MOTION_TERMS],
        gradients: [[f64::INFINITY; 2]; MOTION_TERMS],
        ..valid
    };
    let mut out = Vec::new();
    device.settle(&rules.constants, &[dead, starved, valid], &mut out)?;
    let want = [
        reference(&rules, &dead)?,
        reference(&rules, &starved)?,
        reference(&rules, &valid)?,
    ];
    assert_eq!(want[0].died, Some(ExhaustedAt::Chemistry));
    assert_eq!(want[1].died, Some(ExhaustedAt::Maintenance));
    assert_eq!(want[2].died, None);
    // NaN inputs pass through untouched: compare everything but those.
    assert_eq!(out[0].died, want[0].died);
    assert!(out[0].maintenance.is_none() && out[0].movement.is_none());
    assert!(out[0].remaining.to_bits() == 0 && out[0].exhausted);
    assert_same_outcomes(&out[1..], &want[1..]);

    // Constants the reference rejects are refused before anything runs.
    for bad in [
        MotionConstants {
            maximum_speed: 0.0,
            ..rules.constants
        },
        MotionConstants {
            world_width: f64::INFINITY,
            ..rules.constants
        },
        MotionConstants {
            decay: 1.5,
            ..rules.constants
        },
    ] {
        let result = device.settle(&bad, &[], &mut Vec::new());
        assert!(
            matches!(result, Err(CoreError::NumericalInvalid { .. })),
            "{result:?}"
        );
    }
    Ok(())
}
