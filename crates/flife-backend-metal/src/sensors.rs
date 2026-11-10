//! FL-031 (`flife-8le`), second part: phase 3's sensor pyramid and its
//! sampling on Metal (plan §6.1, §8.1, §10.7). [`MetalSensors`] is a
//! [`SensorBackend`]: every organism's raw sample, bit-identical to
//! [`flife_field::PyramidView::sample`].
//!
//! The work is split by what each side does best:
//! - the device builds every coarse level (`positive-box-v1`'s 2×2
//!   averages of densities) and samples every organism's 33 planes at its
//!   one or two levels, value and derivatives, blended by `log-linear-v1`'s
//!   weight, in `sensors.metal` on `softfloat64.metal`'s binary64;
//! - the host gives each query its level, blend weight and corner cells
//!   through flife-field's own [`SensorPyramid::blend`] and
//!   [`SensorPyramid::corners`], so the position wrapping and the floor
//!   are the reference's own code.
//!
//! One tick is one command: the levels, then the samples. The field is
//! copied to the device and the samples back every tick; nothing is resident
//! yet (see the crate contract's known gaps).

use flife_core::CoreError;
use flife_field::grid::FIELD_PLANES;
use flife_field::{
    Corners, FieldGrid, FieldLayout, FieldState, RawSample, SensorBackend, SensorPyramid,
    SensorQuery,
};
use ft_kernel_metal::compute::{Grid, MathMode, Pipeline};

use crate::owner::{DeviceOwner, Kernel, Limits, Owned, Purpose, metal};

/// The sensor kernel source, with the software binary64 ahead of it.
pub const SENSORS_MSL: &str = concat!(
    include_str!("softfloat64.metal"),
    include_str!("sensors.metal")
);

/// Planes per level.
const PLANES: usize = FIELD_PLANES as usize;

/// `sensors.metal`'s layouts.
const QUERY: usize = 20;
const Q_LEVEL: usize = 0;
const Q_BLENDS: usize = 1;
const Q_LOW: usize = 2;
const Q_HIGH: usize = 10;
const Q_T: usize = 18;
const S_HEADER: usize = 4;
const S_LEVEL: usize = 7;
const LEVEL_WORDS: usize = 6;
/// Words of one query's samples: three arrays of 33 binary64 values.
const SAMPLE_WORDS: usize = 3 * PLANES * 2;

/// Threads per threadgroup.
const THREADS: usize = 256;

fn words(value: f64) -> [u32; 2] {
    let bits = value.to_bits();
    [(bits & 0xFFFF_FFFF) as u32, (bits >> 32) as u32]
}

fn put(target: &mut [u32], at: usize, value: f64) {
    target[at..at + 2].copy_from_slice(&words(value));
}

fn overflow() -> CoreError {
    CoreError::Overflow {
        quantity: "metal sensor size",
    }
}

fn word(value: usize) -> Result<u32, CoreError> {
    u32::try_from(value).map_err(|_| overflow())
}

/// One level's place in the levels buffer.
#[derive(Debug, Clone, Copy)]
struct Level {
    cells_x: usize,
    cells_y: usize,
    /// Its first value in the levels buffer (level 0 is the field).
    offset: usize,
}

/// The device buffers for one pyramid and a query capacity.
#[derive(Debug)]
struct Buffers {
    grid: FieldGrid,
    levels: u32,
    capacity: usize,
    field: Owned<f32>,
    pyramid: Owned<u32>,
    level_params: Vec<Owned<u32>>,
    sample_params: Owned<u32>,
    queries: Owned<u32>,
    samples: Owned<u32>,
}

/// What the host prepares and reads back each tick.
#[derive(Debug, Default)]
struct Host {
    queries: Vec<u32>,
    samples: Vec<u32>,
}

/// Phase 3's sampling on Metal; see the module documentation.
pub struct MetalSensors {
    owner: DeviceOwner,
    first: Pipeline,
    next: Pipeline,
    sample: Pipeline,
    buffers: Option<Buffers>,
    host: Host,
}

