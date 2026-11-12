//! The native Studio host: one window, one world, one frame at a time
//! (plan §13.6).
//!
//! [`Studio`] lives on the process main thread. Each [`Studio::frame`]:
//!
//! 1. drains the window's typed input (pointer, buttons, scroll, keys,
//!    modifiers, focus, capture loss, resize) through [`Controls`];
//! 2. offers the intents that makes to the session thread without
//!    blocking, and drains what the session reports;
//! 3. refits the viewport and camera when the content area changed;
//! 4. applies the camera moves the input made, in order (zoom about the
//!    pointer, pan, reset), and picks under a click through the view as it
//!    was at that click, among the shown epoch's organisms (FL-020); a
//!    pick that selects an organism offers the session an
//!    [`Intent::Inspect`] for it (FL-021), answered later as a
//!    [`Status::Inspected`], and the W key offers an
//!    [`Intent::DumpWeights`] for whatever is then selected, answered as a
//!    [`Status::Weights`];
//! 5. leases the newest render epoch, if newer than the one shown, and
//!    shows it one committed state late: its organisms move from where the
//!    previous epoch had them, over about the interval at which epochs
//!    arrive while the world runs, or [`MOTION_MAX`](crate::MOTION_MAX) for a step while
//!    paused ([`Pacer`]), and the selection stays only if that epoch still
//!    has its identity; after a style key or a camera move it lowers the
//!    pair again, so the change shows while paused too, and otherwise
//!    advances the motion;
//! 6. renders the cloud with FrankenManim's Metal 3D route straight into
//!    the window's drawable, with no pixel readback.
//!
//! An offscreen Studio ([`Surface::Offscreen`]) has no window: its input
//! is what [`Studio::post_input`] posts, the first frame reporting the
//! surface's geometry and focus as a window's first poll does, and step 6
//! renders through the same Metal 3D route to the route's RGBA8 surface and
//! reads it back ([`Studio::last_frame`]). Every other step is the same.
//!
//! The viewer holds the epoch shown and, while the display moves towards
//! it, the one before: [`DISPLAY_PAIR`] leases. At rest only the shown one
//! is held, so the publisher has every other slot.
//!
//! The world is only ever read through epochs, which are projections of
//! committed worlds: a selection is validated against the newest one by
//! identity, never by storage slot. Nothing on this thread waits for a
//! tick, a command or the session's lanes.
//!
//! With [`StudioOptions::run`] the session is kept in a run directory
//! ([`crate::keep`], FL-024): refused before any surface opens when the
//! directory holds anything, its checkpoint charged with the observers,
//! and the session opened through the run's command journal.

use flife_backend_metal::{BackendBudget, ControllerChoice};
use flife_cli::CheckpointAdmission;
use flife_core::ids::{OrganismId, SessionId};
use flife_core::memory::MemoryAdmission;
use flife_core::{CoreError, ValidatedConfig};
use flife_engine::{Arrangement, CPU_REFERENCE, NumericalProfile, World};
use flife_manim::{HUD_TEXT_POINTS, HudFont, Motion, Style, Viewport, WorldView, hex_rgb, linear};
use flife_render::{DISPLAY_PAIR, Epoch, Feed, Lease, Prior, Viewer};
use flife_runtime::SessionClosed;
use fmn_frame::FrameBuffer;
use fmn_render::ThreeDJob;
use fmn_render::metal::{
    INPUT_QUEUE_CAPACITY, InputEvent, MetalRenderer, MetalReport, NativePreviewRenderer,
    PixelDensity, PresentOutcome, PresentationState, SurfaceMetrics,
};
use std::collections::VecDeque;
use std::num::{NonZeroU32, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::keep::Keeping;

use crate::timeline::{Exact, TimelineRecord};
use crate::{
    Brush, CameraMove, Controls, Intent, Pacer, Preview, SessionThread, Status, Supplies, brush,
    material_for,
};

/// The session a Studio opens.
pub const STUDIO_SESSION: SessionId = SessionId::new(1);

/// How far from a click, in logical points, an organism is picked.
pub const PICK_RADIUS_POINTS: f64 = 8.0;

/// How the window opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StudioOptions {
    /// The window title.
    pub title: String,
    /// The content area in logical points; `None` for `[presentation]`'s
    /// width and height.
    pub size: Option<[u32; 2]>,
    /// Whether the user may resize the window.
    pub resizable: bool,
    /// Threads for the world's field and organism phases, replacing
    /// `[execution] cpu_workers`; no result depends on it.
    pub cpu_workers: Option<NonZeroUsize>,
    /// Whether the world starts running when the window opens, as if Space
    /// had been pressed.
    pub run_on_open: bool,
    /// Where due controllers are evaluated. No result depends on it.
    pub controllers: ControllerChoice,
    /// The style the window opens in; keys change it later.
    pub style: Style,
    /// Where frames go: a native window, or offscreen.
    pub surface: Surface,
    /// Whether to record the presentation timeline (FL-023): every change a
    /// frame makes to the view and every frame drawn, as
    /// [`TimelineRecord`]s, from [`Studio::take_recorded`].
    pub record: bool,
    /// A run directory to keep the session in (FL-024, [`crate::Keeping`]):
    /// absent or empty when the Studio opens. `None` keeps nothing.
    pub run: Option<PathBuf>,
}

