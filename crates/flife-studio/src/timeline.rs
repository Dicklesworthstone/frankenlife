//! FL-023: the Studio's presentation timeline (plan §13.8's live capture),
//! and its replay.
//!
//! A recording Studio ([`crate::StudioOptions::record`]) notes, as
//! [`TimelineRecord`]s in the order it makes them, every change a frame
//! makes to its world view: its drawable and density, the overlay, the
//! style, the camera, the selection, the brush's region outlined (FL-022),
//! and the epochs it shows with how far the display is between them. It also notes every frame it draws, with
//! the tick shown and the wall time. The commands are noted as the session
//! accepts and applies them ([`TimelineRecord::status`]). The timeline
//! closes with [`TimelineRecord::end`]: the world the session closed on and
//! every command its book applied, with its payload and tick.
//!
//! [`replay`] founds the configuration's world again and steps it with those
//! commands at those ticks. A resumed Studio's timeline (FL-024's
//! `--resume`) names where its session started ([`TimelineStart`]): the
//! checkpoint of its run directory it restored, which the Studio pinned so
//! retention keeps it, and the tick its session opened at once the command
//! journal replayed past it. Its replay restores that checkpoint instead
//! ([`flife_cli::restore_checkpoint`]) and steps it there with the commands
//! the book applied. It makes the same changes to a fresh view in the
//! same order and draws each frame the Studio drew through the same Metal
//! route ([`MetalRenderer::render_three_d_rgba8`]). Every number a frame
//! depends on is written as its bits ([`Exact`]). So the frames a replay
//! draws are the Studio's, byte for byte, at the ticks the Studio showed
//! them, and the replay closes on the world the Studio closed on, or refuses.
//! A frame drawn in a window is replayed as it was rendered, before the
//! window composited it.

use std::collections::BTreeMap;
use std::num::{NonZeroU64, NonZeroUsize};
use std::path::Path;

use flife_cli::CameraPath;
use flife_core::ids::OrganismId;
use flife_core::memory::MemoryAdmission;
use flife_core::{CoreError, Tick, V2Feature, ValidatedConfig};
use flife_engine::{Disposition, Intervention, MATERIAL_CHANNELS, Stepped, World};
use flife_manim::{
    ColorMapping, FieldMapping, HudFont, Look, Particles, Region, RegionEffect, RegionShape, Style,
    hex_rgb,
};
use flife_render::{AffinityProjection, Lease, Publication, Publisher, Viewer};
use flife_runtime::{CommandStatus, Outcome, SessionClosed};
use fmn_frame::FrameBuffer;
use fmn_render::metal::MetalRenderer;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::Status;

/// The schema of a Studio timeline's records.
pub const TIMELINE_SCHEMA: &str = "frankenlife.timeline.v1";

/// The render-epoch slots [`replay`] allocates for its publisher: a prior, a
/// current, one being written, and one to spare.
pub const REPLAY_SLOTS: usize = 4;

/// A number written as its IEEE 754 bits, in 16 hexadecimal digits: read
/// back, it is exactly the number written.
#[derive(Debug, Clone, Copy)]
pub struct Exact(pub f64);

impl PartialEq for Exact {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}

impl Eq for Exact {}

impl Serialize for Exact {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!("{:016x}", self.0.to_bits()))
    }
}

impl<'de> Deserialize<'de> for Exact {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        if text.len() != 16 {
            return Err(D::Error::custom(format!(
                "`{text}` is not a number's 16 hexadecimal digits"
            )));
        }
        u64::from_str_radix(&text, 16)
            .map(|bits| Self(f64::from_bits(bits)))
            .map_err(D::Error::custom)
    }
}

/// The fixed projection the affinity view and the field's composition take
/// ([`AffinityProjection`], plan §13.4's "recorded matrix and scaling"), as
/// a timeline records it: what the colours of the frames it holds meant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineProjection {
    /// The rows of the 32 × 32 Sylvester Hadamard matrix taken.
    pub rows: [usize; 3],
    /// The matrix, a row per display axis and a column per channel, each
    /// `1` or `−1`.
    pub matrix: [[i8; 32]; 3],
    /// What each sum is divided by besides its unit (the affinity norm
    /// limit, or a block's energy): `√32`.
    pub divisor: Exact,
}

impl From<AffinityProjection> for TimelineProjection {
    fn from(projection: AffinityProjection) -> Self {
        Self {
            rows: projection.rows,
            matrix: projection.matrix,
            divisor: Exact(projection.divisor),
        }
    }
}

/// A [`Style`] by its names, as flags and records spell them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineStyle {
    /// The colour mapping.
    pub mapping: String,
    /// `glow`, `raw` or `sphere`.
    pub look: String,
    /// `points` or `density`.
    pub particles: String,
    /// Whether the field is drawn.
    pub field: bool,
    /// What the field's colour means: `energy`, `composition` or `component-C`
    /// (`0` to `31`). A record from before FL-019's composition mapping has
    /// none, and means `energy`.
    #[serde(default = "energy_field")]
    pub field_mapping: String,
}

/// The field mapping a record without one means.
fn energy_field() -> String {
    FieldMapping::Energy.name().to_owned()
}

impl From<Style> for TimelineStyle {
    fn from(style: Style) -> Self {
        Self {
            mapping: style.mapping.name().to_owned(),
            look: style.look.name().to_owned(),
            particles: style.particles.name().to_owned(),
            field: style.field,
            field_mapping: style.field_mapping.name().to_owned(),
        }
    }
}

impl TimelineStyle {
    /// The style these names spell.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a name that is not one.
    pub fn style(&self) -> Result<Style, CoreError> {
        let invalid = |detail: String| CoreError::NumericalInvalid {
            quantity: "timeline style",
            detail,
        };
        let particles = match self.particles.as_str() {
            "points" => Particles::Points,
            "density" => Particles::Density,
            other => return Err(invalid(format!("`{other}` is not points or density"))),
        };
        Ok(Style {
            mapping: self.mapping.parse::<ColorMapping>().map_err(invalid)?,
            look: self.look.parse::<Look>().map_err(invalid)?,
            particles,
            field: self.field,
            field_mapping: self
                .field_mapping
                .parse::<FieldMapping>()
                .map_err(invalid)?,
        })
    }
}

/// An [`Intervention`], every number exact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum TimelineIntervention {
    /// [`Intervention::InjectEnergy`].
    InjectEnergy {
        /// The cell.
        cell: [u32; 2],
        /// The energy.
        amount: Exact,
    },
    /// [`Intervention::InjectMaterial`].
    InjectMaterial {
        /// The cell.
        cell: [u32; 2],
        /// The energy.
        amount: Exact,
        /// The composition, [`MATERIAL_CHANNELS`] components.
        direction: Vec<Exact>,
    },
    /// [`Intervention::RemoveOrganisms`].
    RemoveOrganisms {
        /// The centre.
        center: [Exact; 2],
        /// The radius.
        radius: Exact,
        /// `remove`, `dissipate` or `recycle`.
        disposition: String,
    },
    /// [`Intervention::IntroduceOrganisms`].
    IntroduceOrganisms {
        /// The cell.
        cell: [u32; 2],
        /// How many.
        count: u32,
        /// Each one's endowment.
        energy: Exact,
    },
}

