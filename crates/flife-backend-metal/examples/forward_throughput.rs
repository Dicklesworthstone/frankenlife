//! Controller throughput: one fused Metal dispatch against the reference
//! controller on one thread and on every core, for the same batch, with the
//! outputs compared bit for bit.
//!
//! `cargo run --release -p flife-backend-metal --example forward_throughput -- [organisms] [hidden widths]`,
//! for example `-- 5000 64,32`. Prints one JSON line.

use std::time::Instant;

use flife_backend_metal::{Batch, MetalControllers};
use flife_core::CoreError;
use flife_neural::{
    ACTION_WIDTH, ControllerScratch, DenseTopology, ReferenceController, SENSOR_WIDTH,
};

const ROUNDS: usize = 15;

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

fn median(mut samples: Vec<f64>) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[samples.len() / 2]
}

fn main() -> Result<(), CoreError> {
    let mut args = std::env::args().skip(1);
    let organisms: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(5_000);
    let widths: Vec<u32> = args
        .next()
        .unwrap_or_else(|| "64,32".to_owned())
        .split(',')
        .filter_map(|w| w.parse().ok())
        .collect();
    let topology = DenseTopology::new(&widths)?;
    let mut draws = Draws(5);
    let genomes: Vec<Vec<f32>> = (0..organisms)
        .map(|_| {
            let mut values = vec![0.0_f32; topology.parameter_count()];
            for layer in topology.layers() {
                let scale = 2.0 / (layer.inputs as f64).sqrt();
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

    let controllers = MetalControllers::open()?;
    let mut batch = Batch::new();
    let mut outputs = Vec::new();
    let mut metal = Vec::new();
    let mut packing = Vec::new();
    let mut uploading = Vec::new();
    for _ in 0..ROUNDS {
        let start = Instant::now();
        batch.clear();
        for (genome, inputs) in genomes.iter().zip(&observations) {
            batch.push(&topology, genome, inputs)?;
        }
        packing.push(start.elapsed().as_secs_f64());
        let start = Instant::now();
        let resident = controllers.upload(&batch)?;
        uploading.push(start.elapsed().as_secs_f64());
        let start = Instant::now();
        controllers.run(&resident, &mut outputs)?;
        metal.push(start.elapsed().as_secs_f64());
    }

    let reference = |range: std::ops::Range<usize>| -> Result<Vec<[f32; ACTION_WIDTH]>, CoreError> {
        let mut scratch = ControllerScratch::new(&topology)?;
        range
            .map(|k| {
                ReferenceController::new(&topology, &genomes[k], 1.0e6)?
                    .evaluate(&observations[k], &mut scratch)
            })
            .collect()
    };
    let mut single = Vec::new();
    let mut expected = Vec::new();
    for _ in 0..3 {
        let start = Instant::now();
        expected = reference(0..organisms)?;
        single.push(start.elapsed().as_secs_f64());
    }
    let threads = std::thread::available_parallelism().map_or(8, usize::from);
    let mut parallel = Vec::new();
    for _ in 0..ROUNDS {
        let start = Instant::now();
        let length = organisms.div_ceil(threads);
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..threads)
                .map(|t| {
                    let range = (t * length).min(organisms)..((t + 1) * length).min(organisms);
                    scope.spawn(move || reference(range))
                })
                .collect();
            for handle in handles {
                let _ = handle.join();
            }
        });
        parallel.push(start.elapsed().as_secs_f64());
    }

    let identical = outputs.len() == organisms
        && outputs.iter().zip(&expected).all(|(got, want)| {
            got.is_some_and(|got| {
                got.iter()
                    .zip(want)
                    .all(|(a, b)| a.to_bits() == b.to_bits())
            })
        });
    let deferred = outputs.iter().filter(|o| o.is_none()).count();
    println!(
        "{{\"device\":{:?},\"organisms\":{organisms},\"hidden\":{widths:?},\"parameters\":{},\"identical\":{identical},\"deferred\":{deferred},\"metal_ms\":{:.3},\"upload_ms\":{:.3},\"pack_ms\":{:.3},\"cpu_1_ms\":{:.3},\"cpu_threads\":{threads},\"cpu_all_ms\":{:.3}}}",
        controllers.device_name(),
        topology.parameter_count(),
        median(metal) * 1e3,
        median(uploading) * 1e3,
        median(packing) * 1e3,
        median(single) * 1e3,
        median(parallel) * 1e3,
    );
    Ok(())
}
