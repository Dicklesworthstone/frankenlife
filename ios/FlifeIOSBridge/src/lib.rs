//! Standalone C-ABI bridge exposing the genuine FrankenLife Rust simulation
//! engine (`flife-core`, `flife-field`, `flife-genome`, `flife-neural`,
//! `flife-engine`, `flife-render`) to native iOS SwiftUI + Metal.
//!
//! Architecture invariants preserved:
//! - Workspace `flife-*` crates keep `#![forbid(unsafe_code)]`; FFI `unsafe`
//!   is isolated strictly in this standalone bridge crate.
//! - Presentation is pure (`INV-09`): the UI/Metal render thread only leases
//!   immutable `flife_render::Epoch` buffers (`Lease`) and never mutates world
//!   state directly.
//! - Native Apple iOS Metal compute plugs directly into `World` via
//!   `IosMetalControllerBackend` (`ControllerBackend`), reproducing
//!   `ReferenceController` bit-for-bit (0 ULP) with persistent `storageModeShared`
//!   shards, AoSoA F16 packing, and `prefers_cpu_overlap() == true`.

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_void};
use std::num::NonZeroUsize;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use flife_core::binary16::F16;
use flife_core::ids::{GenomeId, OrganismId, RenderEpoch};
use flife_core::pool;
use flife_core::{CoreError, RunConfig, V2Feature, validate};
use flife_engine::{
    Arrangement, Disposition, Intervention, NumericalProfile, OrganismSnapshot, RegionMeasurement,
    Stepped, World,
};
use flife_neural::{
    ArenaSlot, ControllerBackend, ControllerJob, DenseTopology, FlatControllerJob, FlatOutputs,
    MAX_CONTROLLER_OUTPUT_WIDTH, MAX_SENSOR_WIDTH, RawOutputs, ShardGeometry, WeightDtype, Weights,
};
use flife_render::{DISPLAY_PAIR, Feed, Lease, Publisher, Viewer};

const WIDEST_HIDDEN: usize = 128;
const FORWARD_THREADS_WIDE: u32 = 64;
const FORWARD_THREADS_COMPACT: u32 = 32;
const JOB_WORDS: usize = 5;
const SHARD_BYTES_CAP: u64 = 128 * 1024 * 1024;
const DEFAULT_SHARD_SLOTS: u64 = 16_384;

const fn forward_threads(widest_hidden: usize) -> u32 {
    if widest_hidden <= 64 {
        FORWARD_THREADS_COMPACT
    } else {
        FORWARD_THREADS_WIDE
    }
}

/* =========================================================================
 * C-ABI Struct Definitions (exact layout match with include/flife_ios.h)
 * ========================================================================= */

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FlifeMetalShardDispatch {
    pub shard_handle: u64,
    pub job_words: *const u32,
    pub job_count: u32,
    pub threads_per_tg: u32,
    pub is_f16: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FlifeMetalVtable {
    pub ctx: *mut c_void,
    pub max_buffer_bytes: u64,
    pub recommended_working_set_bytes: u64,
    pub device_name: *const c_char,

    pub alloc_shard: Option<
        unsafe extern "C" fn(ctx: *mut c_void, bytes: u64, out_contents: *mut *mut u8) -> u64,
    >,
    pub free_shard: Option<unsafe extern "C" fn(ctx: *mut c_void, handle: u64)>,
    pub write_shard: Option<
        unsafe extern "C" fn(
            ctx: *mut c_void,
            handle: u64,
            byte_offset: u64,
            data: *const u8,
            len: u64,
        ) -> i32,
    >,
    pub copy_shard: Option<
        unsafe extern "C" fn(
            ctx: *mut c_void,
            src_handle: u64,
            dst_handle: u64,
            byte_len: u64,
        ) -> i32,
    >,
    pub dispatch_batch: Option<
        unsafe extern "C" fn(
            ctx: *mut c_void,
            dispatches: *const FlifeMetalShardDispatch,
            dispatch_count: u32,
            total_jobs: u32,
            input_width: u32,
            output_width: u32,
            inputs_f32: *const f32,
            outputs_f32: *mut f32,
            faults_u32: *mut u32,
            out_gpu_ns: *mut u64,
        ) -> i32,
    >,
}

// Safety: Swift's MetalControllerEngine synchronizes internal MTLBuffer tables
// and only receives calls from the single simulation worker thread (or parity test).
unsafe impl Send for FlifeMetalVtable {}

#[repr(C)]
pub struct FlifeEpochView {
    pub epoch_id: u64,
    pub tick: u64,
    pub resource_epoch: u64,
    pub dt_seconds: f64,
    pub world_width: f64,
    pub world_height: f64,
    pub maximum_speed: f64,
    pub attempt_energy: f64,
    pub affinity_limit: f64,
    pub population_capacity: u32,

    pub organism_count: u32,
    pub ids: *const u64,
    pub positions: *const f32,
    pub velocities: *const f32,
    pub energies: *const f32,
    pub lineages: *const u64,
    pub generations: *const u32,
    pub topologies: *const u8,
    pub affinities: *const f32,
    pub cell_states: *const i8,
    pub polarities: *const i16,

    pub field_cells_x: u32,
    pub field_cells_y: u32,
    pub field_texels_x: u32,
    pub field_texels_y: u32,
    pub field_energy: *const f32,
    pub field_composition: *const f32,
    pub field_components: *const i8,
    pub field_trail_energy: *const f32,
    pub field_vascular: *const f32,

    pub live_count: u64,
    pub organism_energy_total: f64,
    pub field_energy_total: f64,
    pub max_generation: u32,
    pub total_events: u64,
    pub total_parameters: u64,
    pub genome_logical_bytes: u64,
    pub genome_allocated_bytes: u64,

    pub ledger_initial: f64,
    pub ledger_current: f64,
    pub ledger_injected: f64,
    pub ledger_removed: f64,
    pub ledger_dissipated: f64,
    pub ledger_turnover: f64,
    pub ledger_residual: f64,
    pub ledger_normalized: f64,
    pub has_ledger_normalized: bool,
    pub faulted: bool,

    pub has_tally: bool,
    pub tally_births: u64,
    pub tally_introductions: u64,
    pub tally_deaths: u64,
    pub tally_evaluations: u64,
    pub tally_rejected_births: u64,
    pub tally_refused_births: u64,
    pub tally_uptake: f64,

    pub has_followed: bool,
    pub followed_origin_id: u64,
    pub followed_organism_id: u64,
    pub followed_handoffs: u64,
    pub followed_alive: bool,

    pub sim_ticks_per_second: f64,
    pub last_tick_wall_ms: f64,
    pub last_gpu_ms: f64,
    pub metal_batched_evals: u64,
    pub metal_deferred_evals: u64,
    pub metal_resident_peak_bytes: u64,
    pub metal_total_device_ns: u64,
    pub metal_active: bool,
    pub is_playing: bool,
    pub is_capacity_paused: bool,
}

#[repr(C)]
pub struct FlifeOrganismInfo {
    pub found: bool,
    pub tick: u64,
    pub id: u64,
    pub genome_id: u64,
    pub has_parent: bool,
    pub parent_id: u64,
    pub lineage_id: u64,
    pub generation: u32,
    pub birth_tick: u64,
    pub age_ticks: u64,

    pub pos_x: f64,
    pub pos_y: f64,
    pub vel_x: f64,
    pub vel_y: f64,
    pub energy: f64,
    pub energy_change_rate: f64,

    pub topology_index: u32,
    pub hidden_layer_count: u32,
    pub hidden_widths: [u32; 4],
    pub parameter_count: u64,

    pub local_density: f64,
    pub local_request: f64,
    pub local_gain: f64,
    pub local_toxicity: f64,

    pub affinity: [f64; 32],

    pub has_secretory_readiness: bool,
    pub secretory_readiness: f64,
    pub has_neighbor_h0: bool,
    pub neighbor_h0: f64,

    pub has_descendants: bool,
    pub children_born: u64,
    pub children_alive: u64,
    pub descendants_born: u64,
    pub descendants_alive: u64,
}

impl Default for FlifeOrganismInfo {
    fn default() -> Self {
        Self {
            found: false,
            tick: 0,
            id: 0,
            genome_id: 0,
            has_parent: false,
            parent_id: 0,
            lineage_id: 0,
            generation: 0,
            birth_tick: 0,
            age_ticks: 0,
            pos_x: 0.0,
            pos_y: 0.0,
            vel_x: 0.0,
            vel_y: 0.0,
            energy: 0.0,
            energy_change_rate: 0.0,
            topology_index: 0,
            hidden_layer_count: 0,
            hidden_widths: [0; 4],
            parameter_count: 0,
            local_density: 0.0,
            local_request: 0.0,
            local_gain: 0.0,
            local_toxicity: 0.0,
            affinity: [0.0; 32],
            has_secretory_readiness: false,
            secretory_readiness: 0.0,
            has_neighbor_h0: false,
            neighbor_h0: 0.0,
            has_descendants: false,
            children_born: 0,
            children_alive: 0,
            descendants_born: 0,
            descendants_alive: 0,
        }
    }
}

#[repr(C)]
#[derive(Default)]
pub struct FlifeRegionMeasure {
    pub valid: bool,
    pub tick: u64,
    pub center_x: f64,
    pub center_y: f64,
    pub radius: f64,
    pub organisms: u64,
    pub organism_energy: f64,
    pub lineages: u64,
    pub cells: u64,
    pub field_energy: f64,
}

#[repr(C)]
pub struct FlifePresetSummary {
    pub valid: bool,
    pub name: [c_char; 68],
    pub model: [c_char; 68],
    pub color_mapping: [c_char; 68],
    pub weight_dtype: [c_char; 16],
    pub world_width: f64,
    pub world_height: f64,
    pub grid_width: u32,
    pub grid_height: u32,
    pub initial_count: u32,
    pub maximum_count: u32,
    pub tick_numerator: u32,
    pub tick_denominator: u32,
    pub max_logical_genome_bytes: u64,
    pub max_total_bytes: u64,
    pub v2_feature_mask: u64,
    pub has_contact: bool,
    pub has_v2_visuals: bool,
    pub has_trail_carrier: bool,
    pub has_vascular_field: bool,
    pub has_cell_state: bool,
    pub has_polarity: bool,
    pub error: [c_char; 256],
}

#[repr(C)]
pub struct FlifeParityResult {
    pub success: bool,
    pub digest_matched: bool,
    pub ticks_verified: u64,
    pub mismatch_tick: u64,
    pub live_final: u64,
    pub metal_batched: u64,
    pub metal_deferred: u64,
    pub metal_device_ns: u64,
    pub metal_wall_ns: u64,
    pub cpu_wall_ns: u64,
    pub ledger_residual: f64,
    pub metal_digest_hex: [c_char; 65],
    pub cpu_digest_hex: [c_char; 65],
    pub error: [c_char; 256],
}

/* =========================================================================
 * Helper Utilities
 * ========================================================================= */

fn write_c_str_buf(dst: *mut c_char, cap: usize, text: &str) {
    if dst.is_null() || cap == 0 {
        return;
    }
    let bytes = text.as_bytes();
    let n = bytes.len().min(cap - 1);
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr().cast::<c_char>(), dst, n);
        *dst.add(n) = 0;
    }
}

