//! Standard normals from semantic Box–Muller pairs, shared by founders and
//! mutation.

use flife_core::CoreError;
use flife_core::rng::{Address, Lane, LaneKind, SemanticRng, box_muller};

/// Fill `values` with standard normals. Pair `j` is word 0 and word 1 of
/// the block at `at(lane (candidate, kind, j))`, and gives components `2j`
/// and `2j + 1`; an odd length drops the last sine component.
pub(crate) fn normals(
    rng: &SemanticRng,
    at: impl Fn(Lane) -> Address,
    candidate: u8,
    kind: LaneKind,
    values: &mut [f64],
) -> Result<(), CoreError> {
    for (pair, slots) in (0_u64..).zip(values.chunks_mut(2)) {
        let [first, second, ..] = rng.block(at(Lane::new(candidate, kind, pair)?));
        for (slot, normal) in slots.iter_mut().zip(box_muller(first, second)?) {
            *slot = normal;
        }
    }
    Ok(())
}
