# flife-backend-metal contract

**Purpose.** FL-030 (`flife-w4u`), first part: every due organism's own dense controller evaluated in one fused Metal
dispatch, bit-identical to `flife_neural::ReferenceController` (plan §10.2, §10.3, §10.7). It goes through
FrankenTorch's generic compute gateway (`ft_kernel_metal::compute`), compiled in the gateway's IEEE-safe math mode.
This crate is `forbid(unsafe_code)`; the Metal FFI is upstream.

FL-028 (`flife-xbm`) adds the Metal owner, typed buffers and safe leases (plan §9.5, §10.3, §12.4), in `owner`. One
`DeviceOwner` holds every device buffer the crate uses, with its state, epoch and users. Every command binds its
buffers through it. Readers and writers hold typed leases, and a buffer in use is never freed. `MetalControllers`
and `MetalBackend` now allocate, write, dispatch and read back only through their owner.

FL-029 (`flife-g81`) adds batched submission (plan §10.4, §22.4). `DeviceOwner::submit_batch` runs several ordered
kernels as one command: one command buffer and one serial compute encoder, committed once and waited for once, encoded
by FrankenTorch `71bbe52e`'s `compute::Batch`. The command holds every buffer it binds from its first kernel until its
last has verifiably completed: written buffers exclusively, read-only ones shared. Its receipt gives the epochs it
wrote, the kernels it ran and its device time. `submit` is now a batch of one kernel. While building this, FrankenTorch's
`Gateway::dispatch` turned out to have no autorelease pool: a thread that never exits kept every command buffer it
dispatched, about 1.8 KiB each (+368 MB over 200,000 dispatches on an M5 Ultra). The owner's single-kernel `submit`
went through it, so a long run with Metal controllers grew by that much per tick. `71bbe52e` fixes `dispatch`, and
the batch drains a pool per submission.

FL-031 (`flife-8le`), first part: phase 2 of the tick (transport, decay and the external sources) on Metal,
bit-identical to `flife_field::advance_field_on` (plan §8.1, §10.7, §11.1), in `field`. `MetalField` is a
`flife_field::FieldBackend`. The device does the per-cell work in `field.metal` on `softfloat64.metal`'s binary64:
every plane's substeps, every candidate cell's source weight (its `exp`) and every moment value's decay and
deposits. The host does every sequential F64 sum through flife-field's own functions. The software binary64 moved
from `controller.metal` into `softfloat64.metal`, which both kernels now share.

FL-031, second part: phase 3's sensor pyramid and sampling on Metal, bit-identical to
`flife_field::PyramidView::sample` (plan §6.1, §8.1, §10.7), in `sensors`. `MetalSensors` is a
`flife_field::SensorBackend`. The device builds every coarse level of `positive-box-v1` and samples every
organism's 33 planes, value and both derivatives, at its one or two levels, blended by `log-linear-v1`, in
`sensors.metal` on the same `softfloat64.metal`. The host gives each query its level, weight and corner cells
through flife-field's `SensorPyramid::blend` and `SensorPyramid::corners`.

