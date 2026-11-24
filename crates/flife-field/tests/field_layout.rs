//! FL-003 evidence (plan §23.1): actual F32 byte counts and shape, stride and
//! bounds tests, with the §9.3 layout refusals and the INV-01, INV-02 and
//! INV-15 checks.
//!
//! Expected strides are NumPy's: `np.zeros(shape, np.float32).strides`.

use flife_core::ids::SourceId;
use flife_core::{CoreError, TimeStep, ValidatedConfig};
use flife_field::grid::{minimum_image, wrap_coordinate};
use flife_field::layout::{FIELD_LAYOUT_SCHEMA, HOST_ALIGNMENT};
use flife_field::sources::unit_direction;
use flife_field::state::CHANNELS;
use flife_field::{
    Arrangement, BufferLimits, CARRIER1_ENERGY_PLANE, CARRIER1_FIRST_MOMENT_PLANE, Cell,
    ConeContractionSummary, ConeTolerance, DUAL_CARRIER_PLANES, DeclaredLayout, FIELD_PLANES,
    FieldFootprint, FieldGrid, FieldLayout, FieldPair, FieldState, FieldSummary, FieldView,
    InitialEnergy, Source, SourceKernel, SourceSupport, TransportPlan, TransportScratch,
    advance_field, relayout,
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

/// `cell_at` reads each cell's own value in every plane, as `cell` and
/// `moment` do, and refuses an index past the grid.
#[test]
fn a_cell_by_index_is_the_cell_by_coordinates() {
    let grid = small();
    let state = labelled(grid);
    for y in 0..grid.cells_y() {
        for x in 0..grid.cells_x() {
            let index = grid.cell_index(x, y).expect("cell");
            let cell = state.cell_at(index).expect("in the grid");
            assert_eq!(Some(cell), state.cell(x, y));
            assert_eq!(cell.energy, label(0, x, y));
            for (channel, &value) in (0_u32..).zip(cell.moments.iter()) {
                assert_eq!(value, label(channel + 1, x, y));
            }
        }
    }
    let cells = usize::try_from(grid.cell_count()).expect("fits");
    assert_eq!(state.cell_at(cells), None);
    assert_eq!(state.cell_at(usize::MAX), None);
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

/// A summary's every field, its sums as bits.
fn summary_bits(summary: &FieldSummary) -> (u64, u64, Option<(u32, u32)>, u64) {
    (
        summary.energy_total.to_bits(),
        summary.max_cone_residual.to_bits(),
        summary.max_cone_residual_cell,
        summary.empty_cells,
    )
}

/// The quantity and detail of a numerical refusal.
#[expect(
    clippy::panic,
    reason = "test helper: a failure here is a test failure"
)]
fn refusal(result: Result<FieldSummary, CoreError>) -> (&'static str, String) {
    match result {
        Err(CoreError::NumericalInvalid { quantity, detail }) => (quantity, detail),
        other => panic!("expected a numerical refusal, got {other:?}"),
    }
}