impl From<&Intervention> for TimelineIntervention {
    fn from(intervention: &Intervention) -> Self {
        match *intervention {
            Intervention::InjectEnergy { cell, amount } => Self::InjectEnergy {
                cell,
                amount: Exact(amount),
            },
            Intervention::InjectMaterial {
                cell,
                amount,
                ref direction,
            } => Self::InjectMaterial {
                cell,
                amount: Exact(amount),
                direction: direction.iter().copied().map(Exact).collect(),
            },
            Intervention::RemoveOrganisms {
                center,
                radius,
                disposition,
            } => Self::RemoveOrganisms {
                center: center.map(Exact),
                radius: Exact(radius),
                disposition: match disposition {
                    Disposition::Remove => "remove",
                    Disposition::Dissipate => "dissipate",
                    Disposition::Recycle => "recycle",
                }
                .to_owned(),
            },
            Intervention::IntroduceOrganisms {
                cell,
                count,
                energy,
            } => Self::IntroduceOrganisms {
                cell,
                count,
                energy: Exact(energy),
            },
        }
    }
}

impl TimelineIntervention {
    /// The intervention, exactly as written.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a composition of another length
    /// or a disposition that is not one.
    pub fn intervention(&self) -> Result<Intervention, CoreError> {
        let invalid = |detail: String| CoreError::NumericalInvalid {
            quantity: "timeline intervention",
            detail,
        };
        Ok(match self {
            Self::InjectEnergy { cell, amount } => Intervention::InjectEnergy {
                cell: *cell,
                amount: amount.0,
            },
            Self::InjectMaterial {
                cell,
                amount,
                direction,
            } => {
                let components: Vec<f64> = direction.iter().map(|component| component.0).collect();
                let direction: [f64; MATERIAL_CHANNELS] =
                    components.try_into().map_err(|components: Vec<f64>| {
                        invalid(format!(
                            "a composition of {} components, not {MATERIAL_CHANNELS}",
                            components.len()
                        ))
                    })?;
                Intervention::InjectMaterial {
                    cell: *cell,
                    amount: amount.0,
                    direction,
                }
            }
            Self::RemoveOrganisms {
                center,
                radius,
                disposition,
            } => Intervention::RemoveOrganisms {
                center: center.map(|coordinate| coordinate.0),
                radius: radius.0,
                disposition: match disposition.as_str() {
                    "remove" => Disposition::Remove,
                    "dissipate" => Disposition::Dissipate,
                    "recycle" => Disposition::Recycle,
                    other => return Err(invalid(format!("`{other}` is not a disposition"))),
                },
            },
            Self::IntroduceOrganisms {
                cell,
                count,
                energy,
            } => Intervention::IntroduceOrganisms {
                cell: *cell,
                count: *count,
                energy: energy.0,
            },
        })
    }
}

/// A command the session's book applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedCommand {
    /// Its identity.
    pub command: u64,
    /// The committed tick whose boundary it applied at.
    pub tick: u64,
    /// The tick that publication committed.
    pub published: u64,
    /// The energy it moved.
    pub energy: Exact,
    /// What it did.
    pub intervention: TimelineIntervention,
}

/// Where a resumed Studio's session started (FL-024's `--resume`): its run
/// directory, the checkpoint it restored, and the tick it opened at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineStart {
    /// The run directory, as an absolute path.
    pub run_dir: String,
    /// The checkpoint restored; none for the founding world, which a run
    /// without a checkpoint that restores resumes from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<TimelineCheckpoint>,
    /// The committed tick the session opened at: the checkpoint's (or the
    /// founding's), or later once the command journal replayed past it.
    pub tick: u64,
}

/// The checkpoint a resumed Studio restored, which its timeline pins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineCheckpoint {
    /// Its chunk-store sequence number.
    pub sequence: u64,
    /// The committed tick it holds.
    pub tick: u64,
    /// The holder of the pin that keeps it from retention, in hexadecimal:
    /// the store's `pins/` names it.
    pub pin: String,
}

/// A brush's region, or a measured one, as a record spells it (FL-022,
/// plan §14.3): its numbers as their bits, so a replay outlines exactly
/// what the Studio did. `measures` is written only when true, so a brush's
/// region is recorded as it was before measurement regions, and a record
/// without it is a brush's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "kebab-case")]
pub enum TimelineRegion {
    /// A cell's rectangle.
    Rectangle {
        /// `[[x₀, x₁], [y₀, y₁]]` in world units.
        bounds: [[Exact; 2]; 2],
        /// Whether the stroke removes energy from the world.
        removes: bool,
        /// Whether it is a measurement region rather than a stroke's.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        measures: bool,
    },
    /// A disc.
    Disc {
        /// Its centre in world units.
        center: [Exact; 2],
        /// Its radius in world units.
        radius: Exact,
        /// Whether the stroke removes energy from the world.
        removes: bool,
        /// Whether it is a measurement region rather than a stroke's.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        measures: bool,
    },
}

impl From<Region> for TimelineRegion {
    fn from(region: Region) -> Self {
        let removes = region.effect == RegionEffect::Removes;
        let measures = region.effect == RegionEffect::Measures;
        match region.shape {
            RegionShape::Rectangle(bounds) => Self::Rectangle {
                bounds: bounds.map(|range| range.map(Exact)),
                removes,
                measures,
            },
            RegionShape::Disc { center, radius } => Self::Disc {
                center: center.map(Exact),
                radius: Exact(radius),
                removes,
                measures,
            },
        }
    }
}

impl TryFrom<TimelineRegion> for Region {
    type Error = CoreError;

    /// The region a record spells.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] for a record that both removes and
    /// measures.
    fn try_from(region: TimelineRegion) -> Result<Self, CoreError> {
        let (shape, removes, measures) = match region {
            TimelineRegion::Rectangle {
                bounds,
                removes,
                measures,
            } => (
                RegionShape::Rectangle(bounds.map(|range| range.map(|end| end.0))),
                removes,
                measures,
            ),
            TimelineRegion::Disc {
                center,
                radius,
                removes,
                measures,
            } => (
                RegionShape::Disc {
                    center: center.map(|value| value.0),
                    radius: radius.0,
                },
                removes,
                measures,
            ),
        };
        let effect = match (removes, measures) {
            (false, false) => RegionEffect::Adds,
            (true, false) => RegionEffect::Removes,
            (false, true) => RegionEffect::Measures,
            (true, true) => {
                return Err(conflict(
                    "a region record both removes and measures".to_owned(),
                ));
            }
        };
        Ok(Self { shape, effect })
    }
}

