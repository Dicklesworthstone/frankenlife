//! V2-16 pure read-only (`INV-09`) visual mapping primitive tests:
//! - `cell-state-v1` (clonal `h_0` differentiation within founder `LineageId`),
//! - `polarity-v1` (cyclic perceptual OKLab angle wheel + unpolarized sentinel),
//! - bulk `apical_ticks_mesh` geometry, clockwise winding, and CPU render pixel oracle,
//! - `trail-carrier` & `vascular-conductance` field overlay palettes, log/raw levels,
//!   linear compositing, torus-seamless wrapping, caching, and CPU render pixel checks,
//! - compact legends verified within FL-024's `33,792` HUD vertex cap at 2x Retina,
//! - `INV-09` zero scientific feedback against a headless control world.

mod common;

use std::f64::consts::PI;

use flife_core::{CoreError, FeatureOverride};
use flife_manim::{
    APICAL_TICK_COLOR, APICAL_TICK_END_RATIO, APICAL_TICK_HALF_WIDTH_RATIO,
    APICAL_TICK_START_RATIO, CELL_STATE_MAPPING_NAME, CELL_STATE_REGISTRY_NAME, CELL_STATE_STEPS,
    ColorMapping, DensityLayer, FieldLayer, FieldMapping, FieldOverlayKind, FieldOverlayLayer,
    HUD_MAX_SWATCHES, HUD_SWATCH, HudFont, LEGEND_CELL_STATES, LINEAGE_PALETTE, Look,
    OVERLAY_LOG_GAIN, POLARITY_ANGLE_SCALE, POLARITY_MAPPING_NAME, POLARITY_REGISTRY_NAME,
    POLARITY_UNPOLARIZED, POLARITY_UNPOLARIZED_COLOR, ParticleCloud, Particles, SCALE_DECAY,
    SPHERE_SHADING, Style, TRAIL_CARRIER_NAME, TRAIL_CARRIER_PALETTE, V2ColorMapping,
    VASCULAR_CONDUCTANCE_NAME, VASCULAR_CONDUCTANCE_PALETTE, Viewport, WorldView, apical_tick_draw,
    apical_tick_quad, apical_ticks_mesh, build_composite_overlay_texture, build_overlay_texture,
    cell_state_color, cell_state_legend, cell_state_rgb8, decode_cell_state,
    decode_polarity_radians, display_value, lineage_slot, linear, lower_cell_state_colors,
    lower_polarity_colors, mapped_color, overlay_blend_linear, overlay_level, overlay_log_value,
    palette, polarity_color, polarity_legend, polarity_rgb8, polarity_scene_direction,
    polarity_turn, polarity_wheel_rgb8, quantize_cell_state, quantize_polarity, raw_value,
    trail_carrier_rgb8, traits, vascular_conductance_rgb8,
};
use fmn_core::color::{LinearRgba, Srgb};
use fmn_render::{Texture, ThreeDDraw, ThreeDJob, Tiling, TrueDotDraw};

/// Independent CIE L* (D65) lightness oracle for sRGB bytes.
fn cie_lightness([r, g, b]: [u8; 3]) -> f64 {
    let lin = |c: u8| {
        let u = f64::from(c) / 255.0;
        if u <= 0.040_45 {
            u / 12.92
        } else {
            ((u + 0.055) / 1.055).powf(2.4)
        }
    };
    let y = 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
    if y > 216.0 / 24_389.0 {
        116.0 * y.cbrt() - 16.0
    } else {
        y * 24_389.0 / 27.0
    }
}

