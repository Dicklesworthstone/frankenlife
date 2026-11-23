//! FL-032: phase 10's field check on Metal (plan §4.2, §8.1, §11.1): every
//! cell's INV-01 and INV-02 check, bit-identical to
//! [`FieldState::scan_invariants`]. [`MetalField`](crate::MetalField) runs it
//! as its [`FieldBackend::check`](flife_field::FieldBackend::check).
//!
//! The device checks each cell in `check.metal` on `softfloat64.metal`'s
//! binary64 (its moments' squares in channel order, the square root, the
//! residual and the tolerance's bound) and reduces each threadgroup to its
//! first failing cell and the first cell with its largest positive residual.
//! The host merges the threadgroups in cell order; the world then builds the
//! summary from the scan ([`FieldState::summarize_scan`]), summing the energy
//! in cell order and recomputing the largest residual's cell. One check is
//! one upload of the field and one command.

use flife_core::CoreError;
use flife_field::grid::{DUAL_CARRIER_PLANES, FIELD_PLANES};
use flife_field::{ConeTolerance, FieldScan, FieldState};
use ft_kernel_metal::compute::{Grid, MathMode, Pipeline};

use crate::owner::{DeviceOwner, Owned, Purpose, metal};

/// The phase 10 kernel source, with the software binary64 ahead of it.
pub const CHECK_MSL: &str = concat!(
    include_str!("softfloat64.metal"),
    include_str!("check.metal")
);

/// Threads per threadgroup: `check.metal`'s `CHECK_THREADS`.
const THREADS: usize = 256;

/// Words of `field_check`'s parameter buffer: `[cells, abs(2), rel(2), dual]`.
const PARAMS: usize = 6;

/// Words per threadgroup's result.
const GROUP: usize = 4;

/// No cell.
const NO_CELL: u32 = u32::MAX;

fn overflow() -> CoreError {
    CoreError::Overflow {
        quantity: "metal field check size",
    }
}

fn words(value: f64) -> [u32; 2] {
    let bits = value.to_bits();
    [(bits & 0xFFFF_FFFF) as u32, (bits >> 32) as u32]
}

/// The device buffers for one grid and plane count (`33` single-carrier or
/// `34` dual-carrier).
#[derive(Debug)]
struct Buffers {
    cells: usize,
    planes: usize,
    field: Owned<f32>,
    params: Owned<u32>,
    groups: Owned<u32>,
}

/// Phase 10's kernel and its buffers on one owner's device.
pub(crate) struct Check {
    pipeline: Pipeline,
    buffers: Option<Buffers>,
    words: Vec<u32>,
}

impl std::fmt::Debug for Check {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Check")
            .field("buffers", &self.buffers)
            .finish_non_exhaustive()
    }
}

impl Check {
    /// The kernel compiled on `owner`'s device.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] if it does not compile.
    pub(crate) fn new(owner: &DeviceOwner) -> Result<Self, CoreError> {
        let library = owner.library(CHECK_MSL, MathMode::Safe)?;
        Ok(Self {
            pipeline: library.pipeline("field_check").map_err(metal)?,
            buffers: None,
            words: Vec::new(),
        })
    }

    /// Buffers for `cells` cells and `planes` planes (`33` or `34`), allocated
    /// on first use and again when either changes.
    fn ensure_buffers(
        &mut self,
        owner: &DeviceOwner,
        cells: usize,
        planes: usize,
    ) -> Result<(), CoreError> {
        if self
            .buffers
            .as_ref()
            .is_some_and(|buffers| buffers.cells == cells && buffers.planes == planes)
        {
            return Ok(());
        }
        self.buffers = None;
        let values = cells.checked_mul(planes).ok_or_else(overflow)?;
        let groups = cells.div_ceil(THREADS).max(1);
        self.buffers = Some(Buffers {
            cells,
            planes,
            field: owner.zeroed_owned(values)?,
            params: owner.zeroed_owned(PARAMS)?,
            groups: owner.zeroed_owned(groups.checked_mul(GROUP).ok_or_else(overflow)?)?,
        });
        Ok(())
    }