/// One record of a Studio timeline, in the order the Studio made it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "record", rename_all = "kebab-case")]
pub enum TimelineRecord {
    /// The first record: the run and the view as the Studio opened it.
    Header {
        /// [`TIMELINE_SCHEMA`].
        schema: String,
        /// The configuration's hash, which a replay must run.
        config_hash: String,
        /// The configuration's name.
        name: String,
        /// Its seed.
        seed: u64,
        /// The view's size in points as it opened.
        size: [u32; 2],
        /// The style it opened with.
        style: TimelineStyle,
        /// The projection its affinity and composition colours were drawn
        /// through. A timeline recorded before it was recorded has none.
        /// Boxed: its matrix is most of the record.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        projection: Option<Box<TimelineProjection>>,
        /// Where a resumed Studio's session started; none for a session
        /// that founded its world, which a replay founds again.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<TimelineStart>,
    },
    /// The view took a drawable of `drawable` pixels.
    Resize {
        /// Width and height in pixels.
        drawable: [u32; 2],
    },
    /// The view took a screen of `pixels_per_point`.
    Density {
        /// Drawable pixels per logical point.
        pixels_per_point: Exact,
    },
    /// The overlay was laid out again: `text` and its swatches' colours.
    Overlay {
        /// The text; empty for none.
        text: String,
        /// Each swatch's sRGB colour.
        swatches: Vec<[u8; 3]>,
    },
    /// The style changed.
    Style {
        /// The new style.
        style: TimelineStyle,
    },
    /// The camera moved.
    View {
        /// Its centre in world units.
        center: [Exact; 2],
        /// Its zoom.
        zoom: Exact,
    },
    /// A selection: the organism picked, or none.
    Select {
        /// The organism's identity.
        organism: Option<u64>,
    },
    /// The region an armed brush would act on, outlined on the world, or
    /// none (FL-022).
    Region {
        /// The region.
        region: Option<TimelineRegion>,
    },
    /// [`flife_manim::WorldView::show_between`]: the epoch at `current`,
    /// `fraction` of the way from the one at `prior`.
    Show {
        /// The prior epoch's tick.
        prior: Option<u64>,
        /// The shown epoch's tick.
        current: u64,
        /// How far between them.
        fraction: Exact,
    },
    /// [`flife_manim::WorldView::place_between`], as [`TimelineRecord::Show`].
    Place {
        /// The prior epoch's tick.
        prior: Option<u64>,
        /// The shown epoch's tick.
        current: u64,
        /// How far between them.
        fraction: Exact,
    },
    /// [`flife_manim::WorldView::advance`], as [`TimelineRecord::Show`].
    Advance {
        /// The prior epoch's tick.
        prior: Option<u64>,
        /// The shown epoch's tick.
        current: u64,
        /// How far between them.
        fraction: Exact,
    },
    /// A frame was drawn: presented to the window, or rendered offscreen.
    Draw {
        /// Its index among the frames drawn, from 0.
        frame: u64,
        /// The shown epoch's tick.
        tick: u64,
        /// Wall seconds since the Studio opened (the presentation clock).
        wall_seconds: Exact,
    },
    /// The session accepted a command.
    Submitted {
        /// Its identity.
        command: u64,
        /// The source tick of the epoch it was made on.
        seen: u64,
        /// The committed tick whose boundary it applies at.
        effective: u64,
    },
    /// The session reported a command applied.
    Applied {
        /// Its identity.
        command: u64,
        /// The committed tick whose boundary it applied at.
        tick: u64,
        /// The tick that publication committed.
        published: u64,
    },
    /// The last record: the world the session closed on, and every command
    /// its book applied, in the order applied.
    End {
        /// The committed tick.
        tick: u64,
        /// The world's digest.
        digest: String,
        /// The applied commands.
        applied: Vec<AppliedCommand>,
    },
}

impl TimelineRecord {
    /// The header for `config`'s Studio opening at `size` points in `style`.
    #[must_use]
    pub fn header(config: &ValidatedConfig, size: [u32; 2], style: Style) -> Self {
        let model = config.config();
        Self::Header {
            schema: TIMELINE_SCHEMA.to_owned(),
            config_hash: config.config_hash().to_hex(),
            name: model.name.clone(),
            seed: model.seed,
            size,
            style: style.into(),
            projection: Some(Box::new(AffinityProjection::fixed().into())),
            from: None,
        }
    }

    /// The record of a status a timeline keeps: a command accepted, or
    /// applied.
    #[must_use]
    pub const fn status(status: &Status) -> Option<Self> {
        match status {
            Status::Submitted {
                command,
                seen,
                effective,
                ..
            } => Some(Self::Submitted {
                command: command.get(),
                seen: seen.get(),
                effective: effective.get(),
            }),
            Status::Finished {
                command,
                outcome:
                    Outcome::Applied {
                        tick, published, ..
                    },
            } => Some(Self::Applied {
                command: command.get(),
                tick: tick.get(),
                published: published.get(),
            }),
            _ => None,
        }
    }

    /// The last record: the committed world `closed` holds and every command
    /// its book applied, in the order applied.
    #[must_use]
    pub fn end(closed: &SessionClosed) -> Self {
        let book = &closed.book;
        let applied = book
            .outcome_stream()
            .iter()
            .filter_map(|&id| {
                let Some(CommandStatus::Terminal(Outcome::Applied {
                    tick,
                    published,
                    applied,
                    ..
                })) = book.status(id)
                else {
                    return None;
                };
                Some(AppliedCommand {
                    command: id.get(),
                    tick: tick.get(),
                    published: published.get(),
                    energy: Exact(*applied),
                    intervention: (&book.command(id)?.intervention).into(),
                })
            })
            .collect();
        Self::End {
            tick: closed.boundary.tick.get(),
            digest: closed.world.digest().to_hex(),
            applied,
        }
    }
}

/// A timeline's records from its NDJSON text, one record a line.
///
/// # Errors
/// [`CoreError::NumericalInvalid`] for a line that is not a record.
pub fn read_timeline(text: &str) -> Result<Vec<TimelineRecord>, CoreError> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            serde_json::from_str(line).map_err(|error| CoreError::NumericalInvalid {
                quantity: "timeline record",
                detail: format!("line {}: {error}", index + 1),
            })
        })
        .collect()
}

/// Write `record` to `out` as one line of a timeline file, as
/// [`read_timeline`] reads it back.
///
/// # Errors
/// `out`'s errors.
pub fn write_timeline_record(
    out: &mut impl std::io::Write,
    record: &TimelineRecord,
) -> std::io::Result<()> {
    serde_json::to_writer(&mut *out, record).map_err(std::io::Error::from)?;
    out.write_all(b"\n")
}