fn srgb_distance(a: [u8; 3], b: [u8; 3]) -> f64 {
    let dr = f64::from(a[0]) - f64::from(b[0]);
    let dg = f64::from(a[1]) - f64::from(b[1]);
    let db = f64::from(a[2]) - f64::from(b[2]);
    (dr * dr + dg * dg + db * db).sqrt()
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn texels(texture: &Texture) -> Vec<LinearRgba> {
    (0..texture.height())
        .flat_map(|row| {
            (0..texture.width()).map(move |col| texture.texel(col, row).expect("texel"))
        })
        .collect()
}

/// V2-16 test-plan requirement: verify valid RGB `[0, 1]` across full input
/// ranges and strong visual separation between `h_0 = -1` and `h_0 = +1`
/// within every `LineageId`, plus cyclic perceptual uniformity & branch-cut
/// continuity across every `i16` polarity code.
#[test]
fn cell_state_and_polarity_color_mapping_finite_and_distinct() {
    // Quantizer / decoder contracts & non-finite hygiene.
    assert_eq!(quantize_cell_state(-1.0), -CELL_STATE_STEPS);
    assert_eq!(quantize_cell_state(0.0), 0);
    assert_eq!(quantize_cell_state(1.0), CELL_STATE_STEPS);
    assert_eq!(quantize_cell_state(-5.0), -CELL_STATE_STEPS);
    assert_eq!(quantize_cell_state(5.0), CELL_STATE_STEPS);
    assert_eq!(quantize_cell_state(f32::NAN), 0);
    assert_eq!(quantize_cell_state(f32::INFINITY), 0);
    assert_eq!(quantize_cell_state(f32::NEG_INFINITY), 0);
    assert_eq!(decode_cell_state(i8::MIN), -1.0);
    assert_eq!(decode_cell_state(-CELL_STATE_STEPS), -1.0);
    assert_eq!(decode_cell_state(0), 0.0);
    assert_eq!(decode_cell_state(CELL_STATE_STEPS), 1.0);

    // Find one concrete lineage id hitting each of the 12 palette slots.
    let mut representatives = [0_u64; LINEAGE_PALETTE.len()];
    let mut found = [false; LINEAGE_PALETTE.len()];
    for id in 0_u64..10_000 {
        let slot = lineage_slot(id);
        if !found[slot] {
            representatives[slot] = id;
            found[slot] = true;
        }
        if found.iter().all(|&ok| ok) {
            break;
        }
    }
    assert!(found.iter().all(|&ok| ok));

    for (slot, &lineage) in representatives.iter().enumerate() {
        let mut prev_l = -1.0_f64;
        for q in i8::MIN..=i8::MAX {
            let rgb = cell_state_rgb8(lineage, q);
            let lin = cell_state_color(lineage, q);
            assert_eq!(lin, linear(rgb), "linear table parity slot {slot} q={q}");
            for c in [lin.r, lin.g, lin.b] {
                assert!(c.is_finite() && (0.0..=1.0).contains(&c));
            }
            assert_eq!(lin.a, 1.0);
            if q >= -CELL_STATE_STEPS {
                let l = cie_lightness(rgb);
                assert!(
                    l + 1.0 >= prev_l,
                    "slot {slot} q={q}: lightness fell sharply from {prev_l} to {l}"
                );
                prev_l = prev_l.max(l);
            }
        }
        // Strong visual separation between h_0 = -1 and h_0 = +1 within one LineageId.
        let low = cell_state_rgb8(lineage, -CELL_STATE_STEPS);
        let mid = cell_state_rgb8(lineage, 0);
        let high = cell_state_rgb8(lineage, CELL_STATE_STEPS);
        let dl = cie_lightness(high) - cie_lightness(low);
        let dist = srgb_distance(low, high);
        assert!(
            dl > 45.0,
            "slot {slot}: expected ΔL* > 45 between h_0=-1 and +1, got {dl:.1}"
        );
        assert!(
            dist > 135.0,
            "slot {slot}: expected sRGB distance > 135 between h_0=-1 and +1, got {dist:.1}"
        );
        assert!(
            cie_lightness(low) > 20.0,
            "slot {slot}: h_0=-1 must remain visible on dark canvas, got L*={:.1}",
            cie_lightness(low)
        );
        assert!(
            srgb_distance(low, mid) > 50.0 && srgb_distance(mid, high) > 50.0,
            "slot {slot}: mid state must separate clearly from both poles"
        );
        // i8::MIN clamps identically to -127.
        assert_eq!(cell_state_rgb8(lineage, i8::MIN), low);
    }

    // Distinct lineages remain visually distinct at h_0 = 0.
    for i in 0..representatives.len() {
        for j in (i + 1)..representatives.len() {
            let a = cell_state_rgb8(representatives[i], 0);
            let b = cell_state_rgb8(representatives[j], 0);
            assert!(
                srgb_distance(a, b) > 25.0,
                "lineage slots {i} and {j} collided at h_0=0: {a:?} vs {b:?}"
            );
        }
    }

    // Polarity quantizer & coordinate convention (world y down, matching heading()).
    assert_eq!(quantize_polarity([0.0, 0.0]), POLARITY_UNPOLARIZED);
    assert_eq!(quantize_polarity([f32::NAN, 1.0]), POLARITY_UNPOLARIZED);
    assert_eq!(
        quantize_polarity([1.0, f32::INFINITY]),
        POLARITY_UNPOLARIZED
    );
    assert_eq!(decode_polarity_radians(POLARITY_UNPOLARIZED), None);
    assert_eq!(polarity_turn(POLARITY_UNPOLARIZED), None);
    assert_eq!(polarity_scene_direction(POLARITY_UNPOLARIZED), None);
    assert_eq!(
        polarity_rgb8(POLARITY_UNPOLARIZED),
        POLARITY_UNPOLARIZED_COLOR
    );
    assert_eq!(polarity_wheel_rgb8(f64::NAN), POLARITY_UNPOLARIZED_COLOR);

    let east = quantize_polarity([1.0, 0.0]);
    let north = quantize_polarity([0.0, -1.0]);
    let west = quantize_polarity([-1.0, 0.0]);
    let south = quantize_polarity([0.0, 1.0]);
    assert!((polarity_turn(east).expect("east") - 0.0).abs() < 1e-4);
    assert!((polarity_turn(north).expect("north") - 0.25).abs() < 1e-4);
    assert!((polarity_turn(west).expect("west") - 0.5).abs() < 1e-4);
    assert!((polarity_turn(south).expect("south") - 0.75).abs() < 1e-4);
    // Exact branch-cut continuity across +pi / -pi.
    assert_eq!(
        polarity_rgb8(POLARITY_ANGLE_SCALE),
        polarity_rgb8(-POLARITY_ANGLE_SCALE),
        "+pi and -pi must map to identical colour across branch cut"
    );
    assert_eq!(
        polarity_color(POLARITY_ANGLE_SCALE),
        polarity_color(-POLARITY_ANGLE_SCALE)
    );
    // Opposite directions have strong visual contrast.
    assert!(srgb_distance(polarity_rgb8(east), polarity_rgb8(west)) > 80.0);
    assert!(srgb_distance(polarity_rgb8(north), polarity_rgb8(south)) > 80.0);
    // Exhaustively check ALL 65,536 i16 polarity values for finite [0, 1] & perceptual uniformity.
    let mut min_l = f64::INFINITY;
    let mut max_l = f64::NEG_INFINITY;
    for q in i16::MIN..=i16::MAX {
        let rgb = polarity_rgb8(q);
        let lin = polarity_color(q);
        assert_eq!(lin, linear(rgb), "polarity linear parity q={q}");
        for c in [lin.r, lin.g, lin.b] {
            assert!(c.is_finite() && (0.0..=1.0).contains(&c));
        }
        assert_eq!(lin.a, 1.0);
        if q != POLARITY_UNPOLARIZED {
            let l = cie_lightness(rgb);
            min_l = min_l.min(l);
            max_l = max_l.max(l);
            assert!(
                srgb_distance(rgb, POLARITY_UNPOLARIZED_COLOR) > 35.0,
                "polarized angle q={q} collided with unpolarized grey"
            );
        }
    }
    assert!(
        min_l > 60.0 && max_l < 85.0 && (max_l - min_l) < 12.0,
        "cyclic polarity wheel should stay perceptually uniform and luminous: L* in [{min_l:.1}, {max_l:.1}]"
    );
    assert!((decode_polarity_radians(POLARITY_ANGLE_SCALE).expect("pi") - PI).abs() < 1e-12);
}

/// Verify apical orientation tick geometry, clockwise XY winding, refusal of
/// invalid inputs, seam wrapping, and actual CPU rasterization through
/// FrankenManim's `ThreeDJob::render`.
#[test]
fn apical_tick_geometry_winding_and_cpu_pixel_render() {
    let viewport = Viewport::fit([32.0, 32.0], [160, 160]).expect("viewport");
    let north = quantize_polarity([0.0, -1.0]);
    let east = quantize_polarity([1.0, 0.0]);
    let south = quantize_polarity([0.0, 1.0]);
    let west = quantize_polarity([-1.0, 0.0]);

    // Scene direction oracle: world north (0, -1) -> scene up (0, +1).
    let dir_n = polarity_scene_direction(north).expect("north");
    assert!(dir_n[0].abs() < 2e-4 && (dir_n[1] - 1.0).abs() < 2e-4);
    let dir_e = polarity_scene_direction(east).expect("east");
    assert!((dir_e[0] - 1.0).abs() < 2e-4 && dir_e[1].abs() < 2e-4);
    let dir_s = polarity_scene_direction(south).expect("south");
    assert!(dir_s[0].abs() < 2e-4 && (dir_s[1] + 1.0).abs() < 2e-4);
    let dir_w = polarity_scene_direction(west).expect("west");
    assert!((dir_w[0] + 1.0).abs() < 2e-4 && dir_w[1].abs() < 2e-4);

    // Planted negatives on apical_tick_quad.
    assert!(
        apical_tick_quad(
            [0.0, 0.0, 0.0],
            1.0,
            POLARITY_UNPOLARIZED,
            APICAL_TICK_COLOR
        )
        .is_none()
    );
    assert!(apical_tick_quad([0.0, 0.0, 0.0], 0.0, east, APICAL_TICK_COLOR).is_none());
    assert!(apical_tick_quad([0.0, 0.0, 0.0], -1.0, east, APICAL_TICK_COLOR).is_none());
    assert!(apical_tick_quad([0.0, 0.0, 0.0], 1e308, east, APICAL_TICK_COLOR).is_none());
    assert!(apical_tick_quad([f64::NAN, 0.0, 0.0], 1.0, east, APICAL_TICK_COLOR).is_none());

    // Exact quad dimensions & clockwise XY winding for every cardinal angle.
    let r = 2.0;
    let expected_area = (APICAL_TICK_END_RATIO - APICAL_TICK_START_RATIO)
        * r
        * (2.0 * APICAL_TICK_HALF_WIDTH_RATIO * r);
    for angle in [east, north, west, south] {
        let (verts, idx) =
            apical_tick_quad([1.0, -2.0, 0.0], r, angle, APICAL_TICK_COLOR).expect("quad");
        assert_eq!(idx, [0, 1, 2, 0, 2, 3]);
        let mut signed_area = 0.0_f64;
        for tri in idx.as_chunks::<3>().0 {
            let a = verts[tri[0] as usize].point;
            let b = verts[tri[1] as usize].point;
            let c = verts[tri[2] as usize].point;
            signed_area += 0.5 * ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]));
        }
        assert!(
            signed_area < 0.0,
            "apical tick quad must be wound clockwise in scene XY, signed_area={signed_area}"
        );
        assert!(
            (signed_area.abs() - expected_area).abs() < 1e-9,
            "area {} != expected {expected_area}",
            signed_area.abs()
        );
    }

    // Bulk mesh validation & unpolarized skipping.
    assert!(matches!(
        apical_ticks_mesh(&viewport, &[[16.0, 16.0]], &[], 1.0, APICAL_TICK_COLOR),
        Err(CoreError::LayoutMismatch { .. })
    ));
    assert!(matches!(
        apical_ticks_mesh(
            &viewport,
            &[[16.0, 16.0]],
            &[north],
            -0.5,
            APICAL_TICK_COLOR
        ),
        Err(CoreError::NumericalInvalid { .. })
    ));
    assert!(matches!(
        apical_ticks_mesh(
            &viewport,
            &[[f32::NAN, 16.0]],
            &[north],
            1.0,
            APICAL_TICK_COLOR
        ),
        Err(CoreError::NumericalInvalid { .. })
    ));
    assert!(
        apical_ticks_mesh(
            &viewport,
            &[[16.0, 16.0], [8.0, 8.0]],
            &[POLARITY_UNPOLARIZED, POLARITY_UNPOLARIZED],
            1.0,
            APICAL_TICK_COLOR,
        )
        .expect("all unpolarized")
        .is_none()
    );
    // Polarized entries whose tick quads overflow f32 scene coordinates (e.g.
    // `end = dot_radius * 1.45` is finite in f32, while a diagonal corner
    // `dx * end + |lx| * half_w` exceeds `f32::MAX`) are skipped cleanly,
    // returning Ok(None) rather than an empty-mesh error.
    let near_max_radius = f64::from(f32::MAX) / 1.45 * 0.999;
    assert!(
        apical_ticks_mesh(
            &viewport,
            &[[16.0, 16.0]],
            &[1000_i16],
            near_max_radius,
            APICAL_TICK_COLOR,
        )
        .expect("overflowing tick quad returns Ok(None)")
        .is_none()
    );

    // Real FrankenManim CPU ThreeDJob render test: a sphere-mode organism at
    // world center [16, 16] with northward polarity (world -y => drawable up).
    let dot_r = 2.0_f64; // 10 px radius at 5 px/world-unit.
    let mesh = apical_ticks_mesh(
        &viewport,
        &[[16.0, 16.0], [4.0, 4.0]],
        &[north, POLARITY_UNPOLARIZED],
        dot_r,
        APICAL_TICK_COLOR,
    )
    .expect("mesh")
    .expect("one tick");
    assert_eq!(mesh.vertices().len(), 4);
    assert_eq!(mesh.indices().len(), 6);

    let mut dot = TrueDotDraw::new(
        viewport.scene_point([16.0, 16.0]),
        dot_r,
        LinearRgba {
            r: 0.05,
            g: 0.15,
            b: 0.35,
            a: 1.0,
        },
    );
    dot.shading = SPHERE_SHADING;
    let draws = [ThreeDDraw::TrueDot(dot), apical_tick_draw(&mesh)];
    let camera = viewport
        .camera(Srgb::from_rgb8(0, 0, 0).to_linear(1.0))
        .expect("camera");
    let frame = ThreeDJob::new(&camera, &draws, Tiling::default())
        .expect("job")
        .render(2)
        .expect("render");
    let mut rgba = vec![0_u8; 160 * 160 * 4];
    fmn_frame::convert::rgba16f_to_rgba8_slice(&frame, &mut rgba).expect("rgba8");
    let pixel = |x: usize, y: usize| -> [u8; 4] {
        let i = (y * 160 + x) * 4;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
    };
    // Center is (80, 80); dot radius is 10 px; tick extends from 3.5 px to 14.5 px
    // NORTH (smaller y: rows 66..76 at x=80). At (80, 68) — outside the dot rim,
    // inside the apical tick tip — the pixel must be bright ivory!
    let tip = pixel(80, 68);
    assert!(
        tip[0] > 210 && tip[1] > 210 && tip[2] > 200,
        "northward apical tick tip at (80, 68) must be bright ivory, got {tip:?}"
    );
    // Symmetrically SOUTH of the dot rim at (80, 92), there is no tick -> black background.
    let basal = pixel(80, 92);
    assert!(
        basal[0] < 5 && basal[1] < 5 && basal[2] < 5,
        "basal side at (80, 92) outside dot must stay black, got {basal:?}"
    );
}