**Inputs.**
- The engine path: `MetalBackend::open()`, given to a world with `World::set_controller_backend`. It implements
  `flife_neural::ControllerBackend`, the port through which the world hands each tick's due evaluations over in
  one batch (`ControllerJob`: genome identity, admitted controller, inputs). `ControllerChoice` (`cpu`, `torch`,
  `metal`, `auto`) is what the CLI and Studio select it with (`torch` is `flife-backend-cpu`'s `CpuBackend` on the
  world's workers, on any host): `backend(cpu_workers)` gives `auto` Metal only on a host with
  a device and at most `AUTO_METAL_MAX_WORKERS` = 8 workers, where it measured faster (whole ticks, 256 ticks, two
  interleaved runs: 2 workers 27.5 → 16.3 ms, 4 workers 19.5 → 14.8 ms, 8 workers 15.8 → 14.3 ms). Where it does
  not take Metal, it takes `torch` at most `AUTO_TORCH_MAX_WORKERS` = 4 workers, where that measured faster on
  Linux and macOS (`flife-backend-cpu`'s contract). `ControllerChoice::auto_for(cpu_workers, metal_device)` states
  the rule for either kind of host. Test `tests/world.rs::auto_takes_metal_only_up_to_its_worker_ceiling` checks the
  rule for both kinds on any host, and that this host's `auto` is the rule's choice for it.
- The kernel alone: `Batch::push(topology, parameters, inputs)` for one organism's catalog topology, its parameters
  in the reference layout and its observation's 104 F64 inputs (topologies may mix in a batch); then
  `MetalControllers::open()`, `upload(&batch)` and `run(&resident, &mut outputs)`, or `evaluate` for both.
- FL-028, the owner: `DeviceOwner::open(Limits { bytes, leases_per_purpose })` (`Limits::UNBOUNDED` for the
  controllers today). It is cheap to clone and usable from any thread. It provides:
  - buffers: `zeroed::<T>(len)` and `upload(&[T])` for `T` = `f32` or `u32`, and `zeroed_owned`/`upload_owned`
    for buffers retired when dropped;
  - leases: `write(buffer)` and `read(buffer, Purpose)`, where a purpose is `Rendering`, `Checkpointing` or
    `Readback`;
  - commands: `submit(&pipeline, &[buffer.reads(), buffer.writes(), …], grid)` for one kernel, and FL-029's
    `submit_batch(&[Kernel { pipeline, bindings, grid }, …])` for several in order;
  - `retire(buffer)`, `state`, `epoch`, `elements` and `usage`.

- FL-031, the field: `MetalField::open()` (or `with_owner(owner)`), given to a world with
  `World::set_field_backend`. Each step receives the field pair, transport plan, source kernel, the tick's
  sources and the time step, exactly what `advance_field_on` takes. No CLI or Studio choice selects it yet (see
  the known gaps).
- FL-031, the sensors: `MetalSensors::open()` (or `with_owner(owner)`, to share the field's owner), given to a
  world with `World::set_sensor_backend`. Each tick receives the pyramid (its grid and level count), the observed
  field and every live organism's `SensorQuery` (position, sensing scale), in row order. No CLI or Studio choice
  selects it yet either. `examples/tick_throughput.rs` measures it: a choice such as `cpu+sensors` or
  `metal+sensors+field` puts phase 3 (and phase 2) on Metal beside the controllers.

**Outputs.** One entry per organism, in batch order: its 36 raw outputs, or `None` for a deferred organism. The
caller evaluates a deferred organism on the reference controller.

FL-028's outputs:
- `Buffer<T>`, a typed handle that only names a buffer: its owner, slot and generation.
- `ReadLease<T>`, which reads, gives the epoch it reads, and is released when dropped.
- `WriteLease<T>`, which writes in place and is released when dropped.
- `Receipt`, for a completed command:
  - `written`, the epoch each buffer it wrote now holds, in order of each buffer's first binding;
  - `kernels`, the kernels it ran (FL-029);
  - `device_seconds`, its command buffer's time on the GPU, from start to end (FL-029).
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
  - a buffer bound to one kernel twice, once written, is an alias and is refused (reading it twice is not).
- A command is one or more kernels (FL-029), encoded in order into one command buffer, so each kernel reads what
  earlier ones wrote. Its bindings are checked and held as one:
  - a buffer any kernel writes needs no other reader or writer, and the command holds it exclusively;
  - a buffer the kernels only read needs no writer, and the command holds it shared;
  - one kernel may read a buffer that another, earlier or later, writes.
- The order of a submission:
  1. Every binding of every kernel is checked before any is marked, so a refused submission binds nothing.
  2. The bindings are marked, and the table's lock is released while the gateway encodes, commits and waits.
  3. Once the command buffer's status is known, the bindings are released.

  So for the whole run, every other thread sees the bound buffers `Computing`, and is refused incompatible access.
- Outcomes:
  - a completed command makes each written buffer defined, at its next epoch: one epoch per command, however many
    of its kernels wrote the buffer, since only the command's final contents are ever observable;
  - a command whose command buffer did not complete (the gateway's `CommandBuffer` error) makes each written
    buffer undefined, at its next epoch, and reads of it are refused until it is rewritten;
  - a command whose first binding of a buffer reads it needs defined contents, but one that writes an undefined
    buffer first may read it in a later kernel;
  - a command the gateway refused before encoding (a bad grid or threadgroup in any of its kernels) changes no
    buffer, and none of its kernels runs.
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

**Invariants — the field (FL-031; §8.1, §10.7).**
- The candidate field and every F64 term of the step's report are `advance_field_on`'s, bit for bit.
- Field values cross to the device and back as F32 bit patterns and are never held in a device `float`. This
  device flushes subnormal F32 arithmetic to zero, and these bits keep them.
- The device computes the reference's per-cell expressions operation for operation in software binary64, with
  no fused multiply-add:
  - each substep `((c·u + r_x·(u_w + u_e)) + r_y·(u_s + u_n))` from exact F32 inputs, rounded once;
  - each candidate's minimum-image displacement, `d²` against `r²`, and `exp_neg(d²/2σ²)` (flife-core's
    numerics, ported);
  - each moment's decay `f32(f64(m) × factor)` (skipped at factor 1, as the reference does), then each source's
    deposit in source order, `f32(f64(m) + q·a)`, skipping a zero component.
- The host keeps the sequential F64 sums and the order-dependent steps, using flife-field's own functions: the
  energy totals and the energy plane's decay (`energy_report`), each source's support and weight sum from the
  device's weights (`weigh_into`), the energy-plane deposits and their amounts `q` (`deposit_energy`), and the
  report (`step_report`). A sum of thousands of terms in a fixed order is serial, and one GPU thread's software
  F64 is far slower at it than the host.
- One step is two commands: the substeps and the weights, then the moments once the host has written each
  candidate's amount over its weight. The field is uploaded and the candidate read back every step.
- A non-finite operand anywhere sets the device's fault word, and the step is refused (`NumericalInvalid`). So
  is a non-finite amount, which could alias a weight marker. A displacement the reference refuses is refused the
  same way. The world then takes the step on the reference, which gives the reference's own result or error.
- A backend of the engine path: an error from `advance` hands that step to the reference and drops the backend
  (`flife-engine`'s `World::field_report`).

**Invariants — the sensors (FL-031; §6.1, §8.1, §10.7).**
- Every query's sample is `PyramidView::sample`'s, bit for bit: 33 values, 33 x-derivatives and 33
  y-derivatives in binary64.
- The field crosses to the device as F32 bit patterns, never held in a device `float`. The coarse levels live
  on the device as binary64 words, planar per level, one level after another in one buffer.
- The device computes the reference's expressions operation for operation in software binary64, with no fused
  multiply-add:
  - level 1's densities `f64(u)/(h_x h_y)`, and each coarse value `¼((a + b) + (c + d))` with `a, b` the block's
    lower row;
  - each plane's lerp-form bilinear value at the query's corners, its derivatives
    `(Δ_lower + f_y(Δ_upper − Δ_lower))/spacing_x` and `(Δ_left + f_x(Δ_right − Δ_left))/spacing_y`, and, where
    `t ≠ 0`, the blend `low + t(high − low)` of each.
- The host computes the per-query geometry with the reference's own functions: the level and `t`
  (`SensorPyramid::blend`), and each level's wrapped columns, rows and fractions (`SensorPyramid::corners`).
  So the torus wrapping, the floors and the scale's `log₂` are the reference's code, not a port.
- One tick is one command (FL-029): level 1 from the field, each further level from the one below, then the
  samples. The field is uploaded and the samples read back every tick.
- The device refuses exactly the queries the reference refuses. softfloat64 is the CPU's, bit for bit, on finite
  operands, and it turns every non-finite operand into a quiet NaN (its conversion, add, subtract, multiply and
  divide each test for one first). In the reference, too, an infinity or a NaN that reaches a lerp or a division
  by a spacing never becomes finite again. So a non-finite field value makes every sample that reads it, directly
  or through a coarse level, non-finite on both sides. The host checks every sample, as the reference does, and
  refuses the call (`NumericalInvalid`, "sensor sample"). A query that does not read the value is served bit for
  bit. A scale outside `[s_0, s_L]`, a non-finite scale or position, and a field on another grid are refused
  before any command, as the reference refuses them.
- Buffers are allocated on first use and again when the grid, the level count or the query count outgrows them
  (to the next power of two); the old ones retire when dropped. The 4,096-organism preset holds nine: the field,
  the coarse levels, four level parameters, the sample parameters, queries and samples.
- A backend of the engine path: an error from `sample` hands the tick's sampling to the reference and drops the
  backend (`flife-engine`'s `World::sensor_report`).

**Refusal cases.** No Metal device, which is always the case off macOS: `MissingCapability`. A kernel that does not
compile, or a failed dispatch: `StateConflict`. An empty batch to `upload`: `LayoutMismatch`. FL-028's owner refuses:
- a buffer of no elements: `LayoutMismatch`;
- one beyond the byte limit: `BudgetExhausted` ("metal owner bytes");
- one the device refuses: `AllocationFailed`;
- a read lease beyond its purpose's limit: `BudgetExhausted` ("metal read leases");
- `StateConflict` for: a stale or foreign handle; a retiring buffer; a writer beside a reader or another writer; a
  reader beside a writer; a read of undefined contents (for a command, its first binding of the buffer); an alias;
  a command of no kernels (FL-029); and an out-of-range host read or write.

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
  a refusal before encoding changes nothing. FL-029's `tests/batch.rs` adds a command buffer that really fails.
- `tests/batch.rs` (FL-029, 3 tests), on this host's device:
  - `a_batch_runs_its_kernels_in_order_as_one_command`: `b = a + 1`, `c = b + 1`, then `a = b + c`, as one
    command. Each kernel sees its predecessors' writes. The receipt counts 3 kernels and gives epochs `[2, 1, 1]`:
    `a`, read first and written last, advances once. Afterwards every buffer is `Committed` and nothing is held.
  - `a_batch_is_checked_as_one_command_and_a_refused_one_runs_nothing`. Each refusal leaves every buffer's state,
    epoch and values unchanged, with nothing held or running:
    - an empty batch;
    - an alias in the second kernel;
    - a rendering lease on a buffer a later kernel writes (a kernel reading it twice beside the lease is admitted);
    - a host write lease on a buffer a later kernel reads;
    - a threadgroup above the pipeline's maximum in the second kernel, which the gateway refuses after accepting
      the first.
  - `a_failed_command_buffer_leaves_what_the_batch_writes_undefined`, through FrankenTorch's fault seam. The seam
    leaves the next command buffer uncommitted, and it reports `NotEnqueued`, as one a GPU restart aborted does.
    - Both written buffers become `Available`, undefined at epoch 1, and refuse reads.
    - The buffer the batch only read stays `Committed` at epoch 1.
    - A batch that reads an undefined buffer first is refused.
    - The same batch again, whose first kernel rewrites `b` before the second reads it, is admitted: epochs
      `[2, 2]`, and `c = a + 2`.

    The seam exists only with debug assertions, so this test runs in full under `cargo test` (the dev profile).
    A release build only checks that the seam is absent.
- `tests/world.rs` adds the owner's account after the 400-tick preset run: exactly the backend's five buffers, with
  no lease, command or retiring buffer left. The arithmetic, forward and world tests ran unchanged on the ported
  crate, every output still bit-identical.
- `tests/field.rs` (FL-031, 5 tests), each step against `advance_field_on` on the same input, every candidate value
  and report term compared as bits:
  - `the_metal_step_is_the_reference_step_bit_for_bit`, three random fields each (zeros, subnormals, both signs,
    magnitudes to 1e6) over: the preset's scale (128², eight sources of radius 40, decay); the preset's plan
    without sources; centers on the seams and corners of a non-square grid of non-unit cells; two and five
    substeps; a support covering the whole torus; one-cell supports; centers on cell centers with an integer
    radius, so candidates lie at exactly `d² = r²`, which the reference includes; and no sources. More than
    1.5 million values in all.
  - `a_support_whose_far_weights_underflow_is_still_the_reference`: a radius of 45σ, where `exp` underflows to
    zero and those cells stay in the support.
  - `a_non_finite_operand_is_refused`: an infinity in one moment value is refused, and the next finite step
    succeeds.
  - `a_zero_component_leaves_negative_zero_moments_negative`: moment planes of `-0.0` keep them through the
    stencil and decay, where a source with a zero component in a channel must skip it.
  - `random_worlds_step_as_the_reference`: 48 random worlds (grid shapes, cell sizes, kernels from a few cells
    to the whole torus, one to eight sources anywhere, substeps and decay). Any refusal must be the reference's
    too.
- `tests/field_world.rs` (FL-031, 4 tests), the engine path, on the real preset:
  - with its field on Metal, the world's digest equals the reference's at every one of 400 ticks; every step came
    from the device, none failed, and the owner holds exactly the backend's twelve buffers, with no lease, command
    or retiring buffer left;
  - with field and controllers both on Metal, 200 ticks, the same;
  - the planted negative flips one bit of one moment value after the device's seventh step, and the digests must
    differ at exactly tick 7;
  - a backend failing at its fifth step (after leaving a damaged value in the candidate) hands that step to the
    reference and is dropped: the world stays the reference's, with 4 device steps, 1 reference step and the
    failure recorded.

- `tests/sensors.rs` (FL-031, 3 tests), each sample against `PyramidView::sample` on the same pyramid and field,
  every value compared as bits:
  - `the_metal_samples_are_the_reference_samples_bit_for_bit`: six grids, from no coarse level to four, square,
    stretched (`1.25 × 3` cells), a `64 × 8` torus whose coarsest level is one row, and odd spacings (`17/48`,
    `31/48`), each on a random field (zeros, subnormals, both signs of moment, magnitudes to 1e6). Queries at the
    first, middle and last knot of every level, the cell edges beside them and the value just below each knot,
    the seams from both sides (`0`, `-0`, `L`, just below `L`, `-1e-300`), positions a few lengths away, and 400
    random ones; scales at every `s_l` (`t = 0`), just above it, at `√2·s_l`, just below `s_(l+1)`, at `s_L`, and
    random. Then the first grid again on buffers sized for another, with fewer queries and with more than its
    capacity. 9,960 samples in all.
  - `the_metal_samples_of_the_running_preset_are_the_reference_samples`: the preset's field at ticks 1, 21 and
    41 of a running world, about 3,800 queries each.
  - `the_metal_sampler_refuses_what_the_reference_refuses`: eight scales and positions the reference refuses; a
    field on another grid; no queries (and no samples left from the call before); an infinity, a NaN and a
    negative infinity in three planes. The reference refuses a query at that cell and a coarsest-level query
    above it, and so does the device, alone or beside a good query; a far query is served bit for bit. The
    same without coarse levels, and the device matches again afterwards.
- `tests/sensor_world.rs` (FL-031, 4 tests), the engine path, on the real preset:
  - with its sensors on Metal, the world's digest equals the reference's at every one of 400 ticks; every tick's
    samples came from the device, none failed, and the owner holds exactly the backend's nine buffers, with no
    lease, command or retiring buffer left;
  - with sensors, field and controllers on Metal (sensors and field on one owner), 200 ticks, the same;
  - the planted negative flips bit 51 of every sample's energy density after the device's seventh tick, and the
    digests must differ at exactly tick 7;
  - a backend failing at its fifth tick (after leaving damaged samples) hands that tick to the reference and is
    dropped: the world stays the reference's, with 4 device ticks, 1 reference tick and the failure recorded.

Planted mutants, each run in a scratch copy.
- FL-031's field: 14 planted against `tests/field.rs`, 13 caught:
  - the stencil's sum re-associated;
  - the east neighbour not wrapping;
  - the stencil's fault check dropped;
  - the substeps' ping-pong parity inverted;
  - the cell center's half-cell offset dropped;
  - the radius test made exclusive (caught by the integer-radius case, added for it);
  - decay never applied;
  - a zero component not skipped (caught by the negative-zero test, added for it);
  - an absent candidate not skipped;
  - the column rank read from the row table;
  - the sources deposited in reverse order;
  - the host's amounts not written over the weights;
  - the weights kernel's source lookup off by one at a source's first entry.

  One survived, and it is equivalent: the minimum image taking `+L/2` instead of `-L/2` at exactly half the world.
  The displacement enters the weight only squared.
- FL-031's sensors: 19 planted against `tests/sensors.rs` and `tests/sensor_world.rs`, 19 caught:
  - on the device: the 2×2 average pairing columns instead of rows; level 1 averaging an energy instead of its
    density; a coarse level stored over the finer; the x-derivative divided by the y spacing; the interpolant
    written as `(1 − f)a + fb` instead of in lerp form; the upper level never blended; the blend run from the
    upper level;
  - on the host: a zero weight still blending; buffers kept when the queries outgrow them, or for another grid;
    every level's spacing taken as the base grid's; the upper level's corners taken from the lower level; a
    non-finite sample not refused;
  - in the engine: the backend's samples not used (caught by the planted negative); a decision lane, or an
    observation lane, reading the samples from 0 instead of its run's start; a failing backend kept; a failing
    backend's samples used; the backend never called.

  A first version held a device fault word, set by any kernel that read a non-finite field value. Of 21 mutants
  planted against it, 20 were caught. The survivor, the level-0 read's check dropped, was equivalent: softfloat64
  turns every non-finite operand into a NaN, so the sample came out non-finite and the host refused it anyway.
  So the fault word was redundant. It was also wrong in one case: it refused a tick over a value no query reads,
  which would drop the backend where the reference succeeds. It was removed, and the refusal test now requires a
  far query to be served bit for bit.
- FL-029's owner batch: 14 planted, 14 caught, in the dev profile so the fault-seam test runs:
  - later kernels' bindings unchecked;
  - a later kernel's write forgotten;
  - an alias within one kernel admitted;
  - an undefined buffer read first;
  - an undefined buffer refused even when rewritten first;
  - a written buffer admitted beside readers;
  - a read-only buffer admitted beside a writer;
  - an empty batch admitted;
  - a kernel the gateway refused skipped, and the rest submitted;
  - a failed batch's outputs left defined, or keeping their epoch;
  - a command not counted running;
  - a read-only or a written buffer not held while the command runs.
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

**The field's frontier (FL-031).** On the preset (128², 33 planes, eight sources of about 5,000 cells, one
substep) on an M5 Ultra, 2026-11-09. Other workloads held the host at load 45–90 throughout, so the wall times
are indicative; the device times are steadier. The CPU reference's phase 2 takes 1.8–2.0 ms per tick on one worker
and 2.7–2.9 ms on sixteen, inside whole ticks.

| Metal step, per tick | First version | With host amounts |
|---|---|---|
| device: substeps (540,672 values) | 0.53 ms | 0.54 ms |
| device: weights (about 52,000 candidates, each an `exp`) | 0.44 ms | 0.42 ms |
| device: moments (524,288 values) | 1.00 ms | 0.42 ms |
| wall: command 1, including the upload | 1.6–2.2 ms | 1.6–2.4 ms |
| wall: host sums and deposits | 0.18 ms | 0.21 ms |
| wall: command 2, including the readback | 1.3–1.5 ms | 0.7–1.3 ms |
| phase 2 inside whole ticks, one worker | 3.4–6.3 ms | 2.7–6.5 ms |

The first version divided each moment's weight by its source's sum on the device. Writing the host's amounts `q`
over the weights instead removed a software division per value and 58% of the moments' device time, with
identical results. Whole ticks (256, ABBA, same digest in every run) still ran 0.80× as fast with the Metal field
on one worker and 0.95× on sixteen. The device's arithmetic alone (1.4 ms) is about three quarters of the CPU's
phase 2. Two synchronous round trips and the field's copies each way (2.1 MB up, 2.1 MB down) cost more than the
rest. Software binary64 on this GPU therefore does not pay for phase 2 on its own. It can pay once the field stays
resident and the phases around it (pyramid, sensing, uptake) run on the device too, so that the copies and most
round trips disappear. That is the rest of FL-031 and FL-032.

**The sensors' frontier (FL-031).** Whole ticks of the preset (`tick_throughput -- W 20 8 40
cpu,cpu+sensors,metal,metal+sensors`, the four worlds in rotating 20-tick blocks in one process, medians of 8
blocks after 40 warm-up ticks, the same digest after every block), on an M5 Ultra under other load (load averages
39–58 at the end), 2026-11-10:

| workers | reference | sensors on Metal | Metal controllers | Metal controllers and sensors |
|---|---|---|---|---|
| 1 | 23.4 ticks/s (cv 3.3%) | 22.9 (cv 3.1%), 0.98× | 52.4 (cv 2.8%) | 48.5 (cv 7.9%) |
| 16 | 76.6 (cv 6.1%) | 60.3 (cv 1.8%) | 75.4 (cv 4.2%) | 60.4 (cv 4.4%), 0.80× of Metal controllers |

Rows with a cv above 5% are not admissible: there is no admissible ratio for Metal sensors beside Metal
controllers on one worker, or for sensors on Metal against the reference on sixteen. The admissible ones: on one
worker the Metal sampler roughly matches the reference's (0.98×), and on sixteen it costs a fifth of the tick
beside the Metal controllers (0.80×). Not profiled, so the causes are hypotheses. On one worker the reference
samples serially, and the device's round trip (the field up, 2.1 MB; the samples down, 3.2 MB; one synchronous
command) costs about what that sampling does. On sixteen the reference samples on all lanes in parallel, while the
device's call runs on the calling thread before the lanes start. So nothing selects the Metal sampler. It is the
second phase of the chain that pays only once the field and the samples stay on the device, with the observation
encoding there too (FL-032).

**The softfloat miscompile.** While building the weights kernel, every in-radius weight came out wrong, always
the same wrong value. Instrumented variants and a trace kernel computed the right one. `exp_neg`'s series indexed
a `constant ulong INTEGER[17]` table of binary64 integers inside its loop; computing the divisor with
`d_from_u32(term)` instead made every parity test pass. Unrelated edits to the kernel had also changed the
outcome. The cause is not established (a compiler fault is suspected, macOS 27 on an M5 Ultra), and no reduced
reproducer was built. `softfloat64.metal` says so where the table was. Because the controllers' exact path uses
the same series, `tests/arithmetic.rs`'s exhaustive tests were rerun on the new code and pass. Those tests and
`tests/field.rs` are the guard: a change to the shared binary64 is proven by running them, not by reading it.

**Validity domain.** Apple GPUs through the gateway's safe math mode; the dense-104-36-v1 catalog's shapes. The
owner: `f32` and `u32` buffers. Each command is one command buffer of one or more kernels, submitted synchronously.

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
- FL-030's closure also needs FL-026's packed genome arenas, and its resident-bandwidth benchmark against §19.2's
  weight-stream bound. FL-029's batched submission is now in place.
- FL-029 gaps:
  - Submission is still synchronous: `submit_batch` returns once the command buffer completes, and the calling
    thread waits for it. The owner's bind/release split is where an asynchronous completion would plug in
    (submit, then wait or poll for the receipt, with the bindings held meanwhile). No consumer needs it yet.
  - The engine path still submits one kernel per tick (`MetalBackend::evaluate`). Several kernels per command pay
    off once FL-032 moves the CPU phases around the controllers onto the device.
  - `device_seconds` is in the receipt but not yet in run records or the throughput example.
  - A failed command buffer is tested through the gateway's seam: a real command buffer, left uncommitted. No
    device fault on this host fails a committed one (see the tests).
- FL-028 gaps:
  - The controllers and backend open their owners `UNBOUNDED`. Declaring the backend's byte budget at preflight,
    and admitting observers' and checkpoints' peaks against it, is FL-043. Render and checkpoint consumers of
    device leases are FL-033 and FL-038; today's renderer reads the CPU world's epochs (FL-017).
  - Only `f32` and `u32` elements. Textures, heaps and private-storage buffers are not owned yet.
  - `ResourceState` reports the dominant use when several overlap, by precedence: retiring, computing,
    checkpointing, rendering, committed, available. `usage` gives the lease counts.
- FL-031 gaps (the item stays open):
  - Phases 2 and 3 run on the device. Demand and grants with oversubscribed cells (uptake) are the item's
    remaining evidence.
  - The field is not resident: phase 2 uploads and reads it back every step, in two synchronous commands, and
    phase 3 uploads it again and reads every sample back (3.2 MB on the preset) in a third. Residency needs the
    field kept on the device between them, and the observation encoding on the device to keep the samples there
    (FL-032).
  - Measured slower than the CPU on the preset (above), so nothing selects it by default: no CLI or Studio
    choice exists yet, unlike `ControllerChoice`. The engine port and these tests are its only users.
  - The host sums stay serial on the CPU by design. A device reduction would need a fixed tree, which is a
    different numerical profile (§11.3).
  - The world's invariant checks (about 10% of a one-worker tick in a profile) run on the CPU over the read-back
    field.
- No CUDA backend exists: FrankenTorch has no CUDA crate, and no host here has an NVIDIA GPU.
