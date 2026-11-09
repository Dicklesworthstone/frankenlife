# flife-backend-metal contract

**Purpose.** FL-030 (`flife-w4u`), first part: every due organism's own dense controller evaluated in one fused Metal
dispatch, bit-identical to `flife_neural::ReferenceController` (plan §10.2, §10.3, §10.7). It goes through
FrankenTorch's generic compute gateway (`ft_kernel_metal::compute`), compiled in the gateway's IEEE-safe math mode.
This crate is `forbid(unsafe_code)`; the Metal FFI is upstream.

FL-028 (`flife-xbm`) adds the Metal owner, typed buffers and safe leases (plan §9.5, §10.3, §12.4), in `owner`. One
`DeviceOwner` holds every device buffer the crate uses, with its state, epoch and users. Every command binds its
buffers through it. Readers and writers hold typed leases, and a buffer in use is never freed. `MetalControllers`
and `MetalBackend` now allocate, write, dispatch and read back only through their owner.

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
- FL-028, the owner: `DeviceOwner::open(Limits { bytes, leases_per_purpose })` (`Limits::UNBOUNDED` for the
  controllers today). It is cheap to clone and usable from any thread. It provides:
  - buffers: `zeroed::<T>(len)` and `upload(&[T])` for `T` = `f32` or `u32`, and `zeroed_owned`/`upload_owned`
    for buffers retired when dropped;
  - leases: `write(buffer)` and `read(buffer, Purpose)`, where a purpose is `Rendering`, `Checkpointing` or
    `Readback`;
  - commands: `submit(&pipeline, &[buffer.reads(), buffer.writes(), …], grid)`;
  - `retire(buffer)`, `state`, `epoch`, `elements` and `usage`.

**Outputs.** One entry per organism, in batch order: its 36 raw outputs, or `None` for a deferred organism. The
caller evaluates a deferred organism on the reference controller.

FL-028's outputs:
- `Buffer<T>`, a typed handle that only names a buffer: its owner, slot and generation.
- `ReadLease<T>`, which reads, gives the epoch it reads, and is released when dropped.
- `WriteLease<T>`, which writes in place and is released when dropped.
- `Receipt`, the epochs a completed command wrote.
- `ResourceState`: `Available`, `Computing`, `Committed`, `Rendering`, `Checkpointing` or `Retiring` (§9.5).
- `Usage`: buffers, bytes, retiring, read leases by purpose, write leases and running commands.

**Residency.** `MetalBackend` keeps every buffer on the device across ticks. A genome's parameters are copied once
and found again by genome identity. A hit is used only if the slot was copied from the same memory (pointer and
length): genome parameters are immutable, so that identifies them even if a world were rewound. Its five resident buffers are
`Owned` by its `DeviceOwner`, and a rebuilt buffer retires the one it replaces. Each tick writes
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

**Invariants — the owner (FL-028; §9.5).**
- A buffer has any number of readers or one writer, never both:
  - readers are read leases and the commands that read it;
  - the writer is a write lease or the command that writes it;
  - a buffer bound to one command twice, once written, is an alias and is refused (reading it twice is not).
- The order of a submission:
  1. Every binding is checked before any is marked, so a refused submission binds nothing.
  2. The bindings are marked, and the table's lock is released while the gateway dispatches and waits.
  3. Once the command buffer's status is known, the bindings are released.

  So for the whole run, every other thread sees the bound buffers `Computing`, and is refused incompatible access.
