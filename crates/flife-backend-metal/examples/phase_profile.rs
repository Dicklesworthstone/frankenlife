//! Per-phase CPU vs Metal residency & cost-model profiler (GPU-FORWARD §6b).
//!
//! Measures exact per-tick wall time (`ms/tick`, median, p95, CV%) of:
//! - CPU reference baselines at multiple worker counts (default `w = 1, 8, 12`);
//! - EACH individual Metal backend (`MetalBackend` controllers, `MetalField`,
//!   `MetalSensors`, `MetalUptake`, `MetalSettle`) and all 5 combined on ONE
//!   shared `DeviceOwner` (`metal_all`).
//!
//! Runs on Bundle A (`presets/morphogenesis-tissue-v2.toml`), Bundle B
//! (`presets/supercolony-v2.toml`), and Confluent (`presets/confluent-world.toml`,
//! defaulted to 262,144 founders via `--confluent-initial 262144` so the 262k
//! live regime is measured immediately without multi-minute warmup).
//!
//! For every Metal backend, thin zero-mutation timing proxies separate:
//! - **GPU kernel compute time** (`DeviceOwner::device_time()` delta, i.e.
//!   Metal `GPUEndTime - GPUStartTime`),
//! - **Host↔device staging, copy, and command-buffer submit/wait overhead**
//!   (`phase_wall_ms - gpu_compute_ms`),
//! - **Metal command buffer round-trips per tick** (`commands / ticks`), and
//! - **Net whole-tick delta vs the paired CPU baseline** (`delta_vs_cpu_ms`).
//!
//! Every configuration is verified for **bit-exact `World::digest()`** parity
//! after every interleaved rotating block.
//!
//! Usage:
//! ```text
//! cargo run --release -p flife-backend-metal --example phase_profile -- \
//!   [--workloads bundle-a,bundle-b,confluent] \
//!   [--config path/to/preset.toml] \
//!   [--workers 1,8,12,16] \
//!   [--metal-workers 8,12,16] \
//!   [--modes cpu,torch,controllers,field,sensors,uptake,settle,all] \
//!   [--warmup N] [--ticks N] [--blocks N] \
//!   [--confluent-initial 262144] \
//!   [--json-only]
//! ```

#![forbid(unsafe_code)]

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use flife_backend_cpu::{CpuBackend, SHARD_SLOTS as CPU_SHARD_SLOTS};
use flife_backend_metal::{
    BackendBudget, ChemistryConstants, ChemistryOrganism, ChemistryOutcome, ChemistryPacket,
    DeviceOwner, EmissionOrganism, EmissionOutcome, Limits, MetalBackend, MetalField, MetalSensors,
    MetalSettle, MetalUptake, MotionConstants, MotionOrganism, MotionOutcome, MotionV2Extras,
    MotorSteeringConstants, MutationBirth, MutationConstants, MutationCounts, MutationValues,
    ProposalConstants, ProposalOrganism, ProposalOutcome,
};
use flife_core::{CoreError, TimeStep, ValidatedConfig};
use flife_engine::{Arrangement, NumericalProfile, Stepped, World};
use flife_field::settle::SettleBackend;
use flife_field::{
    ConeTolerance, DepositSummary, Deposits, DualRawSample, ExchangeStencil, FieldBackend,
    FieldGrid, FieldPair, FieldScan, FieldState, FieldStepOptions, FieldStepPhase2,
    FieldStepReport, Grants, RawSample, RequestRule, SensorBackend, SensorPyramid, SensorQuery,
    Source, SourceKernel, TransportPlan, UptakeBackend, UptakeCell,
};
use flife_neural::{
    ArenaSlot, ControllerBackend, ControllerJob, FlatControllerJob, FlatOutputs, RawOutputs,
};

const BUNDLE_A_TOML: &str = include_str!("../../../presets/morphogenesis-tissue-v2.toml");
const BUNDLE_B_TOML: &str = include_str!("../../../presets/supercolony-v2.toml");
const CONFLUENT_TOML: &str = include_str!("../../../presets/confluent-world.toml");

const SUBPHASE_COUNT: usize = 13;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
enum Subphase {
    FieldAdvance = 0,
    FieldReduce = 1,
    FieldCheck = 2,
    SensorsSample = 3,
    UptakeRequest = 4,
    UptakeResolve = 5,
    SettleChemistry = 6,
    SettleMotion = 7,
    SettleEmission = 8,
    SettleProposal = 9,
    SettleMutation = 10,
    ControllerEval = 11,
    ControllerReleaseSlots = 12,
}

impl Subphase {
    const ALL: [Self; SUBPHASE_COUNT] = [
        Self::FieldAdvance,
        Self::FieldReduce,
        Self::FieldCheck,
        Self::SensorsSample,
        Self::UptakeRequest,
        Self::UptakeResolve,
        Self::SettleChemistry,
        Self::SettleMotion,
        Self::SettleEmission,
        Self::SettleProposal,
        Self::SettleMutation,
        Self::ControllerEval,
        Self::ControllerReleaseSlots,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::FieldAdvance => "field.advance",
            Self::FieldReduce => "field.reduce",
            Self::FieldCheck => "field.check",
            Self::SensorsSample => "sensors.sample",
            Self::UptakeRequest => "uptake.request",
            Self::UptakeResolve => "uptake.resolve",
            Self::SettleChemistry => "settle.chemistry",
            Self::SettleMotion => "settle.motion",
            Self::SettleEmission => "settle.emission",
            Self::SettleProposal => "settle.proposal",
            Self::SettleMutation => "settle.mutation",
            Self::ControllerEval => "controller.eval",
            Self::ControllerReleaseSlots => "controller.release_slots",
        }
    }
}

#[derive(Debug, Default)]
struct AtomicSubphase {
    calls: AtomicU64,
    wall_ns: AtomicU64,
    gpu_ns: AtomicU64,
    commands: AtomicU64,
}

#[derive(Debug, Clone, Copy, Default)]
struct SubphaseSnapshot {
    calls: u64,
    wall_ns: u64,
    gpu_ns: u64,
    commands: u64,
}

impl SubphaseSnapshot {
    const fn host_ns(self) -> u64 {
        self.wall_ns.saturating_sub(self.gpu_ns)
    }
}

