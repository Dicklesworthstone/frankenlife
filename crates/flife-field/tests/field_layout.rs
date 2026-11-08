//! FL-003 evidence (plan §23.1): actual F32 byte counts and shape, stride and
//! bounds tests, with the §9.3 layout refusals and the INV-01, INV-02 and
//! INV-15 checks.
//!
//! Expected strides are NumPy's: `np.zeros(shape, np.float32).strides`.

use flife_core::{CoreError, ValidatedConfig};
use flife_field::grid::{minimum_image, wrap_coordinate};
use flife_field::layout::{FIELD_LAYOUT_SCHEMA, HOST_ALIGNMENT};
use flife_field::{
    Arrangement, BufferLimits, Cell, ConeTolerance, DeclaredLayout, FIELD_PLANES, FieldFootprint,
    FieldGrid, FieldLayout, FieldPair, FieldState, FieldView, InitialEnergy, relayout,
};
use fnp_dtype::{ArrayStorage, DType};
use fnp_ndarray::{MemoryOrder, NdLayout};

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

const MIB: u64 = 1 << 20;

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn appendix_a() -> ValidatedConfig {
    let appendix = &PLAN[PLAN.find("## Appendix A").expect("Appendix A heading")..];
    let start = appendix.find("```toml\n").expect("Appendix A TOML block") + "```toml\n".len();
    let length = appendix[start..].find("```").expect("closing fence");
    ValidatedConfig::from_toml(&appendix[start..start + length]).expect("Appendix A is valid")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn grid(cells_x: u32, cells_y: u32, width: f64, height: f64) -> FieldGrid {
    FieldGrid::new(cells_x, cells_y, width, height).expect("valid grid")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn layout(grid: FieldGrid, arrangement: Arrangement) -> FieldLayout {
    FieldLayout::new(grid, arrangement).expect("layout")
}

/// A 5 × 3 grid: non-square, so a swapped x/y stride cannot pass.
fn small() -> FieldGrid {
    grid(5, 3, 10.0, 6.0)
}

/// A value that names its own `(plane, x, y)` and is exact in F32.
fn label(plane: u32, x: u32, y: u32) -> f32 {
    f32::from(u16::try_from(plane * 1000 + y * 10 + x).unwrap_or(u16::MAX))
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn labelled(grid: FieldGrid) -> FieldState {
    let mut state = FieldState::zeroed(grid, &BufferLimits::host()).expect("allocate");
    for plane in 0..FIELD_PLANES {
        let image = state.plane_mut(plane).expect("plane");
        for y in 0..grid.cells_y() {
            for x in 0..grid.cells_x() {
                let index = grid.cell_index(x, y).expect("cell");
                image[index] = label(plane, x, y);
            }
        }
    }
    state
}

#[track_caller]
#[expect(
    clippy::panic,
    reason = "test helper: a failure here is a test failure"
)]
fn layout_refusal(result: Result<FieldLayout, CoreError>, needle: &str) {
    match result {
        Err(CoreError::LayoutMismatch { buffer, detail }) => {
            assert_eq!(buffer, "field");
            assert!(detail.contains(needle), "`{detail}` lacks `{needle}`");
        }
        other => panic!("expected a layout refusal containing `{needle}`, got {other:?}"),
    }
}

// --------------------------------------------------------------- byte counts

#[test]
fn plan_19_3_byte_counts_at_1024_squared() {
    let footprint = FieldFootprint::of(&grid(1024, 1024, 1024.0, 1024.0)).expect("footprint");
    assert_eq!(footprint.cells, 1_048_576);
    assert_eq!(footprint.energy_plane_bytes, 4 * MIB);
    assert_eq!(footprint.moment_planes_bytes, 128 * MIB);
    assert_eq!(footprint.state_bytes, 132 * MIB);
    assert_eq!(footprint.pair_bytes, 264 * MIB);
    assert_eq!(footprint.state_bytes, 138_412_032);
    assert_eq!(footprint.pair_bytes, 276_824_064);
}

#[test]
fn actual_f32_allocation_at_1024_squared() {
    let grid = grid(1024, 1024, 1024.0, 1024.0);
    let pair = FieldPair::zeroed(grid, &BufferLimits::host()).expect("allocate the pair");
    let state = pair.current();
    assert_eq!(
        std::mem::size_of_val(&state.as_slice()[0]),
        4,
        "real f32 elements"
    );
    assert_eq!(state.as_slice().len(), 34_603_008);
    let resident = u64::try_from(std::mem::size_of_val(state.as_slice())).expect("bytes");
    assert_eq!(resident, 132 * MIB);
    assert_eq!(state.allocated_bytes(), 132 * MIB);
    assert_eq!(state.layout().byte_length(), 132 * MIB);
    assert_eq!(state.layout().plane_bytes(), 4 * MIB);
    assert_eq!(pair.allocated_bytes(), 264 * MIB);

    // Each plane is one contiguous 4 MiB image at its planar offset.
    let base = state.as_slice().as_ptr().addr();
    for plane in 0..FIELD_PLANES {
        let image = state.plane(plane).expect("plane");
        assert_eq!(image.len(), 1_048_576);
        let offset = u64::try_from(image.as_ptr().addr() - base).expect("offset");
        assert_eq!(offset, u64::from(plane) * 4 * MIB);
        assert_eq!(
            Some(offset),
            state.layout().element_offset(plane, 0, 0),
            "the descriptor locates plane {plane}"
        );
    }
}

#[test]
fn appendix_a_grid_is_128_square_unit_cells() {
    let grid = FieldGrid::from_config(&appendix_a()).expect("Appendix A grid");
    assert_eq!((grid.cells_x(), grid.cells_y()), (128, 128));
    assert_eq!((grid.cell_width(), grid.cell_height()), (1.0, 1.0));
    assert_eq!(grid.cell_area(), 1.0);
    let footprint = FieldFootprint::of(&grid).expect("footprint");
    // FL-001's array probe measured the same 2,162,688 bytes through NumPy.
    assert_eq!(footprint.state_bytes, 2_162_688);
    assert_eq!(footprint.pair_bytes, 4_325_376);
    let state = FieldState::zeroed(grid, &BufferLimits::host()).expect("allocate");
    assert_eq!(state.allocated_bytes(), 2_162_688);
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn appendix_a_with(energy_per_cell: f64) -> ValidatedConfig {
    let mut config = appendix_a().config().clone();
    config.field_initialization.energy_per_cell = energy_per_cell;
    flife_core::validate(config).expect("still valid")
}

#[test]
fn appendix_a_initial_field_is_uniform_with_zero_moments() {
    let (state, energy) =
        FieldState::initial(&appendix_a(), &BufferLimits::host()).expect("initial field");
    assert_eq!(
        energy,
        InitialEnergy {
            declared_per_cell: 0.0,
            stored_per_cell: 0.0,
            injected_total: 0.0,
        }
    );
    assert!(state.as_slice().iter().all(|value| value.to_bits() == 0));

    // A value F32 represents exactly is stored exactly, and the injected
    // total equals the cell-order F64 sum of the stored field.
    let (state, energy) =
        FieldState::initial(&appendix_a_with(0.375), &BufferLimits::host()).expect("initial");
    assert_eq!(energy.stored_per_cell, 0.375);
    assert_eq!(energy.injected_total, 6144.0);
    assert!(state.energy().iter().all(|&value| value == 0.375));
    for channel in 0..32 {
        assert!(
            state
                .moment(channel)
                .expect("moment")
                .iter()
                .all(|&value| value == 0.0)
        );
    }
    let summary = state
        .check_invariants(ConeTolerance::EXACT)
        .expect("invariants");
    assert_eq!(summary.energy_total, energy.injected_total);
    assert_eq!(summary.empty_cells, 0);

    // 0.1 is not an F32 value: the rounding is recorded, never hidden.
    let (state, energy) =
        FieldState::initial(&appendix_a_with(0.1), &BufferLimits::host()).expect("initial");
    assert_eq!(energy.declared_per_cell, 0.1);
    assert_eq!(energy.stored_per_cell, 0.1_f32);
    assert_ne!(f64::from(energy.stored_per_cell), energy.declared_per_cell);
    assert_eq!(energy.injected_total, f64::from(0.1_f32) * 16_384.0);
    let summary = state
        .check_invariants(ConeTolerance::EXACT)
        .expect("invariants");
    assert_eq!(
        summary.energy_total.to_bits(),
        energy.injected_total.to_bits()
    );

    // Beyond F32: refused, not stored as infinity.
    assert!(matches!(
        FieldState::initial(&appendix_a_with(1.0e39), &BufferLimits::host()),
        Err(CoreError::NumericalInvalid {
            quantity: "field_initialization.energy_per_cell",
            ..
        })
    ));
}

// ------------------------------------------------------------------ strides

#[test]
fn strides_match_numpy_and_frankennumpy() {
    let grid = FieldGrid::from_config(&appendix_a()).expect("grid");
    let planar = layout(grid, Arrangement::Planar);
    assert_eq!(planar.shape(), [33, 128, 128]);
    assert_eq!(planar.strides(), [65_536, 512, 4]);
    let interleaved = layout(grid, Arrangement::Interleaved);
    assert_eq!(interleaved.shape(), [128, 128, 33]);
    assert_eq!(interleaved.strides(), [16_896, 132, 4]);
    for layout in [&planar, &interleaved] {
        assert_eq!(layout.byte_length(), 2_162_688);
        assert_eq!(layout.element_count(), 540_672);
        assert_eq!(layout.dtype(), DType::F32);
        let shape = layout
            .shape()
            .map(|extent| usize::try_from(extent).unwrap_or(0));
        let numpy = NdLayout::contiguous(shape.to_vec(), 4, MemoryOrder::C).expect("fnp");
        assert_eq!(layout.nd_layout().expect("nd layout"), numpy);
        assert!(layout.nd_layout().expect("nd").is_writeable());
        assert!(
            !layout
                .as_read_only()
                .nd_layout()
                .expect("nd")
                .is_writeable()
        );
    }
    assert_eq!(layout(small(), Arrangement::Planar).strides(), [60, 20, 4]);
    assert_eq!(
        layout(small(), Arrangement::Interleaved).strides(),
        [660, 132, 4]
    );
}

#[test]
fn every_element_has_one_offset_inside_the_buffer() {
    for arrangement in Arrangement::ALL {
        let layout = layout(small(), arrangement);
        let mut offsets = Vec::new();
        for plane in 0..FIELD_PLANES {
            for y in 0..3 {
                for x in 0..5 {
                    let offset = layout.element_offset(plane, x, y).expect("in range");
                    let expected = match arrangement {
                        Arrangement::Planar => {
                            ((u64::from(plane) * 3 + u64::from(y)) * 5 + u64::from(x)) * 4
                        }
                        _ => ((u64::from(y) * 5 + u64::from(x)) * 33 + u64::from(plane)) * 4,
                    };
                    assert_eq!(offset, expected, "{arrangement} ({plane}, {x}, {y})");
                    offsets.push(offset);
                }
            }
        }
        offsets.sort_unstable();
        let every: Vec<u64> = (0..layout.byte_length()).step_by(4).collect();
        assert_eq!(offsets, every, "{arrangement}: a bijection onto the buffer");
        assert_eq!(
            offsets.last().map(|last| last + HOST_ALIGNMENT),
            Some(layout.byte_length())
        );
    }
}

#[test]
fn out_of_range_coordinates_are_refused() {
    let grid = small();
    let layout = layout(grid, Arrangement::Planar);
    for (plane, x, y) in [
        (33, 0, 0),
        (0, 5, 0),
        (0, 0, 3),
        (u32::MAX, u32::MAX, u32::MAX),
    ] {
        assert_eq!(layout.element_offset(plane, x, y), None);
        assert_eq!(layout.element_index(plane, x, y), None);
    }
    assert_eq!(grid.cell_index(5, 0), None);
    assert_eq!(grid.cell_index(0, 3), None);
    assert_eq!(grid.cell_index(4, 2), Some(14));

    let mut state = labelled(grid);
    let before = state.clone();
    assert_eq!(state.plane(33), None);
    assert_eq!(state.moment(32), None);
    assert_eq!(state.moment(u32::MAX), None);
    assert_eq!(state.cell(5, 0), None);
    assert_eq!(state.set_cell(0, 3, &Cell::EMPTY), None);
    assert_eq!(state, before, "a refused write changes nothing");
    assert_eq!(state.view().get(33, 0, 0), None);
    assert_eq!(state.view().get(0, 5, 0), None);

    let cell = state.cell(4, 2).expect("cell");
    assert_eq!(cell.energy, label(0, 4, 2));
    for (channel, &value) in (0_u32..).zip(cell.moments.iter()) {
        assert_eq!(value, label(channel + 1, 4, 2));
    }
}

// ----------------------------------------------------- incompatible layouts

#[test]
fn equal_length_layouts_are_refused() {
    let planar = layout(small(), Arrangement::Planar);
    let interleaved = layout(small(), Arrangement::Interleaved);
    let transposed = layout(grid(3, 5, 10.0, 6.0), Arrangement::Planar);
    let resized = layout(grid(5, 3, 20.0, 12.0), Arrangement::Planar);
    for other in [&interleaved, &transposed, &resized] {
        assert_eq!(other.byte_length(), planar.byte_length());
    }
    layout_refusal(
        planar
            .ensure_compatible(&interleaved)
            .map(|()| planar.clone()),
        "length never identifies a layout",
    );
    layout_refusal(
        planar
            .ensure_compatible(&transposed)
            .map(|()| planar.clone()),
        "a 3×5 grid where 5×3 is required",
    );
    layout_refusal(
        planar.ensure_compatible(&resized).map(|()| planar.clone()),
        "energy density",
    );
    assert_eq!(planar.ensure_compatible(&planar.as_read_only()), Ok(()));

    // A reader requiring the planar layout refuses interleaved bytes.
    let state = labelled(small());
    let bytes = relayout(&state.view(), &interleaved).expect("relayout");
    let view = FieldView::new(&bytes, &interleaved).expect("view as declared");
    layout_refusal(
        view.require(&planar).map(|()| planar.clone()),
        "interleaved",
    );
    layout_refusal(
        FieldView::new(&bytes[1..], &interleaved).map(|view| view.layout().clone()),
        "1976-byte buffer",
    );
}

#[test]
fn relayout_moves_each_value_to_its_semantic_index() {
    let grid = small();
    let state = labelled(grid);
    let interleaved = layout(grid, Arrangement::Interleaved);
    let moved = relayout(&state.view(), &interleaved).expect("to interleaved");
    for plane in 0..FIELD_PLANES {
        for y in 0..3 {
            for x in 0..5 {
                let index = usize::try_from((y * 5 + x) * 33 + plane).expect("index");
                assert_eq!(moved[index].to_bits(), label(plane, x, y).to_bits());
            }
        }
    }
    let view = FieldView::new(&moved, &interleaved).expect("view");
    let back = relayout(&view, &layout(grid, Arrangement::Planar)).expect("back");
    let original: Vec<u32> = state.as_slice().iter().map(|v| v.to_bits()).collect();
    let round_trip: Vec<u32> = back.iter().map(|v| v.to_bits()).collect();
    assert_eq!(round_trip, original);
    layout_refusal(
        relayout(
            &state.view(),
            &layout(self::grid(3, 5, 10.0, 6.0), Arrangement::Interleaved),
        )
        .map(|_| interleaved.clone()),
        "grid",
    );
}

// ------------------------------------------------------ FrankenNumPy storage

#[test]
fn frankennumpy_storage_moves_without_copying_or_widening() {
    let state = labelled(small());
    let address = state.as_slice().as_ptr().addr();
    let original = state.clone();
    let (storage, layout) = state.into_array_storage();
    assert_eq!(storage.dtype(), DType::F32);
    assert_eq!(storage.len(), 495);
    let back = FieldState::from_array_storage(storage, &layout).expect("round trip");
    assert_eq!(
        back.as_slice().as_ptr().addr(),
        address,
        "moved, not copied"
    );
    assert_eq!(back, original);

    layout_refusal(
        FieldState::from_array_storage(ArrayStorage::F64(vec![0.0; 495]), &layout)
            .map(|state| state.layout().clone()),
        "f64 storage",
    );
    layout_refusal(
        FieldState::from_array_storage(ArrayStorage::F32(vec![0.0; 494]), &layout)
            .map(|state| state.layout().clone()),
        "494 f32 values",
    );
    layout_refusal(
        FieldState::from_array_storage(
            ArrayStorage::F32(vec![0.0; 495]),
            &FieldLayout::new(*layout.grid(), Arrangement::Interleaved).expect("layout"),
        )
        .map(|state| state.layout().clone()),
        "interleaved",
    );
}

// --------------------------------------------------------- declared layouts

#[test]
fn declared_layouts_round_trip() {
    for arrangement in Arrangement::ALL {
        let layout = layout(grid(128, 64, 256.0, 128.0), arrangement);
        let declared = layout.describe();
        assert_eq!(FieldLayout::from_declared(&declared), Ok(layout));
    }
}

type Edit<'a> = &'a dyn Fn(&mut DeclaredLayout);

#[test]
fn declared_layouts_refuse_every_inconsistency() {
    let good = layout(
        FieldGrid::from_config(&appendix_a()).expect("grid"),
        Arrangement::Planar,
    )
    .describe();
    let with = |edit: Edit<'_>| {
        let mut declared = good.clone();
        edit(&mut declared);
        FieldLayout::from_declared(&declared)
    };
    assert_eq!(
        with(&|d| d.schema = "frankenlife.field-layout.v0".into()),
        Err(CoreError::SchemaMismatch {
            expected: FIELD_LAYOUT_SCHEMA.into(),
            found: "frankenlife.field-layout.v0".into()
        })
    );
    let refusals: [(Edit<'_>, &str); 18] = [
        (
            &|d| {
                d.dtype = "f64".into();
                d.item_size = 8;
            },
            "never widened",
        ),
        (&|d| d.item_size = 8, "8-byte items"),
        (&|d| d.dtype = "float32".into(), "dtype `float32`"),
        (&|d| d.arrangement = "tiled-4".into(), "unknown arrangement"),
        (
            &|d| d.axes = vec!["plane".into(), "x".into(), "y".into()],
            "axes",
        ),
        (&|d| d.shape = vec![32, 128, 128], "32 planes"),
        (&|d| d.shape = vec![33, 16_384], "rank-2 shape"),
        (&|d| d.strides = vec![65_536, 512], "2 strides"),
        // Row padding the declared bytes cannot hold.
        (
            &|d| d.strides = vec![66_560, 520, 4],
            "view exceeds base storage",
        ),
        // Row padding the declared bytes do hold: still not canonical.
        (
            &|d| {
                d.strides = vec![66_560, 520, 4];
                d.byte_length = 2_196_480;
            },
            "padded, permuted or reversed",
        ),
        // Fortran order over the same bytes.
        (
            &|d| d.strides = vec![4, 132, 16_896],
            "padded, permuted or reversed",
        ),
        (
            &|d| d.strides = vec![-65_536, 512, 4],
            "padded, permuted or reversed",
        ),
        (&|d| d.strides = vec![0, 512, 4], "share storage"),
        (&|d| d.byte_length = 2_162_684, "view exceeds base storage"),
        (&|d| d.byte_length = 2_162_692, "needs exactly 2162688"),
        (
            &|d| d.byte_length = 2_162_689,
            "whole number of F32 elements",
        ),
        (
            &|d| d.unit = "ecological-energy/world-length^2".into(),
            "planes in",
        ),
        (&|d| d.measure = "density".into(), "planes in"),
    ];
    for (edit, needle) in refusals {
        layout_refusal(with(edit), needle);
    }
    layout_refusal(with(&|d| d.alignment = 16), "16-byte base alignment");
    layout_refusal(
        with(&|d| d.shape = vec![33, 128, 1 << 32]),
        "cells exceed u32",
    );
    assert!(matches!(
        with(&|d| d.world_width = 0.0),
        Err(CoreError::NumericalInvalid { .. })
    ));
    assert!(matches!(
        with(&|d| d.world_height = f64::NAN),
        Err(CoreError::NumericalInvalid { .. })
    ));
    assert!(matches!(
        with(&|d| d.shape = vec![33, u64::from(u32::MAX), u64::from(u32::MAX)]),
        Err(CoreError::Overflow { .. })
    ));
}

#[test]
fn canonical_layout_text_and_digest_are_pinned() {
    let grid = FieldGrid::from_config(&appendix_a()).expect("grid");
    let declared = layout(grid, Arrangement::Planar).describe();
    assert_eq!(
        declared.canonical_text(),
        "schema\tfrankenlife.field-layout.v1\n\
         dtype\tf32\n\
         item_size\t4\n\
         arrangement\tplanar\n\
         axes\tplane,y,x\n\
         shape\t33,128,128\n\
         strides\t65536,512,4\n\
         byte_length\t2162688\n\
         alignment\t4\n\
         unit\tecological-energy\n\
         measure\tcell-integrated\n\
         world_width\t128.0\n\
         world_height\t128.0\n"
    );
    // Independently: `printf '<the text above>' | shasum -a 256`.
    assert_eq!(
        declared.digest().to_string(),
        "sha256:b9297d12b07f1f76df5157732c2a4e94254bbec9cf9d4935eec1f611786326f2"
    );
}

// ------------------------------------------------------------ INV-15 limits

#[test]
fn buffers_are_admitted_against_checked_limits() {
    let grid = FieldGrid::from_config(&appendix_a()).expect("grid");
    assert_eq!(
        FieldState::zeroed(grid, &BufferLimits::new(2_162_687)).map(|_| ()),
        Err(CoreError::BudgetExhausted {
            budget: "field buffer bytes",
            required: 2_162_688,
            available: 2_162_687,
        })
    );
    assert!(FieldState::zeroed(grid, &BufferLimits::new(2_162_688)).is_ok());

    // 2^56 cells need 132 × 2^56 bytes: within u64, beyond isize::MAX, the
    // most a Rust slice or a NumPy array can span. Refused whatever the
    // backend limit says.
    let huge = self::grid(1 << 28, 1 << 28, 1.0, 1.0);
    assert_eq!(
        FieldLayout::planar(huge),
        Err(CoreError::Overflow {
            quantity: "field buffer bytes"
        })
    );
    assert!(matches!(
        FieldState::zeroed(huge, &BufferLimits::new(u64::MAX)),
        Err(CoreError::Overflow { .. })
    ));
    let isize_max = u64::try_from(isize::MAX).expect("64-bit");
    assert_eq!(
        BufferLimits::new(u64::MAX).admit("any buffer", isize_max + 1),
        Err(CoreError::BudgetExhausted {
            budget: "any buffer",
            required: isize_max + 1,
            available: isize_max,
        })
    );
    assert_eq!(BufferLimits::host().max_buffer_bytes(), isize_max);

    let impossible = self::grid(u32::MAX, u32::MAX, 1.0, 1.0);
    assert_eq!(
        FieldFootprint::of(&impossible),
        Err(CoreError::Overflow {
            quantity: "field footprint bytes"
        })
    );
    assert!(matches!(
        FieldState::zeroed(impossible, &BufferLimits::host()),
        Err(CoreError::Overflow { .. })
    ));
}

#[test]
fn grids_refuse_impossible_sides() {
    for (cells_x, cells_y, width, height) in [
        (0, 1, 1.0, 1.0),
        (1, 0, 1.0, 1.0),
        (1, 1, 0.0, 1.0),
        (1, 1, -1.0, 1.0),
        (1, 1, f64::INFINITY, 1.0),
        (1, 1, 1.0, f64::NAN),
        (2, 1, f64::from_bits(1), 1.0),
    ] {
        assert!(
            matches!(
                FieldGrid::new(cells_x, cells_y, width, height),
                Err(CoreError::NumericalInvalid { .. })
            ),
            "{cells_x}×{cells_y} over {width:?}×{height:?}"
        );
    }
}

// --------------------------------------------------------- periodic boundary

#[test]
fn cells_wrap_on_the_torus() {
    let grid = small();
    assert_eq!(grid.wrap_cell(-1, -1), (4, 2));
    assert_eq!(grid.wrap_cell(5, 3), (0, 0));
    assert_eq!(grid.wrap_cell(-6, 7), (4, 1));
    let (x, y) = grid.wrap_cell(i64::MIN, i64::MAX);
    assert_eq!(
        (i64::from(x), i64::from(y)),
        (i64::MIN.rem_euclid(5), i64::MAX.rem_euclid(3))
    );
}

#[test]
fn coordinates_wrap_into_the_half_open_period() {
    let cases: [(f64, f64); 7] = [
        (-1.0e-20, 0.0),
        (-0.0, 0.0),
        (0.0, 0.0),
        (128.0, 0.0),
        (300.5, 44.5),
        (-0.5, 127.5),
        (127.999, 127.999),
    ];
    for (input, expected) in cases {
        let wrapped = wrap_coordinate(input, 128.0).expect("finite");
        assert_eq!(wrapped.to_bits(), expected.to_bits(), "{input:?}");
        assert!((0.0..128.0).contains(&wrapped));
    }
    for (input, length) in [
        (f64::NAN, 1.0),
        (f64::INFINITY, 1.0),
        (1.0, 0.0),
        (1.0, -1.0),
    ] {
        assert!(wrap_coordinate(input, length).is_err());
    }
    let grid = small();
    assert_eq!(grid.wrap_position([-1.0, 6.5]).expect("wrap"), [9.0, 0.5]);
}

#[test]
fn minimum_image_is_half_open_and_adds_no_rounding() {
    assert_eq!(minimum_image(1.0, 127.0, 128.0).expect("in range"), -2.0);
    assert_eq!(minimum_image(127.0, 1.0, 128.0).expect("in range"), 2.0);
    assert_eq!(minimum_image(0.0, 64.0, 128.0).expect("in range"), -64.0);
    assert_eq!(minimum_image(64.0, 0.0, 128.0).expect("in range"), -64.0);
    assert_eq!(minimum_image(0.25, 0.75, 1.0).expect("in range"), -0.5);
    for (from, to) in [(128.0, 0.0), (0.0, -0.5), (f64::NAN, 0.0)] {
        assert!(minimum_image(from, to, 128.0).is_err());
    }
    // A deterministic sweep: the wrap differs from the rounded difference
    // by exactly 0 or ±L, and lands in [-L/2, L/2).
    let length = 100.0_f64;
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let unit = f64::from(u32::try_from(state >> 32).unwrap_or(0)) / 4_294_967_296.0;
        unit * length
    };
    for _ in 0..10_000 {
        let (from, to) = (next(), next());
        let image = minimum_image(from, to, length).expect("in range");
        let difference = to - from;
        let correction = image - difference;
        assert!(
            [0.0, length, -length].contains(&correction),
            "{from:?} → {to:?}: {image:?} vs {difference:?}"
        );
        assert!((-0.5 * length..0.5 * length).contains(&image));
    }
    assert_eq!(
        small()
            .displacement([9.5, 0.5], [0.5, 5.5])
            .expect("displacement"),
        [1.0, -1.0]
    );
}

// --------------------------------------------------------------- ping-pong

#[test]
fn commit_swaps_buffers_without_reallocating() {
    let mut pair = FieldPair::zeroed(small(), &BufferLimits::host()).expect("pair");
    let bytes = pair.allocated_bytes();
    let (current, next) = pair.stage();
    let (current_address, next_address) = (
        current.as_slice().as_ptr().addr(),
        next.as_slice().as_ptr().addr(),
    );
    next.energy_mut()[0] = 3.0;
    next.copy_from(current).expect("same layout");
    next.energy_mut()[7] = 2.0;
    pair.commit();
    assert_eq!(pair.current().energy()[7], 2.0);
    assert_eq!(pair.current().as_slice().as_ptr().addr(), next_address);
    let (_, candidate) = pair.stage();
    assert_eq!(candidate.as_slice().as_ptr().addr(), current_address);
    assert_eq!(pair.allocated_bytes(), bytes);

    let mut other =
        FieldState::zeroed(grid(3, 5, 10.0, 6.0), &BufferLimits::host()).expect("other");
    layout_refusal(
        other
            .copy_from(pair.current())
            .map(|()| other.layout().clone()),
        "grid",
    );
}

// ---------------------------------------------------------- INV-01, INV-02

#[test]
fn committed_fields_obey_energy_and_cone_invariants() {
    let mut state = FieldState::zeroed(small(), &BufferLimits::host()).expect("allocate");
    let empty = state
        .check_invariants(ConeTolerance::EXACT)
        .expect("zero field");
    assert_eq!(empty.energy_total, 0.0);
    assert_eq!(empty.empty_cells, 15);
    assert_eq!(empty.max_cone_residual, 0.0);
    assert_eq!(empty.max_cone_residual_cell, None);

    // ||m|| = e exactly: on the cone, admissible with no tolerance.
    let mut cell = Cell::EMPTY;
    cell.energy = 1.0;
    cell.moments[..4].copy_from_slice(&[0.5, -0.5, 0.5, -0.5]);
    state.set_cell(1, 2, &cell).expect("in range");
    let mut far = Cell::EMPTY;
    far.energy = 2.0;
    state.set_cell(4, 0, &far).expect("in range");
    let mut third = Cell::EMPTY;
    third.energy = 3.0;
    third.moments[31] = -3.0;
    state.set_cell(0, 1, &third).expect("in range");
    let summary = state
        .check_invariants(ConeTolerance::EXACT)
        .expect("on the cone");
    assert_eq!(summary.energy_total, 6.0);
    assert_eq!(summary.empty_cells, 12);
    assert_eq!(summary.max_cone_residual, 0.0);

    // ||m||² = 1 + 2^-24: just outside the cone.
    cell.moments[4] = 2.0_f32.powi(-12);
    state.set_cell(1, 2, &cell).expect("in range");
    assert!(matches!(
        state.check_invariants(ConeTolerance::EXACT),
        Err(CoreError::NumericalInvalid { quantity: "chemical moment cone", detail })
            if detail.contains("cell (1, 2)")
    ));
    let tolerant = ConeTolerance::new(1.0e-7, 0.0).expect("tolerance");
    let summary = state.check_invariants(tolerant).expect("within tolerance");
    assert_eq!(summary.max_cone_residual_cell, Some((1, 2)));
    assert!(summary.max_cone_residual > 0.0 && summary.max_cone_residual < 1.0e-7);
    let relative = ConeTolerance::new(0.0, 1.0e-7).expect("tolerance");
    assert!(state.check_invariants(relative).is_ok());

    for (energy, moment, quantity) in [
        (-1.0e-30, 0.0, "field energy"),
        (f32::NAN, 0.0, "field energy"),
        (f32::INFINITY, 0.0, "field energy"),
        (1.0, f32::NAN, "field moment"),
        (1.0, f32::NEG_INFINITY, "field moment"),
    ] {
        let mut bad = state.clone();
        let mut cell = Cell::EMPTY;
        cell.energy = energy;
        cell.moments[9] = moment;
        bad.set_cell(3, 1, &cell).expect("in range");
        assert!(
            matches!(
                bad.check_invariants(tolerant),
                Err(CoreError::NumericalInvalid { quantity: q, detail })
                    if q == quantity && detail.contains("cell (3, 1)")
            ),
            "{energy:?}/{moment:?}"
        );
    }
    // Negative zero is zero energy, not a negative one.
    let mut signed_zero = state.clone();
    let mut cell = Cell::EMPTY;
    cell.energy = -0.0;
    signed_zero.set_cell(3, 1, &cell).expect("in range");
    assert!(signed_zero.check_invariants(tolerant).is_ok());

    for (absolute, relative) in [(-1.0, 0.0), (0.0, f64::NAN), (f64::INFINITY, 0.0)] {
        assert!(ConeTolerance::new(absolute, relative).is_err());
    }
}