/// Verify trail-carrier and vascular-conductance palettes, log/raw levels,
/// linear compositing, torus-seamless corner averaging, texture/quad caching,
/// error preservation, and CPU rendered pixel output.
#[test]
fn trail_and_vascular_overlay_palettes_log_ramp_blending_and_caching() {
    // Both palettes rise monotonically in CIE L* and differ clearly from energy & each other.
    for (name, stops, func) in [
        (
            "trail",
            &TRAIL_CARRIER_PALETTE,
            trail_carrier_rgb8 as fn(f64) -> [u8; 3],
        ),
        (
            "vascular",
            &VASCULAR_CONDUCTANCE_PALETTE,
            vascular_conductance_rgb8 as fn(f64) -> [u8; 3],
        ),
    ] {
        assert_eq!(func(0.0), stops[0], "{name} start");
        assert_eq!(func(1.0), stops[stops.len() - 1], "{name} end");
        assert_eq!(func(f64::NAN), stops[0], "{name} NaN");
        let mut prev_l = -1.0_f64;
        for step in 0..=100 {
            let l = cie_lightness(func(f64::from(step) / 100.0));
            assert!(
                l + 0.5 >= prev_l,
                "{name} lightness dropped at {step}%: {prev_l} -> {l}"
            );
            prev_l = prev_l.max(l);
        }
        assert!(cie_lightness(stops[0]) < 5.0, "{name} dark start");
        assert!(
            cie_lightness(stops[stops.len() - 1]) > 85.0,
            "{name} bright end"
        );
    }
    assert!(srgb_distance(trail_carrier_rgb8(0.75), palette(0.75)) > 80.0);
    assert!(srgb_distance(vascular_conductance_rgb8(0.75), palette(0.75)) > 80.0);
    assert!(srgb_distance(trail_carrier_rgb8(0.75), vascular_conductance_rgb8(0.75)) > 55.0);

    // Log & raw levels + non-finite safety (oracle uses exact promoted f32 value).
    let expected_tenth = (OVERLAY_LOG_GAIN * f64::from(0.1_f32)).ln_1p() / OVERLAY_LOG_GAIN.ln_1p();
    assert!((overlay_log_value(0.1_f32, 1.0) - expected_tenth).abs() < 1e-12);
    let expected_eighth = (OVERLAY_LOG_GAIN * 0.125_f64).ln_1p() / OVERLAY_LOG_GAIN.ln_1p();
    assert!((overlay_log_value(0.125_f32, 1.0) - expected_eighth).abs() < 1e-12);
    assert!(
        overlay_log_value(0.1, 1.0) > 0.32,
        "log ramp lifts faint trails/vessels"
    );
    assert_eq!(overlay_log_value(0.0, 1.0), 0.0);
    assert_eq!(overlay_log_value(-1.0, 1.0), 0.0);
    assert_eq!(overlay_log_value(f32::NAN, 1.0), 0.0);
    assert_eq!(overlay_log_value(1.0, 0.0), 0.0);
    assert_eq!(overlay_log_value(1.0, f64::NAN), 0.0);
    assert_eq!(overlay_level(0.5, 2.0, Look::Raw), raw_value(0.5, 2.0));
    assert_eq!(overlay_level(0.5, 2.0, Look::Sphere), raw_value(0.5, 2.0));
    assert_eq!(
        overlay_level(0.5, 2.0, Look::Glow),
        overlay_log_value(0.5, 2.0)
    );

    // Linear blender contracts & NaN safety.
    let base = LinearRgba {
        r: 0.2,
        g: 0.4,
        b: 0.6,
        a: 1.0,
    };
    let ov = LinearRgba {
        r: 0.9,
        g: 0.1,
        b: 0.2,
        a: 1.0,
    };
    assert_eq!(overlay_blend_linear(base, ov, 0.0), base);
    assert_eq!(overlay_blend_linear(base, ov, 1.0), ov);
    let half = overlay_blend_linear(base, ov, 0.5);
    assert!(
        (half.r - 0.55).abs() < 1e-12
            && (half.g - 0.25).abs() < 1e-12
            && (half.b - 0.4).abs() < 1e-12
    );
    let safe = overlay_blend_linear(
        LinearRgba {
            r: f64::NAN,
            g: 0.5,
            b: 0.5,
            a: 1.0,
        },
        ov,
        f64::NAN,
    );
    assert!(
        [safe.r, safe.g, safe.b, safe.a]
            .iter()
            .all(|c| c.is_finite() && (0.0..=1.0).contains(c))
    );

    // Independent torus-seam corner-mean oracle on a 4x4 plane.
    let grid = [4_u32, 4];
    let plane: Vec<f32> = (0..16).map(|i| if i == 15 { 4.0 } else { 0.0 }).collect();
    let tex = build_overlay_texture(
        grid,
        &plane,
        4.0,
        Look::Glow,
        FieldOverlayKind::TrailCarrier,
    )
    .expect("overlay texture");
    let got = texels(&tex);
    // Corner (0, 0) wraps around (3, 3) = 4.0 and (0, 3), (3, 0), (0, 0) = 0 -> mean = 1.0.
    let seam_level = overlay_log_value(1.0, 4.0);
    let seam_expected = linear(trail_carrier_rgb8(seam_level));
    assert!((got[0].r - seam_expected.r).abs() < 1e-5);
    assert!((got[0].g - seam_expected.g).abs() < 1e-5);
    assert!((got[0].b - seam_expected.b).abs() < 1e-5);
    // Interior corner (2, 2) touches only zeros -> mean = 0.0.
    let zero_expected = linear(trail_carrier_rgb8(0.0));
    assert!((got[2 * 4 + 2].r - zero_expected.r).abs() < 1e-5);

    // Composite overlay: zero overlay cell equals pure energy; active cell blends linearly.
    let energy = vec![2.0_f32; 16];
    let comp = build_composite_overlay_texture(
        grid,
        &energy,
        2.0,
        &plane,
        4.0,
        FieldOverlayKind::VascularConductance,
        0.75,
        Look::Glow,
    )
    .expect("composite");
    let comp_texels = texels(&comp);
    let pure_energy = linear(palette(display_value(2.0, 2.0)));
    assert!(
        (comp_texels[2 * 4 + 2].r - pure_energy.r).abs() < 1e-5
            && (comp_texels[2 * 4 + 2].g - pure_energy.g).abs() < 1e-5
            && (comp_texels[2 * 4 + 2].b - pure_energy.b).abs() < 1e-5,
        "zero-overlay cell must preserve base energy bit-for-bit"
    );
    let vasc_col = linear(vascular_conductance_rgb8(seam_level));
    let expected_blend = overlay_blend_linear(pure_energy, vasc_col, 0.75 * seam_level);
    assert!((comp_texels[0].r - expected_blend.r).abs() < 1e-5);
    assert!((comp_texels[0].g - expected_blend.g).abs() < 1e-5);
    assert!((comp_texels[0].b - expected_blend.b).abs() < 1e-5);
    assert!(matches!(
        build_composite_overlay_texture(
            grid,
            &energy,
            2.0,
            &plane,
            4.0,
            FieldOverlayKind::VascularConductance,
            1.5,
            Look::Glow,
        ),
        Err(CoreError::NumericalInvalid { .. })
    ));

    // FieldOverlayLayer caching, scale decay, Raw<->Sphere reuse, track_scale, and error rollback.
    let viewport = Viewport::fit([16.0, 16.0], [96, 96]).expect("viewport");
    let mut layer = FieldOverlayLayer::new(&viewport);
    assert!(layer.draw().is_none());
    layer
        .track_scale(grid, &plane)
        .expect("track_scale while hidden");
    assert_eq!(layer.scale(), 4.0);
    assert!(
        layer.texture().is_none(),
        "track_scale must not build texture"
    );

    layer
        .update(grid, &plane, Look::Raw, FieldOverlayKind::TrailCarrier)
        .expect("update raw");
    let raw_texels = texels(layer.texture().expect("tex"));
    let raw_quad = match layer.draw().expect("draw") {
        ThreeDDraw::Surface(s) => s.mesh.clone(),
        _ => panic!("not surface"),
    };
    // Sphere keeps identical Raw texels & unchanged viewport keeps identical quad.
    layer
        .place(grid, &plane, Look::Sphere, FieldOverlayKind::TrailCarrier)
        .expect("place sphere");
    assert_eq!(texels(layer.texture().expect("tex")), raw_texels);
    let sphere_quad = match layer.draw().expect("draw") {
        ThreeDDraw::Surface(s) => s.mesh,
        _ => panic!("not surface"),
    };
    assert_eq!(sphere_quad, &raw_quad);

    // Malformed plane refuses cleanly without disturbing existing scale or texture.
    assert!(matches!(
        layer.update(grid, &[1.0; 3], Look::Glow, FieldOverlayKind::TrailCarrier),
        Err(CoreError::LayoutMismatch { .. })
    ));
    assert_eq!(layer.scale(), 4.0);
    assert_eq!(texels(layer.texture().expect("kept tex")), raw_texels);

    // Dimmer plane decays scale by SCALE_DECAY.
    let dim = vec![0.0_f32; 16];
    layer
        .update(grid, &dim, Look::Glow, FieldOverlayKind::TrailCarrier)
        .expect("dim");
    assert!((layer.scale() - 4.0 * SCALE_DECAY).abs() < 1e-12);

    // Real FrankenManim CPU render across a seam-centered view: Glow vs Raw distinct.
    let seam_view = viewport.with_view([0.0, 0.0], 2.0).expect("seam view");
    let bright = vec![3.0_f32; 16];
    let camera = seam_view
        .camera(Srgb::from_rgb8(0, 0, 0).to_linear(1.0))
        .expect("camera");
    let render_overlay = |look: Look, kind: FieldOverlayKind| -> [u8; 4] {
        let mut l = FieldOverlayLayer::new(&seam_view);
        l.update(grid, &bright, look, kind).expect("update");
        let draw = l.draw().expect("draw");
        let frame = ThreeDJob::new(&camera, &[draw], Tiling::default())
            .expect("job")
            .render(2)
            .expect("render");
        let mut rgba = vec![0_u8; 96 * 96 * 4];
        fmn_frame::convert::rgba16f_to_rgba8_slice(&frame, &mut rgba).expect("rgba8");
        let mid = (48 * 96 + 48) * 4;
        [rgba[mid], rgba[mid + 1], rgba[mid + 2], rgba[mid + 3]]
    };
    let trail_px = render_overlay(Look::Glow, FieldOverlayKind::TrailCarrier);
    let vasc_px = render_overlay(Look::Glow, FieldOverlayKind::VascularConductance);
    let raw_px = render_overlay(Look::Raw, FieldOverlayKind::TrailCarrier);
    assert!(trail_px[0] > 230 && trail_px[1] > 210 && trail_px[2] < trail_px[0]);
    assert!(vasc_px[0] > 230 && vasc_px[2] > 180);
    assert_ne!(trail_px, vasc_px);
    assert_eq!(raw_px[0], raw_px[1]);
    assert_eq!(raw_px[1], raw_px[2]);
    assert!(
        raw_px[0] > 240,
        "raw max value renders white, got {raw_px:?}"
    );
}

