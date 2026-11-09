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
//! 4. leases the newest render epoch, if newer than the one shown, lowers
//!    it into the particle cloud and returns the lease at once; after a
//!    style key it lowers the newest epoch again in the new style, so the
//!    change shows while paused too;
//! 5. renders the cloud with FrankenManim's Metal 3D route straight into
//!    the window's drawable, with no pixel readback.
//!
//! The world is only ever read through epochs. Nothing on this thread
//! waits for a tick, a command or the session's lanes.

use flife_backend_metal::ControllerChoice;
use flife_core::ids::{RenderEpoch, SessionId};
use flife_core::{CoreError, ValidatedConfig};
use flife_engine::{Arrangement, CPU_REFERENCE, NumericalProfile, World};
use flife_manim::{Style, WorldView, hex_rgb};
use flife_render::{Prior, Viewer};
use flife_runtime::SessionClosed;
use fmn_render::metal::{
    INPUT_QUEUE_CAPACITY, InputEvent, NativePreviewRenderer, PresentOutcome, PresentationState,
    SurfaceMetrics,
};
use std::num::NonZeroUsize;

use crate::{Controls, Intent, SessionThread, Status};

/// The session a Studio opens.
pub const STUDIO_SESSION: SessionId = SessionId::new(1);

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
    /// The epoch on screen.
    pub shown: Option<Prior>,
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
    /// Intents accepted.
    pub intents: u64,
    /// Intents refused.
    pub refused: u64,
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
    viewer: Viewer,
    view: WorldView,
    controls: Controls,
    events: Vec<InputEvent>,
    statuses: Vec<Status>,
    tally: FrameTally,
    window_closed: bool,
    /// Where due controllers are evaluated.
    controllers: String,
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
        let mut world = World::found(
            config,
            Arrangement::FreeList,
            NumericalProfile::reference_f32()?,
        )?;
        if let Some(workers) = options.cpu_workers {
            world.set_cpu_workers(workers)?;
        }
        world.set_controller_backend(options.controllers.backend(world.cpu_workers())?);
        let controllers = world
            .controller_report()
            .backend
            .clone()
            .unwrap_or_else(|| CPU_REFERENCE.to_owned());
        let (owner, handle) = flife_runtime::open(world, config, STUDIO_SESSION)?;
        let feed = handle.feed();
        // Each epoch is copied into the cloud and its lease returned before
        // the frame renders, so one lease is all this viewer ever holds.
        let viewer = feed.viewer(1)?;
        let founding = viewer.latest()?;
        let mut view = WorldView::for_epoch(&founding, [width, height], background)?;
        view.set_style(options.style);
        view.show(&founding)?;
        drop(founding);
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
        Ok(Self {
            preview,
            session,
            viewer,
            view,
            controls,
            events,
            statuses: Vec::new(),
            tally: FrameTally::default(),
            window_closed: false,
            controllers,
        })
    }

    /// Where due controllers are evaluated: a backend and device, or
    /// `cpu-reference`.
    #[must_use]
    pub fn controllers(&self) -> &str {
        &self.controllers
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

        let refitted = match self.controls.drawable_size() {
            Some(size) => self.view.resize(size)?,
            None => false,
        };
        if refitted {
            self.tally.refits += 1;
        }

        let seen = self
            .view
            .shown()
            .map_or(RenderEpoch::new(0), |shown| shown.id);
        let new_epoch = match self.viewer.latest_after(seen)? {
            Some(lease) => {
                self.view.show(&lease)?;
                true
            }
            None => false,
        };
        if new_epoch {
            self.tally.epochs += 1;
        } else if restyled {
            // The same (or a newer) epoch again, lowered in the new style.
            let lease = self.viewer.latest()?;
            self.view.show(&lease)?;
        }

        let outcome = if state == PresentationState::Visible {
            let job = self.view.job()?;
            let report = self
                .preview
                .render_three_d(&job)
                .map_err(|error| conflict("present a studio frame", error))?;
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
            shown: self.view.shown(),
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

fn missing(capability: String) -> CoreError {
    CoreError::MissingCapability { capability }
}

fn conflict(operation: &'static str, error: impl std::fmt::Display) -> CoreError {
    CoreError::StateConflict {
        operation,
        detail: error.to_string(),
    }
}
