//! FL-017 evidence (plan §9.5, §13.2, §13.5, Appendix B.3, C.7, C.15):
//! render epochs are exactly one committed world, in stable identity order,
//! independent of storage arrangement, preallocated and bounded.
//!
//! Every view is compared with the world's own rows. The field texture, its
//! composition and its components are compared with independent block
//! oracles.

mod common;

use common::{ecology, found, step};
use flife_core::CoreError;
use flife_core::ids::OrganismId;
use flife_engine::{Arrangement, World};
use flife_render::{
    AFFINITY_AXES, Bounds, COMPONENT_STEPS, Dtype, Epoch, FIELD_TEXELS_MAX, FieldChannel,
    FieldTexture, Gap, Lease, Match, Publication, Publisher, ROUNDING_MARGIN, View,
    affinity_projection, pairs,
};

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn publisher(world: &World) -> Publisher {
    Publisher::new(world, 3, || {}).expect("a publisher")
}

/// Offer `world` and lease what the feed then shows.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn offer_and_lease(publisher: &mut Publisher, world: &World) -> Lease {
    let publication = publisher.offer(world).expect("an offer");
    assert!(
        matches!(publication, Publication::Published { .. }),
        "{publication:?}"
    );
    publisher
        .feed()
        .viewer(1)
        .expect("a viewer")
        .latest()
        .expect("a lease")
}

/// The F32 coordinate an epoch must show for `coordinate` on a `length`
/// axis, computed independently of the crate.
fn shown(coordinate: f64, length: f64) -> f32 {
    let rounded = coordinate as f32;
    if f64::from(rounded) >= f64::from(length as f32) {
        0.0
    } else {
        rounded
    }
}

/// The 32 × 32 Sylvester Hadamard matrix by its recursion
/// `H₂ₙ = [[Hₙ, Hₙ], [Hₙ, −Hₙ]]` from `H₁ = [1]`, independently of the
/// crate's popcount form.
fn hadamard() -> Vec<Vec<f64>> {
    let mut matrix = vec![vec![1.0]];
    while matrix.len() < 32 {
        let n = matrix.len();
        let mut next = vec![vec![0.0; 2 * n]; 2 * n];
        for row in 0..n {
            for column in 0..n {
                let value = matrix[row][column];
                next[row][column] = value;
                next[row][column + n] = value;
                next[row + n][column] = value;
                next[row + n][column + n] = -value;
            }
        }
        matrix = next;
    }
    matrix
}

/// The affinity view an epoch must show for `affinity` under the norm limit
/// `limit`: rows 7, 14 and 25 of [`hadamard`], dotted in channel order,
/// over `√32 × limit`, clamped and rounded to F32.
fn projected(affinity: &[f64], limit: f64) -> [f32; 3] {
    let matrix = hadamard();
    [7, 14, 25].map(|row: usize| {
        let dot = matrix[row]
            .iter()
            .zip(affinity)
            .fold(
                0.0_f64,
                |sum, (&sign, &value)| {
                    if sign > 0.0 { sum + value } else { sum - value }
                },
            );
        (dot / (32.0_f64.sqrt() * limit)).clamp(-1.0, 1.0) as f32
    })
}

/// Every view of `epoch` against `world`'s committed rows.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn assert_shows(epoch: &Epoch, world: &World) {
    let population = world.population();
    let rows: Vec<_> = population.iter().collect();
    let grid = world.field().grid();
    let [width, height] = [grid.world_width(), grid.world_height()];
    assert_eq!(epoch.tick(), world.tick());
    assert_eq!(epoch.resource(), world.epoch());
    assert_eq!(epoch.step(), world.step_size());
    assert_eq!(epoch.maximum_speed(), world.maximum_speed());
    assert_eq!(epoch.extent(), [width, height]);
    assert_eq!(epoch.capacity(), population.capacity());
    assert_eq!(epoch.len(), rows.len());
    for (row, organism) in rows.iter().enumerate() {
        assert_eq!(epoch.ids()[row], organism.id.get());
        assert_eq!(epoch.id_at(row), Some(organism.id));
        assert_eq!(epoch.row(organism.id), Some(row));
        assert_eq!(
            epoch.positions()[row],
            [
                shown(organism.position[0], width),
                shown(organism.position[1], height)
            ]
        );
        assert_eq!(
            epoch.velocities()[row],
            [organism.velocity[0] as f32, organism.velocity[1] as f32]
        );
        assert_eq!(epoch.energies()[row], organism.energy as f32);
        assert_eq!(epoch.lineages()[row], organism.lineage.get());
        assert_eq!(epoch.generations()[row], organism.generation.get());
        let genome = population.genome(organism.id).expect("genome");
        assert_eq!(
            usize::from(epoch.topologies()[row]),
            genome.topology().index()
        );
        assert_eq!(
            epoch.affinities()[row].map(f32::to_bits),
            projected(
                genome.traits().affinity(),
                world.trait_limits().affinity_norm()
            )
            .map(f32::to_bits),
            "row {row}"
        );
    }
    assert_eq!(epoch.affinity_limit(), world.trait_limits().affinity_norm());
    let metrics = epoch.metrics();
    assert_eq!(metrics.tick, world.tick());
    assert_eq!(metrics.live, rows.len() as u64);
    let energy: f64 = rows.iter().map(|organism| organism.energy).sum();
    assert_eq!(metrics.organism_energy.to_bits(), energy.to_bits());
    assert_eq!(
        metrics.field_energy.to_bits(),
        world.field().energy_total().to_bits()
    );
    assert_eq!(
        metrics.max_generation,
        rows.iter().map(|o| o.generation.get()).max().unwrap_or(0)
    );
    assert_eq!(metrics.events, world.events());
    let texture = epoch.field();
    let composition = composition_oracle(texture.cells(), world.field().energy(), |channel| {
        world.field().moment(channel).expect("a moment plane")
    });
    assert_eq!(
        texture
            .composition()
            .iter()
            .map(|q| q.map(f32::to_bits))
            .collect::<Vec<_>>(),
        composition
            .iter()
            .map(|q| q.map(f32::to_bits))
            .collect::<Vec<_>>(),
        "the field's composition at tick {}",
        world.tick().get()
    );
    let components = components_oracle(texture.cells(), world.field().energy(), |channel| {
        world.field().moment(channel).expect("a moment plane")
    });
    assert!(
        texture.components() == components.as_slice(),
        "the field's components at tick {}",
        world.tick().get()
    );
    epoch.check().expect("every declared bound holds");
}