/// Verify V2-16 string names, parsing, legends, swatch parity, and FL-024's
/// `33,792` HUD vertex budget at 2x Retina (`em_pixels = 26.0`).
#[test]
fn v2_names_legends_and_fl024_hud_vertex_budget() {
    assert_eq!(V2ColorMapping::CellState.name(), CELL_STATE_MAPPING_NAME);
    assert_eq!(
        V2ColorMapping::CellState.registry_name(),
        CELL_STATE_REGISTRY_NAME
    );
    assert_eq!(V2ColorMapping::Polarity.name(), POLARITY_MAPPING_NAME);
    assert_eq!(
        V2ColorMapping::Polarity.registry_name(),
        POLARITY_REGISTRY_NAME
    );
    for mapping in V2ColorMapping::ALL {
        assert_eq!(mapping.name().parse::<V2ColorMapping>(), Ok(mapping));
        assert_eq!(
            mapping.registry_name().parse::<V2ColorMapping>(),
            Ok(mapping)
        );
        assert_eq!(
            V2ColorMapping::from_registry(mapping.registry_name()),
            Some(mapping)
        );
        assert_eq!(mapping.to_string(), mapping.name());
        assert!(!mapping.meaning().is_empty());
    }
    assert!("unknown".parse::<V2ColorMapping>().is_err());

    assert_eq!(FieldOverlayKind::TrailCarrier.name(), TRAIL_CARRIER_NAME);
    assert_eq!(
        FieldOverlayKind::VascularConductance.name(),
        VASCULAR_CONDUCTANCE_NAME
    );
    for kind in FieldOverlayKind::ALL {
        assert_eq!(kind.name().parse::<FieldOverlayKind>(), Ok(kind));
        assert_eq!(kind.to_string(), kind.name());
    }
    assert!("energy".parse::<FieldOverlayKind>().is_err());

    let cs_legend = cell_state_legend();
    assert_eq!(cs_legend, V2ColorMapping::CellState.legend());
    assert_eq!(cs_legend.swatches.len(), LEGEND_CELL_STATES.len());
    let pol_legend = polarity_legend();
    assert_eq!(pol_legend, V2ColorMapping::Polarity.legend());
    assert_eq!(pol_legend.swatches.len(), 5);
    assert_eq!(pol_legend.swatches[4], POLARITY_UNPOLARIZED_COLOR);

    let font = HudFont::bundled().expect("bundled font");
    for legend in [
        cs_legend.clone(),
        pol_legend.clone(),
        FieldOverlayKind::TrailCarrier.legend(),
        FieldOverlayKind::VascularConductance.legend(),
    ] {
        let swatch_chars = legend.text.chars().filter(|&c| c == HUD_SWATCH).count();
        assert_eq!(swatch_chars, legend.swatches.len(), "{}", legend.text);
        assert!(legend.swatches.len() <= HUD_MAX_SWATCHES);
        let lin_swatches: Vec<LinearRgba> = legend.swatches.iter().copied().map(linear).collect();
        let laid = font
            .text_with_swatches(&legend.text, 26.0, &lin_swatches)
            .expect("layout at 2x Retina");
        assert_eq!(laid.refused(), 0, "{}", legend.text);
        assert_eq!(laid.left_out(), 0, "{}", legend.text);
        assert!(
            laid.vertices().len() < 10_000,
            "single v2 legend '{}' used {} vertices",
            legend.text,
            laid.vertices().len()
        );
    }

    // Combined realistic Studio telemetry + cell-state legend + vascular legend
    // at 2x Retina stays strictly within FL-024's 33,792 HUD vertex cap.
    let vasc_legend = FieldOverlayKind::VascularConductance.legend();
    let combined = format!(
        "tick 12345 · pop 4096 / 16384 · 60.0 fps · energy 128450.2\n{}\n{}",
        cs_legend.text, vasc_legend.text
    );
    let combined_swatches: Vec<LinearRgba> = cs_legend
        .swatches
        .iter()
        .chain(&vasc_legend.swatches)
        .copied()
        .map(linear)
        .collect();
    let laid = font
        .text_with_swatches(&combined, 26.0, &combined_swatches)
        .expect("combined overlay");
    assert_eq!(laid.refused(), 0);
    assert_eq!(laid.left_out(), 0);
    assert!(
        laid.vertices().len() <= 33_792,
        "combined v2 overlay used {} vertices > FL-024's 33,792 cap",
        laid.vertices().len()
    );
}

