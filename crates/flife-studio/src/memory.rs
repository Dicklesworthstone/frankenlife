//! The Studio's peak-memory admission (FL-043; plan §19.4, INV-15).
//!
//! The Studio's observers are its feed, its view and its frames. Before the
//! controller backend is installed, [`admit`] charges them as the render
//! term of [`World::admit_memory`], and the backend gets what the budget
//! leaves:
//!
//! - the feed: `execution.render_queue_capacity` epoch slots, each what the
//!   founding epoch reserves;
//! - the view's own buffers and one frame's job
//!   ([`WorldView::reserved_bytes`], [`WorldView::frame_bytes`]);
//! - the frame's upload: FrankenManim's Metal route flattens the job's
//!   rows and bins and the textures it samples on the host and copies them
//!   to the device, each charged at the view's
//!   [`WorldView::preparation_limit`] plus its [`WorldView::texel_bytes`].
//!   FrankenManim bounds neither. Since its `9b116eee` the route keeps the
//!   tables in device buffers between frames, each up to a quarter larger
//!   than the largest table it has held, and never shrinks them
//!   (`MetalRenderer::resident_table_bytes`). On 48 frames through one
//!   renderer, up to 16,384 organisms at 4K, two pixels per point and a
//!   16x zoom, at FrankenManim `d670e74e`: the host held at most 45.9%
//!   and copied at most 43.2% of a frame's sum, and the kept buffers were
//!   at most 51.8% of the largest sum so far;
//! - per drawable pixel, [`DEVICE_BYTES_PER_PIXEL`]: the route's RGBA16F
//!   and RGBA8 surfaces and the window's drawables (offscreen, the one
//!   frame read back to the host instead, 4 of the drawables' 12); and a
//!   status word per 16 x 16 tile;
//! - while a bloom is configured, its row sums on the device
//!   ([`WorldView::bloom_bytes`], FL-019).
//!
//! What the device holds for offscreen frames is read from the device
//! itself (flife-studio's `tests/frame_device.rs`, FrankenManim
//! `d1e1757b`). Over 48 bare frames it was within the device terms less
//! the window's drawables (at most 84.9% of the largest so far), and over
//! 96 frames of the view as the Studio builds it (its overlay, with and
//! without the bloom) at most 64.4%. A bloom held no more than its charge.
//! The window's drawables are not measured.
//!
//! A session kept in a run directory (FL-024) also charges its checkpoint
//! ([`admit_kept`]): the most heap one holds beyond the live world, as the
//! command line's runs charge it.
//!
//! Live resizes and pixel-density changes are admitted through
//! [`admit_resize`] against the observer ceiling [`observer_limit`] before
//! mutating the view: any resize that would push the session past
//! `[execution] maximum_total_bytes` is refused and the view keeps its last
//! admitted drawable size and density (FL-043, INV-15).

use flife_backend_metal::ControllerChoice;
use flife_core::CoreError;
use flife_core::memory::{MemoryAdmission, total};
use flife_engine::World;
use flife_manim::{Viewport, WorldView};
use flife_render::Epoch;

/// What FrankenManim's Metal route and the window hold per drawable pixel:
/// the RGBA16F surface the route renders to (8 bytes), the RGBA8 surface
/// it converts to (4), and the window's three BGRA8 drawables (12). An
/// offscreen Studio holds no drawable but the last frame read back, tight
/// RGBA8 (4), dropped before the next is read: within the same charge.
pub const DEVICE_BYTES_PER_PIXEL: u64 = 8 + 4 + 3 * 4;

/// The edge of the tile the Metal route rasterizes, each with a status
/// word.
const TILE: u64 = 16;

/// Extra host bytes held by the y4m video writer for one even `size` frame
/// (FL-043, FL-050): one tight NV12 buffer (`3/2` bytes per pixel) plus one
/// serialized y4m `FRAME\n` record buffer (`6 + 3/2` bytes per pixel).
#[must_use]
pub fn video_scratch_bytes(size: [u32; 2]) -> u64 {
    let [width, height] = size.map(u64::from);
    let pixels = width.saturating_mul(height);
    let nv12 = pixels.saturating_mul(3) / 2;
    nv12.saturating_mul(2).saturating_add(6)
}

/// Host encoder working-set bytes held while exporting frames at `size`
/// (FL-043, FL-050, INV-15): one PNG encoder working set
/// (`flife_cli::encoding_bytes(size, 1)`) plus, when writing y4m video
/// (`with_video`), [`video_scratch_bytes(size)`].
#[must_use]
pub fn export_scratch_bytes(size: [u32; 2], with_video: bool) -> u64 {
    let png = flife_cli::encoding_bytes(size, 1);
    if with_video {
        png.saturating_add(video_scratch_bytes(size))
    } else {
        png
    }
}

