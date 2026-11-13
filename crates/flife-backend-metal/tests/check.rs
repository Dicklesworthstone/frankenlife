//! FL-032: phase 10's field check on Metal is flife-field's
//! [`FieldState::scan_invariants`], bit for bit: the first cell that fails
//! INV-01 or INV-02, else the first cell with the largest positive cone
//! residual, its residual compared as bits, and the summary
//! [`FieldState::summarize_scan`] builds from it is
//! [`FieldState::check_invariants`]'s. The cases are random fields with
//! cells inside, on and just outside the cone over energies from subnormal
//! to 1e30, every kind of failing cell in turn and in combination, residuals
//! exactly at the tolerance's bound and one ulp past it, ties within one
//! threadgroup and across several, grids whose cells do not fill the last
//! threadgroup, and several tolerances. A tolerance whose bound could
//! overflow is refused.

use flife_backend_metal::MetalField;
use flife_core::CoreError;
use flife_field::state::{CHANNELS, Cell};
use flife_field::{
    BufferLimits, ConeTolerance, FieldBackend, FieldGrid, FieldScan, FieldState, FieldSummary,
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

    /// A cell holding `energy` with moments of norm about `energy · scale`
    /// over a few channels, some zero of either sign.
    fn cell(&mut self, energy: f32, scale: f64) -> Cell {
        let mut cell = Cell::EMPTY;
        cell.energy = energy;
        let mut direction = [0.0_f64; CHANNELS];
        let mut norm = 0.0_f64;
        for component in &mut direction {
            if self.next().is_multiple_of(3) {
                *component = self.unit() * 2.0 - 1.0;
                norm += *component * *component;
            }
        }
        let norm = norm.sqrt();
        for (moment, component) in cell.moments.iter_mut().zip(direction) {
            *moment = if norm > 0.0 {
                (component / norm * f64::from(energy) * scale) as f32
            } else if self.next().is_multiple_of(2) {
                -0.0
            } else {
                0.0
            };
        }
        cell
    }
}

#[expect(clippy::expect_used, reason = "test helper")]
fn grid(cells_x: u32, cells_y: u32) -> FieldGrid {
    FieldGrid::new(cells_x, cells_y, f64::from(cells_x), f64::from(cells_y)).expect("grid")
}

/// A field of `grid` whose cells are inside the cone (most), on it, just
/// outside it, or empty.
#[expect(clippy::expect_used, reason = "test helper")]
fn random_field(grid: FieldGrid, rng: &mut Rng) -> FieldState {
    let mut field = FieldState::zeroed(grid, &LIMITS).expect("field");
    for y in 0..grid.cells_y() {
        for x in 0..grid.cells_x() {
            let energy = rng.energy();
            let scale = match rng.next() % 8 {
                0 => 1.0,
                1 => 1.0 + 1.0e-7 * rng.unit(),
                2 => 0.0,
                _ => rng.unit(),
            };
            let cell = rng.cell(energy, scale);
            field.set_cell(x, y, &cell).expect("in range");
        }
    }
    field
}

type ScanBits = (Option<usize>, Option<(usize, u64)>);

fn scan_bits(scan: &FieldScan) -> ScanBits {
    (
        scan.offending,
        scan.largest
            .map(|(cell, residual)| (cell, residual.to_bits())),
    )
}

fn summary_bits(summary: &FieldSummary) -> (u64, u64, Option<(u32, u32)>, u64) {
    (
        summary.energy_total.to_bits(),
        summary.max_cone_residual.to_bits(),
        summary.max_cone_residual_cell,
        summary.empty_cells,
    )
}

/// What the cases exercised.
#[derive(Debug, Default)]
struct Coverage {
    scans: usize,
    failing: usize,
    largest: usize,
}