fn write_c_array<const N: usize>(dst: &mut [c_char; N], text: &str) {
    dst.fill(0);
    let bytes = text.as_bytes();
    let n = bytes.len().min(N - 1);
    for (slot, &b) in dst[..n].iter_mut().zip(&bytes[..n]) {
        *slot = b as c_char;
    }
}

unsafe fn read_c_str<'a>(ptr: *const c_char) -> Option<&'a str> {
    if ptr.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(ptr) }.to_str().ok()
}

/* =========================================================================
 * Exact Binary16 AoSoA Packing (matches `controller.metal:controller_forward_half`)
 * ========================================================================= */

fn pack_f16_for_metal_into(
    topology: &DenseTopology,
    values: &[F16],
    dst: &mut [u32],
) -> Result<usize, CoreError> {
    let word_count = values.len().div_ceil(2);
    if dst.len() < word_count {
        return Err(CoreError::LayoutMismatch {
            buffer: "ios metal binary16 words",
            detail: format!("dst len {} < required {word_count}", dst.len()),
        });
    }
    let mut cursor = 0_usize;
    let mut pending_lo: Option<u16> = None;
    let write_slice =
        |slice: &[F16], pending: &mut Option<u16>, out_dst: &mut [u32], out_cursor: &mut usize| {
            let mut iter = slice.iter();
            if let Some(lo) = pending.take() {
                if let Some(&hi) = iter.next() {
                    out_dst[*out_cursor] = u32::from(lo) | (u32::from(hi.to_bits()) << 16);
                    *out_cursor += 1;
                } else {
                    *pending = Some(lo);
                    return;
                }
            }
            let rem = iter.as_slice();
            let (pairs, tail) = rem.as_chunks::<2>();
            let next = *out_cursor + pairs.len();
            for (dst_word, [low, high]) in out_dst[*out_cursor..next].iter_mut().zip(pairs) {
                *dst_word = u32::from(low.to_bits()) | (u32::from(high.to_bits()) << 16);
            }
            *out_cursor = next;
            if let Some(&last) = tail.first() {
                *pending = Some(last.to_bits());
            }
        };

    let mut layer_offset = 0_usize;
    for layer in topology.layers() {
        let fan_in = layer.inputs;
        let fan_out = layer.outputs;
        let weight_count = fan_in.checked_mul(fan_out).ok_or(CoreError::Overflow {
            quantity: "ios metal f16 weight count",
        })?;
        let weights = values
            .get(layer.weights..layer.weights + weight_count)
            .ok_or_else(|| CoreError::LayoutMismatch {
                buffer: "ios metal binary16 weights",
                detail: "weight slice out of bounds".to_owned(),
            })?;
        let biases = values
            .get(layer.biases..layer.biases + fan_out)
            .ok_or_else(|| CoreError::LayoutMismatch {
                buffer: "ios metal binary16 biases",
                detail: "bias slice out of bounds".to_owned(),
            })?;
        if ((layer_offset | fan_in) & 3) == 0 && pending_lo.is_none() {
            let quads = fan_in >> 2;
            let (weight_quads, tail) = weights.as_chunks::<4>();
            if !tail.is_empty() || weight_quads.len() != fan_out * quads {
                return Err(CoreError::LayoutMismatch {
                    buffer: "ios metal binary16 weights",
                    detail: "AoSoA quad count mismatch".to_owned(),
                });
            }
            let layer_words = weight_count / 2;
            let row_stride = fan_out * 2;
            let layer_dst = &mut dst[cursor..cursor + layer_words];
            for (o, row_quads) in weight_quads.chunks_exact(quads).enumerate() {
                let o2 = o * 2;
                for (dst_q, w) in layer_dst.chunks_exact_mut(row_stride).zip(row_quads) {
                    dst_q[o2] = u32::from(w[0].to_bits()) | (u32::from(w[1].to_bits()) << 16);
                    dst_q[o2 + 1] = u32::from(w[2].to_bits()) | (u32::from(w[3].to_bits()) << 16);
                }
            }
            cursor += layer_words;
        } else {
            write_slice(weights, &mut pending_lo, dst, &mut cursor);
        }
        write_slice(biases, &mut pending_lo, dst, &mut cursor);
        layer_offset += weight_count + fan_out;
    }
    if let Some(lo) = pending_lo {
        dst[cursor] = u32::from(lo);
        cursor += 1;
    }
    Ok(cursor)
}

fn validate_shape_words(topology: &DenseTopology, length: usize) -> Option<[u32; 3]> {
    let input_width = topology.input_width();
    let output_width = topology.raw_output_width();
    if input_width == 0
        || input_width > MAX_SENSOR_WIDTH
        || output_width == 0
        || output_width > MAX_CONTROLLER_OUTPUT_WIDTH
        || length != topology.parameter_count()
    {
        return None;
    }
    let widths = topology.hidden_widths();
    if widths.is_empty()
        || widths.len() > 2
        || widths.iter().any(|&w| w == 0 || w as usize > WIDEST_HIDDEN)
    {
        return None;
    }
    let mut at = 0_usize;
    let mut fan_in = input_width;
    for (index, layer) in topology.layers().iter().enumerate() {
        let fan_out = widths
            .get(index)
            .map_or(output_width, |&width| width as usize);
        if layer.inputs != fan_in
            || layer.outputs != fan_out
            || layer.weights != at
            || layer.biases != at + fan_in * fan_out
        {
            return None;
        }
        at = layer.biases + fan_out;
        fan_in = fan_out;
    }
    if topology.layers().len() != widths.len() + 1 || at != length {
        return None;
    }
    Some([
        widths.len() as u32,
        widths.first().copied().unwrap_or(0),
        widths.get(1).copied().unwrap_or(0),
    ])
}

/* =========================================================================
 * Native iOS Metal Compute `ControllerBackend`
 * ========================================================================= */

struct IosMetalShard {
    handle: u64,
    /// Direct unified-memory CPU pointer (`MTLBuffer.contents()`) for lock-free
    /// parallel shard packing on Apple Silicon, or null to fall back to `write_shard`.
    contents: *mut u8,
    slot_capacity: usize,
    bytes: u64,
    live_count: usize,
    occupants: Vec<Option<(u32, GenomeId)>>,
    staged: Vec<u32>,
}

// Safety: `IosMetalShard` is owned exclusively by `IosMetalControllerBackend`
// on the single simulation worker thread; `contents` points into a shared
// `MTLBuffer` whose lifetime is tied to `handle` (freed on drop/shrink/grow).
unsafe impl Send for IosMetalShard {}

struct PendingShardUpload {
    job_idx: usize,
    topo_idx: usize,
    shard_key: u64,
    local: usize,
    shard_handle: u64,
    shard_contents: usize, // *mut u8 encoded as usize for Send across worker pool
    shard_bytes: u64,
    offset_elems: u64,
}

struct IosMetalBucket {
    topology: DenseTopology,
    dtype: WeightDtype,
    widest_hidden: usize,
    shape_words: Option<[u32; 3]>,
    geometry: Option<ShardGeometry>,
    shards: HashMap<u64, IosMetalShard>,
}

struct IosMetalControllerBackend {
    vtable: FlifeMetalVtable,
    device_name: String,
    cpu_workers: usize,
    buckets: Vec<Option<IosMetalBucket>>,
    held_bytes: u64,
    last_gpu_ns: u64,
    packed_f16: Vec<u32>,
    pending_uploads: Vec<PendingShardUpload>,
    staged_inputs: Vec<f32>,
    staged_outputs: Vec<f32>,
    staged_faults: Vec<u32>,
    dispatched_indices: Vec<usize>,
    touched: Vec<(usize, u64)>,
}

impl std::fmt::Debug for IosMetalControllerBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IosMetalControllerBackend")
            .field("device", &self.device_name)
            .field("held_bytes", &self.held_bytes)
            .finish_non_exhaustive()
    }
}

impl IosMetalControllerBackend {
    fn new(vtable: FlifeMetalVtable, cpu_workers: usize) -> Result<Self, CoreError> {
        if vtable.alloc_shard.is_none()
            || vtable.free_shard.is_none()
            || vtable.write_shard.is_none()
            || vtable.copy_shard.is_none()
            || vtable.dispatch_batch.is_none()
        {
            return Err(CoreError::MissingCapability {
                capability: "ios-metal-controller: incomplete Swift Metal vtable".to_owned(),
            });
        }
        let device_name = unsafe { read_c_str(vtable.device_name) }
            .unwrap_or("Apple iOS Metal GPU")
            .to_owned();
        Ok(Self {
            vtable,
            device_name,
            cpu_workers: cpu_workers.max(1),
            buckets: Vec::new(),
            held_bytes: 0,
            last_gpu_ns: 0,
            packed_f16: Vec::new(),
            pending_uploads: Vec::new(),
            staged_inputs: Vec::new(),
            staged_outputs: Vec::new(),
            staged_faults: Vec::new(),
            dispatched_indices: Vec::new(),
            touched: Vec::new(),
        })
    }

    fn alloc_device_shard(&self, bytes: u64) -> Result<(u64, *mut u8), CoreError> {
        let Some(f) = self.vtable.alloc_shard else {
            return Err(CoreError::MissingCapability {
                capability: "ios-metal-alloc: missing alloc_shard".to_owned(),
            });
        };
        let mut contents: *mut u8 = ptr::null_mut();
        let handle = unsafe { f(self.vtable.ctx, bytes, &mut contents) };
        if handle == 0 {
            Err(CoreError::AllocationFailed {
                buffer: "ios metal parameter shard",
                bytes,
            })
        } else {
            Ok((handle, contents))
        }
    }

    fn free_device_shard(&self, handle: u64) {
        if handle != 0
            && let Some(f) = self.vtable.free_shard
        {
            unsafe { f(self.vtable.ctx, handle) };
        }
    }

    fn write_device_shard(
        &self,
        handle: u64,
        byte_offset: u64,
        bytes: &[u8],
    ) -> Result<(), CoreError> {
        if bytes.is_empty() {
            return Ok(());
        }
        let Some(f) = self.vtable.write_shard else {
            return Err(CoreError::MissingCapability {
                capability: "ios-metal-write: missing write_shard".to_owned(),
            });
        };
        let rc = unsafe {
            f(
                self.vtable.ctx,
                handle,
                byte_offset,
                bytes.as_ptr(),
                bytes.len() as u64,
            )
        };
        if rc != 0 {
            return Err(CoreError::StateConflict {
                operation: "ios metal write_shard",
                detail: format!("Swift Metal write_shard returned error code {rc}"),
            });
        }
        Ok(())
    }