/// Where a Studio's frames go.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Surface {
    /// A native window on the process main thread: input from AppKit,
    /// frames presented to its drawable at the screen's density.
    #[default]
    Window,
    /// No window, on any thread (FL-024's frame study): `size` points at
    /// `scale` device pixels a point, input posted with
    /// [`Studio::post_input`], each frame rendered through the same Metal
    /// route and read back. The window options (`title`, `resizable`) are
    /// unused.
    Offscreen {
        /// Device pixels a point, as a screen's backing scale.
        scale: NonZeroU32,
    },
}

impl Default for StudioOptions {
    fn default() -> Self {
        Self {
            title: "FrankenLife Studio".into(),
            size: None,
            resizable: true,
            cpu_workers: None,
            run_on_open: false,
            controllers: ControllerChoice::Auto,
            style: Style::default(),
            surface: Surface::Window,
            record: false,
            run: None,
        }
    }
}

/// What one frame did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    /// The window's state after draining its events.
    pub state: PresentationState,
    /// Typed input events drained.
    pub events: usize,
    /// Intents the session accepted into its lane.
    pub intents: usize,
    /// Intents refused because the lane was full or gone.
    pub refused: usize,
    /// Statuses drained from the session.
    pub statuses: usize,
    /// Whether the viewport was refitted to a new content area.
    pub refitted: bool,
    /// Whether a newer epoch was lowered.
    pub new_epoch: bool,
    /// Whether a style key changed the style this frame.
    pub restyled: bool,
    /// Whether a camera move changed the view this frame.
    pub moved: bool,
    /// The epoch shown: the newest lowered, which the display has reached
    /// unless it is still moving towards it.
    pub shown: Option<Prior>,
    /// The display's motion towards the shown epoch; `None` at rest.
    pub motion: Option<Motion>,
    /// The selected organism, which the epoch on screen has.
    pub selected: Option<OrganismId>,
    /// The world point under the pointer.
    pub hover: Option<[f64; 2]>,
    /// The armed brush, if any (FL-022).
    pub brush: Option<Brush>,
    /// While a brush is armed, what a click at the pointer would do now
    /// (plan §14.3): the stroke the click would make through the view as
    /// this frame left it ([`brush::preview`]).
    pub preview: Option<Preview>,
    /// The presentation outcome; `None` when the window was occluded or
    /// closed and nothing was rendered, and offscreen.
    pub outcome: Option<PresentOutcome>,
    /// Whether the frame was drawn: presented to the window's drawable, or
    /// rendered offscreen.
    pub drawn: bool,
}

/// Frame counts for the Studio's life.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameTally {
    /// Frames run.
    pub frames: u64,
    /// Frames presented to a drawable.
    pub presented: u64,
    /// Frames rendered offscreen and read back.
    pub rendered: u64,
    /// Frames skipped because the window was occluded.
    pub occluded: u64,
    /// Epochs lowered.
    pub epochs: u64,
    /// Viewport refits.
    pub refits: u64,
    /// Style changes.
    pub restyles: u64,
    /// Frames whose camera moves changed the view.
    pub camera_moves: u64,
    /// Clicks picked (an organism, or nothing).
    pub selections: u64,
    /// Brush clicks offered to the session as strokes (FL-022).
    pub strokes: u64,
    /// Brush clicks that made no stroke: in the letterbox margin, or the
    /// material brush with no selected organism's affinity known.
    pub unstroked: u64,
    /// Intents accepted.
    pub intents: u64,
    /// Intents refused.
    pub refused: u64,
    /// Bytes the Metal route copied to the device, over every frame rendered.
    pub upload_bytes: u64,
    /// The most bytes one frame copied to the device.
    pub peak_upload_bytes: u64,
    /// Frame-pixel bytes copied back to the host, over every frame rendered
    /// (none for frames presented to the window's drawable).
    pub readback_bytes: u64,
}

impl FrameTally {
    /// Frames drawn: presented to a drawable or rendered offscreen.
    #[must_use]
    pub const fn drawn(&self) -> u64 {
        self.presented.saturating_add(self.rendered)
    }

    /// Count one rendered frame's copies, as its Metal report gives them.
    pub fn count_copies(&mut self, metal: &MetalReport) {
        let upload = u64::try_from(metal.upload_bytes).unwrap_or(u64::MAX);
        let readback = u64::try_from(metal.readback_bytes).unwrap_or(u64::MAX);
        self.upload_bytes = self.upload_bytes.saturating_add(upload);
        self.peak_upload_bytes = self.peak_upload_bytes.max(upload);
        self.readback_bytes = self.readback_bytes.saturating_add(readback);
    }
}

/// What a closed Studio leaves.
#[derive(Debug)]
pub struct StudioClosed {
    /// The session owner's committed world and command book.
    pub session: SessionClosed,
    /// Frame counts.
    pub frames: FrameTally,
    /// Typed input seen, by kind.
    pub input: crate::InputTally,
    /// Input the native queue dropped between polls.
    pub input_dropped: u64,
    /// The statuses the session reported that no frame took, those
    /// reported while it closed last: a run's end, a kept session's last
    /// checkpoints, the close.
    pub statuses: Vec<Status>,
}