/// The components oracle: each texel's moments and energy summed in F64
/// over its block in row-major order, channel by channel, as
/// [`composition_oracle`] sums them; then each channel's sum over the
/// energy's, clamped to `[−1, 1]`, in 127ths, rounded half away from zero
/// by its magnitude's whole and fractional parts; zeros for a block with no
/// energy.
fn components_oracle<'a>(
    cells: [u32; 2],
    energy: &[f32],
    moment: impl Fn(u32) -> &'a [f32],
) -> Vec<[i8; 32]> {
    let block = cells.map(|n| n.div_ceil(FIELD_TEXELS_MAX));
    let texels = [cells[0].div_ceil(block[0]), cells[1].div_ceil(block[1])];
    let planes: Vec<&[f32]> = (0..32).map(&moment).collect();
    let mut components = Vec::new();
    for ty in 0..texels[1] {
        for tx in 0..texels[0] {
            let (mut e, mut m) = (0.0_f64, [0.0_f64; 32]);
            let ys = ty * block[1]..((ty + 1) * block[1]).min(cells[1]);
            let xs = tx * block[0]..((tx + 1) * block[0]).min(cells[0]);
            for y in ys.clone() {
                for x in xs.clone() {
                    e += f64::from(energy[(y * cells[0] + x) as usize]);
                }
            }
            for (channel, plane) in planes.iter().enumerate() {
                for y in ys.clone() {
                    for x in xs.clone() {
                        m[channel] += f64::from(plane[(y * cells[0] + x) as usize]);
                    }
                }
            }
            components.push(if e > 0.0 && e.is_finite() {
                m.map(|sum| {
                    let scaled = (sum / e).clamp(-1.0, 1.0) * 127.0;
                    let magnitude = scaled.abs();
                    let whole =
                        magnitude.trunc() + if magnitude.fract() >= 0.5 { 1.0 } else { 0.0 };
                    let stepped = whole as i8;
                    if scaled < 0.0 { -stepped } else { stepped }
                })
            } else {
                [0; 32]
            });
        }
    }
    components
}

/// The composition oracle: each texel's moments and energy summed in F64
/// over its block in row-major order, channel by channel, then rows 7, 14
/// and 25 of [`hadamard`] dotted with the moment sums in channel order over
/// `√32 Σe`, clamped and rounded to F32; `[0; 3]` for a block with no
/// energy. `moment(c)` is channel `c`'s row-major plane.
fn composition_oracle<'a>(
    cells: [u32; 2],
    energy: &[f32],
    moment: impl Fn(u32) -> &'a [f32],
) -> Vec<[f32; 3]> {
    let matrix = hadamard();
    let block = cells.map(|n| n.div_ceil(FIELD_TEXELS_MAX));
    let texels = [cells[0].div_ceil(block[0]), cells[1].div_ceil(block[1])];
    let planes: Vec<&[f32]> = (0..32).map(&moment).collect();
    let mut composition = Vec::new();
    for ty in 0..texels[1] {
        for tx in 0..texels[0] {
            let (mut e, mut m) = (0.0_f64, [0.0_f64; 32]);
            let ys = ty * block[1]..((ty + 1) * block[1]).min(cells[1]);
            let xs = tx * block[0]..((tx + 1) * block[0]).min(cells[0]);
            for y in ys.clone() {
                for x in xs.clone() {
                    e += f64::from(energy[(y * cells[0] + x) as usize]);
                }
            }
            for (channel, plane) in planes.iter().enumerate() {
                for y in ys.clone() {
                    for x in xs.clone() {
                        m[channel] += f64::from(plane[(y * cells[0] + x) as usize]);
                    }
                }
            }
            composition.push(if e > 0.0 {
                [7, 14, 25].map(|row: usize| {
                    let dot = matrix[row]
                        .iter()
                        .zip(&m)
                        .fold(
                            0.0_f64,
                            |sum, (&sign, &value)| {
                                if sign > 0.0 { sum + value } else { sum - value }
                            },
                        );
                    (dot / (32.0_f64.sqrt() * e)).clamp(-1.0, 1.0) as f32
                })
            } else {
                [0.0; 3]
            });
        }
    }
    composition
}