/// Phase 10 checks the next field on the field's workers (C5): the cells
/// are split into contiguous ranges, each checked by its own thread. Every
/// worker count must give one thread's summary to the bit, keep the first
/// cell of a largest residual tied across ranges, and report the first
/// offending cell in cell-index order when several ranges hold one. The
/// 160 × 128 grid's 20,480 cells make at most five ranges; the counts
/// include zero (taken as one) and more workers than ranges.
#[test]
fn invariants_check_the_same_on_any_number_of_workers() {
    let state = ranged_field();
    let tolerant = ConeTolerance::new(1.0e-7, 0.0).expect("tolerance");
    let one = state.check_invariants(tolerant).expect("within tolerance");
    assert_eq!(one.max_cone_residual_cell, Some((5, 40)));
    assert!(one.empty_cells > 0 && one.energy_total > 0.0);
    let expected = summary_bits(&one);
    let counts = [0, 1, 2, 3, 4, 5, 6, 16, 64];
    for workers in counts {
        let summary = state
            .check_invariants_on(tolerant, workers)
            .expect("within tolerance");
        assert_eq!(summary_bits(&summary), expected, "{workers} workers");
    }

    // A nonfinite moment in the last range and a negative energy in the
    // third: within the tolerance the negative energy is first; with no
    // tolerance the slight cone excess in the first range is.
    let bad = ranged_failures(&state);
    for (tolerance, quantity, cell) in [
        (tolerant, "field energy", "cell (30, 70)"),
        (ConeTolerance::EXACT, "chemical moment cone", "cell (10, 3)"),
    ] {
        let expected = refusal(bad.check_invariants(tolerance));
        assert_eq!(expected.0, quantity);
        assert!(expected.1.contains(cell), "{}", expected.1);
        for workers in counts {
            assert_eq!(
                refusal(bad.check_invariants_on(tolerance, workers)),
                expected,
                "{workers} workers"
            );
        }
    }
}

/// FL-032's scan for a device phase 10: `scan_invariants` names the first
/// failing cell, or the first cell with the largest residual, as
/// `check_invariants` does, and `summarize_scan` turns a scan into
/// `check_invariants`'s summary to the bit, refusing one that cannot be the
/// field's.
#[test]
fn a_scan_is_the_check_and_its_summary_the_checks() {
    let state = ranged_field();
    let width = 160;
    let at = |x: usize, y: usize| y * width + x;
    let tolerant = ConeTolerance::new(1.0e-7, 0.0).expect("tolerance");
    let one = state.check_invariants(tolerant).expect("within tolerance");
    let scan = state.scan_invariants(tolerant);
    assert_eq!(scan.offending, None);
    let (largest, residual) = scan.largest.expect("a positive residual");
    assert_eq!(
        (largest, residual.to_bits()),
        (at(5, 40), one.max_cone_residual.to_bits())
    );
    let summary = state
        .summarize_scan(tolerant, &scan)
        .expect("the field's own scan")
        .expect("no failing cell");
    assert_eq!(summary_bits(&summary), summary_bits(&one));

    // On the cone everywhere: no largest residual, and the summary still
    // the check's.
    let mut exact = FieldState::zeroed(*state.grid(), &BufferLimits::host()).expect("allocate");
    let mut cell = Cell::EMPTY;
    cell.energy = 2.0;
    cell.moments[3] = -2.0;
    exact.set_cell(7, 9, &cell).expect("in range");
    let exact_scan = exact.scan_invariants(ConeTolerance::EXACT);
    assert_eq!(
        (exact_scan.offending, exact_scan.largest),
        (None, None),
        "{exact_scan:?}"
    );
    let summary = exact
        .summarize_scan(ConeTolerance::EXACT, &exact_scan)
        .expect("the field's own scan")
        .expect("no failing cell");
    assert_eq!(
        summary_bits(&summary),
        summary_bits(&exact.check_invariants(ConeTolerance::EXACT).expect("on it"))
    );

    // The first failing cell, whatever fails after it.
    let bad = ranged_failures(&state);
    for (tolerance, first) in [(tolerant, at(30, 70)), (ConeTolerance::EXACT, at(10, 3))] {
        let scan = bad.scan_invariants(tolerance);
        assert_eq!((scan.offending, scan.largest), (Some(first), None));
        assert!(matches!(bad.summarize_scan(tolerance, &scan), Ok(None)));
    }

    // Scans that cannot be this field's.
    let ulp = f64::from_bits(residual.to_bits() + 1);
    let cells = 160 * 128;
    let refused = [
        ("a residual one ulp off", None, Some((largest, ulp))),
        ("another cell's residual", None, Some((at(10, 3), residual))),
        ("a zero residual", None, Some((at(0, 0), 0.0))),
        ("a largest cell outside", None, Some((cells, residual))),
        ("a failing cell outside", Some(cells), None),
    ];
    for (name, offending, largest) in refused {
        let scan = flife_field::FieldScan { offending, largest };
        assert!(
            matches!(
                state.summarize_scan(tolerant, &scan),
                Err(CoreError::StateConflict {
                    operation: "field check backend",
                    ..
                })
            ),
            "{name}"
        );
    }
    // A clean scan of a field whose largest cell, or one energy, fails.
    let mut failing = state.clone();
    let mut over = Cell::EMPTY;
    over.energy = 1.0;
    over.moments[0] = 2.0;
    failing.set_cell(5, 40, &over).expect("in range");
    let negative = ranged_failures(&state);
    for (name, field) in [("its largest cell", &failing), ("an energy", &negative)] {
        assert!(
            matches!(
                field.summarize_scan(tolerant, &scan),
                Err(CoreError::StateConflict { .. })
            ),
            "{name}"
        );
    }
}