#[derive(Debug, Default)]
struct PhaseCounters {
    buckets: [AtomicSubphase; SUBPHASE_COUNT],
}

impl PhaseCounters {
    fn reset(&self) {
        for bucket in &self.buckets {
            bucket.calls.store(0, Ordering::Relaxed);
            bucket.wall_ns.store(0, Ordering::Relaxed);
            bucket.gpu_ns.store(0, Ordering::Relaxed);
            bucket.commands.store(0, Ordering::Relaxed);
        }
    }

    fn record<R>(&self, phase: Subphase, owner: Option<&DeviceOwner>, f: impl FnOnce() -> R) -> R {
        let dev0 = owner.map(DeviceOwner::device_time);
        let start = Instant::now();
        let result = f();
        let wall_ns = u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
        let (gpu_ns, cmds) = match (dev0, owner.map(DeviceOwner::device_time)) {
            (Some(before), Some(after)) => (
                after.nanoseconds.saturating_sub(before.nanoseconds),
                after.commands.saturating_sub(before.commands),
            ),
            _ => (0, 0),
        };
        let bucket = &self.buckets[phase as usize];
        bucket.calls.fetch_add(1, Ordering::Relaxed);
        bucket.wall_ns.fetch_add(wall_ns, Ordering::Relaxed);
        bucket.gpu_ns.fetch_add(gpu_ns, Ordering::Relaxed);
        bucket.commands.fetch_add(cmds, Ordering::Relaxed);
        result
    }

    fn snapshot(&self) -> [SubphaseSnapshot; SUBPHASE_COUNT] {
        std::array::from_fn(|i| {
            let bucket = &self.buckets[i];
            SubphaseSnapshot {
                calls: bucket.calls.load(Ordering::Relaxed),
                wall_ns: bucket.wall_ns.load(Ordering::Relaxed),
                gpu_ns: bucket.gpu_ns.load(Ordering::Relaxed),
                commands: bucket.commands.load(Ordering::Relaxed),
            }
        })
    }
}

#[derive(Debug)]
struct TimedField {
    inner: MetalField,
    owner: DeviceOwner,
    counters: Arc<PhaseCounters>,
}

impl FieldBackend for TimedField {
    fn describe(&self) -> String {
        self.inner.describe()
    }

    fn advance(
        &mut self,
        pair: &mut FieldPair,
        plan: &TransportPlan,
        kernel: &SourceKernel,
        sources: &[Source],
        step: TimeStep,
    ) -> Result<FieldStepReport, CoreError> {
        let owner = &self.owner;
        let inner = &mut self.inner;
        self.counters
            .record(Subphase::FieldAdvance, Some(owner), || {
                inner.advance(pair, plan, kernel, sources, step)
            })
    }

    fn advance_v2(
        &mut self,
        pair: &mut FieldPair,
        plan: &TransportPlan,
        kernel: &SourceKernel,
        sources: &[Source],
        step: TimeStep,
        options: FieldStepOptions<'_>,
    ) -> Result<FieldStepReport, CoreError> {
        let owner = &self.owner;
        let inner = &mut self.inner;
        self.counters
            .record(Subphase::FieldAdvance, Some(owner), || {
                inner.advance_v2(pair, plan, kernel, sources, step, options)
            })
    }

    fn advance_wave2(
        &mut self,
        pair: &mut FieldPair,
        plan: &TransportPlan,
        kernel: &SourceKernel,
        sources: &[Source],
        step: TimeStep,
        phase2: FieldStepPhase2<'_>,
    ) -> Result<FieldStepReport, CoreError> {
        let owner = &self.owner;
        let inner = &mut self.inner;
        self.counters
            .record(Subphase::FieldAdvance, Some(owner), || {
                inner.advance_wave2(pair, plan, kernel, sources, step, phase2)
            })
    }

    fn reduce(
        &mut self,
        field: &mut FieldState,
        grants: &mut Grants,
        deposits: [&mut Deposits; 2],
    ) -> Result<[DepositSummary; 2], CoreError> {
        let owner = &self.owner;
        let inner = &mut self.inner;
        self.counters
            .record(Subphase::FieldReduce, Some(owner), || {
                inner.reduce(field, grants, deposits)
            })
    }

    fn reduce_dual_carrier(
        &mut self,
        field: &mut FieldState,
        grants: &mut Grants,
        carrier0_deposits: [&mut Deposits; 2],
        trail_deposits: &mut Deposits,
    ) -> Result<([DepositSummary; 2], DepositSummary), CoreError> {
        let owner = &self.owner;
        let inner = &mut self.inner;
        self.counters
            .record(Subphase::FieldReduce, Some(owner), || {
                inner.reduce_dual_carrier(field, grants, carrier0_deposits, trail_deposits)
            })
    }

    fn check(
        &mut self,
        field: &FieldState,
        tolerance: ConeTolerance,
    ) -> Result<FieldScan, CoreError> {
        let owner = &self.owner;
        let inner = &mut self.inner;
        self.counters.record(Subphase::FieldCheck, Some(owner), || {
            inner.check(field, tolerance)
        })
    }
}

#[derive(Debug)]
struct TimedSensors {
    inner: MetalSensors,
    owner: DeviceOwner,
    counters: Arc<PhaseCounters>,
}

impl SensorBackend for TimedSensors {
    fn describe(&self) -> String {
        self.inner.describe()
    }

    fn sample(
        &mut self,
        pyramid: &SensorPyramid,
        field: &FieldState,
        queries: &[SensorQuery],
        samples: &mut Vec<RawSample>,
    ) -> Result<(), CoreError> {
        let owner = &self.owner;
        let inner = &mut self.inner;
        self.counters
            .record(Subphase::SensorsSample, Some(owner), || {
                inner.sample(pyramid, field, queries, samples)
            })
    }

    fn sample_dual(
        &mut self,
        pyramid: &SensorPyramid,
        field: &FieldState,
        queries: &[SensorQuery],
        samples: &mut Vec<DualRawSample>,
    ) -> Result<(), CoreError> {
        let owner = &self.owner;
        let inner = &mut self.inner;
        self.counters
            .record(Subphase::SensorsSample, Some(owner), || {
                inner.sample_dual(pyramid, field, queries, samples)
            })
    }
}