/// The native Studio host.
#[derive(Debug)]
pub struct Studio {
    target: Target,
    session: SessionThread,
    feed: Feed,
    viewer: Viewer,
    /// The shown epoch.
    current: Lease,
    /// While the display moves: the epoch it moves from.
    prior: Option<Lease>,
    pacer: Pacer,
    view: WorldView,
    controls: Controls,
    events: Vec<InputEvent>,
    statuses: Vec<Status>,
    tally: FrameTally,
    window_closed: bool,
    /// Where due controllers are evaluated.
    controllers: String,
    /// The peak-memory admission it opened under (FL-043).
    memory: MemoryAdmission,
    /// The run directory the session is kept in (FL-024).
    keeping: Option<Keeping>,
    /// The last inspected organism and the material it converts best
    /// (FL-022): the material brush's composition while it is selected.
    material: Option<(OrganismId, Option<[f64; flife_engine::MATERIAL_CHANNELS]>)>,
    /// `population.initial_energy`: each introduced organism's endowment.
    founder_energy: f64,
    /// The overlay's font (FL-024).
    font: HudFont,
    /// The text drawn over the world.
    overlay: String,
    /// Its swatches' sRGB colours, in order (FL-019's key).
    swatches: Vec<[u8; 3]>,
    /// The timeline recorded since it was last taken, while recording
    /// (FL-023).
    recorded: Option<Vec<TimelineRecord>>,
    /// When it opened: the timeline's presentation clock.
    opened: Instant,
}

impl Studio {
    /// Found `config`'s world, open its session on a session thread and a
    /// native window on this thread (or an offscreen surface), and show the
    /// founding epoch.
    ///
    /// # Errors
    /// - [`CoreError::MissingCapability`] for a window off the process main
    ///   thread or without a window server, without Metal, or when the
    ///   session thread cannot start.
    /// - [`CoreError::NumericalInvalid`] for an offscreen surface too large
    ///   to address.
    /// - The founding, session and view errors of [`World::found`],
    ///   [`flife_runtime::open`] and [`WorldView::new`].
    /// - [`CoreError::BudgetExhausted`] when the world and its observers do
    ///   not fit `maximum_total_bytes` ([`crate::memory::admit`]).
    /// - With [`StudioOptions::run`], before any surface opens:
    ///   [`Keeping::new`]'s refusals; then [`Keeping::open`]'s errors.
    pub fn open(config: &ValidatedConfig, options: &StudioOptions) -> Result<Self, CoreError> {
        let keeping = options
            .run
            .as_ref()
            .map(|run| Keeping::new(run.clone(), config))
            .transpose()?;
        let model = config.config();
        let background =
            hex_rgb(&model.presentation.background).ok_or_else(|| CoreError::NumericalInvalid {
                quantity: "presentation background",
                detail: format!("`{}` is not #rrggbb", model.presentation.background),
            })?;
        let [width, height] = options
            .size
            .unwrap_or([model.presentation.width, model.presentation.height]);
        // The surface first: without one there is nothing to run for.
        let target = match options.surface {
            Surface::Window => {
                let mut preview = NativePreviewRenderer::new(width, height, options.title.clone())
                    .map_err(|error| missing(format!("a native Metal preview window: {error}")))?;
                preview
                    .set_resizable(options.resizable)
                    .map_err(|error| missing(format!("a resizable preview window: {error}")))?;
                // One drawable pixel per device pixel: the window keeps its
                // size in points and each frame is drawn at the screen's
                // density.
                preview
                    .set_pixel_density(PixelDensity::Backing)
                    .map_err(|error| {
                        missing(format!("a preview window at screen density: {error}"))
                    })?;
                Target::Window(preview)
            }
            Surface::Offscreen { scale } => {
                Target::Offscreen(Offscreen::new([width, height], scale)?)
            }
        };
        let mut world = World::found(
            config,
            Arrangement::FreeList,
            NumericalProfile::reference_f32()?,
        )?;
        if let Some(workers) = options.cpu_workers {
            world.set_cpu_workers(workers)?;
        }
        // The inspector's descendants (FL-021): traced from the founding, so
        // every birth counts, and its room is reserved before the memory
        // admission charges the world's state.
        world.trace_ancestry()?;
        // The observers are admitted before the backend (FL-043): the view
        // is made now, for the founding state, and the backend holds what
        // the budget leaves. Frames are drawn at the window's drawable.
        let founding = Epoch::standalone(&world)?;
        let mut view = studio_view(
            config,
            &founding,
            [width, height],
            options.style,
            background,
        )?;
        let font = HudFont::bundled()?;
        let drawable = target
            .surface_metrics()
            .ok()
            .filter(|metrics| metrics.drawable_width > 0 && metrics.drawable_height > 0)
            .map_or([width, height], |metrics| {
                [metrics.drawable_width, metrics.drawable_height]
            });
        let memory = crate::memory::admit_kept(
            &world,
            options.controllers,
            &founding,
            &view,
            drawable,
            keeping.as_ref().map_or(0, Keeping::checkpoint_bytes),
        )?;
        drop(founding);
        let budget = BackendBudget {
            bytes: memory.backend,
            organisms: world.population().capacity() as u64,
        };
        world.set_controller_backend(options.controllers.backend(world.cpu_workers(), budget)?);
        let controllers = world
            .controller_report()
            .backend
            .clone()
            .unwrap_or_else(|| CPU_REFERENCE.to_owned());
        let (owner, handle, checkpoints) = match &keeping {
            Some(keeping) => {
                let (owner, handle, checkpoints) = keeping.open(world, config, STUDIO_SESSION)?;
                (owner, handle, Some(checkpoints))
            }
            None => {
                let (owner, handle) = flife_runtime::open(world, config, STUDIO_SESSION)?;
                (owner, handle, None)
            }
        };
        let feed = handle.feed();
        // The shown epoch and, while the display moves, its prior.
        let viewer = feed.viewer(DISPLAY_PAIR)?;
        let founding = viewer.latest()?;
        view.show(&founding)?;
        let mut pacer = Pacer::new();
        pacer.arrive(Instant::now());
        let session = SessionThread::spawn_kept(owner, handle, checkpoints)?;
        let mut events = Vec::new();
        events
            .try_reserve_exact(INPUT_QUEUE_CAPACITY)
            .map_err(|_| CoreError::AllocationFailed {
                buffer: "studio input events",
                bytes: u64::try_from(INPUT_QUEUE_CAPACITY * size_of::<InputEvent>())
                    .unwrap_or(u64::MAX),
            })?;
        if options.run_on_open {
            session.send(Intent::Resume)?;
        }
        let mut controls = Controls::new();
        controls.set_style(options.style);
        controls.reserve_moves(INPUT_QUEUE_CAPACITY)?;
        Ok(Self {
            target,
            session,
            feed,
            viewer,
            current: founding,
            prior: None,
            pacer,
            view,
            controls,
            events,
            statuses: Vec::new(),
            tally: FrameTally::default(),
            window_closed: false,
            controllers,
            memory,
            keeping,
            material: None,
            founder_energy: model.population.initial_energy,
            font,
            overlay: String::new(),
            swatches: Vec::new(),
            recorded: options.record.then(|| {
                vec![TimelineRecord::header(
                    config,
                    [width, height],
                    options.style,
                )]
            }),
            opened: Instant::now(),
        })
    }

