//! `flife render`: pre-rendered frames of the reference world through the
//! Studio's FrankenManim view, without perturbing the world.

use std::num::NonZeroUsize;

use flife_cli::{RENDER_SCHEMA, RenderOptions, RenderRecord, render};
use flife_core::ValidatedConfig;
use flife_engine::{Arrangement, NumericalProfile, Stepped, World};

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

#[test]
fn rendering_writes_png_frames_and_leaves_the_world_as_a_plain_run_leaves_it() {
    let config = ValidatedConfig::from_toml(PRESET).expect("the preset validates");
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("flife-render-test");
    std::fs::create_dir_all(&out).expect("a scratch directory");
    let options = RenderOptions {
        ticks: 6,
        every: 3,
        size: [96, 64],
        out: out.clone(),
        cpu_workers: NonZeroUsize::new(3),
        render_threads: NonZeroUsize::MIN,
        video: None,
        fps: 30,
    };
    let mut records = Vec::new();
    render(&config, &options, |record| records.push(record.clone())).expect("renders");

    let frames: Vec<_> = records
        .iter()
        .filter_map(|record| match record {
            RenderRecord::Frame {
                schema,
                frame,
                tick,
                path,
                ..
            } => Some((*schema, *frame, *tick, path.clone())),
            RenderRecord::End { .. } => None,
        })
        .collect();
    assert_eq!(
        frames.iter().map(|f| (f.1, f.2)).collect::<Vec<_>>(),
        [(0, 0), (1, 3), (2, 6)],
        "a frame at the founding and every third tick"
    );
    for (schema, _, _, path) in &frames {
        assert_eq!(*schema, RENDER_SCHEMA);
        let png = std::fs::read(path).expect("the frame exists");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "{path} is a PNG");
        // IHDR: width and height, big-endian, after the signature and the
        // chunk's length and type.
        assert_eq!(&png[16..24], &[0, 0, 0, 96, 0, 0, 0, 64]);
    }

    // Rendering only reads epochs: the world is the one a plain run of six
    // ticks on one thread commits.
    let mut plain = World::found(
        &config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("founds");
    for _ in 0..6 {
        assert!(matches!(
            plain.step().expect("steps"),
            Stepped::Published(_)
        ));
    }
    let Some(RenderRecord::End {
        frames: count,
        tick,
        digest,
        video,
        ..
    }) = records.last()
    else {
        panic!("the last record is the end");
    };
    assert_eq!((*count, *tick, video.as_deref()), (3, 6, None));
    assert_eq!(*digest, plain.digest().to_hex());
}
