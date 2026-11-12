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
//! commands at those ticks. It makes the same changes to a fresh view in the
//! same order and draws each frame the Studio drew through the same Metal
//! route ([`MetalRenderer::render_three_d_rgba8`]). Every number a frame
//! depends on is written as its bits ([`Exact`]). So the frames a replay
//! draws are the Studio's, byte for byte, at the ticks the Studio showed
//! them, and the replay closes on the world the Studio closed on, or refuses.
//! A frame drawn in a window is replayed as it was rendered, before the
//! window composited it.

use std::collections::BTreeMap;
use std::num::NonZeroUsize;

use flife_core::ids::OrganismId;
use flife_core::{CoreError, Tick, ValidatedConfig};
use flife_engine::{
    Arrangement, Disposition, Intervention, MATERIAL_CHANNELS, NumericalProfile, Stepped, World,
};
use flife_manim::{
    ColorMapping, FieldMapping, HudFont, Look, Particles, Region, RegionShape, Style, hex_rgb,
};
use flife_render::{Lease, Publication, Publisher};
use flife_runtime::{CommandStatus, Outcome, SessionClosed};
use fmn_frame::FrameBuffer;
use fmn_render::metal::MetalRenderer;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::Status;

/// The schema of a Studio timeline's records.
pub const TIMELINE_SCHEMA: &str = "frankenlife.timeline.v1";

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

/// A [`Style`] by its names, as flags and records spell them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineStyle {
    /// The colour mapping.
    pub mapping: String,
    /// `glow` or `raw`.
    pub look: String,
    /// `points` or `density`.
    pub particles: String,
    /// Whether the field is drawn.
    pub field: bool,
    /// What the field's colour means: `energy` or `composition`. A record
    /// from before FL-019's composition mapping has none, and means
    /// `energy`.
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

/// A brush's region as a record spells it (FL-022): its numbers as their
/// bits, so a replay outlines exactly what the Studio did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "kebab-case")]
pub enum TimelineRegion {
    /// A cell's rectangle.
    Rectangle {
        /// `[[x₀, x₁], [y₀, y₁]]` in world units.
        bounds: [[Exact; 2]; 2],
        /// Whether the stroke removes energy from the world.
        removes: bool,
    },
    /// A disc.
    Disc {
        /// Its centre in world units.
        center: [Exact; 2],
        /// Its radius in world units.
        radius: Exact,
        /// Whether the stroke removes energy from the world.
        removes: bool,
    },
}

impl From<Region> for TimelineRegion {
    fn from(region: Region) -> Self {
        match region.shape {
            RegionShape::Rectangle(bounds) => Self::Rectangle {
                bounds: bounds.map(|range| range.map(Exact)),
                removes: region.removes,
            },
            RegionShape::Disc { center, radius } => Self::Disc {
                center: center.map(Exact),
                radius: Exact(radius),
                removes: region.removes,
            },
        }
    }
}

