//! FL-032, third stage: phase 6's emission on Metal is flife-engine's, bit
//! for bit. The device's new arithmetic (a general floor, its integer, and
//! the deposit split's sum rounding up) is checked against Rust's and
//! flife-core's on a quarter million operands. The kernel is checked
//! against `EmissionPolicy::emit` and what it staged in flife-field's
//! `Deposits` (each entry's payment and offer, and each packet's cell and
//! amount in order) on thousands of organisms: grids from 1 × 1 (every
//! corner one cell) to the preset's, positions on cell centres and edges,
//! fractions of zero and one, zero and full transfer efficiency, and
//! budgets that emission exhausts. Every input the reference refuses is
//! refused, and what it never reads is not checked.

use flife_backend_metal::{
    EMISSION_OPS_WORDS, EmissionAction, EmissionOrganism, EmissionOutcome, MetalEmission,
};
use flife_core::ids::OrganismId;
use flife_core::ledger::{EnergyBudget, add_down};
use flife_core::{CoreError, ValidatedConfig};
use flife_engine::EmissionPolicy;
use flife_field::state::CHANNELS;
use flife_field::{Deposits, FieldGrid};
use flife_neural::Emission;

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

/// `None` off macOS, where `open` must refuse with its typed error.
#[expect(clippy::panic, reason = "test helper")]
fn metal() -> Option<MetalEmission> {
    match MetalEmission::open() {
        Ok(emission) => Some(emission),
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

    /// A binary64 pattern: full random bits, values near integers and
    /// half-integers of either sign, small and huge magnitudes, subnormals
    /// and zeros.
    fn pattern(&mut self) -> u64 {
        let bits = self.next();
        match bits % 8 {
            0 => bits & 0x8000_0000_0000_0000,
            1 => bits & 0x800F_FFFF_FFFF_FFFF,
            2 => (self.signed() * 3.0e3).to_bits(),
            3 => {
                let whole = (self.next() % 4_096) as f64 - 2_048.0;
                let offset = [0.0, 0.5, -0.5, 1.0e-12, -1.0e-12][(self.next() % 5) as usize];
                (whole + offset).to_bits()
            }
            4 => (self.signed() * 1.0e18).to_bits(),
            _ => bits,
        }
    }
}

fn same(device: f64, reference: f64) -> bool {
    device.to_bits() == reference.to_bits() || (device.is_nan() && reference.is_nan())
}

fn pair_value(row: &[u32; EMISSION_OPS_WORDS], k: usize) -> u64 {
    u64::from(row[2 * k]) | (u64::from(row[2 * k + 1]) << 32)
}

/// The floor, its integer and the upward sum are Rust's and flife-core's on
/// every operand, including the signs of zeros and NaN floors.
#[test]
fn the_emission_arithmetic_is_the_references() -> Result<(), CoreError> {
    let Some(emission) = metal() else {
        return Ok(());
    };
    let mut rng = Rng(0x0C4E_3157_0000_00E1);
    let mut operands = Vec::new();
    while operands.len() < 1 << 18 {
        operands.push(rng.pattern());
    }
    for edge in [
        0.0_f64,
        -0.0,
        0.5,
        -0.5,
        -1.0e-20,
        1.0e-20,
        -1.0,
        -1.5,
        -2.5,
        f64::from_bits(1),
        -f64::from_bits(1),
        4_503_599_627_370_495.5,
        -4_503_599_627_370_495.5,
        4_503_599_627_370_496.0,
        -4_611_686_018_427_387_904.0,
        f64::MAX,
        f64::MIN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NAN,
    ] {
        operands.push(edge.to_bits());
    }
    while !operands.len().is_multiple_of(64) {
        operands.push(0);
    }
    let mut rows = Vec::new();
    emission.emission_ops(&operands, &mut rows)?;
    assert_eq!(rows.len(), operands.len());
    let mut counted = [0_usize; 3];
    for (index, (&bits, row)) in operands.iter().zip(&rows).enumerate() {
        let a = f64::from_bits(bits);
        let (got, want) = (f64::from_bits(pair_value(row, 0)), a.floor());
        assert!(
            same(got, want),
            "operand {index}: floor({a:e}) gave {got:e}, not {want:e}"
        );
        counted[0] += usize::from(a.is_finite() && a.fract() != 0.0);
        if a.is_finite() && want.abs() < 4.0e18 {
            let got = pair_value(row, 1) as i64;
            assert_eq!(got, want as i64, "operand {index}: {want:e} as i64");
            counted[1] += 1;
        }
        let third = a / 3.0;
        let want = -add_down(-a, -third);
        if want.is_finite() && (a + third).is_finite() {
            let got = f64::from_bits(pair_value(row, 2));
            assert!(
                same(got, want),
                "operand {index}: add_up({a:e}, {third:e}) gave {got:e}, not {want:e}"
            );
            counted[2] += 1;
        }
    }
    assert!(counted[0] > 50_000, "{} fractional floors", counted[0]);
    assert!(counted[1] > 100_000, "{} integers", counted[1]);
    assert!(counted[2] > 200_000, "{} upward sums", counted[2]);
    Ok(())
}

/// One organism's inputs, and what the reference made of them.
struct Case {
    organism: EmissionOrganism,
    outcome: EmissionOutcome,
}

/// The reference's emission for `organism`: `EmissionPolicy::emit` on a
/// fresh budget, and the deposit it staged, if any.
fn reference(
    policy: &EmissionPolicy,
    grid: &FieldGrid,
    organism: &EmissionOrganism,
) -> Result<EmissionOutcome, CoreError> {
    let mut budget = EnergyBudget::new(organism.remaining)?;
    assert!(!budget.is_exhausted(), "an organism that reaches emission");
    let mut deposits = Deposits::new();
    let emission = organism.emission.map(|action| Emission {
        direction: action.direction,
        spend_fraction: action.spend_fraction,
    });
    let settlement = policy.emit(
        grid,
        OrganismId::new(7),
        organism.position,
        emission.as_ref(),
        &mut budget,
        &mut deposits,
    )?;
    let mut packets = [(0, 0.0); 4];
    let mut packet_count = 0;
    let staged = deposits.entry(0).is_some();
    if let Some((_, paid, offered)) = deposits.entry(0) {
        // The entry holds what the settlement reports.
        assert!(same(paid, settlement.cost.requested) && same(offered, settlement.offered));
        for (slot, packet) in packets.iter_mut().zip(deposits.entry_packets(0)) {
            *slot = packet;
            packet_count += 1;
        }
        assert_eq!(deposits.entry_packets(0).count(), packet_count);
    }
    Ok(EmissionOutcome {
        available: settlement.available,
        requested: settlement.cost.requested,
        paid: settlement.cost.paid,
        offered: settlement.offered,
        staged,
        packets,
        packet_count,
        remaining: budget.remaining(),
        exhausted: budget.is_exhausted(),
    })
}

/// A random organism that reaches emission on `grid`.
fn organism(grid: &FieldGrid, rng: &mut Rng) -> EmissionOrganism {
    let remaining = match rng.next() % 8 {
        0 => f64::from_bits(rng.next() % 0x000F_FFFF_FFFF_FFFF + 1),
        1 => rng.unit() * 1.0e-9 + f64::MIN_POSITIVE,
        2 => rng.unit() * 1.0e4 + 1.0,
        _ => rng.unit() * 2.0 + 1.0e-3,
    };
    let coordinate = |rng: &mut Rng, length: f64, cells: u32| {
        let cell = length / f64::from(cells);
        match rng.next() % 8 {
            0 => 0.0,
            1 => f64::from_bits(length.to_bits() - 1),
            // A cell centre: one weight in each direction is zero.
            2 => (f64::from((rng.next() % u64::from(cells)) as u32) + 0.5) * cell,
            // A cell edge.
            3 => f64::from((rng.next() % u64::from(cells)) as u32) * cell,
            _ => length * rng.unit(),
        }
    };
    let position = [
        coordinate(rng, grid.world_width(), grid.cells_x()),
        coordinate(rng, grid.world_height(), grid.cells_y()),
    ];
    let emission = (!rng.next().is_multiple_of(8)).then(|| {
        let spend_fraction = match rng.next() % 8 {
            0 => 0.0,
            1 => 1.0,
            2 => f64::from_bits(rng.next() % 0x000F_FFFF_FFFF_FFFF + 1),
            _ => rng.unit(),
        };
        let mut direction = [0.0; CHANNELS];
        for component in &mut direction {
            *component = if rng.next().is_multiple_of(4) {
                0.0
            } else {
                rng.signed()
            };
        }
        let fraction = match rng.next() % 4 {
            0 => 1.0,
            _ => rng.unit(),
        };
        let norm = direction.iter().map(|c| c * c).sum::<f64>().sqrt();
        if norm > 0.0 {
            for component in &mut direction {
                *component *= fraction / norm;
            }
        }
        EmissionAction {
            direction,
            spend_fraction,
        }
    });
    EmissionOrganism {
        remaining,
        position,
        emission,
    }
}

fn assert_same_outcomes(device: &[EmissionOutcome], reference: &[EmissionOutcome]) {
    assert_eq!(device.len(), reference.len());
    for (index, (got, want)) in device.iter().zip(reference).enumerate() {
        for (name, got_value, want_value) in [
            ("available", got.available, want.available),
            ("requested", got.requested, want.requested),
            ("paid", got.paid, want.paid),
            ("offered", got.offered, want.offered),
            ("remaining", got.remaining, want.remaining),
        ] {
            assert!(
                got_value.to_bits() == want_value.to_bits(),
                "organism {index}: {name} {got_value:e} on the device, {want_value:e} on the reference"
            );
        }
        assert_eq!(got.staged, want.staged, "organism {index}: staged");
        assert_eq!(got.exhausted, want.exhausted, "organism {index}: exhausted");
        assert_eq!(
            got.packets().len(),
            want.packets().len(),
            "organism {index}: packets {:?} on the device, {:?} on the reference",
            got.packets(),
            want.packets()
        );
        for (k, (g, w)) in got.packets().iter().zip(want.packets()).enumerate() {
            assert!(
                g.0 == w.0 && g.1.to_bits() == w.1.to_bits(),
                "organism {index}: packet {k} {g:?} on the device, {w:?} on the reference"
            );
        }
    }
}

/// Thousands of organisms on four grids under three efficiencies: every
/// settlement, packet and budget is the reference's, bit for bit, and the
/// draws cover the cases that matter.
#[test]
fn emission_on_the_device_is_the_references() -> Result<(), CoreError> {
    let Some(mut device) = metal() else {
        return Ok(());
    };
    let config = ValidatedConfig::from_toml(PRESET).expect("the preset validates");
    let preset = EmissionPolicy::from_config(&config)?;
    let grids = [
        FieldGrid::from_config(&config)?,
        FieldGrid::new(1, 1, 3.5, 2.25)?,
        FieldGrid::new(1, 7, 1.0, 9.1)?,
        FieldGrid::new(3, 2, 100.5, 0.75)?,
    ];
    let mut rng = Rng(0x0C4E_3157_0000_00E2);
    let mut totals = [0_usize; 5];
    for (g, grid) in grids.iter().enumerate() {
        for policy in [preset, EmissionPolicy::new(1.0)?, EmissionPolicy::new(0.0)?] {
            let mut cases = Vec::new();
            let mut refused = 0;
            while cases.len() < 2_048 {
                let organism = organism(grid, &mut rng);
                match reference(&policy, grid, &organism) {
                    Ok(outcome) => cases.push(Case { organism, outcome }),
                    Err(_) => refused += 1,
                }
            }
            let organisms: Vec<EmissionOrganism> = cases.iter().map(|c| c.organism).collect();
            let want: Vec<EmissionOutcome> = cases.iter().map(|c| c.outcome).collect();
            let mut got = Vec::new();
            device.settle(grid, policy.efficiency(), &organisms, &mut got)?;
            assert_same_outcomes(&got, &want);
            assert!(refused < 2_048, "grid {g}: {refused} refused");
            totals[0] += want.iter().filter(|o| o.exhausted).count();
            totals[1] += want.iter().filter(|o| o.staged).count();
            totals[2] += want
                .iter()
                .filter(|o| o.staged && o.packet_count == 0)
                .count();
            totals[3] += want.iter().filter(|o| o.packet_count == 4).count();
            totals[4] += want.iter().filter(|o| o.packet_count == 1).count();
        }
    }
    eprintln!(
        "died at emission {}, staged {}, staged without packets {}, four packets {}, one packet {}",
        totals[0], totals[1], totals[2], totals[3], totals[4]
    );
    assert!(totals[0] > 200, "{} died at emission", totals[0]);
    assert!(totals[1] > 10_000, "{} staged", totals[1]);
    assert!(totals[2] > 500, "{} staged without packets", totals[2]);
    assert!(totals[3] > 1_000, "{} with four packets", totals[3]);
    assert!(totals[4] > 1_000, "{} with one packet", totals[4]);
    Ok(())
}

/// The device refuses the call wherever the reference refuses an organism,
/// one category at a time; it does not check what the reference never
/// reads; and the host refuses an organism that does not reach emission.
#[test]
fn emission_the_reference_refuses_is_refused() -> Result<(), CoreError> {
    let Some(mut device) = metal() else {
        return Ok(());
    };
    let config = ValidatedConfig::from_toml(PRESET).expect("the preset validates");
    let policy = EmissionPolicy::from_config(&config)?;
    let grid = FieldGrid::from_config(&config)?;
    let mut direction = [0.0; CHANNELS];
    direction[3] = 0.6;
    direction[9] = -0.7;
    let valid = EmissionOrganism {
        remaining: 1.5,
        position: [10.25, 99.75],
        emission: Some(EmissionAction {
            direction,
            spend_fraction: 0.125,
        }),
    };
    assert!(reference(&policy, &grid, &valid)?.staged);
    let edit = |change: &dyn Fn(&mut EmissionOrganism, &mut EmissionAction)| {
        let mut organism = valid;
        let mut emission = valid.emission.expect("an emission");
        change(&mut organism, &mut emission);
        organism.emission = Some(emission);
        organism
    };
    let cases: Vec<(&str, EmissionOrganism)> = vec![
        ("a NaN fraction", edit(&|_, e| e.spend_fraction = f64::NAN)),
        (
            "a negative fraction",
            edit(&|_, e| e.spend_fraction = -0.25),
        ),
        ("a fraction above one", edit(&|_, e| e.spend_fraction = 1.5)),
        (
            "a direction past the unit ball",
            edit(&|_, e| e.direction[0] = 0.5),
        ),
        ("a NaN direction", edit(&|_, e| e.direction[31] = f64::NAN)),
        ("a NaN position", edit(&|o, _| o.position[0] = f64::NAN)),
        (
            "an infinite position",
            edit(&|o, _| o.position[1] = f64::INFINITY),
        ),
    ];
    for (name, organism) in &cases {
        assert!(
            reference(&policy, &grid, organism).is_err(),
            "{name}: the reference should refuse this"
        );
        let mut out = vec![reference(&policy, &grid, &valid)?];
        let result = device.settle(&grid, policy.efficiency(), &[valid, *organism], &mut out);
        assert!(
            matches!(
                result,
                Err(CoreError::StateConflict {
                    operation: "metal emission",
                    ..
                })
            ),
            "{name}: the device gave {result:?}"
        );
        assert!(out.is_empty(), "{name}: outcomes were left");
    }

    // Not read, so not checked: the position and direction of an organism
    // that does not emit, or whose request is zero.
    let silent = EmissionOrganism {
        position: [f64::NAN; 2],
        emission: None,
        ..valid
    };
    let idle = edit(&|o, e| {
        e.spend_fraction = 0.0;
        e.direction = [f64::NAN; CHANNELS];
        o.position = [f64::INFINITY; 2];
    });
    let mut out = Vec::new();
    device.settle(&grid, policy.efficiency(), &[silent, idle, valid], &mut out)?;
    assert_same_outcomes(
        &out,
        &[
            reference(&policy, &grid, &silent)?,
            reference(&policy, &grid, &idle)?,
            reference(&policy, &grid, &valid)?,
        ],
    );
    assert!(!out[0].staged && !out[1].staged && out[2].staged);

    // Refused before any command: an organism that does not reach emission,
    // and an efficiency outside [0, 1].
    for remaining in [0.0, -1.0, f64::NAN] {
        let dead = EmissionOrganism { remaining, ..valid };
        let result = device.settle(&grid, policy.efficiency(), &[dead], &mut Vec::new());
        assert!(
            matches!(result, Err(CoreError::StateConflict { .. })),
            "{remaining}: {result:?}"
        );
    }
    let result = device.settle(&grid, 1.5, &[valid], &mut Vec::new());
    assert!(
        matches!(result, Err(CoreError::NumericalInvalid { .. })),
        "{result:?}"
    );
    Ok(())
}
