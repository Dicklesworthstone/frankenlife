//! FL-030's resident-bandwidth benchmark (plan §19.2). The engine's Metal
//! controller path evaluates `N` distinct genomes resident in the backend's
//! per-topology shards (FL-026), every output compared with the reference
//! bit for bit. The device time of each warm batch is turned into the weight
//! bytes it streamed per second, the "sustained useful bandwidth with the
//! actual independent-weight layout" §19.2 asks for. Beside it, a plain
//! streaming read of as many bytes on the same device gives the ceiling that
//! layout is measured against. The two alternate, round by round, so that
//! both see the same contention; each reports its median and its fastest
//! round.
//!
//! `cargo run --release -p flife-backend-metal --example resident_bandwidth -- [organisms,...] [hidden widths] [rounds]`,
//! for example `-- 5000,20000,80000 64,32 21`. Prints one JSON line per
//! population. `plan_sweeps_per_s` is the measured weight bandwidth over
//! §19.2's bank of four billion F32 parameters (16 GB a sweep).

use std::time::Instant;

use flife_backend_metal::{Binding, DeviceOwner, Limits, MetalBackend, Owned};
use flife_core::CoreError;
use flife_core::ids::GenomeId;
use flife_neural::{
    ACTION_WIDTH, ArenaSlot, ControllerBackend, ControllerJob, ControllerScratch, DenseTopology,
    ReferenceController, SENSOR_WIDTH,
};
use ft_kernel_metal::compute::{Grid, MathMode, Pipeline};

/// Fan-in-scaled parameters admit far below this.
const BOUND: f64 = 4.0;

/// §19.2's bank: 400,000 genomes of 10,000 parameters, as F32 bytes.
const PLAN_BANK_BYTES: f64 = 4.0e9 * 4.0;

/// Each thread sums a grid-stride share of the buffer, so every byte is read
/// once, and writes one partial sum so that no read can be dropped.
const STREAM_MSL: &str = r"
#include <metal_stdlib>
using namespace metal;
kernel void stream_read(
    device const float4 *values [[buffer(0)]],
    device const uint *count [[buffer(1)]],
    device float *sums [[buffer(2)]],
    uint id [[thread_position_in_grid]],
    uint threads [[threads_per_grid]])
{
    float4 total = float4(0.0f);
    uint n = count[0];
    for (uint i = id; i < n; i += threads) {
        total += values[i];
    }
    sums[id] = total.x + total.y + total.z + total.w;
}
";
const STREAM_GROUPS: usize = 8_192;
const STREAM_THREADS: usize = 256;

struct Draws(u64);

impl Draws {
    fn symmetric(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) >> 11) as f64 / (1_u64 << 52) as f64 - 1.0
    }
}

fn refused(detail: String) -> CoreError {
    CoreError::StateConflict {
        operation: "resident_bandwidth",
        detail,
    }
}

fn median(samples: &[f64]) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[sorted.len() / 2]
}

fn cv_percent(samples: &[f64]) -> f64 {
    let n = samples.len() as f64;
    let mean = samples.iter().sum::<f64>() / n;
    let variance = samples.iter().map(|s| (s - mean).powi(2)).sum::<f64>() / n;
    100.0 * variance.sqrt() / mean
}

