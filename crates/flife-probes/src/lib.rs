//! FL-001 capability probes for FrankenLife's pinned FrankenSuite closure.
//!
//! Each probe exercises one capability through the same public API that the
//! later product crates will use. It compares the result with an oracle that
//! does not come from the implementation under test: NumPy 2.5.4 for the RNG
//! and array layouts, closed-form arithmetic for tensors and Metal, and a
//! precomputed completion order for the deterministic scheduler. A probe never
//! reports success for work it did not do (plan §18.5). A missing host
//! capability is [`Verdict::Unavailable`], and a capability missing upstream
//! is a [`Verdict::Gap`] that names its tracking bead.
//!
//! The closure audit ([`closure_probe`]) checks that `Cargo.lock` resolves
//! every FrankenSuite package once, at the commit `franken-stack.lock` records.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The probe seed. NumPy oracle values below are for this seed.
pub const PROBE_SEED: u64 = 42;

/// The outcome of one capability probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The capability ran and matched its independent oracle.
    Qualified,
    /// This host lacks the capability (for example no Metal device off
    /// macOS). Reported as such, never replaced by a fallback.
    Unavailable(String),
    /// FrankenLife needs a capability the pinned upstream API does not
    /// provide. The named bead tracks the host work that closes the gap.
    Gap {
        /// The bead that tracks the missing capability.
        bead: &'static str,
        /// What is missing, stated against the pinned API.
        detail: String,
    },
    /// The capability exists but errored or disagreed with its oracle.
    Failed(String),
}

impl Verdict {
    /// The stable lowercase label used in reports and `franken-stack.lock`.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Qualified => "qualified",
            Self::Unavailable(_) => "unavailable",
            Self::Gap { .. } => "gap",
            Self::Failed(_) => "failed",
        }
    }
}

impl fmt::Display for Verdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Qualified => f.write_str("qualified"),
            Self::Unavailable(reason) => write!(f, "unavailable: {reason}"),
            Self::Gap { bead, detail } => write!(f, "gap ({bead}): {detail}"),
            Self::Failed(reason) => write!(f, "failed: {reason}"),
        }
    }
}

/// One probe's verdict plus the evidence it observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeReport {
    /// Stable probe name.
    pub probe: &'static str,
    /// The outcome.
    pub verdict: Verdict,
    /// Observed facts, in the order they were checked.
    pub evidence: Vec<(&'static str, String)>,
}

impl ProbeReport {
    /// Whether the probe disagreed with its oracle or errored.
    #[must_use]
    pub fn failed(&self) -> bool {
        matches!(self.verdict, Verdict::Failed(_))
    }

    /// One self-contained JSON object (NDJSON line) for robot output.
    #[must_use]
    pub fn to_json_line(&self) -> String {
        let mut line = format!(
            "{{\"schema\":\"flife.probe.v1\",\"probe\":{},\"verdict\":{}",
            json_string(self.probe),
            json_string(self.verdict.label())
        );
        match &self.verdict {
            Verdict::Qualified => {}
            Verdict::Unavailable(reason) | Verdict::Failed(reason) => {
                line.push_str(",\"reason\":");
                line.push_str(&json_string(reason));
            }
            Verdict::Gap { bead, detail } => {
                line.push_str(",\"bead\":");
                line.push_str(&json_string(bead));
                line.push_str(",\"reason\":");
                line.push_str(&json_string(detail));
            }
        }
        line.push_str(",\"evidence\":{");
        for (index, (key, value)) in self.evidence.iter().enumerate() {
            if index > 0 {
                line.push(',');
            }
            line.push_str(&json_string(key));
            line.push(':');
            line.push_str(&json_string(value));
        }
        line.push_str("}}");
        line
    }
}

fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", u32::from(c)));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

