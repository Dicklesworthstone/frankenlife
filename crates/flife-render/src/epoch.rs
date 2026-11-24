//! One immutable compact render epoch (plan Appendix B.3, §9.5, §13.2).
//!
//! An [`Epoch`] is a compact projection of exactly one committed world:
//! - its source tick and resource epoch;
//! - one row per live organism, in stable identity order;
//! - a bounded field texture derived from the energy plane, with each
//!   texel's mean chemical composition through a fixed projection and each
//!   of its moment channels per unit energy, quantized for display;
//! - a small metric summary.
//!
//! Every view declares its [`Layout`]: element type, components, count and
//! bounds. A slot's buffers are allocated once, at the world's population
//! ceiling and its texture size, and each projection refills them in place.
//! Publishing therefore never grows them. The field texture's rows are
//! projected on the world's CPU workers (cached private pool via
//! `flife_core::pool`, as the world's own phases use), bit for bit as on one.
//!
//! Rows follow identity order, never storage-slot order. Two storage
//! arrangements of one world project the same epoch (C.7), and a consumer
//! matches organisms between epochs by identity (§13.5).
//!
//! The epoch carries the inputs of §13.4's mappings: lineage, energy,
//! generation, topology, motion and the chemical affinity's fixed
//! projection ([`affinity_projection`]), and the world's constants they
//! scale by: its speed limit and the least energy an organism attempts a
//! birth with. Turning them into sizes and colours is a visual mapping
//! (`presentation.color_mapping`, FL-019).
//! Scientific code never reads an epoch back.

use flife_core::ids::{OrganismId, RenderEpoch, ResourceEpoch};
use flife_core::model::MOMENT_CHANNELS;
use flife_core::{CoreError, LedgerReport, Tick, TimeStep, Transfer, V2Feature};
use flife_engine::{Followed, TickReport, World};

/// The most texels a field texture has along either axis. A larger grid is
/// averaged over blocks of whole cells (§9.5: a bounded derived texture,
/// never a lease on an authoritative field).
pub const FIELD_TEXELS_MAX: u32 = 512;

/// The rows of the 32 × 32 Sylvester Hadamard matrix
/// `H[k][j] = (−1)^popcount(k & j)` that project an affinity direction onto
/// its three display axes: plan §13.4's "fixed projection of a
/// high-dimensional trait", whose matrix and scaling are recorded here. Any
/// two rows of `H` are orthogonal, so over `√32` these are orthonormal.
/// Between them they read all five bits of the channel index, and no one
/// of them is the product of the other two.
pub const AFFINITY_AXES: [usize; 3] = [7, 14, 25];

/// An affinity direction's display coordinates (FL-019): row `k` of
/// [`AFFINITY_AXES`] dotted with `affinity` in channel order, over
/// `√(channels) × limit`, rounded to F32. For 32 channels the rows are
/// orthonormal over `√32`, so a direction within the norm limit `limit`
/// lands in the unit ball; each coordinate is clamped to `[−1, 1]` against
/// rounding. `[0; 3]` for a nonpositive or non-finite `limit`, and `0` for
/// a coordinate that is not finite.
#[must_use]
pub fn affinity_projection(affinity: &[f64], limit: f64) -> [f32; 3] {
    if let Ok(fixed) = <&[f64; CHANNELS]>::try_from(affinity) {
        return affinity_projection_32(fixed, (CHANNELS as f64).sqrt() * limit);
    }
    let dots = AFFINITY_AXES.map(|row| {
        affinity
            .iter()
            .enumerate()
            .fold(0.0_f64, |sum, (channel, &value)| {
                if (row & channel).count_ones() % 2 == 0 {
                    sum + value
                } else {
                    sum - value
                }
            })
    });
    coordinates_with_sqrt(dots, (affinity.len() as f64).sqrt(), limit)
}

/// Fast path for the canonical 32-channel affinity vector with precomputed
/// `scale = √32 × limit`: single pass over `[f64; CHANNELS]` using the
/// compile-time [`AXIS_SIGNS`] table (zero branches, zero `popcount`, zero
/// `sqrt`, bit-identical to [`affinity_projection`]).
#[inline]
fn affinity_projection_32(affinity: &[f64; CHANNELS], scale: f64) -> [f32; 3] {
    if !(scale.is_finite() && scale > 0.0) {
        return [0.0; 3];
    }
    let (mut d0, mut d1, mut d2) = (0.0_f64, 0.0_f64, 0.0_f64);
    for (((&v, &w0), &w1), &w2) in affinity
        .iter()
        .zip(&AXIS_SIGNS[0])
        .zip(&AXIS_SIGNS[1])
        .zip(&AXIS_SIGNS[2])
    {
        d0 += w0 * v;
        d1 += w1 * v;
        d2 += w2 * v;
    }
    [d0, d1, d2].map(|dot| {
        let coordinate = dot / scale;
        if coordinate.is_finite() {
            coordinate.clamp(-1.0, 1.0) as f32
        } else {
            0.0
        }
    })
}

/// The display coordinates of the three dot products `dots` of
/// [`affinity_projection`] over `channels` channels with precomputed
/// `sqrt_channels = √(channels)`: each over `sqrt_channels × limit`, clamped
/// and rounded to F32; `[0; 3]` for a nonpositive or non-finite scale and `0`
/// for a coordinate that is not finite.
#[inline]
fn coordinates_with_sqrt(dots: [f64; 3], sqrt_channels: f64, limit: f64) -> [f32; 3] {
    let scale = sqrt_channels * limit;
    if !(scale.is_finite() && scale > 0.0) {
        return [0.0; 3];
    }
    dots.map(|dot| {
        let coordinate = dot / scale;
        if coordinate.is_finite() {
            coordinate.clamp(-1.0, 1.0) as f32
        } else {
            0.0
        }
    })
}

/// Moment channels per cell on a 33-plane single-carrier field, as an array length.
const CHANNELS: usize = MOMENT_CHANNELS as usize;

/// Total planes in a 34-plane `field-ecology-v2` dual-carrier field (`V2-15`):
/// plane 0 (`e_0`), planes `1..=24` (Carrier-0 moments), plane 25 (`e_1` trail
/// energy), planes `26..=33` (Carrier-1 trail moments).
const DUAL_CARRIER_PLANES: usize = 34;

/// Carrier-0 moment channels on a 34-plane dual-carrier field (`m_0..m_23`).
const CARRIER0_MOMENT_CHANNELS: usize = 24;

/// A field component's display step: a texel's moment channel per unit
/// energy, in `[−1, 1]`, is held as the nearest multiple of `1 / 127`, an
/// [`i8`] in `[−127, 127]` ([`FieldTexture::components`]).
pub const COMPONENT_STEPS: i8 = 127;

/// Quantization denominator for the primary recurrent cell state `h_0`
/// (`V2-16`, `cell-state-v1`): values in `[−1, 1]` quantize onto `[−127, 127]`.
pub const CELL_STATE_STEPS: i8 = 127;

/// Quantization scale for the quantized apical-basal polarity angle
/// (`V2-16`, `polarity-v1`): `atan2(p_y, p_x) ∈ [−π, π]` maps onto
/// `[−32767, +32767]`, preserving exact branch-cut continuity across `±π`.
pub const POLARITY_ANGLE_SCALE: i16 = i16::MAX;

/// Explicit sentinel in [`Epoch::polarities`] indicating an unpolarized
/// organism (zero-length or non-finite polarity vector, e.g., EMT-detached).
pub const POLARITY_UNPOLARIZED: i16 = i16::MIN;

/// Quantize a primary recurrent cell state `h_0 ∈ [−1, 1]` into an [`i8`] in
/// `[−127, 127]` (`V2-16`, `INV-09`). Non-finite inputs map to `0`.
#[must_use]
pub fn quantize_cell_state(h0: f32) -> i8 {
    let value = f64::from(h0);
    if !value.is_finite() {
        return 0;
    }
    ((value.clamp(-1.0, 1.0) * f64::from(CELL_STATE_STEPS)).round() as i32)
        .clamp(-i32::from(CELL_STATE_STEPS), i32::from(CELL_STATE_STEPS)) as i8
}

/// Quantize a world-space polarity vector `[p_x, p_y]` (world x east, y down)
/// into an [`i16`] angle (`V2-16`, `INV-09`). Zero or non-finite vectors map
/// to [`POLARITY_UNPOLARIZED`]; valid angles map `atan2(p_y, p_x) / π × 32767`
/// onto `[−32767, +32767]`.
#[must_use]
pub fn quantize_polarity(polarity: [f32; 2]) -> i16 {
    quantize_polarity_f64([f64::from(polarity[0]), f64::from(polarity[1])])
}

fn quantize_polarity_f64([x, y]: [f64; 2]) -> i16 {
    if !(x.is_finite() && y.is_finite()) || (x == 0.0 && y == 0.0) {
        return POLARITY_UNPOLARIZED;
    }
    let angle = y.atan2(x);
    if !angle.is_finite() {
        return POLARITY_UNPOLARIZED;
    }
    let scaled = (angle / std::f64::consts::PI) * f64::from(POLARITY_ANGLE_SCALE);
    if !scaled.is_finite() {
        return POLARITY_UNPOLARIZED;
    }
    (scaled.round() as i32).clamp(
        -i32::from(POLARITY_ANGLE_SCALE),
        i32::from(POLARITY_ANGLE_SCALE),
    ) as i16
}

/// A block's moment channel sum `moment` per unit of its energy sum
/// `energy` as [`FieldTexture::components`] holds it: `moment / energy`
/// clamped to `[−1, 1]`, times [`COMPONENT_STEPS`], rounded half away from
/// zero; `0` for a block with no energy (`energy` not finite and positive)
/// and for a ratio that is not finite.
fn component_step(moment: f64, energy: f64) -> i8 {
    if !(energy.is_finite() && energy > 0.0) {
        return 0;
    }
    let ratio = moment / energy;
    if !ratio.is_finite() {
        return 0;
    }
    (ratio.clamp(-1.0, 1.0) * f64::from(COMPONENT_STEPS)).round() as i8
}

/// [`AFFINITY_AXES`] as signs, channel by channel: `H[k][c]`. Adding
/// `±1 × v` is exactly adding or subtracting `v`, so a sum over them is the
/// sum [`affinity_projection`]'s fold makes, bit for bit.
const AXIS_SIGNS: [[f64; CHANNELS]; 3] = {
    let mut signs = [[0.0; CHANNELS]; 3];
    let mut axis = 0;
    while axis < 3 {
        let mut channel = 0;
        while channel < CHANNELS {
            signs[axis][channel] = if (AFFINITY_AXES[axis] & channel)
                .count_ones()
                .is_multiple_of(2)
            {
                1.0
            } else {
                -1.0
            };
            channel += 1;
        }
        axis += 1;
    }
    signs
};

/// The projection the affinity view and the field's composition take, as a
/// record carries it (plan §13.4: "A fixed high-dimensional projection has a
/// recorded matrix and scaling"). Display axis `k` of a vector `v` of 32
/// channels is `(Σ_c matrix[k][c] v_c) / (divisor × unit)`, clamped to
/// `[−1, 1]`: the unit is the affinity norm limit for an organism's
/// affinity ([`affinity_projection`]) and a block's energy for the field's
/// composition ([`FieldTexture::composition`]). No entry is fitted to a
/// population, so the same record means the same colours in every world.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AffinityProjection {
    /// The rows of the 32 × 32 Sylvester Hadamard matrix it takes, one per
    /// display axis ([`AFFINITY_AXES`]).
    pub rows: [usize; 3],
    /// The matrix, a row per display axis and a column per channel:
    /// `(−1)^popcount(row & channel)`, each `1` or `−1`.
    pub matrix: [[i8; CHANNELS]; 3],
    /// What each sum is divided by besides its unit: `√32`, the norm of a
    /// row, so the rows are orthonormal after it.
    pub divisor: f64,
}

impl AffinityProjection {
    /// The fixed projection every epoch takes (FL-019): the rows
    /// [`AFFINITY_AXES`] of the Sylvester Hadamard matrix, over `√32`. The
    /// matrix is the one the field's composition sums with, and the one the
    /// affinity view's signs follow, so the record says what is computed.
    #[must_use]
    pub fn fixed() -> Self {
        Self {
            rows: AFFINITY_AXES,
            matrix: AXIS_SIGNS.map(|signs| signs.map(|sign| if sign > 0.0 { 1 } else { -1 })),
            divisor: (CHANNELS as f64).sqrt(),
        }
    }
}

/// The most texels in a field texture's row: a projection carries one row
/// through the moment planes at once, its F64 sums on the stack, reading
/// each plane in one run per row.
const ROW: usize = FIELD_TEXELS_MAX as usize;

/// A view's element type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dtype {
    /// Unsigned 8-bit.
    U8,
    /// Signed 8-bit.
    I8,
    /// Signed 16-bit.
    I16,
    /// Unsigned 32-bit.
    U32,
    /// Unsigned 64-bit.
    U64,
    /// IEEE binary32.
    F32,
}

impl Dtype {
    /// Bytes per element.
    #[must_use]
    pub const fn bytes(self) -> usize {
        match self {
            Self::U8 | Self::I8 => 1,
            Self::I16 => 2,
            Self::U32 | Self::F32 => 4,
            Self::U64 => 8,
        }
    }
}

/// One of an epoch's views.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum View {
    /// Stable organism identities, strictly increasing.
    Identity,
    /// Wrapped positions in world units, `[x, y]` interleaved.
    Position,
    /// Velocities in world units per simulation-second, `[x, y]`.
    Velocity,
    /// Organism energy.
    Energy,
    /// Founder lineage identities.
    Lineage,
    /// Generations since the founder.
    Generation,
    /// Topology catalog indices.
    Topology,
    /// Affinity directions' display coordinates ([`affinity_projection`]).
    Affinity,
    /// The field texture, row-major.
    Field,
    /// Each field texel's mean composition through the same fixed
    /// projection as the affinity ([`FieldTexture::composition`]),
    /// row-major.
    FieldComposition,
    /// Each field texel's moment channels per unit energy, quantized for
    /// display ([`FieldTexture::components`]), row-major.
    FieldComponents,
    /// Optional `V2-16` primary recurrent cell state `h_0` quantized onto
    /// `[−127, 127]` (`1 B/org`, present only when `v2-visual-mappings` and
    /// recurrence are enabled).
    CellState,
    /// Optional `V2-16` apical-basal polarity angle quantized onto
    /// `[−32767, 32767]` with [`POLARITY_UNPOLARIZED`] sentinel (`2 B/org`,
    /// present only when `v2-visual-mappings` and `pcp-polarity` are enabled).
    Polarity,
    /// Optional `V2-16` block-mean Carrier-1 trail energy density (`4 B/texel`,
    /// present only when `v2-visual-mappings` and `dual-carrier-trails` are
    /// enabled).
    FieldTrailEnergy,
    /// Optional `V2-16` block-mean incident vascular conductance (`4 B/texel`,
    /// present only when `v2-visual-mappings` and `vascular-field` are
    /// enabled).
    FieldVascularConductance,
}

impl View {
    /// Every always-present baseline view, in declaration order.
    pub const ALL: [Self; 11] = [
        Self::Identity,
        Self::Position,
        Self::Velocity,
        Self::Energy,
        Self::Lineage,
        Self::Generation,
        Self::Topology,
        Self::Affinity,
        Self::Field,
        Self::FieldComposition,
        Self::FieldComponents,
    ];

