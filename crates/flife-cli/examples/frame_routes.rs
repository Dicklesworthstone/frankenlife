//! Measure `flife render`'s two frame routes on the real preset: FrankenManim's
//! CPU 3D renderer (the current offline route) and its Metal 3D renderer with
//! an on-device RGBA8 transfer (the route the Studio presents). For one frame
//! after `ticks` ticks it prints, as one JSON line, both routes' warm
//! timings, the PNG encoding time, and how the two frames' bytes differ.
//!
//! `cargo run --release -p flife-cli --example frame_routes -- [ticks] [size]`

use std::time::Instant;

use flife_cli::found;
use flife_core::ValidatedConfig;
use flife_manim::{WorldView, hex_rgb};
use flife_render::Publisher;
use fmn_codec::CompressionLevel;
use fmn_render::metal::MetalRenderer;

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    values.get(values.len() / 2).copied().unwrap_or(f64::NAN)
}

#[expect(clippy::expect_used, reason = "a measurement tool")]
fn main() {
    let mut args = std::env::args().skip(1);
    let ticks: u64 = args.next().map_or(64, |t| t.parse().expect("ticks"));
    let size: u32 = args.next().map_or(1280, |s| s.parse().expect("size"));
    let config = ValidatedConfig::from_toml(PRESET).expect("preset");
    let background = hex_rgb(&config.config().presentation.background).expect("background");
    let mut world = found(&config).expect("world");
    for _ in 0..ticks {
        world.step().expect("step");
    }
    let mut publisher = Publisher::new(&world, 3, || {}).expect("publisher");
    let viewer = publisher.feed().viewer(1).expect("viewer");
    publisher.offer(&world).expect("offer");
    let lease = viewer.latest().expect("lease");
    let mut view = WorldView::for_epoch(&lease, [size, size], background).expect("view");
    view.show(&lease).expect("show");
    drop(lease);

    let threads = std::thread::available_parallelism().map_or(1, usize::from);
    let mut cpu_ms = Vec::new();
    let mut cpu = Vec::new();
    for _ in 0..5 {
        let started = Instant::now();
        cpu = view.render_rgba8(threads).expect("cpu frame");
        cpu_ms.push(started.elapsed().as_secs_f64() * 1e3);
    }

    let mut renderer = MetalRenderer::new().expect("a Metal renderer");
    let mut metal_ms = Vec::new();
    let mut metal = Vec::new();
    let mut layout = String::new();
    let metal_rounds = std::env::var("FRAME_ROUTES_METAL_ROUNDS")
        .ok()
        .and_then(|rounds| rounds.parse().ok())
        .unwrap_or(5);
    for _ in 0..metal_rounds {
        let started = Instant::now();
        let job = view.job().expect("job");
        let (frame, report) = renderer.render_three_d_rgba8(&job).expect("metal frame");
        if std::env::var_os("FRAME_ROUTES_REPORT").is_some() {
            eprintln!("{report:?}");
        }
        metal_ms.push(started.elapsed().as_secs_f64() * 1e3);
        layout = format!("{:?}", frame.layout());
        metal = frame.as_bytes().to_vec();
    }

    let mut png_ms = Vec::new();
    for _ in 0..3 {
        let started = Instant::now();
        let png = fmn_codec::png::encode_rgba8(size, size, &cpu, CompressionLevel::Fast);
        png_ms.push(started.elapsed().as_secs_f64() * 1e3);
        assert!(!png.is_empty());
    }

    let same_length = cpu.len() == metal.len();
    let mut differing_bytes = 0_u64;
    let mut differing_pixels = 0_u64;
    let mut largest = 0_u8;
    let mut histogram = [0_u64; 4];
    for (a, b) in cpu.as_chunks::<4>().0.iter().zip(metal.as_chunks::<4>().0) {
        let mut pixel = false;
        for (&x, &y) in a.iter().zip(b) {
            let delta = x.abs_diff(y);
            if delta > 0 {
                differing_bytes += 1;
                pixel = true;
                largest = largest.max(delta);
                histogram[match delta {
                    1 => 0,
                    2..=3 => 1,
                    4..=15 => 2,
                    _ => 3,
                }] += 1;
            }
        }
        differing_pixels += u64::from(pixel);
    }
    println!(
        "{{\"ticks\":{ticks},\"size\":{size},\"live\":{},\"cpu_threads\":{threads},\
         \"cpu_ms\":{:.2},\"metal_ms\":{:.2},\"png_fast_ms\":{:.2},\"metal_layout\":\"{}\",\
         \"same_length\":{same_length},\"pixels\":{},\"differing_pixels\":{differing_pixels},\
         \"differing_bytes\":{differing_bytes},\"largest_byte_delta\":{largest},\
         \"delta_1/2-3/4-15/16+\":{:?}}}",
        world.population().len(),
        median(cpu_ms),
        median(metal_ms),
        median(png_ms),
        layout.replace('"', "'"),
        cpu.len() / 4,
        histogram,
    );
}
