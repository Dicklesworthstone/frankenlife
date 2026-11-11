//! One immutable compact render epoch (plan Appendix B.3, §9.5, §13.2).
//!
//! An [`Epoch`] is a compact projection of exactly one committed world:
//! - its source tick and resource epoch;
//! - one row per live organism, in stable identity order;
//! - a bounded field texture derived from the energy plane;
//! - a small metric summary.
//!
//! Every view declares its [`Layout`]: element type, components, count and
//! bounds. A slot's buffers are allocated once, at the world's population
//! ceiling and its texture size, and each projection refills them in place.
//! Publishing therefore never allocates.
//!
//! Rows follow identity order, never storage-slot order. Two storage
//! arrangements of one world project the same epoch (C.7), and a consumer
//! matches organisms between epochs by identity (§13.5).
//!
//! The epoch carries the inputs of §13.4's mappings: lineage, energy,
//! generation, topology, motion and the chemical affinity's fixed
//! projection ([`affinity_projection`]). Turning them into sizes and
//! colours is a visual mapping (`presentation.color_mapping`, FL-019).
//! Scientific code never reads an epoch back.

use flife_core::ids::{OrganismId, RenderEpoch, ResourceEpoch};
use flife_core::{CoreError, Tick, TimeStep};
use flife_engine::World;

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
    let scale = (affinity.len() as f64).sqrt() * limit;
    if !(scale.is_finite() && scale > 0.0) {
        return [0.0; 3];
    }
    AFFINITY_AXES.map(|row| {
        let dot = affinity
            .iter()
            .enumerate()
            .fold(0.0_f64, |sum, (channel, &value)| {
                if (row & channel).count_ones() % 2 == 0 {
                    sum + value
                } else {
                    sum - value
                }
            });
        let coordinate = dot / scale;
        if coordinate.is_finite() {
            coordinate.clamp(-1.0, 1.0) as f32
        } else {
            0.0
        }
    })
}

/// A view's element type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dtype {
    /// Unsigned 8-bit.
    U8,
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
            Self::U8 => 1,
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
}