    /// Every optional `V2-16` view, in declaration order. Each is allocated
    /// only when `v2-visual-mappings` and its backing `field-ecology-v2`
    /// feature are enabled (`0` bytes reserved on v1 or when disabled).
    pub const V2_OPTIONAL: [Self; 4] = [
        Self::CellState,
        Self::Polarity,
        Self::FieldTrailEnergy,
        Self::FieldVascularConductance,
    ];

    /// The view's name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::Position => "position",
            Self::Velocity => "velocity",
            Self::Energy => "energy",
            Self::Lineage => "lineage",
            Self::Generation => "generation",
            Self::Topology => "topology",
            Self::Affinity => "affinity",
            Self::Field => "field",
            Self::FieldComposition => "field-composition",
            Self::FieldComponents => "field-components",
            Self::CellState => "cell-state",
            Self::Polarity => "polarity",
            Self::FieldTrailEnergy => "field-trail-energy",
            Self::FieldVascularConductance => "field-vascular-conductance",
        }
    }
}

/// What a view's values may be.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Bounds {
    /// Strictly increasing: stable identity order.
    StrictlyIncreasing,
    /// Component `c` in `[0, extent[c])`.
    Wrapped([f32; 2]),
    /// Every component's magnitude at most this.
    Magnitude(f32),
    /// Finite and at least zero.
    NonNegative,
    /// Any value of the element type.
    Any,
}

/// A view's declared layout (B.3).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    /// The view.
    pub view: View,
    /// Its element type.
    pub dtype: Dtype,
    /// Elements per row.
    pub components: usize,
    /// Rows.
    pub count: usize,
    /// What every value satisfies.
    pub bounds: Bounds,
}

impl Layout {
    /// The view's bytes.
    #[must_use]
    pub const fn bytes(&self) -> usize {
        self.count
            .saturating_mul(self.components)
            .saturating_mul(self.dtype.bytes())
    }
}

/// The epoch an epoch replaced as the latest displayable one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prior {
    /// Its identity.
    pub id: RenderEpoch,
    /// Its source tick.
    pub tick: Tick,
}

/// What a field texel shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldChannel {
    /// Plane 0: the mean energy density (energy per unit area) over the
    /// texel's block of cells.
    EnergyDensity,
}

/// A bounded field texture derived from the committed field (§9.5, B.3:
/// an explicit channel projection and source scale): each texel's mean
/// energy density, and its mean composition through a fixed projection and
/// as chosen components (plan §13.4, "Local field"). When `V2-16`
/// (`v2-visual-mappings`) and the corresponding `field-ecology-v2` features
/// are enabled, it also holds optional fixed-capacity trail-energy and
/// vascular-conductance overlay planes (`None` and `0` bytes reserved on v1
/// or when disabled).
#[derive(Debug, Clone, PartialEq)]
pub struct FieldTexture {
    channel: FieldChannel,
    cells: [u32; 2],
    block: [u32; 2],
    texels: [u32; 2],
    texel_size: [f64; 2],
    values: Vec<f32>,
    composition: Vec<[f32; 3]>,
    components: Vec<[i8; CHANNELS]>,
    trail_energy: Option<Vec<f32>>,
    vascular_conductance: Option<Vec<f32>>,
}

impl FieldTexture {
    /// The texture of one row-major energy `plane` on a `cells` grid of
    /// `cell_size` cells, with no chemical moment: for previews and checks
    /// outside an epoch.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] for an empty grid or a plane of the
    /// wrong length; [`CoreError::AllocationFailed`].
    pub fn from_plane(
        cells: [u32; 2],
        cell_size: [f64; 2],
        plane: &[f32],
    ) -> Result<Self, CoreError> {
        Self::from_plane_on(cells, cell_size, plane, 1)
    }

    /// As [`FieldTexture::from_plane`], its texel rows projected on at most
    /// `workers` threads: the same texture bit for bit.
    ///
    /// # Errors
    /// As [`FieldTexture::from_plane`], and [`CoreError::StateConflict`]
    /// when a worker panicked.
    pub fn from_plane_on(
        cells: [u32; 2],
        cell_size: [f64; 2],
        plane: &[f32],
        workers: usize,
    ) -> Result<Self, CoreError> {
        let mut texture = Self::sized(cells, cell_size)?;
        texture.project_on(plane, cell_size[0] * cell_size[1], workers)?;
        Ok(texture)
    }

    /// The texture of a whole field's `planes` in the authoritative planar
    /// layout (plane 0 energy, plane `k + 1` moment channel `k`, each
    /// row-major) on a `cells` grid of `cell_size` cells: what an epoch
    /// projects, for previews and checks outside one.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] for an empty grid or planes of the
    /// wrong length; [`CoreError::AllocationFailed`].
    pub fn from_planes(
        cells: [u32; 2],
        cell_size: [f64; 2],
        planes: &[f32],
    ) -> Result<Self, CoreError> {
        Self::from_planes_on(cells, cell_size, planes, 1)
    }

    /// As [`FieldTexture::from_planes`], its texel rows projected on at
    /// most `workers` threads, as an epoch projects them on the world's:
    /// the same texture bit for bit.
    ///
    /// # Errors
    /// As [`FieldTexture::from_planes`], and [`CoreError::StateConflict`]
    /// when a worker panicked.
    pub fn from_planes_on(
        cells: [u32; 2],
        cell_size: [f64; 2],
        planes: &[f32],
        workers: usize,
    ) -> Result<Self, CoreError> {
        let mut texture = Self::sized(cells, cell_size)?;
        let cell_count = usize::try_from(cells[0]).ok().and_then(|x| {
            usize::try_from(cells[1])
                .ok()
                .and_then(|y| x.checked_mul(y))
        });
        let valid = cell_count.is_some_and(|n| {
            n.checked_mul(1 + CHANNELS) == Some(planes.len())
                || n.checked_mul(DUAL_CARRIER_PLANES) == Some(planes.len())
        });
        if !valid {
            return Err(CoreError::LayoutMismatch {
                buffer: "render field texture",
                detail: format!(
                    "{} values are not the {} or {DUAL_CARRIER_PLANES} planes of a {} × {} grid",
                    planes.len(),
                    1 + MOMENT_CHANNELS,
                    cells[0],
                    cells[1]
                ),
            });
        }
        texture.project_on(planes, cell_size[0] * cell_size[1], workers)?;
        Ok(texture)
    }

    /// As [`FieldTexture::from_planes`], also projecting optional `V2-16`
    /// scalar overlay planes (Carrier-1 trail energy density and incident
    /// vascular conductance), each block-averaged in F64 rows-then-columns.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] for an empty grid or a plane of the
    /// wrong length; [`CoreError::AllocationFailed`].
    pub fn from_planes_with_overlays(
        cells: [u32; 2],
        cell_size: [f64; 2],
        planes: &[f32],
        trail_plane: Option<&[f32]>,
        vascular_plane: Option<&[f32]>,
    ) -> Result<Self, CoreError> {
        Self::from_planes_with_overlays_on(cells, cell_size, planes, trail_plane, vascular_plane, 1)
    }

    /// As [`FieldTexture::from_planes_with_overlays`], its texel rows
    /// projected on at most `workers` threads: bit-identical to `workers = 1`.
    ///
    /// # Errors
    /// As [`FieldTexture::from_planes_with_overlays`], and
    /// [`CoreError::StateConflict`] when a worker panicked.
    pub fn from_planes_with_overlays_on(
        cells: [u32; 2],
        cell_size: [f64; 2],
        planes: &[f32],
        trail_plane: Option<&[f32]>,
        vascular_plane: Option<&[f32]>,
        workers: usize,
    ) -> Result<Self, CoreError> {
        let mut texture = Self::sized_with_v2(
            cells,
            cell_size,
            trail_plane.is_some(),
            vascular_plane.is_some(),
        )?;
        let cell_count = usize::try_from(cells[0]).ok().and_then(|x| {
            usize::try_from(cells[1])
                .ok()
                .and_then(|y| x.checked_mul(y))
        });
        let valid = cell_count.is_some_and(|n| {
            n.checked_mul(1 + CHANNELS) == Some(planes.len())
                || n.checked_mul(DUAL_CARRIER_PLANES) == Some(planes.len())
        });
        if !valid {
            return Err(CoreError::LayoutMismatch {
                buffer: "render field texture",
                detail: format!(
                    "{} values are not the {} or {DUAL_CARRIER_PLANES} planes of a {} × {} grid",
                    planes.len(),
                    1 + MOMENT_CHANNELS,
                    cells[0],
                    cells[1]
                ),
            });
        }
        let cell_area = cell_size[0] * cell_size[1];
        texture.project_on(planes, cell_area, workers)?;
        texture.project_overlays_on(trail_plane, vascular_plane, cell_area, workers)?;
        Ok(texture)
    }

    /// A texture for a `cells` grid of `cell_size` cells, allocated once, with
    /// no optional `V2-16` overlays.
    fn sized(cells: [u32; 2], cell_size: [f64; 2]) -> Result<Self, CoreError> {
        Self::sized_with_v2(cells, cell_size, false, false)
    }

    /// A texture for a `cells` grid of `cell_size` cells, allocated once, with
    /// optional `V2-16` trail-energy and vascular-conductance overlay planes
    /// reserved only when requested (`INV-15`).
    fn sized_with_v2(
        cells: [u32; 2],
        cell_size: [f64; 2],
        trail: bool,
        vascular: bool,
    ) -> Result<Self, CoreError> {
        if cells.contains(&0) {
            return Err(CoreError::LayoutMismatch {
                buffer: "render field texture",
                detail: format!("a {cells:?} grid has no cells"),
            });
        }
        let cell_area = cell_size[0] * cell_size[1];
        if !cell_size.iter().all(|side| side.is_finite() && *side > 0.0)
            || !(cell_area.is_finite() && cell_area > 0.0)
        {
            return Err(CoreError::NumericalInvalid {
                quantity: "render field cell size",
                detail: format!("{cell_size:?} must be finite with positive area"),
            });
        }
        let block = cells.map(|n| n.div_ceil(FIELD_TEXELS_MAX));
        let texels = [cells[0].div_ceil(block[0]), cells[1].div_ceil(block[1])];
        let count = usize::try_from(u64::from(texels[0]) * u64::from(texels[1])).map_err(|_| {
            CoreError::Overflow {
                quantity: "render texel count",
            }
        })?;
        Ok(Self {
            channel: FieldChannel::EnergyDensity,
            cells,
            block,
            texels,
            texel_size: [
                f64::from(block[0]) * cell_size[0],
                f64::from(block[1]) * cell_size[1],
            ],
            values: reserved(count, "render field texture")?,
            composition: reserved(count, "render field composition")?,
            components: reserved(count, "render field components")?,
            trail_energy: trail
                .then(|| reserved(count, "render field trail energy"))
                .transpose()?,
            vascular_conductance: vascular
                .then(|| reserved(count, "render field vascular conductance"))
                .transpose()?,
        })
    }

    /// The channel.
    #[must_use]
    pub const fn channel(&self) -> FieldChannel {
        self.channel
    }

    /// The source grid, `[x, y]` cells.
    #[must_use]
    pub const fn cells(&self) -> [u32; 2] {
        self.cells
    }

    /// Source cells per texel along each axis. The last texel of an axis
    /// covers the remainder.
    #[must_use]
    pub const fn block(&self) -> [u32; 2] {
        self.block
    }

    /// Texels, `[x, y]`.
    #[must_use]
    pub const fn texels(&self) -> [u32; 2] {
        self.texels
    }

    /// A whole texel's size in world units: the source scale.
    #[must_use]
    pub const fn texel_size(&self) -> [f64; 2] {
        self.texel_size
    }

    /// The texels, row-major.
    #[must_use]
    pub fn values(&self) -> &[f32] {
        &self.values
    }

    /// The texture's layout.
    #[must_use]
    pub fn layout(&self) -> Layout {
        Layout {
            view: View::Field,
            dtype: Dtype::F32,
            components: 1,
            count: self.values.len(),
            bounds: Bounds::NonNegative,
        }
    }

    /// Each texel's mean chemical composition, row-major, through the
    /// affinity's fixed projection (plan §13.4, "Local field"): the
    /// block's moments summed channel by channel, `Σm`, and its energy,
    /// `Σe`, give [`affinity_projection`]`(Σm, Σe)`, that is row `k` of
    /// [`AFFINITY_AXES`] dotted with `Σm` over `√32 Σe`. It is the mean
    /// composition per unit energy `Σm / Σe`, weighted by energy, in the
    /// coordinates an affinity direction at its norm limit takes, so an
    /// organism and the material it converts best (`a / ‖a‖`) share them.
    /// Every cell holds `‖m‖₂ ≤ e` (§4.2), so a block's composition is in
    /// the unit ball. A block with no energy, and a texture made from an
    /// energy plane alone, have none: `[0; 3]`.
    #[must_use]
    pub fn composition(&self) -> &[[f32; 3]] {
        &self.composition
    }

    /// The composition's layout.
    #[must_use]
    pub fn composition_layout(&self) -> Layout {
        Layout {
            view: View::FieldComposition,
            dtype: Dtype::F32,
            components: 3,
            count: self.composition.len(),
            bounds: Bounds::Magnitude(1.0),
        }
    }

    /// Each texel's moment channels per unit energy, row-major, channel `c`
    /// at index `c` (plan §13.4, "Local field": chosen components): the
    /// block's moment channel `c` summed, `Σm_c`, over its energy summed,
    /// `Σe`, both as [`FieldTexture::composition`] sums them, held as the
    /// nearest multiple of `1 / `[`COMPONENT_STEPS`]: `round(127 Σm_c /
    /// Σe)`, clamped to `[−127, 127]`, rounded half away from zero. Every
    /// cell holds `|m_c| ≤ ‖m‖₂ ≤ e` (§4.2), so the ratio is in `[−1, 1]`
    /// and the clamp guards only rounding. A block with no energy, and a
    /// texture made from an energy plane alone, have none: zeros. For
    /// display only, as every view is: nothing scientific reads it.
    #[must_use]
    pub fn components(&self) -> &[[i8; CHANNELS]] {
        &self.components
    }

    /// The components' layout.
    #[must_use]
    pub fn components_layout(&self) -> Layout {
        Layout {
            view: View::FieldComponents,
            dtype: Dtype::I8,
            components: CHANNELS,
            count: self.components.len(),
            bounds: Bounds::Magnitude(f32::from(COMPONENT_STEPS)),
        }
    }

    /// Optional `V2-16` block-mean Carrier-1 trail energy density (`e_1`),
    /// row-major (`texels[0] * texels[1]`): `Some` only when
    /// `v2-visual-mappings` and `dual-carrier-trails` are enabled (`None` and
    /// `0` bytes reserved on v1 or when disabled).
    #[must_use]
    pub fn trail_energy(&self) -> Option<&[f32]> {
        self.trail_energy.as_deref()
    }

    /// The trail-energy overlay's declared layout, when present.
    #[must_use]
    pub fn trail_energy_layout(&self) -> Option<Layout> {
        self.trail_energy.as_ref().map(|values| Layout {
            view: View::FieldTrailEnergy,
            dtype: Dtype::F32,
            components: 1,
            count: values.len(),
            bounds: Bounds::NonNegative,
        })
    }