- Outcomes:
  - a completed command makes each written buffer defined, at its next epoch;
  - a command that failed after commit (the gateway's `CommandBuffer` error) makes each written buffer undefined,
    at its next epoch, and reads of it are refused until it is rewritten;
  - a command the gateway refused before encoding (a bad grid or threadgroup) changes no buffer.
- Epochs and leases:
  - an epoch counts writes: 0 for a zeroed buffer, 1 for an upload, and one more per completed write;
  - a read lease's epoch cannot change while the lease is held;
  - a released write lease commits what it wrote, at the next epoch.
- Retirement:
  - retiring refuses every new lease and binding at once;
  - the buffer is freed, and its bytes returned, when its last lease or command releases it;
  - a freed slot is reused under a new generation, so an old handle stays stale.
- Handles: a handle of another owner, or a stale one, is refused before anything is touched.
- Leases hold the owner's table alive, so a lease outlives every owner handle. A dropped lease is released, so a
  consumer that exits or unwinds frees what it held.
- Live bytes never exceed `Limits::bytes`, retiring buffers included, and each purpose holds at most
  `leases_per_purpose` read leases.

**Refusal cases.** No Metal device, which is always the case off macOS: `MissingCapability`. A kernel that does not
compile, or a failed dispatch: `StateConflict`. An empty batch to `upload`: `LayoutMismatch`. FL-028's owner refuses:
- a buffer of no elements: `LayoutMismatch`;
- one beyond the byte limit: `BudgetExhausted` ("metal owner bytes");
- one the device refuses: `AllocationFailed`;
- a read lease beyond its purpose's limit: `BudgetExhausted` ("metal read leases");
- `StateConflict` for: a stale or foreign handle; a retiring buffer; a writer beside a reader or another writer; a
  reader beside a writer; a read of undefined contents; an alias; and an out-of-range host read or write.

A refusal changes nothing.

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
- `tests/owner.rs` (FL-028, 5 tests, about 3 s), on this host's device with small kernels of its own:
  - `typed_buffers_hold_their_elements_within_the_owners_budget`:
    - `f32` and `u32` buffers round-trip, with their element counts, states and epochs;
    - a 64-byte budget refuses 40 bytes with 36 left, and a full owner refuses 4, each allocating nothing;
    - retiring a buffer returns its bytes;
    - no elements is refused, and so is a buffer above the device's largest, which allocates nothing.
  - `a_buffer_has_many_readers_or_one_writer`:
    - rendering and checkpointing leases share a buffer, refuse a writer, and let a command read it;
    - a third rendering lease is refused at a limit of two, and admitted after one is dropped;
    - a write lease refuses a second writer, a reader and a reading command;
    - an out-of-range write is refused;
    - releasing the writer advances the epoch, and a new reader sees its values.
  - `a_command_binds_only_what_it_may_and_a_refused_one_changes_nothing`:
    - a command's receipt and values, and one buffer read twice;
    - three aliases and a foreign buffer refused, with nothing bound or changed;
    - a threadgroup above the pipeline's maximum, refused by the gateway before encoding: the output keeps its
      epoch, state and values, and is writable afterwards.
  - `a_running_command_keeps_what_it_binds_from_every_other_thread`. One thread runs a 2^27-step xorshift kernel
    (about half a second). The test thread waits until it sees the output `Computing`, and then:
    - a running count of one;
    - a readback of the output refused;
    - host writes of both inputs refused;
    - a second command writing the output refused;
    - a rendering reader admitted beside the command;
    - retiring an input deferred, the input then `Retiring`.

    It then checks that the output was still `Computing` after all of that. When the command completes, the receipt
    names epoch 1, the retired input is freed, and the output equals the CPU's xorshift.
  - `retired_buffers_free_when_released_and_their_handles_stay_stale`:
    - a retiring buffer refuses leases, writers and bindings, still counts its bytes, and still reads through the
      lease it had;
    - dropping that lease frees it;
    - the slot comes back as generation 1, and every use of the old handle is refused;
    - an `Owned` buffer retires on drop;
    - another owner's handle is refused;
    - a lease outlives its owner handle;
    - a thread that panics holding a write lease releases it, committing its write at the next epoch.
- The owner's unit test, `a_command_that_failed_after_commit_leaves_its_outputs_undefined`, covers the
  post-commit failure. No device fault on this host fails a command after commit: GPU validation
  (`MTL_SHADER_VALIDATION=1`) still returned success for an out-of-bounds store. So the test settles real
  bindings with the gateway's own `CommandBuffer(Error)`. The written buffer becomes `Available` at epoch 2,
  refuses leases and command reads, and is defined again after a host rewrite. The read buffer is unchanged, and
  a refusal before encoding changes nothing.
- `tests/world.rs` adds the owner's account after the 400-tick preset run: exactly the backend's five buffers, with
  no lease, command or retiring buffer left. The arithmetic, forward and world tests ran unchanged on the ported
  crate, every output still bit-identical.

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

**Validity domain.** Apple GPUs through the gateway's safe math mode; the dense-104-36-v1 catalog's shapes. The
owner: `f32` and `u32` buffers, and the gateway's synchronous dispatch (one command buffer per submission).

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
- FL-028 gaps:
  - Submission is synchronous: the owner holds a command's bindings for exactly the gateway's dispatch-and-wait.
    Several ordered kernels in one command buffer, retained across submissions with completion receipts, are
    FL-029 upstream. The owner's bind/settle split is where an asynchronous completion will plug in.
  - The post-commit failure path is tested with the gateway's error value, not a real device fault (see the tests).
  - The controllers and backend open their owners `UNBOUNDED`. Declaring the backend's byte budget at preflight,
    and admitting observers' and checkpoints' peaks against it, is FL-043. Render and checkpoint consumers of
    device leases are FL-033 and FL-038; today's renderer reads the CPU world's epochs (FL-017).
  - Only `f32` and `u32` elements. Textures, heaps and private-storage buffers are not owned yet.
  - `ResourceState` reports the dominant use when several overlap, by precedence: retiring, computing,
    checkpointing, rendering, committed, available. `usage` gives the lease counts.
- No CUDA backend exists: FrankenTorch has no CUDA crate, and no host here has an NVIDIA GPU.