impl View {
    /// Every view, in declaration order.
    pub const ALL: [Self; 9] = [
        Self::Identity,
        Self::Position,
        Self::Velocity,
        Self::Energy,
        Self::Lineage,
        Self::Generation,
        Self::Topology,
        Self::Affinity,
        Self::Field,
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

/// A bounded field texture derived from the committed energy plane (§9.5,
/// B.3: an explicit channel projection and source scale).
#[derive(Debug, Clone, PartialEq)]
pub struct FieldTexture {
    channel: FieldChannel,
    cells: [u32; 2],
    block: [u32; 2],
    texels: [u32; 2],
    texel_size: [f64; 2],
    values: Vec<f32>,
}

impl FieldTexture {
    /// The texture of one row-major energy `plane` on a `cells` grid of
    /// `cell_size` cells: for previews and checks outside an epoch.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] for an empty grid or a plane of the
    /// wrong length; [`CoreError::AllocationFailed`].
    pub fn from_plane(
        cells: [u32; 2],
        cell_size: [f64; 2],
        plane: &[f32],
    ) -> Result<Self, CoreError> {
        let mut texture = Self::sized(cells, cell_size)?;
        texture.project(plane, cell_size[0] * cell_size[1])?;
        Ok(texture)
    }

    /// A texture for a `cells` grid of `cell_size` cells, allocated once.
    fn sized(cells: [u32; 2], cell_size: [f64; 2]) -> Result<Self, CoreError> {
        if cells.contains(&0) {
            return Err(CoreError::LayoutMismatch {
                buffer: "render field texture",
                detail: format!("a {cells:?} grid has no cells"),
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

    /// Refill from a row-major energy plane of `cell_area` cells. Each texel
    /// sums its block in F64, rows then columns, and divides by the block's
    /// area.
    fn project(&mut self, energy: &[f32], cell_area: f64) -> Result<(), CoreError> {
        let [cells_x, cells_y] = self.cells.map(|n| n as usize);
        if energy.len() != cells_x * cells_y {
            return Err(CoreError::LayoutMismatch {
                buffer: "render field texture",
                detail: format!(
                    "an energy plane of {} values for a {cells_x} × {cells_y} grid",
                    energy.len()
                ),
            });
        }
        let [block_x, block_y] = self.block.map(|n| n as usize);
        self.values.clear();
        for texel_y in 0..self.texels[1] as usize {
            let rows = texel_y * block_y..((texel_y + 1) * block_y).min(cells_y);
            for texel_x in 0..self.texels[0] as usize {
                let columns = texel_x * block_x..((texel_x + 1) * block_x).min(cells_x);
                let mut sum = 0.0_f64;
                for y in rows.clone() {
                    for &value in &energy[y * cells_x + columns.start..y * cells_x + columns.end] {
                        sum += f64::from(value);
                    }
                }
                let area = (rows.len() * columns.len()) as f64 * cell_area;
                self.values.push(saturate(sum / area));
            }
        }
        Ok(())
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
    field: FieldTexture,
    metrics: Metrics,
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

/// `value` rounded to the nearest F32, saturating at the largest finite one.
fn saturate(value: f64) -> f32 {
    (value as f32).clamp(-f32::MAX, f32::MAX)
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
        epoch.project(world, RenderEpoch::new(1), None)?;
        Ok(epoch)
    }

    /// A slot for `world`'s epochs: every buffer allocated once, at its
    /// population ceiling and texture size. It holds `world`'s founding
    /// metadata until its first projection.
    pub(crate) fn allocate(world: &World) -> Result<Self, CoreError> {
        let capacity = world.population().capacity();
        let grid = world.field().grid();
        Ok(Self {
            id: RenderEpoch::new(0),
            tick: world.tick(),
            resource: world.epoch(),
            prior: None,
            step: world.step_size(),
            maximum_speed: world.maximum_speed(),
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
            field: FieldTexture::sized(
                [grid.cells_x(), grid.cells_y()],
                [grid.cell_width(), grid.cell_height()],
            )?,
            metrics: Metrics {
                tick: world.tick(),
                live: 0,
                organism_energy: 0.0,
                field_energy: 0.0,
                max_generation: 0,
                events: 0,
            },
        })
    }

    /// Refill this slot from `world`'s committed state as epoch `id`. On an
    /// error the slot holds nothing publishable; the caller does not
    /// publish it.
    pub(crate) fn project(
        &mut self,
        world: &World,
        id: RenderEpoch,
        prior: Option<Prior>,
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
        let width = extent.map(|length| length as f32);
        self.ids.clear();
        self.positions.clear();
        self.velocities.clear();
        self.energies.clear();
        self.lineages.clear();
        self.generations.clear();
        self.topologies.clear();
        self.affinities.clear();
        let affinity_limit = world.trait_limits().affinity_norm();
        let mut organism_energy = 0.0_f64;
        let mut max_generation = 0_u32;
        for organism in population.iter() {
            let genome =
                population
                    .genome(organism.id)
                    .ok_or_else(|| CoreError::StateConflict {
                        operation: "project render epoch",
                        detail: format!("{} has no genome", organism.id),
                    })?;
            let topology =
                u8::try_from(genome.topology().index()).map_err(|_| CoreError::Overflow {
                    quantity: "render topology index",
                })?;
            self.ids.push(organism.id.get());
            self.positions.push([
                fold(organism.position[0], width[0]),
                fold(organism.position[1], width[1]),
            ]);
            // Every committed component is within the speed limit, and
            // rounding to nearest is monotone, so each F32 component is
            // within the F32 limit.
            self.velocities
                .push([organism.velocity[0] as f32, organism.velocity[1] as f32]);
            self.energies.push(saturate(organism.energy));
            self.lineages.push(organism.lineage.get());
            self.generations.push(organism.generation.get());
            self.topologies.push(topology);
            self.affinities.push(affinity_projection(
                genome.traits().affinity(),
                affinity_limit,
            ));
            organism_energy += organism.energy;
            max_generation = max_generation.max(organism.generation.get());
        }
        self.field
            .project(world.field().energy(), grid.cell_area())?;
        self.id = id;
        self.tick = world.tick();
        self.resource = world.epoch();
        self.prior = prior;
        self.step = world.step_size();
        self.affinity_limit = affinity_limit;
        self.metrics = Metrics {
            tick: world.tick(),
            live: live as u64,
            organism_energy,
            field_energy: world.field().energy_total(),
            max_generation,
            events: world.events(),
        };
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

    /// Every view's declared layout, in [`View::ALL`] order.
    #[must_use]
    pub fn layouts(&self) -> [Layout; 9] {
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
        ]
    }

    /// The bytes this slot's buffers have actually reserved. Allocated
    /// once, it is the population ceiling times 53 bytes a row plus 4 a
    /// texel, and a projection never changes it.
    #[must_use]
    pub fn reserved_bytes(&self) -> u64 {
        fn bytes<T>(values: &Vec<T>) -> u64 {
            (values.capacity() as u64).saturating_mul(size_of::<T>() as u64)
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
            bytes(&self.field.values),
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
        let texels = self.field.texels.map(|n| n as usize);
        if self.field.values.len() != texels[0] * texels[1] {
            return refuse(
                View::Field,
                format!("{} texels for {texels:?}", self.field.values.len()),
            );
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
                    let found = if layout.view == View::Affinity {
                        self.affinities
                            .iter()
                            .find(|a| a.iter().any(exceeds))
                            .map(|a| format!("{a:?}"))
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