    /// The timeline recorded since the last call, in order: empty unless
    /// [`StudioOptions::record`] is set. The first call's first record is
    /// the header; the caller adds the session's statuses
    /// ([`TimelineRecord::status`]) and, once closed, its end
    /// ([`TimelineRecord::end`]).
    pub fn take_recorded(&mut self) -> Vec<TimelineRecord> {
        self.recorded
            .as_mut()
            .map(std::mem::take)
            .unwrap_or_default()
    }

    /// Note `record` on the timeline while recording.
    fn note(&mut self, record: impl FnOnce() -> TimelineRecord) {
        if let Some(recorded) = &mut self.recorded {
            recorded.push(record());
        }
    }

    /// Draw `text` over the world from the next frame (FL-024's metric
    /// overlay; [`crate::overlay_text`] composes the Studio's): lines in
    /// IBM Plex Sans at [`HUD_TEXT_POINTS`] on the window's screen, over a
    /// dark plate in the drawable's top-left corner, each
    /// [`flife_manim::HUD_SWATCH`] in the next of `swatches` (sRGB; the
    /// colour key's, FL-019). An empty text draws nothing; the same text and
    /// swatches again do nothing.
    ///
    /// # Errors
    /// As [`HudFont::text_with_swatches`] and [`WorldView::set_overlay`];
    /// the overlay is unchanged on error.
    pub fn set_overlay(&mut self, text: &str, swatches: &[[u8; 3]]) -> Result<(), CoreError> {
        if text == self.overlay && swatches == self.swatches.as_slice() {
            return Ok(());
        }
        self.lay_out_overlay(text, swatches)?;
        text.clone_into(&mut self.overlay);
        swatches.clone_into(&mut self.swatches);
        Ok(())
    }

    /// The text drawn over the world.
    #[must_use]
    pub fn overlay(&self) -> &str {
        &self.overlay
    }

    /// Ask the session about the selected organism again (FL-021): its
    /// snapshot at the committed boundary arrives as a
    /// [`Status::Inspected`], as a selection's does. The Studio's binary
    /// refreshes the overlay's inspector panel this way, once a second while
    /// the world runs. `false` when nothing is selected or shown, or when
    /// the session refused it (counted as refused).
    pub fn inspect_selected(&mut self) -> bool {
        let (Some(organism), Some(shown)) = (self.view.selected(), self.view.shown()) else {
            return false;
        };
        let intent = Intent::Inspect {
            organism,
            seen: shown.tick,
        };
        if self.session.send(intent).is_ok() {
            self.tally.intents += 1;
            true
        } else {
            self.tally.refused += 1;
            self.controls.refused(intent);
            false
        }
    }

    fn lay_out_overlay(&mut self, text: &str, swatches: &[[u8; 3]]) -> Result<(), CoreError> {
        lay_out_overlay(&mut self.view, &self.font, text, swatches)?;
        self.note(|| TimelineRecord::Overlay {
            text: text.to_owned(),
            swatches: swatches.to_vec(),
        });
        Ok(())
    }

    /// Where due controllers are evaluated: a backend and device, or
    /// `cpu-reference`.
    #[must_use]
    pub fn controllers(&self) -> &str {
        &self.controllers
    }

    /// The peak-memory admission the Studio opened under (FL-043).
    #[must_use]
    pub const fn memory(&self) -> &MemoryAdmission {
        &self.memory
    }

    /// The run directory the session is kept in and its checkpoint
    /// admission (FL-024); `None` when nothing is kept.
    #[must_use]
    pub fn kept(&self) -> Option<(&Path, &CheckpointAdmission)> {
        self.keeping
            .as_ref()
            .map(|keeping| (keeping.run_dir(), keeping.admission()))
    }

    /// What the brushes draw on now: the selected organism's material,
    /// once its inspection has answered, and a founder's endowment.
    fn supplies(&self) -> Supplies {
        Supplies {
            material: self
                .material
                .filter(|(organism, _)| self.view.selected() == Some(*organism))
                .and_then(|(_, material)| material),
            founder_energy: self.founder_energy,
        }
    }

