//! FL-032, first stage: phase 6's chemistry on Metal is flife-engine's,
//! bit for bit. The device's new arithmetic (binary64 `tanh`, `next_down`,
//! the directed-rounding sums, `max` and `min`, the compensated sum) is
//! checked against flife-core's on a quarter million operand pairs. The
//! kernel is checked against `Chemistry::settle` on thousands of organisms:
//! zero and subnormal cells, wide moments, saturating and near-zero
//! efficiencies, budgets that toxicity exhausts, zero budgets, and more
//! packets than the reference keeps on its stack, under the preset's
//! constants and under extreme ones. Every input the reference refuses is
//! refused, one category at a time.

use flife_backend_metal::{
    ChemistryConstants, ChemistryOrganism, ChemistryOutcome, ChemistryPacket, MetalChemistry,
    SETTLE_OPS_WORDS,
};
use flife_core::ledger::{CompensatedSum, EnergyBudget, add_down, sub_down};
use flife_core::numerics::tanh;
use flife_core::{CoreError, ValidatedConfig};
use flife_engine::{Affinity, ChemicalPacket, Chemistry, composition};
use flife_field::state::CHANNELS;
use flife_field::{BufferLimits, FieldGrid, FieldState};

const LIMITS: BufferLimits = BufferLimits::new(1 << 30);
const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