/// The block-mean oracle: each texel's mean energy density, summed in F64
/// over its cells in row-major order.
fn oracle(cells: [u32; 2], cell_size: [f64; 2], plane: &[f32]) -> (Vec<f32>, [u32; 2]) {
    let block = cells.map(|n| n.div_ceil(FIELD_TEXELS_MAX));
    let texels = [cells[0].div_ceil(block[0]), cells[1].div_ceil(block[1])];
    let mut values = Vec::new();
    for ty in 0..texels[1] {
        for tx in 0..texels[0] {
            let (mut sum, mut n) = (0.0_f64, 0_u32);
            for y in ty * block[1]..((ty + 1) * block[1]).min(cells[1]) {
                for x in tx * block[0]..((tx + 1) * block[0]).min(cells[0]) {
                    sum += f64::from(plane[(y * cells[0] + x) as usize]);
                    n += 1;
                }
            }
            values.push((sum / (f64::from(n) * cell_size[0] * cell_size[1])) as f32);
        }
    }
    (values, texels)
}

/// B.3: an epoch is exactly one committed world. At the founding tick and
/// after births and deaths, every view, the metrics and the field texture
/// match the world's own committed state, and every declared layout holds.
#[test]
fn an_epoch_shows_exactly_one_committed_world() {
    let config = ecology(&[]);
    let mut world = found(&config, Arrangement::FreeList);
    let mut publisher = publisher(&world);
    let founding = publisher
        .feed()
        .viewer(1)
        .expect("viewer")
        .latest()
        .expect("lease");
    assert_eq!(founding.id().get(), 1);
    assert_eq!(founding.prior(), None);
    assert_shows(&founding, &world);
    drop(founding);

    let (mut births, mut deaths) = (0, 0);
    let mut previous = publisher.latest();
    for _ in 0..288 {
        let report = step(&mut world);
        births += report.births.len();
        deaths += report.deaths.len();
        let lease = offer_and_lease(&mut publisher, &world);
        assert_eq!(lease.prior(), Some(previous));
        assert_eq!(lease.id().get(), previous.id.get() + 1);
        assert_shows(&lease, &world);
        previous = publisher.latest();
    }
    assert!(births > 0 && deaths > 0, "{births} births, {deaths} deaths");

    let lease = publisher
        .feed()
        .viewer(1)
        .expect("viewer")
        .latest()
        .expect("lease");
    let layouts = lease.layouts();
    let declared: Vec<_> = layouts
        .iter()
        .map(|layout| (layout.view, layout.dtype, layout.components))
        .collect();
    assert_eq!(
        declared,
        vec![
            (View::Identity, Dtype::U64, 1),
            (View::Position, Dtype::F32, 2),
            (View::Velocity, Dtype::F32, 2),
            (View::Energy, Dtype::F32, 1),
            (View::Lineage, Dtype::U64, 1),
            (View::Generation, Dtype::U32, 1),
            (View::Topology, Dtype::U8, 1),
            (View::Affinity, Dtype::F32, 3),
            (View::Field, Dtype::F32, 1),
            (View::FieldComposition, Dtype::F32, 3),
            (View::FieldComponents, Dtype::I8, 32),
        ]
    );
    assert_eq!(
        layouts.map(|layout| layout.view).to_vec(),
        View::ALL.to_vec()
    );
    for layout in &layouts[..8] {
        assert_eq!(layout.count, lease.len());
    }
    assert_eq!(layouts[1].bounds, Bounds::Wrapped([32.0, 32.0]));
    assert_eq!(layouts[2].bounds, Bounds::Magnitude(4.0));
    assert_eq!(layouts[7].bounds, Bounds::Magnitude(1.0));
    assert_eq!(layouts[8].count, 32 * 32);
    assert_eq!(layouts[9].count, 32 * 32);
    assert_eq!(layouts[9].bounds, Bounds::Magnitude(1.0));
    assert_eq!(layouts[10].count, 32 * 32);
    assert_eq!(layouts[10].bounds, Bounds::Magnitude(127.0));
    // The world's sources and emissions carry chemistry, so the composition
    // the oracle matched is not all none.
    assert!(
        lease
            .field()
            .composition()
            .iter()
            .any(|q| q.iter().any(|c| c.abs() > 0.01)),
        "no texel has a composition"
    );

    let texture = lease.field();
    let grid = world.field().grid();
    assert_eq!(texture.channel(), FieldChannel::EnergyDensity);
    assert_eq!(texture.cells(), [32, 32]);
    assert_eq!(texture.block(), [1, 1]);
    assert_eq!(texture.texels(), [32, 32]);
    assert_eq!(
        texture.texel_size(),
        [grid.cell_width(), grid.cell_height()]
    );
    let (expected, _) = oracle(
        [32, 32],
        [grid.cell_width(), grid.cell_height()],
        world.field().energy(),
    );
    assert_eq!(texture.values(), expected.as_slice());
}