/// Scan `field` at `tolerance` on the device and on the reference and
/// compare them; for a clean field, the summary from the device's scan
/// must be the check's.
#[expect(clippy::expect_used, reason = "test helper")]
fn compare(
    device: &mut MetalField,
    name: &str,
    field: &FieldState,
    tolerance: ConeTolerance,
    seen: &mut Coverage,
) -> FieldScan {
    let expected = field.scan_invariants(tolerance);
    let got = device.check(field, tolerance).expect("the device checks");
    assert_eq!(scan_bits(&got), scan_bits(&expected), "{name}");
    seen.scans += 1;
    seen.failing += usize::from(got.offending.is_some());
    seen.largest += usize::from(got.largest.is_some());
    match field.check_invariants(tolerance) {
        Ok(check) => {
            let summary = field
                .summarize_scan(tolerance, &got)
                .expect("the device's scan is the field's")
                .expect("no failing cell");
            assert_eq!(summary_bits(&summary), summary_bits(&check), "{name}");
        }
        Err(_) => assert!(got.offending.is_some(), "{name}: the check fails"),
    }
    got
}

#[test]
#[expect(clippy::too_many_lines, reason = "one case after another")]
fn the_metal_check_is_the_reference_scan_bit_for_bit() {
    let Some(mut device) = metal() else { return };
    let mut rng = Rng(0x5EED_0F32_0010_0001);
    let mut seen = Coverage::default();
    let tolerances = [
        ConeTolerance::EXACT,
        ConeTolerance::new(1.0e-7, 0.0).expect("tolerance"),
        ConeTolerance::new(0.0, 2.0e-7).expect("tolerance"),
        ConeTolerance::new(1.0e-30, 1.0e-6).expect("tolerance"),
        ConeTolerance::new(1.0e-6, 1.0e-6).expect("tolerance"),
    ];

    // Random fields on grids that fill their last threadgroup and do not
    // (851 cells: three whole groups of 256 and 83 cells), at each
    // tolerance.
    for (cells_x, cells_y) in [(64, 64), (37, 23), (128, 128), (1, 1)] {
        for round in 0..3 {
            let field = random_field(grid(cells_x, cells_y), &mut rng);
            for (index, &tolerance) in tolerances.iter().enumerate() {
                let name = format!("{cells_x}×{cells_y} round {round} tolerance {index}");
                compare(&mut device, &name, &field, tolerance, &mut seen);
            }
        }
    }

    // Every kind of failing cell, alone, at cells in several threadgroups,
    // then two at once (the first wins), into a field inside the cone.
    let shape = grid(37, 23);
    let mut calm = FieldState::zeroed(shape, &LIMITS).expect("field");
    for y in 0..shape.cells_y() {
        for x in 0..shape.cells_x() {
            let energy = (rng.unit() * 4.0) as f32;
            let scale = rng.unit() * 0.5;
            calm.set_cell(x, y, &rng.cell(energy, scale))
                .expect("in range");
        }
    }
    let loose = tolerances[1];
    // Each moment is set in its channel and the next (`2^-12` past the cone
    // is one channel's).
    let failing: [(&str, f32, usize, f32, f32); 10] = [
        ("a NaN energy", f32::NAN, 0, 0.0, 0.0),
        ("an infinite energy", f32::INFINITY, 0, 0.0, 0.0),
        ("a negative energy", -1.0, 0, 0.0, 0.0),
        (
            "a negative subnormal energy",
            -f32::from_bits(1),
            0,
            0.0,
            0.0,
        ),
        ("a NaN moment", 1.0, 30, 0.0, f32::NAN),
        ("an infinite moment", 1.0, 0, f32::NEG_INFINITY, 0.0),
        ("a moment past the cone", 1.0, 7, 2.0, 0.0),
        (
            "a moment past the cone by 2^-12",
            1.0,
            0,
            1.0,
            2.0_f32.powi(-12),
        ),
        ("two largest moments", f32::MAX, 1, f32::MAX, -f32::MAX),
        (
            "subnormals past their energy",
            f32::from_bits(3),
            2,
            f32::from_bits(5),
            0.0,
        ),
    ];
    for (name, energy, channel, moment, next) in failing {
        for (x, y) in [(0, 0), (36, 6), (7, 14), (36, 22)] {
            let mut bad = calm.clone();
            let mut cell = Cell::EMPTY;
            cell.energy = energy;
            cell.moments[channel] = moment;
            cell.moments[channel + 1] = next;
            bad.set_cell(x, y, &cell).expect("in range");
            let scan = compare(&mut device, name, &bad, ConeTolerance::EXACT, &mut seen);
            assert_eq!(scan.offending, shape.cell_index(x, y), "{name}");
        }
    }
    let mut twice = calm.clone();
    let mut nan = Cell::EMPTY;
    nan.energy = f32::NAN;
    twice.set_cell(30, 20, &nan).expect("in range");
    let mut over = Cell::EMPTY;
    over.energy = 1.0;
    over.moments[4] = 3.0;
    twice.set_cell(3, 2, &over).expect("in range");
    let scan = compare(&mut device, "two failing cells", &twice, loose, &mut seen);
    assert_eq!(scan.offending, shape.cell_index(3, 2));
    // Negative zero is zero energy, not a negative one.
    let mut signed = calm.clone();
    let mut zero = Cell::EMPTY;
    zero.energy = -0.0;
    zero.moments[5] = -0.0;
    signed.set_cell(9, 9, &zero).expect("in range");
    let scan = compare(&mut device, "negative zero", &signed, loose, &mut seen);
    assert_eq!(scan.offending, None);

    // A residual exactly at the bound passes; one ulp less bound fails.
    // ||m|| = 1.5 over e = 1 is a residual of exactly 0.5.
    let mut edge = calm.clone();
    let mut cell = Cell::EMPTY;
    cell.energy = 1.0;
    cell.moments[0] = 1.5;
    edge.set_cell(20, 10, &cell).expect("in range");
    let at = ConeTolerance::new(0.5, 0.0).expect("tolerance");
    let scan = compare(&mut device, "at the bound", &edge, at, &mut seen);
    assert_eq!(
        scan_bits(&scan),
        (
            None,
            Some((shape.cell_index(20, 10).expect("cell"), 0.5_f64.to_bits()))
        )
    );
    let below = ConeTolerance::new(f64::from_bits(0.5_f64.to_bits() - 1), 0.0).expect("tolerance");
    let scan = compare(&mut device, "past the bound", &edge, below, &mut seen);
    assert_eq!(scan.offending, shape.cell_index(20, 10));
    let relative = ConeTolerance::new(0.0, 0.5).expect("tolerance");
    compare(
        &mut device,
        "at a relative bound",
        &edge,
        relative,
        &mut seen,
    );

    // Ties: equal residuals in one threadgroup, the later at the lower
    // lane of a reduction step, and across threadgroups; the first cell
    // holds the largest.
    let mut tied = calm.clone();
    let mut over = Cell::EMPTY;
    over.energy = 1.0;
    over.moments[0] = 1.0;
    over.moments[1] = 2.0_f32.powi(-12);
    for (x, y) in [(10, 8), (2, 8), (35, 15), (1, 21)] {
        tied.set_cell(x, y, &over).expect("in range");
    }
    let scan = compare(&mut device, "ties", &tied, loose, &mut seen);
    assert_eq!(scan.largest.map(|(cell, _)| cell), shape.cell_index(2, 8));

    // The multi-range field of flife-field's worker test: 160 × 128 cells,
    // equal largest residuals 9,600 cells apart.
    let mut ranged = FieldState::zeroed(grid(160, 128), &LIMITS).expect("field");
    for y in 0..128 {
        for x in 0..160 {
            if (x + y) % 7 != 0 {
                let energy = (rng.unit() * 4.0) as f32;
                ranged
                    .set_cell(x, y, &rng.cell(energy, 0.5))
                    .expect("in range");
            }
        }
    }
    ranged.set_cell(5, 40, &over).expect("in range");
    ranged.set_cell(70, 100, &over).expect("in range");
    let scan = compare(&mut device, "ranged", &ranged, loose, &mut seen);
    assert_eq!(scan.largest.map(|(cell, _)| cell), Some(40 * 160 + 5));

    println!("{seen:?}");
    assert!(
        seen.scans > 100 && seen.failing > 60 && seen.largest > 20,
        "{seen:?}"
    );
}

#[test]
fn a_bound_past_binary64_is_refused() {
    let Some(mut device) = metal() else { return };
    let field = FieldState::zeroed(grid(4, 4), &LIMITS).expect("field");
    let huge = ConeTolerance::new(0.0, 1.0e300).expect("a valid tolerance");
    assert!(matches!(
        device.check(&field, huge),
        Err(CoreError::MissingCapability { .. })
    ));
    // The reference still checks it.
    assert!(field.check_invariants(huge).is_ok());
}