    /// Run one frame.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] when the window cannot be polled or a
    /// frame cannot be rendered, presented or read back, and the view's
    /// errors.
    pub fn frame(&mut self) -> Result<Frame, CoreError> {
        self.tally.frames += 1;
        self.events.clear();
        let state = self.target.poll_input(&mut self.events)?;
        let (mut intents, mut refused) = (0, 0);
        for event in &self.events {
            if let Some(intent) = self.controls.apply(event) {
                match self.session.send(intent) {
                    Ok(()) => intents += 1,
                    Err(_) => {
                        refused += 1;
                        self.controls.refused(intent);
                    }
                }
            }
        }
        self.tally.intents += intents as u64;
        self.tally.refused += refused as u64;
        if state == PresentationState::Closed {
            self.window_closed = true;
        }

        let before = self.statuses.len();
        for status in self.session.statuses() {
            self.controls.follow(&status);
            if let Status::Inspected {
                organism,
                snapshot: Some(snapshot),
                ..
            } = &status
            {
                self.material = Some((*organism, material_for(snapshot.traits.affinity())));
            }
            self.statuses.push(status);
        }
        let statuses = self.statuses.len() - before;

        let style = self.controls.style();
        let restyled = style != self.view.style();
        if restyled {
            self.view.set_style(style);
            self.tally.restyles += 1;
            self.note(|| TimelineRecord::Style {
                style: style.into(),
            });
        }

        let resized = match self.controls.drawable_size() {
            Some(size) => {
                let resized = self.view.resize(size)?;
                if resized {
                    self.note(|| TimelineRecord::Resize { drawable: size });
                }
                resized
            }
            None => false,
        };
        let redensed = match self.controls.metrics() {
            Some(metrics) if metrics.backing_scale.is_finite() && metrics.backing_scale > 0.0 => {
                let redensed = self.view.set_pixels_per_point(metrics.backing_scale)?;
                if redensed {
                    self.note(|| TimelineRecord::Density {
                        pixels_per_point: Exact(metrics.backing_scale),
                    });
                }
                redensed
            }
            _ => false,
        };
        if redensed && !self.overlay.is_empty() {
            // The text keeps its size in points on the new screen.
            let text = std::mem::take(&mut self.overlay);
            let swatches = std::mem::take(&mut self.swatches);
            let laid = self.lay_out_overlay(&text, &swatches);
            self.overlay = text;
            self.swatches = swatches;
            laid?;
        }
        let refitted = resized || redensed;
        if refitted {
            self.tally.refits += 1;
        }

        let logical = self
            .controls
            .metrics()
            .map(|metrics| [metrics.logical_width, metrics.logical_height]);
        let mut viewport = *self.view.viewport();
        let mut picked = None;
        let mut strokes = Vec::new();
        let mut weights = false;
        let supplies = self.supplies();
        for change in self.controls.take_moves() {
            match change {
                CameraMove::Zoom { anchor, factor } => {
                    if let Some(at) = logical
                        .and_then(|size| viewport.logical_to_drawable([anchor.x, anchor.y], size))
                    {
                        viewport = viewport.zoomed_about(at, factor)?;
                    }
                }
                CameraMove::Pan { delta } => {
                    if let Some(size) = logical
                        && let Some(per_point) = drawable_per_point(&viewport, size)
                    {
                        viewport =
                            viewport.panned([delta[0] * per_point[0], delta[1] * per_point[1]])?;
                    }
                }
                CameraMove::Reset => {
                    viewport = viewport.refit();
                }
                CameraMove::Select { at } => {
                    if let Some(size) = logical
                        && let Some(per_point) = drawable_per_point(&viewport, size)
                        && let Some(point) = viewport.logical_to_drawable([at.x, at.y], size)
                    {
                        let radius = PICK_RADIUS_POINTS * per_point[0].min(per_point[1]);
                        picked = Some(self.view.pick_in(&viewport, point, radius));
                    }
                }
                // Through the view as it was at the click, as a selection.
                CameraMove::Brush { at, brush } => {
                    match logical.and_then(|size| {
                        brush::stroke(
                            brush,
                            &viewport,
                            size,
                            self.current.field().cells(),
                            [at.x, at.y],
                            &supplies,
                        )
                    }) {
                        Some(stroke) => strokes.push(stroke),
                        None => self.tally.unstroked += 1,
                    }
                }
                CameraMove::Weights => weights = true,
            }
        }
        let moved = self.view.set_view(viewport)?;
        if moved {
            self.tally.camera_moves += 1;
            let taken = *self.view.viewport();
            self.note(|| TimelineRecord::View {
                center: taken.center().map(Exact),
                zoom: Exact(taken.zoom()),
            });
        }
        if let Some(choice) = picked {
            let selected = self.view.select(choice)?;
            self.tally.selections += 1;
            self.note(|| TimelineRecord::Select {
                organism: choice.map(OrganismId::get),
            });
            if let Some(organism) = selected
                && let Some(shown) = self.view.shown()
            {
                let intent = Intent::Inspect {
                    organism,
                    seen: shown.tick,
                };
                if self.session.send(intent).is_ok() {
                    intents += 1;
                    self.tally.intents += 1;
                } else {
                    refused += 1;
                    self.tally.refused += 1;
                    self.controls.refused(intent);
                }
            }
        }
        for stroke in strokes {
            let seen = self
                .view
                .shown()
                .map_or_else(|| self.current.tick(), |shown| shown.tick);
            let intent = Intent::Intervene { stroke, seen };
            self.tally.strokes += 1;
            if self.session.send(intent).is_ok() {
                intents += 1;
                self.tally.intents += 1;
            } else {
                refused += 1;
                self.tally.refused += 1;
                self.controls.refused(intent);
            }
        }
        // A weight dump (FL-021) of whatever is selected once the frame's
        // moves are taken; with nothing selected, none.
        if weights && let (Some(organism), Some(shown)) = (self.view.selected(), self.view.shown())
        {
            let intent = Intent::DumpWeights {
                organism,
                seen: shown.tick,
            };
            if self.session.send(intent).is_ok() {
                intents += 1;
                self.tally.intents += 1;
            } else {
                refused += 1;
                self.tally.refused += 1;
                self.controls.refused(intent);
            }
        }

        let now = Instant::now();
        let shown = self.current.id();
        let mut new_epoch = false;
        if self.feed.latest().is_some_and(|latest| latest.id > shown) {
            // The pair is this viewer's whole budget: release the prior
            // first. The shown epoch becomes the prior of the newer one.
            self.prior = None;
            if let Some(lease) = self.viewer.latest_after(shown)? {
                self.prior = Some(std::mem::replace(&mut self.current, lease));
                if self.controls.running() {
                    self.pacer.arrive(now);
                } else {
                    self.pacer.arrive_alone(now);
                }
                new_epoch = true;
                self.tally.epochs += 1;
            }
        }
        let fraction = if self.prior.is_some() {
            self.pacer.fraction(now)
        } else {
            1.0
        };
        let prior = self.prior.as_deref();
        let ticks = (
            prior.map(|prior| prior.tick().get()),
            self.current.tick().get(),
        );
        let display = if new_epoch || restyled {
            // A newer epoch, or the pair again in the new style.
            self.view.show_between(prior, &self.current, fraction)?;
            Some(TimelineRecord::Show {
                prior: ticks.0,
                current: ticks.1,
                fraction: Exact(fraction),
            })
        } else if moved {
            // The pair placed in the new view, its display scales unmoved.
            self.view.place_between(prior, &self.current, fraction)?;
            Some(TimelineRecord::Place {
                prior: ticks.0,
                current: ticks.1,
                fraction: Exact(fraction),
            })
        } else if self.view.motion().is_some() {
            self.view.advance(prior, &self.current, fraction)?;
            Some(TimelineRecord::Advance {
                prior: ticks.0,
                current: ticks.1,
                fraction: Exact(fraction),
            })
        } else {
            None
        };
        if let Some(display) = display {
            self.note(|| display);
        }
        let shown_tick = ticks.1;
        if self.view.motion().is_none() {
            // At rest: the publisher may have the prior's slot back.
            self.prior = None;
        }

        // What a click at the pointer would do, through the view as this
        // frame leaves it, and its region outlined on the world (FL-022,
        // plan §14.3).
        let preview = self.controls.brush().map(|brush| {
            brush::preview(
                brush,
                self.view.viewport(),
                logical,
                self.current.field().cells(),
                self.controls
                    .pointer()
                    .map(|pointer| [pointer.x, pointer.y]),
                &self.supplies(),
            )
        });
        let region = preview.as_ref().and_then(brush::preview_region);
        if self.view.set_region(region)? {
            self.note(|| TimelineRecord::Region {
                region: region.map(Into::into),
            });
        }

        let mut drawn = false;
        let outcome = if state == PresentationState::Visible {
            let job = self.view.job()?;
            let frame = self.tally.drawn();
            match &mut self.target {
                Target::Window(preview) => {
                    let report = preview
                        .render_three_d(&job)
                        .map_err(|error| conflict("present a studio frame", error))?;
                    self.tally.count_copies(&report.metal);
                    match report.outcome {
                        PresentOutcome::Presented => {
                            self.tally.presented += 1;
                            drawn = true;
                            if let Some(recorded) = &mut self.recorded {
                                recorded.push(TimelineRecord::Draw {
                                    frame,
                                    tick: shown_tick,
                                    wall_seconds: Exact(self.opened.elapsed().as_secs_f64()),
                                });
                            }
                        }
                        PresentOutcome::Occluded => self.tally.occluded += 1,
                    }
                    Some(report.outcome)
                }
                Target::Offscreen(offscreen) => {
                    let report = offscreen.render(&job)?;
                    self.tally.count_copies(&report);
                    self.tally.rendered += 1;
                    drawn = true;
                    if let Some(recorded) = &mut self.recorded {
                        recorded.push(TimelineRecord::Draw {
                            frame,
                            tick: shown_tick,
                            wall_seconds: Exact(self.opened.elapsed().as_secs_f64()),
                        });
                    }
                    None
                }
            }
        } else {
            if state == PresentationState::Occluded {
                self.tally.occluded += 1;
            }
            None
        };

        Ok(Frame {
            state,
            events: self.events.len(),
            intents,
            refused,
            statuses,
            refitted,
            new_epoch,
            restyled,
            moved,
            shown: self.view.shown(),
            motion: self.view.motion(),
            selected: self.view.selected(),
            hover: self.controls.hover(self.view.viewport()),
            brush: self.controls.brush(),
            preview,
            outcome,
            drawn,
        })
    }

