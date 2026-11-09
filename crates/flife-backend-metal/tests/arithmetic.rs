//! The kernel's arithmetic against the CPU's: the device's own F32 product
//! and sum, the software binary64, and the activation over every positive F32
//! below tanh's saturation point.

use std::thread;

use flife_backend_metal::{BINARY64_RESULT_WORDS, MetalControllers};
use flife_core::CoreError;
use flife_core::numerics::tanh;

/// The controllers on macOS. Elsewhere, `None` after checking that opening
/// is refused as a missing capability.
#[expect(
    clippy::panic,
    reason = "test helper: a failure here is a test failure"
)]
fn controllers() -> Option<MetalControllers> {
    match MetalControllers::open() {
        Ok(controllers) => Some(controllers),
        Err(CoreError::MissingCapability { .. }) if !cfg!(target_os = "macos") => None,
        Err(error) => panic!("Metal controllers did not open: {error}"),
    }
}

/// SplitMix64.
struct Draws(u64);

impl Draws {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

fn same_f32(a: f32, b: f32) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

fn same_f64(a: u64, b: f64) -> bool {
    a == b.to_bits() || (f64::from_bits(a).is_nan() && b.is_nan())
}

/// The device's F32 product and sum are the CPU's wherever the operands
/// and the product are zero or at least 2^-102 in magnitude, the range the
/// kernel computes in. Signed zeros included.
#[test]
fn the_device_rounds_f32_products_and_sums_as_the_cpu_does_above_the_tiny_range()
-> Result<(), CoreError> {
    let Some(controllers) = controllers() else {
        return Ok(());
    };
    let mut draws = Draws(7);
    let mut pairs = Vec::new();
    for index in 0..(1 << 20) {
        let bits = draws.next();
        let (a, b) = (bits as u32, (bits >> 32) as u32);
        // Random patterns, signed zeros, operands near each other
        // (cancellation), and products near 2^-102.
        let pair = match index % 5 {
            0 => [
                f32::from_bits(a & 0x8000_0000),
                f32::from_bits(b & 0x8000_0000),
            ],
            1 => [f32::from_bits(a), f32::from_bits(a ^ (b & 0xFF))],
            2 => [
                f32::from_bits(0x1A00_0000 | (a & 0x81FF_FFFF)),
                f32::from_bits(0x1A00_0000 | (b & 0x81FF_FFFF)),
            ],
            _ => [f32::from_bits(a), f32::from_bits(b)],
        };
        pairs.push(pair);
    }
    let tiny = |value: f32| value != 0.0 && value.abs() < 2.0_f32.powi(-102);
    let mut results = Vec::new();
    controllers.binary32_ops(&pairs, &mut results)?;
    let mut checked = 0;
    let mut flushed = 0;
    let mut fused = 0;
    for (index, ([a, b], [product, sum, error])) in pairs.iter().zip(&results).enumerate() {
        if !a.is_finite()
            || !b.is_finite()
            || tiny(*a)
            || tiny(*b)
            || tiny(a * b)
            || a * b == 0.0 && *a != 0.0 && *b != 0.0
        {
            // Outside the kernel's range: the kernel defers these. Count the
            // ones the device actually flushes, which is why it defers.
            flushed += usize::from(!same_f32(*product, a * b) || !same_f32(*sum, a + b));
            continue;
        }
        checked += 1;
        assert!(
            same_f32(*product, a * b),
            "pair {index}: {a:e} x {b:e} gave {product:e}, not {:e}",
            a * b
        );
        assert!(
            same_f32(*sum, a + b),
            "pair {index}: {a:e} + {b:e} gave {sum:e}, not {:e}",
            a + b
        );
        // The fast path's exact products: the fused multiply-add rounds once,
        // so it gives the product's rounding error, which is normal when the
        // product is at least 2^-70.
        if (a * b).is_finite() && (a * b).abs() >= 2.0_f32.powi(-70) {
            let want = a.mul_add(*b, -(a * b));
            assert!(
                same_f32(*error, want),
                "pair {index}: fma({a:e}, {b:e}, -p) gave {error:e}, not {want:e}"
            );
            fused += 1;
        }
    }
    assert!(checked > 600_000, "only {checked} pairs in range");
    assert!(fused > 250_000, "only {fused} fused multiply-adds checked");
    // The deferral is needed: the device does flush.
    assert!(flushed > 0, "the device flushed no subnormal result");
    Ok(())
}

#[test]
fn the_software_binary64_is_ieee_round_to_nearest_even() -> Result<(), CoreError> {
    let Some(controllers) = controllers() else {
        return Ok(());
    };
    let mut draws = Draws(42);
    let mut pairs = Vec::new();
    for index in 0..(1 << 18) {
        let (a, b) = (draws.next(), draws.next());
        // Finite operands: full random patterns, operands near each other
        // (cancellation), small exponents (subnormals), and values in the
        // activation's own range.
        let finite = |bits: u64| {
            if (bits >> 52) & 0x7FF == 0x7FF {
                bits & !(1 << 62)
            } else {
                bits
            }
        };
        let pair = match index % 6 {
            // Exact F32 midpoints in `a`, for the F32 rounding's ties; both
            // parities of the kept bit occur.
            5 => [
                finite((a & !0x1FFF_FFFF) | 0x1000_0000),
                finite((b & !0x1FFF_FFFF) | 0x1000_0000 | (index as u64 & 1) << 29),
            ],
            0 => [finite(a), finite(b)],
            1 => [finite(a), finite(a) ^ (b & 0xFFFF)],
            2 => [a & 0x803F_FFFF_FFFF_FFFF, b & 0x803F_FFFF_FFFF_FFFF],
            3 => [
                f64::from_bits(0x3FF0_0000_0000_0000 | (a & 0xF_FFFF_FFFF_FFFF)).to_bits()
                    ^ (a & (1 << 63)),
                (f64::from_bits(0x3F80_0000_0000_0000 | (b & 0x3F_FFFF_FFFF_FFFF))).to_bits(),
            ],
            _ => [
                (f64::from(index as u32) * 0.5).to_bits() ^ (a & (1 << 63)),
                finite(b),
            ],
        };
        pairs.push(pair);
    }
    let mut results = Vec::new();
    controllers.binary64_ops(&pairs, &mut results)?;
    for (index, (&[a, b], row)) in pairs.iter().zip(&results).enumerate() {
        let (x, y) = (f64::from_bits(a), f64::from_bits(b));
        let word = |k: usize| u64::from(row[2 * k]) | (u64::from(row[2 * k + 1]) << 32);
        let expected = [x + y, x - y, x * y, x / y, x.round()];
        for (k, (name, want)) in ["add", "sub", "mul", "div", "round"]
            .iter()
            .zip(expected)
            .enumerate()
        {
            assert!(
                same_f64(word(k), want),
                "pair {index}: {name}({x:e}, {y:e}) gave {:e}, not {want:e}",
                f64::from_bits(word(k))
            );
        }
        assert_eq!(row.len(), BINARY64_RESULT_WORDS);
        assert!(
            same_f32(f32::from_bits(row[10]), x as f32),
            "pair {index}: {x:e} as f32"
        );
        assert!(
            same_f32(f32::from_bits(row[11]), y as f32),
            "pair {index}: {y:e} as f32"
        );
    }
    Ok(())
}

/// The reference activation of the F32 with bits `bits`.
fn reference(bits: u32) -> u32 {
    tanh(f64::from(f32::from_bits(bits))).map_or(u32::MAX, |value| (value as f32).to_bits())
}

/// Compare the kernel's activation with the reference on `count` patterns
/// from `base`, the reference computed on all cores.
#[expect(
    clippy::panic,
    reason = "test helper: a failure here is a test failure"
)]
fn compare(
    controllers: &MetalControllers,
    base: u32,
    count: usize,
    software_only: bool,
    out: &mut [u32],
) -> Result<u32, CoreError> {
    let fallbacks = controllers.activation_bits(base, count, software_only, out)?;
    let threads = thread::available_parallelism().map_or(8, usize::from);
    let length = count.div_ceil(threads);
    thread::scope(|scope| {
        for (chunk, values) in out.chunks(length).enumerate() {
            scope.spawn(move || {
                for (offset, &got) in values.iter().enumerate() {
                    let bits = base + (chunk * length + offset) as u32;
                    let want = reference(bits);
                    if got != want {
                        panic!(
                            "activation of {:e} ({bits:#010x}) gave {:e}, not {:e}",
                            f32::from_bits(bits),
                            f32::from_bits(got),
                            f32::from_bits(want)
                        );
                    }
                }
            });
        }
    });
    Ok(fallbacks)
}

