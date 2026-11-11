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
//!    [`Status::Inspected`];
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
//! The viewer holds the epoch shown and, while the display moves towards
//! it, the one before: [`DISPLAY_PAIR`] leases. At rest only the shown one
//! is held, so the publisher has every other slot.
//!
//! The world is only ever read through epochs, which are projections of
//! committed worlds: a selection is validated against the newest one by
//! identity, never by storage slot. Nothing on this thread waits for a
//! tick, a command or the session's lanes.

use flife_backend_metal::{BackendBudget, ControllerChoice};
use flife_core::ids::{OrganismId, SessionId};
use flife_core::memory::MemoryAdmission;
use flife_core::{CoreError, ValidatedConfig};
use flife_engine::{Arrangement, CPU_REFERENCE, NumericalProfile, World};
use flife_manim::{Motion, Style, Viewport, WorldView, hex_rgb};
use flife_render::{DISPLAY_PAIR, Epoch, Feed, Lease, Prior, Viewer};
use flife_runtime::SessionClosed;
use fmn_render::metal::{
    INPUT_QUEUE_CAPACITY, InputEvent, MetalReport, NativePreviewRenderer, PixelDensity,
    PresentOutcome, PresentationState, SurfaceMetrics,
};
use std::num::NonZeroUsize;
use std::time::Instant;

use crate::{CameraMove, Controls, Intent, Pacer, SessionThread, Status};

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
    /// The presentation outcome; `None` when the window was occluded or
    /// closed and nothing was rendered.
    pub outcome: Option<PresentOutcome>,
}

/// Frame counts for the Studio's life.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameTally {
    /// Frames run.
    pub frames: u64,
    /// Frames presented to a drawable.
    pub presented: u64,
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
}

/// The native Studio host.
#[derive(Debug)]
pub struct Studio {
    preview: NativePreviewRenderer,
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
}

