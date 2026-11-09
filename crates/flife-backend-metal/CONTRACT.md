# flife-backend-metal contract

**Purpose.** FL-030 (`flife-w4u`), first part: every due organism's own dense controller evaluated in one fused Metal
dispatch, bit-identical to `flife_neural::ReferenceController` (plan §10.2, §10.3, §10.7). It goes through
FrankenTorch's generic compute gateway (`ft_kernel_metal::compute`), compiled in the gateway's IEEE-safe math mode.
This crate is `forbid(unsafe_code)`; the Metal FFI is upstream.

**Inputs.**
- The engine path: `MetalBackend::open()`, given to a world with `World::set_controller_backend`. It implements
  `flife_neural::ControllerBackend`, the port through which the world hands each tick's due evaluations over in
  one batch (`ControllerJob`: genome identity, admitted controller, inputs). `ControllerChoice` (`cpu`, `metal`,
  `auto`) is what the CLI and Studio select it with: `backend(cpu_workers)` gives `auto` Metal only on a host with
  a device and at most `AUTO_METAL_MAX_WORKERS` = 8 workers, where it measured faster (whole ticks, 256 ticks, two
  interleaved runs: 2 workers 27.5 → 16.3 ms, 4 workers 19.5 → 14.8 ms, 8 workers 15.8 → 14.3 ms). Test
  `tests/world.rs::auto_takes_metal_only_up_to_its_worker_ceiling`.
- The kernel alone: `Batch::push(topology, parameters, inputs)` for one organism's catalog topology, its parameters
  in the reference layout and its observation's 104 F64 inputs (topologies may mix in a batch); then
  `MetalControllers::open()`, `upload(&batch)` and `run(&resident, &mut outputs)`, or `evaluate` for both.

**Outputs.** One entry per organism, in batch order: its 36 raw outputs, or `None` for a deferred organism. The
caller evaluates a deferred organism on the reference controller.

**Residency.** `MetalBackend` keeps every buffer on the device across ticks. A genome's parameters are copied once
and found again by genome identity. A hit is used only if the slot was copied from the same memory (pointer and
length): genome parameters are immutable, so that identifies them even if a world were rewound. Each tick writes
only the inputs and job words in place (`SharedBuffer::write_f32/write_u32`, FrankenTorch `1c4febfb`). When the
parameter buffer fills, it is rebuilt at 1.5 times the current batch's need, which drops dead genomes. A job the
kernel cannot shape, or whose inputs are not finite in F32, is deferred without dispatch.

**Invariants.**
- Every non-deferred output is the reference controller's, bit for bit:
  - inputs rounded once to F32 on the host, as the reference rounds them;
  - each neuron accumulates in F32, bias first and then the inputs ascending, with every product and sum rounded
    and no fused multiply-add (`#pragma clang fp contract(off)` and the safe math mode);
  - the hidden activation is `numerics::tanh(f64::from(z)) as f32`.
- The activation is computed two ways.
  - **Software binary64, the exact path.** It runs the reference algorithm operation for operation, because Apple
    GPUs have no F64. It uses Berkeley SoftFloat 3's add, subtract, multiply and pack routines, a restoring long
    division, and exact conversions.
  - **Float-float, the fast path.** It uses hardware F32 with two-sum and FMA two-product, about 48 bits, in three
    regimes:
    - below 2^-13, the identity;
    - below 1/2, tanh's odd Taylor series (16 terms);
    - below 20, `1 - 2e/(1+e)` with `e = exp(-2a)`, after a three-part ln 2 reduction and a 14-term series.

    A fast result is kept only if every value within 2^-38 of it, relative, rounds to the same F32; otherwise the
    lane falls back to the exact path. The reference's F64 value lies within about 2^-51 of tanh, and the fast value
    within about 2^-44, so a kept result is the reference's.
- This device flushes subnormal F32 results to zero, and the reference does not. A sum of two normal F32s of
  magnitude at least 2^-102 is a multiple of 2^-125, so it is zero or normal, and a product at least 2^-102 is
  normal. A step that could produce a subnormal therefore has a nonzero operand or product below 2^-102. The kernel
  defers any organism with such a step, and any whose pre-activation or output is not finite.
- `Batch::push` refuses a non-finite F32 input with the reference's own error, and parameters that do not fill the
  topology. It also refuses a topology outside the kernel's shape: one or two hidden layers, each at most 128 wide.
  The batch is unchanged on refusal.

**Refusal cases.** No Metal device, which is always the case off macOS: `MissingCapability`. A kernel that does not
compile, or a failed dispatch: `StateConflict`. An empty batch to `upload`: `LayoutMismatch`.

**Independent tests.**
- `tests/arithmetic.rs` (5 tests):
  - The device's F32 product and sum equal the CPU's on 1M pairs (random patterns, signed zeros, cancellation,
    products near 2^-102) wherever the operands and product are zero or at least 2^-102. Outside that range the
    device does flush: the test requires at least one flushed result. Its `fma(a, b, -a*b)` equals the CPU's
    `mul_add` on the 310,054 pairs whose product is at least 2^-70, so two-product is exact there.
  - The software binary64's add, sub, mul, div, round and F32 rounding equal the CPU's on 262,144 finite pairs:
    random, cancelling, subnormal, unit-range and exact F32 midpoints.
  - **Exhaustive, twice:** both the forward pass's activation (fast path with fallback) and the software binary64
    alone equal the reference for every positive F32 below 20, where the reference saturates. That is all
    1,101,004,800 patterns, zero and subnormals included, in about 3 s each. The fast path must fall back at least
    once and fewer than 1,000,000 times, so the fallback is exercised and the fast path carries the load.
  - Negative inputs (64 random windows of 65,536) and the saturated region equal the reference.