    /// Optional `V2-16` block-mean incident vascular conductance (`g_e`),
    /// row-major (`texels[0] * texels[1]`): `Some` only when
    /// `v2-visual-mappings` and `vascular-field` are enabled (`None` and `0`
    /// bytes reserved on v1 or when disabled).
    #[must_use]
    pub fn vascular_conductance(&self) -> Option<&[f32]> {
        self.vascular_conductance.as_deref()
    }

    /// The vascular-conductance overlay's declared layout, when present.
    #[must_use]
    pub fn vascular_conductance_layout(&self) -> Option<Layout> {
        self.vascular_conductance.as_ref().map(|values| Layout {
            view: View::FieldVascularConductance,
            dtype: Dtype::F32,
            components: 1,
            count: values.len(),
            bounds: Bounds::NonNegative,
        })
    }

    /// Refill optional `V2-16` overlay buffers in place without reallocating.
    /// Validates both offered planes upfront so a refusal leaves neither buffer
    /// mutated. When a buffer is reserved but its backing plane is not yet
    /// supplied by the field state, it is zero-filled cleanly.
    fn project_overlays_on(
        &mut self,
        trail_plane: Option<&[f32]>,
        vascular_plane: Option<&[f32]>,
        cell_area: f64,
        workers: usize,
    ) -> Result<(), CoreError> {
        let cells = self.cells.map(|n| n as usize);
        let block = self.block.map(|n| n as usize);
        let texels = self.texels.map(|n| n as usize);
        if self.trail_energy.is_some() {
            if let Some(plane) = trail_plane {
                validate_scalar_plane(plane, cells, "render field trail energy")?;
            }
        } else if trail_plane.is_some() {
            return Err(CoreError::LayoutMismatch {
                buffer: "render field trail energy",
                detail: "trail plane offered to a texture without a trail buffer".to_owned(),
            });
        }
        if self.vascular_conductance.is_some() {
            if let Some(plane) = vascular_plane {
                validate_scalar_plane(plane, cells, "render field vascular conductance")?;
            }
        } else if vascular_plane.is_some() {
            return Err(CoreError::LayoutMismatch {
                buffer: "render field vascular conductance",
                detail: "vascular plane offered to a texture without a vascular buffer".to_owned(),
            });
        }
        if let Some(out) = &mut self.trail_energy {
            if let Some(plane) = trail_plane {
                let grid = ScalarOverlayGrid {
                    cells,
                    block,
                    texels,
                    divisor: cell_area,
                };
                project_validated_scalar_plane_on(plane, grid, workers, out)?;
            } else {
                out.clear();
                out.resize(texels[0] * texels[1], 0.0);
            }
        }
        if let Some(out) = &mut self.vascular_conductance {
            if let Some(plane) = vascular_plane {
                let grid = ScalarOverlayGrid {
                    cells,
                    block,
                    texels,
                    divisor: 1.0,
                };
                project_validated_scalar_plane_on(plane, grid, workers, out)?;
            } else {
                out.clear();
                out.resize(texels[0] * texels[1], 0.0);
            }
        }
        Ok(())
    }

    /// Refill optional `V2-16` overlay buffers from a live committed world
    /// (`trail_energy` Carrier-1 plane and `(edge_x, edge_y)` vascular face
    /// conductances) in place without reallocating (`INV-15`). Validates both
    /// inputs upfront so a refusal leaves neither overlay buffer mutated.
    fn project_overlays_from_world_on(
        &mut self,
        trail_plane: Option<&[f32]>,
        vascular_edges: Option<(&[f32], &[f32])>,
        cell_area: f64,
        workers: usize,
    ) -> Result<(), CoreError> {
        let cells = self.cells.map(|n| n as usize);
        let block = self.block.map(|n| n as usize);
        let texels = self.texels.map(|n| n as usize);
        if self.trail_energy.is_some()
            && let Some(plane) = trail_plane
        {
            validate_scalar_plane(plane, cells, "render field trail energy")?;
        }
        if self.vascular_conductance.is_some()
            && let Some((edge_x, edge_y)) = vascular_edges
        {
            validate_vascular_edges(edge_x, edge_y, cells)?;
        }
        if let Some(out) = &mut self.trail_energy {
            if let Some(plane) = trail_plane {
                let grid = ScalarOverlayGrid {
                    cells,
                    block,
                    texels,
                    divisor: cell_area,
                };
                project_validated_scalar_plane_on(plane, grid, workers, out)?;
            } else {
                out.clear();
                out.resize(texels[0] * texels[1], 0.0);
            }
        }
        if let Some(out) = &mut self.vascular_conductance {
            if let Some((edge_x, edge_y)) = vascular_edges {
                let grid = ScalarOverlayGrid {
                    cells,
                    block,
                    texels,
                    divisor: 1.0,
                };
                project_validated_vascular_edges_on(edge_x, edge_y, grid, workers, out)?;
            } else {
                out.clear();
                out.resize(texels[0] * texels[1], 0.0);
            }
        }
        Ok(())
    }

    /// Refill from row-major `planes` of `cell_area` cells: an energy plane
    /// alone, the 33 planes of a single-carrier field, or the 34 planes of a
    /// `field-ecology-v2` dual-carrier field (projecting Carrier-0's `e_0` and
    /// 24 moment channels `m_0..m_23`, while Carrier-1's `e_1` projects via
    /// `trail_energy`). Each texel sums its block's energy in F64, rows then
    /// columns, and divides it by the block's area; with moments, it sums each
    /// channel the same way, projects the sums over the energy's
    /// ([`FieldTexture::composition`]) and holds each over the energy's
    /// ([`FieldTexture::components`]).
    ///
    /// The texel rows are split into contiguous runs on at most `workers`
    /// threads, the calling thread's first, each with at least
    /// [`MINIMUM_TEXEL_ROWS_PER_WORKER`] rows. A texel is computed from its
    /// own block alone, in the same order on any thread, so the texture is
    /// the one-thread texture bit for bit.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] for planes of the wrong length, which
    /// leaves the texture as it was; [`CoreError::StateConflict`] when a
    /// worker panicked.
    fn project_on(
        &mut self,
        planes: &[f32],
        cell_area: f64,
        workers: usize,
    ) -> Result<(), CoreError> {
        let [cells_x, cells_y] = self.cells.map(|n| n as usize);
        let cells = usize::try_from(self.cells[0]).ok().and_then(|x| {
            usize::try_from(self.cells[1])
                .ok()
                .and_then(|y| x.checked_mul(y))
        });
        let single_carrier = cells
            .and_then(|cells| cells.checked_mul(1 + CHANNELS))
            .is_some_and(|total| planes.len() == total);
        let dual_carrier = cells
            .and_then(|cells| cells.checked_mul(DUAL_CARRIER_PLANES))
            .is_some_and(|total| planes.len() == total);
        let Some(cells) =
            cells.filter(|&cells| planes.len() == cells || single_carrier || dual_carrier)
        else {
            return Err(CoreError::LayoutMismatch {
                buffer: "render field texture",
                detail: format!(
                    "{} values for an energy plane, {} single-carrier planes, or \
                     {DUAL_CARRIER_PLANES} dual-carrier planes of a {} × {} grid",
                    planes.len(),
                    1 + CHANNELS,
                    self.cells[0],
                    self.cells[1]
                ),
            });
        };
        let (energy, moments) = if dual_carrier {
            (
                &planes[..cells],
                &planes[cells..cells * (1 + CARRIER0_MOMENT_CHANNELS)],
            )
        } else {
            planes.split_at(cells)
        };
        validate_scalar_plane(energy, [cells_x, cells_y], "render field energy")?;
        let [width, height] = self.texels.map(|n| n as usize);
        let count = width * height;
        // The buffers' capacity is the texture's, so resizing once never
        // allocates; steady-state reused slots keep `len == count` and skip
        // ~5 MiB of redundant single-threaded zeroing per epoch (`Rows::project`
        // unconditionally writes every texel in parallel).
        resize_len(&mut self.values, count, 0.0);
        resize_len(&mut self.composition, count, [0.0; 3]);
        resize_len(&mut self.components, count, [0; CHANNELS]);
        let rows = Rows {
            energy,
            moments,
            cells: [cells_x, cells_y],
            block: self.block.map(|n| n as usize),
            width,
            cell_area,
        };
        let used = workers
            .min(height.div_ceil(MINIMUM_TEXEL_ROWS_PER_WORKER))
            .max(1);
        if used == 1 {
            rows.project(
                0,
                &mut self.values,
                &mut self.composition,
                &mut self.components,
            );
            return Ok(());
        }
        let per = height.div_ceil(used).max(1);
        let texels = per * width;
        let mut runs = self
            .values
            .chunks_mut(texels)
            .zip(self.composition.chunks_mut(texels))
            .zip(self.components.chunks_mut(texels));
        let Some(((values, composition), components)) = runs.next() else {
            return Ok(());
        };
        // Pass the caller's configured `workers` budget (not clamped `used`)
        // so render and engine phases reuse one cached private pool.
        flife_core::pool::scope_workers(workers, "project render field texture", |scope| {
            for (index, ((values, composition), components)) in runs.enumerate() {
                scope.spawn(move || {
                    rows.project((index + 1) * per, values, composition, components);
                });
            }
            rows.project(0, values, composition, components);
        })
    }
}

/// Resize `vec` to `len` without zeroing an already-initialized prefix.
#[inline]
fn resize_len<T: Copy>(vec: &mut Vec<T>, len: usize, fill: T) {
    if vec.len() < len {
        vec.resize(len, fill);
    } else {
        vec.truncate(len);
    }
}

/// Exact reciprocal `1.0 / d` when `d` is a positive power of two in normal
/// range (`[2^-500, 2^500]`), so `sum * inv` is bit-identical to `sum / d`
/// (pure exponent shift, zero rounding) for every finite `f64` summed from
/// `f32` cells.
#[inline]
fn exact_power_of_two_recip(d: f64) -> Option<f64> {
    if d.is_finite() && d > 0.0 {
        let bits = d.to_bits();
        let mantissa = bits & ((1_u64 << 52) - 1);
        let exp = ((bits >> 52) & 0x7ff) as i32 - 1023;
        if mantissa == 0 && (-500..=500).contains(&exp) {
            return Some(1.0 / d);
        }
    }
    None
}

/// Half-integer safety margin for precomputed `127.0 / energy` multiplication:
/// `|fl(m * fl(127 / e)) - fl(fl(m / e) * 127)| < 6e-14 << 1e-10`, so any value
/// at least `1e-10` from every half-integer `k + 0.5` has a provably identical
/// `.round()` result; anything closer (or non-finite / out of `[-127, 127]`)
/// falls back to exact [`component_step`].
const FAST_ROUND_MARGIN: f64 = 0.499_999_999_9;

/// Single-pass fused dot-product accumulation and quantized component-step
/// projection for one whole-cell (`1×1`) moment channel row.
#[inline]
#[expect(
    clippy::too_many_arguments,
    reason = "hot inner loop takes disjoint pre-sliced row buffers directly"
)]
fn project_channel_f32(
    run: &[f32],
    energies: &[f64],
    scales: &[f64],
    signs: [f64; 3],
    d0s: &mut [f64],
    d1s: &mut [f64],
    d2s: &mut [f64],
    steps: &mut [i8],
) {
    let [s0, s1, s2] = signs;
    let mut fallback = 0_u8;
    for (((((d0, d1), d2), step_out), &scale), &raw_m) in d0s
        .iter_mut()
        .zip(d1s.iter_mut())
        .zip(d2s.iter_mut())
        .zip(steps.iter_mut())
        .zip(scales)
        .zip(run)
    {
        let m = f64::from(raw_m);
        *d0 += s0 * m;
        *d1 += s1 * m;
        *d2 += s2 * m;
        let scaled = m * scale;
        let rounded = scaled.round();
        let ok = (rounded.abs() <= 127.0) & ((scaled - rounded).abs() < FAST_ROUND_MARGIN);
        fallback |= u8::from(!ok);
        *step_out = rounded as i8;
    }
    if fallback != 0 {
        for ((step_out, &raw_m), (&scale, &energy)) in
            steps.iter_mut().zip(run).zip(scales.iter().zip(energies))
        {
            let m = f64::from(raw_m);
            let scaled = m * scale;
            let rounded = scaled.round();
            if !((rounded.abs() <= 127.0) && ((scaled - rounded).abs() < FAST_ROUND_MARGIN)) {
                *step_out = component_step(m, energy);
            }
        }
    }
}

/// Single-pass fused dot-product accumulation and quantized component-step
/// projection for one pre-summed (`block > 1`) moment channel row.
#[inline]
#[expect(
    clippy::too_many_arguments,
    reason = "hot inner loop takes disjoint pre-sliced row buffers directly"
)]
fn project_channel_f64(
    sums: &[f64],
    energies: &[f64],
    scales: &[f64],
    signs: [f64; 3],
    d0s: &mut [f64],
    d1s: &mut [f64],
    d2s: &mut [f64],
    steps: &mut [i8],
) {
    let [s0, s1, s2] = signs;
    let mut fallback = 0_u8;
    for (((((d0, d1), d2), step_out), &scale), &m) in d0s
        .iter_mut()
        .zip(d1s.iter_mut())
        .zip(d2s.iter_mut())
        .zip(steps.iter_mut())
        .zip(scales)
        .zip(sums)
    {
        *d0 += s0 * m;
        *d1 += s1 * m;
        *d2 += s2 * m;
        let scaled = m * scale;
        let rounded = scaled.round();
        let ok = (rounded.abs() <= 127.0) & ((scaled - rounded).abs() < FAST_ROUND_MARGIN);
        fallback |= u8::from(!ok);
        *step_out = rounded as i8;
    }
    if fallback != 0 {
        for ((step_out, &m), (&scale, &energy)) in
            steps.iter_mut().zip(sums).zip(scales.iter().zip(energies))
        {
            let scaled = m * scale;
            let rounded = scaled.round();
            if !((rounded.abs() <= 127.0) && ((scaled - rounded).abs() < FAST_ROUND_MARGIN)) {
                *step_out = component_step(m, energy);
            }
        }
    }
}

/// The fewest texel rows worth handing to another thread when a texture is
/// projected on workers.
pub const MINIMUM_TEXEL_ROWS_PER_WORKER: usize = 8;

/// What a texture's texel rows are projected from: its planes, grid and
/// blocks. Any run of whole rows projects alone.
#[derive(Clone, Copy)]
struct Rows<'a> {
    /// The energy plane.
    energy: &'a [f32],
    /// The moment planes in channel order; empty for an energy plane alone.
    moments: &'a [f32],
    /// The grid, `[x, y]` cells.
    cells: [usize; 2],
    /// Cells per texel, `[x, y]`.
    block: [usize; 2],
    /// Texels per row.
    width: usize,
    /// A cell's area.
    cell_area: f64,
}