/// §9.5: a large grid's texture is bounded by averaging whole-cell blocks;
/// an axis that does not divide evenly ends in a partial block. Planes of
/// the wrong size and empty grids are refused.
#[test]
fn a_large_field_is_a_bounded_block_mean_texture() {
    let cells = [1030_u32, 515];
    let cell_size = [0.5, 0.25];
    let plane: Vec<f32> = (0..cells[0] * cells[1])
        .map(|i| ((i * 7919) % 1013) as f32 * 0.125)
        .collect();
    let texture = FieldTexture::from_plane(cells, cell_size, &plane).expect("texture");
    let (expected, texels) = oracle(cells, cell_size, &plane);
    assert_eq!(texture.block(), [3, 2]);
    assert_eq!(texels, [344, 258]);
    assert_eq!(texture.texels(), texels);
    assert!(texels.iter().all(|&n| n <= FIELD_TEXELS_MAX));
    assert_eq!(texture.texel_size(), [1.5, 0.5]);
    assert_eq!(texture.values(), expected.as_slice());
    assert_eq!(texture.layout().count, 344 * 258);

    let exact = FieldTexture::from_plane([512, 1], [1.0, 1.0], &[2.0; 512]).expect("texture");
    assert_eq!((exact.block(), exact.texels()), ([1, 1], [512, 1]));
    let halved = FieldTexture::from_plane([1024, 1], [1.0, 1.0], &[2.0; 1024]).expect("texture");
    assert_eq!((halved.block(), halved.texels()), ([2, 1], [512, 1]));
    assert!(halved.values().iter().all(|&v| v == 2.0));

    assert!(matches!(
        FieldTexture::from_plane(cells, cell_size, &plane[1..]),
        Err(CoreError::LayoutMismatch { .. })
    ));
    assert!(matches!(
        FieldTexture::from_plane([0, 4], cell_size, &[]),
        Err(CoreError::LayoutMismatch { .. })
    ));
}

/// Plan §13.4, "Local field": a texel's composition is its block's
/// energy-weighted mean composition `Σm / Σe` through the affinity's fixed
/// projection. Hand-computed cells: a composition along a projection row
/// lands on its axis and its opposite on the opposite end; opposite
/// emissions that cancelled leave none; a cell with no energy has none; one
/// channel lands at `±1/√32` on each axis by the signs of its column.
/// Across a block the mean is weighted by energy: three units along a row
/// and one against it are half way, not none. An energy plane alone has no
/// composition, and planes of the wrong length are refused.
#[test]
fn the_field_composition_is_the_energy_weighted_mean_composition_projected() {
    const CHANNELS: usize = 32;
    let matrix = hadamard();
    let along = |row: usize, scale: f64| -> [f64; CHANNELS] {
        std::array::from_fn(|channel| matrix[row][channel] / 32.0_f64.sqrt() * scale)
    };
    let planes = |cells: &[(f32, [f64; CHANNELS])]| -> Vec<f32> {
        let mut planes: Vec<f32> = cells.iter().map(|(e, _)| *e).collect();
        for channel in 0..CHANNELS {
            planes.extend(cells.iter().map(|(_, m)| m[channel] as f32));
        }
        planes
    };
    let near = |got: [f32; 3], want: [f64; 3]| {
        got.iter()
            .zip(want)
            .all(|(&g, w)| (f64::from(g) - w).abs() < 1e-6)
    };

    // Cells: (energy, moment) on a 6 x 1 grid.
    let mut single = [0.0; CHANNELS];
    single[5] = 4.0;
    let cells: [(f32, [f64; CHANNELS]); 6] = [
        (2.0, along(7, 2.0)),
        (1.0, along(7, -1.0)),
        (3.0, [0.0; CHANNELS]),
        (0.0, [0.0; CHANNELS]),
        (4.0, single),
        (5.0, along(25, 5.0)),
    ];
    let texture = FieldTexture::from_planes([6, 1], [1.0, 1.0], &planes(&cells)).expect("texture");
    let q = texture.composition();
    assert!(near(q[0], [1.0, 0.0, 0.0]), "{:?}", q[0]);
    assert!(near(q[1], [-1.0, 0.0, 0.0]), "{:?}", q[1]);
    assert_eq!(q[2], [0.0; 3]);
    assert_eq!(q[3], [0.0; 3]);
    // Column 5: 7 & 5 = 5 (even), 14 & 5 = 4 (odd), 25 & 5 = 1 (odd).
    let third = 1.0 / 32.0_f64.sqrt();
    assert!(near(q[4], [third, -third, -third]), "{:?}", q[4]);
    assert!(near(q[5], [0.0, 0.0, 1.0]), "{:?}", q[5]);
    assert_eq!(texture.values(), &[2.0, 1.0, 3.0, 0.0, 4.0, 5.0]);
    assert_eq!(texture.composition_layout().count, 6);
    assert_eq!(texture.composition_layout().bounds, Bounds::Magnitude(1.0));

    // 1024 cells in blocks of two: three units along row 7 then one against
    // it, then two cells of opposite compositions and equal energy.
    let mut wide = Vec::new();
    for pair in 0..512 {
        if pair % 2 == 0 {
            wide.push((3.0_f32, along(7, 3.0)));
            wide.push((1.0_f32, along(7, -1.0)));
        } else {
            wide.push((1.0_f32, along(14, 1.0)));
            wide.push((1.0_f32, along(14, -1.0)));
        }
    }
    let planar = planes(&wide);
    let blocked = FieldTexture::from_planes([1024, 1], [1.0, 1.0], &planar).expect("texture");
    assert_eq!(blocked.block(), [2, 1]);
    let blocks = blocked.composition();
    assert!(near(blocks[0], [0.5, 0.0, 0.0]), "{:?}", blocks[0]);
    assert!(near(blocks[1], [0.0, 0.0, 0.0]), "{:?}", blocks[1]);
    assert_eq!(blocked.values()[0], 2.0);
    let expected = composition_oracle([1024, 1], &planar[..1024], |channel| {
        let start = 1024 * (1 + channel as usize);
        &planar[start..start + 1024]
    });
    assert_eq!(blocks, expected.as_slice());

    // Blocks of 2 x 2 with a partial last column and row, each cell's
    // moment within its energy: the oracle's values, in the unit ball.
    let grid = [521_u32, 515];
    let count = (grid[0] * grid[1]) as usize;
    let energy = |i: usize| ((i * 7919) % 1013) as f32 * 0.01;
    let mut planar: Vec<f32> = (0..count).map(energy).collect();
    for channel in 0..CHANNELS {
        planar.extend((0..count).map(|i| {
            let unit = (((i * 31 + channel * 17) % 101) as f32 / 101.0 - 0.5) * 2.0;
            energy(i) * unit / 32.0_f32.sqrt()
        }));
    }
    let partial = FieldTexture::from_planes(grid, [0.5, 0.25], &planar).expect("texture");
    assert_eq!((partial.block(), partial.texels()), ([2, 2], [261, 258]));
    assert_eq!(
        partial.values(),
        oracle(grid, [0.5, 0.25], &planar[..count]).0.as_slice()
    );
    let expected = composition_oracle(grid, &planar[..count], |channel| {
        let start = count * (1 + channel as usize);
        &planar[start..start + count]
    });
    assert_eq!(partial.composition(), expected.as_slice());
    assert!(
        partial
            .composition()
            .iter()
            .any(|q| q.iter().any(|c| c.abs() > 0.01))
    );
    for q in partial.composition() {
        let norm = q.iter().map(|&c| f64::from(c).powi(2)).sum::<f64>().sqrt();
        assert!(norm <= 1.0 + 1e-6, "{q:?}");
    }

    let energy_only =
        FieldTexture::from_plane([6, 1], [1.0, 1.0], &planes(&cells)[..6]).expect("texture");
    assert_eq!(energy_only.composition(), &[[0.0; 3]; 6]);
    for length in [6, 6 * 33 - 1, 6 * 33 + 1] {
        let mut planar = planes(&cells);
        planar.resize(length, 0.0);
        assert!(
            matches!(
                FieldTexture::from_planes([6, 1], [1.0, 1.0], &planar),
                Err(CoreError::LayoutMismatch { .. })
            ),
            "{length} values"
        );
    }
}