/// [`invariants_check_the_same_on_any_number_of_workers`]'s field: 160 ×
/// 128 cells, most holding energy and one moment inside the cone, two just
/// outside it with the same residual (cells (5, 40) and (70, 100)), and a
/// slighter one at (10, 3).
#[expect(clippy::expect_used, reason = "test helper")]
fn ranged_field() -> FieldState {
    let grid = grid(160, 128, 160.0, 128.0);
    let mut state = FieldState::zeroed(grid, &BufferLimits::host()).expect("allocate");
    let mut seed = 0x5eed_u64;
    let mut next = move || {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        f64::from(u32::try_from(seed >> 40).unwrap_or(0)) / 16_777_216.0
    };
    for y in 0..128_u32 {
        for x in 0..160_u32 {
            if (x + y) % 7 == 0 {
                continue;
            }
            let mut cell = Cell::EMPTY;
            cell.energy = (next() * 4.0) as f32;
            cell.moments[(x as usize * 3 + y as usize) % 32] =
                cell.energy * 0.5 * (next() as f32 - 0.5);
            state.set_cell(x, y, &cell).expect("in range");
        }
    }
    // Just outside the cone: two cells with the same residual in the second
    // and fourth of five ranges, and a smaller one in the first.
    let mut over = Cell::EMPTY;
    over.energy = 1.0;
    over.moments[0] = 1.0;
    over.moments[1] = 2.0_f32.powi(-12);
    let mut slight = over;
    slight.moments[1] = 2.0_f32.powi(-13);
    state.set_cell(10, 3, &slight).expect("in range");
    state.set_cell(5, 40, &over).expect("in range");
    state.set_cell(70, 100, &over).expect("in range");
    state
}

/// `state` with a nonfinite moment in the last range, at (150, 120), and a
/// negative energy in the third, at (30, 70).
#[expect(clippy::expect_used, reason = "test helper")]
fn ranged_failures(state: &FieldState) -> FieldState {
    let mut bad = state.clone();
    let mut nan = Cell::EMPTY;
    nan.energy = 1.0;
    nan.moments[9] = f32::NAN;
    bad.set_cell(150, 120, &nan).expect("in range");
    let mut negative = Cell::EMPTY;
    negative.energy = -1.0;
    bad.set_cell(30, 70, &negative).expect("in range");
    bad
}