#[derive(Debug)]
struct TimedUptake {
    inner: MetalUptake,
    owner: DeviceOwner,
    counters: Arc<PhaseCounters>,
}

impl UptakeBackend for TimedUptake {
    fn describe(&self) -> String {
        self.inner.describe()
    }

    fn request(
        &mut self,
        field: &FieldState,
        rule: &RequestRule,
        stencils: &[ExchangeStencil],
        densities: &mut Vec<f64>,
        requests: &mut Vec<f64>,
    ) -> Result<(), CoreError> {
        let owner = &self.owner;
        let inner = &mut self.inner;
        self.counters
            .record(Subphase::UptakeRequest, Some(owner), || {
                inner.request(field, rule, stencils, densities, requests)
            })
    }

    fn resolve(
        &mut self,
        cells: &[UptakeCell],
        requested: &[f64],
        demanded: &mut Vec<f64>,
        quanta: &mut Vec<u32>,
    ) -> Result<(), CoreError> {
        let owner = &self.owner;
        let inner = &mut self.inner;
        self.counters
            .record(Subphase::UptakeResolve, Some(owner), || {
                inner.resolve(cells, requested, demanded, quanta)
            })
    }
}

#[derive(Debug)]
struct TimedSettle {
    inner: MetalSettle,
    owner: DeviceOwner,
    counters: Arc<PhaseCounters>,
}

impl SettleBackend for TimedSettle {
    fn describe(&self) -> String {
        self.inner.describe()
    }

    fn chemistry(
        &mut self,
        observed: &FieldState,
        constants: &ChemistryConstants,
        organisms: &[ChemistryOrganism],
        packets: &[ChemistryPacket],
        out: &mut Vec<ChemistryOutcome>,
    ) -> Result<(), CoreError> {
        let owner = &self.owner;
        let inner = &mut self.inner;
        self.counters
            .record(Subphase::SettleChemistry, Some(owner), || {
                inner.chemistry(observed, constants, organisms, packets, out)
            })
    }

    fn motion(
        &mut self,
        constants: &MotionConstants,
        organisms: &[MotionOrganism],
        out: &mut Vec<MotionOutcome>,
    ) -> Result<(), CoreError> {
        let owner = &self.owner;
        let inner = &mut self.inner;
        self.counters
            .record(Subphase::SettleMotion, Some(owner), || {
                inner.motion(constants, organisms, out)
            })
    }

    fn motion_v2(
        &mut self,
        constants: &MotionConstants,
        motor_constants: Option<&MotorSteeringConstants>,
        organisms: &[MotionOrganism],
        extras: &[MotionV2Extras],
        out: &mut Vec<MotionOutcome>,
    ) -> Result<(), CoreError> {
        let owner = &self.owner;
        let inner = &mut self.inner;
        self.counters
            .record(Subphase::SettleMotion, Some(owner), || {
                inner.motion_v2(constants, motor_constants, organisms, extras, out)
            })
    }

    fn emission(
        &mut self,
        grid: &FieldGrid,
        efficiency: f64,
        organisms: &[EmissionOrganism],
        out: &mut Vec<EmissionOutcome>,
    ) -> Result<(), CoreError> {
        let owner = &self.owner;
        let inner = &mut self.inner;
        self.counters
            .record(Subphase::SettleEmission, Some(owner), || {
                inner.emission(grid, efficiency, organisms, out)
            })
    }

    fn emission_with_floor(
        &mut self,
        grid: &FieldGrid,
        efficiency: f64,
        energy_floor: f64,
        organisms: &[EmissionOrganism],
        out: &mut Vec<EmissionOutcome>,
    ) -> Result<(), CoreError> {
        let owner = &self.owner;
        let inner = &mut self.inner;
        self.counters
            .record(Subphase::SettleEmission, Some(owner), || {
                inner.emission_with_floor(grid, efficiency, energy_floor, organisms, out)
            })
    }

    fn proposal(
        &mut self,
        constants: &ProposalConstants,
        organisms: &[ProposalOrganism],
        out: &mut Vec<ProposalOutcome>,
    ) -> Result<(), CoreError> {
        let owner = &self.owner;
        let inner = &mut self.inner;
        self.counters
            .record(Subphase::SettleProposal, Some(owner), || {
                inner.proposal(constants, organisms, out)
            })
    }

    fn mutation(
        &mut self,
        constants: &MutationConstants,
        births: &[MutationBirth],
        values: MutationValues<'_>,
        out: &mut Vec<MutationCounts>,
    ) -> Result<(), CoreError> {
        let owner = &self.owner;
        let inner = &mut self.inner;
        self.counters
            .record(Subphase::SettleMutation, Some(owner), || {
                inner.mutation(constants, births, values, out)
            })
    }
}

#[derive(Debug)]
struct TimedController {
    inner: MetalBackend,
    owner: DeviceOwner,
    counters: Arc<PhaseCounters>,
}

impl ControllerBackend for TimedController {
    fn describe(&self) -> String {
        self.inner.describe()
    }

    fn resident_bytes(&self) -> u64 {
        self.inner.resident_bytes()
    }

    fn forget(&mut self) {
        self.inner.forget();
    }

    fn release_slots(&mut self, slots: &[ArenaSlot]) {
        let inner = &mut self.inner;
        self.counters
            .record(Subphase::ControllerReleaseSlots, None, || {
                inner.release_slots(slots);
            });
    }

    fn last_device_nanoseconds(&self) -> u64 {
        self.inner.last_device_nanoseconds()
    }