impl std::fmt::Debug for MetalSensors {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MetalSensors")
            .field("device", &self.owner.device_name())
            .field("buffers", &self.buffers)
            .finish_non_exhaustive()
    }
}

/// Every level's geometry and offset, level 0 first.
fn layout(pyramid: &SensorPyramid) -> Result<Vec<Level>, CoreError> {
    let mut levels = Vec::new();
    let mut offset = 0_usize;
    for level in 0..=pyramid.levels() {
        let geometry = pyramid.level_grid(level).ok_or_else(overflow)?;
        let (cells_x, cells_y) = (geometry.cells_x as usize, geometry.cells_y as usize);
        levels.push(Level {
            cells_x,
            cells_y,
            offset: if level == 0 { 0 } else { offset },
        });
        if level > 0 {
            offset = cells_x
                .checked_mul(cells_y)
                .and_then(|cells| cells.checked_mul(PLANES))
                .and_then(|values| offset.checked_add(values))
                .ok_or_else(overflow)?;
        }
    }
    Ok(levels)
}

impl MetalSensors {
    /// Open the default Metal device and compile the sensor kernels.
    ///
    /// # Errors
    /// [`CoreError::MissingCapability`] without a Metal device (always, off
    /// macOS); [`CoreError::StateConflict`] if the kernels do not compile.
    pub fn open() -> Result<Self, CoreError> {
        Self::with_owner(DeviceOwner::open(Limits::UNBOUNDED)?)
    }

    /// The sensor kernels on `owner`'s device; their buffers are `owner`'s.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] if the kernels do not compile.
    pub fn with_owner(owner: DeviceOwner) -> Result<Self, CoreError> {
        let library = owner.library(SENSORS_MSL, MathMode::Safe)?;
        let pipeline = |name: &str| library.pipeline(name).map_err(metal);
        Ok(Self {
            first: pipeline("pyramid_first")?,
            next: pipeline("pyramid_next")?,
            sample: pipeline("sensor_sample")?,
            owner,
            buffers: None,
            host: Host::default(),
        })
    }

    /// The device's name.
    #[must_use]
    pub fn device_name(&self) -> String {
        self.owner.device_name()
    }

    /// The owner of every buffer these kernels use.
    #[must_use]
    pub const fn owner(&self) -> &DeviceOwner {
        &self.owner
    }

    /// Buffers for `pyramid` and at least `count` queries, allocated on
    /// first use and again when the pyramid changes or the queries outgrow
    /// them; the old ones retire when dropped.
    fn ensure_buffers(
        &mut self,
        pyramid: &SensorPyramid,
        levels: &[Level],
        count: usize,
    ) -> Result<(), CoreError> {
        let grid = *pyramid.grid();
        let current = self.buffers.as_ref().is_some_and(|buffers| {
            buffers.grid == grid && buffers.levels == pyramid.levels() && buffers.capacity >= count
        });
        if current {
            return Ok(());
        }
        self.buffers = None;
        let capacity = count.max(1).next_power_of_two();
        let cells = usize::try_from(grid.cell_count()).map_err(|_| overflow())?;
        let values = cells.checked_mul(PLANES).ok_or_else(overflow)?;
        let coarse = levels.last().map_or(0, |last| {
            if levels.len() > 1 {
                last.offset + last.cells_x * last.cells_y * PLANES
            } else {
                0
            }
        });
        let owner = &self.owner;
        let mut level_params = Vec::new();
        for _ in 1..levels.len() {
            level_params.push(owner.zeroed_owned(LEVEL_WORDS)?);
        }
        self.buffers = Some(Buffers {
            grid,
            levels: pyramid.levels(),
            capacity,
            field: owner.zeroed_owned(values)?,
            pyramid: owner.zeroed_owned(coarse.checked_mul(2).ok_or_else(overflow)?.max(2))?,
            level_params,
            sample_params: owner.zeroed_owned(S_HEADER + S_LEVEL * levels.len())?,
            queries: owner.zeroed_owned(capacity.checked_mul(QUERY).ok_or_else(overflow)?)?,
            samples: owner
                .zeroed_owned(capacity.checked_mul(SAMPLE_WORDS).ok_or_else(overflow)?)?,
        });
        Ok(())
    }
}