type Evidence = Vec<(&'static str, String)>;

fn run(probe: &'static str, body: impl FnOnce(&mut Evidence) -> Result<(), String>) -> ProbeReport {
    let mut evidence = Vec::new();
    let verdict = match body(&mut evidence) {
        Ok(()) => Verdict::Qualified,
        Err(reason) => Verdict::Failed(reason),
    };
    ProbeReport {
        probe,
        verdict,
        evidence,
    }
}

fn require(condition: bool, describe: impl FnOnce() -> String) -> Result<(), String> {
    if condition { Ok(()) } else { Err(describe()) }
}

// ------------------------------------------------------------------ runtime

const LAB_TASKS: u64 = 6;

/// Asupersync: a production current-thread runtime runs a spawned task, and
/// the deterministic lab replays a seeded schedule exactly.
///
/// The lab's root region is created with the budget that FrankenTorch's
/// `ft-runtime` returns. That call type-checks only if FrankenTorch and
/// FrankenLife resolve the same asupersync package, so a split closure fails
/// to compile rather than at run time (plan §12.8).
#[must_use]
pub fn runtime_probe() -> ProbeReport {
    run("runtime", |evidence| {
        let runtime = asupersync::runtime::RuntimeBuilder::current_thread()
            .build()
            .map_err(|error| format!("current-thread runtime build failed: {error}"))?;
        let handle = runtime.handle();
        let answer = runtime.block_on(async move {
            match handle.try_spawn(async { 6_u64 * 7 }) {
                Ok(join) => Ok(join.await),
                Err(error) => Err(format!("spawn into the root region failed: {error}")),
            }
        })?;
        require(answer == 42, || {
            format!("spawned task returned {answer}, expected 42")
        })?;
        evidence.push((
            "production",
            "current_thread block_on joined a spawned task".into(),
        ));

        // Unique virtual deadlines fix the completion order: task `s` sleeps
        // (rank(s) + 1) µs with rank(s) = (5 s + seed) mod 6, so completions
        // arrive in rank order. For seed 42 that is [0, 5, 4, 3, 2, 1].
        let expected: Vec<u64> = {
            let mut by_rank = vec![0; 6];
            for sequence in 0..LAB_TASKS {
                let rank = (sequence * 5 + PROBE_SEED) % LAB_TASKS;
                by_rank[usize::try_from(rank).unwrap_or(0)] = sequence;
            }
            by_rank
        };
        let first = lab_completion_order(PROBE_SEED)?;
        let replay = lab_completion_order(PROBE_SEED)?;
        require(first == expected, || {
            format!("lab completion order {first:?}, expected {expected:?}")
        })?;
        require(replay == first, || {
            format!("lab replay {replay:?} differs from first run {first:?}")
        })?;
        evidence.push(("lab_order", format!("{first:?}")));
        evidence.push(("lab_replay", "identical".into()));
        evidence.push((
            "type_universe",
            "ft_runtime::asupersync_infinite_budget() accepted by this crate's LabRuntime".into(),
        ));
        Ok(())
    })
}

fn lab_completion_order(seed: u64) -> Result<Vec<u64>, String> {
    use asupersync::lab::{AutoAdvanceTermination, LabConfig, LabRuntime};
    use asupersync::time::sleep;
    use asupersync::types::{Budget, Time};

    let completed = Arc::new(Mutex::new(Vec::new()));
    let config = LabConfig::new(seed).max_steps(10_000).with_auto_advance();
    let mut runtime = LabRuntime::new(config);
    // FrankenTorch's Budget, used as this crate's Budget: the compile-time
    // proof of one asupersync package (see `runtime_probe`).
    let region = runtime
        .state
        .create_root_region(ft_runtime::asupersync_infinite_budget());
    for sequence in 0..LAB_TASKS {
        let rank = (sequence * 5 + seed) % LAB_TASKS;
        let completed = Arc::clone(&completed);
        let (task, _) = runtime
            .state
            .create_task(region, Budget::INFINITE, async move {
                sleep(Time::ZERO, Duration::from_nanos((rank + 1) * 1_000)).await;
                if let Ok(mut ledger) = completed.lock() {
                    ledger.push(sequence);
                }
            })
            .map_err(|error| format!("lab task creation failed: {error}"))?;
        runtime.scheduler.lock().schedule(task, 0);
    }
    let report = runtime.run_with_auto_advance();
    if report.termination != AutoAdvanceTermination::Quiescent {
        return Err(format!(
            "lab run ended {:?} instead of quiescent",
            report.termination
        ));
    }
    let order = completed
        .lock()
        .map_err(|_| "lab completion ledger was poisoned".to_owned())?
        .clone();
    Ok(order)
}

// ---------------------------------------------------------------------- RNG

/// `numpy.random.PCG64DXSM(42).random_raw(4)` (NumPy 2.5.4).
pub const NUMPY_PCG64DXSM_SEED42: [u64; 4] = [
    0xab1c_5033_8e63_481d,
    0x01bd_f91d_548d_1872,
    0xa872_905d_0418_d0a1,
    0x5f0a_8427_0b80_eabc,
];
/// `Generator(PCG64DXSM(42)).random()` = `0x1.5638a0671cc69p-1` (NumPy 2.5.4).
pub const NUMPY_RANDOM_SEED42_BITS: u64 = 0x3fe5_638a_0671_cc69;
/// `PCG64DXSM(42)` after `.advance(1000)`, then `.random_raw()` (NumPy 2.5.4).
pub const NUMPY_ADVANCE_1000_SEED42: u64 = 0x6cea_8362_21a1_4f5f;
/// `SeedSequence(42).generate_state(4)` (NumPy 2.5.4).
pub const NUMPY_SEED_SEQUENCE_42: [u32; 4] = [0xcd54_0ab7, 0x9f1e_2e6d, 0x79fb_94b6, 0xd578_73dc];
/// `SeedSequence(42, spawn_key=(7,)).generate_state(2)` (NumPy 2.5.4).
pub const NUMPY_SPAWNED_SEED_SEQUENCE_42_7: [u32; 2] = [0xcf67_d34a, 0xe080_24ce];

/// FrankenNumPy's PCG64DXSM and `SeedSequence` reproduce NumPy bit for bit,
/// including jump-ahead and spawned child streams (seed ensembles need both).
#[must_use]
pub fn rng_probe() -> ProbeReport {
    use fnp_random_core::{Pcg64Dxsm, SeedSequence};
    run("rng", |evidence| {
        let mut generator = Pcg64Dxsm::from_seed(PROBE_SEED);
        let words: [u64; 4] = std::array::from_fn(|_| generator.next_u64());
        require(words == NUMPY_PCG64DXSM_SEED42, || {
            format!("PCG64DXSM words {words:x?} differ from NumPy")
        })?;
        let hex: Vec<String> = words.iter().map(|word| format!("{word:#018x}")).collect();
        evidence.push(("pcg64dxsm_words", hex.join(",")));

        let draw = Pcg64Dxsm::from_seed(PROBE_SEED).next_f64();
        require(draw.to_bits() == NUMPY_RANDOM_SEED42_BITS, || {
            format!("first random() {draw:e} has bits {:#018x}", draw.to_bits())
        })?;
        evidence.push(("random_f64_bits", format!("{:#018x}", draw.to_bits())));

        let mut jumped = Pcg64Dxsm::from_seed(PROBE_SEED);
        jumped.advance(1000);
        let after = jumped.next_u64();
        require(after == NUMPY_ADVANCE_1000_SEED42, || {
            format!("advance(1000) then draw gave {after:#018x}")
        })?;
        evidence.push(("advance_1000", format!("{after:#018x}")));

        let state = SeedSequence::from_seed(PROBE_SEED).generate_state(4);
        require(state == NUMPY_SEED_SEQUENCE_42, || {
            format!("SeedSequence(42) state {state:x?} differs from NumPy")
        })?;
        let spawned = SeedSequence::with_spawn_key(PROBE_SEED, &[7]).generate_state(2);
        require(spawned == NUMPY_SPAWNED_SEED_SEQUENCE_42_7, || {
            format!("SeedSequence(42, spawn_key=(7,)) state {spawned:x?} differs from NumPy")
        })?;
        evidence.push((
            "seed_sequence",
            "seed and spawn-key states match NumPy".into(),
        ));
        Ok(())
    })
}

// -------------------------------------------------------------------- array

/// The reference field's array: 128 x 128 cells of energy plus 32 moment
/// channels (Appendix A), as f32.
pub const FIELD_SHAPE: [usize; 3] = [128, 128, 33];

/// FrankenNumPy lays out the reference field exactly as NumPy does:
/// `np.zeros((128, 128, 33), np.float32)` has C strides (16896, 132, 4),
/// F strides (4, 512, 65536) and 2,162,688 bytes, and
/// `np.broadcast_shapes((128, 1, 33), (1, 128, 1))` is (128, 128, 33).
#[must_use]
pub fn array_probe() -> ProbeReport {
    use fnp_dtype::DType;
    use fnp_ndarray::{MemoryOrder, NdLayout, broadcast_shape};
    run("array", |evidence| {
        let item = DType::F32.item_size();
        require(item == 4, || format!("f32 item size {item}"))?;
        let c = NdLayout::contiguous(FIELD_SHAPE.to_vec(), item, MemoryOrder::C)
            .map_err(|error| format!("C layout refused: {error:?}"))?;
        require(c.strides == [16_896, 132, 4], || {
            format!("C strides {:?}", c.strides)
        })?;
        let bytes = c
            .nbytes()
            .map_err(|error| format!("nbytes refused: {error:?}"))?;
        require(bytes == 2_162_688, || format!("nbytes {bytes}"))?;
        require(c.is_contiguous(), || "C layout is not contiguous".into())?;
        let f = NdLayout::contiguous(FIELD_SHAPE.to_vec(), item, MemoryOrder::F)
            .map_err(|error| format!("F layout refused: {error:?}"))?;
        require(f.strides == [4, 512, 65_536], || {
            format!("F strides {:?}", f.strides)
        })?;
        let shape = broadcast_shape(&[128, 1, 33], &[1, 128, 1])
            .map_err(|error| format!("broadcast refused: {error:?}"))?;
        require(shape == FIELD_SHAPE, || {
            format!("broadcast shape {shape:?}")
        })?;
        evidence.push(("c_strides", format!("{:?}", c.strides)));
        evidence.push(("f_strides", format!("{:?}", f.strides)));
        evidence.push(("nbytes", bytes.to_string()));
        Ok(())
    })
}

// ------------------------------------------------------------------- tensor

/// FrankenTorch's CPU tensor path computes a matrix product exactly:
/// `[[1,2,3],[4,5,6]] @ [[7,8],[9,10],[11,12]] = [[58,64],[139,154]]`.
#[must_use]
pub fn tensor_probe() -> ProbeReport {
    use ft_api::FrankenTorchSession;
    use ft_core::ExecutionMode;
    run("tensor", |evidence| {
        let mut session = FrankenTorchSession::new(ExecutionMode::Strict);
        let lhs = session
            .tensor_variable(vec![1., 2., 3., 4., 5., 6.], vec![2, 3], false)
            .map_err(|error| format!("lhs tensor: {error:?}"))?;
        let rhs = session
            .tensor_variable(vec![7., 8., 9., 10., 11., 12.], vec![3, 2], false)
            .map_err(|error| format!("rhs tensor: {error:?}"))?;
        let product = session
            .tensor_matmul(lhs, rhs)
            .map_err(|error| format!("matmul: {error:?}"))?;
        let shape = session
            .tensor_shape(product)
            .map_err(|error| format!("shape: {error:?}"))?;
        let values = session
            .tensor_values(product)
            .map_err(|error| format!("values: {error:?}"))?;
        require(shape == [2, 2], || format!("product shape {shape:?}"))?;
        let exact = values.len() == 4
            && values
                .iter()
                .zip([58.0_f64, 64.0, 139.0, 154.0])
                .all(|(got, want)| got.to_bits() == want.to_bits());
        require(exact, || format!("product values {values:?}"))?;
        evidence.push(("matmul", format!("{values:?}")));
        Ok(())
    })
}

// -------------------------------------------------------------- Manim frame

/// Dots in the Manim probe frame.
pub const FRAME_DOTS: usize = 256;

struct ProbeCloud {
    points: Vec<[f64; 3]>,
}

impl fmn::prelude::SceneConstruct for ProbeCloud {
    fn construct(&mut self, stage: &mut fmn::prelude::Stage<'_>) -> fmn::Result<()> {
        use fmn::prelude::{DotCloud, YELLOW};
        if self.points.is_empty() {
            return Ok(());
        }
        let cloud = DotCloud::new(self.points.iter().copied())
            .colored(YELLOW, 1.0)
            .with_radius(0.06);
        stage.add(cloud)?;
        Ok(())
    }
}

fn probe_points() -> Vec<[f64; 3]> {
    let mut generator = fnp_random_core::Pcg64Dxsm::from_seed(PROBE_SEED);
    (0..FRAME_DOTS)
        .map(|_| {
            let x = generator.next_f64() * 12.0 - 6.0;
            let y = generator.next_f64() * 6.0 - 3.0;
            [x, y, 0.0]
        })
        .collect()
}

struct Rendered {
    digest: String,
    bytes: u64,
    frames: u64,
}

fn render_cloud(path: &Path, points: Vec<[f64; 3]>) -> Result<Rendered, String> {
    use fmn::prelude::{DeterminismMode, RenderFormat, RenderOptions, render};
    let mut options =
        RenderOptions::new(path).map_err(|error| format!("render options: {error}"))?;
    options.format = RenderFormat::Png;
    options.config.camera.resolution = (320, 180);
    options.config.determinism.mode = DeterminismMode::Certified;
    options.config.determinism.seed = PROBE_SEED;
    let camera = options
        .camera_config()
        .map_err(|error| format!("camera config: {error}"))?;
    options.camera = Some(camera);
    let mut scene = ProbeCloud { points };
    let report = render(&mut scene, options).map_err(|error| format!("render: {error}"))?;
    Ok(Rendered {
        digest: report.artifact.digest.to_hex(),
        bytes: report.artifact.bytes,
        frames: report.artifact.frame_count,
    })
}

/// Native FrankenManim renders a seeded 256-dot `DotCloud` (positions from
/// the probe's PCG64DXSM stream) through the public `fmn::render` camera
/// route in certified mode, and a second render reproduces it byte for byte.
/// A control render of the same camera with no dots must differ, so a blank
/// or dot-independent frame cannot pass.
///
/// `out_dir` must exist; publication is no-clobber, so the file names carry
/// `tag` and the process id.
#[must_use]
pub fn manim_frame_probe(out_dir: &Path, tag: &str) -> ProbeReport {
    run("manim_frame", |evidence| {
        let pid = std::process::id();
        let first = render_cloud(
            &out_dir.join(format!("flife-probe-{tag}-{pid}-a.png")),
            probe_points(),
        )?;
        let second = render_cloud(
            &out_dir.join(format!("flife-probe-{tag}-{pid}-b.png")),
            probe_points(),
        )?;
        let control = render_cloud(
            &out_dir.join(format!("flife-probe-{tag}-{pid}-empty.png")),
            Vec::new(),
        )?;
        require(first.frames == 1, || {
            format!("{} frames published", first.frames)
        })?;
        require(first.bytes > 0, || "empty PNG".into())?;
        require(first.digest == second.digest, || {
            format!(
                "re-render digest {} differs from {}",
                second.digest, first.digest
            )
        })?;
        require(control.digest != first.digest, || {
            "the dot cloud left the frame unchanged from an empty scene".into()
        })?;
        evidence.push(("dots", FRAME_DOTS.to_string()));
        evidence.push(("png_bytes", first.bytes.to_string()));
        evidence.push(("png_sha256", first.digest));
        evidence.push(("rerender", "byte-identical".into()));
        evidence.push(("empty_control_sha256", control.digest));
        Ok(())
    })
}

// -------------------------------------------------------------------- Metal

const AFFINE_MSL: &str = r"
#include <metal_stdlib>
using namespace metal;
kernel void flife_probe_affine(device float *out [[buffer(0)]],
                               const device float *in [[buffer(1)]],
                               uint index [[thread_position_in_grid]]) {
    out[index] = in[index] * 2.0f + 1.0f;
}
";

/// Elements in the Metal probe dispatch.
pub const METAL_ELEMENTS: usize = 256;

/// FrankenTorch's generic Metal compute gateway compiles caller MSL, binds
/// unified-memory buffers, dispatches and reads back `2x + 1` exactly for
/// x = 0..255. Off macOS, or without a device, the verdict is unavailable.
#[must_use]
pub fn metal_probe() -> ProbeReport {
    use ft_kernel_metal::Error as MetalError;
    use ft_kernel_metal::compute::{Gateway, Grid};
    let gateway = match Gateway::open() {
        Ok(gateway) => gateway,
        Err(MetalError::Unavailable) => {
            return ProbeReport {
                probe: "metal",
                verdict: Verdict::Unavailable("no Metal device on this host".into()),
                evidence: Vec::new(),
            };
        }
        Err(error) => {
            return ProbeReport {
                probe: "metal",
                verdict: Verdict::Failed(format!("gateway open failed: {error}")),
                evidence: Vec::new(),
            };
        }
    };
    run("metal", |evidence| {
        let library = gateway
            .library(AFFINE_MSL)
            .map_err(|error| format!("MSL compile: {error}"))?;
        let pipeline = library
            .pipeline("flife_probe_affine")
            .map_err(|error| format!("pipeline: {error}"))?;
        let input: Vec<f32> = (0..METAL_ELEMENTS)
            .map(|i| f32::from(u16::try_from(i).unwrap_or(u16::MAX)))
            .collect();
        let source = gateway
            .buffer_f32(&input)
            .map_err(|error| format!("input buffer: {error}"))?;
        let output = gateway
            .buffer_zeroed(METAL_ELEMENTS * 4)
            .map_err(|error| format!("output buffer: {error}"))?;
        gateway
            .dispatch(
                &pipeline,
                &[&output, &source],
                Grid::linear(METAL_ELEMENTS / 64, 64),
            )
            .map_err(|error| format!("dispatch: {error}"))?;
        let mut result = vec![0.0_f32; METAL_ELEMENTS];
        output
            .read_f32(&mut result)
            .map_err(|error| format!("readback: {error}"))?;
        let mismatch = input
            .iter()
            .zip(&result)
            .position(|(x, y)| (x * 2.0 + 1.0).to_bits() != y.to_bits());
        require(mismatch.is_none(), || {
            format!("GPU result differs at element {mismatch:?}")
        })?;
        evidence.push(("device", gateway.device_name()));
        evidence.push(("unified_memory", gateway.has_unified_memory().to_string()));
        evidence.push((
            "thread_execution_width",
            pipeline.thread_execution_width().to_string(),
        ));
        evidence.push(("elements", METAL_ELEMENTS.to_string()));
        Ok(())
    })
}

// ------------------------------------------------------------- native input

/// The bead that built the native input host (FL-018).
pub const INPUT_HOST_BEAD: &str = "flife-jdf";

/// Every state the pinned presenter can report. The match has no wildcard,
/// so an upstream variant breaks this build and forces requalification.
#[must_use]
pub fn surface_state_name(state: ft_kernel_metal::presentation::PresentationState) -> &'static str {
    use ft_kernel_metal::presentation::PresentationState;
    match state {
        PresentationState::Visible => "visible",
        PresentationState::Occluded => "occluded",
        PresentationState::Closed => "closed",
    }
}

