//! FL-032: phase 9 on Metal is flife-field's [`reduce_field_on`], bit for
//! bit: the grants' debit, then the emission and recycling deposits, into
//! the field, with each reduction's summary. The cases are scattered
//! organisms over mixed energies and signed moments, oversubscribed cells
//! the grants drain, a hotspot of thousands of deposits in one cell, tiny
//! and huge amounts (one cell lifted past the largest F32), subnormal
//! energies and moments, a stretched torus, and each part alone. A
//! reduction the device cannot finish leaves the field, the grants and the
//! deposits as they were (plan §8.3), and the reference then reduces them
//! as if nothing had happened; the reference's refusals are the device's.

use flife_backend_metal::MetalField;
use flife_core::CoreError;
use flife_core::ids::OrganismId;
use flife_field::state::CHANNELS;
use flife_field::{
    BufferLimits, Demand, DepositSummary, Deposits, FieldBackend, FieldGrid, FieldState, Grants,
    reduce_field_on, resolve_uptake,
};

const LIMITS: BufferLimits = BufferLimits::new(1 << 30);

/// `None` off macOS, where `open` must refuse with its typed error.
#[expect(clippy::panic, reason = "test helper")]
fn metal() -> Option<MetalField> {
    match MetalField::open() {
        Ok(field) => Some(field),
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
            3 => (self.unit() * 1.0e-30) as f32,
            4 => (self.unit() * 1.0e6) as f32,
            5 => (self.unit() * 1.0e30) as f32,
            _ => (self.unit() * 4.0) as f32,
        }
    }

    /// A signed moment, sometimes zero of either sign or subnormal.
    fn moment(&mut self) -> f32 {
        match self.next() % 12 {
            0 => 0.0,
            1 => -0.0,
            2 => -f32::from_bits((self.next() % 0x007F_FFFF) as u32 + 1),
            3 => (self.unit() * 1.0e6 - 5.0e5) as f32,
            _ => (self.unit() * 2.0 - 1.0) as f32,
        }
    }

    /// A direction in the unit ball, some components zero of either sign.
    fn direction(&mut self) -> [f64; CHANNELS] {
        let scale = 1.0 / (CHANNELS as f64).sqrt();
        std::array::from_fn(|_| match self.next() % 8 {
            0 => 0.0,
            1 => -0.0,
            _ => (self.unit() * 2.0 - 1.0) * scale,
        })
    }
}

#[expect(clippy::expect_used, reason = "test helper")]
fn field_of(grid: FieldGrid, rng: &mut Rng, energy: impl Fn(&mut Rng) -> f32) -> FieldState {
    let mut field = FieldState::zeroed(grid, &LIMITS).expect("field");
    for value in field.energy_mut() {
        *value = energy(rng);
    }
    for value in field.moments_mut() {
        *value = rng.moment();
    }
    field
}

fn below(value: f64) -> f64 {
    f64::from_bits(value.to_bits() - 1)
}