    /// [`FieldState::scan_invariants`] on the device (both 33-plane single-carrier
    /// and 34-plane dual-carrier `V2-15` independent Lorentz cones); see the
    /// module documentation.
    ///
    /// # Errors
    /// [`CoreError::MissingCapability`] for a tolerance whose bound could
    /// overflow binary64 at the largest F32 energy (the device's software
    /// binary64 carries no infinity); [`CoreError::LayoutMismatch`] for a
    /// field whose planes are unsupported or not whole; the device's errors;
    /// a cell outside the field in the device's answer.
    pub(crate) fn run(
        &mut self,
        owner: &DeviceOwner,
        field: &FieldState,
        tolerance: ConeTolerance,
    ) -> Result<FieldScan, CoreError> {
        let bound = tolerance.absolute() + tolerance.relative() * f64::from(f32::MAX);
        if !bound.is_finite() {
            return Err(CoreError::MissingCapability {
                capability: format!(
                    "metal field check of a cone tolerance bound of {bound:e} at the largest energy"
                ),
            });
        }
        let plane_count = field.planes();
        if plane_count != FIELD_PLANES && plane_count != DUAL_CARRIER_PLANES {
            return Err(CoreError::LayoutMismatch {
                buffer: "metal field check",
                detail: format!(
                    "unsupported plane count {plane_count} (expected {FIELD_PLANES} or {DUAL_CARRIER_PLANES})"
                ),
            });
        }
        let planes = usize::try_from(plane_count).map_err(|_| overflow())?;
        let cells = usize::try_from(field.grid().cell_count()).map_err(|_| overflow())?;
        let values = field.as_slice();
        if Some(values.len()) != cells.checked_mul(planes) {
            return Err(CoreError::LayoutMismatch {
                buffer: "metal field check",
                detail: format!(
                    "{} values for {cells} cells across {planes} planes",
                    values.len()
                ),
            });
        }
        if cells == 0 {
            return Ok(FieldScan {
                offending: None,
                largest: None,
            });
        }
        let cell_words = u32::try_from(cells).map_err(|_| overflow())?;
        self.ensure_buffers(owner, cells, planes)?;
        let buffers = self.buffers.as_ref().ok_or_else(overflow)?;
        let groups = cells.div_ceil(THREADS);
        owner.write(buffers.field.buffer())?.write(0, values)?;
        let [absolute_low, absolute_high] = words(tolerance.absolute());
        let [relative_low, relative_high] = words(tolerance.relative());
        owner.write(buffers.params.buffer())?.write(
            0,
            &[
                cell_words,
                absolute_low,
                absolute_high,
                relative_low,
                relative_high,
                u32::from(field.is_dual_carrier()),
            ],
        )?;
        owner.submit(
            &self.pipeline,
            &[
                buffers.field.buffer().reads(),
                buffers.params.buffer().reads(),
                buffers.groups.buffer().writes(),
            ],
            Grid::linear(groups, THREADS),
        )?;
        self.words.clear();
        self.words.resize(groups * GROUP, 0);
        owner
            .read(buffers.groups.buffer(), Purpose::Readback)?
            .read(&mut self.words)?;

        // The threadgroups in cell order: the first failing cell, else the
        // first cell with the largest residual (a later group's replaces an
        // earlier one's only when strictly larger).
        let mut offending = None;
        let mut largest: Option<(usize, u64)> = None;
        for group in self.words.as_chunks::<GROUP>().0 {
            let [fails, low, high, at] = *group;
            if fails != NO_CELL && offending.is_none() {
                offending = Some(fails as usize);
            }
            let bits = u64::from(low) | (u64::from(high) << 32);
            if at != NO_CELL && largest.is_none_or(|(_, best)| bits > best) {
                largest = Some((at as usize, bits));
            }
        }
        for at in offending.iter().chain(largest.iter().map(|(at, _)| at)) {
            if *at >= cells {
                return Err(CoreError::StateConflict {
                    operation: "metal field check",
                    detail: format!("cell {at} of {cells}"),
                });
            }
        }
        Ok(match offending {
            Some(_) => FieldScan {
                offending,
                largest: None,
            },
            None => FieldScan {
                offending: None,
                largest: largest.map(|(at, bits)| (at, f64::from_bits(bits))),
            },
        })
    }
}