/// The kind of every typed input event the pinned presenter can deliver.
/// The match has no wildcard, so an upstream variant breaks this build and
/// forces requalification of [`native_input_probe`] and the Studio controls.
#[must_use]
pub fn input_event_name(event: &ft_kernel_metal::presentation::InputEvent) -> &'static str {
    use ft_kernel_metal::presentation::InputEvent;
    match event {
        InputEvent::PointerMoved { .. } => "pointer-moved",
        InputEvent::Button { .. } => "button",
        InputEvent::Scroll { .. } => "scroll",
        InputEvent::Key { .. } => "key",
        InputEvent::ModifiersChanged { .. } => "modifiers-changed",
        InputEvent::FocusGained => "focus-gained",
        InputEvent::FocusLost => "focus-lost",
        InputEvent::PointerCaptureLost { .. } => "pointer-capture-lost",
        InputEvent::Resized(_) => "resized",
    }
}

/// The pinned presenter's typed input API, checked statically: one value of
/// each [`InputEvent`](ft_kernel_metal::presentation::InputEvent) kind
/// decodes through [`input_event_name`], and the queue is bounded. The
/// runtime half, a real window delivering real events, is
/// [`native_window_probe`] and FL-018's live Studio test (plan §13.1, S21).
#[must_use]
pub fn native_input_probe() -> ProbeReport {
    use ft_kernel_metal::presentation::{
        ButtonSet, INPUT_QUEUE_CAPACITY, InputEvent, LogicalPoint, Modifiers, PointerButton,
        PresentationState, ScrollUnit, SurfaceMetrics,
    };
    let at = LogicalPoint { x: 1.0, y: 2.0 };
    let none = Modifiers::NONE;
    let events = [
        InputEvent::PointerMoved {
            position: at,
            buttons: ButtonSet::EMPTY,
            modifiers: none,
        },
        InputEvent::Button {
            button: PointerButton::Primary,
            pressed: true,
            position: at,
            clicks: 1,
            modifiers: none,
        },
        InputEvent::Scroll {
            delta: [0.0, 1.0],
            unit: ScrollUnit::Points,
            position: at,
            modifiers: none,
        },
        InputEvent::Key {
            code: 49,
            text: " ".into(),
            pressed: true,
            repeat: false,
            modifiers: none,
        },
        InputEvent::ModifiersChanged { modifiers: none },
        InputEvent::FocusGained,
        InputEvent::FocusLost,
        InputEvent::PointerCaptureLost {
            released: ButtonSet::EMPTY.with(PointerButton::Primary),
        },
        InputEvent::Resized(SurfaceMetrics {
            logical_width: 320.0,
            logical_height: 180.0,
            backing_scale: 2.0,
            drawable_width: 320,
            drawable_height: 180,
        }),
    ];
    let kinds: Vec<&str> = events.iter().map(input_event_name).collect();
    let states = [
        PresentationState::Visible,
        PresentationState::Occluded,
        PresentationState::Closed,
    ]
    .map(surface_state_name);
    let mut distinct = kinds.clone();
    distinct.sort_unstable();
    distinct.dedup();
    let verdict = if distinct.len() == events.len() && INPUT_QUEUE_CAPACITY > 0 {
        Verdict::Qualified
    } else {
        Verdict::Failed(format!(
            "{} input kinds decode to {} names; queue capacity {INPUT_QUEUE_CAPACITY}",
            events.len(),
            distinct.len()
        ))
    };
    ProbeReport {
        probe: "native_input",
        verdict,
        evidence: vec![
            ("input_events", kinds.join(",")),
            ("queue_capacity", INPUT_QUEUE_CAPACITY.to_string()),
            ("presentation_states", states.join(",")),
        ],
    }
}