/// Positions at cell centers, edges, the seams and far away, then uniform
/// ones.
fn positions(grid: &FieldGrid, rng: &mut Rng, count: usize) -> Vec<[f64; 2]> {
    let (width, height) = (grid.world_width(), grid.world_height());
    let (dx, dy) = (grid.cell_width(), grid.cell_height());
    let mut out = vec![
        [0.0, 0.0],
        [-0.0, below(height)],
        [width, height],
        [0.5 * dx, 0.5 * dy],
        [dx, 2.0 * dy],
        [-width * 1.5, height * 3.25],
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

/// Distinct identities in a shuffled order, from `first`.
fn identities(first: u64, count: usize, rng: &mut Rng) -> Vec<OrganismId> {
    let mut ids: Vec<u64> = (0..count as u64).map(|id| first + id * 7).collect();
    for index in (1..ids.len()).rev() {
        let other = (rng.next() % (index as u64 + 1)) as usize;
        ids.swap(index, other);
    }
    ids.into_iter().map(OrganismId::new).collect()
}

/// The reference's grants for demands of `amount` at `positions`.
#[expect(clippy::expect_used, reason = "test helper")]
fn grants_for(
    field: &FieldState,
    positions: &[[f64; 2]],
    rng: &mut Rng,
    mut amount: impl FnMut(&mut Rng) -> f64,
) -> Grants {
    let ids = identities(3, positions.len(), rng);
    let demands: Vec<Demand> = positions
        .iter()
        .zip(ids)
        .map(|(&position, organism)| Demand {
            organism,
            position,
            amount: amount(rng),
        })
        .collect();
    let mut grants = Grants::new();
    resolve_uptake(field, &demands, &mut grants).expect("the reference resolves");
    grants
}

/// Deposits paying `paid` at `positions`, each offering a fraction of it
/// (sometimes nothing, sometimes all) in a random direction.
#[expect(clippy::expect_used, reason = "test helper")]
fn deposits_at(
    grid: &FieldGrid,
    positions: &[[f64; 2]],
    first: u64,
    rng: &mut Rng,
    mut paid: impl FnMut(&mut Rng) -> f64,
) -> Deposits {
    let mut deposits = Deposits::new();
    let ids = identities(first, positions.len(), rng);
    for (&position, organism) in positions.iter().zip(ids) {
        let paid = paid(rng);
        let offered = match rng.next() % 6 {
            0 => 0.0,
            1 => paid,
            _ => paid * rng.unit(),
        };
        deposits
            .push(grid, organism, position, paid, offered, &rng.direction())
            .expect("a valid deposit");
    }
    deposits
}

type SummaryBits = (usize, usize, u64, u64, u64);

fn summary_bits(summary: &DepositSummary) -> SummaryBits {
    (
        summary.organisms,
        summary.cells,
        summary.paid.to_bits(),
        summary.offered.to_bits(),
        summary.applied.to_bits(),
    )
}

fn field_bits(field: &FieldState) -> Vec<u32> {
    field
        .as_slice()
        .iter()
        .map(|value| value.to_bits())
        .collect()
}

/// What the cases exercised.
#[derive(Debug, Default)]
struct Coverage {
    debited: usize,
    drained: usize,
    shared: usize,
    packets: usize,
}

/// Reduce `grants` and the two deposit sets into `field` on the reference
/// and on `device`, from the same inputs, and compare every bit. Returns
/// the packets reduced.
#[expect(clippy::panic, reason = "test helper")]
fn compare(
    device: &mut MetalField,
    name: &str,
    field: &FieldState,
    grants: &Grants,
    deposits: [&Deposits; 2],
    seen: &mut Coverage,
) -> usize {
    let (mut expected_field, mut expected_grants) = (field.clone(), grants.clone());
    let [mut first, mut second] = deposits.map(Clone::clone);
    let expected = reduce_field_on(
        &mut expected_field,
        &mut expected_grants,
        [&mut first, &mut second],
        1,
    );
    let (mut got_field, mut got_grants) = (field.clone(), grants.clone());
    let [mut third, mut fourth] = deposits.map(Clone::clone);
    let got = device.reduce(&mut got_field, &mut got_grants, [&mut third, &mut fourth]);
    match (expected, got) {
        (Ok(expected), Ok(got)) => {
            assert_eq!(
                expected.each_ref().map(summary_bits),
                got.each_ref().map(summary_bits),
                "{name}: the summaries"
            );
            let (want, have) = (field_bits(&expected_field), field_bits(&got_field));
            if let Some(at) = want.iter().zip(&have).position(|(a, b)| a != b) {
                panic!(
                    "{name}: value {at} is {:e} ({:#010x}), not {:e} ({:#010x})",
                    f32::from_bits(have[at]),
                    have[at],
                    f32::from_bits(want[at]),
                    want[at]
                );
            }
            // The device applied everything, as the reference did.
            assert_eq!(
                [
                    got_grants.is_applied(),
                    third.is_applied(),
                    fourth.is_applied()
                ],
                [true; 3],
                "{name}: the device left something unapplied"
            );
        }
        (Err(expected), Err(got)) => {
            assert_eq!(expected.to_string(), got.to_string(), "{name}");
        }
        (expected, got) => panic!("{name}: {expected:?} against {got:?}"),
    }
    for debit in grants.debits().iter().filter(|debit| debit.granted > 0.0) {
        seen.debited += 1;
        seen.drained += usize::from(debit.energy_after == 0.0);
    }
    let mut per_cell = vec![0_usize; field.energy().len()];
    let mut packets = 0;
    for set in deposits {
        per_cell.fill(0);
        for index in 0..set.len() {
            for (cell, _) in set.entry_packets(index) {
                per_cell[cell] += 1;
                packets += 1;
            }
        }
        seen.shared += per_cell.iter().filter(|&&count| count > 1).count();
    }
    seen.packets += packets;
    packets
}

#[test]
#[expect(clippy::too_many_lines, reason = "one case after another")]
fn the_metal_reduction_is_the_reference_reduction_bit_for_bit() {
    let Some(mut device) = metal() else { return };
    let mut rng = Rng(0x5EED_0F32_0009_0001);
    let mut seen = Coverage::default();
    let grid = FieldGrid::new(32, 32, 32.0, 32.0).expect("grid");
    let none = Deposits::new();

    // Scattered organisms over mixed energies: grants, emissions and a few
    // recycled residuals.
    let field = field_of(grid, &mut rng, Rng::energy);
    let at = positions(&grid, &mut rng, 1_500);
    let grants = grants_for(&field, &at, &mut rng, |rng| rng.unit() * 2.0);
    let at = positions(&grid, &mut rng, 1_200);
    let emissions = deposits_at(&grid, &at, 5, &mut rng, |rng| rng.unit() * 3.0);
    let at = positions(&grid, &mut rng, 40);
    let recycling = deposits_at(&grid, &at, 2, &mut rng, |rng| rng.unit() * 50.0);
    let scattered = [&emissions, &recycling];
    compare(
        &mut device,
        "scattered",
        &field,
        &grants,
        scattered,
        &mut seen,
    );

    // Oversubscribed cells, which the grants drain, then deposits into them.
    let field = field_of(grid, &mut rng, |rng| (rng.unit() * 3.0) as f32);
    let at = positions(&grid, &mut rng, 4_000);
    let grants = grants_for(&field, &at, &mut rng, |rng| 5.0 + rng.unit() * 50.0);
    let at = positions(&grid, &mut rng, 2_000);
    let emissions = deposits_at(&grid, &at, 5, &mut rng, |rng| rng.unit());
    compare(
        &mut device,
        "drained",
        &field,
        &grants,
        [&emissions, &none],
        &mut seen,
    );

    // A hotspot: thousands of deposits at one cell center (a one-cell
    // footprint) and around it, onto a cell some demands share.
    let field = field_of(grid, &mut rng, |_| 1.75);
    let centre = [10.5, 20.5];
    let mut at = vec![centre; 3_000];
    for index in 0..1_000 {
        let offset = f64::from(index % 40) * 0.01 - 0.2;
        at.push([centre[0] + offset, centre[1] - offset]);
    }
    let grants = grants_for(&field, &at[..500], &mut rng, |rng| rng.unit() * 0.01);
    let emissions = deposits_at(&grid, &at, 5, &mut rng, |rng| rng.unit() * 1.0e-3);
    let recycling = deposits_at(&grid, &at[..100], 2, &mut rng, |rng| rng.unit() * 4.0);
    compare(
        &mut device,
        "hotspot",
        &field,
        &grants,
        [&emissions, &recycling],
        &mut seen,
    );

    // Tiny amounts onto subnormal cells, and huge ones, one lifting a cell
    // past the largest F32 (rounded down to it).
    let field = field_of(grid, &mut rng, |rng| {
        f32::from_bits((rng.next() % 0x007F_FFFF) as u32 + 1)
    });
    let at = positions(&grid, &mut rng, 800);
    let tiny = deposits_at(&grid, &at, 5, &mut rng, |rng| {
        f64::from(f32::from_bits(1)) * 10.0 * rng.unit()
    });
    let huge = deposits_at(&grid, &at[..200], 2, &mut rng, |rng| 1.0e30 * rng.unit());
    let grants = grants_for(&field, &at[..300], &mut rng, |rng| 1.0e-44 * rng.unit());
    compare(
        &mut device,
        "subnormal and huge",
        &field,
        &grants,
        [&tiny, &huge],
        &mut seen,
    );
    let mut full = field_of(grid, &mut rng, |_| 1.0);
    full.energy_mut()[grid.cell_index(3, 3).expect("cell")] = f32::MAX;
    let overflowing = deposits_at(&grid, &[[3.5, 3.5]; 3], 5, &mut rng, |_| 1.0e38);
    let no_grants = grants_for(&full, &[], &mut rng, |_| 0.0);
    compare(
        &mut device,
        "past the largest F32",
        &full,
        &no_grants,
        [&overflowing, &none],
        &mut seen,
    );

    // Sums whose rounding down and to nearest land on either side of an F32:
    // 1 − 2⁻⁵³ then 3·2⁻⁵⁵ into an empty cell total 1 − 2⁻⁵³ rounded down
    // (1 to nearest), whose F32 at or below is the one below 1; and 4 − 2⁻⁵¹
    // into a cell of 4 is 8 − 2⁻⁵⁰ rounded down (8 to nearest, a tie to
    // even), whose F32 is the one below 8.
    let mut exact = field_of(grid, &mut rng, |_| 1.0);
    let (empty, four) = (grid.cell_index(1, 1), grid.cell_index(2, 1));
    let (empty, four) = (empty.expect("cell"), four.expect("cell"));
    exact.energy_mut()[empty] = 0.0;
    exact.energy_mut()[four] = 4.0;
    let mut rounding = Deposits::new();
    for (organism, cell, amount) in [
        (1, empty, below(1.0)),
        (2, empty, 3.0 * 2.0_f64.powi(-55)),
        (3, four, 4.0 - 2.0_f64.powi(-51)),
    ] {
        let direction = rng.direction();
        rounding
            .push_split(
                &grid,
                OrganismId::new(organism),
                amount,
                amount,
                &direction,
                &[(cell, amount)],
            )
            .expect("a one-cell split");
    }
    let no_grants = grants_for(&exact, &[], &mut rng, |_| 0.0);
    let mut reduced = exact.clone();
    reduce_field_on(
        &mut reduced,
        &mut no_grants.clone(),
        [&mut rounding.clone(), &mut Deposits::new()],
        1,
    )
    .expect("the reference reduces");
    assert_eq!(
        [reduced.energy()[empty], reduced.energy()[four]].map(f32::to_bits),
        [1.0_f32.to_bits() - 1, 8.0_f32.to_bits() - 1],
        "the case is not on the rounding boundary"
    );
    compare(
        &mut device,
        "rounding",
        &exact,
        &no_grants,
        [&rounding, &none],
        &mut seen,
    );

    // Each part alone, nothing at all, and every regime at once on a
    // stretched torus, then the first grid again on buffers grown larger.
    let field = field_of(grid, &mut rng, Rng::energy);
    let at = positions(&grid, &mut rng, 600);
    let grants = grants_for(&field, &at, &mut rng, |rng| rng.unit());
    let emissions = deposits_at(&grid, &at, 5, &mut rng, |rng| rng.unit());
    let no_grants = grants_for(&field, &[], &mut rng, |_| 0.0);
    compare(
        &mut device,
        "grants alone",
        &field,
        &grants,
        [&none, &none],
        &mut seen,
    );
    compare(
        &mut device,
        "emissions alone",
        &field,
        &no_grants,
        [&emissions, &none],
        &mut seen,
    );
    compare(
        &mut device,
        "recycling alone",
        &field,
        &no_grants,
        [&none, &emissions],
        &mut seen,
    );
    compare(
        &mut device,
        "nothing",
        &field,
        &no_grants,
        [&none, &none],
        &mut seen,
    );
    let stretched = FieldGrid::new(48, 16, 40.0, 7.0).expect("grid");
    let field = field_of(stretched, &mut rng, Rng::energy);
    let at = positions(&stretched, &mut rng, 3_000);
    let grants = grants_for(&field, &at, &mut rng, |rng| rng.unit() * 3.0);
    let emissions = deposits_at(&stretched, &at, 5, &mut rng, |rng| rng.unit() * 2.0);
    let recycling = deposits_at(&stretched, &at[..64], 2, &mut rng, |rng| rng.unit() * 9.0);
    compare(
        &mut device,
        "stretched",
        &field,
        &grants,
        [&emissions, &recycling],
        &mut seen,
    );
    let field = field_of(grid, &mut rng, Rng::energy);
    let at = positions(&grid, &mut rng, 6_000);
    let grants = grants_for(&field, &at, &mut rng, |rng| rng.unit());
    let emissions = deposits_at(&grid, &at, 5, &mut rng, |rng| rng.unit());
    compare(
        &mut device,
        "grown",
        &field,
        &grants,
        [&emissions, &none],
        &mut seen,
    );

    println!("{seen:?}");
    assert!(seen.packets > 40_000, "{seen:?}");
    assert!(seen.debited > 1_000 && seen.drained > 100, "{seen:?}");
    assert!(seen.shared > 500, "{seen:?}");
}

#[test]
#[expect(clippy::too_many_lines, reason = "each failure, then the refusals")]
fn a_reduction_the_device_cannot_finish_leaves_everything_as_it_was() {
    let Some(mut device) = metal() else { return };
    let mut rng = Rng(0x5EED_0F32_0009_0002);
    let grid = FieldGrid::new(16, 16, 16.0, 16.0).expect("grid");
    let none = Deposits::new();

    // A cell holding a non-finite moment receives a deposit: the device
    // meets it after it has reduced other cells and debited the grants.
    let mut field = field_of(grid, &mut rng, |rng| 0.5 + rng.unit() as f32);
    let cell = grid.cell_index(5, 7).expect("cell");
    field.moments_mut()[3 * grid.cell_count() as usize + cell] = f32::NAN;
    let at = positions(&grid, &mut rng, 400);
    let grants = grants_for(&field, &at, &mut rng, |rng| rng.unit() * 0.1);
    let mut at = positions(&grid, &mut rng, 300);
    at.push([5.5, 7.5]);
    let emissions = deposits_at(&grid, &at, 5, &mut rng, |rng| rng.unit());
    let recycling = deposits_at(&grid, &at[..20], 2, &mut rng, |rng| rng.unit() * 3.0);
    let before = field_bits(&field);
    let (mut got_field, mut got_grants) = (field.clone(), grants.clone());
    let (mut third, mut fourth) = (emissions.clone(), recycling.clone());
    let result = device.reduce(&mut got_field, &mut got_grants, [&mut third, &mut fourth]);
    assert!(
        matches!(result, Err(CoreError::NumericalInvalid { .. })),
        "{result:?}"
    );
    assert_eq!(field_bits(&got_field), before, "the field was written");
    assert!(!got_grants.is_applied() && !third.is_applied() && !fourth.is_applied());
    // Nothing was applied: the reference reduces what the device left as
    // it reduces the untouched inputs.
    let (mut expected_field, mut expected_grants) = (field.clone(), grants.clone());
    let (mut first, mut second) = (emissions.clone(), recycling.clone());
    let expected = reduce_field_on(
        &mut expected_field,
        &mut expected_grants,
        [&mut first, &mut second],
        1,
    )
    .expect("the reference reduces");
    let after = reduce_field_on(
        &mut got_field,
        &mut got_grants,
        [&mut third, &mut fourth],
        1,
    )
    .expect("nothing was applied");
    assert_eq!(
        expected.each_ref().map(summary_bits),
        after.each_ref().map(summary_bits)
    );
    assert_eq!(field_bits(&expected_field), field_bits(&got_field));

    // A non-finite moment in a cell the grants drain is not met: it
    // becomes zero, on both.
    let mut drained = field_of(grid, &mut rng, |rng| (rng.unit() * 0.5) as f32);
    for value in drained.moments_mut() {
        if rng.next().is_multiple_of(50) {
            *value = f32::INFINITY;
        }
    }
    let at = positions(&grid, &mut rng, 2_000);
    let grants = grants_for(&drained, &at, &mut rng, |_| 1.0e6);
    assert!(
        grants
            .debits()
            .iter()
            .all(|debit| debit.energy_after == 0.0)
    );
    let mut seen = Coverage::default();
    compare(
        &mut device,
        "infinities drained",
        &drained,
        &grants,
        [&none, &none],
        &mut seen,
    );

    // The reference's refusals are the device's, with nothing written:
    // grants or deposits already applied, deposits staged for another
    // grid, and one organism depositing twice.
    let field = field_of(grid, &mut rng, |rng| 1.0 + rng.unit() as f32);
    let at = positions(&grid, &mut rng, 100);
    let grants = grants_for(&field, &at, &mut rng, |rng| rng.unit() * 0.1);
    let emissions = deposits_at(&grid, &at, 5, &mut rng, |rng| rng.unit());
    let mut applied_grants = grants.clone();
    let mut scratch = field.clone();
    reduce_field_on(
        &mut scratch,
        &mut applied_grants,
        [&mut Deposits::new(), &mut Deposits::new()],
        1,
    )
    .expect("the reference reduces");
    let mut applied_deposits = emissions.clone();
    reduce_field_on(
        &mut scratch.clone(),
        &mut grants_for(&scratch, &[], &mut rng, |_| 0.0),
        [&mut applied_deposits, &mut Deposits::new()],
        1,
    )
    .expect("the reference reduces");
    let other = FieldGrid::new(8, 8, 8.0, 8.0).expect("grid");
    let foreign = deposits_at(
        &other,
        &positions(&other, &mut rng, 10),
        5,
        &mut rng,
        |_| 1.0,
    );
    let mut twice = Deposits::new();
    for _ in 0..2 {
        twice
            .push(
                &grid,
                OrganismId::new(9),
                [1.5, 1.5],
                1.0,
                0.5,
                &rng.direction(),
            )
            .expect("a valid deposit");
    }
    for (name, grants, deposits) in [
        ("grants applied", &applied_grants, [&emissions, &none]),
        ("deposits applied", &grants, [&applied_deposits, &none]),
        ("another grid", &grants, [&none, &foreign]),
        ("one organism twice", &grants, [&twice, &none]),
    ] {
        let mut got = field.clone();
        let mut got_grants = grants.clone();
        let [mut a, mut b] = deposits.map(Clone::clone);
        let refused = device.reduce(&mut got, &mut got_grants, [&mut a, &mut b]);
        assert!(refused.is_err(), "{name}: {refused:?}");
        assert_eq!(field_bits(&got), field_bits(&field), "{name}: written");
        assert_eq!(got_grants.is_applied(), grants.is_applied(), "{name}");
        assert_eq!(
            [a.is_applied(), b.is_applied()],
            deposits.map(Deposits::is_applied),
            "{name}"
        );
        compare(&mut device, name, &field, grants, deposits, &mut seen);
    }
}
