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
//!   rows and bins on the host and copies them to the device, each charged
//!   at the view's [`WorldView::preparation_limit`]. FrankenManim bounds
//!   neither; on 48 frames, up to 16,384 organisms at 4K, two pixels per
//!   point and a 16x zoom, the host's held at most 71% of that limit;
//! - per drawable pixel, [`DEVICE_BYTES_PER_PIXEL`]: the route's RGBA16F
//!   and RGBA8 surfaces and the window's drawables; and a status word per
//!   16 x 16 tile.
//!
//! What a resize past the opening size adds, in pixels and density bins,
//! is not admitted again.

use flife_backend_metal::ControllerChoice;
use flife_core::CoreError;
use flife_core::memory::{MemoryAdmission, total};
use flife_engine::World;
use flife_manim::WorldView;
use flife_render::Epoch;

/// What FrankenManim's Metal route and the window hold per drawable pixel:
/// the RGBA16F surface the route renders to (8 bytes), the RGBA8 surface
/// it converts to (4), and the window's three BGRA8 drawables (12).
pub const DEVICE_BYTES_PER_PIXEL: u64 = 8 + 4 + 3 * 4;

/// The edge of the tile the Metal route rasterizes, each with a status
/// word.
const TILE: u64 = 16;

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
    let [width, height] = drawable.map(u64::from);
    let tiles = width.div_ceil(TILE).saturating_mul(height.div_ceil(TILE));
    total([
        founding.reserved_bytes().saturating_mul(slots),
        view.reserved_bytes(),
        view.frame_bytes(),
        view.preparation_limit().saturating_mul(2),
        width
            .saturating_mul(height)
            .saturating_mul(DEVICE_BYTES_PER_PIXEL),
        tiles.saturating_mul(4),
    ])
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
    world.admit_memory(
        controllers != ControllerChoice::Cpu,
        0,
        observer_bytes(world, founding, view, drawable),
    )
}