/// Open a real native preview window, poll its typed input for `polls`
/// frames and close it. Qualified when the window reports its real content
/// geometry (a `resized` event with the requested logical size) and every
/// event decodes; the evidence records each kind seen, focus included. It
/// must run on the main thread (a binary's `main`, not a test harness
/// thread), and it opens a window.
#[must_use]
pub fn native_window_probe(polls: u32) -> ProbeReport {
    use ft_kernel_metal::compute::Gateway;
    use ft_kernel_metal::presentation::{
        InputEvent, NativePresenter, PresentationConfig, PresentationError,
    };
    const SIZE: [u32; 2] = [320, 180];
    let gateway = match Gateway::open() {
        Ok(gateway) => gateway,
        Err(error) => {
            return ProbeReport {
                probe: "native_window",
                verdict: Verdict::Unavailable(format!("no Metal gateway: {error}")),
                evidence: Vec::new(),
            };
        }
    };
    let config = PresentationConfig::new(SIZE[0], SIZE[1], "FrankenLife input probe");
    let mut presenter = match NativePresenter::open(&gateway, config) {
        Ok(presenter) => presenter,
        Err(PresentationError::Unavailable) => {
            return ProbeReport {
                probe: "native_window",
                verdict: Verdict::Unavailable("native presentation unavailable".into()),
                evidence: Vec::new(),
            };
        }
        Err(error) => {
            return ProbeReport {
                probe: "native_window",
                verdict: Verdict::Failed(format!("presenter open failed: {error}")),
                evidence: Vec::new(),
            };
        }
    };
    let mut observed: BTreeMap<&'static str, u32> = BTreeMap::new();
    let mut kinds: BTreeMap<&'static str, u32> = BTreeMap::new();
    let mut geometry = None;
    let mut events = Vec::new();
    let mut failure = None;
    for _ in 0..polls {
        events.clear();
        match presenter.poll_input(&mut events) {
            Ok(state) => *observed.entry(surface_state_name(state)).or_default() += 1,
            Err(error) => {
                failure = Some(format!("poll_input failed: {error}"));
                break;
            }
        }
        for event in &events {
            *kinds.entry(input_event_name(event)).or_default() += 1;
            if let InputEvent::Resized(metrics) = event {
                geometry = Some(*metrics);
            }
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    let dropped = presenter.input_dropped();
    let closed = presenter.close();
    let tally = |counts: &BTreeMap<&'static str, u32>| {
        counts
            .iter()
            .map(|(name, count)| format!("{name}x{count}"))
            .collect::<Vec<_>>()
            .join(",")
    };
    let mut evidence = vec![
        ("polls", polls.to_string()),
        ("observed_states", tally(&observed)),
        ("typed_input_events", tally(&kinds)),
        ("input_dropped", dropped.to_string()),
    ];
    if let Some(metrics) = geometry {
        evidence.push((
            "geometry",
            format!(
                "{}x{} points at backing scale {}",
                metrics.logical_width, metrics.logical_height, metrics.backing_scale
            ),
        ));
    }
    let requested = geometry.is_some_and(|metrics| {
        metrics.logical_width == f64::from(SIZE[0]) && metrics.logical_height == f64::from(SIZE[1])
    });
    let verdict = match (failure, closed) {
        (Some(reason), _) => Verdict::Failed(reason),
        (None, Err(error)) => Verdict::Failed(format!("close failed: {error}")),
        (None, Ok(())) if requested && dropped == 0 => Verdict::Qualified,
        (None, Ok(())) => Verdict::Failed(format!(
            "the window reported geometry {geometry:?}, not {SIZE:?} points, with {dropped} \
             events dropped"
        )),
    };
    ProbeReport {
        probe: "native_window",
        verdict,
        evidence,
    }
}

// ------------------------------------------------------------------ closure

/// One `[[package]]` record from `Cargo.lock`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedPackage {
    /// Package name.
    pub name: String,
    /// Package version.
    pub version: String,
    /// The `source` line; `None` for workspace members.
    pub source: Option<String>,
}