impl From<TimelineRegion> for Region {
    fn from(region: TimelineRegion) -> Self {
        match region {
            TimelineRegion::Rectangle { bounds, removes } => Self {
                shape: RegionShape::Rectangle(bounds.map(|range| range.map(|end| end.0))),
                removes,
            },
            TimelineRegion::Disc {
                center,
                radius,
                removes,
            } => Self {
                shape: RegionShape::Disc {
                    center: center.map(|value| value.0),
                    radius: radius.0,
                },
                removes,
            },
        }
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
    /// Its index among the frames the Studio drew.
    pub frame: u64,
    /// The tick shown.
    pub tick: Tick,
    /// When the Studio drew it, in wall seconds since it opened.
    pub wall_seconds: f64,
    /// The frame, tight RGBA8 at the drawable's size.
    pub pixels: &'a FrameBuffer,
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
///   end, of another schema or configuration; a status that disagrees with
///   the book's record; an epoch shown before one already passed; a camera
///   the view does not take exactly; a frame recorded at another tick than
///   the epoch the view was given; or a replay that closes on another
///   world than the Studio's.
/// - [`CoreError::MissingCapability`] without a Metal device.
/// - The world's, the view's and `drawn`'s errors.
pub fn replay(
    config: &ValidatedConfig,
    records: &[TimelineRecord],
    cpu_workers: Option<NonZeroUsize>,
    mut drawn: impl FnMut(ReplayFrame<'_>) -> Result<(), CoreError>,
) -> Result<Replayed, CoreError> {
    let Some(TimelineRecord::Header {
        schema,
        config_hash,
        size,
        style,
        ..
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
    // The book's commands by the boundary they applied at, in the order
    // applied; the statuses must agree with it.
    let mut at: BTreeMap<u64, Vec<Intervention>> = BTreeMap::new();
    let mut ticks: BTreeMap<u64, (u64, u64)> = BTreeMap::new();
    for command in applied {
        at.entry(command.tick)
            .or_default()
            .push(command.intervention.intervention()?);
        ticks.insert(command.command, (command.tick, command.published));
    }
    for record in records {
        if let TimelineRecord::Applied {
            command,
            tick,
            published,
        } = record
            && ticks.get(command) != Some(&(*tick, *published))
        {
            return Err(conflict(format!(
                "command {command} was reported applied at tick {tick}, which its book does not \
                 record"
            )));
        }
    }
    // The last record that shows each epoch: it is kept until then.
    let mut last_use: BTreeMap<u64, usize> = BTreeMap::new();
    for (index, record) in records.iter().enumerate() {
        if let TimelineRecord::Show { prior, current, .. }
        | TimelineRecord::Place { prior, current, .. }
        | TimelineRecord::Advance { prior, current, .. } = record
        {
            for tick in prior.iter().chain([current]) {
                last_use.insert(*tick, index);
            }
        }
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
    let mut renderer = MetalRenderer::new().map_err(|error| CoreError::MissingCapability {
        capability: format!("a Metal renderer to replay a studio timeline: {error}"),
    })?;
    let mut world = World::found(
        config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32()?,
    )?;
    if let Some(workers) = cpu_workers {
        world.set_cpu_workers(workers)?;
    }
    // A prior, a current and one being written, and one to spare.
    let mut publisher = Publisher::new(&world, 4, || {})?;
    let viewer = publisher.feed().viewer(3)?;
    let founding = viewer.latest()?;
    // As `Studio::open` makes it.
    let mut view = crate::host::studio_view(config, &founding, *size, style.style()?, background)?;
    view.show(&founding)?;
    let font = HudFont::bundled()?;
    let mut epochs: BTreeMap<u64, Lease> = BTreeMap::new();
    epochs.insert(0, founding);
    // The tick of the epoch the view was last given: each frame's.
    let mut showing = 0;
    let mut frames = 0;
    for (index, record) in records.iter().enumerate() {
        match record {
            TimelineRecord::Resize { drawable } => {
                view.resize(*drawable)?;
            }
            TimelineRecord::Density { pixels_per_point } => {
                view.set_pixels_per_point(pixels_per_point.0)?;
            }
            TimelineRecord::Overlay { text, swatches } => {
                crate::host::lay_out_overlay(&mut view, &font, text, swatches)?;
            }
            TimelineRecord::Style { style } => view.set_style(style.style()?),
            TimelineRecord::View { center, zoom } => {
                let viewport = view
                    .viewport()
                    .with_view([center[0].0, center[1].0], zoom.0)?;
                view.set_view(viewport)?;
                let taken = view.viewport();
                if [taken.center()[0], taken.center()[1], taken.zoom()].map(Exact)
                    != [center[0], center[1], *zoom]
                {
                    return Err(conflict(format!(
                        "the view took {:?} at zoom {} for the recorded {center:?} at {zoom:?}",
                        taken.center(),
                        taken.zoom()
                    )));
                }
            }
            TimelineRecord::Select { organism } => {
                view.select(organism.map(OrganismId::new))?;
            }
            TimelineRecord::Region { region } => {
                view.set_region(region.map(Region::from))?;
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
                        step_to(&mut world, *tick, &at)?;
                        if let Publication::Busy { .. } = publisher.offer(&world)? {
                            return Err(conflict(format!(
                                "no render slot was free for tick {tick}"
                            )));
                        }
                        let lease = viewer.latest()?;
                        if lease.tick().get() != *tick {
                            return Err(conflict(format!(
                                "tick {tick} was published as tick {}",
                                lease.tick().get()
                            )));
                        }
                        epochs.insert(*tick, lease);
                    }
                }
                let shown = &epochs[current];
                let from = prior.map(|tick| &*epochs[&tick]);
                match record {
                    TimelineRecord::Show { .. } => view.show_between(from, shown, fraction.0)?,
                    TimelineRecord::Place { .. } => view.place_between(from, shown, fraction.0)?,
                    _ => view.advance(from, shown, fraction.0)?,
                };
                showing = *current;
                epochs.retain(|tick, _| last_use.get(tick).is_some_and(|&last| last > index));
            }
            TimelineRecord::Draw {
                frame,
                tick,
                wall_seconds,
            } => {
                if *tick != showing {
                    return Err(conflict(format!(
                        "frame {frame} was recorded showing tick {tick}, but the view shows tick \
                         {showing}"
                    )));
                }
                let job = view.job()?;
                let (pixels, _) = renderer
                    .render_three_d_rgba8(&job)
                    .map_err(|error| conflict(format!("render frame {frame}: {error}")))?;
                drawn(ReplayFrame {
                    frame: *frame,
                    tick: Tick::new(*tick),
                    wall_seconds: wall_seconds.0,
                    pixels: &pixels,
                })?;
                frames += 1;
            }
            TimelineRecord::Header { .. }
            | TimelineRecord::Submitted { .. }
            | TimelineRecord::Applied { .. }
            | TimelineRecord::End { .. } => {}
        }
    }
    drop(epochs);
    step_to(&mut world, *end, &at)?;
    let reached = world.digest().to_hex();
    if reached != *digest {
        return Err(conflict(format!(
            "the replay reached {reached} at tick {end}, not the Studio's {digest}"
        )));
    }
    Ok(Replayed {
        frames,
        tick: world.tick(),
        digest: reached,
        commands: applied.len(),
    })
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