/// Plan §13.4's chosen components: each texel holds each moment channel per
/// unit energy, `Σm_c / Σe`, in 127ths. Hand-computed cells: half the
/// energy in one channel is 64 (63.5, rounded away from zero), its opposite
/// −64; a moment at the norm limit is 127; one third in two channels is 42
/// each; a cell with no energy, or with energy but no moment, has zeros.
/// Across a block the sums are energy-weighted: three units at +1 and one
/// at −1 in a channel are +0.5, 64. On a grid with partial blocks the
/// oracle's values hold. An energy plane alone has zeros.
#[test]
fn the_field_components_are_each_channel_per_unit_energy() {
    const CHANNELS: usize = 32;
    let planes = |cells: &[(f32, [f32; CHANNELS])]| -> Vec<f32> {
        let mut planes: Vec<f32> = cells.iter().map(|(e, _)| *e).collect();
        for channel in 0..CHANNELS {
            planes.extend(cells.iter().map(|(_, m)| m[channel]));
        }
        planes
    };
    let with = |pairs: &[(usize, f32)]| -> [f32; CHANNELS] {
        let mut m = [0.0; CHANNELS];
        for &(channel, value) in pairs {
            m[channel] = value;
        }
        m
    };
    let cells: [(f32, [f32; CHANNELS]); 6] = [
        (2.0, with(&[(5, 1.0)])),
        (2.0, with(&[(5, -1.0)])),
        (4.0, with(&[(31, 4.0)])),
        (3.0, with(&[(0, 1.0), (9, -1.0)])),
        (0.0, [0.0; CHANNELS]),
        (5.0, [0.0; CHANNELS]),
    ];
    let texture = FieldTexture::from_planes([6, 1], [1.0, 1.0], &planes(&cells)).expect("texture");
    let q = texture.components();
    assert_eq!(q.len(), 6);
    assert_eq!(q[0], with_steps(&[(5, 64)]));
    assert_eq!(q[1], with_steps(&[(5, -64)]));
    assert_eq!(q[2], with_steps(&[(31, 127)]));
    assert_eq!(q[3], with_steps(&[(0, 42), (9, -42)]));
    assert_eq!(q[4], [0; CHANNELS]);
    assert_eq!(q[5], [0; CHANNELS]);
    assert_eq!(COMPONENT_STEPS, 127);
    let layout = texture.components_layout();
    assert_eq!(
        (layout.view, layout.dtype, layout.components, layout.count),
        (View::FieldComponents, Dtype::I8, 32, 6)
    );
    assert_eq!(layout.bounds, Bounds::Magnitude(127.0));

    // 1024 cells in blocks of two: three units at +1 in channel 3 with one
    // unit at -1, then a cell at +1 in channel 7 with an empty one.
    let mut wide = Vec::new();
    for pair in 0..512 {
        if pair % 2 == 0 {
            wide.push((3.0_f32, with(&[(3, 3.0)])));
            wide.push((1.0_f32, with(&[(3, -1.0)])));
        } else {
            wide.push((1.0_f32, with(&[(7, 1.0)])));
            wide.push((0.0_f32, [0.0; CHANNELS]));
        }
    }
    let planar = planes(&wide);
    let blocked = FieldTexture::from_planes([1024, 1], [1.0, 1.0], &planar).expect("texture");
    assert_eq!(blocked.block(), [2, 1]);
    assert_eq!(blocked.components()[0], with_steps(&[(3, 64)]));
    assert_eq!(blocked.components()[1], with_steps(&[(7, 127)]));
    let expected = components_oracle([1024, 1], &planar[..1024], |channel| {
        let start = 1024 * (1 + channel as usize);
        &planar[start..start + 1024]
    });
    assert!(blocked.components() == expected.as_slice());

    // Blocks of 2 x 2 with a partial last column and row, each cell's
    // moment within its energy: the oracle's values, not all zero.
    let grid = [521_u32, 515];
    let count = (grid[0] * grid[1]) as usize;
    let energy = |i: usize| ((i * 7919) % 1013) as f32 * 0.01;
    let mut planar: Vec<f32> = (0..count).map(energy).collect();
    for channel in 0..CHANNELS {
        planar.extend((0..count).map(|i| {
            let unit = (((i * 31 + channel * 17) % 101) as f32 / 101.0 - 0.5) * 2.0;
            energy(i) * unit / 32.0_f32.sqrt()
        }));
    }
    let partial = FieldTexture::from_planes(grid, [0.5, 0.25], &planar).expect("texture");
    assert_eq!((partial.block(), partial.texels()), ([2, 2], [261, 258]));
    let expected = components_oracle(grid, &planar[..count], |channel| {
        let start = count * (1 + channel as usize);
        &planar[start..start + count]
    });
    assert!(partial.components() == expected.as_slice());
    assert!(
        partial
            .components()
            .iter()
            .any(|q| q.iter().any(|&c| c.unsigned_abs() > 10)),
        "the partial grid's components are not all near zero"
    );

    let energy_only =
        FieldTexture::from_plane([6, 1], [1.0, 1.0], &planes(&cells)[..6]).expect("texture");
    assert_eq!(energy_only.components(), &[[0; CHANNELS]; 6]);
}