/// Parse the `[[package]]` records of a `Cargo.lock`.
#[must_use]
pub fn parse_cargo_lock(text: &str) -> Vec<LockedPackage> {
    let mut packages = Vec::new();
    let mut current: Option<LockedPackage> = None;
    for line in text.lines() {
        if line == "[[package]]" {
            packages.extend(current.take());
            current = Some(LockedPackage {
                name: String::new(),
                version: String::new(),
                source: None,
            });
        } else if line.starts_with('[') {
            packages.extend(current.take());
        } else if let Some(package) = current.as_mut() {
            if let Some(value) = quoted_value(line, "name") {
                package.name = value;
            } else if let Some(value) = quoted_value(line, "version") {
                package.version = value;
            } else if let Some(value) = quoted_value(line, "source") {
                package.source = Some(value);
            }
        }
    }
    packages.extend(current);
    packages
}

fn quoted_value(line: &str, key: &str) -> Option<String> {
    let rest = line.strip_prefix(key)?.strip_prefix(" = \"")?;
    rest.strip_suffix('"').map(str::to_owned)
}

/// The commits `franken-stack.lock` records: `[repos]` and `[patches]` rows,
/// keyed by repository name.
#[must_use]
pub fn parse_stack_pins(text: &str) -> BTreeMap<String, String> {
    let mut pins = BTreeMap::new();
    let mut section = "";
    for line in text.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = if name == "repos" || name == "patches" {
                "pins"
            } else {
                ""
            };
            continue;
        }
        if section == "pins" {
            let mut fields = line.split('\t');
            if let (Some(repo), Some(commit)) = (fields.next(), fields.next()) {
                pins.insert(repo.to_owned(), commit.to_owned());
            }
        }
    }
    pins
}