    /// The newest epoch the session has published: its committed tick,
    /// which the display may not show yet.
    #[must_use]
    pub fn committed(&self) -> Option<Prior> {
        self.feed.latest()
    }

    /// Whether the window closed or a close was asked for.
    #[must_use]
    pub const fn should_close(&self) -> bool {
        self.window_closed || self.controls.close_requested()
    }

    /// Take every status drained so far, in order.
    pub fn take_statuses(&mut self) -> std::vec::Drain<'_, Status> {
        self.statuses.drain(..)
    }

    /// The input controls.
    #[must_use]
    pub const fn controls(&self) -> &Controls {
        &self.controls
    }

    /// The world view.
    #[must_use]
    pub const fn view(&self) -> &WorldView {
        &self.view
    }

    /// The pacer that times the display's motion.
    #[must_use]
    pub const fn pacer(&self) -> &Pacer {
        &self.pacer
    }

    /// The epoch shown and, while the display moves towards it, the epoch
    /// it moves from: the leases the Studio holds.
    #[must_use]
    pub fn display_pair(&self) -> (Option<&Epoch>, &Epoch) {
        (self.prior.as_deref(), &self.current)
    }

    /// Frame counts so far.
    #[must_use]
    pub const fn tally(&self) -> FrameTally {
        self.tally
    }

    /// Input the native queue (or the offscreen surface's) dropped between
    /// polls.
    #[must_use]
    pub fn input_dropped(&self) -> u64 {
        self.target.input_dropped()
    }

    /// The surface's geometry now: the window's, or the offscreen
    /// surface's as its last drained [`InputEvent::Resized`] gave it.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] once the window has closed.
    pub fn surface_metrics(&self) -> Result<SurfaceMetrics, CoreError> {
        self.target.surface_metrics()
    }

    /// How frames map to the surface: [`PixelDensity::Backing`] from
    /// [`Studio::open`], so each frame is drawn at the screen's density (or
    /// the offscreen surface's scale).
    #[must_use]
    pub fn pixel_density(&self) -> PixelDensity {
        match &self.target {
            Target::Window(preview) => preview.pixel_density(),
            Target::Offscreen(_) => PixelDensity::Backing,
        }
    }

    /// Whether the Studio draws offscreen ([`Surface::Offscreen`]).
    #[must_use]
    pub const fn is_offscreen(&self) -> bool {
        matches!(self.target, Target::Offscreen(_))
    }

    /// Post one typed input event to an offscreen Studio; the next frame
    /// drains it as a window's next poll would. Past
    /// [`INPUT_QUEUE_CAPACITY`] undrained events it is dropped and counted
    /// ([`Studio::input_dropped`]), as the native queue drops them.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] for a window, whose input is AppKit's.
    pub fn post_input(&mut self, event: InputEvent) -> Result<(), CoreError> {
        match &mut self.target {
            Target::Window(_) => Err(conflict(
                "post studio input",
                "a window's input comes from AppKit",
            )),
            Target::Offscreen(offscreen) => {
                offscreen.post(event);
                Ok(())
            }
        }
    }

    /// The last frame an offscreen Studio rendered, tight RGBA8 at the
    /// drawable's size; `None` for a window or before a frame.
    #[must_use]
    pub fn last_frame(&self) -> Option<&FrameBuffer> {
        match &self.target {
            Target::Window(_) => None,
            Target::Offscreen(offscreen) => offscreen.frame.as_ref(),
        }
    }

    /// Post one real AppKit event into the application queue for this
    /// window; the next frame drains it like any other (test seam).
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] when AppKit refuses it.
    #[cfg(feature = "synthetic-input")]
    pub fn post(&mut self, input: &fmn_render::metal::SyntheticInput) -> Result<(), CoreError> {
        match &mut self.target {
            Target::Window(preview) => preview
                .post_synthetic(input)
                .map_err(|error| conflict("post synthetic studio input", error)),
            Target::Offscreen(_) => Err(conflict(
                "post synthetic studio input",
                "an offscreen studio has no window",
            )),
        }
    }

    /// Close the window and the session, returning the committed world.
    ///
    /// # Errors
    /// The session's closing errors. The window is closed first either way.
    pub fn close(mut self) -> Result<StudioClosed, CoreError> {
        let window = self.target.close();
        // The display's leases go before the session closes.
        drop(self.prior.take());
        drop(self.current);
        let input_dropped = self.target.input_dropped();
        let (session, statuses) = self.session.close_reporting()?;
        window?;
        Ok(StudioClosed {
            session,
            frames: self.tally,
            input: self.controls.tally(),
            input_dropped,
            statuses,
        })
    }
}