    fn copy_device_shard(&self, src: u64, dst: u64, byte_len: u64) -> Result<(), CoreError> {
        if byte_len == 0 {
            return Ok(());
        }
        let Some(f) = self.vtable.copy_shard else {
            return Err(CoreError::MissingCapability {
                capability: "ios-metal-copy: missing copy_shard".to_owned(),
            });
        };
        let rc = unsafe { f(self.vtable.ctx, src, dst, byte_len) };
        if rc != 0 {
            return Err(CoreError::StateConflict {
                operation: "ios metal copy_shard",
                detail: format!("Swift Metal copy_shard returned error code {rc}"),
            });
        }
        Ok(())
    }

    fn clear_all_shards(&mut self) {
        let free_fn = self.vtable.free_shard;
        let ctx = self.vtable.ctx;
        for bucket in self.buckets.drain(..).flatten() {
            for shard in bucket.shards.into_values() {
                if shard.handle != 0
                    && let Some(f) = free_fn
                {
                    unsafe { f(ctx, shard.handle) };
                }
            }
        }
        self.held_bytes = 0;
        self.last_gpu_ns = 0;
    }

    fn ensure_bucket(&mut self, job: &FlatControllerJob<'_>) -> Option<usize> {
        let topo_idx = usize::try_from(job.slot.topology()).ok()?;
        if self.buckets.len() <= topo_idx {
            self.buckets.resize_with(topo_idx + 1, || None);
        }
        let ctrl = job.controller;
        let topology = ctrl.topology();
        let params = ctrl.parameters();
        let length = params.len();
        let dtype = params.dtype();
        let max_buf = self
            .vtable
            .max_buffer_bytes
            .clamp(16 * 1024 * 1024, SHARD_BYTES_CAP);

        let bucket = self.buckets[topo_idx].get_or_insert_with(|| {
            let elem_limit = (max_buf / dtype.bytes()).min(u64::from(u32::MAX));
            let shape_words = validate_shape_words(topology, length);
            let widest_hidden =
                topology.hidden_widths().iter().copied().max().unwrap_or(0) as usize;
            let geometry = shape_words.and_then(|_| {
                ShardGeometry::fitting_for_dtype(
                    length as u64,
                    dtype,
                    elem_limit,
                    DEFAULT_SHARD_SLOTS,
                )
                .ok()
            });
            IosMetalBucket {
                topology: topology.clone(),
                dtype,
                widest_hidden,
                shape_words,
                geometry,
                shards: HashMap::new(),
            }
        });
        let ok = bucket.shape_words.is_some()
            && bucket.dtype == dtype
            && bucket.topology == *topology
            && bucket
                .geometry
                .is_some_and(|g| g.parameters() == length as u64);
        ok.then_some(topo_idx)
    }

    fn upload_genome_to_shard_fallback(
        &mut self,
        handle: u64,
        offset_elems: u64,
        topology: &DenseTopology,
        weights: Weights<'_>,
    ) -> Result<(), CoreError> {
        match weights {
            Weights::F32(values) => {
                let byte_offset = offset_elems * 4;
                let bytes = unsafe {
                    std::slice::from_raw_parts(values.as_ptr().cast::<u8>(), values.len() * 4)
                };
                self.write_device_shard(handle, byte_offset, bytes)?;
            }
            Weights::F16(values) => {
                let word_count = values.len().div_ceil(2);
                if self.packed_f16.len() < word_count {
                    self.packed_f16.resize(word_count, 0);
                }
                let written =
                    pack_f16_for_metal_into(topology, values, &mut self.packed_f16[..word_count])?;
                let byte_offset = offset_elems * 2;
                let bytes = unsafe {
                    std::slice::from_raw_parts(self.packed_f16.as_ptr().cast::<u8>(), written * 4)
                };
                self.write_device_shard(handle, byte_offset, bytes)?;
            }
        }
        Ok(())
    }

    fn write_genome_direct(
        shard_contents: *mut u8,
        shard_bytes: u64,
        offset_elems: u64,
        topology: &DenseTopology,
        weights: Weights<'_>,
        scratch: &mut Vec<u32>,
    ) -> Result<(), CoreError> {
        match weights {
            Weights::F32(values) => {
                let byte_offset = offset_elems.checked_mul(4).ok_or(CoreError::Overflow {
                    quantity: "ios metal f32 byte offset",
                })?;
                let byte_len = (values.len() as u64)
                    .checked_mul(4)
                    .ok_or(CoreError::Overflow {
                        quantity: "ios metal f32 byte len",
                    })?;
                let end = byte_offset
                    .checked_add(byte_len)
                    .ok_or(CoreError::Overflow {
                        quantity: "ios metal f32 byte end",
                    })?;
                if end > shard_bytes {
                    return Err(CoreError::LayoutMismatch {
                        buffer: "ios metal f32 shard",
                        detail: format!("write end {end} exceeds shard bytes {shard_bytes}"),
                    });
                }
                unsafe {
                    ptr::copy_nonoverlapping(
                        values.as_ptr().cast::<u8>(),
                        shard_contents.add(byte_offset as usize),
                        byte_len as usize,
                    );
                }
            }
            Weights::F16(values) => {
                let word_count = values.len().div_ceil(2);
                let byte_offset = offset_elems.checked_mul(2).ok_or(CoreError::Overflow {
                    quantity: "ios metal f16 byte offset",
                })?;
                let byte_len = (word_count as u64)
                    .checked_mul(4)
                    .ok_or(CoreError::Overflow {
                        quantity: "ios metal f16 byte len",
                    })?;
                let end = byte_offset
                    .checked_add(byte_len)
                    .ok_or(CoreError::Overflow {
                        quantity: "ios metal f16 byte end",
                    })?;
                if end > shard_bytes {
                    return Err(CoreError::LayoutMismatch {
                        buffer: "ios metal f16 shard",
                        detail: format!("write end {end} exceeds shard bytes {shard_bytes}"),
                    });
                }
                let dst_ptr = unsafe { shard_contents.add(byte_offset as usize) };
                if (dst_ptr as usize).is_multiple_of(align_of::<u32>()) {
                    let dst_words = unsafe {
                        std::slice::from_raw_parts_mut(dst_ptr.cast::<u32>(), word_count)
                    };
                    pack_f16_for_metal_into(topology, values, dst_words)?;
                } else {
                    if scratch.len() < word_count {
                        scratch.resize(word_count, 0);
                    }
                    let written =
                        pack_f16_for_metal_into(topology, values, &mut scratch[..word_count])?;
                    unsafe {
                        ptr::copy_nonoverlapping(
                            scratch.as_ptr().cast::<u8>(),
                            dst_ptr,
                            written * 4,
                        );
                    }
                }
            }
        }
        Ok(())
    }