/// Suite packages that must appear exactly once, with a git source.
pub const SINGLE_COPY_PACKAGES: &[&str] = &[
    "asupersync",
    "franken-decision",
    "franken-evidence",
    "franken-kernel",
    "fmn",
    "fmn-core",
    "fmn-render",
    "fnp-dtype",
    "fnp-ndarray",
    "fnp-random",
    "fnp-random-core",
    "frankentorch-api",
    "frankentorch-core",
    "frankentorch-runtime",
    "ft-kernel-metal",
    "block",
];

const SUITE_OWNER: &str = "github.com/Dicklesworthstone/";

/// Split a git source into (repository name, resolved commit).
fn git_repo_and_commit(source: &str) -> Option<(&str, &str)> {
    let rest = source.strip_prefix("git+https://")?;
    let (url, commit) = rest.split_once('#')?;
    let path = url.split('?').next()?;
    let repo = path.strip_prefix(SUITE_OWNER)?;
    Some((repo.trim_end_matches(".git"), commit))
}

/// Every FrankenSuite git source in `Cargo.lock` resolves at the commit
/// `franken-stack.lock` records for its repository, no suite repository is
/// resolved at two commits, and each [`SINGLE_COPY_PACKAGES`] entry appears
/// exactly once with a git source. This is the plan's "one dependency
/// universe" (§12.8): it also proves the root patches took effect, since an
/// unpatched crates.io asupersync or `block` would be a second copy or a
/// registry source.
#[must_use]
pub fn closure_probe(cargo_lock: &str, stack_lock: &str) -> ProbeReport {
    run("closure", |evidence| {
        let packages = parse_cargo_lock(cargo_lock);
        require(packages.len() > 50, || {
            format!("only {} packages parsed from Cargo.lock", packages.len())
        })?;
        let pins = parse_stack_pins(stack_lock);
        let mut problems = Vec::new();
        let mut resolved: BTreeMap<&str, &str> = BTreeMap::new();
        for package in &packages {
            let Some(source) = package.source.as_deref() else {
                continue;
            };
            if !source.contains(SUITE_OWNER) {
                continue;
            }
            let Some((repo, commit)) = git_repo_and_commit(source) else {
                problems.push(format!(
                    "{} has a non-git suite source {source}",
                    package.name
                ));
                continue;
            };
            match pins.get(repo) {
                None => problems.push(format!(
                    "{} resolves repository {repo}, which franken-stack.lock does not pin",
                    package.name
                )),
                Some(pin) if pin != commit => problems.push(format!(
                    "{} resolves {repo}@{commit} but franken-stack.lock pins {pin}",
                    package.name
                )),
                Some(_) => {}
            }
            if let Some(previous) = resolved.insert(repo, commit)
                && previous != commit
            {
                problems.push(format!("{repo} resolves at both {previous} and {commit}"));
            }
        }
        for name in SINGLE_COPY_PACKAGES {
            let copies: Vec<&LockedPackage> = packages.iter().filter(|p| p.name == *name).collect();
            match copies.as_slice() {
                [only]
                    if only
                        .source
                        .as_deref()
                        .is_some_and(|s| s.starts_with("git+")) => {}
                [only] => problems.push(format!(
                    "{name} resolves from {:?}, not a pinned git source",
                    only.source
                )),
                [] => problems.push(format!("{name} is absent from Cargo.lock")),
                many => problems.push(format!("{name} has {} copies in Cargo.lock", many.len())),
            }
        }
        if !problems.is_empty() {
            return Err(problems.join("; "));
        }
        evidence.push(("packages", packages.len().to_string()));
        evidence.push((
            "suite_repos",
            resolved
                .iter()
                .map(|(repo, commit)| format!("{repo}@{}", commit.get(..8).unwrap_or(commit)))
                .collect::<Vec<_>>()
                .join(","),
        ));
        evidence.push(("single_copy", SINGLE_COPY_PACKAGES.len().to_string()));
        Ok(())
    })
}

/// The committed `Cargo.lock` this crate was built against.
pub const CARGO_LOCK: &str = include_str!("../../../Cargo.lock");
/// The committed `franken-stack.lock`.
pub const STACK_LOCK: &str = include_str!("../../../franken-stack.lock");

/// Every probe that runs without a window, in report order.
#[must_use]
pub fn headless_probes(out_dir: &Path, tag: &str) -> Vec<ProbeReport> {
    vec![
        closure_probe(CARGO_LOCK, STACK_LOCK),
        runtime_probe(),
        rng_probe(),
        array_probe(),
        tensor_probe(),
        manim_frame_probe(out_dir, tag),
        metal_probe(),
        native_input_probe(),
    ]
}