impl Rows<'_> {
    /// Project texel rows `first..`, as many whole rows as `values` holds,
    /// into `values`, `composition` and `components`, which hold the same
    /// count of texels; every texel in all three buffers is written
    /// unconditionally, bit-identical to the independent block oracle.
    fn project(
        self,
        first: usize,
        values: &mut [f32],
        composition: &mut [[f32; 3]],
        components: &mut [[i8; CHANNELS]],
    ) {
        let [cells_x, cells_y] = self.cells;
        let [block_x, block_y] = self.block;
        let width = self.width;
        let whole = block_x == 1 && block_y == 1;
        let plane_cells = cells_x * cells_y;
        let num_channels = if plane_cells > 0 {
            self.moments.len() / plane_cells
        } else {
            0
        };
        let full_x = cells_x / block_x;
        let inv_cell_area = exact_power_of_two_recip(self.cell_area);
        let sqrt_channels = (CHANNELS as f64).sqrt();

        // A texel row at a time: each plane is read in one contiguous run per
        // row (avoiding 32-way L1D set conflicts on power-of-two/320² planes),
        // accumulating the three Hadamard dot products and contiguous quantized
        // component rows in a single fused pass, then transposing 8×32 tiles
        // sequentially into `row_components`.
        let mut energies = [0.0_f64; ROW];
        let mut scales_127 = [0.0_f64; ROW];
        let mut sums = [0.0_f64; ROW];
        let mut dots = [[0.0_f64; ROW]; 3];
        let mut channel_steps = [[0_i8; ROW]; CHANNELS];

        let rows_out = values
            .chunks_exact_mut(width)
            .zip(composition.chunks_exact_mut(width))
            .zip(components.chunks_exact_mut(width));
        for (row, ((row_values, row_composition), row_components)) in rows_out.enumerate() {
            let texel_y = first + row;
            let rows = texel_y * block_y..((texel_y + 1) * block_y).min(cells_y);

            if whole {
                let energy_row = &self.energy[texel_y * cells_x..][..width];
                if let Some(inv_area) = inv_cell_area {
                    for ((total, value), &raw) in energies[..width]
                        .iter_mut()
                        .zip(row_values.iter_mut())
                        .zip(energy_row)
                    {
                        let sum = f64::from(raw);
                        *total = sum;
                        *value = saturate_nonnegative(sum * inv_area);
                    }
                } else {
                    let cell_area = self.cell_area;
                    for ((total, value), &raw) in energies[..width]
                        .iter_mut()
                        .zip(row_values.iter_mut())
                        .zip(energy_row)
                    {
                        let sum = f64::from(raw);
                        *total = sum;
                        *value = saturate_nonnegative(sum / cell_area);
                    }
                }
            } else {
                let row_energies = &mut energies[..width];
                row_energies.fill(0.0);
                for y in rows.clone() {
                    let line = &self.energy[y * cells_x..(y + 1) * cells_x];
                    for (sum, chunk) in row_energies[..full_x]
                        .iter_mut()
                        .zip(line[..full_x * block_x].chunks_exact(block_x))
                    {
                        for &value in chunk {
                            *sum += f64::from(value);
                        }
                    }
                    if width > full_x {
                        let rem = &mut row_energies[full_x];
                        for &value in &line[full_x * block_x..cells_x] {
                            *rem += f64::from(value);
                        }
                    }
                }
                let full_area = (rows.len() * block_x) as f64 * self.cell_area;
                if let Some(inv_area) = exact_power_of_two_recip(full_area) {
                    for (&sum, value) in row_energies[..full_x]
                        .iter()
                        .zip(row_values[..full_x].iter_mut())
                    {
                        *value = saturate_nonnegative(sum * inv_area);
                    }
                } else {
                    for (&sum, value) in row_energies[..full_x]
                        .iter()
                        .zip(row_values[..full_x].iter_mut())
                    {
                        *value = saturate_nonnegative(sum / full_area);
                    }
                }
                if width > full_x {
                    let rem_area =
                        (rows.len() * (cells_x - full_x * block_x)) as f64 * self.cell_area;
                    row_values[full_x] = saturate_nonnegative(row_energies[full_x] / rem_area);
                }
            }

            if num_channels == 0 {
                row_composition.fill([0.0; 3]);
                row_components.fill([0; CHANNELS]);
                continue;
            }

            // Compute `127.0 / energy` once per texel (32× fewer fdivs) and
            // locate the active span `[first_nz, last_nz)` with positive energy.
            let mut first_nz = width;
            let mut last_nz = 0_usize;
            for (x, (&energy, scale)) in energies[..width]
                .iter()
                .zip(scales_127[..width].iter_mut())
                .enumerate()
            {
                if energy > 0.0 {
                    if first_nz == width {
                        first_nz = x;
                    }
                    last_nz = x + 1;
                    *scale = if (1.0e-100..=1.0e100).contains(&energy) {
                        f64::from(COMPONENT_STEPS) / energy
                    } else {
                        f64::NAN
                    };
                } else {
                    *scale = 0.0;
                }
            }
            if first_nz == width {
                row_composition.fill([0.0; 3]);
                row_components.fill([0; CHANNELS]);
                continue;
            }
            if first_nz > 0 {
                row_composition[..first_nz].fill([0.0; 3]);
                row_components[..first_nz].fill([0; CHANNELS]);
            }
            if last_nz < width {
                row_composition[last_nz..].fill([0.0; 3]);
                row_components[last_nz..].fill([0; CHANNELS]);
            }

            let active_len = last_nz - first_nz;
            let energies_act = &energies[first_nz..last_nz];
            let scales_act = &scales_127[first_nz..last_nz];
            let [d0_all, d1_all, d2_all] = &mut dots;
            let d0s = &mut d0_all[..active_len];
            let d1s = &mut d1_all[..active_len];
            let d2s = &mut d2_all[..active_len];
            d0s.fill(0.0);
            d1s.fill(0.0);
            d2s.fill(0.0);

            if whole {
                let row_offset = texel_y * cells_x + first_nz;
                for (channel, (plane, steps_buf)) in self
                    .moments
                    .chunks_exact(plane_cells)
                    .zip(channel_steps.iter_mut())
                    .enumerate()
                {
                    let signs = [
                        AXIS_SIGNS[0][channel],
                        AXIS_SIGNS[1][channel],
                        AXIS_SIGNS[2][channel],
                    ];
                    let run = &plane[row_offset..row_offset + active_len];
                    project_channel_f32(
                        run,
                        energies_act,
                        scales_act,
                        signs,
                        d0s,
                        d1s,
                        d2s,
                        &mut steps_buf[..active_len],
                    );
                }
            } else {
                let full_end = last_nz.min(full_x);
                let has_full = first_nz < full_end;
                let has_rem = last_nz > full_x;
                let full_count = full_end.saturating_sub(first_nz);
                let active_sums = &mut sums[..active_len];
                for (channel, (plane, steps_buf)) in self
                    .moments
                    .chunks_exact(plane_cells)
                    .zip(channel_steps.iter_mut())
                    .enumerate()
                {
                    let signs = [
                        AXIS_SIGNS[0][channel],
                        AXIS_SIGNS[1][channel],
                        AXIS_SIGNS[2][channel],
                    ];
                    active_sums.fill(0.0);
                    for y in rows.clone() {
                        let line = &plane[y * cells_x..(y + 1) * cells_x];
                        if has_full {
                            let active_line = &line[first_nz * block_x..full_end * block_x];
                            for (sum, chunk) in active_sums[..full_count]
                                .iter_mut()
                                .zip(active_line.chunks_exact(block_x))
                            {
                                for &value in chunk {
                                    *sum += f64::from(value);
                                }
                            }
                        }
                        if has_rem {
                            let rem = &mut active_sums[full_x - first_nz];
                            for &value in &line[full_x * block_x..cells_x] {
                                *rem += f64::from(value);
                            }
                        }
                    }
                    project_channel_f64(
                        active_sums,
                        energies_act,
                        scales_act,
                        signs,
                        d0s,
                        d1s,
                        d2s,
                        &mut steps_buf[..active_len],
                    );
                }
            }

            // Blocked 8×32 cache-line transpose from `channel_steps` into
            // `row_components` (channels `num_channels..CHANNELS` stay zeroed
            // on 24-channel dual-carrier fields automatically).
            let out_active = &mut row_components[first_nz..last_nz];
            let mut chunks = out_active.chunks_exact_mut(8);
            let mut base = 0_usize;
            for out_block in chunks.by_ref() {
                let mut tile = [[0_i8; CHANNELS]; 8];
                for (channel, src_row) in channel_steps[..num_channels].iter().enumerate() {
                    let src = &src_row[base..base + 8];
                    for (dst, &val) in tile.iter_mut().zip(src) {
                        dst[channel] = val;
                    }
                }
                out_block.copy_from_slice(&tile);
                base += 8;
            }
            let rem = chunks.into_remainder();
            if !rem.is_empty() {
                for (offset, texel) in rem.iter_mut().enumerate() {
                    let mut col = [0_i8; CHANNELS];
                    for (channel, src_row) in channel_steps[..num_channels].iter().enumerate() {
                        col[channel] = src_row[base + offset];
                    }
                    *texel = col;
                }
            }

            for ((((out, &energy), &d0), &d1), &d2) in row_composition[first_nz..last_nz]
                .iter_mut()
                .zip(energies_act)
                .zip(d0s.iter())
                .zip(d1s.iter())
                .zip(d2s.iter())
            {
                *out = coordinates_with_sqrt([d0, d1, d2], sqrt_channels, energy);
            }
        }
    }
}

/// A bounded summary of the source world (B.3), at its source tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    /// The source tick.
    pub tick: Tick,
    /// Live organisms.
    pub live: u64,
    /// Their energy, summed in identity order in F64.
    pub organism_energy: f64,
    /// The field's energy, summed in cell order in F64.
    pub field_energy: f64,
    /// The highest live generation.
    pub max_generation: u32,
    /// Births and deaths published so far.
    pub events: u64,
    /// The live organisms' logical neural parameters, summed (§14.1).
    pub parameters: u64,
    /// The logical bytes the genome quota charges the live genomes.
    pub genome_logical_bytes: u64,
    /// The bytes allocated to the live genomes' parameters.
    pub genome_allocated_bytes: u64,
    /// The run's ledger after the source tick: its cumulative injection,
    /// removal, dissipation and residual since the run was founded.
    pub ledger: LedgerReport,
    /// Whether a numerical fault has stopped the run.
    pub faulted: bool,
    /// What the publisher counted from the committed ticks its owner gave
    /// it ([`crate::Publisher::count`]), through the source tick; `None`
    /// for an epoch outside a feed, or when the publisher was not given
    /// every tick up to the source tick.
    pub tally: Option<Tally>,
}

/// Counts over the committed ticks a publisher was given, from the tick it
/// opened at (plan §14.1): what a tick's report says and the world keeps no
/// running total of. Each is summed in tick order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tally {
    /// The world's tick when the publisher opened. For a founded run that is
    /// 0, and the counts are the run's.
    pub since: Tick,
    /// Births.
    pub births: u64,
    /// Organisms introduced by commands.
    pub introductions: u64,
    /// Deaths.
    pub deaths: u64,
    /// Controller evaluations, regular and forced.
    pub evaluations: u64,
    /// Birth attempts whose every mutation candidate was rejected.
    pub rejected_births: u64,
    /// Birth attempts `refuse-excess-births-v1` refused for capacity.
    pub refused_births: u64,
    /// The energy organisms gained by uptake: the uptake transfer's credit,
    /// what granted uptake left after each organism's efficiency.
    pub uptake: f64,
}

impl Tally {
    /// Nothing counted yet, from `since`.
    #[must_use]
    pub const fn from(since: Tick) -> Self {
        Self {
            since,
            births: 0,
            introductions: 0,
            deaths: 0,
            evaluations: 0,
            rejected_births: 0,
            refused_births: 0,
            uptake: 0.0,
        }
    }

    /// Add one committed tick's report.
    pub fn add(&mut self, report: &TickReport) {
        self.births += report.births.len() as u64;
        self.introductions += report.introductions.len() as u64;
        self.deaths += report.deaths.len() as u64;
        self.evaluations += report.evaluated as u64;
        self.rejected_births += report.rejected.len() as u64;
        self.refused_births += report.refused.len() as u64;
        self.uptake += report.closure.credited(Transfer::Uptake);
    }
}

/// Grid geometry for projecting one row-major scalar overlay plane into a
/// bounded field texture block-mean buffer.
#[derive(Clone, Copy)]
struct ScalarOverlayGrid {
    cells: [usize; 2],
    block: [usize; 2],
    texels: [usize; 2],
    divisor: f64,
}

/// Validate that `plane` has exact cell count `cells[0] * cells[1]` and every
/// cell is finite and nonnegative.
fn validate_scalar_plane(
    plane: &[f32],
    cells: [usize; 2],
    buffer: &'static str,
) -> Result<(), CoreError> {
    let [cells_x, cells_y] = cells;
    let expected = cells_x.checked_mul(cells_y);
    if expected != Some(plane.len()) {
        return Err(CoreError::LayoutMismatch {
            buffer,
            detail: format!(
                "{} values are not one scalar plane of a {} × {} grid",
                plane.len(),
                cells_x,
                cells_y
            ),
        });
    }
    // `value >= 0.0 && value < f32::INFINITY` is equivalent to
    // `value.is_finite() && value >= 0.0` (NaN and ±Inf fail) and vectorizes
    // cleanly across 4 f32 lanes without `enumerate()` tracking on the happy path.
    if plane
        .iter()
        .all(|&value| value >= 0.0 && value < f32::INFINITY)
    {
        return Ok(());
    }
    if let Some((cell, &value)) = plane
        .iter()
        .enumerate()
        .find(|(_, value)| !value.is_finite() || **value < 0.0)
    {
        return Err(CoreError::NumericalInvalid {
            quantity: buffer,
            detail: format!("cell {cell} value {value} is not finite and >= 0"),
        });
    }
    Ok(())
}

/// Validate that `(edge_x, edge_y)` have exact cell count `cells[0] * cells[1]`
/// and every face conductance is finite and nonnegative.
fn validate_vascular_edges(
    edge_x: &[f32],
    edge_y: &[f32],
    cells: [usize; 2],
) -> Result<(), CoreError> {
    let [cells_x, cells_y] = cells;
    let expected = cells_x.checked_mul(cells_y);
    if expected != Some(edge_x.len()) || expected != Some(edge_y.len()) {
        return Err(CoreError::LayoutMismatch {
            buffer: "render field vascular conductance",
            detail: format!(
                "vascular edge buffers ({}, {}) do not match a {} × {} grid",
                edge_x.len(),
                edge_y.len(),
                cells_x,
                cells_y
            ),
        });
    }
    for (axis, edges) in [("east", edge_x), ("north", edge_y)] {
        if edges
            .iter()
            .all(|&value| value >= 0.0 && value < f32::INFINITY)
        {
            continue;
        }
        if let Some((cell, &value)) = edges
            .iter()
            .enumerate()
            .find(|(_, value)| !value.is_finite() || **value < 0.0)
        {
            return Err(CoreError::NumericalInvalid {
                quantity: "render field vascular conductance",
                detail: format!("{axis} edge at cell {cell} value {value} is not finite and >= 0"),
            });
        }
    }
    Ok(())
}