/// `None` off macOS, where `open` must refuse with its typed error.
#[expect(clippy::panic, reason = "test helper")]
fn metal() -> Option<MetalChemistry> {
    match MetalChemistry::open() {
        Ok(chemistry) => Some(chemistry),
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

    /// A finite binary64 pattern: full random bits with the exponent kept
    /// below infinity, operands near each other's magnitude, small exponents,
    /// zeros of both signs, and values in tanh's active range.
    fn finite(&mut self, other: u64) -> u64 {
        let bits = self.next();
        match bits % 8 {
            0 => bits & 0x8000_0000_0000_0000,
            1 => other ^ (self.next() & 0x8000_0000_000F_FFFF),
            2 => bits & 0x800F_FFFF_FFFF_FFFF,
            3 | 4 => (self.signed() * 25.0).to_bits(),
            5 => (self.signed() * 1.0e-3).to_bits(),
            _ => {
                if (bits >> 52) & 0x7FF == 0x7FF {
                    bits & !(1 << 62)
                } else {
                    bits
                }
            }
        }
    }

    /// A cell energy over many magnitudes, with zeros of both signs and
    /// subnormals among them.
    fn energy(&mut self) -> f32 {
        match self.next() % 16 {
            0 => 0.0,
            1 => -0.0,
            2 => f32::from_bits((self.next() % 0x007F_FFFF) as u32 + 1),
            3 => (self.unit() * 1.0e-30) as f32,
            4 => (self.unit() * 1.0e6) as f32,
            5 => (self.unit() * 1.0e30) as f32,
            _ => (self.unit() * 4.0) as f32,
        }
    }

    /// A moment of either sign over many magnitudes, zero, or subnormal.
    fn moment(&mut self) -> f32 {
        match self.next() % 12 {
            0 => 0.0,
            1 => -0.0,
            2 => f32::from_bits((self.next() % 0x007F_FFFF) as u32 + 1),
            3 => (self.signed() * 1.0e-20) as f32,
            4 => (self.signed() * 1.0e20) as f32,
            _ => (self.signed() * 3.0) as f32,
        }
    }

    /// A grant: zero, tiny, subnormal, or over many magnitudes.
    fn granted(&mut self) -> f64 {
        match self.next() % 10 {
            0 => 0.0,
            1 => -0.0,
            2 => f64::from_bits(self.next() % 0x000F_FFFF_FFFF_FFFF + 1),
            3 => self.unit() * 1.0e-12,
            4 => self.unit() * 1.0e6,
            _ => self.unit() * 0.25,
        }
    }

    /// A budget's opening energy: zero, tiny, or over many magnitudes.
    fn remaining(&mut self) -> f64 {
        match self.next() % 10 {
            0 => 0.0,
            1 => self.unit() * 1.0e-9,
            2 => f64::from_bits(self.next() % 0x000F_FFFF_FFFF_FFFF + 1),
            3 => self.unit() * 1.0e4,
            _ => self.unit() * 2.0,
        }
    }
}

fn same(device: f64, reference: f64) -> bool {
    device.to_bits() == reference.to_bits()
}

fn pair_value(row: &[u32; SETTLE_OPS_WORDS], k: usize) -> f64 {
    f64::from_bits(u64::from(row[2 * k]) | (u64::from(row[2 * k + 1]) << 32))
}

/// The settlement arithmetic the kernel adds is flife-core's on every
/// checked pair, including the signs of zeros and the subnormals.
#[test]
fn the_settlement_arithmetic_is_the_references() -> Result<(), CoreError> {
    let Some(chemistry) = metal() else {
        return Ok(());
    };
    let mut rng = Rng(0x0C4E_3157_0000_0032);
    let mut pairs = Vec::new();
    let mut previous = 0_u64;
    while pairs.len() < 1 << 18 {
        let a = rng.finite(previous);
        let b = rng.finite(a);
        previous = b;
        pairs.push([a, b]);
    }
    // Edges: zeros, the smallest subnormals, the largest finite values, and
    // tanh's branch points and saturation.
    let edges = [
        0.0,
        -0.0,
        f64::from_bits(1),
        -f64::from_bits(1),
        f64::MIN_POSITIVE,
        f64::MAX,
        f64::MIN,
        1.0,
        -1.0,
        20.0,
        -20.0,
        f64::from_bits(1.0_f64.to_bits() - 1),
        f64::from_bits(20.0_f64.to_bits() - 1),
        0.5,
        1.0e-300,
        2.0,
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
    chemistry.settle_ops(&pairs, &mut rows)?;
    assert_eq!(rows.len(), pairs.len());
    let mut counted = [0_usize; 7];
    for (index, ([a, b], row)) in pairs.iter().zip(&rows).enumerate() {
        let (a, b) = (f64::from_bits(*a), f64::from_bits(*b));
        let want = tanh(a).expect("a finite argument");
        let got = pair_value(row, 0);
        assert!(
            same(got, want),
            "pair {index}: tanh({a:e}) gave {got:e}, not {want:e}"
        );
        counted[0] += 1;
        if (a + b).is_finite() && (a - b).is_finite() {
            let (got, want) = (pair_value(row, 1), add_down(a, b));
            assert!(
                same(got, want),
                "pair {index}: add_down({a:e}, {b:e}) gave {got:e}, not {want:e}"
            );
            let (got, want) = (pair_value(row, 2), sub_down(a, b));
            assert!(
                same(got, want),
                "pair {index}: sub_down({a:e}, {b:e}) gave {got:e}, not {want:e}"
            );
            counted[1] += 1;
            counted[2] += 1;
        }
        let (got, want) = (pair_value(row, 3), a.next_down());
        assert!(
            same(got, want),
            "pair {index}: next_down({a:e}) gave {got:e}, not {want:e}"
        );
        counted[3] += 1;
        let (got, want) = (pair_value(row, 4), a.max(b));
        assert!(
            same(got, want),
            "pair {index}: max({a:e}, {b:e}) gave {got:e}, not {want:e}"
        );
        let (got, want) = (pair_value(row, 5), a.min(b));
        assert!(
            same(got, want),
            "pair {index}: min({a:e}, {b:e}) gave {got:e}, not {want:e}"
        );
        counted[4] += 1;
        counted[5] += 1;
        let mut sum = CompensatedSum::ZERO;
        let mut finite = true;
        for value in [a, b, -a, b] {
            sum.add(value);
            let [running, compensation] = sum.parts();
            finite &= running.is_finite() && compensation.is_finite();
        }
        if finite && sum.value().is_finite() {
            let (got, want) = (pair_value(row, 6), sum.value());
            assert!(
                same(got, want),
                "pair {index}: the compensated sum gave {got:e}, not {want:e}"
            );
            counted[6] += 1;
        }
    }
    // Most pairs are in range for every operation: the checks are not vacuous.
    for (operation, &count) in counted.iter().enumerate() {
        assert!(
            count > 200_000,
            "operation {operation}: only {count} pairs checked"
        );
    }
    // The negative zero's handling is exercised.
    assert!(same(0.0_f64.max(-0.0), 0.0) && same(0.0_f64.min(-0.0), -0.0));
    Ok(())
}

/// The constants of `config` for both sides.
fn preset_constants() -> (Chemistry, ChemistryConstants) {
    #[expect(clippy::expect_used, reason = "test helper")]
    let config = ValidatedConfig::from_toml(PRESET).expect("the preset validates");
    let c = &config.config().chemistry;
    constants(
        c.maximum_efficiency,
        c.affinity_gain,
        c.discrimination_threshold,
        c.toxicity_sensitivity,
        c.affinity_norm_limit,
    )
}

#[expect(clippy::expect_used, reason = "test helper")]
fn constants(
    maximum_efficiency: f64,
    gain: f64,
    threshold: f64,
    toxicity_sensitivity: f64,
    affinity_norm_limit: f64,
) -> (Chemistry, ChemistryConstants) {
    (
        Chemistry::new(
            maximum_efficiency,
            gain,
            threshold,
            toxicity_sensitivity,
            affinity_norm_limit,
        )
        .expect("valid constants"),
        ChemistryConstants {
            maximum_efficiency,
            gain,
            threshold,
            toxicity_sensitivity,
            affinity_norm_limit,
        },
    )
}

/// A field of `width × height` cells with random energies and moments.
#[expect(clippy::expect_used, reason = "test helper")]
fn random_field(width: u32, height: u32, rng: &mut Rng) -> FieldState {
    let grid = FieldGrid::new(width, height, f64::from(width), f64::from(height)).expect("grid");
    let mut field = FieldState::zeroed(grid, &LIMITS).expect("field");
    for value in field.energy_mut() {
        *value = rng.energy();
    }
    for value in field.moments_mut() {
        *value = rng.moment();
    }
    field
}

/// An affinity of norm `fraction × limit` in a random direction, some of
/// its components zero.
fn affinity(rng: &mut Rng, limit: f64, fraction: f64) -> [f64; CHANNELS] {
    let mut direction = [0.0; CHANNELS];
    for component in &mut direction {
        *component = if rng.next().is_multiple_of(5) {
            0.0
        } else {
            rng.signed()
        };
    }
    let norm = direction.iter().map(|c| c * c).sum::<f64>().sqrt();
    if norm > 0.0 {
        for component in &mut direction {
            *component *= fraction * limit / norm;
        }
    }
    direction
}

/// One organism's inputs on both sides.
struct Case {
    affinity: [f64; CHANNELS],
    remaining: f64,
    packets: Vec<ChemistryPacket>,
}

/// The reference's settlement of `case`, with the budget after it.
fn reference(
    chemistry: &Chemistry,
    field: &FieldState,
    case: &Case,
) -> Result<ChemistryOutcome, CoreError> {
    let affinity = Affinity::new(case.affinity, chemistry)?;
    let mut packets = Vec::new();
    for packet in &case.packets {
        packets.push(ChemicalPacket {
            granted: packet.granted,
            composition: composition(field, packet.cell)?,
        });
    }
    let mut budget = EnergyBudget::new(case.remaining)?;
    let settlement = chemistry.settle(&affinity, &packets, &mut budget)?;
    Ok(ChemistryOutcome {
        absorbed: settlement.absorbed,
        gain: settlement.gain,
        requested_toxicity: settlement.toxicity.requested,
        paid_toxicity: settlement.toxicity.paid,
        remaining: budget.remaining(),
        exhausted: budget.is_exhausted(),
    })
}

/// `cases` as one device call's organisms and packets.
#[expect(clippy::expect_used, reason = "test helper")]
fn device_inputs(cases: &[Case]) -> (Vec<ChemistryOrganism>, Vec<ChemistryPacket>) {
    let mut organisms = Vec::new();
    let mut packets = Vec::new();
    for case in cases {
        let budget = EnergyBudget::new(case.remaining).expect("a valid budget");
        organisms.push(ChemistryOrganism {
            affinity: case.affinity,
            remaining: budget.remaining(),
            exhausted: budget.is_exhausted(),
            first: packets.len(),
            packets: case.packets.len(),
        });
        packets.extend_from_slice(&case.packets);
    }
    (organisms, packets)
}

/// Random organisms the reference settles, and how many it refused while
/// drawing them.
fn valid_cases(
    chemistry: &Chemistry,
    constants: &ChemistryConstants,
    field: &FieldState,
    rng: &mut Rng,
    count: usize,
) -> (Vec<Case>, Vec<ChemistryOutcome>, usize) {
    let cells = field.energy().len() as u64;
    let mut cases = Vec::new();
    let mut outcomes = Vec::new();
    let mut refused = 0;
    while cases.len() < count {
        let fraction = match rng.next() % 6 {
            0 => 0.0,
            1 => 1.0,
            _ => rng.unit(),
        };
        let packet_count = match rng.next() % 10 {
            0 => 0,
            1 => 5 + (rng.next() % 4) as usize,
            _ => 1 + (rng.next() % 4) as usize,
        };
        // Neighbouring cells, as a footprint is, or anywhere.
        let base = rng.next() % cells;
        let packets = (0..packet_count)
            .map(|k| ChemistryPacket {
                cell: if rng.next().is_multiple_of(3) {
                    (rng.next() % cells) as usize
                } else {
                    ((base + k as u64) % cells) as usize
                },
                granted: rng.granted(),
            })
            .collect();
        let case = Case {
            affinity: affinity(rng, constants.affinity_norm_limit, fraction),
            remaining: rng.remaining(),
            packets,
        };
        match reference(chemistry, field, &case) {
            Ok(outcome) => {
                cases.push(case);
                outcomes.push(outcome);
            }
            Err(_) => refused += 1,
        }
    }
    (cases, outcomes, refused)
}

fn assert_same_outcomes(device: &[ChemistryOutcome], reference: &[ChemistryOutcome]) {
    assert_eq!(device.len(), reference.len());
    for (index, (got, want)) in device.iter().zip(reference).enumerate() {
        let fields = [
            ("absorbed", got.absorbed, want.absorbed),
            ("gain", got.gain, want.gain),
            (
                "requested toxicity",
                got.requested_toxicity,
                want.requested_toxicity,
            ),
            ("paid toxicity", got.paid_toxicity, want.paid_toxicity),
            ("remaining", got.remaining, want.remaining),
        ];
        for (name, got_value, want_value) in fields {
            assert!(
                same(got_value, want_value),
                "organism {index}: {name} {got_value:e} on the device, {want_value:e} on the reference"
            );
        }
        assert_eq!(
            got.exhausted, want.exhausted,
            "organism {index}: exhaustion"
        );
    }
}

/// Thousands of organisms under the preset's constants and under extreme
/// ones: every settlement and budget is the reference's, bit for bit, and
/// the draws cover the cases that matter.
#[test]
fn chemistry_on_the_device_is_the_references_settlement() -> Result<(), CoreError> {
    let Some(mut device) = metal() else {
        return Ok(());
    };
    let mut rng = Rng(0x0C4E_3157_0000_0006);
    let field = random_field(64, 48, &mut rng);
    let sets = [
        preset_constants(),
        // Saturating: tanh at ±1, η_max 1, a tiny threshold.
        constants(1.0, 1.0e6, 1.0e-12, 3.0, 1.0),
        // Near zero: no gain at all, every efficiency -η_max tanh(b).
        constants(0.5, 0.0, 0.75, 0.25, 0.5),
    ];
    for (set, (chemistry, constants)) in sets.iter().enumerate() {
        let (cases, want, refused) = valid_cases(chemistry, constants, &field, &mut rng, 4_096);
        let (organisms, packets) = device_inputs(&cases);
        let mut got = Vec::new();
        device.settle(&field, constants, &organisms, &packets, &mut got)?;
        assert_same_outcomes(&got, &want);

        // Coverage: exhaustion by toxicity, zero budgets, gains, harms,
        // empty and long packet lists.
        // Exhausted by this settlement's toxicity, not by a zero opening.
        let exhausted = cases
            .iter()
            .zip(&want)
            .filter(|(case, o)| case.remaining > 0.0 && o.exhausted)
            .count();
        let gained = want.iter().filter(|o| o.gain > 0.0).count();
        let harmed = want.iter().filter(|o| o.paid_toxicity > 0.0).count();
        let long = cases.iter().filter(|c| c.packets.len() > 4).count();
        let empty = cases.iter().filter(|c| c.packets.is_empty()).count();
        assert!(exhausted > 50, "set {set}: {exhausted} exhausted");
        assert!(harmed > 1_000, "set {set}: {harmed} paid toxicity");
        assert!(
            long > 100 && empty > 100,
            "set {set}: {long} long, {empty} empty"
        );
        if set != 2 {
            assert!(gained > 500, "set {set}: {gained} gained");
        }
        // Some draws were refused by the reference (a zero budget's gain),
        // which this test leaves to the refusal test.
        assert!(refused < 4_096, "set {set}: {refused} refused");
    }
    Ok(())
}

/// The device refuses the call wherever the reference refuses an organism,
/// one category at a time, and leaves the outcomes empty.
#[test]
fn chemistry_the_reference_refuses_is_refused() -> Result<(), CoreError> {
    let Some(mut device) = metal() else {
        return Ok(());
    };
    let mut rng = Rng(0x0C4E_3157_0000_0009);
    let mut field = random_field(16, 16, &mut rng);
    // Cell 0: a positive energy and an infinite moment; cell 1: a plain
    // cell whose composition makes a positive efficiency.
    field.energy_mut()[0] = 1.0;
    field.moment_mut(3).expect("channel 3")[0] = f32::INFINITY;
    field.energy_mut()[1] = 1.0;
    for channel in 0..32 {
        field.moment_mut(channel).expect("a channel")[1] = 0.5;
    }
    let (chemistry, constants) = preset_constants();
    let limit = constants.affinity_norm_limit;
    let toward = [limit / (CHANNELS as f64).sqrt(); CHANNELS];
    let plain = |cell: usize| ChemistryPacket {
        cell,
        granted: 0.125,
    };
    let mut nonfinite = toward;
    nonfinite[7] = f64::NAN;
    let mut long = toward;
    long[0] *= 1.5;
    let cases = [
        (
            "a non-finite affinity",
            Case {
                affinity: nonfinite,
                remaining: 1.0,
                packets: vec![plain(1)],
            },
        ),
        (
            "an affinity past the limit",
            Case {
                affinity: long,
                remaining: 1.0,
                packets: vec![],
            },
        ),
        (
            "a negative grant",
            Case {
                affinity: toward,
                remaining: 1.0,
                packets: vec![
                    plain(1),
                    ChemistryPacket {
                        cell: 1,
                        granted: -1.0e-300,
                    },
                ],
            },
        ),
        (
            "a NaN grant",
            Case {
                affinity: toward,
                remaining: 1.0,
                packets: vec![ChemistryPacket {
                    cell: 1,
                    granted: f64::NAN,
                }],
            },
        ),
        (
            "an infinite moment",
            Case {
                affinity: toward,
                remaining: 1.0,
                packets: vec![plain(0)],
            },
        ),
        (
            "a gain for a zero budget",
            Case {
                affinity: toward,
                remaining: 0.0,
                packets: vec![plain(1)],
            },
        ),
        (
            "a cell outside the field",
            Case {
                affinity: toward,
                remaining: 1.0,
                packets: vec![plain(256)],
            },
        ),
    ];
    for (name, case) in &cases {
        assert!(
            reference(&chemistry, &field, case).is_err(),
            "{name}: the reference should refuse this"
        );
        // The refused organism among valid ones, so the call refuses as a
        // whole.
        let valid = Case {
            affinity: toward,
            remaining: 2.0,
            packets: vec![plain(1)],
        };
        assert!(reference(&chemistry, &field, &valid).is_ok());
        let both = [
            valid,
            Case {
                affinity: case.affinity,
                remaining: case.remaining,
                packets: case.packets.clone(),
            },
        ];
        let mut organisms = Vec::new();
        let mut packets = Vec::new();
        for case in &both {
            organisms.push(ChemistryOrganism {
                affinity: case.affinity,
                remaining: case.remaining,
                exhausted: case.remaining == 0.0,
                first: packets.len(),
                packets: case.packets.len(),
            });
            packets.extend_from_slice(&case.packets);
        }
        let mut out = vec![ChemistryOutcome {
            absorbed: 1.0,
            gain: 1.0,
            requested_toxicity: 1.0,
            paid_toxicity: 1.0,
            remaining: 1.0,
            exhausted: false,
        }];
        let result = device.settle(&field, &constants, &organisms, &packets, &mut out);
        assert!(
            matches!(
                result,
                Err(CoreError::StateConflict {
                    operation: "metal chemistry",
                    ..
                })
            ),
            "{name}: the device gave {result:?}"
        );
        assert!(out.is_empty(), "{name}: outcomes were left");
        // Alone, the valid organism settles as the reference does.
        let (organisms, packets) = device_inputs(&both[..1]);
        device.settle(&field, &constants, &organisms, &packets, &mut out)?;
        assert_same_outcomes(&out, &[reference(&chemistry, &field, &both[0])?]);
    }

    // Packets past the list, and constants the reference rejects, are refused
    // before anything runs.
    let organism = ChemistryOrganism {
        affinity: toward,
        remaining: 1.0,
        exhausted: false,
        first: 1,
        packets: 1,
    };
    let result = device.settle(
        &field,
        &constants,
        &[organism],
        &[plain(1)],
        &mut Vec::new(),
    );
    assert!(
        matches!(result, Err(CoreError::LayoutMismatch { .. })),
        "{result:?}"
    );
    let bad = ChemistryConstants {
        toxicity_sensitivity: 0.0,
        ..constants
    };
    let result = device.settle(&field, &bad, &[], &[], &mut Vec::new());
    assert!(
        matches!(result, Err(CoreError::NumericalInvalid { .. })),
        "{result:?}"
    );
    Ok(())
}