/// Every positive F32 below 20, where the reference stops saturating, and
/// zero and the subnormals with them: 1,101,004,800 patterns. Returns the
/// fallbacks to the software binary64.
fn every_positive_f32_below_saturation(software_only: bool) -> Result<u64, CoreError> {
    let Some(controllers) = controllers() else {
        return Ok(0);
    };
    let saturation = 20.0_f32.to_bits();
    const CHUNK: u32 = 1 << 24;
    let mut out = vec![0_u32; CHUNK as usize];
    let mut base = 0_u32;
    let mut fallbacks = 0_u64;
    while base < saturation {
        let count = CHUNK.min(saturation - base);
        fallbacks += u64::from(compare(
            &controllers,
            base,
            count as usize,
            software_only,
            &mut out[..count as usize],
        )?);
        base += count;
    }
    Ok(fallbacks)
}

/// The forward pass's activation, fast path and fallback together.
#[test]
fn the_activation_is_the_reference_tanh_for_every_positive_f32_below_saturation()
-> Result<(), CoreError> {
    let fallbacks = every_positive_f32_below_saturation(false)?;
    if cfg!(target_os = "macos") {
        // The guard does refuse some values, so the fallback is exercised,
        // and the fast path settles nearly all of them.
        assert!(
            (1..1_000_000).contains(&fallbacks),
            "{fallbacks} fallbacks in 1,101,004,800"
        );
    }
    Ok(())
}

/// The software binary64 alone, which the fast path falls back to.
#[test]
fn the_software_activation_is_the_reference_tanh_for_every_positive_f32_below_saturation()
-> Result<(), CoreError> {
    every_positive_f32_below_saturation(true).map(|_| ())
}

/// The sign is the reference's: negatives sampled across their whole range,
/// and the saturated region above 20.
#[test]
fn the_activation_is_odd_and_saturates_as_the_reference_does() -> Result<(), CoreError> {
    let Some(controllers) = controllers() else {
        return Ok(());
    };
    let mut out = vec![0_u32; 1 << 16];
    let mut draws = Draws(99);
    for _ in 0..64 {
        // A random 2^16-pattern window of negatives below -0 and above -inf,
        // and one of positives from 20 to infinity.
        let negative = 0x8000_0000 + (draws.next() as u32 % (0x7F80_0000 - (1 << 16)));
        compare(&controllers, negative & !0xFF, 1 << 16, false, &mut out)?;
        let large = 20.0_f32.to_bits()
            + (draws.next() as u32 % (0x7F80_0000 - 20.0_f32.to_bits() - (1 << 16)));
        compare(&controllers, large & !0xFF, 1 << 16, false, &mut out)?;
    }
    Ok(())
}