/// Project one validated row-major scalar `plane` into `out` in place (zero
/// allocation once reserved), summing each block in F64 rows-then-columns on at
/// most `workers` threads, bit-identical to `workers = 1`.
fn project_validated_scalar_plane_on(
    plane: &[f32],
    grid: ScalarOverlayGrid,
    workers: usize,
    out: &mut Vec<f32>,
) -> Result<(), CoreError> {
    let [cells_x, cells_y] = grid.cells;
    let [width, height] = grid.texels;
    let count = width * height;
    resize_len(out, count, 0.0);
    let used = workers
        .min(height.div_ceil(MINIMUM_TEXEL_ROWS_PER_WORKER))
        .max(1);
    let [block_x, block_y] = grid.block;
    let whole = block_x == 1 && block_y == 1;
    let inv_divisor = exact_power_of_two_recip(grid.divisor);
    let project_run = |first_row: usize, slice: &mut [f32]| {
        if whole {
            if let Some(inv) = inv_divisor {
                for (offset, row_values) in slice.chunks_exact_mut(width).enumerate() {
                    let row = &plane[(first_row + offset) * cells_x..][..width];
                    for (out_value, &value) in row_values.iter_mut().zip(row) {
                        *out_value = saturate_nonnegative(f64::from(value) * inv);
                    }
                }
            } else {
                let divisor = grid.divisor;
                for (offset, row_values) in slice.chunks_exact_mut(width).enumerate() {
                    let row = &plane[(first_row + offset) * cells_x..][..width];
                    for (out_value, &value) in row_values.iter_mut().zip(row) {
                        *out_value = saturate_nonnegative(f64::from(value) / divisor);
                    }
                }
            }
            return;
        }
        for (offset, row_values) in slice.chunks_exact_mut(width).enumerate() {
            let texel_y = first_row + offset;
            let rows = texel_y * block_y..((texel_y + 1) * block_y).min(cells_y);
            for (texel_x, out_value) in row_values.iter_mut().enumerate() {
                let cols = texel_x * block_x..((texel_x + 1) * block_x).min(cells_x);
                let mut sum = 0.0_f64;
                for y in rows.clone() {
                    for &value in &plane[y * cells_x + cols.start..y * cells_x + cols.end] {
                        sum += f64::from(value);
                    }
                }
                let denom = (rows.len() * cols.len()) as f64 * grid.divisor;
                *out_value = saturate_nonnegative(sum / denom);
            }
        }
    };
    if used == 1 {
        project_run(0, out);
        return Ok(());
    }
    let per = height.div_ceil(used).max(1);
    let chunk = per * width;
    let mut runs = out.chunks_mut(chunk);
    let Some(first_run) = runs.next() else {
        return Ok(());
    };
    let project_run = &project_run;
    flife_core::pool::scope_workers(workers, "project render field overlay", |scope| {
        for (index, run) in runs.enumerate() {
            scope.spawn(move || project_run((index + 1) * per, run));
        }
        project_run(0, first_run);
    })
}

/// Project validated periodic `(edge_x, edge_y)` vascular face conductances
/// into `out` in place (zero allocation once reserved): each cell `(x, y)`
/// takes the mean of its four incident periodic faces `(east, west, north,
/// south)` in F64, averaged over each texel's block rows-then-columns,
/// bit-identical across worker counts.
fn project_validated_vascular_edges_on(
    edge_x: &[f32],
    edge_y: &[f32],
    grid: ScalarOverlayGrid,
    workers: usize,
    out: &mut Vec<f32>,
) -> Result<(), CoreError> {
    let [cells_x, cells_y] = grid.cells;
    let [width, height] = grid.texels;
    let count = width * height;
    resize_len(out, count, 0.0);
    let used = workers
        .min(height.div_ceil(MINIMUM_TEXEL_ROWS_PER_WORKER))
        .max(1);
    let [block_x, block_y] = grid.block;
    let whole = block_x == 1 && block_y == 1;
    let project_run = |first_row: usize, slice: &mut [f32]| {
        if whole {
            for (offset, row_values) in slice.chunks_exact_mut(width).enumerate() {
                let y = first_row + offset;
                let y_prev = if y == 0 { cells_y - 1 } else { y - 1 };
                let row_cur = y * cells_x;
                let row_prev = y_prev * cells_x;
                // Column 0 wraps `x_prev = cells_x - 1`; columns `1..width`
                // zip four contiguous slices cleanly with zero per-cell branch.
                let ex_row = &edge_x[row_cur..row_cur + width];
                let ey_cur = &edge_y[row_cur..row_cur + width];
                let ey_prev = &edge_y[row_prev..row_prev + width];
                row_values[0] = saturate_nonnegative(
                    0.25 * (f64::from(ex_row[0])
                        + f64::from(ex_row[width - 1])
                        + f64::from(ey_cur[0])
                        + f64::from(ey_prev[0])),
                );
                for ((((out_value, &east), &west), &north), &south) in row_values[1..]
                    .iter_mut()
                    .zip(&ex_row[1..])
                    .zip(&ex_row[..width - 1])
                    .zip(&ey_cur[1..])
                    .zip(&ey_prev[1..])
                {
                    *out_value = saturate_nonnegative(
                        0.25 * (f64::from(east)
                            + f64::from(west)
                            + f64::from(north)
                            + f64::from(south)),
                    );
                }
            }
            return;
        }
        for (offset, row_values) in slice.chunks_exact_mut(width).enumerate() {
            let texel_y = first_row + offset;
            let rows = texel_y * block_y..((texel_y + 1) * block_y).min(cells_y);
            for (texel_x, out_value) in row_values.iter_mut().enumerate() {
                let cols = texel_x * block_x..((texel_x + 1) * block_x).min(cells_x);
                let mut sum = 0.0_f64;
                for y in rows.clone() {
                    let y_prev = if y == 0 { cells_y - 1 } else { y - 1 };
                    let row_cur = y * cells_x;
                    let row_prev = y_prev * cells_x;
                    for x in cols.clone() {
                        let x_prev = if x == 0 { cells_x - 1 } else { x - 1 };
                        let incident = 0.25
                            * (f64::from(edge_x[row_cur + x])
                                + f64::from(edge_x[row_cur + x_prev])
                                + f64::from(edge_y[row_cur + x])
                                + f64::from(edge_y[row_prev + x]));
                        sum += incident;
                    }
                }
                let denom = (rows.len() * cols.len()) as f64;
                *out_value = saturate_nonnegative(sum / denom);
            }
        }
    };
    if used == 1 {
        project_run(0, out);
        return Ok(());
    }
    let per = height.div_ceil(used).max(1);
    let chunk = per * width;
    let mut runs = out.chunks_mut(chunk);
    let Some(first_run) = runs.next() else {
        return Ok(());
    };
    let project_run = &project_run;
    flife_core::pool::scope_workers(workers, "project render field vascular overlay", |scope| {
        for (index, run) in runs.enumerate() {
            scope.spawn(move || project_run((index + 1) * per, run));
        }
        project_run(0, first_run);
    })
}

/// Which optional `V2-16` views a world activates. All four are `false` on
/// `field-ecology-v1*` and whenever `v2-visual-mappings` is disabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct V2Views {
    cell_states: bool,
    polarities: bool,
    trail_energy: bool,
    vascular_conductance: bool,
}

impl V2Views {
    fn for_world(world: &World) -> Self {
        let config = world.config();
        if !config.is_v2_feature_enabled(V2Feature::V2VisualMappings) {
            return Self {
                cell_states: false,
                polarities: false,
                trail_energy: false,
                vascular_conductance: false,
            };
        }
        Self {
            cell_states: config.is_v2_feature_enabled(V2Feature::EpigeneticRecurrence)
                || world.recurrent_state_width() > 0,
            polarities: config.is_v2_feature_enabled(V2Feature::PcpPolarity),
            trail_energy: config.is_v2_feature_enabled(V2Feature::DualCarrierTrails),
            vascular_conductance: config.is_v2_feature_enabled(V2Feature::VascularField),
        }
    }
}

/// An immutable compact render epoch: exactly one committed world (B.3).
///
/// A consumer only ever sees an epoch through a lease taken after its
/// publication, so it never reads a partly written one.
#[derive(Debug, Clone, PartialEq)]
pub struct Epoch {
    id: RenderEpoch,
    tick: Tick,
    resource: ResourceEpoch,
    prior: Option<Prior>,
    step: TimeStep,
    maximum_speed: f64,
    attempt_energy: f64,
    affinity_limit: f64,
    extent: [f64; 2],
    capacity: usize,
    ids: Vec<u64>,
    positions: Vec<[f32; 2]>,
    velocities: Vec<[f32; 2]>,
    energies: Vec<f32>,
    lineages: Vec<u64>,
    generations: Vec<u32>,
    topologies: Vec<u8>,
    affinities: Vec<[f32; 3]>,
    cell_states: Option<Vec<i8>>,
    polarities: Option<Vec<i16>>,
    field: FieldTexture,
    metrics: Metrics,
    followed: Option<Followed>,
}

/// A vector with exactly `count` elements of capacity, or a reported
/// allocation failure (INV-15).
pub(crate) fn reserved<T>(count: usize, buffer: &'static str) -> Result<Vec<T>, CoreError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| CoreError::AllocationFailed {
            buffer,
            bytes: u64::try_from(count.saturating_mul(size_of::<T>())).unwrap_or(u64::MAX),
        })?;
    Ok(values)
}

/// Nonnegative `value` rounded to the nearest F32 in `[0.0, f32::MAX]`,
/// saturating positive overflow (including `+∞`) at `f32::MAX` and mapping
/// `NaN`, `-∞`, negative values, and `-0.0` to canonical `+0.0`.
fn saturate_nonnegative(value: f64) -> f32 {
    if value.is_nan() || value <= 0.0 {
        0.0
    } else {
        (value as f32).min(f32::MAX)
    }
}

/// A coordinate in `[0, length)` rounded to F32 and kept half-open: one
/// that rounds up to `length` is the seam, which is `0`.
fn fold(coordinate: f64, length: f32) -> f32 {
    let rounded = coordinate as f32;
    if rounded >= length { 0.0 } else { rounded }
}

impl Epoch {
    /// `world`'s committed state as an epoch outside any feed: what one of
    /// a publisher's slots holds, so a host can size its observers before
    /// its feed exists (FL-043). Its identity is 1 and it has no prior; it
    /// is never published.
    ///
    /// # Errors
    /// As a slot's allocation and projection.
    pub fn standalone(world: &World) -> Result<Self, CoreError> {
        let mut epoch = Self::allocate(world)?;
        epoch.project(world, RenderEpoch::new(1), None, None)?;
        Ok(epoch)
    }

    /// A slot for `world`'s epochs: every buffer allocated once, at its
    /// population ceiling and texture size. It holds `world`'s founding
    /// metadata until its first projection.
    pub(crate) fn allocate(world: &World) -> Result<Self, CoreError> {
        let capacity = world.population().capacity();
        let grid = world.field().grid();
        let v2 = V2Views::for_world(world);
        Ok(Self {
            id: RenderEpoch::new(0),
            tick: world.tick(),
            resource: world.epoch(),
            prior: None,
            step: world.step_size(),
            maximum_speed: world.maximum_speed(),
            attempt_energy: world.minimum_attempt_energy(),
            affinity_limit: world.trait_limits().affinity_norm(),
            extent: [grid.world_width(), grid.world_height()],
            capacity,
            ids: reserved(capacity, "render identity view")?,
            positions: reserved(capacity, "render position view")?,
            velocities: reserved(capacity, "render velocity view")?,
            energies: reserved(capacity, "render energy view")?,
            lineages: reserved(capacity, "render lineage view")?,
            generations: reserved(capacity, "render generation view")?,
            topologies: reserved(capacity, "render topology view")?,
            affinities: reserved(capacity, "render affinity view")?,
            cell_states: v2
                .cell_states
                .then(|| reserved(capacity, "render cell-state view"))
                .transpose()?,
            polarities: v2
                .polarities
                .then(|| reserved(capacity, "render polarity view"))
                .transpose()?,
            field: FieldTexture::sized_with_v2(
                [grid.cells_x(), grid.cells_y()],
                [grid.cell_width(), grid.cell_height()],
                v2.trail_energy,
                v2.vascular_conductance,
            )?,
            metrics: Metrics {
                tick: world.tick(),
                live: 0,
                organism_energy: 0.0,
                field_energy: 0.0,
                max_generation: 0,
                events: 0,
                parameters: 0,
                genome_logical_bytes: 0,
                genome_allocated_bytes: 0,
                ledger: world.ledger().report(),
                faulted: false,
                tally: None,
            },
            followed: world.followed(),
        })
    }