/// Plan §11.5 bounded moment-only cone contraction:
/// - `check_invariants_with_trigger_on` matches `check_invariants_on` bit-for-bit
///   and tracks `any_exceeds_trigger` accurately even when the largest absolute
///   residual cell is below the relative trigger and a smaller-energy cell is above.
/// - `contract_cone_on` never mutates `e_0` or `e_1` (0 bits touched), leaves
///   cells at or below `trigger` bit-for-bit untouched, brings every contracted
///   cell strictly inside `||m||₂ ≤ e` (`ConeTolerance::EXACT`), works on both
///   33-plane single-carrier and 34-plane dual-carrier (`V2-15`) fields, handles
///   `e == 0`, subnormals, `-0.0`, and near-`f32::MAX`, and is bit-identical
///   across worker counts `[0, 1, 2, 4, 8]`.
#[test]
fn plan_11_5_cone_contraction_is_exact_energy_preserving_dual_carrier_and_worker_invariant() {
    let g = grid(160, 128, 160.0, 128.0);
    let limits = BufferLimits::host();
    let mut base = FieldState::zeroed(g, &limits).expect("allocate");

    let mut raw_dir = [0.0_f64; CHANNELS];
    for (k, slot) in raw_dir.iter_mut().enumerate() {
        *slot = (((k * 17 + 5) % 23) as f64 - 11.0) / 11.0;
    }
    let dir = unit_direction(&raw_dir).expect("unit dir");

    // Cell (10, 3): tiny residual (~3e-8 <= 1e-6) — must remain bit-for-bit untouched.
    let mut below_small = Cell::EMPTY;
    below_small.energy = 1.0;
    below_small.moments[0] = 1.0;
    below_small.moments[1] = 2.0_f32.powi(-12);
    base.set_cell(10, 3, &below_small).expect("set");

    // Cell (12, 50): e = 10.0, residual ~3e-6 (rel ~3e-7 <= 1e-6) — larger ABSOLUTE
    // residual than (5, 40), yet still below the relative trigger! Must stay untouched.
    let mut below_large_abs = Cell::EMPTY;
    below_large_abs.energy = 10.0;
    below_large_abs.moments[0] = 10.0;
    below_large_abs.moments[1] = (20.0_f64 * 3.0e-6).sqrt() as f32;
    base.set_cell(12, 50, &below_large_abs).expect("set");

    // Cells (5, 40) and (70, 100) in distinct worker ranges: e = 0.16908655,
    // rel excess ~4e-6 (above 1e-6 trigger, below 1e-5 INV-02 gate).
    let mut above = Cell::EMPTY;
    above.energy = 0.169_086_55_f32;
    let boost = f64::from(above.energy) * (1.0 + 4.0e-6);
    for (slot, &c) in above.moments.iter_mut().zip(&dir) {
        *slot = (boost * c) as f32;
    }
    base.set_cell(5, 40, &above).expect("set");
    base.set_cell(70, 100, &above).expect("set");

    // Cell (20, 60): e = 0.0 with tiny dust moment above trigger's absolute floor
    // (checked under a looser tolerance floor so the `e == 0` zeroing branch runs).
    let mut zero_e = Cell::EMPTY;
    zero_e.energy = 0.0;
    zero_e.moments[3] = 2.0 * f32::MIN_POSITIVE;
    base.set_cell(20, 60, &zero_e).expect("set");

    let tolerance = ConeTolerance::new(1.0e-36, 1.0e-5).expect("tol");
    let trigger = ConeTolerance::new(f64::from(f32::MIN_POSITIVE), 1.0e-6).expect("trigger");

    let plain = base.check_invariants_on(tolerance, 1).expect("passes gate");
    assert_eq!(
        plain.max_cone_residual_cell,
        Some((12, 50)),
        "max_abs cell is below relative trigger while (5, 40) is above"
    );
    for workers in [0, 1, 2, 4, 8] {
        let (with_trig, needs) = base
            .check_invariants_with_trigger_on(tolerance, trigger, workers)
            .expect("passes gate with trigger");
        assert_eq!(summary_bits(&with_trig), summary_bits(&plain));
        assert!(
            needs,
            "workers={workers}: must flag cells above relative trigger despite larger max_abs cell below trigger"
        );
    }

    let energy_before: Vec<u32> = base.energy().iter().map(|v| v.to_bits()).collect();
    let below_small_before = base.cell(10, 3).expect("cell");
    let below_large_before = base.cell(12, 50).expect("cell");

    let mut serial = base.clone();
    let s_serial = serial.contract_cone(trigger).expect("contract serial");
    assert_eq!(s_serial.cells, 3, "(5, 40), (70, 100), and (20, 60)");
    assert!(s_serial.max_contraction > 0.0);
    assert!(
        s_serial.max_relative > 3.5e-6 && s_serial.max_relative < 5.0e-6,
        "relative contraction {:?}",
        s_serial
    );
    let serial_bits: Vec<u32> = serial.as_slice().iter().map(|v| v.to_bits()).collect();
    let energy_after: Vec<u32> = serial.energy().iter().map(|v| v.to_bits()).collect();
    assert_eq!(
        energy_after, energy_before,
        "contract_cone must never touch a single energy bit"
    );
    assert_eq!(
        serial.cell(10, 3).expect("c"),
        below_small_before,
        "below-trigger cell (10, 3) must stay bit-exact"
    );
    assert_eq!(
        serial.cell(12, 50).expect("c"),
        below_large_before,
        "below-trigger cell (12, 50) must stay bit-exact"
    );
    assert_eq!(
        serial.cell(20, 60).expect("c").moments[3].to_bits(),
        0,
        "zero-energy cell moments must be zeroed"
    );
    // Every contracted cell now satisfies ||m|| <= e with zero residual!
    for (x, y) in [(5, 40), (70, 100), (20, 60)] {
        let c = serial.cell(x, y).expect("cell");
        let sq: f64 = c.moments.iter().map(|&v| f64::from(v) * f64::from(v)).sum();
        assert!(
            sq.sqrt() <= f64::from(c.energy),
            "contracted cell ({x}, {y}) must satisfy ||m|| <= e exactly: ||m||={}, e={}",
            sq.sqrt(),
            c.energy
        );
    }
    let (_, still_needs) = serial
        .check_invariants_with_trigger_on(tolerance, trigger, 4)
        .expect("passes after contraction");
    assert!(
        !still_needs,
        "no cell should exceed trigger after contraction"
    );

    // Bit-exact worker invariance across [0, 1, 2, 4, 8].
    for workers in [0, 1, 2, 4, 8] {
        let mut parallel = base.clone();
        let s = parallel
            .contract_cone_on(trigger, workers)
            .expect("contract parallel");
        assert_eq!(s, s_serial, "workers={workers}: summary mismatch");
        let bits: Vec<u32> = parallel.as_slice().iter().map(|v| v.to_bits()).collect();
        assert_eq!(bits, serial_bits, "workers={workers}: field bits mismatch");
    }

    // Dual-carrier 34-plane (V2-15): Carrier 0 clean, Carrier 1 exceeds trigger.
    let mut dual = FieldState::zeroed_with_planes(g, DUAL_CARRIER_PLANES, &limits).expect("dual");
    let c1_idx = 70 * 160 + 15;
    let e1_val = 0.05_f32;
    dual.plane_mut(CARRIER1_ENERGY_PLANE).expect("e1")[c1_idx] = e1_val;
    let c1_boost = f64::from(e1_val) * (1.0 + 5.0e-6);
    let c1_norm = (8.0_f64).sqrt();
    for ch in 0..8_u32 {
        let sign = if ch % 2 == 0 { 1.0 } else { -1.0 };
        dual.plane_mut(CARRIER1_FIRST_MOMENT_PLANE + ch)
            .expect("m1")[c1_idx] = (sign * c1_boost / c1_norm) as f32;
    }
    let dual_ref_tol = ConeTolerance::new(f64::from(f32::MIN_POSITIVE), 1.0e-5).expect("tol");
    let (dual_sum, dual_needs) = dual
        .check_invariants_with_trigger_on(dual_ref_tol, trigger, 4)
        .expect("dual passes gate");
    assert!(
        dual_needs,
        "dual-carrier check must flag Carrier 1 exceeding trigger even when Carrier 0 is clean"
    );
    assert_eq!(dual_sum.max_cone_residual_cell, Some((15, 70)));
    let e0_before: Vec<u32> = dual.energy().iter().map(|v| v.to_bits()).collect();
    let e1_before: Vec<u32> = dual
        .plane(CARRIER1_ENERGY_PLANE)
        .expect("e1")
        .iter()
        .map(|v| v.to_bits())
        .collect();
    let mut dual_w4 = dual.clone();
    let ds1 = dual.contract_cone_on(trigger, 1).expect("dual 1");
    let ds4 = dual_w4.contract_cone_on(trigger, 4).expect("dual 4");
    assert_eq!(ds1, ds4);
    assert_eq!(ds1.cells, 1);
    assert_eq!(
        dual.as_slice()
            .iter()
            .map(|v| v.to_bits())
            .collect::<Vec<_>>(),
        dual_w4
            .as_slice()
            .iter()
            .map(|v| v.to_bits())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        dual.energy()
            .iter()
            .map(|v| v.to_bits())
            .collect::<Vec<_>>(),
        e0_before
    );
    assert_eq!(
        dual.plane(CARRIER1_ENERGY_PLANE)
            .expect("e1")
            .iter()
            .map(|v| v.to_bits())
            .collect::<Vec<_>>(),
        e1_before
    );
    dual.check_invariants(ConeTolerance::EXACT)
        .expect("dual-carrier contracted exactly onto cone");

    // Edge binades: subnormal, MIN_POSITIVE, negative zero, and near-f32::MAX.
    let mut edge = FieldState::zeroed(grid(4, 4, 4.0, 4.0), &limits).expect("edge");
    for (x, e_val, m0) in [
        // Subnormal `bits(100)` has 1 ULP = 1%, so step explicitly to `-bits(101)`.
        (0_u32, f32::from_bits(100), -f32::from_bits(101)),
        (
            1,
            f32::MIN_POSITIVE,
            -(f64::from(f32::MIN_POSITIVE) * (1.0 + 4.0e-6)) as f32,
        ),
        (2, 1.0_f32, -(1.0_f64 * (1.0 + 4.0e-6)) as f32),
        (
            3,
            f32::MAX * 0.5,
            -(f64::from(f32::MAX * 0.5) * (1.0 + 4.0e-6)) as f32,
        ),
    ] {
        let mut cell = Cell::EMPTY;
        cell.energy = e_val;
        cell.moments[0] = m0;
        cell.moments[1] = -0.0;
        edge.set_cell(x, 0, &cell).expect("set");
    }
    let edge_summary = edge
        .contract_cone(ConeTolerance::EXACT)
        .expect("edge contract");
    assert_eq!(edge_summary.cells, 4);
    edge.check_invariants(ConeTolerance::EXACT)
        .expect("all edge binades land strictly inside exact cone");
    assert_eq!(
        edge.cell(2, 0).expect("c").moments[1].to_bits(),
        0,
        "-0.0 canonicalized cleanly without underflow"
    );
}