/// Admit a Studio timeline export (FL-043, FL-050, INV-15) against
/// `[execution] maximum_total_bytes` before creating the export directory or
/// allocating the renderer or encoder buffers. Charges peak observer bytes
/// across the timeline's rendered geometry (or `target_resolution` when
/// overriding to e.g. 1080p or 4K Cinema) plus [`export_scratch_bytes`] at
/// the peak exported frame size.
///
/// # Errors
/// - [`CoreError::NumericalInvalid`] for an invalid drawable size or pixel
///   density;
/// - [`World::admit_memory`]'s [`CoreError::BudgetExhausted`] refusal when
///   the export's world, observer and encoder buffers exceed the budget.
pub fn admit_export(
    world: &World,
    restored: &Epoch,
    view: &WorldView,
    initial_size: [u32; 2],
    records: &[crate::TimelineRecord],
    target_resolution: Option<([u32; 2], f64)>,
    with_video: bool,
) -> Result<MemoryAdmission, CoreError> {
    let peak_observers = crate::timeline::peak_observer_bytes_for(
        world,
        restored,
        view,
        initial_size,
        records,
        target_resolution,
    )?;
    let peak_export_scratch = match target_resolution {
        Some((size, _)) => export_scratch_bytes(size, with_video),
        None => {
            let mut drawable = initial_size;
            let mut max_scratch = export_scratch_bytes(drawable, with_video);
            for record in records {
                if let crate::TimelineRecord::Resize { drawable: resized } = record {
                    drawable = *resized;
                    max_scratch = max_scratch.max(export_scratch_bytes(drawable, with_video));
                }
            }
            max_scratch
        }
    };
    world.admit_memory(false, 0, peak_observers.saturating_add(peak_export_scratch))
}

/// The Studio's observer bytes for `world` (see the [module
/// documentation](self)): its feed sized by `founding`, a standalone epoch
/// of the world, `view` and its frames at `drawable` pixels.
#[must_use]
pub fn observer_bytes(
    world: &World,
    founding: &Epoch,
    view: &WorldView,
    drawable: [u32; 2],
) -> u64 {
    let slots = u64::from(world.config().config().execution.render_queue_capacity);
    let feed_bytes = founding.reserved_bytes().saturating_mul(slots);
    observer_bytes_with(
        feed_bytes,
        view.reserved_bytes(),
        view.frame_bytes(),
        view.preparation_limit(),
        view.texel_bytes(),
        view.bloom_bytes(drawable),
        drawable,
    )
}

fn observer_bytes_with(
    feed_bytes: u64,
    reserved_bytes: u64,
    frame_bytes: u64,
    preparation_limit: u64,
    texel_bytes: u64,
    bloom_bytes: u64,
    drawable: [u32; 2],
) -> u64 {
    let [width, height] = drawable.map(u64::from);
    let tiles = width.div_ceil(TILE).saturating_mul(height.div_ceil(TILE));
    total([
        feed_bytes,
        reserved_bytes,
        frame_bytes,
        total([preparation_limit, texel_bytes]).saturating_mul(2),
        width
            .saturating_mul(height)
            .saturating_mul(DEVICE_BYTES_PER_PIXEL),
        tiles.saturating_mul(4),
        bloom_bytes,
    ])
}

/// The Studio's observer bytes if `view` were resized to `drawable` pixels at
/// `pixels_per_point` (FL-043): recomputes the density bins, texel upload,
/// device surfaces, tile status words and bloom scratch without mutating
/// `view`.
///
/// # Errors
/// [`CoreError::NumericalInvalid`] for an invalid drawable size or pixel
/// density.
pub fn observer_bytes_for_resize(
    feed_bytes: u64,
    view: &WorldView,
    drawable: [u32; 2],
    pixels_per_point: f64,
) -> Result<u64, CoreError> {
    let target: Viewport = view
        .viewport()
        .resized(drawable)?
        .with_pixels_per_point(pixels_per_point)?;
    Ok(observer_bytes_with(
        feed_bytes,
        view.reserved_bytes_for(&target),
        view.frame_bytes(),
        view.preparation_limit(),
        view.texel_bytes_for(&target),
        view.bloom_bytes(drawable),
        drawable,
    ))
}