/// Steps `pairs` of channel and value, zero elsewhere.
fn with_steps(pairs: &[(usize, i8)]) -> [i8; 32] {
    let mut steps = [0; 32];
    for &(channel, value) in pairs {
        steps[channel] = value;
    }
    steps
}

/// C.7: two storage arrangements of one stable-identity world occupy
/// different slots yet publish identical epochs at every tick, and the
/// identity matching between consecutive epochs is identical too.
#[test]
fn c7_two_storage_arrangements_publish_identical_epochs() {
    let config = ecology(&[]);
    let mut compact = found(&config, Arrangement::Compact);
    let mut free_list = found(&config, Arrangement::FreeList);
    let mut a = publisher(&compact);
    let mut b = publisher(&free_list);
    let mut previous: Option<(Lease, Lease)> = None;
    let mut layouts_differed = 0;
    for _ in 0..288 {
        step(&mut compact);
        step(&mut free_list);
        assert_eq!(compact.digest(), free_list.digest());
        if compact.population().slots() != free_list.population().slots() {
            layouts_differed += 1;
        }
        let x = offer_and_lease(&mut a, &compact);
        let y = offer_and_lease(&mut b, &free_list);
        assert_eq!(*x, *y, "tick {}", compact.tick().get());
        if let Some((px, py)) = &previous {
            let mx: Vec<Match> = pairs(px, &x).collect();
            let my: Vec<Match> = pairs(py, &y).collect();
            assert_eq!(mx, my);
        }
        previous = Some((x, y));
    }
    assert!(
        layouts_differed > 0,
        "the two arrangements never placed organisms differently"
    );
}

/// §13.5: matching is by identity. Between consecutive epochs, continuing
/// organisms pair with themselves and every birth and death of the tick
/// appears once. When the free list gives a dead organism's slot to a
/// newborn, possibly ticks later, the newborn is a birth and the dead
/// occupant is gone: the slot never links two identities.
#[test]
fn a_reused_slot_never_pairs_two_identities() {
    let config = ecology(&[]);
    let mut world = found(&config, Arrangement::FreeList);
    let mut publisher = publisher(&world);
    let mut prior = publisher
        .feed()
        .viewer(2)
        .expect("viewer")
        .latest()
        .expect("lease");
    let mut last: Vec<Option<OrganismId>> = world.population().slots().to_vec();
    let (mut reused, mut total_births, mut total_deaths, mut births_after_death) = (0, 0, 0, 0);
    for _ in 0..1024 {
        let report = step(&mut world);
        if total_deaths > 0 {
            births_after_death += report.births.len();
        }
        total_births += report.births.len();
        total_deaths += report.deaths.len();
        let current = offer_and_lease(&mut publisher, &world);
        let (mut born, mut died, mut continuing) = (Vec::new(), Vec::new(), 0);
        for found in pairs(&prior, &current) {
            match found {
                Match::Continuing {
                    prior: p,
                    current: c,
                } => {
                    assert_eq!(prior.ids()[p], current.ids()[c]);
                    continuing += 1;
                }
                Match::Born { current: c } => born.push(current.ids()[c]),
                Match::Died { prior: p } => died.push(prior.ids()[p]),
            }
        }
        let births: Vec<u64> = report.births.iter().map(|b| b.child.get()).collect();
        let deaths: Vec<u64> = report.deaths.iter().map(|d| d.id.get()).collect();
        assert_eq!(born, births);
        assert_eq!(died, deaths);
        assert_eq!(continuing + born.len(), current.len());
        for (slot, occupant) in world.population().slots().iter().enumerate() {
            let Some(after) = *occupant else { continue };
            if let Some(Some(before)) = last.get(slot).copied()
                && before != after
            {
                reused += 1;
                assert!(
                    births.contains(&after.get()),
                    "{after} entered a reused slot unborn"
                );
                assert_eq!(current.row(before), None, "{before} still shown");
                assert_eq!(prior.row(after), None);
            }
            if let Some(entry) = last.get_mut(slot) {
                *entry = Some(after);
            } else {
                last.push(Some(after));
            }
        }
        prior = current;
    }
    assert!(
        reused > 0,
        "no slot was reused: {total_births} births, {total_deaths} deaths, \
         {births_after_death} births after the first death, {} live",
        world.population().len()
    );
}