impl Studio {
    /// Found `config`'s world, open its session on a session thread and a
    /// native window on this thread, and show the founding epoch.
    ///
    /// # Errors
    /// - [`CoreError::MissingCapability`] off the process main thread,
    ///   without Metal or a window server, or when the session thread
    ///   cannot start.
    /// - The founding, session and view errors of [`World::found`],
    ///   [`flife_runtime::open`] and [`WorldView::new`].
    /// - [`CoreError::BudgetExhausted`] when the world and its observers do
    ///   not fit `maximum_total_bytes` ([`crate::memory::admit`]).
    pub fn open(config: &ValidatedConfig, options: &StudioOptions) -> Result<Self, CoreError> {
        let model = config.config();
        let background =
            hex_rgb(&model.presentation.background).ok_or_else(|| CoreError::NumericalInvalid {
                quantity: "presentation background",
                detail: format!("`{}` is not #rrggbb", model.presentation.background),
            })?;
        let [width, height] = options
            .size
            .unwrap_or([model.presentation.width, model.presentation.height]);
        // The window first: without one there is nothing to run for.
        let mut preview = NativePreviewRenderer::new(width, height, options.title.clone())
            .map_err(|error| missing(format!("a native Metal preview window: {error}")))?;
        preview
            .set_resizable(options.resizable)
            .map_err(|error| missing(format!("a resizable preview window: {error}")))?;
        // One drawable pixel per device pixel: the window keeps its size in
        // points and each frame is drawn at the screen's density.
        preview
            .set_pixel_density(PixelDensity::Backing)
            .map_err(|error| missing(format!("a preview window at screen density: {error}")))?;
        let mut world = World::found(
            config,
            Arrangement::FreeList,
            NumericalProfile::reference_f32()?,
        )?;
        if let Some(workers) = options.cpu_workers {
            world.set_cpu_workers(workers)?;
        }
        // The observers are admitted before the backend (FL-043): the view
        // is made now, for the founding state, and the backend holds what
        // the budget leaves. Frames are drawn at the window's drawable.
        let founding = Epoch::standalone(&world)?;
        let mut view = WorldView::for_epoch(&founding, [width, height], background)?;
        view.set_style(options.style);
        let presentation = &model.presentation;
        view.set_bloom(
            presentation
                .bloom_enabled
                .then_some(presentation.bloom_strength),
        )?;
        view.set_point_radius(presentation.point_radius_pixels)?;
        let drawable = preview
            .surface_metrics()
            .ok()
            .filter(|metrics| metrics.drawable_width > 0 && metrics.drawable_height > 0)
            .map_or([width, height], |metrics| {
                [metrics.drawable_width, metrics.drawable_height]
            });
        let memory = crate::memory::admit(&world, options.controllers, &founding, &view, drawable)?;
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
        let (owner, handle) = flife_runtime::open(world, config, STUDIO_SESSION)?;
        let feed = handle.feed();
        // The shown epoch and, while the display moves, its prior.
        let viewer = feed.viewer(DISPLAY_PAIR)?;
        let founding = viewer.latest()?;
        view.show(&founding)?;
        let mut pacer = Pacer::new();
        pacer.arrive(Instant::now());
        let session = SessionThread::spawn(owner, handle)?;
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
            preview,
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
        })
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

    /// Run one frame.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] when the window cannot be polled or a
    /// frame cannot be rendered or presented, and the view's errors.
    pub fn frame(&mut self) -> Result<Frame, CoreError> {
        self.tally.frames += 1;
        self.events.clear();
        let state = self
            .preview
            .poll_input(&mut self.events)
            .map_err(|error| conflict("poll the studio window", error))?;
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
            self.statuses.push(status);
        }
        let statuses = self.statuses.len() - before;

        let style = self.controls.style();
        let restyled = style != self.view.style();
        if restyled {
            self.view.set_style(style);
            self.tally.restyles += 1;
        }

        let resized = match self.controls.drawable_size() {
            Some(size) => self.view.resize(size)?,
            None => false,
        };
        let redensed = match self.controls.metrics() {
            Some(metrics) if metrics.backing_scale.is_finite() && metrics.backing_scale > 0.0 => {
                self.view.set_pixels_per_point(metrics.backing_scale)?
            }
            _ => false,
        };
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
            }
        }
        let moved = self.view.set_view(viewport)?;
        if moved {
            self.tally.camera_moves += 1;
        }
        if let Some(choice) = picked {
            let selected = self.view.select(choice)?;
            self.tally.selections += 1;
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
        if new_epoch || restyled {
            // A newer epoch, or the pair again in the new style.
            self.view.show_between(prior, &self.current, fraction)?;
        } else if moved {
            // The pair placed in the new view, its display scales unmoved.
            self.view.place_between(prior, &self.current, fraction)?;
        } else if self.view.motion().is_some() {
            self.view.advance(prior, &self.current, fraction)?;
        }
        if self.view.motion().is_none() {
            // At rest: the publisher may have the prior's slot back.
            self.prior = None;
        }

        let outcome = if state == PresentationState::Visible {
            let job = self.view.job()?;
            let report = self
                .preview
                .render_three_d(&job)
                .map_err(|error| conflict("present a studio frame", error))?;
            self.tally.count_copies(&report.metal);
            match report.outcome {
                PresentOutcome::Presented => self.tally.presented += 1,
                PresentOutcome::Occluded => self.tally.occluded += 1,
            }
            Some(report.outcome)
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
            outcome,
        })
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

    /// Input the native queue dropped between polls.
    #[must_use]
    pub fn input_dropped(&self) -> u64 {
        self.preview.input_dropped()
    }

    /// The native surface's geometry now.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] once the window has closed.
    pub fn surface_metrics(&self) -> Result<SurfaceMetrics, CoreError> {
        self.preview
            .surface_metrics()
            .map_err(|error| conflict("read the studio surface", error))
    }

    /// How frames map to the window: [`PixelDensity::Backing`] from
    /// [`Studio::open`], so each frame is drawn at the screen's density.
    #[must_use]
    pub fn pixel_density(&self) -> PixelDensity {
        self.preview.pixel_density()
    }

    /// Post one real AppKit event into the application queue for this
    /// window; the next frame drains it like any other (test seam).
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] when AppKit refuses it.
    #[cfg(feature = "synthetic-input")]
    pub fn post(&mut self, input: &fmn_render::metal::SyntheticInput) -> Result<(), CoreError> {
        self.preview
            .post_synthetic(input)
            .map_err(|error| conflict("post synthetic studio input", error))
    }

    /// Close the window and the session, returning the committed world.
    ///
    /// # Errors
    /// The session's closing errors. The window is closed first either way.
    pub fn close(mut self) -> Result<StudioClosed, CoreError> {
        let window = self
            .preview
            .close()
            .map_err(|error| conflict("close the studio window", error));
        // The display's leases go before the session closes.
        drop(self.prior.take());
        drop(self.current);
        let input_dropped = self.preview.input_dropped();
        let session = self.session.close()?;
        window?;
        Ok(StudioClosed {
            session,
            frames: self.tally,
            input: self.controls.tally(),
            input_dropped,
        })
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