/// The most observer bytes a live resize may hold without exceeding
/// `memory.budget_bytes`, given the fixed non-observer terms and the
/// controller backend grant already handed out (FL-043).
#[must_use]
pub fn observer_limit(memory: &MemoryAdmission) -> u64 {
    let non_render = memory.fixed_bytes().saturating_sub(memory.render);
    memory
        .budget_bytes
        .saturating_sub(non_render)
        .saturating_sub(memory.backend)
}

/// Admit a live resize to `drawable` pixels at `pixels_per_point` under
/// `memory`'s budget ceiling (FL-043, INV-15). Returns the new observer byte
/// charge on success.
///
/// # Errors
/// - [`CoreError::NumericalInvalid`] for an invalid drawable size or pixel
///   density;
/// - [`CoreError::BudgetExhausted`] when the resized observer terms would
///   exceed [`observer_limit`].
pub fn admit_resize(
    memory: &MemoryAdmission,
    feed_bytes: u64,
    view: &WorldView,
    drawable: [u32; 2],
    pixels_per_point: f64,
) -> Result<u64, CoreError> {
    let required = observer_bytes_for_resize(feed_bytes, view, drawable, pixels_per_point)?;
    let available = observer_limit(memory);
    if required > available {
        return Err(CoreError::BudgetExhausted {
            budget: "peak memory on resize (maximum_total_bytes)",
            required,
            available,
        });
    }
    Ok(required)
}

/// The Studio's observer bytes for `world` at `drawable` pixels and
/// `pixels_per_point` density (FL-043): recomputes density bins and texel
/// uploads for the actual surface geometry without mutating `view`.
///
/// # Errors
/// [`CoreError::NumericalInvalid`] for an invalid drawable size or pixel
/// density.
pub fn observer_bytes_at(
    world: &World,
    founding: &Epoch,
    view: &WorldView,
    drawable: [u32; 2],
    pixels_per_point: f64,
) -> Result<u64, CoreError> {
    let slots = u64::from(world.config().config().execution.render_queue_capacity);
    let feed_bytes = founding.reserved_bytes().saturating_mul(slots);
    observer_bytes_for_resize(feed_bytes, view, drawable, pixels_per_point)
}

/// Whether `controllers` reserves the batched controller path (FL-043,
/// V2-12): both dense and recurrent worlds use batched Torch/Metal under
/// `Auto`, `Torch`, and `Metal`; only explicit `Cpu` runs on the reference
/// path without a batched budget.
const fn uses_batched_controllers(controllers: ControllerChoice) -> bool {
    match controllers {
        ControllerChoice::Cpu => false,
        ControllerChoice::Auto | ControllerChoice::Torch | ControllerChoice::Metal => true,
    }
}

/// Admit `world` with `controllers`' backend and the Studio's observers
/// ([`observer_bytes`]). Call it after the world's workers are set and
/// before its backend is installed, and give the backend
/// [`MemoryAdmission::backend`] bytes.
///
/// # Errors
/// [`World::admit_memory`]'s refusal.
pub fn admit(
    world: &World,
    controllers: ControllerChoice,
    founding: &Epoch,
    view: &WorldView,
    drawable: [u32; 2],
) -> Result<MemoryAdmission, CoreError> {
    admit_kept(world, controllers, founding, view, drawable, 0)
}

/// [`admit`] with a kept session's `checkpoint` bytes (FL-024): the
/// checkpoint term of [`World::admit_memory`].
///
/// # Errors
/// [`World::admit_memory`]'s refusal.
pub fn admit_kept(
    world: &World,
    controllers: ControllerChoice,
    founding: &Epoch,
    view: &WorldView,
    drawable: [u32; 2],
    checkpoint: u64,
) -> Result<MemoryAdmission, CoreError> {
    world.admit_memory(
        uses_batched_controllers(controllers),
        checkpoint,
        observer_bytes(world, founding, view, drawable),
    )
}

/// [`admit_kept`] charging observers for the surface's opening `(drawable,
/// pixels_per_point)` geometry so HiDPI, fractional-scale, or OS-resized
/// opening surfaces match frame-1 [`admit_resize`] admission exactly (FL-043).
///
/// # Errors
/// [`CoreError::NumericalInvalid`] for an invalid drawable size or pixel
/// density; [`World::admit_memory`]'s refusal.
pub fn admit_kept_at(
    world: &World,
    controllers: ControllerChoice,
    founding: &Epoch,
    view: &WorldView,
    drawable: [u32; 2],
    pixels_per_point: f64,
    checkpoint: u64,
) -> Result<MemoryAdmission, CoreError> {
    let observers = observer_bytes_at(world, founding, view, drawable, pixels_per_point)?;
    world.admit_memory(uses_batched_controllers(controllers), checkpoint, observers)
}