    /// Refill this slot from `world`'s committed state as epoch `id`, with
    /// the publisher's `tally` through its tick. On an error the slot holds
    /// nothing publishable; the caller does not publish it.
    pub(crate) fn project(
        &mut self,
        world: &World,
        id: RenderEpoch,
        prior: Option<Prior>,
        tally: Option<Tally>,
    ) -> Result<(), CoreError> {
        let population = world.population();
        let live = population.len();
        if live > self.capacity {
            return Err(CoreError::BudgetExhausted {
                budget: "render epoch rows",
                required: live as u64,
                available: self.capacity as u64,
            });
        }
        let grid = world.field().grid();
        let extent = [grid.world_width(), grid.world_height()];
        if extent != self.extent || [grid.cells_x(), grid.cells_y()] != self.field.cells {
            return Err(CoreError::LayoutMismatch {
                buffer: "render epoch",
                detail: format!(
                    "a slot allocated for a {:?} world of {:?} cells offered a {extent:?} world",
                    self.extent, self.field.cells
                ),
            });
        }
        let v2 = V2Views::for_world(world);
        let slot_v2 = V2Views {
            cell_states: self.cell_states.is_some(),
            polarities: self.polarities.is_some(),
            trail_energy: self.field.trail_energy.is_some(),
            vascular_conductance: self.field.vascular_conductance.is_some(),
        };
        if v2 != slot_v2 {
            return Err(CoreError::LayoutMismatch {
                buffer: "render epoch",
                detail: format!(
                    "a slot allocated for optional views {slot_v2:?} was offered a world with {v2:?}"
                ),
            });
        }
        let width = extent.map(|length| length as f32);
        // Steady-state reused slots keep `len == live` without zero-clearing
        // already-initialized buffers, and writing into exact-length slices
        // eliminates 8–10 `Vec::push` grow checks + len updates per organism.
        resize_len(&mut self.ids, live, 0);
        resize_len(&mut self.positions, live, [0.0; 2]);
        resize_len(&mut self.velocities, live, [0.0; 2]);
        resize_len(&mut self.energies, live, 0.0);
        resize_len(&mut self.lineages, live, 0);
        resize_len(&mut self.generations, live, 0);
        resize_len(&mut self.topologies, live, 0);
        resize_len(&mut self.affinities, live, [0.0; 3]);
        if let Some(states) = &mut self.cell_states {
            resize_len(states, live, 0);
        }
        if let Some(polarities) = &mut self.polarities {
            resize_len(polarities, live, 0);
        }
        let affinity_limit = world.trait_limits().affinity_norm();
        let affinity_scale = (CHANNELS as f64).sqrt() * affinity_limit;
        let v2_rules = world.v2_rules();
        let affinity_rotation = v2_rules.affinity_rotation_enabled();
        let mut organism_energy = 0.0_f64;
        let mut max_generation = 0_u32;
        let mut parameters = 0_u64;
        let mut cell_states_it = self.cell_states.as_deref_mut().map(|s| s.iter_mut());
        let mut polarities_it = self.polarities.as_deref_mut().map(|s| s.iter_mut());
        let mut written = 0_usize;
        let dst = self
            .ids
            .iter_mut()
            .zip(self.positions.iter_mut())
            .zip(self.velocities.iter_mut())
            .zip(self.energies.iter_mut())
            .zip(self.lineages.iter_mut())
            .zip(self.generations.iter_mut())
            .zip(self.topologies.iter_mut())
            .zip(self.affinities.iter_mut());
        for (
            (
                ((((((id_out, pos_out), vel_out), energy_out), lineage_out), gen_out), topo_out),
                aff_out,
            ),
            (organism, genome),
        ) in dst.zip(population.iter_with_genomes())
        {
            let genome = genome.ok_or_else(|| CoreError::StateConflict {
                operation: "project render epoch",
                detail: format!("{} has no genome", organism.id),
            })?;
            let topology =
                u8::try_from(genome.topology().index()).map_err(|_| CoreError::Overflow {
                    quantity: "render topology index",
                })?;
            *id_out = organism.id.get();
            *pos_out = [
                fold(organism.position[0], width[0]),
                fold(organism.position[1], width[1]),
            ];
            // Every committed component is within the speed limit, and
            // rounding to nearest is monotone, so each F32 component is
            // within the F32 limit.
            *vel_out = [organism.velocity[0] as f32, organism.velocity[1] as f32];
            *energy_out = saturate_nonnegative(organism.energy);
            *lineage_out = organism.lineage.get();
            *gen_out = organism.generation.get();
            *topo_out = topology;
            // Live primary recurrent cell state `h_0` (matches `World::cell_states()`
            // without allocating a temporary Vec per published epoch).
            let h0 = organism
                .held
                .recurrent_state()
                .first()
                .copied()
                .unwrap_or(0.0);
            // Project live V2-12 effective metabolic affinity `a_eff(h_0)` when
            // epigenetic affinity rotation is active (matches
            // `World::effective_affinity(id)` with zero heap allocation).
            if affinity_rotation && h0 != 0.0 {
                let effective = flife_engine::effective_affinity(
                    genome.traits(),
                    h0,
                    v2_rules,
                    affinity_limit,
                )?;
                *aff_out = affinity_projection_32(&effective, affinity_scale);
            } else {
                *aff_out = affinity_projection_32(genome.traits().affinity(), affinity_scale);
            }
            if let Some(slot) = cell_states_it.as_mut().and_then(Iterator::next) {
                *slot = quantize_cell_state(h0);
            }
            if let Some(slot) = polarities_it.as_mut().and_then(Iterator::next) {
                *slot = quantize_polarity_f64(organism.polarity);
            }
            organism_energy += organism.energy;
            max_generation = max_generation.max(organism.generation.get());
            parameters += genome.parameters().len() as u64;
            written += 1;
        }
        if written != live {
            self.ids.truncate(written);
            self.positions.truncate(written);
            self.velocities.truncate(written);
            self.energies.truncate(written);
            self.lineages.truncate(written);
            self.generations.truncate(written);
            self.topologies.truncate(written);
            self.affinities.truncate(written);
            if let Some(states) = &mut self.cell_states {
                states.truncate(written);
            }
            if let Some(polarities) = &mut self.polarities {
                polarities.truncate(written);
            }
        }
        // Between ticks the world's workers are idle: the texture's rows
        // are projected on them.
        self.field.project_on(
            world.field().as_slice(),
            grid.cell_area(),
            world.cpu_workers(),
        )?;
        // Project live V2-16 overlay planes (Carrier-1 trail energy density and
        // incident vascular edge conductance) when reserved by this slot.
        let trail_plane = self
            .field
            .trail_energy
            .is_some()
            .then(|| world.field().trail_energy())
            .flatten();
        let vascular_edges = self
            .field
            .vascular_conductance
            .is_some()
            .then(|| world.field().vascular_edges())
            .flatten();
        self.field.project_overlays_from_world_on(
            trail_plane,
            vascular_edges,
            grid.cell_area(),
            world.cpu_workers(),
        )?;
        self.id = id;
        self.tick = world.tick();
        self.resource = world.epoch();
        self.prior = prior;
        self.step = world.step_size();
        self.maximum_speed = world.maximum_speed();
        self.attempt_energy = world.minimum_attempt_energy();
        self.affinity_limit = affinity_limit;
        self.metrics = Metrics {
            tick: world.tick(),
            live: live as u64,
            organism_energy,
            field_energy: world.field().energy_total(),
            max_generation,
            events: world.events(),
            parameters,
            genome_logical_bytes: world.quota().logical_bytes(),
            genome_allocated_bytes: world.quota().allocated_bytes(),
            ledger: world.ledger().report(),
            faulted: world.fault().is_some(),
            tally,
        };
        self.followed = world.followed();
        Ok(())
    }

    /// The epoch's identity: issued once, strictly increasing.
    #[must_use]
    pub const fn id(&self) -> RenderEpoch {
        self.id
    }

    /// The committed tick it shows.
    #[must_use]
    pub const fn tick(&self) -> Tick {
        self.tick
    }

    /// The committed resource epoch it shows.
    #[must_use]
    pub const fn resource(&self) -> ResourceEpoch {
        self.resource
    }

    /// The epoch it replaced as the latest displayable one.
    #[must_use]
    pub const fn prior(&self) -> Option<Prior> {
        self.prior
    }

    /// The world's timestep.
    #[must_use]
    pub const fn step(&self) -> TimeStep {
        self.step
    }

    /// The world's speed limit.
    #[must_use]
    pub const fn maximum_speed(&self) -> f64 {
        self.maximum_speed
    }

    /// The least energy an organism of the world attempts a birth with
    /// (`reproduction.minimum_parent_energy_before_attempt`): the
    /// lineage-energy mapping's scale.
    #[must_use]
    pub const fn attempt_energy(&self) -> f64 {
        self.attempt_energy
    }

    /// The world's size, `[width, height]` in world units.
    #[must_use]
    pub const fn extent(&self) -> [f64; 2] {
        self.extent
    }

    /// The rows the slot was allocated for: the population ceiling.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Live organisms.
    #[must_use]
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    /// Whether no organism is alive.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// Stable identities, strictly increasing.
    #[must_use]
    pub fn ids(&self) -> &[u64] {
        &self.ids
    }

    /// Row `row`'s identity.
    #[must_use]
    pub fn id_at(&self, row: usize) -> Option<OrganismId> {
        self.ids.get(row).map(|&raw| OrganismId::new(raw))
    }

    /// The row of `id`, found by identity, never by slot: for selection
    /// and temporal matching.
    #[must_use]
    pub fn row(&self, id: OrganismId) -> Option<usize> {
        self.ids.binary_search(&id.get()).ok()
    }

    /// Wrapped positions, each component in `[0, extent)` after rounding
    /// to F32.
    #[must_use]
    pub fn positions(&self) -> &[[f32; 2]] {
        &self.positions
    }

    /// Velocities, each component within the F32 speed limit.
    #[must_use]
    pub fn velocities(&self) -> &[[f32; 2]] {
        &self.velocities
    }

    /// Energies, rounded to F32.
    #[must_use]
    pub fn energies(&self) -> &[f32] {
        &self.energies
    }

    /// Founder lineages.
    #[must_use]
    pub fn lineages(&self) -> &[u64] {
        &self.lineages
    }

    /// Generations.
    #[must_use]
    pub fn generations(&self) -> &[u32] {
        &self.generations
    }

    /// Topology catalog indices.
    #[must_use]
    pub fn topologies(&self) -> &[u8] {
        &self.topologies
    }

    /// Affinity directions' display coordinates ([`affinity_projection`]),
    /// each in `[−1, 1]` and together within the unit ball up to rounding.
    #[must_use]
    pub fn affinities(&self) -> &[[f32; 3]] {
        &self.affinities
    }

    /// Optional `V2-16` primary recurrent cell states `h_0` quantized onto
    /// `[−127, 127]` (`1 B/org`), in stable identity order (`C.7`). `Some`
    /// only when `v2-visual-mappings` and recurrence are enabled (`None` and
    /// `0` bytes reserved on v1 or when disabled).
    #[must_use]
    pub fn cell_states(&self) -> Option<&[i8]> {
        self.cell_states.as_deref()
    }

    /// The cell-state view's declared layout, when present.
    #[must_use]
    pub fn cell_states_layout(&self) -> Option<Layout> {
        self.cell_states.as_ref().map(|values| Layout {
            view: View::CellState,
            dtype: Dtype::I8,
            components: 1,
            count: values.len(),
            bounds: Bounds::Magnitude(f32::from(CELL_STATE_STEPS)),
        })
    }

    /// Optional `V2-16` apical-basal polarity angles quantized onto
    /// `[−32767, +32767]` with [`POLARITY_UNPOLARIZED`] sentinel (`2 B/org`),
    /// in stable identity order (`C.7`). `Some` only when `v2-visual-mappings`
    /// and `pcp-polarity` are enabled (`None` and `0` bytes reserved on v1 or
    /// when disabled).
    #[must_use]
    pub fn polarities(&self) -> Option<&[i16]> {
        self.polarities.as_deref()
    }

    /// The polarity view's declared layout, when present.
    #[must_use]
    pub fn polarities_layout(&self) -> Option<Layout> {
        self.polarities.as_ref().map(|values| Layout {
            view: View::Polarity,
            dtype: Dtype::I16,
            components: 1,
            count: values.len(),
            bounds: Bounds::Any,
        })
    }

    /// The norm limit the affinity coordinates are scaled by:
    /// `chemistry.affinity_norm_limit`.
    #[must_use]
    pub const fn affinity_limit(&self) -> f64 {
        self.affinity_limit
    }

    /// The field texture.
    #[must_use]
    pub const fn field(&self) -> &FieldTexture {
        &self.field
    }

    /// The metric summary.
    #[must_use]
    pub const fn metrics(&self) -> &Metrics {
        &self.metrics
    }

    /// The line of descent the world follows as of this tick
    /// ([`World::follow`]; plan §2.2): a host following a selected
    /// organism's descendants sees here, at the tick it happened, that the
    /// line has passed to an heir, which a live line's organism names among
    /// this epoch's rows.
    #[must_use]
    pub const fn followed(&self) -> Option<Followed> {
        self.followed
    }

    /// Every view's declared layout, in [`View::ALL`] order.
    #[must_use]
    pub fn layouts(&self) -> [Layout; 11] {
        let rows = |view, dtype, components, bounds| Layout {
            view,
            dtype,
            components,
            count: self.ids.len(),
            bounds,
        };
        let extent = self.extent.map(|length| length as f32);
        [
            rows(View::Identity, Dtype::U64, 1, Bounds::StrictlyIncreasing),
            rows(View::Position, Dtype::F32, 2, Bounds::Wrapped(extent)),
            rows(
                View::Velocity,
                Dtype::F32,
                2,
                Bounds::Magnitude(self.maximum_speed as f32),
            ),
            rows(View::Energy, Dtype::F32, 1, Bounds::NonNegative),
            rows(View::Lineage, Dtype::U64, 1, Bounds::Any),
            rows(View::Generation, Dtype::U32, 1, Bounds::Any),
            rows(View::Topology, Dtype::U8, 1, Bounds::Any),
            rows(View::Affinity, Dtype::F32, 3, Bounds::Magnitude(1.0)),
            self.field.layout(),
            self.field.composition_layout(),
            self.field.components_layout(),
        ]
    }

    /// Every optional `V2-16` view's declared layout that is present in this
    /// epoch, in [`View::V2_OPTIONAL`] order. Empty on `field-ecology-v1*` or
    /// when `v2-visual-mappings` is disabled.
    #[must_use]
    pub fn optional_layouts(&self) -> Vec<Layout> {
        let mut layouts = Vec::new();
        if let Some(layout) = self.cell_states_layout() {
            layouts.push(layout);
        }
        if let Some(layout) = self.polarities_layout() {
            layouts.push(layout);
        }
        if let Some(layout) = self.field.trail_energy_layout() {
            layouts.push(layout);
        }
        if let Some(layout) = self.field.vascular_conductance_layout() {
            layouts.push(layout);
        }
        layouts
    }

    /// Every baseline and present optional view's declared layout, in
    /// [`View::ALL`] followed by [`View::V2_OPTIONAL`] order.
    #[must_use]
    pub fn all_layouts(&self) -> Vec<Layout> {
        let optional = self.optional_layouts();
        let mut layouts = Vec::with_capacity(View::ALL.len() + optional.len());
        layouts.extend(self.layouts());
        layouts.extend(optional);
        layouts
    }

    /// The bytes this slot's buffers have actually reserved (`INV-15`).
    /// Allocated once, it is the population ceiling times 53 bytes a row plus
    /// 48 a texel on v1 (and when `v2-visual-mappings` is disabled), plus any
    /// enabled optional `V2-16` views (`1 B/org` cell-state, `2 B/org`
    /// polarity, `4 B/texel` trail-energy, `4 B/texel` vascular-conductance),
    /// and a projection never changes it.
    #[must_use]
    pub fn reserved_bytes(&self) -> u64 {
        fn bytes<T>(values: &Vec<T>) -> u64 {
            (values.capacity() as u64).saturating_mul(size_of::<T>() as u64)
        }
        fn opt_bytes<T>(values: &Option<Vec<T>>) -> u64 {
            values.as_ref().map_or(0, bytes)
        }
        [
            bytes(&self.ids),
            bytes(&self.positions),
            bytes(&self.velocities),
            bytes(&self.energies),
            bytes(&self.lineages),
            bytes(&self.generations),
            bytes(&self.topologies),
            bytes(&self.affinities),
            opt_bytes(&self.cell_states),
            opt_bytes(&self.polarities),
            bytes(&self.field.values),
            bytes(&self.field.composition),
            bytes(&self.field.components),
            opt_bytes(&self.field.trail_energy),
            opt_bytes(&self.field.vascular_conductance),
        ]
        .into_iter()
        .fold(0, u64::saturating_add)
    }