/// One frame a replay drew.
#[derive(Debug)]
pub struct ReplayFrame<'a> {
    /// Its index among the frames the Studio drew (or sampled in Cinema
    /// simulation-time export).
    pub frame: u64,
    /// The tick shown.
    pub tick: Tick,
    /// When the Studio drew it, in wall seconds since it opened (or
    /// `frame / fps` in simulation-time export).
    pub wall_seconds: f64,
    /// The frame, tight RGBA8 at the drawable's size.
    pub pixels: &'a FrameBuffer,
    /// The active presentation style rendered for this frame.
    pub style: Style,
    /// The rendered viewport center in world units.
    pub camera_center: [f64; 2],
    /// The rendered viewport zoom.
    pub camera_zoom: f64,
}

/// Verified header, closing state, and planned live-capture draw records of a
/// Studio timeline ([`validate_timeline`]).
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedTimeline {
    /// The configuration's name from the header.
    pub name: String,
    /// The configuration's seed from the header.
    pub seed: u64,
    /// The view's opening size in points.
    pub size: [u32; 2],
    /// The opening style, already validated against `config`.
    pub initial_style: Style,
    /// Where a resumed Studio started, if any.
    pub from: Option<TimelineStart>,
    /// The committed tick the session opened at (`from.tick` or `0`).
    pub opened: u64,
    /// The committed tick the session closed on.
    pub end_tick: u64,
    /// Number of applied commands recorded in `End`.
    pub applied_commands: usize,
    /// Every recorded `Draw` in strict sequential frame order.
    pub draws: Vec<PlannedDraw>,
}

/// One validated `TimelineRecord::Draw` in a Studio timeline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlannedDraw {
    /// Sequential frame index (`0, 1, 2, ...`).
    pub frame: u64,
    /// Shown epoch tick (non-decreasing, `opened <= tick <= end_tick`).
    pub tick: u64,
    /// Wall seconds since Studio opened (finite, non-negative, non-decreasing).
    pub wall_seconds: f64,
    /// Active drawable size at this draw.
    pub drawable: [u32; 2],
    /// Active presentation style at this draw.
    pub style: Style,
}

/// Refuse `V2-16` colour mappings (`cell-state`, `polarity`) and field
/// overlays (`trail-carrier`, `vascular-conductance`) when `config` does not
/// enable `v2-visual-mappings` (`INV-09`).
///
/// # Errors
/// [`CoreError::StateConflict`] naming the disallowed v2 visual mapping.
pub fn validate_v2_style(config: &ValidatedConfig, style: Style) -> Result<(), CoreError> {
    if config.is_v2_feature_enabled(V2Feature::V2VisualMappings) {
        return Ok(());
    }
    if style.mapping.is_v2() {
        return Err(conflict(format!(
            "style colour mapping `{}` requires a field-ecology-v2 configuration with \
             `v2-visual-mappings` enabled",
            style.mapping.name()
        )));
    }
    if style.field_mapping.is_v2_overlay() {
        return Err(conflict(format!(
            "style field mapping `{}` requires a field-ecology-v2 configuration with \
             `v2-visual-mappings` enabled",
            style.field_mapping.name()
        )));
    }
    Ok(())
}