/// V2-16 test-plan requirement (`INV-09`): batch colour lowering handles both
/// active and disabled (`None`) slices cleanly, refuses mismatched lengths,
/// and running all V2-16 visual mappings every tick leaves the simulation's
/// `WorldDigest` bit-identical to a strictly headless control world.
#[test]
fn inv09_presentation_zero_scientific_feedback() {
    let lineages = [10_u64, 20, 30];
    let states = [-127_i8, 0, 127];
    let polarities = [
        quantize_polarity([1.0, 0.0]),
        POLARITY_UNPOLARIZED,
        quantize_polarity([0.0, -1.0]),
    ];
    let mut out = Vec::new();
    lower_cell_state_colors(&lineages, Some(&states), &mut out).expect("cell state colors");
    assert_eq!(
        out,
        vec![
            cell_state_color(10, -127),
            cell_state_color(20, 0),
            cell_state_color(30, 127),
        ]
    );
    lower_cell_state_colors(&lineages, None, &mut out).expect("disabled fallback");
    assert_eq!(
        out,
        vec![
            cell_state_color(10, 0),
            cell_state_color(20, 0),
            cell_state_color(30, 0),
        ]
    );
    assert!(matches!(
        lower_cell_state_colors(&lineages, Some(&[0_i8; 2]), &mut out),
        Err(CoreError::LayoutMismatch { .. })
    ));

    lower_polarity_colors(3, Some(&polarities), &mut out).expect("polarity colors");
    assert_eq!(
        out,
        vec![
            polarity_color(polarities[0]),
            polarity_color(POLARITY_UNPOLARIZED),
            polarity_color(polarities[2]),
        ]
    );
    lower_polarity_colors(3, None, &mut out).expect("polarity disabled fallback");
    assert_eq!(out, vec![polarity_color(POLARITY_UNPOLARIZED); 3]);
    assert!(matches!(
        lower_polarity_colors(3, Some(&[0_i16; 2]), &mut out),
        Err(CoreError::LayoutMismatch { .. })
    ));

    // INV-09 bit-exact world digest check: headless vs observed with all V2-16 primitives.
    let config = common::small();
    let mut headless = common::found(&config);
    let mut observed = common::found(&config);
    let initial = common::epoch(&observed);
    let viewport = Viewport::fit(initial.extent(), [128, 128]).expect("viewport");
    let mut trail_layer = FieldOverlayLayer::new(&viewport);
    let mut vasc_layer = FieldOverlayLayer::new(&viewport);
    let mut color_buf = Vec::new();

    for _ in 0..16 {
        common::advance(&mut headless, 1);
        common::advance(&mut observed, 1);
        let epoch = common::epoch(&observed);
        let synthetic_states: Vec<i8> = epoch
            .energies()
            .iter()
            .map(|&e| quantize_cell_state((e - 1.5).clamp(-1.0, 1.0)))
            .collect();
        let synthetic_polarities: Vec<i16> = epoch
            .velocities()
            .iter()
            .map(|&v| quantize_polarity(v))
            .collect();
        lower_cell_state_colors(epoch.lineages(), Some(&synthetic_states), &mut color_buf)
            .expect("cell state lowering");
        lower_polarity_colors(epoch.len(), Some(&synthetic_polarities), &mut color_buf)
            .expect("polarity lowering");
        let _ticks = apical_ticks_mesh(
            &viewport,
            epoch.positions(),
            &synthetic_polarities,
            0.35,
            APICAL_TICK_COLOR,
        )
        .expect("apical ticks");
        let tex = epoch.field();
        trail_layer
            .update(
                tex.texels(),
                tex.values(),
                Look::Glow,
                FieldOverlayKind::TrailCarrier,
            )
            .expect("trail overlay");
        vasc_layer
            .update(
                tex.texels(),
                tex.values(),
                Look::Glow,
                FieldOverlayKind::VascularConductance,
            )
            .expect("vascular overlay");
        let _comp = build_composite_overlay_texture(
            tex.texels(),
            tex.values(),
            trail_layer.scale(),
            tex.values(),
            vasc_layer.scale(),
            FieldOverlayKind::VascularConductance,
            0.5,
            Look::Glow,
        )
        .expect("composite");
    }

    assert_eq!(
        observed.digest(),
        headless.digest(),
        "INV-09 violated: V2-16 presentation primitives altered WorldDigest"
    );
}

