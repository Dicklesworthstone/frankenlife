//! FL-031: phase 5 on Metal is flife-field's [`demands`] and
//! [`resolve_uptake`], bit for bit: every demand, packet, debit, total and
//! summary. The cases are scattered and oversubscribed cells, a hotspot of
//! thousands of packets in one cell with tied remainders, subnormal, zero
//! and negative-zero energies, requests below half a quantum, organisms
//! whose identities are not in input order, and the running preset. The
//! errors are the reference's, and a cell partition the device cannot serve
//! is refused before anything runs.

use flife_backend_metal::MetalUptake;
use flife_core::ids::OrganismId;
use flife_core::{CoreError, TimeStep, ValidatedConfig};
use flife_engine::{Arrangement, NumericalProfile, Stepped, World};
use flife_field::{
    BufferLimits, Demand, ExchangeStencil, FieldGrid, FieldState, Grants, RequestRule,
    UptakeBackend, UptakeCell, UptakeWork, demands, demands_on, resolve_uptake, resolve_uptake_on,
};

const LIMITS: BufferLimits = BufferLimits::new(1 << 30);
const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

/// `None` off macOS, where `open` must refuse with its typed error.
#[expect(clippy::panic, reason = "test helper")]
fn metal() -> Option<MetalUptake> {
    match MetalUptake::open() {
        Ok(uptake) => Some(uptake),
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

    /// A cell energy over many magnitudes, with zeros of both signs and
    /// subnormals among them.
    fn energy(&mut self) -> f32 {
        match self.next() % 16 {
            0 => 0.0,
            1 => -0.0,
            2 => f32::from_bits((self.next() % 0x007F_FFFF) as u32 + 1),
            3 => f32::MIN_POSITIVE * self.unit() as f32,
            4 => (self.unit() * 1.0e-30) as f32,
            5 => (self.unit() * 1.0e6) as f32,
            6 => (self.unit() * 1.0e30) as f32,
            _ => (self.unit() * 4.0) as f32,
        }
    }
}

#[expect(clippy::expect_used, reason = "test helper")]
fn field_of(grid: FieldGrid, energy: impl FnMut() -> f32) -> FieldState {
    let mut field = FieldState::zeroed(grid, &LIMITS).expect("field");
    let mut energy = energy;
    for value in field.energy_mut() {
        *value = energy();
    }
    field
}

/// Distinct identities in a shuffled order.
fn identities(count: usize, rng: &mut Rng) -> Vec<OrganismId> {
    let mut ids: Vec<u64> = (0..count as u64).map(|id| id * 7 + 3).collect();
    for index in (1..ids.len()).rev() {
        let other = (rng.next() % (index as u64 + 1)) as usize;
        ids.swap(index, other);
    }
    ids.into_iter().map(OrganismId::new).collect()
}

fn below(value: f64) -> f64 {
    f64::from_bits(value.to_bits() - 1)
}

/// Positions at cell centers, knots, edges, the seams and far away, then
/// uniform ones.
fn positions(grid: &FieldGrid, rng: &mut Rng, count: usize) -> Vec<[f64; 2]> {
    let (width, height) = (grid.world_width(), grid.world_height());
    let (dx, dy) = (grid.cell_width(), grid.cell_height());
    let mut out = vec![
        [0.0, 0.0],
        [-0.0, below(height)],
        [width, height],
        [0.5 * dx, 0.5 * dy],
        [below(0.5 * dx), 0.5 * dy],
        [dx, 2.0 * dy],
        [-width * 1.5, height * 3.25],
        [f64::MIN_POSITIVE, -1.0e-300],
    ];
    while out.len() < count {
        out.push([
            (rng.unit() * 3.0 - 1.0) * width,
            (rng.unit() * 3.0 - 1.0) * height,
        ]);
    }
    out.truncate(count);
    out
}

type PacketBits = (usize, u64, usize, u64, u64);
type DebitBits = (usize, u32, u32, u64, u64, bool);

fn packet_bits(grants: &Grants) -> Vec<PacketBits> {
    grants
        .packets()
        .iter()
        .map(|packet| {
            (
                packet.cell,
                packet.organism.get(),
                packet.demand,
                packet.requested.to_bits(),
                packet.granted.to_bits(),
            )
        })
        .collect()
}

fn debit_bits(grants: &Grants) -> Vec<DebitBits> {
    grants
        .debits()
        .iter()
        .map(|debit| {
            (
                debit.cell,
                debit.energy_before.to_bits(),
                debit.energy_after.to_bits(),
                debit.demanded.to_bits(),
                debit.granted.to_bits(),
                debit.contended,
            )
        })
        .collect()
}

/// The F32 unit in the last place of a nonnegative `energy`.
fn quantum_of(energy: f32) -> f64 {
    let exponent = (energy.to_bits() >> 23) & 0xff;
    f64::from_bits(u64::from(exponent.max(1) + 873) << 52)
}

/// What the cells of one resolution exercised: (contended, deficit, exact,
/// surplus, empty) cells, recomputed from the reference's packets with
/// `cell_target`'s arithmetic. Coverage only: the expected values are the
/// reference's own.
#[derive(Debug, Default, Clone, Copy)]
struct Coverage {
    contended: usize,
    deficit: usize,
    exact: usize,
    surplus: usize,
    empty: usize,
}

fn coverage(grants: &Grants, into: &mut Coverage) {
    for (cell, range) in grants.bins().occupied() {
        let Some(debit) = grants.debits().iter().find(|debit| debit.cell == cell) else {
            continue;
        };
        let bits = debit.energy_before.to_bits();
        let exponent = (bits >> 23) & 0xff;
        let whole = u64::from(bits & 0x7f_ffff) | if exponent == 0 { 0 } else { 0x80_0000 };
        let quantum = quantum_of(debit.energy_before);
        into.contended += usize::from(debit.contended);
        if whole == 0 {
            into.empty += 1;
            continue;
        }
        let (target, scale) = if debit.contended {
            (whole, whole as f64 / debit.demanded)
        } else {
            let ideal = (debit.demanded / quantum)
                .round_ties_even()
                .min(whole as f64);
            (ideal as u64, 1.0 / quantum)
        };
        let floors: u64 = grants.packets()[range]
            .iter()
            .map(|packet| (packet.requested * scale).floor() as u64)
            .sum();
        match floors.cmp(&target) {
            std::cmp::Ordering::Less => into.deficit += 1,
            std::cmp::Ordering::Equal => into.exact += 1,
            std::cmp::Ordering::Greater => into.surplus += 1,
        }
    }
}

/// Resolve `demands` on the device and the reference and compare every
/// bit; the packets compared.
#[expect(clippy::panic, reason = "test helper")]
fn compare(
    device: &mut MetalUptake,
    name: &str,
    field: &FieldState,
    demands: &[Demand],
    seen: &mut Coverage,
) -> usize {
    let mut expected = Grants::new();
    let mut got = Grants::new();
    let mut work = UptakeWork::new();
    let reference = resolve_uptake(field, demands, &mut expected)
        .unwrap_or_else(|error| panic!("{name}: the reference refused: {error}"));
    let summary = resolve_uptake_on(device, field, demands, &mut work, &mut got)
        .unwrap_or_else(|error| panic!("{name}: the device refused: {error}"));
    assert_eq!(packet_bits(&got), packet_bits(&expected), "{name}: packets");
    assert_eq!(debit_bits(&got), debit_bits(&expected), "{name}: debits");
    let totals = |grants: &Grants| -> Vec<u64> {
        grants
            .totals()
            .iter()
            .map(|total| total.to_bits())
            .collect()
    };
    assert_eq!(totals(&got), totals(&expected), "{name}: totals");
    assert_eq!(
        (summary.requested.to_bits(), summary.granted.to_bits()),
        (reference.requested.to_bits(), reference.granted.to_bits()),
        "{name}: summary totals"
    );
    assert_eq!(
        (
            summary.cells,
            summary.contended_cells,
            summary.drained_cells,
            summary.largest_cell_packets
        ),
        (
            reference.cells,
            reference.contended_cells,
            reference.drained_cells,
            reference.largest_cell_packets
        ),
        "{name}: summary counts"
    );
    coverage(&expected, seen);
    expected.packets().len()
}

/// Demands of `amount(index)` at `positions`, with shuffled identities.
fn demands_at(
    positions: &[[f64; 2]],
    rng: &mut Rng,
    mut amount: impl FnMut(usize, &mut Rng) -> f64,
) -> Vec<Demand> {
    let ids = identities(positions.len(), rng);
    positions
        .iter()
        .zip(ids)
        .enumerate()
        .map(|(index, (&position, organism))| Demand {
            organism,
            position,
            amount: amount(index, rng),
        })
        .collect()
}

#[test]
#[expect(clippy::too_many_lines, reason = "one case after another")]
fn the_metal_grants_are_the_reference_grants_bit_for_bit() {
    let Some(mut device) = metal() else { return };
    let mut rng = Rng(0x5EED_0F31_0005_A1E5);
    let mut seen = Coverage::default();
    let mut compared = 0;
    let grid = FieldGrid::new(32, 32, 32.0, 32.0).expect("grid");

    // Scattered organisms over mixed energies, at the request rule's scale.
    let field = field_of(grid, || rng.energy());
    let at = positions(&grid, &mut rng, 1_500);
    let scattered = demands_at(&at, &mut rng, |_, rng| rng.unit() * 2.0);
    compared += compare(&mut device, "scattered", &field, &scattered, &mut seen);

    // Oversubscribed: every cell asked for many times its energy.
    let field = field_of(grid, || (rng.unit() * 3.0) as f32);
    let at = positions(&grid, &mut rng, 4_000);
    let oversubscribed = demands_at(&at, &mut rng, |_, rng| 5.0 + rng.unit() * 50.0);
    compared += compare(
        &mut device,
        "oversubscribed",
        &field,
        &oversubscribed,
        &mut seen,
    );

    // A hotspot: thousands of organisms at one cell center (a one-cell
    // footprint) and around it, many with equal amounts, so remainders tie
    // and identities break the ties.
    let field = field_of(grid, || 1.75);
    let centre = [10.5, 20.5];
    let mut at = vec![centre; 3_000];
    for index in 0..1_000 {
        let offset = f64::from(index % 40) * 0.01 - 0.2;
        at.push([centre[0] + offset, centre[1] - offset]);
    }
    for (name, amount) in [
        ("hotspot, contended", 0.375),
        ("hotspot, uncontended", 1.0e-4),
        ("hotspot, irrational", std::f64::consts::PI * 1.0e-3),
    ] {
        let hotspot = demands_at(&at, &mut rng, |index, rng| {
            if index % 3 == 0 {
                amount * (1.0 + rng.unit())
            } else {
                amount
            }
        });
        compared += compare(&mut device, name, &field, &hotspot, &mut seen);
    }

    // Subnormal cells, asked for a few quanta each or many times more.
    let field = field_of(grid, || {
        f32::from_bits((rng.next() % 0x007F_FFFF) as u32 + 1)
    });
    let quantum = f64::from(f32::from_bits(1));
    let at = positions(&grid, &mut rng, 2_000);
    for (name, scale) in [("subnormal, few", 3.0), ("subnormal, many", 1.0e9)] {
        let subnormal = demands_at(&at, &mut rng, |_, rng| quantum * scale * rng.unit());
        compared += compare(&mut device, name, &field, &subnormal, &mut seen);
    }

    // Zero cells of both signs and requests below half a quantum.
    let field = field_of(grid, || match rng.next() % 3 {
        0 => 0.0,
        1 => -0.0,
        _ => 1.0e6,
    });
    let at = positions(&grid, &mut rng, 2_000);
    let tiny = demands_at(&at, &mut rng, |index, rng| {
        if index % 2 == 0 {
            1.0e-3 * rng.unit()
        } else {
            0.0
        }
    });
    compared += compare(
        &mut device,
        "zero cells and tiny requests",
        &field,
        &tiny,
        &mut seen,
    );

    // Edges, one organism at each cell center (a one-cell footprint of
    // weight exactly one): demands of exactly k + 1/2 quanta, which round to
    // even; exactly the energy, which is not contended; just above it, and
    // random multiples of it, each contended alone in its cell, where the
    // floor of q (n/q) can fall a quantum short of n.
    let field = field_of(grid, || (0.25 + rng.unit() * 4.0) as f32);
    let centres: Vec<[f64; 2]> = (0..32 * 32)
        .map(|cell| [f64::from(cell % 32) + 0.5, f64::from(cell / 32) + 0.5])
        .collect();
    let energy_at = |position: [f64; 2]| -> f32 {
        let stencil = ExchangeStencil::at(&grid, position).expect("stencil");
        assert_eq!(stencil.weights(), [1.0]);
        field.energy()[stencil.cells()[0]]
    };
    let edges = demands_at(&centres, &mut rng, |index, rng| {
        let energy = energy_at(centres[index]);
        let value = f64::from(energy);
        match index % 4 {
            0 => (f64::from(index as u32 % 7) + 0.5) * quantum_of(energy),
            1 => value,
            2 => f64::from_bits(value.to_bits() + 1),
            _ => value * (1.0 + 3.0 * rng.unit()),
        }
    });
    compared += compare(&mut device, "edges", &field, &edges, &mut seen);

    // Every energy regime at once, on a stretched torus, then again on
    // buffers grown for more packets and fewer.
    let stretched = FieldGrid::new(48, 16, 40.0, 7.0).expect("grid");
    let field = field_of(stretched, || rng.energy());
    let at = positions(&stretched, &mut rng, 3_000);
    let mixed = demands_at(&at, &mut rng, |_, rng| {
        let magnitude = match rng.next() % 4 {
            0 => 1.0e-40,
            1 => 1.0e-6,
            2 => 1.0,
            _ => 1.0e7,
        };
        magnitude * rng.unit()
    });
    compared += compare(&mut device, "mixed", &field, &mixed, &mut seen);
    compared += compare(&mut device, "a few", &field, &mixed[..5], &mut seen);
    compared += compare(&mut device, "none", &field, &[], &mut seen);
    compared += compare(&mut device, "mixed again", &field, &mixed, &mut seen);

    println!("compared {compared} packets: {seen:?}");
    assert!(compared > 40_000, "{compared}");
    assert!(seen.contended > 100, "{seen:?}");
    assert!(seen.deficit > 100, "{seen:?}");
    assert!(seen.exact > 100, "{seen:?}");
    assert!(seen.empty > 10, "{seen:?}");
}

/// The request rule at `seconds` per tick with `capacity` and `half`.
#[expect(clippy::expect_used, reason = "test helper")]
fn rule(numerator: u32, denominator: u32, capacity: f64, half: f64) -> RequestRule {
    RequestRule {
        step: TimeStep::new(numerator, denominator).expect("step"),
        capacity_per_second: capacity,
        half_saturation_density: half,
    }
}

/// Compare the device's demands with the reference's; the demands compared.
#[expect(clippy::panic, reason = "test helper")]
fn compare_demands(
    device: &mut MetalUptake,
    name: &str,
    field: &FieldState,
    rule: &RequestRule,
    requesters: &[(OrganismId, [f64; 2])],
) -> usize {
    let (mut expected, mut got, mut work) = (Vec::new(), Vec::new(), UptakeWork::new());
    demands(field, rule, requesters.iter().copied(), &mut expected)
        .unwrap_or_else(|error| panic!("{name}: the reference refused: {error}"));
    demands_on(
        device,
        field,
        rule,
        requesters.iter().copied(),
        &mut work,
        &mut got,
    )
    .unwrap_or_else(|error| panic!("{name}: the device refused: {error}"));
    let bits = |list: &[Demand]| -> Vec<(u64, [u64; 2], u64)> {
        list.iter()
            .map(|demand| {
                (
                    demand.organism.get(),
                    demand.position.map(f64::to_bits),
                    demand.amount.to_bits(),
                )
            })
            .collect()
    };
    assert_eq!(bits(&got), bits(&expected), "{name}");
    requesters.len()
}

#[test]
fn the_metal_demands_are_the_reference_demands_bit_for_bit() {
    let Some(mut device) = metal() else { return };
    let mut rng = Rng(0x0DE3_A11D_5EED);
    let mut compared = 0;
    for (name, cells, world, step) in [
        ("square", [32, 32], [32.0, 32.0], rule(1, 10, 1.25, 0.5)),
        ("stretched", [48, 16], [40.0, 7.0], rule(1, 3, 0.7, 2.0e-3)),
        ("coarse", [8, 8], [100.0, 100.0], rule(2, 1, 1.0e3, 1.0e-30)),
    ] {
        let grid = FieldGrid::new(cells[0], cells[1], world[0], world[1]).expect("grid");
        let field = field_of(grid, || rng.energy());
        let at = positions(&grid, &mut rng, 3_000);
        let requesters: Vec<_> = identities(at.len(), &mut rng).into_iter().zip(at).collect();
        compared += compare_demands(&mut device, name, &field, &step, &requesters);
        compared += compare_demands(&mut device, "a few", &field, &step, &requesters[..3]);
    }
    assert!(compared > 9_000, "{compared}");
}

#[test]
fn the_metal_uptake_of_the_running_preset_is_the_reference_uptake() {
    let Some(mut device) = metal() else { return };
    let config = ValidatedConfig::from_toml(PRESET).expect("the preset validates");
    let chemistry = &config.config().chemistry;
    let rule = RequestRule {
        step: config.report().timestep,
        capacity_per_second: chemistry.uptake_capacity_per_second,
        half_saturation_density: chemistry.uptake_half_saturation_density,
    };
    let mut world = World::found(
        &config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("the world founds");
    let mut seen = Coverage::default();
    let (mut packets, mut organisms) = (0, 0);
    for tick in 0..120 {
        match world.step().expect("the world steps") {
            Stepped::Published(_) => {}
            other => panic!("unexpected outcome {other:?}"),
        }
        if tick % 30 == 0 {
            let field = world.field().clone();
            let requesters: Vec<_> = world
                .population()
                .iter()
                .map(|organism| (organism.id, organism.position))
                .collect();
            organisms += compare_demands(&mut device, "preset demands", &field, &rule, &requesters);
            let mut list = Vec::new();
            demands(&field, &rule, requesters.iter().copied(), &mut list)
                .expect("the reference demands");
            packets += compare(&mut device, "preset grants", &field, &list, &mut seen);
        }
    }
    println!("{organisms} organisms, {packets} packets: {seen:?}");
    assert!(packets > organisms, "{packets} {organisms}");
}

#[test]
fn the_metal_uptake_refuses_what_the_reference_refuses() {
    let Some(mut device) = metal() else { return };
    let mut rng = Rng(0x00BA_D00F_7ACE);
    let grid = FieldGrid::new(16, 16, 16.0, 16.0).expect("grid");
    let field = field_of(grid, || 2.0);
    let step = rule(1, 10, 1.0, 0.5);
    let mut work = UptakeWork::new();
    let (mut list, mut grants) = (Vec::new(), Grants::new());
    let one = |position| [(OrganismId::new(1), position)];

    // A non-finite energy under a footprint: the reference refuses its
    // density, and the device refuses the NaN softfloat64 makes of it.
    for bad in [f32::NAN, f32::INFINITY, -1.0] {
        let mut broken = field.clone();
        broken.energy_mut()[0] = bad;
        assert!(demands(&broken, &step, one([0.5, 0.5]), &mut list).is_err());
        assert!(
            demands_on(
                &mut device,
                &broken,
                &step,
                one([0.5, 0.5]),
                &mut work,
                &mut list
            )
            .is_err(),
            "{bad}"
        );
        // Away from it, the same demand.
        compare_demands(&mut device, "away", &broken, &step, &one([8.5, 8.5]));
    }

    // An amount or position the reference refuses, and a cell energy it
    // refuses: the device path refuses too, and leaves the grants empty.
    let good = Demand {
        organism: OrganismId::new(2),
        position: [3.5, 3.5],
        amount: 0.25,
    };
    for (demand, energy) in [
        (
            Demand {
                amount: -1.0,
                ..good
            },
            2.0,
        ),
        (
            Demand {
                amount: f64::NAN,
                ..good
            },
            2.0,
        ),
        (
            Demand {
                position: [f64::NAN, 1.0],
                ..good
            },
            2.0,
        ),
        (good, f32::NAN),
        (good, -0.5),
    ] {
        let mut broken = field.clone();
        broken.energy_mut()[3 * 16 + 3] = energy;
        let pair = [good_other(), demand];
        assert!(resolve_uptake(&broken, &pair, &mut grants).is_err());
        assert!(
            resolve_uptake_on(&mut device, &broken, &pair, &mut work, &mut grants).is_err(),
            "{demand:?} {energy}"
        );
        assert!(grants.packets().is_empty() && grants.debits().is_empty());
    }

    // Cells that do not partition the packets in order.
    let (mut demanded, mut quanta) = (Vec::new(), Vec::new());
    let cell = |start, len| UptakeCell {
        start,
        len,
        energy: 1.0,
    };
    for cells in [
        vec![cell(0, 2), cell(3, 1)],
        vec![cell(0, 0), cell(0, 4)],
        vec![cell(0, 2), cell(2, 1)],
        vec![cell(1, 3)],
    ] {
        assert!(matches!(
            device.resolve(&cells, &[0.1; 4], &mut demanded, &mut quanta),
            Err(CoreError::StateConflict { .. })
        ));
    }
    // A cell whose packets' sum overflows: softfloat64 makes its demand a
    // NaN, which the device refuses.
    assert!(matches!(
        device.resolve(
            &[cell(0, 2)],
            &[f64::MAX, f64::MAX],
            &mut demanded,
            &mut quanta
        ),
        Err(CoreError::StateConflict { .. })
    ));
    // A request that overflows: the reference's is an infinity and
    // softfloat64's a NaN, so the device refuses it rather than return
    // another value.
    let huge = rule(2, 1, f64::MAX, 0.5);
    demands(&field, &huge, one([3.5, 3.5]), &mut list).expect("the reference requests");
    assert_eq!(list[0].amount, f64::INFINITY);
    assert!(matches!(
        demands_on(
            &mut device,
            &field,
            &huge,
            one([3.5, 3.5]),
            &mut work,
            &mut list
        ),
        Err(CoreError::StateConflict { .. })
    ));
    // A stencil from a larger grid.
    let large = FieldGrid::new(64, 64, 64.0, 64.0).expect("grid");
    let far = ExchangeStencil::at(&large, [60.5, 60.5]).expect("stencil");
    let (mut densities, mut requests) = (Vec::new(), Vec::new());
    assert!(matches!(
        device.request(&field, &step, &[far], &mut densities, &mut requests),
        Err(CoreError::LayoutMismatch { .. })
    ));

    // The device still matches the reference afterwards.
    let at = positions(&grid, &mut rng, 500);
    let after = demands_at(&at, &mut rng, |_, rng| rng.unit());
    compare(
        &mut device,
        "after the errors",
        &field,
        &after,
        &mut Coverage::default(),
    );
}

fn good_other() -> Demand {
    Demand {
        organism: OrganismId::new(9),
        position: [12.5, 12.5],
        amount: 0.5,
    }
}