/// The Studio's world view of `epoch`: `size` points on `background`, with
/// an overlay (admitted with the view, FL-024, FL-043), in `style`, with
/// the configuration's bloom and point radius. [`Studio::open`] makes it
/// so, and FL-023's [`crate::replay`] makes it again.
pub(crate) fn studio_view(
    config: &ValidatedConfig,
    epoch: &Epoch,
    size: [u32; 2],
    style: Style,
    background: [u8; 3],
) -> Result<WorldView, CoreError> {
    let mut view = WorldView::for_epoch(epoch, size, background)?;
    view.enable_overlay();
    view.set_style(style);
    let presentation = &config.config().presentation;
    view.set_bloom(
        presentation
            .bloom_enabled
            .then_some(presentation.bloom_strength),
    )?;
    view.set_point_radius(presentation.point_radius_pixels)?;
    Ok(view)
}

/// Lay `text` out over `view` in `font` at [`HUD_TEXT_POINTS`] on its
/// screen, each swatch in the next of `swatches`; empty draws nothing.
pub(crate) fn lay_out_overlay(
    view: &mut WorldView,
    font: &HudFont,
    text: &str,
    swatches: &[[u8; 3]],
) -> Result<(), CoreError> {
    let laid = if text.is_empty() {
        None
    } else {
        let em = HUD_TEXT_POINTS * view.viewport().pixels_per_point();
        let colors: Vec<_> = swatches.iter().map(|&rgb| linear(rgb)).collect();
        Some(font.text_with_swatches(text, em, &colors)?)
    };
    view.set_overlay(laid)
}