    /// Check every view against its declared layout: counts, order and
    /// bounds. A consumer may call this; the publisher's projection
    /// satisfies it by construction.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] naming the first violation.
    pub fn check(&self) -> Result<(), CoreError> {
        let refuse = |view: View, detail: String| {
            Err(CoreError::LayoutMismatch {
                buffer: "render epoch",
                detail: format!("{} view: {detail}", view.name()),
            })
        };
        let count = self.ids.len();
        let counts = [
            (View::Position, self.positions.len()),
            (View::Velocity, self.velocities.len()),
            (View::Energy, self.energies.len()),
            (View::Lineage, self.lineages.len()),
            (View::Generation, self.generations.len()),
            (View::Topology, self.topologies.len()),
            (View::Affinity, self.affinities.len()),
        ];
        for (view, length) in counts {
            if length != count {
                return refuse(view, format!("{length} rows for {count} identities"));
            }
        }
        if count > self.capacity {
            return refuse(
                View::Identity,
                format!("{count} rows exceed the capacity {}", self.capacity),
            );
        }
        if let Some(pair) = self.ids.windows(2).find(|pair| pair[0] >= pair[1]) {
            return refuse(View::Identity, format!("{} before {}", pair[0], pair[1]));
        }
        if let Some(line) = self.followed
            && line.alive
            && self.row(line.organism).is_none()
        {
            return refuse(
                View::Identity,
                format!("the followed line's {} has no row", line.organism),
            );
        }
        let texels = self.field.texels.map(|n| n as usize);
        if self.field.values.len() != texels[0] * texels[1] {
            return refuse(
                View::Field,
                format!("{} texels for {texels:?}", self.field.values.len()),
            );
        }
        if self.field.composition.len() != texels[0] * texels[1] {
            return refuse(
                View::FieldComposition,
                format!("{} texels for {texels:?}", self.field.composition.len()),
            );
        }
        if self.field.components.len() != texels[0] * texels[1] {
            return refuse(
                View::FieldComponents,
                format!("{} texels for {texels:?}", self.field.components.len()),
            );
        }
        if let Some(states) = &self.cell_states {
            if states.len() != count {
                return refuse(
                    View::CellState,
                    format!("{} rows for {count} identities", states.len()),
                );
            }
            if let Some(&value) = states
                .iter()
                .find(|&&v| i16::from(v).abs() > i16::from(CELL_STATE_STEPS))
            {
                return refuse(
                    View::CellState,
                    format!("{value} exceeds {CELL_STATE_STEPS}"),
                );
            }
        }
        if let Some(polarities) = &self.polarities
            && polarities.len() != count
        {
            return refuse(
                View::Polarity,
                format!("{} rows for {count} identities", polarities.len()),
            );
        }
        if let Some(trail) = &self.field.trail_energy {
            if trail.len() != texels[0] * texels[1] {
                return refuse(
                    View::FieldTrailEnergy,
                    format!("{} texels for {texels:?}", trail.len()),
                );
            }
            if let Some(value) = trail.iter().find(|v| !v.is_finite() || **v < 0.0) {
                return refuse(
                    View::FieldTrailEnergy,
                    format!("{value} is not finite and >= 0"),
                );
            }
        }
        if let Some(vascular) = &self.field.vascular_conductance {
            if vascular.len() != texels[0] * texels[1] {
                return refuse(
                    View::FieldVascularConductance,
                    format!("{} texels for {texels:?}", vascular.len()),
                );
            }
            if let Some(value) = vascular.iter().find(|v| !v.is_finite() || **v < 0.0) {
                return refuse(
                    View::FieldVascularConductance,
                    format!("{value} is not finite and >= 0"),
                );
            }
        }
        for layout in self.layouts() {
            match layout.bounds {
                Bounds::Wrapped(extent) => {
                    if let Some(p) = self.positions.iter().find(|p| {
                        !(0.0..extent[0]).contains(&p[0]) || !(0.0..extent[1]).contains(&p[1])
                    }) {
                        return refuse(layout.view, format!("{p:?} is outside {extent:?}"));
                    }
                }
                Bounds::Magnitude(limit) => {
                    let exceeds = |c: &f32| c.is_nan() || c.abs() > limit;
                    let found = if layout.view == View::FieldComponents {
                        self.field
                            .components
                            .iter()
                            .find(|q| q.iter().any(|&c| f32::from(c).abs() > limit))
                            .map(|q| format!("{q:?}"))
                    } else if layout.view == View::Affinity {
                        self.affinities
                            .iter()
                            .find(|a| a.iter().any(exceeds))
                            .map(|a| format!("{a:?}"))
                    } else if layout.view == View::FieldComposition {
                        self.field
                            .composition
                            .iter()
                            .find(|q| q.iter().any(exceeds))
                            .map(|q| format!("{q:?}"))
                    } else {
                        self.velocities
                            .iter()
                            .find(|v| v.iter().any(exceeds))
                            .map(|v| format!("{v:?}"))
                    };
                    if let Some(row) = found {
                        return refuse(layout.view, format!("{row} exceeds {limit}"));
                    }
                }
                Bounds::NonNegative => {
                    let values = if layout.view == View::Field {
                        &self.field.values
                    } else {
                        &self.energies
                    };
                    if let Some(value) = values.iter().find(|v| !v.is_finite() || **v < 0.0) {
                        return refuse(layout.view, format!("{value} is not finite and >= 0"));
                    }
                }
                Bounds::StrictlyIncreasing | Bounds::Any => {}
            }
        }
        if self.metrics.tick != self.tick || self.metrics.live != count as u64 {
            return refuse(
                View::Identity,
                format!(
                    "metrics for tick {} with {} live describe tick {} with {count}",
                    self.metrics.tick.get(),
                    self.metrics.live,
                    self.tick.get()
                ),
            );
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "unit tests: an unexpected error is a test failure"
)]
mod tests {
    use super::*;
    use flife_core::ValidatedConfig;
    use flife_engine::{Arrangement, NumericalProfile};

    const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

    fn v2_all_world() -> World {
        let mut toml = PRESET.replace(
            "model = \"field-ecology-v1\"",
            "model = \"field-ecology-v2\"",
        );
        toml.push_str(
            "\n[contact]\ndiameter = 0.25\nstiffness = 0.5\nadhesion = 0.25\nadhesion_range = 2.0\n",
        );
        let overrides = [flife_core::FeatureOverride::Enable("all-v2".to_owned())];
        let config = ValidatedConfig::from_toml_with_overrides(&toml, &overrides)
            .expect("valid v2 all-features config");
        World::found(
            &config,
            Arrangement::FreeList,
            NumericalProfile::reference_f32().expect("profile"),
        )
        .expect("founds")
    }

    #[test]
    fn check_refuses_corrupted_optional_v2_views() {
        let world = v2_all_world();
        let clean = Epoch::standalone(&world).expect("standalone epoch");
        clean.check().expect("clean v2 epoch passes check");
        assert!(clean.cell_states().is_some());
        assert!(clean.polarities().is_some());
        assert!(clean.field().trail_energy().is_some());
        assert!(clean.field().vascular_conductance().is_some());

        // Planted negative 1: cell_states = -128 exceeds CELL_STATE_STEPS (127).
        let mut bad = clean.clone();
        bad.cell_states.as_mut().expect("cell_states")[0] = i8::MIN;
        assert!(matches!(bad.check(), Err(CoreError::LayoutMismatch { .. })));

        // Planted negative 2: cell_states row-count mismatch.
        let mut bad = clean.clone();
        bad.cell_states.as_mut().expect("cell_states").pop();
        assert!(matches!(bad.check(), Err(CoreError::LayoutMismatch { .. })));

        // Planted negative 3: polarities row-count mismatch.
        let mut bad = clean.clone();
        bad.polarities.as_mut().expect("polarities").pop();
        assert!(matches!(bad.check(), Err(CoreError::LayoutMismatch { .. })));

        // Planted negative 4: negative trail_energy texel.
        let mut bad = clean.clone();
        bad.field.trail_energy.as_mut().expect("trail")[0] = -0.01;
        assert!(matches!(bad.check(), Err(CoreError::LayoutMismatch { .. })));

        // Planted negative 5: NaN vascular_conductance texel.
        let mut bad = clean.clone();
        bad.field.vascular_conductance.as_mut().expect("vascular")[0] = f32::NAN;
        assert!(matches!(bad.check(), Err(CoreError::LayoutMismatch { .. })));

        // Planted negative 6: trail_energy texel-count mismatch.
        let mut bad = clean;
        bad.field.trail_energy.as_mut().expect("trail").pop();
        assert!(matches!(bad.check(), Err(CoreError::LayoutMismatch { .. })));
    }

    #[test]
    fn dual_carrier_and_vascular_edge_projection_edge_cases() {
        // 1. 1x1 grid with zero Carrier-0 energy, non-zero Carrier-1 trail, and
        //    periodic self-wrapping vascular edges.
        let mut one = vec![0.0_f32; DUAL_CARRIER_PLANES];
        one[25] = 2.5; // Carrier-1 trail energy e_1
        one[26] = 1.0; // Carrier-1 trail moment (must not bleed into Carrier-0 components)
        let mut tex_1x1 =
            FieldTexture::sized_with_v2([1, 1], [0.5, 0.5], true, true).expect("1x1 texture");
        tex_1x1.project_on(&one, 0.25, 4).expect("1x1 34-plane");
        tex_1x1
            .project_overlays_from_world_on(Some(&[2.5]), Some((&[0.2], &[0.8])), 0.25, 4)
            .expect("1x1 overlays");
        assert_eq!(tex_1x1.values(), &[0.0]);
        assert_eq!(tex_1x1.composition(), &[[0.0; 3]]);
        assert_eq!(tex_1x1.components(), &[[0_i8; CHANNELS]]);
        assert_eq!(tex_1x1.trail_energy(), Some(&[10.0_f32][..])); // 2.5 / 0.25
        assert_eq!(tex_1x1.vascular_conductance(), Some(&[0.5_f32][..])); // (0.2+0.2+0.8+0.8)/4

        // 2. Thin non-square grids (1x7 and 7x1) and downsampled asymmetric
        //    1025x513 grid (3x2 blocks with partial edge blocks): exact F64
        //    incident-edge oracle + 1-vs-4 worker bit-identity.
        for cells in [[1_u32, 7], [7, 1], [513, 515], [1025, 513]] {
            let (w, h) = (cells[0] as usize, cells[1] as usize);
            let n = w * h;
            let mut ex = vec![0.0_f32; n];
            let mut ey = vec![0.0_f32; n];
            let mut trail = vec![0.0_f32; n];
            for y in 0..h {
                for x in 0..w {
                    let idx = y * w + x;
                    ex[idx] = 0.1 + ((x * 5 + y * 11) % 37) as f32 * 0.02;
                    ey[idx] = 0.2 + ((x * 13 + y * 3) % 29) as f32 * 0.02;
                    trail[idx] = ((x * 7 + y * 17) % 53) as f32 * 0.01;
                }
            }
            let cell_size = [0.5_f64, 0.25_f64];
            let cell_area = cell_size[0] * cell_size[1];
            let mut t1 = FieldTexture::sized_with_v2(cells, cell_size, true, true).expect("t1");
            let mut t4 = FieldTexture::sized_with_v2(cells, cell_size, true, true).expect("t4");
            t1.project_overlays_from_world_on(Some(&trail), Some((&ex, &ey)), cell_area, 1)
                .expect("1 worker");
            t4.project_overlays_from_world_on(Some(&trail), Some((&ex, &ey)), cell_area, 4)
                .expect("4 workers");
            assert_eq!(t1.trail_energy(), t4.trail_energy(), "{cells:?} trail");
            assert_eq!(
                t1.vascular_conductance(),
                t4.vascular_conductance(),
                "{cells:?} vascular"
            );

            // Independent F64 oracle for vascular conductance.
            let [bx, by] = t1.block().map(|v| v as usize);
            let [tx, ty] = t1.texels().map(|v| v as usize);
            let mut expected = Vec::with_capacity(tx * ty);
            for row in 0..ty {
                let ys = row * by..((row + 1) * by).min(h);
                for col in 0..tx {
                    let xs = col * bx..((col + 1) * bx).min(w);
                    let mut sum = 0.0_f64;
                    for y in ys.clone() {
                        let yp = if y == 0 { h - 1 } else { y - 1 };
                        for x in xs.clone() {
                            let xp = if x == 0 { w - 1 } else { x - 1 };
                            sum += 0.25
                                * (f64::from(ex[y * w + x])
                                    + f64::from(ex[y * w + xp])
                                    + f64::from(ey[y * w + x])
                                    + f64::from(ey[yp * w + x]));
                        }
                    }
                    expected.push((sum / (ys.len() * xs.len()) as f64) as f32);
                }
            }
            assert_eq!(
                t1.vascular_conductance(),
                Some(expected.as_slice()),
                "{cells:?} oracle"
            );
        }

        // 3. Non-finite and negative overlay/edge values refuse with NumericalInvalid
        //    atomically without mutating EITHER previously projected overlay buffer.
        let before_trail = tex_1x1.trail_energy().expect("trail").to_vec();
        let before_vasc = tex_1x1.vascular_conductance().expect("vasc").to_vec();
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.01] {
            assert!(matches!(
                tex_1x1.project_overlays_from_world_on(Some(&[bad]), None, 0.25, 1),
                Err(CoreError::NumericalInvalid { .. })
            ));
            assert_eq!(tex_1x1.trail_energy(), Some(before_trail.as_slice()));
            assert_eq!(tex_1x1.vascular_conductance(), Some(before_vasc.as_slice()));

            assert!(matches!(
                tex_1x1.project_overlays_from_world_on(None, Some((&[bad], &[0.5])), 0.25, 1),
                Err(CoreError::NumericalInvalid { .. })
            ));
            assert_eq!(tex_1x1.trail_energy(), Some(before_trail.as_slice()));
            assert_eq!(tex_1x1.vascular_conductance(), Some(before_vasc.as_slice()));

            assert!(matches!(
                tex_1x1.project_overlays_from_world_on(None, Some((&[0.5], &[bad])), 0.25, 1),
                Err(CoreError::NumericalInvalid { .. })
            ));
            assert_eq!(tex_1x1.trail_energy(), Some(before_trail.as_slice()));
            assert_eq!(tex_1x1.vascular_conductance(), Some(before_vasc.as_slice()));

            assert!(matches!(
                tex_1x1.project_overlays_on(Some(&[1.0]), Some(&[bad]), 0.25, 1),
                Err(CoreError::NumericalInvalid { .. })
            ));
            assert_eq!(tex_1x1.trail_energy(), Some(before_trail.as_slice()));
            assert_eq!(tex_1x1.vascular_conductance(), Some(before_vasc.as_slice()));
        }

        // 4. 34-plane dual-carrier with zero-energy Carrier-0 cell beside a populated
        //    cell: Carrier-0 channels 0..24 project cleanly, 24..32 stay 0, zero NaNs.
        let mut planes_34 = vec![0.0_f32; 2 * DUAL_CARRIER_PLANES];
        // Cell 0: e_0 = 0, e_1 = 3.0; Cell 1: e_0 = 2.0, m_0 = 1.0, e_1 = 0.0.
        planes_34[25 * 2] = 3.0;
        planes_34[1] = 2.0;
        planes_34[2 + 1] = 1.0;
        let tex_34 = FieldTexture::from_planes_with_overlays_on(
            [2, 1],
            [1.0, 1.0],
            &planes_34,
            Some(&planes_34[25 * 2..26 * 2]),
            Some(&[0.25, 0.75]),
            2,
        )
        .expect("34-plane with zero-energy cell");
        assert_eq!(tex_34.values(), &[0.0, 2.0]);
        assert_eq!(tex_34.composition()[0], [0.0; 3]);
        assert_eq!(tex_34.components()[0], [0_i8; CHANNELS]);
        assert_eq!(tex_34.components()[1][0], 64); // round(127 * 0.5)
        assert!(tex_34.components()[1][24..].iter().all(|&c| c == 0));
        assert_eq!(tex_34.trail_energy(), Some(&[3.0, 0.0][..]));
        assert_eq!(tex_34.vascular_conductance(), Some(&[0.25, 0.75][..]));
    }