/// Validate a timeline's structure, configuration identity, projection,
/// command book agreement, V2-16 style gating, tick monotonicity, and
/// wall-time monotonicity without touching disk or requiring a Metal device
/// (FL-023, FL-050, G10).
///
/// # Errors
/// [`CoreError::StateConflict`] or [`CoreError::NumericalInvalid`] naming the
/// first inconsistency.
pub fn validate_timeline(
    config: &ValidatedConfig,
    records: &[TimelineRecord],
) -> Result<ValidatedTimeline, CoreError> {
    let Some(TimelineRecord::Header {
        schema,
        config_hash,
        name,
        seed,
        size,
        style,
        projection,
        from,
    }) = records.first()
    else {
        return Err(conflict("a timeline opens with its header".into()));
    };
    if schema != TIMELINE_SCHEMA {
        return Err(conflict(format!("`{schema}` is not {TIMELINE_SCHEMA}")));
    }
    if *config_hash != config.config_hash().to_hex() {
        return Err(conflict(format!(
            "the timeline ran configuration {config_hash}, not {}",
            config.config_hash().to_hex()
        )));
    }
    let fixed = TimelineProjection::from(AffinityProjection::fixed());
    if let Some(recorded) = projection
        && **recorded != fixed
    {
        return Err(conflict(format!(
            "the timeline drew affinity through rows {:?} of the Hadamard matrix over {}, not \
             rows {:?} over {}",
            recorded.rows, recorded.divisor.0, fixed.rows, fixed.divisor.0
        )));
    }
    if size[0] == 0 || size[1] == 0 {
        return Err(CoreError::NumericalInvalid {
            quantity: "timeline header size",
            detail: format!("opening size {size:?} must be non-zero"),
        });
    }
    let initial_style = style.style()?;
    validate_v2_style(config, initial_style)?;
    let Some(TimelineRecord::End {
        tick: end_tick,
        applied,
        ..
    }) = records.last()
    else {
        return Err(conflict(
            "a timeline closes with its end: the Studio did not close".into(),
        ));
    };
    let opened = from.as_ref().map_or(0, |start| start.tick);
    if opened > *end_tick {
        return Err(conflict(format!(
            "session opened at tick {opened}, after its closing tick {end_tick}"
        )));
    }

    let mut book_ticks: BTreeMap<u64, (u64, u64)> = BTreeMap::new();
    for command in applied {
        let _ = command.intervention.intervention()?;
        if command.tick > *end_tick {
            return Err(conflict(format!(
                "applied command {} at tick {} is past closing tick {end_tick}",
                command.command, command.tick
            )));
        }
        if book_ticks
            .insert(command.command, (command.tick, command.published))
            .is_some()
        {
            return Err(conflict(format!(
                "applied command {} appears more than once in the timeline end record",
                command.command
            )));
        }
    }

    let last_index = records.len().saturating_sub(1);
    let mut showing = opened;
    let mut cur_drawable = *size;
    let mut cur_style = initial_style;
    let mut last_wall: Option<f64> = None;
    let mut draws = Vec::new();
    for (index, record) in records.iter().enumerate() {
        match record {
            TimelineRecord::Header { .. } if index > 0 => {
                return Err(conflict(format!(
                    "duplicate timeline header at record {index}"
                )));
            }
            TimelineRecord::End { .. } if index < last_index => {
                return Err(conflict(format!(
                    "timeline end record at index {index} before final record {last_index}"
                )));
            }
            TimelineRecord::Applied {
                command,
                tick,
                published,
            } => {
                if book_ticks.get(command) != Some(&(*tick, *published)) {
                    return Err(conflict(format!(
                        "command {command} was reported applied at tick {tick}, which its book \
                         does not record"
                    )));
                }
            }
            TimelineRecord::Resize { drawable } => {
                if drawable[0] == 0 || drawable[1] == 0 {
                    return Err(CoreError::NumericalInvalid {
                        quantity: "timeline resize drawable",
                        detail: format!("drawable {drawable:?} must be non-zero"),
                    });
                }
                cur_drawable = *drawable;
            }
            TimelineRecord::Density { pixels_per_point } => {
                if !pixels_per_point.0.is_finite() || pixels_per_point.0 <= 0.0 {
                    return Err(CoreError::NumericalInvalid {
                        quantity: "timeline pixel density",
                        detail: format!(
                            "density {} must be finite and positive",
                            pixels_per_point.0
                        ),
                    });
                }
            }
            TimelineRecord::Style { style } => {
                let parsed = style.style()?;
                validate_v2_style(config, parsed)?;
                cur_style = parsed;
            }
            TimelineRecord::Region { region } => {
                if let Some(region) = region {
                    let _ = Region::try_from(*region)?;
                }
            }
            TimelineRecord::Show {
                prior,
                current,
                fraction,
            }
            | TimelineRecord::Place {
                prior,
                current,
                fraction,
            }
            | TimelineRecord::Advance {
                prior,
                current,
                fraction,
            } => {
                if !fraction.0.is_finite() || !(0.0..=1.0).contains(&fraction.0) {
                    return Err(CoreError::NumericalInvalid {
                        quantity: "timeline display fraction",
                        detail: format!("fraction {} is outside [0, 1]", fraction.0),
                    });
                }
                if let Some(prior_tick) = prior
                    && (*prior_tick < opened || *prior_tick > *current)
                {
                    return Err(conflict(format!(
                        "prior epoch tick {prior_tick} is outside [{opened}, {current}]"
                    )));
                }
                if *current < showing || *current > *end_tick {
                    return Err(conflict(format!(
                        "shown epoch tick {current} is outside [{showing}, {end_tick}]"
                    )));
                }
                showing = *current;
            }
            TimelineRecord::Draw {
                frame,
                tick,
                wall_seconds,
            } => {
                let expected_frame = draws.len() as u64;
                if *frame != expected_frame {
                    return Err(conflict(format!(
                        "draw frame index {frame} is out of sequence (expected {expected_frame})"
                    )));
                }
                if *tick != showing {
                    return Err(conflict(format!(
                        "frame {frame} was recorded showing tick {tick}, but the view shows tick \
                         {showing}"
                    )));
                }
                let wall = wall_seconds.0;
                if !wall.is_finite() || wall < 0.0 {
                    return Err(conflict(
                        "the frames' wall times are not finite and in order".into(),
                    ));
                }
                if let Some(prev) = last_wall
                    && wall < prev
                {
                    return Err(conflict(
                        "the frames' wall times are not finite and in order".into(),
                    ));
                }
                last_wall = Some(wall);
                draws.push(PlannedDraw {
                    frame: *frame,
                    tick: *tick,
                    wall_seconds: wall,
                    drawable: cur_drawable,
                    style: cur_style,
                });
            }
            TimelineRecord::View { center, zoom } => {
                if !center.iter().all(|coord| coord.0.is_finite()) {
                    return Err(CoreError::NumericalInvalid {
                        quantity: "timeline viewport center",
                        detail: format!("center [{}, {}] must be finite", center[0].0, center[1].0),
                    });
                }
                if !(1.0..=flife_manim::MAXIMUM_ZOOM).contains(&zoom.0) {
                    return Err(CoreError::NumericalInvalid {
                        quantity: "timeline viewport zoom",
                        detail: format!(
                            "zoom {} is outside [1, {}]",
                            zoom.0,
                            flife_manim::MAXIMUM_ZOOM
                        ),
                    });
                }
            }
            TimelineRecord::Header { .. }
            | TimelineRecord::Overlay { .. }
            | TimelineRecord::Select { .. }
            | TimelineRecord::Submitted { .. }
            | TimelineRecord::End { .. } => {}
        }
    }

    Ok(ValidatedTimeline {
        name: name.clone(),
        seed: *seed,
        size: *size,
        initial_style,
        from: from.clone(),
        opened,
        end_tick: *end_tick,
        applied_commands: applied.len(),
        draws,
    })
}

/// Controls for [`replay_with`] (used internally by Cinema export and
/// [`replay`]).
pub(crate) struct ReplayConfig<'a> {
    pub cpu_workers: Option<NonZeroUsize>,
    pub target_resolution: Option<([u32; 2], f64)>,
    pub extra_render_bytes: u64,
    pub camera: Option<&'a CameraPath>,
    pub sample_every_ticks: Option<(NonZeroU64, std::num::NonZeroU32)>,
}

/// What a replay reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replayed {
    /// Frames drawn.
    pub frames: u64,
    /// The committed tick it closed on.
    pub tick: Tick,
    /// The world's digest there: the Studio's.
    pub digest: String,
    /// Commands applied.
    pub commands: usize,
}

fn conflict(detail: String) -> CoreError {
    CoreError::StateConflict {
        operation: "replay a studio timeline",
        detail,
    }
}