/// A query's level part: its corner cells and fractions.
fn put_corners(query: &mut [u32], at: usize, corners: &Corners) {
    query[at..at + 4].copy_from_slice(&[
        corners.columns[0],
        corners.columns[1],
        corners.rows[0],
        corners.rows[1],
    ]);
    put(query, at + 4, corners.fx);
    put(query, at + 6, corners.fy);
}

impl SensorBackend for MetalSensors {
    fn describe(&self) -> String {
        format!("metal ({})", self.owner.device_name())
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one tick's queries, command and samples, in order"
    )]
    fn sample(
        &mut self,
        pyramid: &SensorPyramid,
        field: &FieldState,
        queries: &[SensorQuery],
        samples: &mut Vec<RawSample>,
    ) -> Result<(), CoreError> {
        let grid = *pyramid.grid();
        if field.grid() != &grid {
            return Err(CoreError::LayoutMismatch {
                buffer: "metal sensor pyramid",
                detail: format!("built for {grid:?}, offered a field on {:?}", field.grid()),
            });
        }
        FieldLayout::planar(grid)?.ensure_compatible(field.layout())?;
        let levels = layout(pyramid)?;
        let count = queries.len();
        samples.clear();
        if count == 0 {
            return Ok(());
        }

        // The host's part: each query's level, weight and corners, through
        // flife-field's own functions.
        let host = &mut self.host;
        host.queries.clear();
        host.queries.resize(count * QUERY, 0);
        for (query, descriptor) in queries.iter().zip(host.queries.as_chunks_mut::<QUERY>().0) {
            let (level, t) = pyramid.blend(query.scale)?;
            descriptor[Q_LEVEL] = level;
            put_corners(descriptor, Q_LOW, &pyramid.corners(level, query.position)?);
            if t != 0.0 {
                descriptor[Q_BLENDS] = 1;
                put_corners(
                    descriptor,
                    Q_HIGH,
                    &pyramid.corners(level + 1, query.position)?,
                );
                put(descriptor, Q_T, t);
            }
        }

        self.ensure_buffers(pyramid, &levels, count)?;
        let Self {
            owner,
            first,
            next,
            sample,
            buffers,
            host,
        } = self;
        let buffers = buffers.as_ref().ok_or_else(overflow)?;
        let area = grid.cell_area();
        let cells = usize::try_from(grid.cell_count()).map_err(|_| overflow())?;

        // The parameters, and the field.
        let mut sample_params = vec![0_u32; S_HEADER + S_LEVEL * levels.len()];
        sample_params[0] = word(count)?;
        sample_params[1] = word(cells)?;
        put(&mut sample_params, 2, area);
        for (index, level) in levels.iter().enumerate() {
            let geometry = pyramid
                .level_grid(u32::try_from(index).map_err(|_| overflow())?)
                .ok_or_else(overflow)?;
            let at = S_HEADER + S_LEVEL * index;
            sample_params[at] = word(level.cells_x)?;
            sample_params[at + 1] = word(level.cells_x * level.cells_y)?;
            sample_params[at + 2] = word(level.offset)?;
            put(&mut sample_params, at + 3, geometry.spacing_x);
            put(&mut sample_params, at + 5, geometry.spacing_y);
        }
        for (index, params) in buffers.level_params.iter().enumerate() {
            // Coarse level `index + 1` from level `index`.
            let (fine, coarse) = (levels[index], levels[index + 1]);
            let mut words_ = [0_u32; LEVEL_WORDS];
            words_[0] = word(fine.cells_x)?;
            words_[1] = word(coarse.cells_x)?;
            words_[2] = word(coarse.cells_y)?;
            if index == 0 {
                words_[3] = word(cells)?;
                let area = words(area);
                words_[4] = area[0];
                words_[5] = area[1];
            } else {
                words_[3] = word(fine.cells_x * fine.cells_y)?;
                words_[4] = word(fine.offset)?;
                words_[5] = word(coarse.offset)?;
            }
            owner.write(params.buffer())?.write(0, &words_)?;
        }
        owner
            .write(buffers.field.buffer())?
            .write(0, field.as_slice())?;
        owner
            .write(buffers.sample_params.buffer())?
            .write(0, &sample_params)?;
        owner
            .write(buffers.queries.buffer())?
            .write(0, &host.queries)?;

        // One command: each coarse level from the one below, then the
        // samples.
        let first_bindings = [
            buffers.field.buffer().reads(),
            buffers.level_params.first().map_or_else(
                || buffers.sample_params.buffer().reads(),
                |params| params.buffer().reads(),
            ),
            buffers.pyramid.buffer().writes(),
        ];
        let next_bindings: Vec<_> = buffers
            .level_params
            .iter()
            .skip(1)
            .map(|params| [params.buffer().reads(), buffers.pyramid.buffer().writes()])
            .collect();
        let sample_bindings = [
            buffers.field.buffer().reads(),
            buffers.pyramid.buffer().reads(),
            buffers.sample_params.buffer().reads(),
            buffers.queries.buffer().reads(),
            buffers.samples.buffer().writes(),
        ];
        let mut kernels: Vec<Kernel<'_>> = Vec::new();
        if let Some(level) = levels.get(1) {
            kernels.push(Kernel {
                pipeline: first,
                bindings: &first_bindings,
                grid: Grid::linear(
                    (level.cells_x * level.cells_y * PLANES).div_ceil(THREADS),
                    THREADS,
                ),
            });
        }
        for (bindings, level) in next_bindings.iter().zip(levels.iter().skip(2)) {
            kernels.push(Kernel {
                pipeline: next,
                bindings,
                grid: Grid::linear(
                    (level.cells_x * level.cells_y * PLANES).div_ceil(THREADS),
                    THREADS,
                ),
            });
        }
        kernels.push(Kernel {
            pipeline: sample,
            bindings: &sample_bindings,
            grid: Grid::linear((count * PLANES).div_ceil(THREADS), THREADS),
        });
        owner.submit_batch(&kernels)?;

        // The samples, checked as the reference checks them: a non-finite
        // field value a sample read made it a NaN (`sensors.metal`).
        host.samples.resize(count * SAMPLE_WORDS, 0);
        owner
            .read(buffers.samples.buffer(), Purpose::Readback)?
            .read(&mut host.samples)?;
        samples
            .try_reserve(count)
            .map_err(|_| CoreError::AllocationFailed {
                buffer: "metal sensor samples",
                bytes: u64::try_from(count.saturating_mul(size_of::<RawSample>()))
                    .unwrap_or(u64::MAX),
            })?;
        for block in host.samples.as_chunks::<SAMPLE_WORDS>().0 {
            let value = |array: usize, plane: usize| {
                let at = 2 * (array * PLANES + plane);
                f64::from_bits(u64::from(block[at]) | (u64::from(block[at + 1]) << 32))
            };
            let raw = RawSample {
                values: std::array::from_fn(|plane| value(0, plane)),
                dx: std::array::from_fn(|plane| value(1, plane)),
                dy: std::array::from_fn(|plane| value(2, plane)),
            };
            let finite = raw
                .values
                .iter()
                .chain(&raw.dx)
                .chain(&raw.dy)
                .all(|value| value.is_finite());
            if !finite {
                return Err(CoreError::NumericalInvalid {
                    quantity: "sensor sample",
                    detail: "interpolation produced a nonfinite value".into(),
                });
            }
            samples.push(raw);
        }
        Ok(())
    }
}