/// Where the Studio's frames go ([`Surface`]).
#[derive(Debug)]
enum Target {
    Window(NativePreviewRenderer),
    Offscreen(Offscreen),
}

impl Target {
    /// Drain the input since the last poll into `events`.
    fn poll_input(&mut self, events: &mut Vec<InputEvent>) -> Result<PresentationState, CoreError> {
        match self {
            Self::Window(preview) => preview
                .poll_input(events)
                .map_err(|error| conflict("poll the studio window", error)),
            Self::Offscreen(offscreen) => {
                for event in offscreen.posted.drain(..) {
                    if let InputEvent::Resized(metrics) = event {
                        offscreen.metrics = metrics;
                    }
                    events.push(event);
                }
                Ok(PresentationState::Visible)
            }
        }
    }

    fn surface_metrics(&self) -> Result<SurfaceMetrics, CoreError> {
        match self {
            Self::Window(preview) => preview
                .surface_metrics()
                .map_err(|error| conflict("read the studio surface", error)),
            Self::Offscreen(offscreen) => Ok(offscreen.metrics),
        }
    }

    fn input_dropped(&self) -> u64 {
        match self {
            Self::Window(preview) => preview.input_dropped(),
            Self::Offscreen(offscreen) => offscreen.dropped,
        }
    }

    fn close(&mut self) -> Result<(), CoreError> {
        match self {
            Self::Window(preview) => preview
                .close()
                .map_err(|error| conflict("close the studio window", error)),
            Self::Offscreen(_) => Ok(()),
        }
    }
}

/// An offscreen surface: FrankenManim's Metal renderer, the input posted
/// since the last frame, and the last frame read back.
#[derive(Debug)]
struct Offscreen {
    renderer: MetalRenderer,
    metrics: SurfaceMetrics,
    posted: VecDeque<InputEvent>,
    dropped: u64,
    frame: Option<FrameBuffer>,
}

impl Offscreen {
    /// `size` points at `scale` pixels a point, its first poll reporting
    /// the geometry and the focus, as a window's does once it is key.
    fn new(size: [u32; 2], scale: NonZeroU32) -> Result<Self, CoreError> {
        if !MetalRenderer::is_available() {
            return Err(missing(
                "a Metal device for offscreen studio frames".to_owned(),
            ));
        }
        let renderer = MetalRenderer::new()
            .map_err(|error| missing(format!("a Metal renderer for offscreen frames: {error}")))?;
        let pixels = |points: u32| {
            points
                .checked_mul(scale.get())
                .ok_or_else(|| CoreError::NumericalInvalid {
                    quantity: "offscreen studio drawable",
                    detail: format!("{points} points at {scale} pixels a point"),
                })
        };
        let metrics = SurfaceMetrics {
            logical_width: f64::from(size[0]),
            logical_height: f64::from(size[1]),
            backing_scale: f64::from(scale.get()),
            drawable_width: pixels(size[0])?,
            drawable_height: pixels(size[1])?,
        };
        let mut posted = VecDeque::new();
        posted
            .try_reserve_exact(INPUT_QUEUE_CAPACITY)
            .map_err(|_| CoreError::AllocationFailed {
                buffer: "offscreen studio input",
                bytes: u64::try_from(INPUT_QUEUE_CAPACITY * size_of::<InputEvent>())
                    .unwrap_or(u64::MAX),
            })?;
        posted.push_back(InputEvent::Resized(metrics));
        posted.push_back(InputEvent::FocusGained);
        Ok(Self {
            renderer,
            metrics,
            posted,
            dropped: 0,
            frame: None,
        })
    }

    fn post(&mut self, event: InputEvent) {
        if self.posted.len() < INPUT_QUEUE_CAPACITY {
            self.posted.push_back(event);
        } else {
            self.dropped += 1;
        }
    }

    /// Render `job` and read it back; the last frame goes first, so one is
    /// held at a time.
    fn render(&mut self, job: &ThreeDJob<'_>) -> Result<MetalReport, CoreError> {
        self.frame = None;
        let (frame, report) = self
            .renderer
            .render_three_d_rgba8(job)
            .map_err(|error| conflict("render an offscreen studio frame", error))?;
        self.frame = Some(frame);
        Ok(report)
    }
}

/// Drawable pixels per logical point on each axis, or `None` while the
/// content area is empty.
fn drawable_per_point(viewport: &Viewport, logical: [f64; 2]) -> Option<[f64; 2]> {
    if !logical
        .iter()
        .all(|length| length.is_finite() && *length > 0.0)
    {
        return None;
    }
    let [width, height] = viewport.drawable().map(f64::from);
    Some([width / logical[0], height / logical[1]])
}

fn missing(capability: String) -> CoreError {
    CoreError::MissingCapability { capability }
}

fn conflict(operation: &'static str, error: impl std::fmt::Display) -> CoreError {
    CoreError::StateConflict {
        operation,
        detail: error.to_string(),
    }
}