    #[test]
    fn field_texture_energy_plane_validation_and_nonnegative_saturation() {
        // 1. `saturate_nonnegative` maps positive overflow (including +Inf) to
        //    f32::MAX and NaN / negative / -0.0 to canonical +0.0.
        assert_eq!(saturate_nonnegative(f64::INFINITY), f32::MAX);
        assert_eq!(saturate_nonnegative(1.0e100), f32::MAX);
        assert_eq!(saturate_nonnegative(f64::NAN).to_bits(), 0.0_f32.to_bits());
        assert_eq!(
            saturate_nonnegative(f64::NEG_INFINITY).to_bits(),
            0.0_f32.to_bits()
        );
        assert_eq!(saturate_nonnegative(-0.0).to_bits(), 0.0_f32.to_bits());
        assert_eq!(saturate_nonnegative(-1.0).to_bits(), 0.0_f32.to_bits());
        assert_eq!(saturate_nonnegative(3.5), 3.5_f32);

        // 2. Positive overflow in density (`energy / cell_area` overflowing f64
        //    to +Inf) saturates cleanly to `f32::MAX` rather than black `0.0`.
        let saturated = FieldTexture::from_plane([1, 1], [1.0e-150, 1.0e-150], &[1.0e20])
            .expect("positive overflow saturates");
        assert_eq!(saturated.values(), &[f32::MAX]);

        // 3. `from_plane`, `from_planes`, and `from_planes_with_overlays` refuse
        //    non-finite and negative energy cells with `NumericalInvalid`.
        let mut valid_planes = vec![0.0_f32; 2 * (1 + CHANNELS)];
        valid_planes[0] = 2.0;
        valid_planes[1] = 4.0;
        valid_planes[2] = 1.0; // m_0 on cell 0
        let mut live = FieldTexture::from_planes([2, 1], [1.0, 1.0], &valid_planes)
            .expect("valid initial planes");
        let before_values = live.values().to_vec();
        let before_comp = live.composition().to_vec();
        let before_ch = live.components().to_vec();

        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.001] {
            assert!(
                matches!(
                    FieldTexture::from_plane([2, 1], [1.0, 1.0], &[1.0, bad]),
                    Err(CoreError::NumericalInvalid { .. })
                ),
                "from_plane must refuse {bad}"
            );
            let mut bad_planes = valid_planes.clone();
            bad_planes[1] = bad;
            assert!(
                matches!(
                    FieldTexture::from_planes([2, 1], [1.0, 1.0], &bad_planes),
                    Err(CoreError::NumericalInvalid { .. })
                ),
                "from_planes must refuse {bad}"
            );
            assert!(
                matches!(
                    FieldTexture::from_planes_with_overlays(
                        [2, 1],
                        [1.0, 1.0],
                        &bad_planes,
                        Some(&[0.5, 0.5]),
                        Some(&[0.5, 0.5]),
                    ),
                    Err(CoreError::NumericalInvalid { .. })
                ),
                "from_planes_with_overlays must refuse {bad}"
            );
            // Atomic refusal: `project_on` leaves previous texel buffers intact.
            assert!(
                matches!(
                    live.project_on(&bad_planes, 1.0, 2),
                    Err(CoreError::NumericalInvalid { .. })
                ),
                "project_on must refuse {bad}"
            );
            assert_eq!(live.values(), before_values.as_slice());
            assert_eq!(live.composition(), before_comp.as_slice());
            assert_eq!(live.components(), before_ch.as_slice());
        }
    }

    #[test]
    fn optimized_field_and_affinity_projections_match_independent_scalar_oracles_bit_for_bit() {
        // Independent naive scalar oracle for `affinity_projection`.
        let naive_affinity = |affinity: &[f64], limit: f64| -> [f32; 3] {
            let scale = (affinity.len() as f64).sqrt() * limit;
            if !(scale.is_finite() && scale > 0.0) {
                return [0.0; 3];
            }
            AFFINITY_AXES.map(|row| {
                let dot = affinity
                    .iter()
                    .enumerate()
                    .fold(0.0_f64, |sum, (channel, &value)| {
                        if (row & channel).count_ones().is_multiple_of(2) {
                            sum + value
                        } else {
                            sum - value
                        }
                    });
                let c = dot / scale;
                if c.is_finite() {
                    c.clamp(-1.0, 1.0) as f32
                } else {
                    0.0
                }
            })
        };

        // 1. 32-channel `affinity_projection` fast path matches naive popcount oracle bit-for-bit.
        let mut aff = [0.0_f64; CHANNELS];
        for (i, slot) in aff.iter_mut().enumerate() {
            *slot = ((i as f64 * 1.37).sin()) * 0.25;
        }
        for limit in [1.0, 0.37, 2.5, 0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(
                affinity_projection(&aff, limit).map(f32::to_bits),
                naive_affinity(&aff, limit).map(f32::to_bits),
                "affinity_projection mismatch for limit {limit}"
            );
        }
        let mut aff_edge = [-0.0_f64; CHANNELS];
        aff_edge[7] = f64::NAN;
        assert_eq!(
            affinity_projection(&aff_edge, 1.0).map(f32::to_bits),
            naive_affinity(&aff_edge, 1.0).map(f32::to_bits)
        );

        // 2. Adversarial half-integer (`k + 0.5`), ±1 ULP neighbor, cone-overshoot,
        //    non-finite moment, zero/subnormal/extreme energy test on `project_channel_f32`
        //    and `FieldTexture::from_planes_on`.
        let energies_probe = [
            0.0_f32,
            -0.0_f32,
            0.3_f32,
            1.7_f32,
            2.0_f32,
            3.141_592_7_f32,
            137.0_f32,
            254.0_f32,
            1.0e-35_f32,
            1.0e35_f32,
        ];
        // Build a 10 × 17 grid covering every half-integer `k + 0.5` across channels and cells.
        let (gx, gy) = (10_usize, 17_usize);
        let n = gx * gy;
        let mut planes = vec![0.0_f32; n * (1 + CHANNELS)];
        for y in 0..gy {
            for x in 0..gx {
                let idx = y * gx + x;
                let e = if y == 0 || y == 8 {
                    // Empty rows to exercise whole-row skipping.
                    0.0_f32
                } else {
                    energies_probe[x]
                };
                planes[idx] = e;
                for c in 0..CHANNELS {
                    let k = ((y * 32 + c) % 127) as f64;
                    let sign = if (x + y + c).is_multiple_of(2) {
                        1.0
                    } else {
                        -1.0
                    };
                    let raw = match (y + c) % 7 {
                        0 => (sign * (k + 0.5) / 127.0 * f64::from(e)) as f32,
                        1 => ((sign * (k + 0.5) / 127.0 * f64::from(e)) as f32).next_up(),
                        2 => ((sign * (k + 0.5) / 127.0 * f64::from(e)) as f32).next_down(),
                        3 => (sign * 1.003 * f64::from(e)) as f32,
                        4 => (sign * 1.5 * f64::from(e)) as f32,
                        5 if x == 3 && c == 11 => f32::NAN,
                        5 if x == 4 && c == 12 => f32::INFINITY,
                        5 if x == 5 && c == 13 => f32::NEG_INFINITY,
                        _ => (sign * (k + 0.25) / 127.0 * f64::from(e)) as f32,
                    };
                    planes[(1 + c) * n + idx] = raw;
                }
            }
        }

        // Compare against independent scalar oracle on both power-of-2 and non-power-of-2 cell areas.
        for cell_size in [[0.25, 0.25], [0.3, 0.7]] {
            let area = cell_size[0] * cell_size[1];
            let tex_1 = FieldTexture::from_planes_on([gx as u32, gy as u32], cell_size, &planes, 1)
                .expect("1-worker texture");
            let tex_4 = FieldTexture::from_planes_on([gx as u32, gy as u32], cell_size, &planes, 4)
                .expect("4-worker texture");
            assert_eq!(tex_1, tex_4, "pool parity for cell_size {cell_size:?}");
            for idx in 0..n {
                let e = f64::from(planes[idx]);
                let want_val = saturate_nonnegative(e / area);
                assert_eq!(
                    tex_1.values()[idx].to_bits(),
                    want_val.to_bits(),
                    "energy value mismatch at cell {idx}"
                );
                let mut m = [0.0_f64; CHANNELS];
                for (c, (m_slot, &got_step)) in
                    m.iter_mut().zip(&tex_1.components()[idx]).enumerate()
                {
                    *m_slot = f64::from(planes[(1 + c) * n + idx]);
                    let want_step = component_step(*m_slot, e);
                    assert_eq!(
                        got_step, want_step,
                        "component_step mismatch at cell {idx} ch {c} (e={e}, m={})",
                        *m_slot
                    );
                }
                let want_comp = naive_affinity(&m, e);
                assert_eq!(
                    tex_1.composition()[idx].map(f32::to_bits),
                    want_comp.map(f32::to_bits),
                    "composition mismatch at cell {idx}"
                );
            }
        }

        // 3. Reused-slot transitions: dense 32-ch -> sparse prefix/suffix ->
        //    24-ch dual-carrier (channels 24..32 MUST zero) -> energy-only (all zero) -> dense.
        let mut slot = FieldTexture::from_planes([gx as u32, gy as u32], [0.25, 0.25], &planes)
            .expect("initial dense 32-ch");
        assert!(slot.components().iter().any(|t| t[31] != 0));

        // 3a. Sparse: only middle columns `x ∈ 4..7` on rows `y ∈ 2..4` have energy.
        let mut sparse = vec![0.0_f32; n * (1 + CHANNELS)];
        for y in 2..4 {
            for x in 4..7 {
                let idx = y * gx + x;
                sparse[idx] = 2.0;
                sparse[(1 + 31) * n + idx] = 1.0;
            }
        }
        slot.project_on(&sparse, 0.0625, 4)
            .expect("sparse reuse succeeds");
        for y in 0..gy {
            for x in 0..gx {
                let idx = y * gx + x;
                if (2..4).contains(&y) && (4..7).contains(&x) {
                    assert_eq!(slot.components()[idx][31], 64);
                } else {
                    assert_eq!(slot.composition()[idx], [0.0; 3]);
                    assert_eq!(slot.components()[idx], [0; CHANNELS]);
                }
            }
        }

        // 3b. Dual-carrier (34 planes, 24 Carrier-0 channels) onto a slot previously holding ch 31 != 0.
        slot.project_on(&planes, 0.0625, 4)
            .expect("refill dense 32-ch");
        assert!(slot.components().iter().any(|t| t[31] != 0));
        let mut dual = vec![0.0_f32; n * DUAL_CARRIER_PLANES];
        for idx in 0..n {
            dual[idx] = 4.0;
            for c in 0..CARRIER0_MOMENT_CHANNELS {
                dual[(1 + c) * n + idx] = if c.is_multiple_of(2) { 2.0 } else { -2.0 };
            }
        }
        slot.project_on(&dual, 0.0625, 4)
            .expect("dual-carrier reuse succeeds");
        for (idx, texel) in slot.components().iter().enumerate() {
            for (c, &got) in texel[..CARRIER0_MOMENT_CHANNELS].iter().enumerate() {
                assert_eq!(got, if c.is_multiple_of(2) { 64 } else { -64 });
            }
            for (offset, &got) in texel[CARRIER0_MOMENT_CHANNELS..].iter().enumerate() {
                let c = CARRIER0_MOMENT_CHANNELS + offset;
                assert_eq!(
                    got, 0,
                    "stale dual-carrier trailing channel {c} at cell {idx}"
                );
            }
        }

        // 3c. Energy-only onto a slot previously holding non-zero composition & components.
        slot.project_on(&dual[..n], 0.0625, 4)
            .expect("energy-only reuse succeeds");
        assert!(slot.composition().iter().all(|&c| c == [0.0; 3]));
        assert!(slot.components().iter().all(|&t| t == [0; CHANNELS]));

        // 4. Partial-block `!whole` grid (515 × 17, block = [2, 1], width = 258,
        //    full_x = 257, trailing 1-cell column) including sparse active span
        //    hitting ONLY the last partial column.
        let (px, py) = (515_usize, 17_usize);
        let pn = px * py;
        let mut partial_planes = vec![0.0_f32; pn * (1 + CHANNELS)];
        // Row 0: energy ONLY in trailing partial cell x = 514.
        partial_planes[514] = 2.0;
        partial_planes[(1 + 5) * pn + 514] = 1.0;
        // Row 3: energy across prefix, middle, and trailing partial cell.
        for x in [0_usize, 1, 100, 101, 514] {
            partial_planes[3 * px + x] = 3.0;
            partial_planes[(1 + 17) * pn + 3 * px + x] = -1.5;
        }
        let partial_1 =
            FieldTexture::from_planes_on([px as u32, py as u32], [0.5, 0.5], &partial_planes, 1)
                .expect("partial 1-worker");
        let partial_4 =
            FieldTexture::from_planes_on([px as u32, py as u32], [0.5, 0.5], &partial_planes, 4)
                .expect("partial 4-worker");
        assert_eq!(partial_1, partial_4);
        let tw = partial_1.texels()[0] as usize;
        assert_eq!(tw, 258);
        assert_eq!(partial_1.values()[257], 8.0); // 2.0 / (1 * 0.25)
        assert_eq!(partial_1.components()[257][5], 64);
        assert_eq!(partial_1.components()[3 * tw][17], -64);
        assert_eq!(partial_1.components()[3 * tw + 50][17], -64);
        assert_eq!(partial_1.components()[3 * tw + 257][17], -64);

        // 5. Vascular `1×1` fast path (including `1×1` edge case & prime `17×13` grid)
        //    and scalar trail overlay match naive periodic scalar oracles bit-for-bit.
        for (vx, vy) in [(1_usize, 1_usize), (17, 13)] {
            let vn = vx * vy;
            let ex: Vec<f32> = (0..vn)
                .map(|i| ((i * 73 + 11) % 97) as f32 * 0.01)
                .collect();
            let ey: Vec<f32> = (0..vn).map(|i| ((i * 59 + 7) % 89) as f32 * 0.01).collect();
            let mut out_1 = Vec::new();
            let mut out_4 = Vec::new();
            let grid = ScalarOverlayGrid {
                cells: [vx, vy],
                block: [1, 1],
                texels: [vx, vy],
                divisor: 1.0,
            };
            project_validated_vascular_edges_on(&ex, &ey, grid, 1, &mut out_1)
                .expect("vascular 1-worker");
            project_validated_vascular_edges_on(&ex, &ey, grid, 4, &mut out_4)
                .expect("vascular 4-worker");
            assert_eq!(out_1, out_4);
            for y in 0..vy {
                let y_prev = if y == 0 { vy - 1 } else { y - 1 };
                for x in 0..vx {
                    let x_prev = if x == 0 { vx - 1 } else { x - 1 };
                    let want = saturate_nonnegative(
                        0.25 * (f64::from(ex[y * vx + x])
                            + f64::from(ex[y * vx + x_prev])
                            + f64::from(ey[y * vx + x])
                            + f64::from(ey[y_prev * vx + x])),
                    );
                    assert_eq!(
                        out_1[y * vx + x].to_bits(),
                        want.to_bits(),
                        "vascular mismatch at ({x},{y}) on {vx}×{vy}"
                    );
                }
            }

            let mut trail_1 = Vec::new();
            let mut trail_4 = Vec::new();
            let trail_grid = ScalarOverlayGrid {
                cells: [vx, vy],
                block: [1, 1],
                texels: [vx, vy],
                divisor: 0.0625,
            };
            project_validated_scalar_plane_on(&ex, trail_grid, 1, &mut trail_1)
                .expect("trail 1-worker");
            project_validated_scalar_plane_on(&ex, trail_grid, 4, &mut trail_4)
                .expect("trail 4-worker");
            assert_eq!(trail_1, trail_4);
            for (&raw, &got) in ex.iter().zip(&trail_1) {
                let want = saturate_nonnegative(f64::from(raw) / 0.0625);
                assert_eq!(got.to_bits(), want.to_bits());
            }
        }
    }
}