/// §9.5: every slot is allocated once, at the population ceiling and the
/// texture size, and no projection reallocates, up to the ceiling itself.
#[test]
fn slots_are_allocated_once_and_never_grow() {
    let config = ecology(&[("population", "maximum_count", "72")]);
    let mut world = found(&config, Arrangement::FreeList);
    let mut publisher = publisher(&world);
    let row = 8 + 8 + 8 + 4 + 8 + 4 + 1 + 12;
    let slot = 72 * row + 32 * 32 * (4 + 12 + 32);
    assert_eq!(publisher.reserved_bytes(), 3 * slot);
    let viewer = publisher.feed().viewer(1).expect("viewer");
    let (mut peak, mut ceiling) = (0, false);
    for _ in 0..512 {
        match world.step().expect("advance") {
            flife_engine::Stepped::Published(_) => {}
            flife_engine::Stepped::Paused(_) => {
                ceiling = true;
                break;
            }
        }
        publisher.offer(&world).expect("offer");
        let lease = viewer.latest().expect("lease");
        assert_eq!(lease.reserved_bytes(), slot);
        peak = peak.max(lease.len());
        assert_eq!(publisher.reserved_bytes(), 3 * slot);
    }
    assert!(ceiling, "the population never reached its ceiling");
    assert!(peak > 60, "the population only reached {peak}");
}

