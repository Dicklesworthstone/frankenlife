//! Field byte counts and per-buffer limits (plan §19.3, §19.4, INV-15).
//!
//! Every count is checked `u64` arithmetic on the real F32 element size. A
//! buffer is admitted only if it fits the per-buffer limit and Rust's
//! largest allocation (`isize::MAX` bytes). Whole-run peak-memory admission,
//! which adds genomes, scratch, render, births and checkpoints, belongs to
//! FL-043; it consumes these counts.

use flife_core::CoreError;
use flife_core::model::MOMENT_CHANNELS;

use crate::grid::{FIELD_PLANES, FieldGrid};
use crate::layout::HOST_ALIGNMENT;

/// The largest single buffer a backend can allocate or bind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferLimits {
    max_buffer_bytes: u64,
}

impl BufferLimits {
    /// A limit of `max_buffer_bytes`, for example a device's maximum buffer
    /// length. Rust's own `isize::MAX` limit still applies.
    #[must_use]
    pub const fn new(max_buffer_bytes: u64) -> Self {
        Self { max_buffer_bytes }
    }

    /// The host limit: Rust's largest allocation, `isize::MAX` bytes.
    #[must_use]
    pub fn host() -> Self {
        Self::new(rust_allocation_limit())
    }

    /// The per-buffer limit in bytes.
    #[must_use]
    pub const fn max_buffer_bytes(&self) -> u64 {
        self.max_buffer_bytes
    }

    /// Admit one buffer of `bytes`.
    ///
    /// # Errors
    /// [`CoreError::BudgetExhausted`] above the limit or `isize::MAX`.
    pub fn admit(&self, buffer: &'static str, bytes: u64) -> Result<(), CoreError> {
        let limit = self.max_buffer_bytes.min(rust_allocation_limit());
        if bytes > limit {
            return Err(CoreError::BudgetExhausted {
                budget: buffer,
                required: bytes,
                available: limit,
            });
        }
        Ok(())
    }
}

fn rust_allocation_limit() -> u64 {
    u64::try_from(isize::MAX).unwrap_or(u64::MAX)
}

/// The bytes a grid's field occupies in real F32 storage (§19.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldFootprint {
    /// Cells in the grid.
    pub cells: u64,
    /// One scalar energy plane.
    pub energy_plane_bytes: u64,
    /// The 32 signed moment planes.
    pub moment_planes_bytes: u64,
    /// One complete field state: energy plus moments.
    pub state_bytes: u64,
    /// The current and next field states.
    pub pair_bytes: u64,
}

impl FieldFootprint {
    /// The footprint of `grid`.
    ///
    /// # Errors
    /// [`CoreError::Overflow`] past `u64::MAX`.
    pub fn of(grid: &FieldGrid) -> Result<Self, CoreError> {
        let overflow = || CoreError::Overflow {
            quantity: "field footprint bytes",
        };
        let cells = grid.cell_count();
        let energy_plane_bytes = cells.checked_mul(HOST_ALIGNMENT).ok_or_else(overflow)?;
        let moment_planes_bytes = energy_plane_bytes
            .checked_mul(u64::from(MOMENT_CHANNELS))
            .ok_or_else(overflow)?;
        let state_bytes = energy_plane_bytes
            .checked_mul(u64::from(FIELD_PLANES))
            .ok_or_else(overflow)?;
        let pair_bytes = state_bytes.checked_mul(2).ok_or_else(overflow)?;
        Ok(Self {
            cells,
            energy_plane_bytes,
            moment_planes_bytes,
            state_bytes,
            pair_bytes,
        })
    }

    /// Admit one complete field state as a single buffer.
    ///
    /// # Errors
    /// [`CoreError::BudgetExhausted`] when it does not fit `limits`.
    pub fn admit_state(&self, limits: &BufferLimits) -> Result<(), CoreError> {
        limits.admit("field buffer bytes", self.state_bytes)
    }
}