    fn evaluate(
        &mut self,
        jobs: &[ControllerJob<'_>],
        outputs: &mut Vec<Option<RawOutputs>>,
    ) -> Result<(), CoreError> {
        let owner = &self.owner;
        let inner = &mut self.inner;
        self.counters
            .record(Subphase::ControllerEval, Some(owner), || {
                inner.evaluate(jobs, outputs)
            })
    }

    fn supports_flat_f32(&self) -> bool {
        self.inner.supports_flat_f32()
    }

    fn evaluate_flat_f32(
        &mut self,
        jobs: &[FlatControllerJob<'_>],
        input_width: usize,
        inputs_f32: &[f32],
        inputs_finite: &[u8],
        outputs: &mut FlatOutputs,
    ) -> Result<(), CoreError> {
        let owner = &self.owner;
        let inner = &mut self.inner;
        self.counters
            .record(Subphase::ControllerEval, Some(owner), || {
                inner.evaluate_flat_f32(jobs, input_width, inputs_f32, inputs_finite, outputs)
            })
    }
}

#[derive(Debug)]
struct TimedTorchController {
    inner: CpuBackend,
    counters: Arc<PhaseCounters>,
}

impl ControllerBackend for TimedTorchController {
    fn describe(&self) -> String {
        self.inner.describe()
    }

    fn resident_bytes(&self) -> u64 {
        self.inner.resident_bytes()
    }

    fn forget(&mut self) {
        self.inner.forget();
    }

    fn release_slots(&mut self, slots: &[ArenaSlot]) {
        let inner = &mut self.inner;
        self.counters
            .record(Subphase::ControllerReleaseSlots, None, || {
                inner.release_slots(slots);
            });
    }

    fn last_device_nanoseconds(&self) -> u64 {
        self.inner.last_device_nanoseconds()
    }

    fn evaluate(
        &mut self,
        jobs: &[ControllerJob<'_>],
        outputs: &mut Vec<Option<RawOutputs>>,
    ) -> Result<(), CoreError> {
        let inner = &mut self.inner;
        self.counters.record(Subphase::ControllerEval, None, || {
            inner.evaluate(jobs, outputs)
        })
    }

    fn supports_flat_f32(&self) -> bool {
        self.inner.supports_flat_f32()
    }

    fn evaluate_flat_f32(
        &mut self,
        jobs: &[FlatControllerJob<'_>],
        input_width: usize,
        inputs_f32: &[f32],
        inputs_finite: &[u8],
        outputs: &mut FlatOutputs,
    ) -> Result<(), CoreError> {
        let inner = &mut self.inner;
        self.counters.record(Subphase::ControllerEval, None, || {
            inner.evaluate_flat_f32(jobs, input_width, inputs_f32, inputs_finite, outputs)
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MetalMode {
    Controllers,
    Field,
    Sensors,
    Uptake,
    Settle,
    All,
}

impl MetalMode {
    const fn label(self) -> &'static str {
        match self {
            Self::Controllers => "metal_controllers",
            Self::Field => "metal_field",
            Self::Sensors => "metal_sensors",
            Self::Uptake => "metal_uptake",
            Self::Settle => "metal_settle",
            Self::All => "metal_all",
        }
    }
}

#[derive(Debug, Clone)]
enum RunMode {
    Cpu(usize),
    Torch(usize),
    Metal { mode: MetalMode, cpu_workers: usize },
}

impl RunMode {
    fn label(&self) -> String {
        match *self {
            Self::Cpu(w) => format!("cpu_w{w}"),
            Self::Torch(w) => format!("torch_w{w}"),
            Self::Metal { mode, cpu_workers } => format!("{}_w{cpu_workers}", mode.label()),
        }
    }

    const fn workers(&self) -> usize {
        match *self {
            Self::Cpu(w) | Self::Torch(w) => w,
            Self::Metal { cpu_workers, .. } => cpu_workers,
        }
    }

    const fn is_metal(&self) -> bool {
        matches!(self, Self::Metal { .. })
    }

    const fn is_accelerated(&self) -> bool {
        matches!(self, Self::Torch(_) | Self::Metal { .. })
    }
}

#[derive(Debug)]
struct WorkloadSpec {
    name: String,
    toml: String,
    default_warmup: u64,
    default_ticks: u64,
    default_blocks: usize,
    note: String,
}

#[derive(Debug)]
struct CliOptions {
    workloads: Vec<String>,
    custom_config: Option<String>,
    cpu_workers: Vec<usize>,
    metal_workers: Vec<usize>,
    include_cpu: bool,
    include_torch: bool,
    metal_modes: Vec<MetalMode>,
    warmup: Option<u64>,
    ticks: Option<u64>,
    blocks: Option<usize>,
    confluent_initial: usize,
    json_only: bool,
}

fn conflict(detail: String) -> CoreError {
    CoreError::StateConflict {
        operation: "phase_profile",
        detail,
    }
}

fn parse_csv_usize(raw: &str) -> Result<Vec<usize>, CoreError> {
    let mut out = Vec::new();
    for item in raw.split(',').filter(|s| !s.is_empty()) {
        let value = item
            .parse::<usize>()
            .map_err(|e| conflict(format!("invalid integer `{item}`: {e}")))?;
        if value == 0 {
            return Err(conflict("worker counts must be >= 1".to_owned()));
        }
        if !out.contains(&value) {
            out.push(value);
        }
    }
    if out.is_empty() {
        return Err(conflict("expected at least one worker count".to_owned()));
    }
    Ok(out)
}

fn parse_modes(raw: &str) -> Result<(bool, bool, Vec<MetalMode>), CoreError> {
    let mut include_cpu = false;
    let mut include_torch = false;
    let mut metal_modes = Vec::new();
    for item in raw.split(',').filter(|s| !s.is_empty()) {
        match item {
            "cpu" => include_cpu = true,
            "torch" | "cpu_torch" => include_torch = true,
            "controllers" | "controller" | "metal_controllers" => {
                metal_modes.push(MetalMode::Controllers);
            }
            "field" | "metal_field" => metal_modes.push(MetalMode::Field),
            "sensors" | "metal_sensors" => metal_modes.push(MetalMode::Sensors),
            "uptake" | "metal_uptake" => metal_modes.push(MetalMode::Uptake),
            "settle" | "metal_settle" => metal_modes.push(MetalMode::Settle),
            "all" | "metal_all" => metal_modes.push(MetalMode::All),
            other => {
                return Err(conflict(format!(
                    "unknown mode `{other}` (expected cpu,torch,controllers,field,sensors,uptake,settle,all)"
                )));
            }
        }
    }
    Ok((include_cpu, include_torch, metal_modes))
}

fn parse_args() -> Result<CliOptions, CoreError> {
    let mut opts = CliOptions {
        workloads: vec![
            "bundle-a".to_owned(),
            "bundle-b".to_owned(),
            "confluent".to_owned(),
        ],
        custom_config: None,
        cpu_workers: vec![1, 8, 12],
        metal_workers: vec![8],
        include_cpu: true,
        include_torch: false,
        metal_modes: vec![
            MetalMode::Controllers,
            MetalMode::Field,
            MetalMode::Sensors,
            MetalMode::Uptake,
            MetalMode::Settle,
            MetalMode::All,
        ],
        warmup: None,
        ticks: None,
        blocks: None,
        confluent_initial: 262_144,
        json_only: false,
    };

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let next_val = |flag: &str, iter: &mut std::iter::Skip<std::env::Args>| {
            iter.next()
                .ok_or_else(|| conflict(format!("missing value for {flag}")))
        };
        match arg.as_str() {
            "--workloads" => {
                let raw = next_val("--workloads", &mut args)?;
                opts.workloads = raw
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect();
            }
            "--config" => {
                opts.custom_config = Some(next_val("--config", &mut args)?);
            }
            "--workers" => {
                let raw = next_val("--workers", &mut args)?;
                opts.cpu_workers = parse_csv_usize(&raw)?;
            }
            "--metal-workers" => {
                let raw = next_val("--metal-workers", &mut args)?;
                opts.metal_workers = parse_csv_usize(&raw)?;
            }
            "--modes" => {
                let raw = next_val("--modes", &mut args)?;
                let (cpu, torch, metal) = parse_modes(&raw)?;
                opts.include_cpu = cpu;
                opts.include_torch = torch;
                opts.metal_modes = metal;
            }
            "--warmup" => {
                let raw = next_val("--warmup", &mut args)?;
                opts.warmup = Some(
                    raw.parse::<u64>()
                        .map_err(|e| conflict(format!("invalid --warmup `{raw}`: {e}")))?,
                );
            }
            "--ticks" => {
                let raw = next_val("--ticks", &mut args)?;
                let value = raw
                    .parse::<u64>()
                    .map_err(|e| conflict(format!("invalid --ticks `{raw}`: {e}")))?;
                if value == 0 {
                    return Err(conflict("--ticks must be >= 1".to_owned()));
                }
                opts.ticks = Some(value);
            }
            "--blocks" => {
                let raw = next_val("--blocks", &mut args)?;
                let value = raw
                    .parse::<usize>()
                    .map_err(|e| conflict(format!("invalid --blocks `{raw}`: {e}")))?;
                if value == 0 {
                    return Err(conflict("--blocks must be >= 1".to_owned()));
                }
                opts.blocks = Some(value);
            }
            "--confluent-initial" => {
                let raw = next_val("--confluent-initial", &mut args)?;
                opts.confluent_initial = raw
                    .parse::<usize>()
                    .map_err(|e| conflict(format!("invalid --confluent-initial `{raw}`: {e}")))?;
            }
            "--json-only" => opts.json_only = true,
            other => {
                return Err(conflict(format!("unknown argument `{other}`")));
            }
        }
    }
    Ok(opts)
}

fn override_population_initial_count(toml: &str, count: usize) -> Result<String, CoreError> {
    let mut in_population = false;
    let mut replaced = false;
    let mut out = Vec::new();
    for line in toml.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_population = trimmed == "[population]";
        }
        if in_population && trimmed.starts_with("initial_count") && trimmed.contains('=') {
            out.push(format!("initial_count = {count}"));
            replaced = true;
        } else {
            out.push(line.to_owned());
        }
    }
    if !replaced {
        return Err(conflict(
            "could not find `[population] initial_count` to override".to_owned(),
        ));
    }
    Ok(out.join("\n"))
}

fn resolve_workloads(opts: &CliOptions) -> Result<Vec<WorkloadSpec>, CoreError> {
    if let Some(path) = &opts.custom_config {
        let toml = std::fs::read_to_string(path)
            .map_err(|e| conflict(format!("failed to read `{path}`: {e}")))?;
        return Ok(vec![WorkloadSpec {
            name: path.clone(),
            toml,
            default_warmup: 4,
            default_ticks: 4,
            default_blocks: 6,
            note: "custom config".to_owned(),
        }]);
    }
    let mut specs = Vec::new();
    for name in &opts.workloads {
        match name.as_str() {
            "bundle-a" => specs.push(WorkloadSpec {
                name: "bundle-a".to_owned(),
                toml: BUNDLE_A_TOML.to_owned(),
                default_warmup: 4,
                default_ticks: 4,
                default_blocks: 6,
                note: "presets/morphogenesis-tissue-v2.toml (320x320x33, 14 V2 features, [contact])"
                    .to_owned(),
            }),
            "bundle-b" => specs.push(WorkloadSpec {
                name: "bundle-b".to_owned(),
                toml: BUNDLE_B_TOML.to_owned(),
                default_warmup: 4,
                default_ticks: 4,
                default_blocks: 6,
                note: "presets/supercolony-v2.toml (320x320x34 dual-carrier + vascular, 11 V2 features)"
                    .to_owned(),
            }),
            "confluent" => {
                let (toml, note) = if opts.confluent_initial > 0 {
                    (
                        override_population_initial_count(CONFLUENT_TOML, opts.confluent_initial)?,
                        format!(
                            "presets/confluent-world.toml (320x320x33, [contact], initial_count={} override)",
                            opts.confluent_initial
                        ),
                    )
                } else {
                    (
                        CONFLUENT_TOML.to_owned(),
                        "presets/confluent-world.toml (unmodified initial_count=10954)".to_owned(),
                    )
                };
                specs.push(WorkloadSpec {
                    name: "confluent".to_owned(),
                    toml,
                    default_warmup: 2,
                    default_ticks: 2,
                    default_blocks: 4,
                    note,
                });
            }
            other => {
                return Err(conflict(format!(
                    "unknown workload `{other}` (expected bundle-a, bundle-b, confluent)"
                )));
            }
        }
    }
    Ok(specs)
}

struct Entry {
    mode: RunMode,
    label: String,
    world: World,
    counters: Arc<PhaseCounters>,
    block_ms_per_tick: Vec<f64>,
}

fn build_entry(mode: RunMode, config: &ValidatedConfig) -> Result<Entry, CoreError> {
    let label = mode.label();
    let profile = NumericalProfile::reference_f32()?;
    let mut world = World::found(config, Arrangement::FreeList, profile)?;
    let counters = Arc::new(PhaseCounters::default());
    match mode {
        RunMode::Cpu(workers) => {
            let w = NonZeroUsize::new(workers).unwrap_or(NonZeroUsize::MIN);
            world.set_cpu_workers(w)?;
        }
        RunMode::Torch(workers) => {
            let w = NonZeroUsize::new(workers).unwrap_or(NonZeroUsize::MIN);
            world.set_cpu_workers(w)?;
            let inner = CpuBackend::with_budget(w, CPU_SHARD_SLOTS, BackendBudget::UNBOUNDED);
            world.set_controller_backend(Some(Box::new(TimedTorchController {
                inner,
                counters: Arc::clone(&counters),
            })));
        }
        RunMode::Metal {
            mode: metal_mode,
            cpu_workers,
        } => {
            let w = NonZeroUsize::new(cpu_workers).unwrap_or(NonZeroUsize::MIN);
            world.set_cpu_workers(w)?;
            let owner = DeviceOwner::open(Limits::UNBOUNDED)?;
            let install_controllers = |world: &mut World,
                                       owner: &DeviceOwner,
                                       counters: &Arc<PhaseCounters>|
             -> Result<(), CoreError> {
                let mut inner =
                    MetalBackend::with_owner(owner.clone(), u64::MAX, BackendBudget::UNBOUNDED)?;
                inner.set_staging_workers(w);
                world.set_controller_backend(Some(Box::new(TimedController {
                    inner,
                    owner: owner.clone(),
                    counters: Arc::clone(counters),
                })));
                Ok(())
            };
            match metal_mode {
                MetalMode::Controllers => install_controllers(&mut world, &owner, &counters)?,
                MetalMode::Field => {
                    world.set_field_backend(Some(Box::new(TimedField {
                        inner: MetalField::with_owner(owner.clone())?,
                        owner,
                        counters: Arc::clone(&counters),
                    })));
                }
                MetalMode::Sensors => {
                    world.set_sensor_backend(Some(Box::new(TimedSensors {
                        inner: MetalSensors::with_owner(owner.clone())?,
                        owner,
                        counters: Arc::clone(&counters),
                    })));
                }
                MetalMode::Uptake => {
                    world.set_uptake_backend(Some(Box::new(TimedUptake {
                        inner: MetalUptake::with_owner(owner.clone())?,
                        owner,
                        counters: Arc::clone(&counters),
                    })));
                }
                MetalMode::Settle => {
                    world.set_settle_backend(Some(Box::new(TimedSettle {
                        inner: MetalSettle::with_owner(owner.clone())?,
                        owner,
                        counters: Arc::clone(&counters),
                    })));
                }
                MetalMode::All => {
                    world.set_field_backend(Some(Box::new(TimedField {
                        inner: MetalField::with_owner(owner.clone())?,
                        owner: owner.clone(),
                        counters: Arc::clone(&counters),
                    })));
                    world.set_sensor_backend(Some(Box::new(TimedSensors {
                        inner: MetalSensors::with_owner(owner.clone())?,
                        owner: owner.clone(),
                        counters: Arc::clone(&counters),
                    })));
                    world.set_uptake_backend(Some(Box::new(TimedUptake {
                        inner: MetalUptake::with_owner(owner.clone())?,
                        owner: owner.clone(),
                        counters: Arc::clone(&counters),
                    })));
                    install_controllers(&mut world, &owner, &counters)?;
                    world.set_settle_backend(Some(Box::new(TimedSettle {
                        inner: MetalSettle::with_owner(owner.clone())?,
                        owner,
                        counters: Arc::clone(&counters),
                    })));
                }
            }
        }
    }
    Ok(Entry {
        mode,
        label,
        world,
        counters,
        block_ms_per_tick: Vec::new(),
    })
}

fn step_block(world: &mut World, ticks: u64) -> Result<(f64, usize), CoreError> {
    let start = Instant::now();
    let mut live = world.population().len();
    for _ in 0..ticks {
        match world.step()? {
            Stepped::Published(report) => live = report.live,
            Stepped::Paused(shortfall) => {
                return Err(conflict(format!(
                    "world paused during profile: {shortfall:?}"
                )));
            }
        }
    }
    Ok((start.elapsed().as_secs_f64() * 1e3 / ticks as f64, live))
}

fn percentile(samples: &[f64], p: f64) -> f64 {
    if samples.is_empty() {
        return f64::NAN;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn cv_percent(samples: &[f64]) -> f64 {
    if samples.len() < 2 {
        return 0.0;
    }
    let n = samples.len() as f64;
    let mean = samples.iter().sum::<f64>() / n;
    if mean <= 0.0 {
        return 0.0;
    }
    let var = samples.iter().map(|&x| (x - mean).powi(2)).sum::<f64>() / n;
    100.0 * var.sqrt() / mean
}

fn json_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out
}

fn fallbacks_and_failures(world: &World) -> (u64, Vec<String>) {
    let f = world.field_report();
    let s = world.sensor_report();
    let u = world.uptake_report();
    let c = world.controller_report();
    let t = world.settle_report();
    let fallbacks = f.reference_steps
        + f.reference_reductions
        + f.reference_checks
        + s.reference_ticks
        + u.reference_ticks
        + c.reference_ticks
        + t.reference_ticks;
    let mut failures = Vec::new();
    for (name, err) in [
        ("field", &f.failure),
        ("sensors", &s.failure),
        ("uptake", &u.failure),
        ("controllers", &c.failure),
        ("settle", &t.failure),
    ] {
        if let Some(msg) = err {
            failures.push(format!("{name}: {msg}"));
        }
    }
    (fallbacks, failures)
}

#[expect(
    clippy::too_many_lines,
    reason = "orchestrates interleaved benchmark blocks, parity verification, and structured reporting"
)]
fn run_workload(spec: &WorkloadSpec, opts: &CliOptions) -> Result<(), CoreError> {
    let config = ValidatedConfig::from_toml(&spec.toml)?;
    let warmup = opts.warmup.unwrap_or(spec.default_warmup);
    let ticks = opts.ticks.unwrap_or(spec.default_ticks);
    let blocks = opts.blocks.unwrap_or(spec.default_blocks);
    let timed_ticks = ticks * blocks as u64;

    let mut modes = Vec::new();
    if opts.include_cpu {
        let mut cpu_list = opts.cpu_workers.clone();
        if opts.include_torch || !opts.metal_modes.is_empty() {
            for &w in &opts.metal_workers {
                if !cpu_list.contains(&w) {
                    cpu_list.push(w);
                }
            }
        }
        for w in cpu_list {
            modes.push(RunMode::Cpu(w));
        }
    }
    if opts.include_torch {
        for &w in &opts.metal_workers {
            modes.push(RunMode::Torch(w));
        }
    }
    for &mode in &opts.metal_modes {
        for &w in &opts.metal_workers {
            modes.push(RunMode::Metal {
                mode,
                cpu_workers: w,
            });
        }
    }

    let mut entries = Vec::new();
    for mode in modes {
        match build_entry(mode.clone(), &config) {
            Ok(mut entry) => {
                if warmup > 0 {
                    step_block(&mut entry.world, warmup)?;
                }
                entry.counters.reset();
                entries.push(entry);
            }
            Err(CoreError::MissingCapability { capability }) => {
                eprintln!(
                    "[phase_profile] skipping {}: missing capability ({capability})",
                    mode.label()
                );
            }
            Err(err) => return Err(err),
        }
    }
    if entries.is_empty() {
        return Err(conflict("no runnable modes configured".to_owned()));
    }

    let live_start = entries[0].world.population().len();
    let mut live_end = live_start;
    let mut digests_equal = true;

    // Interleaved rotating blocks so thermal state is balanced across every mode.
    for block_idx in 0..blocks {
        let n = entries.len();
        for offset in 0..n {
            let idx = (block_idx + offset) % n;
            let (ms, live) = step_block(&mut entries[idx].world, ticks)?;
            entries[idx].block_ms_per_tick.push(ms);
            live_end = live;
        }
        let golden = entries[0].world.digest();
        for entry in &entries[1..] {
            if entry.world.digest() != golden {
                digests_equal = false;
                eprintln!(
                    "[phase_profile] DIGEST MISMATCH on {} block {block_idx}: {} ({}) != {} ({})",
                    spec.name,
                    entry.label,
                    entry.world.digest().to_hex(),
                    entries[0].label,
                    golden.to_hex()
                );
            }
        }
    }

    let golden_digest = entries[0].world.digest().to_hex();
    let primary_metal_w = opts.metal_workers.first().copied().unwrap_or(8);
    let cpu_ref_label = format!("cpu_w{primary_metal_w}");
    let cpu_ref_ms = entries
        .iter()
        .find(|e| e.label == cpu_ref_label)
        .or_else(|| entries.first())
        .map_or(f64::NAN, |e| percentile(&e.block_ms_per_tick, 0.5));
    let cpu_w1_ms = entries
        .iter()
        .find(|e| e.label == "cpu_w1")
        .map(|e| percentile(&e.block_ms_per_tick, 0.5));

    if !opts.json_only {
        eprintln!();
        eprintln!("=== phase_profile: {} ({}) ===", spec.name, spec.note);
        eprintln!(
            "warmup={warmup} ticks/block={ticks} blocks={blocks} timed_ticks={timed_ticks} \
             live={live_start}->{live_end} digest={golden_digest} digests_equal={digests_equal}"
        );
        eprintln!(
            "{:<26} {:>9} {:>9} {:>6} {:>10} {:>8} {:>9} {:>9} {:>9} {:>6} {:>24}",
            "mode",
            "med_ms",
            "p95_ms",
            "cv%",
            "delta_ms",
            "vs_cpu",
            "back_ms",
            "gpu_ms",
            "host_ms",
            "cmds/t",
            "verdict"
        );
    }

    let mut json_rows = Vec::with_capacity(entries.len());
    for entry in &entries {
        let med_ms = percentile(&entry.block_ms_per_tick, 0.5);
        let p95_ms = percentile(&entry.block_ms_per_tick, 0.95);
        let cv = cv_percent(&entry.block_ms_per_tick);
        let same_w_label = format!("cpu_w{}", entry.mode.workers());
        let (matched_cpu_label, matched_cpu_ms) =
            if let Some(base) = entries.iter().find(|e| e.label == same_w_label) {
                (same_w_label, percentile(&base.block_ms_per_tick, 0.5))
            } else {
                (cpu_ref_label.clone(), cpu_ref_ms)
            };
        let delta_ref_ms = med_ms - cpu_ref_ms;
        let speedup_ref = cpu_ref_ms / med_ms;
        let delta_matched_ms = med_ms - matched_cpu_ms;
        let speedup_matched = matched_cpu_ms / med_ms;
        let (display_delta_ms, display_speedup) = if entry.mode.is_accelerated() {
            (delta_matched_ms, speedup_matched)
        } else {
            (delta_ref_ms, speedup_ref)
        };
        let speedup_w1 = cpu_w1_ms.map_or(f64::NAN, |w1| w1 / med_ms);

        let snap = entry.counters.snapshot();
        let total_wall_ns: u64 = snap.iter().map(|s| s.wall_ns).sum();
        let total_gpu_ns: u64 = snap.iter().map(|s| s.gpu_ns).sum();
        let total_host_ns: u64 = snap.iter().map(|s| s.host_ns()).sum();
        let total_cmds: u64 = snap.iter().map(|s| s.commands).sum();

        let denom = timed_ticks.max(1) as f64;
        let back_ms = total_wall_ns as f64 / (denom * 1e6);
        let gpu_ms = total_gpu_ns as f64 / (denom * 1e6);
        let host_ms = total_host_ns as f64 / (denom * 1e6);
        let cmds_per_tick = total_cmds as f64 / denom;
        let host_pct = if total_wall_ns > 0 {
            100.0 * total_host_ns as f64 / total_wall_ns as f64
        } else {
            0.0
        };
        let implied_cpu_replaced_ms = if entry.mode.is_accelerated() {
            back_ms - delta_matched_ms
        } else {
            0.0
        };

        let (fallbacks, failures) = fallbacks_and_failures(&entry.world);
        let deferred = entry.world.controller_report().deferred;
        let verdict = if !entry.mode.is_accelerated() {
            "CPU_BASELINE"
        } else if fallbacks > 0 || !failures.is_empty() {
            "FALLBACK_DETECTED"
        } else if delta_matched_ms < 0.0 {
            "BEATS_CPU"
        } else if !entry.mode.is_metal() {
            "SLOWER_CPU_KERNEL"
        } else if host_ms >= gpu_ms {
            "SLOWER_ROUNDTRIP_BOUND"
        } else {
            "SLOWER_COMPUTE_BOUND"
        };

        if !opts.json_only {
            eprintln!(
                "{:<26} {:>9.3} {:>9.3} {:>5.1}% {:>+10.3} {:>7.2}x {:>9.3} {:>9.3} {:>9.3} {:>6.1} {:>24}",
                entry.label,
                med_ms,
                p95_ms,
                cv,
                display_delta_ms,
                display_speedup,
                back_ms,
                gpu_ms,
                host_ms,
                cmds_per_tick,
                verdict
            );
            for sub in Subphase::ALL {
                let s = snap[sub as usize];
                if s.calls > 0 {
                    let s_wall = s.wall_ns as f64 / (denom * 1e6);
                    let s_gpu = s.gpu_ns as f64 / (denom * 1e6);
                    let s_host = s.host_ns() as f64 / (denom * 1e6);
                    let s_cmds = s.commands as f64 / denom;
                    let s_hpct = if s.wall_ns > 0 {
                        100.0 * s.host_ns() as f64 / s.wall_ns as f64
                    } else {
                        0.0
                    };
                    eprintln!(
                        "    ├─ {:<23} wall={s_wall:>7.3}ms  gpu={s_gpu:>7.3}ms  \
                         host={s_host:>7.3}ms ({s_hpct:>4.1}%)  cmds/t={s_cmds:>4.1}  calls={}",
                        sub.name(),
                        s.calls
                    );
                }
            }
            if !failures.is_empty() || fallbacks > 0 || deferred > 0 {
                eprintln!(
                    "    └─ alerts: fallbacks={fallbacks} deferred={deferred} failures={failures:?}"
                );
            }
        }

        let subphase_json: Vec<String> = Subphase::ALL
            .into_iter()
            .filter_map(|sub| {
                let s = snap[sub as usize];
                (s.calls > 0).then(|| {
                    format!(
                        "{{\"subphase\":\"{}\",\"calls\":{},\"wall_ms_per_tick\":{:.4},\
                         \"gpu_ms_per_tick\":{:.4},\"host_ms_per_tick\":{:.4},\
                         \"commands_per_tick\":{:.2}}}",
                        sub.name(),
                        s.calls,
                        s.wall_ns as f64 / (denom * 1e6),
                        s.gpu_ns as f64 / (denom * 1e6),
                        s.host_ns() as f64 / (denom * 1e6),
                        s.commands as f64 / denom,
                    )
                })
            })
            .collect();
        let f_rep = entry.world.field_report();
        let s_rep = entry.world.sensor_report();
        let u_rep = entry.world.uptake_report();
        let c_rep = entry.world.controller_report();
        let t_rep = entry.world.settle_report();

        json_rows.push(format!(
            "{{\"mode\":\"{}\",\"workers\":{},\"median_ms\":{med_ms:.4},\"p95_ms\":{p95_ms:.4},\
             \"cv_percent\":{cv:.2},\"cv_ok\":{},\"ticks_per_s\":{:.3},\
             \"matched_cpu_mode\":\"{}\",\"delta_vs_matched_cpu_ms\":{delta_matched_ms:.4},\
             \"speedup_vs_matched_cpu\":{speedup_matched:.4},\
             \"delta_vs_ref_ms\":{delta_ref_ms:.4},\"speedup_vs_ref\":{speedup_ref:.4},\
             \"speedup_vs_w1\":{},\"backend_wall_ms\":{back_ms:.4},\
             \"gpu_compute_ms\":{gpu_ms:.4},\"host_staging_ms\":{host_ms:.4},\
             \"host_overhead_pct\":{host_pct:.2},\"commands_per_tick\":{cmds_per_tick:.2},\
             \"implied_cpu_replaced_ms\":{implied_cpu_replaced_ms:.4},\
             \"fallbacks\":{fallbacks},\"controller_deferred\":{deferred},\
             \"field_steps\":{},\"field_reductions\":{},\"field_checks\":{},\
             \"sensor_ticks\":{},\"uptake_ticks\":{},\"settle_ticks\":{},\
             \"controller_batched\":{},\"verdict\":\"{verdict}\",\"subphases\":[{}]}}",
            json_escape(&entry.label),
            entry.mode.workers(),
            cv <= 5.0,
            1e3 / med_ms,
            json_escape(&matched_cpu_label),
            if speedup_w1.is_finite() {
                format!("{speedup_w1:.4}")
            } else {
                "null".to_owned()
            },
            f_rep.steps,
            f_rep.reductions,
            f_rep.checks,
            s_rep.ticks,
            u_rep.ticks,
            t_rep.ticks,
            c_rep.batched,
            subphase_json.join(",")
        ));
    }

    println!(
        "{{\"workload\":\"{}\",\"note\":\"{}\",\"warmup\":{warmup},\"ticks_per_block\":{ticks},\
         \"blocks\":{blocks},\"timed_ticks\":{timed_ticks},\"live_start\":{live_start},\
         \"live_end\":{live_end},\"cpu_ref_mode\":\"{}\",\"digests_equal\":{digests_equal},\
         \"digest\":\"{golden_digest}\",\"rows\":[{}]}}",
        json_escape(&spec.name),
        json_escape(&spec.note),
        json_escape(&cpu_ref_label),
        json_rows.join(",")
    );

    if !digests_equal {
        return Err(conflict(format!(
            "digest mismatch detected on workload `{}`",
            spec.name
        )));
    }
    Ok(())
}

fn main() -> Result<(), CoreError> {
    let opts = parse_args()?;
    let workloads = resolve_workloads(&opts)?;
    for spec in &workloads {
        run_workload(spec, &opts)?;
    }
    Ok(())
}