/// V2-16 end-to-end integration on real `field-ecology-v2` (`all-v2`) epochs:
/// - `ParticleCloud` and `DensityLayer` consume `Epoch::cell_states()` and
///   `Epoch::polarities()` directly via `ColorMapping::CellState` and
///   `ColorMapping::Polarity`;
/// - `FieldLayer` consumes `FieldTexture::trail_energy()` and
///   `FieldTexture::vascular_conductance()` directly via
///   `FieldMapping::TrailCarrier` and `FieldMapping::VascularConductance`;
/// - `WorldView` builds and emits the bulk `apical_ticks_draw()` mesh strictly
///   in `ColorMapping::Polarity + Look::Sphere + Particles::Points` when
///   `Epoch::polarities()` is present and polarized, keeping it synchronized
///   across `resize`, `set_point_radius`, `set_body_radius`, `set_view` +
///   `place`, and `show_between`;
/// - Rendering every v2 `ColorMapping` and `FieldMapping` through `WorldView`
///   every tick preserves bit-exact `WorldDigest` vs a headless control world
///   (`INV-09`).
#[test]
fn v2_world_view_cloud_field_density_and_apical_ticks_end_to_end() {
    let all_v2 = common::small_v2(true, &[FeatureOverride::Enable("all-v2".to_owned())]);
    let mut headless = common::found(&all_v2);
    let mut observed = common::found(&all_v2);
    common::advance(&mut headless, 20);
    common::advance(&mut observed, 20);

    let first = common::epoch(&observed);
    assert!(
        first.cell_states().is_some(),
        "all-v2 must expose Epoch::cell_states"
    );
    assert!(
        first.polarities().is_some(),
        "all-v2 must expose Epoch::polarities"
    );
    assert!(
        first.field().trail_energy().is_some(),
        "all-v2 must expose FieldTexture::trail_energy"
    );
    assert!(
        first.field().vascular_conductance().is_some(),
        "all-v2 must expose FieldTexture::vascular_conductance"
    );

    let viewport = Viewport::fit(first.extent(), [128, 128]).expect("viewport");
    let rows: Vec<_> = traits(&first).collect();
    assert_eq!(rows.len(), first.len());
    let states = first.cell_states().expect("cell states");
    let polarities = first.polarities().expect("polarities");
    for (i, row) in rows.iter().enumerate() {
        assert_eq!(row.cell_state, states[i], "row {i} cell_state");
        assert_eq!(row.polarity, polarities[i], "row {i} polarity");
    }

    // ParticleCloud directly reads Epoch::cell_states and Epoch::polarities.
    let mut cloud = ParticleCloud::for_epoch(&first).expect("cloud");
    for mapping in [ColorMapping::CellState, ColorMapping::Polarity] {
        let style = Style {
            mapping,
            ..Style::default()
        };
        let draws = cloud
            .update_styled(&first, &viewport, &style)
            .expect("cloud update");
        assert_eq!(draws.dots.len(), first.len());
        for (i, (dot, row)) in draws.dots.iter().zip(&rows).enumerate() {
            assert_eq!(
                dot.color,
                mapped_color(mapping, row, first.maximum_speed()),
                "{mapping} row {i}"
            );
        }
    }

    // DensityLayer supports both v2 ColorMappings directly.
    let mut density = DensityLayer::new(&viewport);
    for mapping in [ColorMapping::CellState, ColorMapping::Polarity] {
        let style = Style {
            mapping,
            particles: Particles::Density,
            ..Style::default()
        };
        density.update(&first, &style).expect("density update");
        assert!(density.texture().is_some());
        assert!(density.draw().is_some());
    }

    // FieldLayer supports TrailCarrier and VascularConductance directly on real v2 FieldTexture.
    let mut field_layer = FieldLayer::new(&viewport);
    for field_mapping in [
        FieldMapping::TrailCarrier,
        FieldMapping::VascularConductance,
    ] {
        field_layer
            .update_mapped(first.field(), Look::Glow, field_mapping)
            .expect("field overlay glow");
        assert!(field_layer.texture().is_some());
        field_layer
            .place_mapped(first.field(), Look::Raw, field_mapping)
            .expect("field overlay raw");
        for texel in texels(field_layer.texture().expect("raw tex")) {
            assert_eq!(texel.r, texel.g);
            assert_eq!(texel.g, texel.b);
            assert_eq!(texel.a, 1.0);
        }
    }

    // WorldView: render across styles, verify apical ticks strictly gated on
    // Polarity + Sphere + Points, and verify INV-09 bit-exact WorldDigest.
    let mut view = WorldView::for_epoch(&first, [128, 128], [2, 3, 8]).expect("world view");
    assert_eq!(
        view.scene_objects(),
        6,
        "v2 polarity-capable view accounts for the bulk apical-tick scene object"
    );
    let mut saw_ticks = false;
    for step in 0..8 {
        common::advance(&mut headless, 1);
        common::advance(&mut observed, 1);
        let epoch = common::epoch(&observed);
        let polarized_count = epoch
            .polarities()
            .expect("polarities")
            .iter()
            .filter(|&&p| p != POLARITY_UNPOLARIZED)
            .count();

        // 1. CellState + TrailCarrier (Glow): no apical ticks.
        view.set_style(Style {
            mapping: ColorMapping::CellState,
            look: Look::Glow,
            particles: Particles::Points,
            field: true,
            field_mapping: FieldMapping::TrailCarrier,
        });
        view.show(&epoch).expect("show cell-state");
        assert!(view.apical_ticks_draw().is_none());
        let frame_cs = view.render_rgba8(2).expect("render cell-state");
        assert_eq!(frame_cs.len(), 128 * 128 * 4);

        // 2. Polarity + Glow: no apical ticks (sphere mode only).
        view.set_style(Style {
            mapping: ColorMapping::Polarity,
            look: Look::Glow,
            particles: Particles::Points,
            field: true,
            field_mapping: FieldMapping::VascularConductance,
        });
        view.show(&epoch).expect("show polarity glow");
        assert!(view.apical_ticks_draw().is_none());

        // 3. Polarity + Sphere + Points: apical ticks emitted iff polarized > 0.
        view.set_style(Style {
            mapping: ColorMapping::Polarity,
            look: Look::Sphere,
            particles: Particles::Points,
            field: true,
            field_mapping: FieldMapping::VascularConductance,
        });
        view.show(&epoch).expect("show polarity sphere");
        assert_eq!(view.apical_ticks_draw().is_some(), polarized_count > 0);
        if polarized_count > 0 {
            saw_ticks = true;
            let draws = view.draws().expect("draws");
            // The field, the N dots' cloud and the bulk apical-ticks mesh (no
            // halos in Sphere look).
            assert!(matches!(
                draws.as_slice(),
                [
                    ThreeDDraw::Surface(_),
                    ThreeDDraw::DotCloud(dots),
                    ThreeDDraw::Surface(_),
                ] if dots.dots.len() == epoch.len()
            ));
            let frame_pol = view.render_rgba8(2).expect("render polarity sphere");
            assert_eq!(frame_pol.len(), 128 * 128 * 4);
            assert_ne!(frame_pol, frame_cs);

            // Resize, point radius, body radius, and zoomed placement keep ticks valid.
            if step == 0 {
                view.set_point_radius(2.0).expect("point radius");
                assert!(view.apical_ticks_draw().is_some());
                view.set_body_radius(0.4).expect("body radius");
                assert!(view.apical_ticks_draw().is_some());
                let zoomed = view
                    .viewport()
                    .zoomed_about([64.0, 64.0], 2.0)
                    .expect("zoom");
                view.set_view(zoomed).expect("set zoomed");
                view.place(&epoch).expect("place zoomed");
                let _ = view.render_rgba8(2).expect("render zoomed ticks");
                let reset = view.viewport().refit();
                view.set_view(reset).expect("reset");
            }
        }

        // 4. Polarity + Sphere + Density: no apical ticks.
        view.set_style(Style {
            mapping: ColorMapping::Polarity,
            look: Look::Sphere,
            particles: Particles::Density,
            field: true,
            field_mapping: FieldMapping::Energy,
        });
        view.show(&epoch).expect("show polarity sphere density");
        assert!(view.apical_ticks_draw().is_none());

        assert_eq!(
            observed.digest(),
            headless.digest(),
            "INV-09 violated at v2 tick {step}"
        );
    }
    assert!(
        saw_ticks,
        "all-v2 tissue world must exercise non-empty apical orientation ticks"
    );

    // Unified ColorMapping/FieldMapping legends match v2 primitives & stay within FL-024 cap.
    assert_eq!(ColorMapping::CellState.legend(&[]), cell_state_legend());
    assert_eq!(ColorMapping::Polarity.legend(&[]), polarity_legend());
    assert_eq!(
        FieldMapping::TrailCarrier.legend(),
        Some(FieldOverlayKind::TrailCarrier.legend())
    );
    assert_eq!(
        FieldMapping::VascularConductance.legend(),
        Some(FieldOverlayKind::VascularConductance.legend())
    );
    let font = HudFont::bundled().expect("bundled font");
    for legend in [
        ColorMapping::CellState.legend(&[]),
        ColorMapping::Polarity.legend(&[]),
        FieldMapping::TrailCarrier.legend().expect("trail legend"),
        FieldMapping::VascularConductance
            .legend()
            .expect("vasc legend"),
    ] {
        let lin_swatches: Vec<LinearRgba> = legend.swatches.iter().copied().map(linear).collect();
        let laid = font
            .text_with_swatches(&legend.text, 26.0, &lin_swatches)
            .expect("hud layout");
        assert_eq!(laid.refused(), 0);
        assert_eq!(laid.left_out(), 0);
        assert!(laid.vertices().len() <= 33_792);
    }
}