/// Phase-locked F32 decay + source rounding reproduces the Arm 1 seed 101
/// `INV-02` cone-drift mechanism in ~211 ticks without contraction, and plan
/// §11.5 Option B contraction (`trigger = (f32::MIN_POSITIVE, 1e-6)`) bounds
/// it indefinitely with 0 energy bits altered and unchanged `1e-5` gate.
#[test]
fn plan_11_5_cone_contraction_stops_phase_locked_decay_stencil_source_drift() {
    let g = grid(8, 8, 8.0, 8.0);
    let limits = BufferLimits::host();
    let step = TimeStep::new(1, 128).expect("dt");
    let plan = TransportPlan::new(&g, step, 0.0, 0.01).expect("plan");
    let kernel = SourceKernel::new(0.1, 4.0).expect("1-cell kernel");

    let mut raw_dir = [0.0_f64; CHANNELS];
    for (k, slot) in raw_dir.iter_mut().enumerate() {
        *slot = (((k * 17 + 5) % 23) as f64 - 11.0) / 11.0;
    }
    let dir = unit_direction(&raw_dir).expect("unit dir");
    // Exact Arm 1 (103, 87) post-stencil binade: decay drops 887 ULPs (-0.476 ULP err)
    // and source adds +887 ULPs (-0.25 ULP err), locking e while ||m|| drifts outward.
    let e0 = 0.169_097_63_f32;
    let power = 887.25 * 2.0_f64.powi(-26) * 128.0;
    let sources = [Source::new(SourceId::new(0), [4.5, 4.5], dir, power).expect("source")];

    let seed_pair = || {
        let mut pair = FieldPair::zeroed(g, &limits).expect("pair");
        let mut cell = Cell::EMPTY;
        cell.energy = e0;
        for (slot, &c) in cell.moments.iter_mut().zip(&dir) {
            *slot = (f64::from(e0) * c) as f32;
        }
        pair.current_mut().set_cell(4, 4, &cell).expect("set");
        pair
    };

    let gate = ConeTolerance::new(f64::from(f32::MIN_POSITIVE), 1.0e-5).expect("gate");
    let trigger = ConeTolerance::new(f64::from(f32::MIN_POSITIVE), 1.0e-6).expect("trigger");

    // 1. Without contraction: breaches the 1e-5 INV-02 gate within 500 ticks.
    let mut raw_pair = seed_pair();
    let mut scratch = TransportScratch::new(&g, &limits).expect("scratch");
    let mut support = SourceSupport::new();
    let mut raw_failed_at = None;
    for tick in 0..500_u32 {
        advance_field(
            &mut raw_pair,
            &plan,
            &kernel,
            &sources,
            step,
            &mut scratch,
            &mut support,
        )
        .expect("step");
        if raw_pair.next().check_invariants(gate).is_err() {
            raw_failed_at = Some(tick);
            break;
        }
        raw_pair.commit();
    }
    assert!(
        raw_failed_at.is_some(),
        "phase-locked decay+source fixture must breach 1e-5 without contraction"
    );

    // 2. With plan §11.5 Option B contraction: passes all 500 ticks cleanly,
    //    never alters a single energy bit, keeps INV-02 gate at 1e-5, and
    //    logs every contraction honestly.
    let mut fixed_pair = seed_pair();
    let mut contracted_ticks = 0_u64;
    let mut total_contracted = 0_u64;
    let mut max_pre_rel = 0.0_f64;
    let mut worst_contraction = ConeContractionSummary::default();
    for _ in 0..500_u32 {
        advance_field(
            &mut fixed_pair,
            &plan,
            &kernel,
            &sources,
            step,
            &mut scratch,
            &mut support,
        )
        .expect("step");
        let (_, next) = fixed_pair.stage();
        let e_before: Vec<u32> = next.energy().iter().map(|v| v.to_bits()).collect();
        let (summary, needs) = next
            .check_invariants_with_trigger_on(gate, trigger, 1)
            .expect("passes unchanged 1e-5 gate");
        max_pre_rel = max_pre_rel.max(summary.max_cone_residual / f64::from(e0));
        if needs {
            let c = next.contract_cone(trigger).expect("contract");
            contracted_ticks += 1;
            total_contracted += c.cells;
            worst_contraction.max_contraction =
                worst_contraction.max_contraction.max(c.max_contraction);
            worst_contraction.max_relative = worst_contraction.max_relative.max(c.max_relative);
        }
        let e_after: Vec<u32> = next.energy().iter().map(|v| v.to_bits()).collect();
        assert_eq!(e_after, e_before, "contraction must never modify energy");
        fixed_pair.commit();
    }
    assert!(
        contracted_ticks > 0 && total_contracted > 0,
        "contraction must fire on phase-locked ticks"
    );
    assert!(
        max_pre_rel < 2.0e-6 && worst_contraction.max_relative < 2.0e-6,
        "pre-contraction residual ({max_pre_rel:e}) and contraction ({worst_contraction:?}) stay bounded well below 1e-5"
    );
}