/// C.15: latest-value publication can skip many ticks. A gap's travel
/// bound is `ticks · Δt · maximum speed`; a one-tick gap allows
/// minimum-image interpolation, and a gap long enough to cross half the
/// world does not. Epochs out of order or from different worlds are
/// refused.
#[test]
fn c15_a_long_skipped_gap_is_ambiguous_and_a_short_one_is_not() {
    let config = ecology(&[]);
    let mut world = found(&config, Arrangement::FreeList);
    let mut publisher = publisher(&world);
    let feed = publisher.feed();
    let first = feed.viewer(2).expect("viewer").latest().expect("lease");
    step(&mut world);
    let next = offer_and_lease(&mut publisher, &world);
    let short = Gap::between(&first, &next).expect("gap");
    let dt = world.step_size().seconds_per_tick();
    assert_eq!(short.ticks, 1);
    assert_eq!(short.travel_bound, dt * 4.0);
    assert!(short.unambiguous);

    // `first` and the leases on epoch 2 keep two slots. The third takes
    // the next boundary and stays displayable, so every later one is busy.
    let hold = feed.viewer(2).expect("viewer");
    let held = (hold.latest().expect("lease"), next);
    for _ in 0..600 {
        step(&mut world);
        publisher.offer(&world).expect("offer");
    }
    assert_eq!(publisher.stats().busy, 599);
    assert_eq!(publisher.stats().pending, Some(world.tick()));
    drop(held);
    let shown = hold.latest().expect("lease");
    assert_eq!(
        shown.tick().get(),
        2,
        "the feed kept its last displayable epoch"
    );
    step(&mut world);
    let after = offer_and_lease(&mut publisher, &world);
    assert_eq!(after.prior().map(|p| p.id), Some(shown.id()));
    let long = Gap::between(&shown, &after).expect("gap");
    assert_eq!(long.ticks, 600);
    assert_eq!(long.travel_bound, 600.0 * dt * 4.0);
    // 600 ticks at 4 units/s and Δt = 1/128 reach 18.75 units: past half
    // the 32-unit world, so minimum-image interpolation could reverse
    // an organism's path.
    assert!(!long.unambiguous, "{long:?}");
    assert_eq!(
        long.unambiguous,
        2.0 * long.travel_bound * (1.0 + ROUNDING_MARGIN) < 32.0
    );

    assert!(matches!(
        Gap::between(&after, &shown),
        Err(CoreError::StateConflict { .. })
    ));
    assert!(matches!(
        Gap::between(&after, &after),
        Err(CoreError::StateConflict { .. })
    ));
    let other = found(
        &ecology(&[("world", "width", "64.0"), ("world", "grid_width", "64")]),
        Arrangement::FreeList,
    );
    let foreign = publisher_lease(&other);
    assert!(matches!(
        Gap::between(&first, &foreign),
        Err(CoreError::StateConflict { .. })
    ));
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn publisher_lease(world: &World) -> Lease {
    let publisher = publisher(world);
    let lease = publisher
        .feed()
        .viewer(1)
        .expect("viewer")
        .latest()
        .expect("lease");
    drop(publisher);
    lease
}

/// Selecting by identity finds an organism's row, and an identity that is
/// not alive has none.
#[test]
fn selection_is_by_identity() {
    let config = ecology(&[]);
    let world = found(&config, Arrangement::Compact);
    let lease = publisher_lease(&world);
    assert_eq!(lease.row(OrganismId::new(0)), Some(0));
    assert_eq!(lease.row(OrganismId::new(47)), Some(47));
    assert_eq!(lease.row(OrganismId::new(48)), None);
    assert_eq!(lease.id_at(48), None);
}

/// FL-019, plan §13.4: the affinity view is a fixed projection with a
/// recorded matrix and scaling. The rows are 7, 14 and 25 of the Sylvester
/// Hadamard matrix, orthonormal over √32; a direction within the norm limit
/// lands in the unit ball, and a direction along a row lands on its axis.
#[test]
fn the_affinity_view_is_a_fixed_orthonormal_projection() {
    assert_eq!(AFFINITY_AXES, [7, 14, 25]);
    let matrix = hadamard();
    for (index, &a) in AFFINITY_AXES.iter().enumerate() {
        for &b in &AFFINITY_AXES[index..] {
            let dot: f64 = matrix[a].iter().zip(&matrix[b]).map(|(x, y)| x * y).sum();
            assert_eq!(dot, if a == b { 32.0 } else { 0.0 }, "rows {a} and {b}");
        }
    }
    // Along a row, at the limit: that axis's coordinate is 1, the others 0.
    for (axis, &row) in AFFINITY_AXES.iter().enumerate() {
        let limit = 1.5;
        let along: Vec<f64> = matrix[row]
            .iter()
            .map(|sign| sign * limit / 32.0_f64.sqrt())
            .collect();
        let mut expected = [0.0_f32; 3];
        expected[axis] = 1.0;
        let got = affinity_projection(&along, limit);
        for (component, (&g, &e)) in got.iter().zip(&expected).enumerate() {
            assert!(
                (g - e).abs() <= 1e-6,
                "axis {axis} component {component}: {got:?}"
            );
        }
    }
    // Beyond the limit, a coordinate is clamped to the unit interval.
    for (axis, &row) in AFFINITY_AXES.iter().enumerate() {
        let beyond: Vec<f64> = matrix[row].iter().map(|sign| -3.0 * sign).collect();
        let mut expected = [0.0_f32; 3];
        expected[axis] = -1.0;
        assert_eq!(affinity_projection(&beyond, 1.0), expected, "axis {axis}");
    }
    // Pseudorandom directions at the limit stay in the ball, equal to the
    // oracle bit for bit.
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 11) as f64 / (1_u64 << 53) as f64 - 0.5
    };
    for _ in 0..200 {
        let raw: Vec<f64> = (0..32).map(|_| next()).collect();
        let norm = raw.iter().map(|v| v * v).sum::<f64>().sqrt();
        let direction: Vec<f64> = raw.iter().map(|v| v / norm * 0.8).collect();
        let q = affinity_projection(&direction, 0.8);
        assert_eq!(
            q.map(f32::to_bits),
            projected(&direction, 0.8).map(f32::to_bits)
        );
        let radius = q
            .iter()
            .map(|&c| f64::from(c) * f64::from(c))
            .sum::<f64>()
            .sqrt();
        assert!(radius <= 1.0 + 1e-6, "{q:?}");
    }
    // A limit that scales nothing projects to the origin.
    let unit = [1.0; 32];
    assert_eq!(affinity_projection(&unit, 0.0), [0.0; 3]);
    assert_eq!(affinity_projection(&unit, f64::NAN), [0.0; 3]);
    assert_eq!(affinity_projection(&unit, -1.0), [0.0; 3]);
    let mut broken = [0.0; 32];
    broken[3] = f64::INFINITY;
    assert_eq!(affinity_projection(&broken, 1.0), [0.0; 3]);
    // A world whose norm limit is not 1 scales its epoch by its own limit.
    let config = ecology(&[("chemistry", "affinity_norm_limit", "0.5")]);
    let world = found(&config, Arrangement::FreeList);
    let limited = publisher(&world);
    let lease = limited
        .feed()
        .viewer(1)
        .expect("viewer")
        .latest()
        .expect("lease");
    assert_eq!(lease.affinity_limit(), 0.5);
    assert_shows(&lease, &world);
    let radii: Vec<f64> = lease
        .affinities()
        .iter()
        .map(|q| q.iter().map(|&c| f64::from(c).powi(2)).sum::<f64>().sqrt())
        .collect();
    // Founders sit at the limit, so their projections are as spread as at
    // limit 1, not halved.
    let mean = radii.iter().sum::<f64>() / radii.len() as f64;
    assert!((0.15..0.5).contains(&mean), "mean radius {mean}");
    drop(lease);
    // A slot made for the limit-1 world, offered the 0.5 world, states and
    // scales by the world it shows, not the one it was made for.
    let mut publisher = publisher(&found(&ecology(&[]), Arrangement::FreeList));
    let mut world = world;
    step(&mut world);
    let lease = offer_and_lease(&mut publisher, &world);
    assert_eq!(lease.affinity_limit(), 0.5);
    assert_shows(&lease, &world);
}