/// V2-16 zero-regression guarantee (`INV-09`, `INV-15`): on `field-ecology-v1`,
/// `--v1-compat`, and `--disable v2-visual-mappings`, `WorldView` reserves zero
/// extra bytes, emits zero apical-tick draws, keeps `scene_objects() == 5`, and
/// renders cleanly even if a user selects a v2 `ColorMapping` or `FieldMapping`.
#[test]
fn v1_and_disabled_v2_visual_mappings_incur_zero_overhead_and_fallback_cleanly() {
    let v1_cfg = common::config(&[
        ("world", "width", "32.0"),
        ("world", "height", "32.0"),
        ("world", "grid_width", "32"),
        ("world", "grid_height", "32"),
        ("population", "initial_count", "48"),
        ("population", "maximum_count", "72"),
        ("population", "initial_energy", "3.0"),
        ("execution", "command_durability", "\"volatile-in-memory\""),
    ]);
    let v1_world = common::found(&v1_cfg);
    let v1_epoch = common::epoch(&v1_world);
    let v1_view = WorldView::for_epoch(&v1_epoch, [128, 128], [0, 0, 0]).expect("v1 view");
    let baseline = (
        v1_view.reserved_bytes(),
        v1_view.preparation_limit(),
        v1_view.frame_bytes(),
        v1_view.scene_objects(),
    );
    assert_eq!(baseline.3, 5);

    let disabled_cases = [
        (
            "v1-compat",
            common::small_v2(false, &[FeatureOverride::V1Compat]),
        ),
        (
            "all-v2-minus-visual-mappings",
            common::small_v2(
                true,
                &[
                    FeatureOverride::Enable("all-v2".to_owned()),
                    FeatureOverride::Disable("v2-visual-mappings".to_owned()),
                ],
            ),
        ),
    ];
    for (label, cfg) in disabled_cases {
        let mut world = common::found(&cfg);
        common::advance(&mut world, 6);
        let epoch = common::epoch(&world);
        assert!(epoch.cell_states().is_none(), "{label}: cell_states");
        assert!(epoch.polarities().is_none(), "{label}: polarities");
        assert!(
            epoch.field().trail_energy().is_none(),
            "{label}: trail_energy"
        );
        assert!(
            epoch.field().vascular_conductance().is_none(),
            "{label}: vascular_conductance"
        );

        let mut view = WorldView::for_epoch(&epoch, [128, 128], [0, 0, 0]).expect("view");
        assert_eq!(
            (
                view.reserved_bytes(),
                view.preparation_limit(),
                view.frame_bytes(),
                view.scene_objects(),
            ),
            baseline,
            "{label}: zero extra WorldView memory or scene objects when disabled"
        );

        // Selecting Polarity + Sphere + TrailCarrier when disabled falls back cleanly with 0 ticks.
        view.set_style(Style {
            mapping: ColorMapping::Polarity,
            look: Look::Sphere,
            particles: Particles::Points,
            field: true,
            field_mapping: FieldMapping::TrailCarrier,
        });
        view.show(&epoch).expect("fallback show");
        assert!(view.apical_ticks_draw().is_none(), "{label}: no ticks");
        assert!(
            matches!(
                view.draws().expect("draws").as_slice(),
                [ThreeDDraw::Surface(_), ThreeDDraw::DotCloud(dots)]
                    if dots.dots.len() == epoch.len()
            ),
            "{label}: field + dots only"
        );
        assert_eq!(
            view.reserved_bytes(),
            baseline.0,
            "{label}: reserved_bytes unchanged after show"
        );
        let rgba = view.render_rgba8(2).expect("fallback render");
        assert_eq!(rgba.len(), 128 * 128 * 4);
    }
}
