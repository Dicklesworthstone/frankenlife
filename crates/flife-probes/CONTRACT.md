# flife-probes — contract

**Purpose.** FL-001 (`flife-nw7`): qualify the pinned FrankenSuite dependency closure before any product crate
builds on it (plan §12.8, §18.5, gate G0 Stack). Each probe calls the public API a later crate will use and checks
the result against an oracle that does not come from the code under test.

**Inputs.** The committed `Cargo.lock` and `franken-stack.lock` (embedded at build time), an output directory for
the Manim PNGs, and optionally a window poll count.

**Outputs.** One `ProbeReport` per probe: a verdict and the observed evidence. The `flife-probe` binary prints them
as NDJSON lines (`flife.probe.v1`) and exits 0 (no failure), 1 (a probe failed) or 2 (usage).

| Probe | Checks | Oracle |
|---|---|---|
| `closure` | Each suite git source resolves at its `franken-stack.lock` commit; no repo at two commits; one git-sourced copy of asupersync, the franken-* runtime crates, fmn, fnp-*, ft-* and `block` | `franken-stack.lock` |
| `runtime` | Asupersync current-thread runtime joins a spawned task; the lab replays a seeded schedule; FrankenTorch's `Budget` is this crate's `Budget` | Closed-form completion order `[0,5,4,3,2,1]`; the type check |
| `rng` | PCG64DXSM words, `random()`, `advance(1000)`, `SeedSequence` and spawn keys | NumPy 2.5.4 values |
| `array` | Reference-field `(128,128,33)` f32 C/F strides, bytes, broadcast | NumPy 2.5.4 values |
| `tensor` | FrankenTorch CPU `matmul` | Exact integer arithmetic |
| `manim_frame` | `fmn::render` of a seeded 256-dot `DotCloud`, certified mode, camera route, rendered twice | Byte-identical re-render, one published frame, and a digest that differs from an empty-scene control |
| `metal` | Gateway compiles MSL, dispatches 256 threads, reads back `2x+1` | Exact f32 arithmetic |
| `native_input` | The presenter's event surface | The pinned `PresentationState` enum, matched without a wildcard |
| `native_window` (opt-in) | A real window opens, is polled and closes | States observed |

**Invariants.** No probe reports `qualified` for work it did not do. A missing host capability is `unavailable`; a
capability missing upstream is `gap` with its bead; a wrong answer or an error is `failed`.

**Refusal cases.** A missing output directory or unknown argument is a usage error. A no-clobber render collision
is `failed`, never overwritten.

**Determinism.** Every probe is seeded with 42. The lab, RNG, array and tensor checks are bit-exact. The Manim digest
is certified-mode output, so it should match across the certified matrix. Only the macOS digest has been observed.

**Validity domain.** The commits in `franken-stack.lock` and the toolchain in `rust-toolchain.toml`. The Metal and
window probes need macOS with a GPU and, for the window, a window server and the main thread.

**Known gaps.** Native typed input (pointer, scroll, key, modifier, resize, focus) is absent from the pinned
presenter. It is tracked by FL-018 (`flife-jdf`) and blocks interactive Studio, not the headless stack. The window
probe is not run in `cargo test`, because test threads are not the main thread.