- `tests/world.rs` (3 tests), the engine path. The real preset (4,096 founders, `[64, 32]`, every organism
  evaluated every tick) runs 400 ticks with its controllers on Metal next to the reference world. The digests
  match at every tick, through the founder crash, 42 births and mutation. All 1,254,756 evaluations came from the
  device (none deferred, no tick redone on the reference). The planted negative flips one output bit of one
  organism at tick 9 inside a wrapping backend, and the digests must differ at exactly tick 9. Off macOS the test
  checks the typed `MissingCapability` refusal instead.
- `tests/forward.rs` (2 tests):
  - 2,000 organisms in one batch, over all five catalog topologies and six regimes: typical, saturating, small,
    subnormal, hidden overflow and output overflow. Every output is bit-identical to the reference, and the batch
    defers exactly where it should:
    - each reference error is a deferral;
    - only the subnormal regime defers a finite result;
    - both errors and the deferral occur.
  - Refusals match the reference, and leave the batch unchanged.

Planted mutants, each run in a scratch copy.
- Caught:
  - binary64 or F32 ties away from even;
  - the activation's sign dropped;
  - fused multiply-add accumulation;
  - inputs accumulated in reverse;
  - either deferral disabled;
  - the fast path's guard disabled;
  - two-product without the FMA.
- Survived, and each is equivalent on the whole domain (the exhaustive test proves the rounded activation unchanged
  at every input):
  - one term fewer in either software series, or in either fast series;
  - the guard's 2^-38 margin set to zero;
  - its power-of-two gap ignored;
  - the identity threshold doubled;
  - the third part of ln 2 dropped.

  These are accuracy to spare on this domain; the margin is kept deliberately.

**Throughput.** `examples/forward_throughput.rs` prints one JSON line. It compares the first and the warm (resident
buffers) dispatch with the reference on one thread and on every core, outputs compared bit for bit. One run,
2026-11-09 on an M5 Ultra, with other workloads holding the GPU at 45–60% and the CPU at load 49; the figures are
indicative (an earlier run on the same tree had `[8]` warm at 0.54 ms). 5,000 organisms, every output identical, none
deferred:

| Topology | Metal (warm) | 36 CPU threads | 1 CPU thread | First dispatch | Upload |
|---|---|---|---|---|---|
| `[8]` | 1.04 ms | 1.39 ms | 7.0 ms | 2.3 ms | 3.5 ms |
| `[32]` | 0.81 ms | 2.82 ms | 22.2 ms | 8.0 ms | 13.4 ms |
| `[64, 32]` | 0.98 ms | 4.77 ms | 47.8 ms | 11.6 ms | 26.1 ms |
| `[96, 64]` | 1.28 ms | 6.84 ms | 86.6 ms | 25.0 ms | 47.3 ms |
| `[128, 96]` | 1.78 ms | 9.46 ms | 137.4 ms | 32.6 ms | 66.3 ms |

With the software path alone the `[64, 32]` warm dispatch took 4–6 ms; the activation was 88% of it. The first
dispatch over fresh buffers pays page mapping, and the upload is the per-call buffer creation, so a per-tick
`evaluate` would lose to the CPU. The engine path must keep buffers resident; FrankenTorch `1c4febfb` adds the
in-place `SharedBuffer::write_f32/write_u32` for that.

**Validity domain.** Apple GPUs through the gateway's safe math mode; the dense-104-36-v1 catalog's shapes.

Whole ticks (`flife run`, preset, 512 ticks, median of three interleaved runs, cv ≤ 2.5%, same digest in all 12
runs): one CPU worker 27.7 ms per tick with the reference and 12.5 ms with Metal (41.2 and 17.8 ms over the first
128 ticks, with all founders alive); 16 workers 9.5 and 9.6 ms. On 16 workers the main thread spends 19% in phases 3
and 4 (10% waiting on `MetalBackend::evaluate`) and most of the rest in serial phases. Those are chemistry
composition (14%), the field step (11%), uptake, deposits, grants and invariant checks (each about 5%), and
per-pass thread spawning (6%).

**Known gaps.**
- Only phases 3 and 4's controller evaluation runs on the GPU; observation encoding (about 69 `tanh` per organism),
  decoding, chemistry, uptake and the field step stay on the CPU (FL-032 moves them). On many CPU workers the tick
  is bound by those, so Metal does not shorten it there.
- FL-030's closure also needs FL-026's packed genome arenas and FL-029's batched submission upstream, and its
  resident-bandwidth benchmark against §19.2's weight-stream bound.
- No CUDA backend exists: FrankenTorch has no CUDA crate, and no host here has an NVIDIA GPU.
