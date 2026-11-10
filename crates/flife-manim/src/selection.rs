//! Picking an organism under the pointer (plan §13.6, FL-020).
//!
//! A pick reads what is drawn: each organism's stable identity and the
//! position it is drawn at (the epoch's own, or an interpolation's), placed
//! through the [`Viewport`]. It returns an identity, never a storage slot,
//! so a host validates it against the authoritative world before showing
//! anything; an organism that has died since is gone, whatever now occupies
//! its old slot.

use flife_core::ids::OrganismId;

use crate::Viewport;

/// The default pick radius in drawable pixels.
pub const PICK_RADIUS_PIXELS: f64 = 8.0;

/// The organism drawn nearest the drawable position `at`, within
/// `radius_pixels` of it, from `drawn` (identity, world position) pairs:
/// the nearest by drawable distance, the lowest identity among equals.
/// `None` when nothing is drawn that close, or for a non-finite position or
/// radius.
#[must_use]
pub fn pick(
    viewport: &Viewport,
    drawn: impl IntoIterator<Item = (u64, [f64; 2])>,
    at: [f64; 2],
    radius_pixels: f64,
) -> Option<OrganismId> {
    if !(at.iter().all(|value| value.is_finite()) && radius_pixels.is_finite()) {
        return None;
    }
    let mut best: Option<(f64, u64)> = None;
    let reach = radius_pixels * radius_pixels;
    for (id, position) in drawn {
        let place = viewport.world_to_drawable(position);
        let (dx, dy) = (place[0] - at[0], place[1] - at[1]);
        let distance = dx * dx + dy * dy;
        if distance > reach {
            continue;
        }
        let better = match best {
            None => true,
            Some((nearest, holder)) => distance < nearest || (distance == nearest && id < holder),
        };
        if better {
            best = Some((distance, id));
        }
    }
    best.map(|(_, id)| OrganismId::new(id))
}
