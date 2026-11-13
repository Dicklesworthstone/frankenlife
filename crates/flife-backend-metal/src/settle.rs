//! FL-032 (`flife-kf8`): phase 6 on Metal for the engine. [`MetalSettle`]
//! serves flife-field's `SettleBackend` with [`MetalChemistry`],
//! [`MetalMotion`] and [`MetalEmission`] on one device owner, so a world
//! installs it with `World::set_settle_backend`. Each method is one command
//! (three a tick), and each gives the reference's values bit for bit or
//! refuses, handing the tick's phase 6 to the reference.

use flife_core::CoreError;
use flife_field::settle::{
    ChemistryConstants, ChemistryOrganism, ChemistryOutcome, ChemistryPacket, EmissionOrganism,
    EmissionOutcome, MotionConstants, MotionOrganism, MotionOutcome, SettleBackend,
};
use flife_field::{FieldGrid, FieldState};

use crate::chemistry::MetalChemistry;
use crate::emission::MetalEmission;
use crate::motion::MetalMotion;
use crate::owner::{DeviceOwner, Limits};

/// Phase 6's three kernels on one device; see the module documentation.
#[derive(Debug)]
pub struct MetalSettle {
    chemistry: MetalChemistry,
    motion: MetalMotion,
    emission: MetalEmission,
}

impl MetalSettle {
    /// Open the default Metal device and compile phase 6's kernels.
    ///
    /// # Errors
    /// [`CoreError::MissingCapability`] without a Metal device (always, off
    /// macOS); [`CoreError::StateConflict`] if the kernels do not compile.
    pub fn open() -> Result<Self, CoreError> {
        Self::with_owner(DeviceOwner::open(Limits::UNBOUNDED)?)
    }

    /// Phase 6's kernels on `owner`'s device; their buffers are `owner`'s.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] if the kernels do not compile.
    pub fn with_owner(owner: DeviceOwner) -> Result<Self, CoreError> {
        Ok(Self {
            chemistry: MetalChemistry::with_owner(owner.clone())?,
            motion: MetalMotion::with_owner(owner.clone())?,
            emission: MetalEmission::with_owner(owner)?,
        })
    }

    /// The device's name.
    #[must_use]
    pub fn device_name(&self) -> String {
        self.chemistry.device_name()
    }

    /// The owner of every buffer these kernels use.
    #[must_use]
    pub const fn owner(&self) -> &DeviceOwner {
        self.chemistry.owner()
    }
}

impl SettleBackend for MetalSettle {
    fn describe(&self) -> String {
        format!("metal ({})", self.device_name())
    }

    fn chemistry(
        &mut self,
        observed: &FieldState,
        constants: &ChemistryConstants,
        organisms: &[ChemistryOrganism],
        packets: &[ChemistryPacket],
        out: &mut Vec<ChemistryOutcome>,
    ) -> Result<(), CoreError> {
        self.chemistry
            .settle(observed, constants, organisms, packets, out)
    }

    fn motion(
        &mut self,
        constants: &MotionConstants,
        organisms: &[MotionOrganism],
        out: &mut Vec<MotionOutcome>,
    ) -> Result<(), CoreError> {
        self.motion.settle(constants, organisms, out)
    }

    fn emission(
        &mut self,
        grid: &FieldGrid,
        efficiency: f64,
        organisms: &[EmissionOrganism],
        out: &mut Vec<EmissionOutcome>,
    ) -> Result<(), CoreError> {
        self.emission.settle(grid, efficiency, organisms, out)
    }
}
