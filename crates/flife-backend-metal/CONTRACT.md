# flife-backend-metal contract

**Purpose.** FL-030 (`flife-w4u`), first part: every due organism's own dense controller evaluated in one fused Metal
dispatch, bit-identical to `flife_neural::ReferenceController` (plan §10.2, §10.3, §10.7). It goes through
FrankenTorch's generic compute gateway (`ft_kernel_metal::compute`), compiled in the gateway's IEEE-safe math mode.
This crate is `forbid(unsafe_code)`; the Metal FFI is upstream.

**Inputs.**
- `Batch::push(topology, parameters, inputs)`: one organism's catalog topology, its parameters in the reference
  layout, and its observation's 104 F64 inputs. Organisms of different topologies share a batch.
- `MetalControllers::open()`, then `upload(&batch)` and `run(&resident, &mut outputs)`, or `evaluate` for both.

**Outputs.** One entry per organism, in batch order: its 36 raw outputs, or `None` for a deferred organism. The
caller evaluates a deferred organism on the reference controller.

**Invariants.**
- Every non-deferred output is the reference controller's, bit for bit:
  - inputs rounded once to F32 on the host, as the reference rounds them;
  - each neuron accumulates in F32, bias first and then the inputs ascending, with every product and sum rounded
    and no fused multiply-add (`#pragma clang fp contract(off)` and the safe math mode);
  - the hidden activation is `numerics::tanh(f64::from(z)) as f32`. It runs operation for operation on a software
    IEEE binary64 (Berkeley SoftFloat 3's add, subtract, multiply and pack routines, a restoring long division,
    exact conversions), because Apple GPUs have no F64.
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
- `tests/arithmetic.rs` (4 tests):
  - The device's F32 product and sum equal the CPU's on 1M pairs (random patterns, signed zeros, cancellation,
    products near 2^-102) wherever the operands and product are zero or at least 2^-102. Outside that range the
    device does flush: the test requires at least one flushed result, so the deferral is shown to be necessary.
  - The software binary64's add, sub, mul, div, round and F32 rounding equal the CPU's on 262,144 finite pairs:
    random, cancelling, subnormal, unit-range and exact F32 midpoints.
  - **Exhaustive:** the activation equals the reference for every positive F32 below 20, where the reference
    saturates. That is all 1,101,004,800 patterns, zero and subnormals included, in about 4 s.
  - Negative inputs (64 random windows of 65,536) and the saturated region equal the reference.
- `tests/forward.rs` (2 tests):
  - 2,000 organisms in one batch, over all five catalog topologies and six regimes: typical, saturating, small,
    subnormal, hidden overflow and output overflow. Every output is bit-identical to the reference, and the batch
    defers exactly where it should:
    - each reference error is a deferral;
    - only the subnormal regime defers a finite result;
    - both errors and the deferral occur.
  - Refusals match the reference, and leave the batch unchanged.

Planted mutants, each run in a scratch copy:
- Caught: binary64 ties away from even; F32 rounding ties away from even; the activation's sign dropped; fused
  multiply-add accumulation; inputs accumulated in reverse; either deferral disabled.
- Survived, as equivalent on the whole domain: one term fewer in either series. The exhaustive test proves the
  rounded activation unchanged for every input, because the dropped term is below what F32 rounding resolves.

**Throughput.** `examples/forward_throughput.rs` prints one JSON line comparing one dispatch with the reference on one
thread and on every core, outputs compared bit for bit. Measured 2026-11-09 on an M5 Ultra while other workloads
held the GPU at 45–60% and the CPU at load 40–55, so the figures are indicative. For 5,000 `[64, 32]` organisms:
- A dispatch on freshly uploaded buffers takes 15–27 ms, dominated by first-touch mapping. Upload itself takes 21–35
  ms.
- A warm dispatch on resident buffers takes 4–6 ms. The 36-core reference takes 4–5 ms (24 ms in one loaded run),
  and one core 46–53 ms.
- With the activation replaced by the identity (timing only), a warm dispatch is 0.6–0.7 ms. The activation's
  software binary64 is about 88% of the kernel. Coalescing the weight reads changed nothing measurable.

**Validity domain.** Apple GPUs through the gateway's safe math mode; the dense-104-36-v1 catalog's shapes.

**Known gaps.**
- The engine does not call this yet: wiring the batch into the tick (`decide_all`) behind a switch, with a
  whole-world digest equal to the CPU run's, is the next step.
- Buffers are created per batch. A resident parameter arena needs an in-place buffer write in FrankenTorch's gateway,
  which it lacks (FL-029).
- The activation's next lever is a float-float fast path with a guard against F32 rounding midpoints, falling back
  to the software binary64. It would be proven exhaustively over the same domain.
- No CUDA backend exists: FrankenTorch has no CUDA crate, and no host here has an NVIDIA GPU.
