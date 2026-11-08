//! Units named in schemas (plan §4.3).
//!
//! Every numeric configuration key has exactly one unit, recorded in the
//! model descriptor. Simulation seconds and display seconds are different
//! units. Total source power (energy per second) and a density rate are not
//! interchangeable.

use std::fmt;

/// The unit of a numeric configuration quantity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Unit {
    /// A pure ratio, gain, norm or exponent.
    Dimensionless,
    /// A probability or threshold on a probability-valued output.
    Probability,
    /// A count of things (cells, organisms, channels, slots, workers).
    Count,
    /// A whole number of scientific ticks.
    Ticks,
    /// One part of the rational simulation-seconds-per-tick.
    TickRationalPart,
    /// Bytes.
    Bytes,
    /// A seed or other identifier with no physical meaning.
    Identifier,
    /// World length.
    WorldLength,
    /// World length per simulation-second.
    WorldLengthPerSecond,
    /// World length per simulation-second squared.
    WorldLengthPerSecondSquared,
    /// Diffusion: world length squared per simulation-second.
    Diffusion,
    /// Simulation seconds.
    SimulationSeconds,
    /// A rate: per simulation-second.
    PerSimulationSecond,
    /// Ecological energy (the model's accounting unit).
    Energy,
    /// Ecological energy per simulation-second.
    EnergyPerSecond,
    /// Ecological energy per world-length squared.
    EnergyDensity,
    /// Ecological energy per controller parameter evaluated.
    EnergyPerParameterEvaluation,
    /// The motion-cost coefficient: energy × simulation-second / length².
    EnergySecondsPerLengthSquared,
    /// Display pixels.
    DisplayPixels,
    /// Display frames per display second.
    DisplayFramesPerSecond,
}

impl Unit {
    /// The canonical unit name used in the model descriptor.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Dimensionless => "dimensionless",
            Self::Probability => "probability",
            Self::Count => "count",
            Self::Ticks => "simulation-ticks",
            Self::TickRationalPart => "simulation-seconds-per-tick-rational-part",
            Self::Bytes => "bytes",
            Self::Identifier => "identifier",
            Self::WorldLength => "world-length",
            Self::WorldLengthPerSecond => "world-length/simulation-second",
            Self::WorldLengthPerSecondSquared => "world-length/simulation-second^2",
            Self::Diffusion => "world-length^2/simulation-second",
            Self::SimulationSeconds => "simulation-seconds",
            Self::PerSimulationSecond => "1/simulation-second",
            Self::Energy => "ecological-energy",
            Self::EnergyPerSecond => "ecological-energy/simulation-second",
            Self::EnergyDensity => "ecological-energy/world-length^2",
            Self::EnergyPerParameterEvaluation => "ecological-energy/parameter-evaluation",
            Self::EnergySecondsPerLengthSquared => {
                "ecological-energy*simulation-second/world-length^2"
            }
            Self::DisplayPixels => "display-pixels",
            Self::DisplayFramesPerSecond => "display-frames/display-second",
        }
    }
}

impl fmt::Display for Unit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}