/// The reference's outputs for every genome, on every core.
fn reference(
    topology: &DenseTopology,
    genomes: &[Vec<f32>],
    observations: &[[f64; SENSOR_WIDTH]],
) -> Result<Vec<Option<[f32; ACTION_WIDTH]>>, CoreError> {
    let threads = std::thread::available_parallelism().map_or(8, usize::from);
    let length = genomes.len().div_ceil(threads).max(1);
    let parts: Vec<Result<Vec<Option<[f32; ACTION_WIDTH]>>, CoreError>> =
        std::thread::scope(|scope| {
            let handles: Vec<_> = genomes
                .chunks(length)
                .zip(observations.chunks(length))
                .map(|(genomes, observations)| {
                    scope.spawn(move || {
                        let mut scratch = ControllerScratch::new(topology)?;
                        genomes
                            .iter()
                            .zip(observations)
                            .map(|(genome, inputs)| {
                                Ok(ReferenceController::new(topology, genome, BOUND)?
                                    .evaluate(inputs, &mut scratch)
                                    .ok())
                            })
                            .collect()
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| {
                    handle
                        .join()
                        .unwrap_or_else(|_| Err(refused("a reference thread panicked".to_owned())))
                })
                .collect()
        });
    let mut all = Vec::with_capacity(genomes.len());
    for part in parts {
        all.extend(part?);
    }
    Ok(all)
}

/// A plain streaming read of a copy of the genomes' bytes, on its own owner.
struct Stream {
    /// Its buffers: the values, their float4 count, one sum per thread.
    buffers: (Owned<f32>, Owned<u32>, Owned<f32>),
    bindings: [Binding; 3],
    pipeline: Pipeline,
    owner: DeviceOwner,
    bytes: f64,
}

impl Stream {
    fn new(values: &[f32]) -> Result<Self, CoreError> {
        let owner = DeviceOwner::open(Limits::UNBOUNDED)?;
        let library = owner.library(STREAM_MSL, MathMode::Safe)?;
        let pipeline = library
            .pipeline("stream_read")
            .map_err(|error| refused(format!("{error:?}")))?;
        let quads = u32::try_from(values.len() / 4)
            .map_err(|_| refused("the stream buffer is past 2^32 float4s".to_owned()))?;
        let buffers = (
            owner.upload_owned(values)?,
            owner.upload_owned(&[quads])?,
            owner.zeroed_owned::<f32>(STREAM_GROUPS * STREAM_THREADS)?,
        );
        let bindings = [
            buffers.0.buffer().reads(),
            buffers.1.buffer().reads(),
            buffers.2.buffer().writes(),
        ];
        Ok(Self {
            buffers,
            bindings,
            pipeline,
            owner,
            bytes: f64::from(quads) * 16.0,
        })
    }

    /// One streaming read; its device seconds.
    fn run(&self) -> Result<f64, CoreError> {
        let grid = Grid::linear(STREAM_GROUPS, STREAM_THREADS);
        Ok(self
            .owner
            .submit(&self.pipeline, &self.bindings, grid)?
            .device_seconds)
    }
}

/// Whether every dispatched output is the reference's, bit for bit.
fn identical(
    outputs: &[Option<[f32; ACTION_WIDTH]>],
    expected: &[Option<[f32; ACTION_WIDTH]>],
) -> bool {
    outputs.len() == expected.len()
        && outputs
            .iter()
            .zip(expected)
            .all(|(got, want)| match (got, want) {
                (Some(got), Some(want)) => got
                    .iter()
                    .zip(want)
                    .all(|(a, b)| a.to_bits() == b.to_bits()),
                (None, _) => true,
                (Some(_), None) => false,
            })
}

fn fastest(samples: &[f64]) -> f64 {
    samples.iter().copied().fold(f64::INFINITY, f64::min)
}

fn main() -> Result<(), CoreError> {
    let mut args = std::env::args().skip(1);
    let populations: Vec<usize> = args
        .next()
        .unwrap_or_else(|| "5000,20000,80000".to_owned())
        .split(',')
        .filter_map(|n| n.parse().ok())
        .collect();
    let widths: Vec<u32> = args
        .next()
        .unwrap_or_else(|| "64,32".to_owned())
        .split(',')
        .filter_map(|w| w.parse().ok())
        .collect();
    let rounds: usize = args
        .next()
        .and_then(|r| r.parse().ok())
        .unwrap_or(21)
        .max(1);
    let topology = DenseTopology::new(&widths)?;
    let parameters = topology.parameter_count();
    for &organisms in &populations {
        let mut draws = Draws(organisms as u64);
        let genomes: Vec<Vec<f32>> = (0..organisms)
            .map(|_| {
                let mut values = vec![0.0_f32; parameters];
                for layer in topology.layers() {
                    let scale = 1.0 / (layer.inputs as f64).sqrt();
                    for value in &mut values[layer.weights..layer.biases + layer.outputs] {
                        *value = (draws.symmetric() * scale) as f32;
                    }
                }
                values
            })
            .collect();
        let observations: Vec<[f64; SENSOR_WIDTH]> = (0..organisms)
            .map(|_| std::array::from_fn(|_| draws.symmetric() * 2.0))
            .collect();
        let jobs = genomes
            .iter()
            .zip(&observations)
            .enumerate()
            .map(|(index, (genome, inputs))| {
                Ok(ControllerJob {
                    genome: GenomeId::new(index as u64 + 1),
                    slot: ArenaSlot::new(0, index as u64, 1),
                    controller: ReferenceController::new(&topology, genome, BOUND)?,
                    inputs,
                })
            })
            .collect::<Result<Vec<_>, CoreError>>()?;
        let expected = reference(&topology, &genomes, &observations)?;

        let mut backend = MetalBackend::open()?;
        let mut outputs = Vec::new();
        let start = Instant::now();
        backend.evaluate(&jobs, &mut outputs)?;
        let upload = start.elapsed().as_secs_f64();
        let mut same = identical(&outputs, &expected);
        let stream = {
            let flat: Vec<f32> = genomes.iter().flatten().copied().collect();
            Stream::new(&flat)?
        };
        stream.run()?;
        let mut wall = Vec::with_capacity(rounds);
        let mut device = Vec::with_capacity(rounds);
        let mut streamed = Vec::with_capacity(rounds);
        let mut kernels = 0;
        let mut deferred = 0;
        for _ in 0..rounds {
            let start = Instant::now();
            backend.evaluate(&jobs, &mut outputs)?;
            wall.push(start.elapsed().as_secs_f64());
            let (ran, seconds) = backend
                .last_command()
                .ok_or_else(|| refused("a batch dispatched nothing".to_owned()))?;
            kernels = ran;
            device.push(seconds);
            same &= identical(&outputs, &expected);
            deferred += outputs.iter().filter(|output| output.is_none()).count();
            streamed.push(stream.run()?);
        }
        let bytes = (organisms * parameters * size_of::<f32>()) as f64;
        let weight_rate = bytes / median(&device);
        let stream_rate = stream.bytes / median(&streamed);
        let weight_peak = bytes / fastest(&device);
        let stream_peak = stream.bytes / fastest(&streamed);
        println!(
            "{{\"device\":{:?},\"organisms\":{organisms},\"hidden\":{widths:?},\"parameters\":{parameters},\
             \"weight_bytes\":{bytes:.0},\"shards\":{},\"kernels\":{kernels},\"uploads\":{},\
             \"identical\":{same},\"deferred\":{deferred},\"rounds\":{rounds},\"upload_ms\":{:.3},\
             \"wall_ms\":{:.3},\"device_ms\":{:.3},\"device_cv_percent\":{:.2},\"device_fastest_ms\":{:.3},\
             \"weight_gb_per_s\":{:.1},\"weight_fastest_gb_per_s\":{:.1},\"wall_gb_per_s\":{:.1},\
             \"stream_device_ms\":{:.3},\"stream_cv_percent\":{:.2},\"stream_gb_per_s\":{:.1},\
             \"stream_fastest_gb_per_s\":{:.1},\"fraction_of_stream\":{:.3},\"fastest_fraction\":{:.3},\
             \"plan_sweeps_per_s\":{:.2}}}",
            stream.owner.device_name(),
            backend.shards(),
            backend.uploads(),
            upload * 1e3,
            median(&wall) * 1e3,
            median(&device) * 1e3,
            cv_percent(&device),
            fastest(&device) * 1e3,
            weight_rate / 1e9,
            weight_peak / 1e9,
            bytes / median(&wall) / 1e9,
            median(&streamed) * 1e3,
            cv_percent(&streamed),
            stream_rate / 1e9,
            stream_peak / 1e9,
            weight_rate / stream_rate,
            weight_peak / stream_peak,
            weight_rate / PLAN_BANK_BYTES,
        );
        drop(stream.buffers);
    }
    Ok(())
}