/// Redraw every frame a Studio's timeline records, passing each to `drawn`
/// in order, and reach the world it closed on (see the module).
///
/// # Errors
/// - [`CoreError::StateConflict`] for a timeline without its header or its
///   end, of another schema or configuration, or whose colours were drawn
///   through another affinity projection than [`AffinityProjection::fixed`]
///   (one recorded before projections were has none, and is taken); a
///   status that disagrees with
///   the book's record; an epoch shown before one already passed; a camera
///   the view does not take exactly; a frame recorded at another tick than
///   the epoch the view was given; or a replay that closes on another
///   world than the Studio's.
/// - For a resumed Studio's timeline, [`flife_cli::restore_checkpoint`]'s
///   refusals: its run directory gone or of another configuration, held
///   by another process, or no longer holding the checkpoint whole; and
///   [`CoreError::StateConflict`] for a checkpoint of another tick than
///   the one restored.
/// - [`CoreError::MissingCapability`] without a Metal device.
/// - The world's, the view's and `drawn`'s errors.
pub fn replay(
    config: &ValidatedConfig,
    records: &[TimelineRecord],
    cpu_workers: Option<NonZeroUsize>,
    mut drawn: impl FnMut(ReplayFrame<'_>) -> Result<(), CoreError>,
) -> Result<Replayed, CoreError> {
    let cfg = ReplayConfig {
        cpu_workers,
        target_resolution: None,
        extra_render_bytes: 0,
        camera: None,
        sample_every_ticks: None,
    };
    let (replayed, _admitted, ()) =
        replay_with(config, records, &cfg, |_| Ok(()), |(), frame| drawn(frame))?;
    Ok(replayed)
}

/// Replay `records` under `options`, invoking `on_admitted` once the world is
/// restored and FL-043 memory admission passes (before the first frame is
/// rendered, so export directory creation never runs on a refused replay),
/// passing the initialized state `S` and each rendered frame to `drawn`.
pub(crate) fn replay_with<S>(
    config: &ValidatedConfig,
    records: &[TimelineRecord],
    options: &ReplayConfig<'_>,
    on_admitted: impl FnOnce(&MemoryAdmission) -> Result<S, CoreError>,
    mut drawn: impl FnMut(&mut S, ReplayFrame<'_>) -> Result<(), CoreError>,
) -> Result<(Replayed, MemoryAdmission, S), CoreError> {
    let validated = validate_timeline(config, records)?;
    let Some(TimelineRecord::End {
        tick: end,
        digest,
        applied,
    }) = records.last()
    else {
        return Err(conflict(
            "a timeline closes with its end: the Studio did not close".into(),
        ));
    };
    let mut at: BTreeMap<u64, Vec<Intervention>> = BTreeMap::new();
    for command in applied {
        at.entry(command.tick)
            .or_default()
            .push(command.intervention.intervention()?);
    }

    let model = config.config();
    let background =
        hex_rgb(&model.presentation.background).ok_or_else(|| CoreError::NumericalInvalid {
            quantity: "presentation background",
            detail: format!("`{}` is not #rrggbb", model.presentation.background),
        })?;
    if !MetalRenderer::is_available() {
        return Err(CoreError::MissingCapability {
            capability: "a Metal device to replay a studio timeline".into(),
        });
    }
    // The world the Studio's session was opened on: its configuration's
    // founding world, or the checkpoint a resumed run restored.
    let mut world = match validated
        .from
        .as_ref()
        .and_then(|start| Some((start, start.checkpoint.as_ref()?)))
    {
        Some((start, checkpoint)) => {
            let world = flife_cli::restore_checkpoint(
                Path::new(&start.run_dir),
                config,
                checkpoint.sequence,
            )?;
            if world.tick().get() != checkpoint.tick {
                return Err(conflict(format!(
                    "checkpoint {} of {} holds tick {}, where the Studio restored tick {}",
                    checkpoint.sequence,
                    start.run_dir,
                    world.tick().get(),
                    checkpoint.tick
                )));
            }
            world
        }
        None => flife_cli::found_on(config, options.cpu_workers)?,
    };
    if let Some(workers) = options.cpu_workers {
        world.set_cpu_workers(workers)?;
    }
    // FL-043: admit the replay world, its peak observer buffers (across the
    // timeline or target Cinema resolution), and any extra export encoder
    // scratch BEFORE allocating the 4-slot publisher, Metal renderer, or
    // output directory (INV-15).
    let standalone = flife_render::Epoch::standalone(&world)?;
    let mut view = crate::host::studio_view(
        config,
        &standalone,
        validated.size,
        validated.initial_style,
        background,
    )?;
    let peak_observers = peak_observer_bytes_for(
        &world,
        &standalone,
        &view,
        validated.size,
        records,
        options.target_resolution,
    )?;
    let admitted = world.admit_memory(
        false,
        0,
        peak_observers.saturating_add(options.extra_render_bytes),
    )?;
    drop(standalone);
    let mut renderer = MetalRenderer::new().map_err(|error| CoreError::MissingCapability {
        capability: format!("a Metal renderer to replay a studio timeline: {error}"),
    })?;
    // Notify caller only after validation, restore, admission, and Metal
    // initialization all succeeded.
    let mut sink = on_admitted(&admitted)?;

    // A prior, a current and one being written, and one to spare.
    let mut publisher = Publisher::new(&world, REPLAY_SLOTS, || {})?;
    let viewer = publisher.feed().viewer(3)?;
    let restored = viewer.latest()?;
    let opened = validated.opened;
    let opening = if opened == restored.tick().get() {
        restored
    } else {
        drop(restored);
        publish_at(&mut world, opened, &at, &mut publisher, &viewer)?
    };
    if let Some((drawable, scale)) = options.target_resolution {
        view.refit_surface(drawable, scale)?;
    }
    view.show(&opening)?;
    let font = HudFont::bundled()?;

    let mut frames = 0_u64;
    if let Some((every_ticks, fps)) = options.sample_every_ticks {
        // Simulation-time export (plan §13.8, FL-050): sample exact integer
        // ticks `opened + k * every_ticks <= end`, applying recorded
        // presentation changes (style, camera, overlay, selection, region) in
        // committed-tick order up to each sample tick.
        let samples = crate::export::plan_simulation_samples(
            opened,
            *end,
            every_ticks,
            config.report().timestep.seconds_per_tick(),
            fps,
        )?;
        // Group recorded presentation events by the committed tick active when
        // they occurred.
        let mut events_by_tick: Vec<(u64, &TimelineRecord)> = Vec::new();
        let mut rec_tick = opened;
        for record in records {
            match record {
                TimelineRecord::Show { current, .. }
                | TimelineRecord::Place { current, .. }
                | TimelineRecord::Advance { current, .. } => rec_tick = *current,
                TimelineRecord::Resize { .. }
                | TimelineRecord::Density { .. }
                | TimelineRecord::Overlay { .. }
                | TimelineRecord::Style { .. }
                | TimelineRecord::View { .. }
                | TimelineRecord::Select { .. }
                | TimelineRecord::Region { .. } => events_by_tick.push((rec_tick, record)),
                _ => {}
            }
        }
        let mut event_cursor = 0_usize;
        let mut pending_drawable = validated.size;
        let mut pending_scale = 1.0_f64;
        let mut geometry_dirty = false;
        let mut current_epoch = Some(opening);
        for sample in samples {
            // Apply all presentation updates recorded at or before sample.tick.
            while event_cursor < events_by_tick.len()
                && events_by_tick[event_cursor].0 <= sample.tick
            {
                let record = events_by_tick[event_cursor].1;
                event_cursor += 1;
                if options.target_resolution.is_none() && geometry_dirty && flushes_geometry(record)
                {
                    view.refit_surface(pending_drawable, pending_scale)?;
                    geometry_dirty = false;
                }
                match record {
                    TimelineRecord::Resize { drawable } if options.target_resolution.is_none() => {
                        view.viewport().resized(*drawable)?;
                        pending_drawable = *drawable;
                        geometry_dirty = true;
                    }
                    TimelineRecord::Density { pixels_per_point }
                        if options.target_resolution.is_none() =>
                    {
                        view.viewport().with_pixels_per_point(pixels_per_point.0)?;
                        pending_scale = pixels_per_point.0;
                        geometry_dirty = true;
                    }
                    TimelineRecord::Overlay { text, swatches } => {
                        crate::host::lay_out_overlay(&mut view, &font, text, swatches)?;
                    }
                    TimelineRecord::Style { style } => view.set_style(style.style()?),
                    TimelineRecord::View { center, zoom } if options.camera.is_none() => {
                        let viewport = view
                            .viewport()
                            .with_view([center[0].0, center[1].0], zoom.0)?;
                        view.set_view(viewport)?;
                    }
                    TimelineRecord::Select { organism } => {
                        view.select(organism.map(OrganismId::new))?;
                    }
                    TimelineRecord::Region { region } => {
                        view.set_region(region.map(Region::try_from).transpose()?)?;
                    }
                    _ => {}
                }
            }
            if options.target_resolution.is_none() && geometry_dirty {
                view.refit_surface(pending_drawable, pending_scale)?;
                geometry_dirty = false;
            }
            if !current_epoch
                .as_ref()
                .is_some_and(|held| held.tick().get() == sample.tick)
            {
                drop(current_epoch.take());
                current_epoch = Some(publish_at(
                    &mut world,
                    sample.tick,
                    &at,
                    &mut publisher,
                    &viewer,
                )?);
            }
            let epoch = current_epoch
                .as_ref()
                .ok_or_else(|| conflict("missing published epoch".into()))?;
            if let Some(camera) = options.camera {
                let (viewport, _) = camera.viewport(view.viewport(), sample.tick)?;
                view.set_view(viewport)?;
            }
            view.show(epoch)?;
            let job = view.job()?;
            let (pixels, _) = renderer
                .render_three_d_rgba8(&job)
                .map_err(|error| conflict(format!("render frame {}: {error}", sample.frame)))?;
            let vp = view.viewport();
            drawn(
                &mut sink,
                ReplayFrame {
                    frame: sample.frame,
                    tick: Tick::new(sample.tick),
                    wall_seconds: sample.video_seconds,
                    pixels: &pixels,
                    style: view.style(),
                    camera_center: vp.center(),
                    camera_zoom: vp.zoom(),
                },
            )?;
            frames += 1;
        }
        drop(current_epoch);
    } else {
        // Live-capture replay (FL-023, plus optional target Cinema resolution
        // or CameraPath override).
        let mut last_use: BTreeMap<u64, usize> = BTreeMap::new();
        let mut pending_display: (Option<u64>, u64) = (None, opened);
        for (index, record) in records.iter().enumerate() {
            match record {
                TimelineRecord::Show { prior, current, .. }
                | TimelineRecord::Place { prior, current, .. }
                | TimelineRecord::Advance { prior, current, .. } => {
                    pending_display = (*prior, *current);
                    for tick in prior.iter().chain([current]) {
                        last_use.insert(*tick, index);
                    }
                }
                TimelineRecord::Draw { .. } if options.camera.is_some() => {
                    for tick in pending_display.0.iter().chain([&pending_display.1]) {
                        last_use.insert(*tick, index);
                    }
                }
                _ => {}
            }
        }
        let mut epochs: BTreeMap<u64, Lease> = BTreeMap::new();
        epochs.insert(opened, opening);
        let mut showing = opened;
        let mut last_display: (Option<u64>, u64, f64) = (None, opened, 1.0);
        let mut pending_drawable = validated.size;
        let mut pending_scale = 1.0_f64;
        let mut geometry_dirty = false;
        for (index, record) in records.iter().enumerate() {
            if options.target_resolution.is_none() && geometry_dirty && flushes_geometry(record) {
                view.refit_surface(pending_drawable, pending_scale)?;
                geometry_dirty = false;
            }
            match record {
                TimelineRecord::Resize { drawable } => {
                    if options.target_resolution.is_none() {
                        view.viewport().resized(*drawable)?;
                        pending_drawable = *drawable;
                        geometry_dirty = true;
                    }
                }
                TimelineRecord::Density { pixels_per_point } => {
                    if options.target_resolution.is_none() {
                        view.viewport().with_pixels_per_point(pixels_per_point.0)?;
                        pending_scale = pixels_per_point.0;
                        geometry_dirty = true;
                    }
                }
                TimelineRecord::Overlay { text, swatches } => {
                    crate::host::lay_out_overlay(&mut view, &font, text, swatches)?;
                }
                TimelineRecord::Style { style } => view.set_style(style.style()?),
                TimelineRecord::View { center, zoom } => {
                    if options.camera.is_none() {
                        let viewport = view
                            .viewport()
                            .with_view([center[0].0, center[1].0], zoom.0)?;
                        view.set_view(viewport)?;
                        let taken = view.viewport();
                        if [taken.center()[0], taken.center()[1], taken.zoom()].map(Exact)
                            != [center[0], center[1], *zoom]
                        {
                            return Err(conflict(format!(
                                "the view took {:?} at zoom {} for the recorded {center:?} at \
                                 {zoom:?}",
                                taken.center(),
                                taken.zoom()
                            )));
                        }
                    }
                }
                TimelineRecord::Select { organism } => {
                    view.select(organism.map(OrganismId::new))?;
                }
                TimelineRecord::Region { region } => {
                    view.set_region(region.map(Region::try_from).transpose()?)?;
                }
                TimelineRecord::Show {
                    prior,
                    current,
                    fraction,
                }
                | TimelineRecord::Place {
                    prior,
                    current,
                    fraction,
                }
                | TimelineRecord::Advance {
                    prior,
                    current,
                    fraction,
                } => {
                    for tick in prior.iter().chain([current]) {
                        if !epochs.contains_key(tick) {
                            let lease =
                                publish_at(&mut world, *tick, &at, &mut publisher, &viewer)?;
                            epochs.insert(*tick, lease);
                        }
                    }
                    let shown = &epochs[current];
                    let from = prior.map(|tick| &*epochs[&tick]);
                    match record {
                        TimelineRecord::Show { .. } => {
                            view.show_between(from, shown, fraction.0)?;
                        }
                        TimelineRecord::Place { .. } => {
                            view.place_between(from, shown, fraction.0)?;
                        }
                        _ => {
                            view.advance(from, shown, fraction.0)?;
                        }
                    }
                    showing = *current;
                    last_display = (*prior, *current, fraction.0);
                    epochs.retain(|tick, _| last_use.get(tick).is_some_and(|&last| last > index));
                }
                TimelineRecord::Draw {
                    frame,
                    tick,
                    wall_seconds,
                } => {
                    if *tick != showing {
                        return Err(conflict(format!(
                            "frame {frame} was recorded showing tick {tick}, but the view shows \
                             tick {showing}"
                        )));
                    }
                    if let Some(camera) = options.camera {
                        let (viewport, _) = camera.viewport(view.viewport(), *tick)?;
                        if view.set_view(viewport)? {
                            let (prior_tick, current_tick, fraction) = last_display;
                            let shown = epochs.get(&current_tick).ok_or_else(|| {
                                conflict(format!(
                                    "missing epoch for tick {current_tick} during camera export"
                                ))
                            })?;
                            let from = prior_tick
                                .map(|prior| {
                                    epochs.get(&prior).map(std::ops::Deref::deref).ok_or_else(
                                        || {
                                            conflict(format!(
                                                "missing prior epoch for tick {prior} during \
                                                 camera export"
                                            ))
                                        },
                                    )
                                })
                                .transpose()?;
                            view.place_between(from, shown, fraction)?;
                        }
                        epochs.retain(|t, _| last_use.get(t).is_some_and(|&last| last > index));
                    }
                    let job = view.job()?;
                    let (pixels, _) = renderer
                        .render_three_d_rgba8(&job)
                        .map_err(|error| conflict(format!("render frame {frame}: {error}")))?;
                    let vp = view.viewport();
                    drawn(
                        &mut sink,
                        ReplayFrame {
                            frame: *frame,
                            tick: Tick::new(*tick),
                            wall_seconds: wall_seconds.0,
                            pixels: &pixels,
                            style: view.style(),
                            camera_center: vp.center(),
                            camera_zoom: vp.zoom(),
                        },
                    )?;
                    frames += 1;
                }
                TimelineRecord::Header { .. }
                | TimelineRecord::Submitted { .. }
                | TimelineRecord::Applied { .. }
                | TimelineRecord::End { .. } => {}
            }
        }
        if options.target_resolution.is_none() && geometry_dirty {
            view.refit_surface(pending_drawable, pending_scale)?;
        }
        drop(epochs);
    }

    step_to(&mut world, *end, &at)?;
    let reached = world.digest().to_hex();
    if reached != *digest {
        return Err(conflict(format!(
            "the replay reached {reached} at tick {end}, not the Studio's {digest}"
        )));
    }
    Ok((
        Replayed {
            frames,
            tick: world.tick(),
            digest: reached,
            commands: applied.len(),
        },
        admitted,
        sink,
    ))
}

/// Whether `record` depends on the view's current `(drawable,
/// pixels_per_point)` geometry and therefore flushes any coalesced
/// `Resize` / `Density` updates before executing.
const fn flushes_geometry(record: &TimelineRecord) -> bool {
    matches!(
        record,
        TimelineRecord::Overlay { .. }
            | TimelineRecord::View { .. }
            | TimelineRecord::Select { .. }
            | TimelineRecord::Region { .. }
            | TimelineRecord::Show { .. }
            | TimelineRecord::Place { .. }
            | TimelineRecord::Advance { .. }
            | TimelineRecord::Draw { .. }
    )
}

/// Peak observer bytes a timeline replay requires across its initial size,
/// every geometry flush point (`Overlay`, `View`, `Select`, `Region`, `Show`,
/// `Place`, `Advance`, `Draw`), and its final surface state (FL-043).
/// Coalesces adjacent `Resize` and `Density` records exactly as [`replay`]
/// does via `WorldView::refit_surface`, so a Retina `Resize` paired with
/// `Density` before a frame never incurs a phantom 1x density-bin charge.
///
/// # Errors
/// [`CoreError::NumericalInvalid`] for an invalid drawable size or pixel
/// density in `records`.
pub fn peak_observer_bytes(
    world: &World,
    restored: &flife_render::Epoch,
    view: &flife_manim::WorldView,
    initial_size: [u32; 2],
    records: &[TimelineRecord],
) -> Result<u64, CoreError> {
    peak_observer_bytes_for(world, restored, view, initial_size, records, None)
}

/// As [`peak_observer_bytes`], optionally charging a fixed Cinema target
/// `(drawable, pixels_per_point)` (such as 1080p or 4K) instead of the
/// timeline's recorded window resizes (FL-043, FL-050).
///
/// # Errors
/// [`CoreError::NumericalInvalid`] for an invalid drawable size or pixel
/// density.
pub fn peak_observer_bytes_for(
    _world: &World,
    restored: &flife_render::Epoch,
    view: &flife_manim::WorldView,
    initial_size: [u32; 2],
    records: &[TimelineRecord],
    target_resolution: Option<([u32; 2], f64)>,
) -> Result<u64, CoreError> {
    let feed_bytes = restored
        .reserved_bytes()
        .saturating_mul(REPLAY_SLOTS as u64);
    let mut cur_drawable = initial_size;
    let mut cur_scale = 1.0_f64;
    let mut geometry_dirty = false;
    let mut peak =
        crate::memory::observer_bytes_for_resize(feed_bytes, view, cur_drawable, cur_scale)?;
    if let Some((target_drawable, target_scale)) = target_resolution {
        let target_bytes = crate::memory::observer_bytes_for_resize(
            feed_bytes,
            view,
            target_drawable,
            target_scale,
        )?;
        return Ok(peak.max(target_bytes));
    }
    for record in records {
        if geometry_dirty && flushes_geometry(record) {
            let required = crate::memory::observer_bytes_for_resize(
                feed_bytes,
                view,
                cur_drawable,
                cur_scale,
            )?;
            peak = peak.max(required);
            geometry_dirty = false;
        }
        match record {
            TimelineRecord::Resize { drawable } => {
                view.viewport().resized(*drawable)?;
                cur_drawable = *drawable;
                geometry_dirty = true;
            }
            TimelineRecord::Density { pixels_per_point } => {
                view.viewport().with_pixels_per_point(pixels_per_point.0)?;
                cur_scale = pixels_per_point.0;
                geometry_dirty = true;
            }
            _ => {}
        }
    }
    if geometry_dirty {
        let final_bytes =
            crate::memory::observer_bytes_for_resize(feed_bytes, view, cur_drawable, cur_scale)?;
        peak = peak.max(final_bytes);
    }
    Ok(peak)
}

/// Step `world` to committed tick `tick` ([`step_to`]) and publish it: the
/// epoch a frame there shows.
fn publish_at(
    world: &mut World,
    tick: u64,
    at: &BTreeMap<u64, Vec<Intervention>>,
    publisher: &mut Publisher,
    viewer: &Viewer,
) -> Result<Lease, CoreError> {
    step_to(world, tick, at)?;
    if let Publication::Busy { .. } = publisher.offer(world)? {
        return Err(conflict(format!("no render slot was free for tick {tick}")));
    }
    let lease = viewer.latest()?;
    if lease.tick().get() != tick {
        return Err(conflict(format!(
            "tick {tick} was published as tick {}",
            lease.tick().get()
        )));
    }
    Ok(lease)
}

/// Step `world` to committed tick `tick`, applying at each boundary the
/// commands `at` records there.
fn step_to(
    world: &mut World,
    tick: u64,
    at: &BTreeMap<u64, Vec<Intervention>>,
) -> Result<(), CoreError> {
    if world.tick().get() > tick {
        return Err(conflict(format!(
            "tick {tick} is shown after tick {}",
            world.tick().get()
        )));
    }
    while world.tick().get() < tick {
        let boundary = world.tick().get();
        let commands = at.get(&boundary).map_or(&[][..], Vec::as_slice);
        if let Stepped::Paused(shortfall) = world.step_with(commands)? {
            return Err(conflict(format!(
                "the world paused at tick {boundary}: {shortfall:?}"
            )));
        }
    }
    Ok(())
}