    fn flush_pending_uploads(&mut self, jobs: &[FlatControllerJob<'_>]) -> Result<(), CoreError> {
        if self.pending_uploads.is_empty() {
            return Ok(());
        }

        // Refresh final shard pointers/handles and drop any upload that was
        // superseded by a later occupant in the same batch (guaranteeing strictly
        // disjoint destination ranges for parallel unified-memory packing).
        let mut live_uploads = std::mem::take(&mut self.pending_uploads);
        live_uploads.retain_mut(|u| {
            let job = &jobs[u.job_idx];
            let Some(bucket) = self.buckets.get(u.topo_idx).and_then(Option::as_ref) else {
                return false;
            };
            let Some(shard) = bucket.shards.get(&u.shard_key) else {
                return false;
            };
            if shard.occupants.get(u.local).copied().flatten()
                != Some((job.slot.generation(), job.genome))
            {
                return false;
            }
            u.shard_handle = shard.handle;
            u.shard_contents = shard.contents as usize;
            u.shard_bytes = shard.bytes;
            true
        });

        // When called via `World::step()` with `prefers_cpu_overlap() == true`,
        // `flife_core::pool::overlap_off_cpu` runs `evaluate_flat_f32` on a pool
        // worker thread (`ON_POOL_THREAD == true`), so `pool::parallel_for_each_mut`
        // executes serially on that overlap thread alongside `cpu_workers - 1`
        // chemistry threads (preserving the single-worker budget). Standalone
        // callers outside `overlap_off_cpu` parallelize across `self.cpu_workers`.
        let all_direct = live_uploads.iter().all(|u| u.shard_contents != 0);
        let result = if all_direct && live_uploads.len() >= 16 && self.cpu_workers > 1 {
            let workers = self.cpu_workers.min(live_uploads.len());
            let chunk_len = live_uploads.len().div_ceil(workers);
            let mut results: Vec<Result<(), CoreError>> = vec![Ok(()); workers];
            let uploads_ref = &live_uploads;
            pool::parallel_for_each_mut(
                workers,
                "ios metal parallel shard pack",
                &mut results,
                |chunk_idx, slot_res| {
                    let start = chunk_idx * chunk_len;
                    if start >= uploads_ref.len() {
                        return;
                    }
                    let end = (start + chunk_len).min(uploads_ref.len());
                    let mut scratch = Vec::new();
                    for u in &uploads_ref[start..end] {
                        let job = &jobs[u.job_idx];
                        if let Err(err) = Self::write_genome_direct(
                            u.shard_contents as *mut u8,
                            u.shard_bytes,
                            u.offset_elems,
                            job.controller.topology(),
                            job.controller.parameters(),
                            &mut scratch,
                        ) {
                            *slot_res = Err(err);
                            return;
                        }
                    }
                },
            )?;
            results.into_iter().collect::<Result<(), CoreError>>()
        } else {
            let mut res = Ok(());
            for u in &live_uploads {
                let job = &jobs[u.job_idx];
                let step = if u.shard_contents != 0 {
                    Self::write_genome_direct(
                        u.shard_contents as *mut u8,
                        u.shard_bytes,
                        u.offset_elems,
                        job.controller.topology(),
                        job.controller.parameters(),
                        &mut self.packed_f16,
                    )
                } else {
                    self.upload_genome_to_shard_fallback(
                        u.shard_handle,
                        u.offset_elems,
                        job.controller.topology(),
                        job.controller.parameters(),
                    )
                };
                if let Err(err) = step {
                    res = Err(err);
                    break;
                }
            }
            res
        };

        live_uploads.clear();
        self.pending_uploads = live_uploads;
        result
    }

    fn run_flat(
        &mut self,
        jobs: &[FlatControllerJob<'_>],
        input_width: usize,
        inputs_f32: &[f32],
        inputs_finite: &[u8],
        outputs: &mut FlatOutputs,
    ) -> Result<(), CoreError> {
        self.last_gpu_ns = 0;
        let expected_inputs = jobs
            .len()
            .checked_mul(input_width)
            .ok_or(CoreError::Overflow {
                quantity: "ios flat controller inputs",
            })?;
        if inputs_f32.len() != expected_inputs || inputs_finite.len() != jobs.len() {
            outputs.clear();
            return Err(CoreError::LayoutMismatch {
                buffer: "ios flat controller inputs",
                detail: "input slice length mismatch".to_owned(),
            });
        }
        let Some(first) = jobs.first() else {
            outputs.clear();
            return Ok(());
        };
        let output_width = jobs
            .iter()
            .find_map(|job| {
                let t = job.controller.topology();
                (t.input_width() == input_width).then_some(t.raw_output_width())
            })
            .unwrap_or_else(|| first.controller.topology().raw_output_width());
        outputs.prepare(jobs.len(), output_width)?;

        if input_width == 0
            || input_width > MAX_SENSOR_WIDTH
            || output_width == 0
            || output_width > MAX_CONTROLLER_OUTPUT_WIDTH
        {
            return Ok(());
        }

        for bucket in self.buckets.iter_mut().flatten() {
            for shard in bucket.shards.values_mut() {
                shard.staged.clear();
            }
        }
        self.dispatched_indices.clear();
        self.touched.clear();
        self.pending_uploads.clear();

        let mut overwrote_existing = false;

        for (index, (job, &is_finite)) in jobs.iter().zip(inputs_finite).enumerate() {
            if is_finite == 0 {
                continue;
            }
            let topo = job.controller.topology();
            if topo.input_width() != input_width || topo.raw_output_width() != output_width {
                continue;
            }
            let Some(topo_idx) = self.ensure_bucket(job) else {
                continue;
            };
            let (geometry, dtype, shape_words) = {
                let Some(b) = self.buckets[topo_idx].as_ref() else {
                    continue;
                };
                let (Some(g), Some(sw)) = (b.geometry, b.shape_words) else {
                    continue;
                };
                (g, b.dtype, sw)
            };

            let at = geometry.locate(job.slot.slot());
            let Ok(local) = usize::try_from(at.slot_in_shard) else {
                continue;
            };
            let Ok(slots_per_shard) = usize::try_from(geometry.slots_per_shard()) else {
                continue;
            };
            if local >= slots_per_shard {
                continue;
            }
            let Ok(offset_word) = u32::try_from(at.offset) else {
                continue;
            };

            let stride = geometry.slot_stride_elements();
            let elem_bytes = dtype.bytes();
            let existing_cap = self.buckets[topo_idx]
                .as_ref()
                .and_then(|b| b.shards.get(&at.shard))
                .map(|s| s.slot_capacity);

            if existing_cap.is_none_or(|cap| local >= cap) {
                // Lazy look-ahead ONLY when allocating or growing a shard (zero steady-state overhead):
                // find the max slot for this (topology, shard) across remaining jobs so each shard
                // allocates ONCE at its final power-of-two capacity.
                let mut target_local = local;
                for (later_job, &later_fin) in
                    jobs[index + 1..].iter().zip(&inputs_finite[index + 1..])
                {
                    if later_fin == 0 {
                        continue;
                    }
                    if later_job.slot.topology() as usize != topo_idx {
                        continue;
                    }
                    let later_at = geometry.locate(later_job.slot.slot());
                    if later_at.shard == at.shard
                        && let Ok(ll) = usize::try_from(later_at.slot_in_shard)
                        && ll < slots_per_shard
                    {
                        target_local = target_local.max(ll);
                    }
                }
                let needed = (target_local + 1)
                    .next_power_of_two()
                    .clamp(256, slots_per_shard);
                let new_elems = (needed as u64) * stride;
                let new_bytes = new_elems
                    .checked_mul(elem_bytes)
                    .ok_or(CoreError::Overflow {
                        quantity: "ios metal shard bytes",
                    })?
                    .max(16);
                let (new_handle, new_contents) = match self.alloc_device_shard(new_bytes) {
                    Ok(pair) => pair,
                    Err(_) => continue, // defer gracefully if device memory tight
                };

                if let Some(old_cap) = existing_cap {
                    let (old_handle, old_bytes) = {
                        let shard = self.buckets[topo_idx]
                            .as_ref()
                            .and_then(|b| b.shards.get(&at.shard))
                            .ok_or_else(|| CoreError::StateConflict {
                                operation: "ios metal grow shard",
                                detail: "missing existing shard".to_owned(),
                            })?;
                        (shard.handle, shard.bytes)
                    };
                    let copy_bytes = ((old_cap as u64) * stride * elem_bytes).min(old_bytes);
                    if let Err(err) = self.copy_device_shard(old_handle, new_handle, copy_bytes) {
                        self.free_device_shard(new_handle);
                        return Err(err);
                    }
                    self.free_device_shard(old_handle);
                    self.held_bytes = self
                        .held_bytes
                        .saturating_sub(old_bytes)
                        .saturating_add(new_bytes);
                    if let Some(shard) = self.buckets[topo_idx]
                        .as_mut()
                        .and_then(|b| b.shards.get_mut(&at.shard))
                    {
                        shard.handle = new_handle;
                        shard.contents = new_contents;
                        shard.slot_capacity = needed;
                        shard.bytes = new_bytes;
                        shard.occupants.resize(needed, None);
                    }
                } else {
                    self.held_bytes = self.held_bytes.saturating_add(new_bytes);
                    if let Some(bucket) = self.buckets[topo_idx].as_mut() {
                        bucket.shards.insert(
                            at.shard,
                            IosMetalShard {
                                handle: new_handle,
                                contents: new_contents,
                                slot_capacity: needed,
                                bytes: new_bytes,
                                live_count: 0,
                                occupants: vec![None; needed],
                                staged: Vec::new(),
                            },
                        );
                    }
                }
            }

            let own = Some((job.slot.generation(), job.genome));
            if let Some(shard) = self.buckets[topo_idx]
                .as_mut()
                .and_then(|b| b.shards.get_mut(&at.shard))
            {
                if shard.occupants[local] != own {
                    if shard.occupants[local].take().is_some() {
                        shard.live_count = shard.live_count.saturating_sub(1);
                        overwrote_existing = true;
                    }
                    shard.occupants[local] = own;
                    shard.live_count += 1;
                    self.pending_uploads.push(PendingShardUpload {
                        job_idx: index,
                        topo_idx,
                        shard_key: at.shard,
                        local,
                        shard_handle: shard.handle,
                        shard_contents: shard.contents as usize,
                        shard_bytes: shard.bytes,
                        offset_elems: at.offset,
                    });
                }
            } else {
                continue;
            }

            let row = self.dispatched_indices.len();
            self.dispatched_indices.push(index);
            if !overwrote_existing
                && let Some(shard) = self.buckets[topo_idx]
                    .as_mut()
                    .and_then(|b| b.shards.get_mut(&at.shard))
            {
                if shard.staged.is_empty() {
                    self.touched.push((topo_idx, at.shard));
                }
                shard.staged.extend_from_slice(&[
                    row as u32,
                    offset_word,
                    shape_words[0],
                    shape_words[1],
                    shape_words[2],
                ]);
            }
        }

        // Flush all newborn / mutated controller weights directly into Apple
        // Silicon unified memory (`MTLBuffer.contents()`), zero FFI hops!
        // If any upload fails, invalidate all resident shards so `occupants`
        // never falsely marks unwritten slots as resident on a subsequent call.
        if let Err(err) = self.flush_pending_uploads(jobs) {
            self.clear_all_shards();
            return Err(err);
        }

        if overwrote_existing {
            // Rebuild staged lists cleanly dropping any evicted earlier duplicate slot.
            self.touched.clear();
            for bucket in self.buckets.iter_mut().flatten() {
                for shard in bucket.shards.values_mut() {
                    shard.staged.clear();
                }
            }
            let candidates = std::mem::take(&mut self.dispatched_indices);
            for idx in candidates {
                let job = &jobs[idx];
                let Ok(topo_idx) = usize::try_from(job.slot.topology()) else {
                    continue;
                };
                let Some(bucket) = self.buckets.get_mut(topo_idx).and_then(Option::as_mut) else {
                    continue;
                };
                let (Some(geometry), Some(shape_words)) = (bucket.geometry, bucket.shape_words)
                else {
                    continue;
                };
                let at = geometry.locate(job.slot.slot());
                let Some(shard) = bucket.shards.get_mut(&at.shard) else {
                    continue;
                };
                let local = at.slot_in_shard as usize;
                if shard.occupants.get(local).copied().flatten()
                    != Some((job.slot.generation(), job.genome))
                {
                    continue;
                }
                let row = self.dispatched_indices.len();
                self.dispatched_indices.push(idx);
                if shard.staged.is_empty() {
                    self.touched.push((topo_idx, at.shard));
                }
                shard.staged.extend_from_slice(&[
                    row as u32,
                    at.offset as u32,
                    shape_words[0],
                    shape_words[1],
                    shape_words[2],
                ]);
            }
        }

        let dispatched_count = self.dispatched_indices.len();
        if dispatched_count == 0 {
            return Ok(());
        }

        // Gather compact inputs if not all jobs were dispatched; otherwise borrow inputs_f32 directly!
        let all_dispatched = dispatched_count == jobs.len() && !overwrote_existing;
        let dispatch_inputs_ptr: *const f32 = if all_dispatched {
            inputs_f32.as_ptr()
        } else {
            let need = dispatched_count * input_width;
            self.staged_inputs.clear();
            self.staged_inputs.reserve(need);
            for &orig_idx in &self.dispatched_indices {
                let start = orig_idx * input_width;
                self.staged_inputs
                    .extend_from_slice(&inputs_f32[start..start + input_width]);
            }
            self.staged_inputs.as_ptr()
        };

        let out_elems = dispatched_count * output_width;
        let (out_ptr, use_direct_out): (*mut f32, bool) = if all_dispatched {
            (outputs.values.as_mut_ptr(), true)
        } else {
            if self.staged_outputs.len() < out_elems {
                self.staged_outputs.resize(out_elems, 0.0);
            }
            (self.staged_outputs.as_mut_ptr(), false)
        };
        if self.staged_faults.len() < dispatched_count {
            self.staged_faults.resize(dispatched_count, 0);
        }

        let mut dispatches = Vec::with_capacity(self.touched.len());
        for &(topo_idx, shard_key) in &self.touched {
            if let Some(bucket) = self.buckets[topo_idx].as_ref()
                && let Some(shard) = bucket.shards.get(&shard_key)
                && !shard.staged.is_empty()
            {
                dispatches.push(FlifeMetalShardDispatch {
                    shard_handle: shard.handle,
                    job_words: shard.staged.as_ptr(),
                    job_count: (shard.staged.len() / JOB_WORDS) as u32,
                    threads_per_tg: forward_threads(bucket.widest_hidden),
                    is_f16: u32::from(bucket.dtype == WeightDtype::F16),
                });
            }
        }

        let Some(dispatch_fn) = self.vtable.dispatch_batch else {
            return Err(CoreError::MissingCapability {
                capability: "ios-metal-dispatch: missing dispatch_batch".to_owned(),
            });
        };
        let mut gpu_ns = 0_u64;
        let rc = unsafe {
            dispatch_fn(
                self.vtable.ctx,
                dispatches.as_ptr(),
                dispatches.len() as u32,
                dispatched_count as u32,
                input_width as u32,
                output_width as u32,
                dispatch_inputs_ptr,
                out_ptr,
                self.staged_faults.as_mut_ptr(),
                &mut gpu_ns,
            )
        };
        if rc != 0 {
            outputs.clear();
            return Err(CoreError::StateConflict {
                operation: "ios metal dispatch_batch",
                detail: format!("Metal GPU dispatch failed with code {rc}"),
            });
        }
        self.last_gpu_ns = gpu_ns;

        if use_direct_out {
            for (i, &fault) in self.staged_faults[..dispatched_count].iter().enumerate() {
                outputs.valid[i] = u8::from(fault == 0);
            }
        } else {
            for (row, &orig_idx) in self.dispatched_indices.iter().enumerate() {
                if self.staged_faults[row] == 0 {
                    let src = row * output_width;
                    let dst = orig_idx * output_width;
                    outputs.values[dst..dst + output_width]
                        .copy_from_slice(&self.staged_outputs[src..src + output_width]);
                    outputs.valid[orig_idx] = 1;
                }
            }
        }
        Ok(())
    }
}

impl Drop for IosMetalControllerBackend {
    fn drop(&mut self) {
        self.clear_all_shards();
    }
}

impl ControllerBackend for IosMetalControllerBackend {
    fn describe(&self) -> String {
        format!("ios-metal:{}", self.device_name)
    }

    fn resident_bytes(&self) -> u64 {
        self.held_bytes
    }

    fn forget(&mut self) {
        self.clear_all_shards();
    }

    fn release_slots(&mut self, slots: &[ArenaSlot]) {
        for &slot in slots {
            let Ok(topo_idx) = usize::try_from(slot.topology()) else {
                continue;
            };
            let Some(bucket) = self.buckets.get_mut(topo_idx).and_then(Option::as_mut) else {
                continue;
            };
            let Some(geometry) = bucket.geometry else {
                continue;
            };
            let at = geometry.locate(slot.slot());
            let Ok(local) = usize::try_from(at.slot_in_shard) else {
                continue;
            };
            let mut reclaim = false;
            if let Some(shard) = bucket.shards.get_mut(&at.shard)
                && let Some(occ) = shard.occupants.get_mut(local)
                && occ.is_some_and(|(slot_gen, _)| slot_gen == slot.generation())
            {
                *occ = None;
                shard.live_count = shard.live_count.saturating_sub(1);
                reclaim = shard.live_count == 0;
            }
            if reclaim && let Some(removed) = bucket.shards.remove(&at.shard) {
                self.held_bytes = self.held_bytes.saturating_sub(removed.bytes);
                self.free_device_shard(removed.handle);
            }
        }
    }

    fn last_device_nanoseconds(&self) -> u64 {
        self.last_gpu_ns
    }

    fn supports_flat_f32(&self) -> bool {
        true
    }

    fn prefers_cpu_overlap(&self) -> bool {
        true
    }

    fn evaluate_flat_f32(
        &mut self,
        jobs: &[FlatControllerJob<'_>],
        input_width: usize,
        inputs_f32: &[f32],
        inputs_finite: &[u8],
        outputs: &mut FlatOutputs,
    ) -> Result<(), CoreError> {
        self.run_flat(jobs, input_width, inputs_f32, inputs_finite, outputs)
    }

    fn evaluate(
        &mut self,
        jobs: &[ControllerJob<'_>],
        outputs: &mut Vec<Option<RawOutputs>>,
    ) -> Result<(), CoreError> {
        outputs.clear();
        let Some(first) = jobs.first() else {
            self.last_gpu_ns = 0;
            return Ok(());
        };
        let input_width = first.controller.topology().input_width();
        let mut flat_jobs = Vec::with_capacity(jobs.len());
        let mut flat_inputs = Vec::with_capacity(jobs.len() * input_width);
        let mut flat_finite = Vec::with_capacity(jobs.len());
        for job in jobs {
            flat_jobs.push(FlatControllerJob {
                genome: job.genome,
                slot: job.slot,
                controller: job.controller,
            });
            if job.inputs.len() != input_width {
                flat_inputs.extend(std::iter::repeat_n(0.0_f32, input_width));
                flat_finite.push(0);
            } else {
                let mut ok = true;
                for &v in job.inputs {
                    let f = v as f32;
                    ok &= f.is_finite();
                    flat_inputs.push(f);
                }
                flat_finite.push(u8::from(ok));
            }
        }
        let mut flat_out = FlatOutputs::default();
        self.run_flat(
            &flat_jobs,
            input_width,
            &flat_inputs,
            &flat_finite,
            &mut flat_out,
        )?;
        let out_w = flat_out.output_width;
        let template = RawOutputs::zeroed(out_w)?;
        outputs.resize(jobs.len(), None);
        for (i, slot) in outputs.iter_mut().enumerate() {
            if let Some(row) = flat_out.row(i) {
                let mut raw = template;
                raw.as_mut_slice().copy_from_slice(row);
                *slot = Some(raw);
            }
        }
        Ok(())
    }
}

/* =========================================================================
 * Live Simulation Session (Dedicated Worker Thread + Lock-Free Epoch Feed)
 * ========================================================================= */

#[derive(Default, Clone, Copy)]
struct LiveTelemetry {
    sim_ticks_per_second: f64,
    last_tick_wall_ms: f64,
    last_gpu_ms: f64,
    metal_batched_evals: u64,
    metal_deferred_evals: u64,
    metal_resident_peak_bytes: u64,
    metal_total_device_ns: u64,
    metal_active: bool,
    is_playing: bool,
    is_capacity_paused: bool,
}

type InspectReplySlot = Arc<(Mutex<Option<Option<OrganismSnapshot>>>, Condvar)>;
type MeasureReplySlot = Arc<(Mutex<Option<Result<RegionMeasurement, CoreError>>>, Condvar)>;

enum SyncQuery {
    Inspect(OrganismId, InspectReplySlot),
    Measure([f64; 2], f64, MeasureReplySlot),
}

struct SharedControl {
    playing: bool,
    capacity_paused: bool,
    worker_stepping: bool,
    step_requests: u64,
    target_tps: f64, // 0.0 = uncapped
    shutdown: bool,
    pending_interventions: Vec<Intervention>,
    watch_update: Option<Option<OrganismId>>,
    follow_update: Option<Option<OrganismId>>,
    queries: Vec<SyncQuery>,
}

pub struct FlifeSession {
    control: Arc<(Mutex<SharedControl>, Condvar)>,
    telemetry: Arc<Mutex<LiveTelemetry>>,
    viewer: Mutex<Viewer>,
    #[allow(dead_code)]
    feed: Feed,
    dt_seconds: f64,
    world_width: f64,
    world_height: f64,
    grid_x: u32,
    grid_y: u32,
    worker: Option<JoinHandle<()>>,
    running: Arc<AtomicBool>,
    epoch_counter: AtomicU64,
}

pub struct FlifeEpochHandle {
    _lease: Lease,
}

fn fill_organism_info(snap: &OrganismSnapshot, out: &mut FlifeOrganismInfo) {
    *out = FlifeOrganismInfo::default();
    out.found = true;
    out.tick = snap.tick.get();
    out.id = snap.organism.id.get();
    out.genome_id = snap.organism.genome.get();
    if let Some(p) = snap.organism.parent {
        out.has_parent = true;
        out.parent_id = p.get();
    }
    out.lineage_id = snap.organism.lineage.get();
    out.generation = snap.organism.generation.get();
    out.birth_tick = snap.organism.birth_tick.get();
    out.age_ticks = snap.age_ticks;
    out.pos_x = snap.organism.position[0];
    out.pos_y = snap.organism.position[1];
    out.vel_x = snap.organism.velocity[0];
    out.vel_y = snap.organism.velocity[1];
    out.energy = snap.organism.energy;
    out.energy_change_rate = snap.organism.energy_change_rate;
    out.topology_index = snap.topology as u32;
    let hl = snap.hidden_widths.len().min(4);
    out.hidden_layer_count = hl as u32;
    for (dst, &w) in out.hidden_widths[..hl]
        .iter_mut()
        .zip(&snap.hidden_widths[..hl])
    {
        *dst = w;
    }
    out.parameter_count = snap.parameters as u64;
    out.local_density = snap.local.density;
    out.local_request = snap.local.request;
    out.local_gain = snap.local.gain;
    out.local_toxicity = snap.local.toxicity;
    let aff = snap
        .effective_affinity
        .as_ref()
        .unwrap_or_else(|| snap.traits.affinity());
    out.affinity.copy_from_slice(aff);
    if let Some(sr) = snap.secretory_readiness {
        out.has_secretory_readiness = true;
        out.secretory_readiness = sr;
    }
    if let Some(nh) = snap.last_neighbor_h0 {
        out.has_neighbor_h0 = true;
        out.neighbor_h0 = nh;
    }
    if let Some(desc) = &snap.descendants {
        out.has_descendants = true;
        out.children_born = desc.children_born;
        out.children_alive = desc.children_living;
        out.descendants_born = desc.born;
        out.descendants_alive = desc.living;
    }
}

fn process_sync_queries(world: &World, queries: Vec<SyncQuery>) {
    for q in queries {
        match q {
            SyncQuery::Inspect(id, slot) => {
                let res = world.organism_snapshot(id).ok().flatten();
                let (lock, cvar) = &*slot;
                if let Ok(mut guard) = lock.lock() {
                    *guard = Some(res);
                    cvar.notify_all();
                }
            }
            SyncQuery::Measure(center, radius, slot) => {
                let res = world.measure_region(center, radius);
                let (lock, cvar) = &*slot;
                if let Ok(mut guard) = lock.lock() {
                    *guard = Some(res);
                    cvar.notify_all();
                }
            }
        }
    }
}

fn worker_loop(
    mut world: World,
    mut publisher: Publisher,
    control: Arc<(Mutex<SharedControl>, Condvar)>,
    telemetry: Arc<Mutex<LiveTelemetry>>,
    running: Arc<AtomicBool>,
) {
    let mut ema_tps = 0.0_f64;
    let mut prev_device_ns = 0_u64;

    loop {
        let (should_step, target_tps, interventions, watch_upd, follow_upd, queries) = {
            let (lock, cvar) = &*control;
            let mut guard = match lock.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            if guard.worker_stepping {
                guard.worker_stepping = false;
                cvar.notify_all();
            }
            while !guard.shutdown
                && (!guard.playing || guard.capacity_paused)
                && guard.step_requests == 0
                && guard.queries.is_empty()
                && guard.watch_update.is_none()
                && guard.follow_update.is_none()
            {
                // Ensure any pending epoch boundary gets published while paused if a lease freed.
                drop(guard);
                let _ = publisher.offer(&world);
                guard = match lock.lock() {
                    Ok(g) => g,
                    Err(p) => p.into_inner(),
                };
                if guard.shutdown
                    || (guard.playing && !guard.capacity_paused)
                    || guard.step_requests > 0
                    || !guard.queries.is_empty()
                    || guard.watch_update.is_some()
                    || guard.follow_update.is_some()
                {
                    break;
                }
                guard = match cvar.wait_timeout(guard, Duration::from_millis(16)) {
                    Ok((g, _)) => g,
                    Err(p) => p.into_inner().0,
                };
            }
            if guard.shutdown {
                let pending_q = std::mem::take(&mut guard.queries);
                guard.worker_stepping = false;
                cvar.notify_all();
                drop(guard);
                process_sync_queries(&world, pending_q);
                break;
            }
            let do_step = if guard.step_requests > 0 {
                guard.step_requests -= 1;
                guard.capacity_paused = false;
                true
            } else {
                guard.playing && !guard.capacity_paused
            };
            if do_step {
                guard.worker_stepping = true;
            }
            let target_tps = guard.target_tps;
            let interventions = if do_step {
                std::mem::take(&mut guard.pending_interventions)
            } else {
                Vec::new()
            };
            let watch_upd = guard.watch_update.take();
            let follow_upd = guard.follow_update.take();
            let queries = std::mem::take(&mut guard.queries);
            (
                do_step,
                target_tps,
                interventions,
                watch_upd,
                follow_upd,
                queries,
            )
        };

        if let Some(w) = watch_upd {
            world.watch(w);
        }
        if let Some(f) = follow_upd {
            let _ = world.follow(f);
            let _ = publisher.offer(&world);
        }
        if !queries.is_empty() {
            process_sync_queries(&world, queries);
        }

        if should_step {
            let t0 = Instant::now();
            let step_result = if interventions.is_empty() {
                world.step()
            } else {
                world.step_with(&interventions)
            };
            let elapsed = t0.elapsed();
            let wall_ms = elapsed.as_secs_f64() * 1000.0;

            let paused_now = match step_result {
                Ok(Stepped::Published(report)) => {
                    publisher.count(&report);
                    let _ = publisher.offer(&world);
                    false
                }
                Ok(Stepped::Paused(_)) | Err(_) => {
                    // Explicitly halt playing so the worker parks cleanly on the condvar
                    // instead of busy-spinning at 100% CPU.
                    if let Ok(mut g) = control.0.lock() {
                        g.capacity_paused = true;
                        g.playing = false;
                        control.1.notify_all();
                    }
                    let _ = publisher.offer(&world);
                    true
                }
            };

            let ctrl = world.controller_report();
            let delta_gpu_ns = ctrl.device_nanoseconds.saturating_sub(prev_device_ns);
            prev_device_ns = ctrl.device_nanoseconds;
            let inst_tps = if wall_ms > 0.001 {
                1000.0 / wall_ms
            } else {
                0.0
            };
            ema_tps = if ema_tps == 0.0 {
                inst_tps
            } else {
                ema_tps * 0.85 + inst_tps * 0.15
            };

            let (is_playing_now, cap_paused_now) = control
                .0
                .lock()
                .map(|mut g| {
                    g.worker_stepping = false;
                    control.1.notify_all();
                    (g.playing, g.capacity_paused || paused_now)
                })
                .unwrap_or((false, paused_now));

            if let Ok(mut tel) = telemetry.lock() {
                tel.sim_ticks_per_second = ema_tps;
                tel.last_tick_wall_ms = wall_ms;
                tel.last_gpu_ms = (delta_gpu_ns as f64) * 1.0e-6;
                tel.metal_batched_evals = ctrl.batched;
                tel.metal_deferred_evals = ctrl.deferred;
                tel.metal_resident_peak_bytes = ctrl.resident_peak_bytes;
                tel.metal_total_device_ns = ctrl.device_nanoseconds;
                tel.metal_active = ctrl.backend.is_some() && ctrl.failure.is_none();
                tel.is_playing = is_playing_now;
                tel.is_capacity_paused = cap_paused_now;
            }

            if target_tps > 0.0 {
                let target_dur = Duration::from_secs_f64(1.0 / target_tps);
                if let Some(rem) = target_dur.checked_sub(elapsed)
                    && rem >= Duration::from_micros(250)
                {
                    let (lock, cvar) = &*control;
                    if let Ok(guard) = lock.lock()
                        && !guard.shutdown
                        && guard.queries.is_empty()
                    {
                        let _ = cvar.wait_timeout(guard, rem);
                    }
                }
            }
        } else {
            let (is_playing_now, cap_paused_now) = control
                .0
                .lock()
                .map(|g| (g.playing, g.capacity_paused))
                .unwrap_or((false, false));
            if let Ok(mut tel) = telemetry.lock() {
                tel.is_playing = is_playing_now;
                tel.is_capacity_paused = cap_paused_now;
            }
        }
    }

    publisher.close();
    running.store(false, Ordering::Release);
}

/* =========================================================================
 * Exported C-ABI Entry Points
 * ========================================================================= */

#[unsafe(no_mangle)]
pub unsafe extern "C" fn flife_session_create(
    config_toml: *const c_char,
    metal_opt: *const FlifeMetalVtable,
    cpu_workers: u32,
    err_buf: *mut c_char,
    err_cap: usize,
) -> *mut FlifeSession {
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<Box<FlifeSession>, String> {
        let toml_text = unsafe { read_c_str(config_toml) }
            .ok_or_else(|| "config_toml pointer was null or invalid UTF-8".to_owned())?;
        let raw = RunConfig::from_toml(toml_text).map_err(|e| e.to_string())?;
        let validated = validate(raw).map_err(|e| e.to_string())?;

        let workers_usize = if cpu_workers == 0 {
            thread::available_parallelism()
                .map(|n| n.get().clamp(1, 6))
                .unwrap_or(4)
        } else {
            (cpu_workers as usize).clamp(1, 16)
        };
        let workers = NonZeroUsize::new(workers_usize).unwrap_or(NonZeroUsize::MIN);

        let mut world = World::found_on(
            &validated,
            Arrangement::Compact,
            NumericalProfile::reference_f32().map_err(|e| e.to_string())?,
            workers,
        )
        .map_err(|e| e.to_string())?;

        let _ = world.trace_ancestry();

        let mut metal_attached = false;
        if !metal_opt.is_null() {
            let vtable = unsafe { *metal_opt };
            if let Ok(backend) = IosMetalControllerBackend::new(vtable, workers.get()) {
                world.set_controller_backend(Some(Box::new(backend)));
                metal_attached = true;
            }
        }

        let cfg = validated.config();
        let dt_seconds =
            f64::from(cfg.world.tick_numerator) / f64::from(cfg.world.tick_denominator);
        let world_width = cfg.world.width;
        let world_height = cfg.world.height;
        let grid_x = cfg.world.grid_width;
        let grid_y = cfg.world.grid_height;

        let control = Arc::new((
            Mutex::new(SharedControl {
                playing: true,
                capacity_paused: false,
                worker_stepping: false,
                step_requests: 0,
                target_tps: 0.0,
                shutdown: false,
                pending_interventions: Vec::new(),
                watch_update: None,
                follow_update: None,
                queries: Vec::new(),
            }),
            Condvar::new(),
        ));
        let control_wake = Arc::clone(&control);
        let publisher = Publisher::new(&world, 4, move || {
            control_wake.1.notify_all();
        })
        .map_err(|e| e.to_string())?;
        let feed = publisher.feed();
        let viewer = feed.viewer(DISPLAY_PAIR).map_err(|e| e.to_string())?;

        let telemetry = Arc::new(Mutex::new(LiveTelemetry {
            metal_active: metal_attached,
            is_playing: true,
            ..LiveTelemetry::default()
        }));
        let running = Arc::new(AtomicBool::new(true));

        let c_clone = Arc::clone(&control);
        let t_clone = Arc::clone(&telemetry);
        let r_clone = Arc::clone(&running);
        let worker = thread::Builder::new()
            .name("flife-sim-worker".to_owned())
            .stack_size(8 * 1024 * 1024)
            .spawn(move || worker_loop(world, publisher, c_clone, t_clone, r_clone))
            .map_err(|e| format!("failed to spawn simulation thread: {e}"))?;

        Ok(Box::new(FlifeSession {
            control,
            telemetry,
            viewer: Mutex::new(viewer),
            feed,
            dt_seconds,
            world_width,
            world_height,
            grid_x,
            grid_y,
            worker: Some(worker),
            running,
            epoch_counter: AtomicU64::new(0),
        }))
    }));

    match result {
        Ok(Ok(session)) => Box::into_raw(session),
        Ok(Err(msg)) => {
            write_c_str_buf(err_buf, err_cap, &msg);
            ptr::null_mut()
        }
        Err(_) => {
            write_c_str_buf(err_buf, err_cap, "panic inside flife_session_create");
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn flife_session_destroy(session: *mut FlifeSession) {
    if session.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let mut boxed = unsafe { Box::from_raw(session) };
        {
            let (lock, cvar) = &*boxed.control;
            if let Ok(mut g) = lock.lock() {
                g.shutdown = true;
                g.playing = false;
                cvar.notify_all();
            }
        }
        if let Some(handle) = boxed.worker.take() {
            let _ = handle.join();
        }
    }));
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn flife_session_set_playing(session: *mut FlifeSession, playing: bool) {
    if session.is_null() {
        return;
    }
    let s = unsafe { &*session };
    let (lock, cvar) = &*s.control;
    if let Ok(mut g) = lock.lock() {
        g.playing = playing;
        if playing {
            g.capacity_paused = false;
        }
        cvar.notify_all();
    }
    if let Ok(mut t) = s.telemetry.lock() {
        t.is_playing = playing;
        if playing {
            t.is_capacity_paused = false;
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn flife_session_pause_and_quiesce(
    session: *mut FlifeSession,
    timeout_ms: u32,
) -> bool {
    if session.is_null() {
        return true;
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let s = unsafe { &*session };
        let (lock, cvar) = &*s.control;
        let mut guard = match lock.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        guard.playing = false;
        guard.step_requests = 0;
        cvar.notify_all();
        if let Ok(mut t) = s.telemetry.lock() {
            t.is_playing = false;
        }
        let deadline = Instant::now() + Duration::from_millis(u64::from(timeout_ms));
        while guard.worker_stepping && !guard.shutdown && s.running.load(Ordering::Acquire) {
            let Some(rem) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            guard = match cvar.wait_timeout(guard, rem) {
                Ok((g, _)) => g,
                Err(p) => p.into_inner().0,
            };
        }
        !guard.worker_stepping
    }));
    result.unwrap_or(false)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn flife_session_step_once(session: *mut FlifeSession) {
    if session.is_null() {
        return;
    }
    let s = unsafe { &*session };
    let (lock, cvar) = &*s.control;
    if let Ok(mut g) = lock.lock() {
        g.playing = false;
        g.capacity_paused = false;
        g.step_requests = g.step_requests.saturating_add(1);
        cvar.notify_all();
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn flife_session_set_target_tps(session: *mut FlifeSession, target_tps: f64) {
    if session.is_null() {
        return;
    }
    let s = unsafe { &*session };
    let (lock, cvar) = &*s.control;
    if let Ok(mut g) = lock.lock() {
        g.target_tps = if target_tps.is_finite() && target_tps > 0.0 {
            target_tps
        } else {
            0.0
        };
        cvar.notify_all();
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn flife_session_acquire_latest_epoch(
    session: *mut FlifeSession,
    last_seen_epoch_id: u64,
    out_view: *mut FlifeEpochView,
) -> *mut FlifeEpochHandle {
    if session.is_null() || out_view.is_null() {
        return ptr::null_mut();
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let s = unsafe { &*session };
        let viewer = match s.viewer.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let maybe_lease = if last_seen_epoch_id == 0 {
            viewer.latest().ok()
        } else {
            viewer
                .latest_after(RenderEpoch::new(last_seen_epoch_id))
                .ok()
                .flatten()
        };
        let Some(lease) = maybe_lease else {
            return ptr::null_mut();
        };

        let tel = s.telemetry.lock().map(|g| *g).unwrap_or_default();
        let metrics = lease.metrics();
        let field = lease.field();
        let [tex_x, tex_y] = field.texels();
        let [cells_x, cells_y] = field.cells();
        let [ext_x, ext_y] = lease.extent();
        let org_count = lease.len() as u32;

        let (has_norm, norm_val) = match metrics.ledger.normalized {
            Some(v) => (true, v),
            None => (false, f64::NAN),
        };
        let (has_tally, t_births, t_intros, t_deaths, t_evals, t_rej, t_ref, t_uptake) =
            match metrics.tally {
                Some(t) => (
                    true,
                    t.births,
                    t.introductions,
                    t.deaths,
                    t.evaluations,
                    t.rejected_births,
                    t.refused_births,
                    t.uptake,
                ),
                None => (false, 0, 0, 0, 0, 0, 0, 0.0),
            };
        let (has_fol, f_orig, f_org, f_hand, f_alive) = match lease.followed() {
            Some(f) => (true, f.origin.get(), f.organism.get(), f.handoffs, f.alive),
            None => (false, 0, 0, 0, false),
        };

        s.epoch_counter.fetch_add(1, Ordering::Relaxed);

        let view = FlifeEpochView {
            epoch_id: lease.id().get(),
            tick: lease.tick().get(),
            resource_epoch: lease.resource().get(),
            dt_seconds: s.dt_seconds,
            world_width: ext_x,
            world_height: ext_y,
            maximum_speed: lease.maximum_speed(),
            attempt_energy: lease.attempt_energy(),
            affinity_limit: lease.affinity_limit(),
            population_capacity: lease.capacity() as u32,

            organism_count: org_count,
            ids: lease.ids().as_ptr(),
            positions: lease.positions().as_ptr().cast::<f32>(),
            velocities: lease.velocities().as_ptr().cast::<f32>(),
            energies: lease.energies().as_ptr(),
            lineages: lease.lineages().as_ptr(),
            generations: lease.generations().as_ptr(),
            topologies: lease.topologies().as_ptr(),
            affinities: lease.affinities().as_ptr().cast::<f32>(),
            cell_states: lease.cell_states().map_or(ptr::null(), |sl| sl.as_ptr()),
            polarities: lease.polarities().map_or(ptr::null(), |sl| sl.as_ptr()),

            field_cells_x: cells_x,
            field_cells_y: cells_y,
            field_texels_x: tex_x,
            field_texels_y: tex_y,
            field_energy: field.values().as_ptr(),
            field_composition: field.composition().as_ptr().cast::<f32>(),
            field_components: field.components().as_ptr().cast::<i8>(),
            field_trail_energy: field.trail_energy().map_or(ptr::null(), |sl| sl.as_ptr()),
            field_vascular: field
                .vascular_conductance()
                .map_or(ptr::null(), |sl| sl.as_ptr()),

            live_count: metrics.live,
            organism_energy_total: metrics.organism_energy,
            field_energy_total: metrics.field_energy,
            max_generation: metrics.max_generation,
            total_events: metrics.events,
            total_parameters: metrics.parameters,
            genome_logical_bytes: metrics.genome_logical_bytes,
            genome_allocated_bytes: metrics.genome_allocated_bytes,

            ledger_initial: metrics.ledger.initial,
            ledger_current: metrics.ledger.current,
            ledger_injected: metrics.ledger.injected,
            ledger_removed: metrics.ledger.removed,
            ledger_dissipated: metrics.ledger.dissipated,
            ledger_turnover: metrics.ledger.turnover,
            ledger_residual: metrics.ledger.residual,
            ledger_normalized: norm_val,
            has_ledger_normalized: has_norm,
            faulted: metrics.faulted,

            has_tally,
            tally_births: t_births,
            tally_introductions: t_intros,
            tally_deaths: t_deaths,
            tally_evaluations: t_evals,
            tally_rejected_births: t_rej,
            tally_refused_births: t_ref,
            tally_uptake: t_uptake,

            has_followed: has_fol,
            followed_origin_id: f_orig,
            followed_organism_id: f_org,
            followed_handoffs: f_hand,
            followed_alive: f_alive,

            sim_ticks_per_second: tel.sim_ticks_per_second,
            last_tick_wall_ms: tel.last_tick_wall_ms,
            last_gpu_ms: tel.last_gpu_ms,
            metal_batched_evals: tel.metal_batched_evals,
            metal_deferred_evals: tel.metal_deferred_evals,
            metal_resident_peak_bytes: tel.metal_resident_peak_bytes,
            metal_total_device_ns: tel.metal_total_device_ns,
            metal_active: tel.metal_active,
            is_playing: tel.is_playing,
            is_capacity_paused: tel.is_capacity_paused,
        };
        unsafe { *out_view = view };
        Box::into_raw(Box::new(FlifeEpochHandle { _lease: lease }))
    }));
    result.unwrap_or(ptr::null_mut())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn flife_epoch_release(handle: *mut FlifeEpochHandle) {
    if handle.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        drop(unsafe { Box::from_raw(handle) });
    }));
}

fn push_intervention(session: &FlifeSession, intervention: Intervention) -> bool {
    let (lock, cvar) = &*session.control;
    let Ok(mut g) = lock.lock() else {
        return false;
    };
    if g.pending_interventions.len() >= 128 {
        return false;
    }
    g.pending_interventions.push(intervention);
    cvar.notify_all();
    true
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn flife_session_inject_energy(
    session: *mut FlifeSession,
    cell_x: u32,
    cell_y: u32,
    amount: f64,
) -> bool {
    if session.is_null() || !amount.is_finite() || amount <= 0.0 || amount > f64::from(f32::MAX) {
        return false;
    }
    let s = unsafe { &*session };
    if cell_x >= s.grid_x || cell_y >= s.grid_y {
        return false;
    }
    push_intervention(
        s,
        Intervention::InjectEnergy {
            cell: [cell_x, cell_y],
            amount,
        },
    )
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn flife_session_inject_material(
    session: *mut FlifeSession,
    cell_x: u32,
    cell_y: u32,
    amount: f64,
    direction32: *const f64,
) -> bool {
    if session.is_null()
        || direction32.is_null()
        || !amount.is_finite()
        || amount <= 0.0
        || amount > f64::from(f32::MAX)
    {
        return false;
    }
    let s = unsafe { &*session };
    if cell_x >= s.grid_x || cell_y >= s.grid_y {
        return false;
    }
    let mut dir = [0.0_f64; 32];
    let slice = unsafe { std::slice::from_raw_parts(direction32, 32) };
    let mut norm_sq = 0.0_f64;
    for (dst, &v) in dir.iter_mut().zip(slice) {
        if !v.is_finite() {
            return false;
        }
        *dst = v;
        norm_sq += v * v;
    }
    if norm_sq > 1.0 {
        let inv = 0.999_999 / norm_sq.sqrt();
        for v in &mut dir {
            *v *= inv;
        }
    }
    push_intervention(
        s,
        Intervention::InjectMaterial {
            cell: [cell_x, cell_y],
            amount,
            direction: dir,
        },
    )
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn flife_session_introduce_organisms(
    session: *mut FlifeSession,
    cell_x: u32,
    cell_y: u32,
    count: u32,
    energy: f64,
) -> bool {
    if session.is_null()
        || count == 0
        || count > 4096
        || !energy.is_finite()
        || energy <= 0.0
        || energy > f64::from(f32::MAX)
    {
        return false;
    }
    let s = unsafe { &*session };
    if cell_x >= s.grid_x || cell_y >= s.grid_y {
        return false;
    }
    push_intervention(
        s,
        Intervention::IntroduceOrganisms {
            cell: [cell_x, cell_y],
            count,
            energy,
        },
    )
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn flife_session_remove_organisms(
    session: *mut FlifeSession,
    center_x: f64,
    center_y: f64,
    radius: f64,
    disposition: u32,
) -> bool {
    if session.is_null() || !center_x.is_finite() || !center_y.is_finite() || !radius.is_finite() {
        return false;
    }
    let s = unsafe { &*session };
    let cx = center_x.rem_euclid(s.world_width);
    let cy = center_y.rem_euclid(s.world_height);
    let limit = 0.5 * s.world_width.min(s.world_height);
    let r = radius.clamp(1e-3, limit * 0.999);
    let disp = match disposition {
        0 => Disposition::Remove,
        1 => Disposition::Dissipate,
        _ => Disposition::Recycle,
    };
    push_intervention(
        s,
        Intervention::RemoveOrganisms {
            center: [cx, cy],
            radius: r,
            disposition: disp,
        },
    )
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn flife_session_inspect_organism(
    session: *mut FlifeSession,
    organism_id: u64,
    out_info: *mut FlifeOrganismInfo,
) -> bool {
    if session.is_null() || out_info.is_null() {
        return false;
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let s = unsafe { &*session };
        if !s.running.load(Ordering::Acquire) {
            return false;
        }
        let reply = Arc::new((Mutex::new(None), Condvar::new()));
        {
            let (lock, cvar) = &*s.control;
            let Ok(mut g) = lock.lock() else {
                return false;
            };
            g.queries.push(SyncQuery::Inspect(
                OrganismId::new(organism_id),
                Arc::clone(&reply),
            ));
            cvar.notify_all();
        }
        let (rlock, rcvar) = &*reply;
        let Ok(mut guard) = rlock.lock() else {
            return false;
        };
        let deadline = Instant::now() + Duration::from_millis(500);
        while guard.is_none() {
            let Some(rem) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            guard = match rcvar.wait_timeout(guard, rem) {
                Ok((g, _)) => g,
                Err(_) => return false,
            };
        }
        let out = unsafe { &mut *out_info };
        match guard.take().flatten() {
            Some(snap) => {
                fill_organism_info(&snap, out);
                true
            }
            None => {
                *out = FlifeOrganismInfo::default();
                false
            }
        }
    }));
    result.unwrap_or(false)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn flife_session_set_watch(
    session: *mut FlifeSession,
    enabled: bool,
    organism_id: u64,
) {
    if session.is_null() {
        return;
    }
    let s = unsafe { &*session };
    let (lock, cvar) = &*s.control;
    if let Ok(mut g) = lock.lock() {
        g.watch_update = Some(enabled.then(|| OrganismId::new(organism_id)));
        cvar.notify_all();
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn flife_session_set_follow(
    session: *mut FlifeSession,
    enabled: bool,
    organism_id: u64,
) {
    if session.is_null() {
        return;
    }
    let s = unsafe { &*session };
    let (lock, cvar) = &*s.control;
    if let Ok(mut g) = lock.lock() {
        g.follow_update = Some(enabled.then(|| OrganismId::new(organism_id)));
        cvar.notify_all();
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn flife_session_measure_region(
    session: *mut FlifeSession,
    center_x: f64,
    center_y: f64,
    radius: f64,
    out_measure: *mut FlifeRegionMeasure,
) -> bool {
    if session.is_null()
        || out_measure.is_null()
        || !center_x.is_finite()
        || !center_y.is_finite()
        || !radius.is_finite()
    {
        return false;
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let s = unsafe { &*session };
        if !s.running.load(Ordering::Acquire) {
            return false;
        }
        let cx = center_x.rem_euclid(s.world_width);
        let cy = center_y.rem_euclid(s.world_height);
        let limit = 0.5 * s.world_width.min(s.world_height);
        let r = radius.clamp(1e-3, limit * 0.999);
        let reply = Arc::new((Mutex::new(None), Condvar::new()));
        {
            let (lock, cvar) = &*s.control;
            let Ok(mut g) = lock.lock() else {
                return false;
            };
            g.queries
                .push(SyncQuery::Measure([cx, cy], r, Arc::clone(&reply)));
            cvar.notify_all();
        }
        let (rlock, rcvar) = &*reply;
        let Ok(mut guard) = rlock.lock() else {
            return false;
        };
        let deadline = Instant::now() + Duration::from_millis(500);
        while guard.is_none() {
            let Some(rem) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            guard = match rcvar.wait_timeout(guard, rem) {
                Ok((g, _)) => g,
                Err(_) => return false,
            };
        }
        let out = unsafe { &mut *out_measure };
        match guard.take() {
            Some(Ok(m)) => {
                *out = FlifeRegionMeasure {
                    valid: true,
                    tick: m.tick.get(),
                    center_x: m.center[0],
                    center_y: m.center[1],
                    radius: m.radius,
                    organisms: m.organisms,
                    organism_energy: m.organism_energy,
                    lineages: m.lineages,
                    cells: m.cells,
                    field_energy: m.field_energy,
                };
                true
            }
            _ => {
                *out = FlifeRegionMeasure::default();
                false
            }
        }
    }));
    result.unwrap_or(false)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn flife_inspect_preset(
    config_toml: *const c_char,
    out_summary: *mut FlifePresetSummary,
) {
    if out_summary.is_null() {
        return;
    }
    let out = unsafe { &mut *out_summary };
    *out = FlifePresetSummary {
        valid: false,
        name: [0; 68],
        model: [0; 68],
        color_mapping: [0; 68],
        weight_dtype: [0; 16],
        world_width: 0.0,
        world_height: 0.0,
        grid_width: 0,
        grid_height: 0,
        initial_count: 0,
        maximum_count: 0,
        tick_numerator: 1,
        tick_denominator: 32,
        max_logical_genome_bytes: 0,
        max_total_bytes: 0,
        v2_feature_mask: 0,
        has_contact: false,
        has_v2_visuals: false,
        has_trail_carrier: false,
        has_vascular_field: false,
        has_cell_state: false,
        has_polarity: false,
        error: [0; 256],
    };
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let Some(text) = (unsafe { read_c_str(config_toml) }) else {
            write_c_array(&mut out.error, "null or invalid UTF-8 TOML");
            return;
        };
        let raw = match RunConfig::from_toml(text) {
            Ok(r) => r,
            Err(e) => {
                write_c_array(&mut out.error, &e.to_string());
                return;
            }
        };
        let validated = match validate(raw) {
            Ok(v) => v,
            Err(e) => {
                write_c_array(&mut out.error, &e.to_string());
                return;
            }
        };
        let cfg = validated.config();
        out.valid = true;
        write_c_array(&mut out.name, &cfg.name);
        write_c_array(&mut out.model, &cfg.model);
        write_c_array(&mut out.color_mapping, &cfg.presentation.color_mapping);
        write_c_array(
            &mut out.weight_dtype,
            &cfg.controller.canonical_weight_dtype,
        );
        out.world_width = cfg.world.width;
        out.world_height = cfg.world.height;
        out.grid_width = cfg.world.grid_width;
        out.grid_height = cfg.world.grid_height;
        out.initial_count = cfg.population.initial_count;
        out.maximum_count = cfg.population.maximum_count;
        out.tick_numerator = cfg.world.tick_numerator;
        out.tick_denominator = cfg.world.tick_denominator;
        out.max_logical_genome_bytes = cfg.execution.maximum_logical_genome_bytes;
        out.max_total_bytes = cfg.execution.maximum_total_bytes;
        out.v2_feature_mask = cfg.v2_feature_bitmask();
        out.has_contact = cfg.contact.is_some();
        let v2_vis = cfg.is_v2_feature_enabled(V2Feature::V2VisualMappings);
        out.has_v2_visuals = v2_vis;
        out.has_trail_carrier = v2_vis && cfg.is_v2_feature_enabled(V2Feature::DualCarrierTrails);
        out.has_vascular_field = v2_vis && cfg.is_v2_feature_enabled(V2Feature::VascularField);
        out.has_cell_state = v2_vis
            && (cfg.is_v2_feature_enabled(V2Feature::EpigeneticRecurrence)
                || cfg.v2_abi().recurrent_state_width > 0);
        out.has_polarity = v2_vis && cfg.is_v2_feature_enabled(V2Feature::PcpPolarity);
    }));
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn flife_verify_metal_parity(
    config_toml: *const c_char,
    metal: *const FlifeMetalVtable,
    ticks: u32,
    cpu_workers: u32,
    out_result: *mut FlifeParityResult,
) {
    if out_result.is_null() {
        return;
    }
    let out = unsafe { &mut *out_result };
    *out = FlifeParityResult {
        success: false,
        digest_matched: false,
        ticks_verified: 0,
        mismatch_tick: 0,
        live_final: 0,
        metal_batched: 0,
        metal_deferred: 0,
        metal_device_ns: 0,
        metal_wall_ns: 0,
        cpu_wall_ns: 0,
        ledger_residual: 0.0,
        metal_digest_hex: [0; 65],
        cpu_digest_hex: [0; 65],
        error: [0; 256],
    };

    let res = catch_unwind(AssertUnwindSafe(|| -> Result<(), String> {
        let toml_text = unsafe { read_c_str(config_toml) }
            .ok_or_else(|| "null or invalid UTF-8 TOML".to_owned())?;
        if metal.is_null() {
            return Err("metal vtable was null".to_owned());
        }
        let vtable = unsafe { *metal };
        let raw = RunConfig::from_toml(toml_text).map_err(|e| e.to_string())?;
        let validated = validate(raw).map_err(|e| e.to_string())?;
        let workers =
            NonZeroUsize::new((cpu_workers as usize).clamp(1, 8)).unwrap_or(NonZeroUsize::MIN);
        let profile = NumericalProfile::reference_f32().map_err(|e| e.to_string())?;

        let mut world_metal = World::found_on(&validated, Arrangement::Compact, profile, workers)
            .map_err(|e| e.to_string())?;
        let backend =
            IosMetalControllerBackend::new(vtable, workers.get()).map_err(|e| e.to_string())?;
        world_metal.set_controller_backend(Some(Box::new(backend)));

        let mut world_cpu = World::found_on(&validated, Arrangement::Compact, profile, workers)
            .map_err(|e| e.to_string())?;

        let mut metal_wall_ns = 0_u64;
        let mut cpu_wall_ns = 0_u64;

        for t in 1..=ticks {
            let tm0 = Instant::now();
            world_metal
                .step()
                .map_err(|e| format!("metal tick {t}: {e}"))?;
            metal_wall_ns = metal_wall_ns.saturating_add(tm0.elapsed().as_nanos() as u64);

            let tc0 = Instant::now();
            world_cpu.step().map_err(|e| format!("cpu tick {t}: {e}"))?;
            cpu_wall_ns = cpu_wall_ns.saturating_add(tc0.elapsed().as_nanos() as u64);

            let dm = world_metal.digest();
            let dc = world_cpu.digest();
            if dm != dc {
                write_c_array(&mut out.metal_digest_hex, &format!("{dm}"));
                write_c_array(&mut out.cpu_digest_hex, &format!("{dc}"));
                out.success = true;
                out.digest_matched = false;
                out.mismatch_tick = u64::from(t);
                out.ticks_verified = u64::from(t - 1);
                return Ok(());
            }
            out.ticks_verified = u64::from(t);
        }

        let dm = world_metal.digest();
        let dc = world_cpu.digest();
        write_c_array(&mut out.metal_digest_hex, &format!("{dm}"));
        write_c_array(&mut out.cpu_digest_hex, &format!("{dc}"));

        let cr = world_metal.controller_report();
        if let Some(fail) = &cr.failure {
            return Err(format!("metal controller backend failed: {fail}"));
        }

        out.success = true;
        out.digest_matched = dm == dc;
        out.live_final = world_metal.population().len() as u64;
        out.metal_batched = cr.batched;
        out.metal_deferred = cr.deferred;
        out.metal_device_ns = cr.device_nanoseconds;
        out.metal_wall_ns = metal_wall_ns;
        out.cpu_wall_ns = cpu_wall_ns;
        out.ledger_residual = world_metal.ledger().report().residual;
        Ok(())
    }));

    match res {
        Ok(Ok(())) => {}
        Ok(Err(msg)) => write_c_array(&mut out.error, &msg),
        Err(_) => write_c_array(&mut out.error, "panic in flife_verify_metal_parity"),
    }
}
