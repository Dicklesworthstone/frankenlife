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

FL-031, last part: phase 5's demands and grants on Metal, bit-identical to `flife_field::demands` and
`flife_field::resolve_uptake` (plan §5.2, §8.1, §10.7), in `uptake`. `MetalUptake` is a `flife_field::UptakeBackend`.
The device computes each organism's footprint density and saturating request, and each occupied cell's compensated
demand, quantum, target, floor shares, remainders and largest-remainder deficit, in `uptake.metal` on the same
`softfloat64.metal`. The host takes the footprints, stages and groups the packets in identity order through
flife-field's `demands_on` and `resolve_uptake_on`, and refuses a cell whose quanta do not sum to its grant.

FL-032 (`flife-kf8`), first stage: phase 6's chemistry on Metal, bit-identical to flife-engine's `composition`,
`Chemistry::settle` and the budget's `credit` and `pay` (plan §5.3, §8.2, §10.7), in `chemistry`.
`MetalChemistry::settle` takes plain data (the run's constants; each organism's affinity, budget and packet range;
each packet's cell and grant), because a backend does not depend on the engine (§18.2).

FL-032, second stage: phase 6's maintenance and motion on Metal, in `motion`. `MetalMotion::settle` runs what
flife-engine's `settle_stages` runs after chemistry, from the exhaustion check after chemistry to the one after
motion: `MaintenanceCosts::bill`, `MaintenanceBill::settle`, `uptake_ratio`, `MotionResponse::new`, and `Motion`'s
`acceleration`, `propose`, `settle` and `displace` (plan §5.4, §5.6, §8.2, §10.7). It takes plain data too: the run's
constants, and each organism's budget after chemistry, controller evaluation and parameter count, sensing scale,
absorbed uptake, velocity, position, motion response and sensor gradients.

FL-032, third stage: phase 6's emission on Metal, in `emission`. `MetalEmission::settle` runs, for each organism
alive after motion, `EmissionPolicy::emit` with flife-field's `Deposits::push` (its payment, offer and direction
checks, `ExchangeStencil::at` and the deposit split) and the budget's payment, then the exhaustion check after
emission (plan §5.5, §8.2, §10.7). It returns each deposit's packets, cell and amount in stencil order, for the
caller to stage in identity order. flife-field's `Deposits` gains `entry` and `entry_packets`, read-only, which the
tests compare the device with.

FL-032, fourth stage: the three stages in a world. Their plain-data types moved to flife-field's `settle`, the
port `SettleBackend`, unchanged; this crate re-exports them. `MetalSettle` (`open`, `with_owner`, `device_name`,
`owner`) holds `MetalChemistry`, `MetalMotion` and `MetalEmission` on one `DeviceOwner` and implements the port;
`World::set_settle_backend` runs it. The engine replays each organism's payments on its own budget and refuses a
settlement that differs by a bit (flife-engine's contract), so a backend that fails or is refused hands its tick
to the reference and is dropped.

FL-032, fifth stage: phase 7's reproduction proposal on Metal, in `proposal` (`philox.metal`, `proposal.metal`).
`MetalProposal::settle` runs, for each organism alive after emission, `Reproduction::propose`: the exhaustion and
cooldown checks, `attempts` (the probability's range, then word 0 of fnp-philox-semantic-v1's block for the birth
stream, the parent, the tick and the attempt lane, as a half-open uniform against the probability) and `split`
(the balance and fraction checks, the attempt minimum, the birth cost rounded down, the child's and parent's
shares, their minimums) on software binary64 (plan §7.4, §8.2). The block is Philox4x64-10 of the counter plus
one, as NumPy increments it, under the key [world seed, stream code]. `MetalSettle` holds a `MetalProposal` on
its owner and implements the port's `proposal`; the engine checks the cooldown and the probability and
recomputes every attempt's split, so it takes from the device only whether the draw made an attempt
(flife-engine's contract).

FL-032, sixth stage: phase 8's parameter mutation on Metal, in `birth` (`numerics.metal`, `birth.metal`).
`MetalBirth::mutate` takes the rates (`MutationConstants`: the world seed, the replacement and weight
probabilities, the weight's standard deviation) and a batch of births (flife-field's `MutationBirth`: parent,
tick, candidate, and the child's dense member as its depth, at most three layers' inputs, outputs and weight and
bias offsets, and its parameter count), with every child's candidate parameters, copied from the parent or
mapped onto another member, in the reference layout and the genome's dtype (`ParameterBits`: F32 or F16). It
refuses a birth whose layers are not a dense member's: no layers or more than three, a layer's weights not at
the last layer's end or its biases not after its weights, or a parameter count past the layers' end. It mutates each parameter as `Mutator::propose` does (plan §7.3). Word 0 of the block for the mutation
stream, the parent, the tick and the parameter's lane (candidate; weight or bias; layer, output, input) decides a
replacement against the replacement probability, and word 2 a perturbation against the weight probability. A
replacement is the fan-in draw of word 1. A perturbation adds the deviation times component 0 of Box–Muller on
draw 1's first two words, and rounds once to the dtype. `numerics.metal` holds what that needs of flife-core
beyond softfloat64: `exp` of either sign, `ln`, `sin_cos_turns`, `open_unit`, `box_muller` and binary16's
conversions, operation for operation. Each birth's counts of replaced and perturbed parameters come back; on an
error the values are left as they were. Topology choice, mapping, traits, the seal and the quota stay on the
host. `MetalSettle` holds a `MetalBirth` on its owner and implements the port's `mutation`: the engine sends
each tick's births' first candidates in one batch and takes the values and counts as given (flife-engine's
contract). A later candidate, after the no-op check or the seal rejects the first, is mutated on the host.

FL-032, seventh stage: phase 9 on Metal, in `reduce` (`reduce.metal`): the uptake grants' debit and the
emission and recycling deposits' reductions into the tick's candidate field, bit for bit
`flife_field::reduce_field_on` (plan §8.1, §11.1). `MetalField` implements the port's `reduce`, and a world
given it as its field backend runs phase 9 there. flife-field's own `Grants::debit` and `Deposits::reduction`
make every check and group each set's packets by (cell, identity) before anything is written. The host sends
each debit that granted energy (cell, energy before and after, strictly ascending cells), each set's grouped
packets (amount and deposit index) with its cell offsets, and its deposits' directions. One command then runs
up to three kernels on a device copy of the field. `grants_debit` (a thread per debit) writes the energy and
scales each moment by `after / before`, or zeroes a drained cell's. `deposits_reduce` (a thread per cell, once
per set) sums the cell's amounts rounding down, its moment sums `Σ amount · component`, the energy rounded down
to the F32 at or below `before + total`, what the cell received, and each moment raised by its sum times
`received / total` when that is not zero, as `apply_deposits_on` does. Every value is binary64 on
`softfloat64.metal`, and the field crosses as bits. The host reads back what each occupied cell received and
sums it in cell order (`DepositReduction::summarize`). It then copies the device's field over the host's, and
only then marks the grants and deposits applied. A non-finite value a kernel meets or would write raises the
fault word (softfloat64 cannot carry an infinity), and nothing is written or applied (plan §8.3): the engine
hands the reduction to the reference. Residency (the field kept on the device across phases and ticks), phase
10's checks on the device and whole-tick parity are FL-032's remaining work.

**Inputs.**
- The engine path: `MetalBackend::open()`, given to a world with `World::set_controller_backend`. It implements
  `flife_neural::ControllerBackend`, the port through which the world hands each tick's due evaluations over in
  one batch (`ControllerJob`: genome identity, arena slot, admitted controller, inputs). `ControllerChoice` (`cpu`, `torch`,
  `metal`, `auto`) is what the CLI and Studio select it with (`torch` is `flife-backend-cpu`'s `CpuBackend` on the
  world's workers, on any host): `backend(cpu_workers, budget)` gives `auto` Metal only on a host with
  a device and at most `AUTO_METAL_MAX_WORKERS` = 8 workers, where it measured faster (whole ticks, 256 ticks, two
  interleaved runs: 2 workers 27.5 → 16.3 ms, 4 workers 19.5 → 14.8 ms, 8 workers 15.8 → 14.3 ms). Where it does
  not take Metal, it takes `torch` at most `AUTO_TORCH_MAX_WORKERS` = 4 workers, where that measured faster on
  Linux and macOS (`flife-backend-cpu`'s contract). `ControllerChoice::auto_for(cpu_workers, metal_device)` states
  the rule for either kind of host. Test `tests/world.rs::auto_takes_metal_only_up_to_its_worker_ceiling` checks the
  rule for both kinds on any host, and that this host's `auto` is the rule's choice for it.
- FL-043: `backend(cpu_workers, budget)` takes a `BackendBudget` (re-exported from `flife-neural`: bytes, and the
  organisms the world can hold), `BackendBudget::UNBOUNDED` for none. `MetalBackend::with_budget(shard_slots,
  budget)` opens its own `DeviceOwner` with `Limits { bytes: budget.bytes, .. }`; `open` and `with_shard_slots` are
  unbounded. `SHARD_SHARE` (8) and `staging_bytes(organisms, shards)` are public.
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
  sources and the time step, exactly what `advance_field_on` takes. Each phase 9 (FL-032) receives the
  candidate field, the tick's grants and its emission and recycling deposits, exactly what `reduce_field_on`
  takes. No CLI or Studio choice selects it yet (see the known gaps).
- FL-031, the sensors: `MetalSensors::open()` (or `with_owner(owner)`, to share the field's owner), given to a
  world with `World::set_sensor_backend`. Each tick receives the pyramid (its grid and level count), the observed
  field and every live organism's `SensorQuery` (position, sensing scale), in row order. No CLI or Studio choice
  selects it yet either. `examples/tick_throughput.rs` measures it: a choice such as `cpu+sensors` or
  `metal+sensors+field` puts phase 3 (and phase 2) on Metal beside the controllers.
- FL-031, uptake: `MetalUptake::open()` (or `with_owner(owner)`), given to a world with
  `World::set_uptake_backend`. Each tick it receives the observed field, the request rule (`Δt`, `U_max`, `K`)
  and every organism's footprint, in identity order; then each occupied cell (its first packet, packet count and
  F32 energy) and every packet's request, in (cell, identity) order. No CLI or Studio choice selects it;
  `tick_throughput`'s `+uptake` measures it.

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
- `MetalBackend::last_command` (FL-030): the last batch's kernels and its command buffer's device seconds.

**Residency.** `MetalBackend` keeps every buffer on the device across ticks. FL-026: each topology has a bucket of
fixed-size parameter shards of at most `SHARD_BYTES` (256 MiB, 6,718 genomes of the preset's topology), fewer
slots if `with_shard_slots` says so, and no more elements than the device's largest buffer or the kernel's 32-bit
offsets hold. A command's kernels run one after another, so each shard a batch touches adds a kernel's fixed
latency; that is why a shard is large (the shard-size measurements below). A job's `ArenaSlot` locates
its parameters (`ShardGeometry::locate`: shard, offset and extent, checked in 64 bits). A genome is uploaded once,
into its slot, and used only while that slot holds the same generation and genome; a newborn at the next
generation of a dead genome's slot is uploaded in its place. A parameter shard is allocated at its full size when
first touched, so a topology's first genome allocates a whole shard, and it is never rebuilt or dropped before
`forget`. Each shard has its own job words (row, offset,
hidden width, layer widths), and a tick submits one kernel per touched shard in one command buffer. Each tick
writes only the inputs and job words in place (`SharedBuffer::write_f32/write_u32`, FrankenTorch `1c4febfb`). The
buffers are `Owned` by its `DeviceOwner`: two per shard, plus the inputs, outputs and fault flags. A job buffer, or
the per-organism buffers, grow to the next power of two when a batch outgrows them, and a rebuilt buffer retires
the one it replaces. A job whose slot holds another genome after placement (one batch naming a slot twice), whose
bucket is another topology's, which the kernel cannot shape, or whose inputs are not finite in F32, is deferred
without dispatch.

FL-043: under a budget, a shard holds at most `1/SHARD_SHARE` of what a batch's staging bound leaves
(`staging_bytes`: the per-organism device buffers and the host's staged inputs, outputs, flags, dispatch lists and
job words for every organism the world can hold, and each shard's job words), so a small budget still makes several
shards. A shard is made only if its parameters and host records, with the staging bound for one more shard, fit
in what the shards already held leave; the owner refuses any device buffer past the budget as well. A job whose shard is not made is deferred, and the world
evaluates it on the reference, which gives the same bits. `resident_bytes` is the owner's bytes plus the host
vectors and each shard's host records.

FL-030, compact F16 (plan §10.3): a bucket holds the weight dtype of its first genome. Its F16 shards hold binary16
values two to a `u32` word, the even element in the low half (the owner keeps only `f32` and `u32` buffers), and
`controller_forward_half` reads them as `half`. It is the same `forward_pass` template as `controller_forward`,
instantiated for `half`, and each weight and bias is widened exactly to F32 before the same arithmetic. So a shard
of `SHARD_BYTES` holds twice the slots (13,437 of the preset's topology), a shard is charged two bytes a parameter
under a budget, and each warm batch streams half the weight bytes. A job whose dtype is not its bucket's is
deferred, and so is an F16 member with an odd parameter count (none in the catalog), whose slots would share words.

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

**Invariants — chemistry (FL-032; §5.3, §8.2, §10.7).**
- Every organism's absorbed `U`, gain `G`, requested and paid toxicity, and its budget's balance and exhaustion
  after them are the reference's, bit for bit.
- The field crosses to the device as F32 bit patterns. Each packet's composition `z = m/e` is computed in binary64
  from them, and is neutral where `e` is not positive.
- The device computes the reference's expressions operation for operation in software binary64:
  - `Affinity::new`'s squared norm, in channel order, against the limit's square;
  - `a·z` in channel order, then `η_max tanh(k a·z − b)` with `numerics::tanh` (`d_tanh`; `tanh_reference`, the
    controllers' activation, now calls it);
  - the three Neumaier sums of `settle_efficiencies` in packet order;
  - `G` credited with `add_down`, and `κ` times the harm paid with `min`, `sub_down` and the exhaustion rule.
- `f64::max` and `f64::min` are taken as they run on the reference's host, AArch64's `fmaxnm` and `fminnm`: +0 over
  −0, and −0 under +0. A zero's sign changes no sum here, since the sums start at +0, but the device follows the host
  anyway.
- Refused after the command (`StateConflict`, so the reference decides): an organism with a non-finite or over-limit
  affinity, a non-finite tanh argument, a negative or non-finite grant, an efficiency outside [−1, 1], a gain for an
  exhausted budget, a credit that overflows, a non-finite total or toxicity, or a cell outside the field. softfloat64
  turns infinities into NaNs, so every non-finite intermediate is refused, never carried. Refused before it:
  constants outside `Chemistry::new`'s ranges (`NumericalInvalid`) and packet ranges past the list
  (`LayoutMismatch`).
- One command per call. The field's 33 planes, every affinity and every packet go to the device, and the outcomes
  come back.

**Invariants — maintenance and motion (FL-032; §5.4, §5.6, §8.2, §10.7).**
- Every organism's maintenance settlement (total requested and paid, and the basal, sensor and controller shares
  paid), motion settlement (proposed and applied velocity, cost requested and paid, whether it was scaled), final
  velocity, position, balance and exhaustion, and the stage that exhausted it, are the reference's, bit for bit.
- The stages run in the reference's order, and each runs only for an organism the previous exhaustion check left
  alive. What the reference never reads (the motion inputs of an organism maintenance exhausts, everything past
  the budget of one chemistry exhausted) is never checked.
- The device computes the reference's expressions operation for operation in software binary64, with no fused
  multiply-add:
  - the bill `Δt·basal`, `Δt·(sensor·(s/unit))` and `neural·parameters` when evaluated; their total
    `(basal + sensor) + controller` paid through `energy`'s `+ 0`, then split basal first with `min` and
    `sub_down`;
  - `U/(Δt·U_max)`, at most one, zero at zero capacity;
  - the response's squared norm in term order against the limit's square;
  - `g_u Σ_j χ_j ∇_j` in term order, `e^(−γΔt) v + c·f`, and both radial clamps: `limit/sqrt(v·v)`, then the shrink
    by `1 − ε/2` while `v·v` exceeds `limit²`;
  - `c_v (v·v) Δt`, paid whole when affordable, otherwise the balance paid and the velocity scaled by
    `sqrt(available/requested)`;
  - `wrap_coordinate(x + Δt v)`: `rem_euclid`, and `+0` for a remainder that rounds up to the period or is zero.
- softfloat64 gains a correctly rounded `d_sqrt` (restoring, 54 result bits and a sticky remainder), an exact
  `d_fmod` (musl's integer long division), `d_rem_euclid` and `d_wrap`.
- Refused after the command (`StateConflict`, so the reference decides): an organism with a sensing scale that is
  not a positive length, a bill share that is not a finite nonnegative energy, a negative or non-finite absorbed
  uptake, a non-finite response or one past the limit, a non-finite gradient, a non-finite velocity, or a
  non-finite displaced position. Also refused where the reference goes on but the device cannot carry the value:
  a velocity or acceleration whose square overflows (the reference clamps it to zero), a motion cost that
  overflows (the reference scales the motion to nothing), a clamp that needs more than 64 shrinks, and a zero
  `Δt·U_max` from a positive capacity. Refused before it: constants outside the engine's ranges
  (`NumericalInvalid`).
- An organism's balance is a budget's, never −0 (`EnergyBudget` adds `+ 0`); the device takes it as given.
- One command per call. Every organism's state goes to the device (215 words each), and the outcomes come back.

**Invariants — emission (FL-032; §5.5, §8.2, §10.7).**
- Every organism's settlement (energy available, payment requested and paid, offer), whether it staged a deposit,
  each packet's cell and amount in order, and its balance and exhaustion after the stage are the reference's, bit
  for bit.
- The device computes the reference's expressions operation for operation in software binary64:
  - `min(f·available, available)`, nothing staged for a zero request, the offer `ρ·requested`;
  - the direction's squared norm, folded in channel order, at most one;
  - the stencil: the wrapped position, `x/Δx − ½` split by `floor` (`d_floor`, `d_to_long`), the four bilinear
    weights in corner order, zero weights skipped, a cell met twice merged by adding, then sorted by cell;
  - the split: every cell but the first of largest weight takes `offered·w`, summed with `add_up`; that cell takes
    `max(sub_down(offered, others), 0)`; packets of zero amount are dropped;
  - the payment through `energy`'s `+ 0`, and exhaustion.
- Only organisms that reach emission are taken: the host refuses one whose balance is not finite and positive
  (`StateConflict`), since the reference never calls `emit` for it. What the reference never reads (the direction
  and position of an organism that does not emit, or whose request is zero) is never checked.
- Refused after the command (`StateConflict`, so the reference decides): a spend fraction outside [0, 1], a payment
  or offer the deposit refuses, a direction outside the unit ball, or a position that is not finite. Refused
  before it: an efficiency outside [0, 1] (`NumericalInvalid`) and a grid of more than `u32::MAX` cells
  (`Overflow`).
- One command per call. Every organism's state goes to the device (73 words each), and the outcomes come back.

**Invariants — uptake (FL-031; §5.2, §8.1, §10.7, INV-04).**
- Every demand's amount is `demands`'s, and every packet's grant, every cell's debit (energy before and after,
  demand, grant, contended), every organism's total and the summary are `resolve_uptake`'s, bit for bit.
- The energy plane crosses to the device as F32 bit patterns, so a subnormal energy keeps its value.
- The device computes the reference's expressions operation for operation in software binary64, with no fused
  multiply-add:
  - a footprint's density `(0 + Σ w_c e_c)/(h_x h_y)`, in footprint order, and its request
    `((Δt U_max) r)/(K + r)`;
  - a cell's demand `A_c`, the Neumaier `CompensatedSum` of its packets' requests in identity order;
  - its quantum `Q = 2^(max(exp, 1) − 150)` and count `n` from the F32 bits, whether `A_c > e_c`, its target (`n`,
    or `round_ties_even(A_c/Q)` and at most `n`) and scale (`n/A_c`, or `1/Q`);
  - each packet's ideal share `q·scale`, its floor and its remainder.
- A deficit is apportioned as the reference's passes give it, in closed form. One thread per packet ranks it
  largest remainder first, ties to the lower identity, by comparing remainder bits: a remainder is +0 or positive,
  so its bits order as `total_cmp` does. Every packet gains `deficit / len` quanta, and the first `deficit % len`
  in that order one more.
- The device never apportions a surplus (floors above the target). The reference handles one, but no valid input
  makes one. Uncontended, the floors of `q_i/Q` pass `round(A_c/Q)` only if `Σ q_i − A_c ≥ Q/2`, while the
  compensated sum is within about `2^-52 A_c ≤ 2^-28 Q` of the exact sum. Contended, the ideal shares sum to at most
  `n(1 + 5·2^-53) < n + 1`. Such a cell would keep its floors, the host's sum check would refuse it, and the world
  would compute the tick on the reference. None of the tests' 48,711 occupied cells had one. A first version
  ported the reference's surplus passes: a planted mutant in them survived, because nothing reaches them, so they
  were removed.
- The host computes every footprint, and stages and groups the packets in identity order, with flife-field's own
  code (`demands_on`, `resolve_uptake_on`). It checks each density with the reference's rule and each cell's energy,
  and records each debit from the device's demand and quanta with the reference's `cell_target`. A cell whose
  quanta do not sum to its grant is refused (INV-04).
- Refused before any command: a footprint naming a cell outside the field (`LayoutMismatch`, the reference's own
  refusal), and cells that do not partition the packets in order (`StateConflict`). Refused after it: any
  non-finite density, request or demand the device returns (`StateConflict`). softfloat64 gives a NaN where IEEE
  gives an infinity (an overflowing request, or a cell's demand), so the device never returns a value the reference
  would not. The world then computes the tick on the reference, which reports its own error if it has one.
- Two commands a tick: the requests, then the grants (each cell, then the deficit ranks). The energy plane, the
  footprints and the packets are uploaded, and the results read back, every tick. Buffers grow to the next power
  of two; the preset holds eleven.
- A backend of the engine path: an error hands the tick's phase 5 to the reference and drops the backend
  (`flife-engine`'s `World::uptake_report`).

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
- `tests/world.rs` (6 tests), the engine path. The real preset (4,096 founders, `[64, 32]`, every organism
  evaluated every tick) runs 400 ticks with its controllers on Metal next to the reference world. The digests
  match at every tick, through the founder crash, 42 births and mutation. All 1,254,756 evaluations came from the
  device (none deferred, no tick redone on the reference). The planted negative flips one output bit of one
  organism at tick 9 inside a wrapping backend, and the digests must differ at exactly tick 9. Off macOS the test
  checks the typed `MissingCapability` refusal instead.
  - FL-026: in the 400-tick run, each genome was uploaded once (4,138 genomes, 4,138 uploads), newborns into dead
    genomes' slots, so one shard holds every genome, and the owner's bytes are at most `SHARD_BYTES` and 8 MiB.
  - At an evaluation interval of four ticks, in shards of 64 (64 kernels in one command), the world is the
    reference world at every tick, and each of 4,139 genomes is uploaded exactly once while it lives.
  - FL-043, `under_half_its_bytes_metal_defers_and_the_world_is_the_reference`: an unbounded backend holds 273.4 MB
    after 40 ticks of the preset; under half of that (136.7 MB), whose owner's limit is that budget, 400 ticks
    equal the reference world at every tick, with 42 births, 721,847 evaluations on the device and 532,909
    deferred, no tick redone and no failure. The owner holds 96.1 MB and the backend's resident peak is 97.6 MB.
    Planted mutants: dropping the shard check fails it, and so does an unbounded owner (by the limit's
    assertion; with the check in place the owner's limit is never reached, so the run alone would not show it).
  - FL-030, `an_f16_world_on_metal_is_the_reference_world_at_every_tick`: the preset with
    `canonical_weight_dtype = "f16"` on Metal, in half shards, equals the reference F16 world at every tick for
    400 ticks, with births, every evaluation on the device and no tick redone.
- `tests/residency.rs` (FL-026, 4 tests), in shards of 16 and 64:
  - 300 genomes over five topologies are uploaded once into 20 shards. Batches of a few upload and drop nothing.
    A slot at its next generation, holding another genome, is uploaded in its place with that genome's reference
    outputs; in one batch naming a slot at both generations, the last one uploaded is served and the other
    deferred. Another genome at the same slot and generation is another occupant; a job naming another
    topology's bucket is deferred. The owner holds two buffers per shard and three more.
  - Shard sizes 3, 7 and 256, and slots from `u32::MAX - 7` and from `2^40`, give the reference's bits.
  - FL-030: 500 genomes over every topology, rounded once to binary16 (subnormals included), in half shards of 64,
    give the reference's bits with nothing deferred. An F32 job at a slot of an F16 member is deferred.
  - FL-030: the same 500 genomes in F32 take exactly two bytes a parameter more of the owner's bytes (the two whole
    shards of 64 each topology needs), and under a budget of two thirds of the F32 backend's bytes, some F32 jobs
    are deferred while every F16 job is uploaded and gives the reference's bits.
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
- `tests/world.rs` adds the owner's account after the 400-tick preset run: exactly the backend's buffers (since
  FL-026, two per shard and three more), with no lease, command or retiring buffer left. The arithmetic, forward and world tests ran unchanged on the ported
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
- `tests/field_world.rs` (FL-031 and FL-032, 5 tests), the engine path, on the real preset:
  - with its field on Metal, the world's digest equals the reference's at every one of 400 ticks; every step and
    every phase 9 reduction came from the device, none failed, and the owner holds exactly the backend's 26
    buffers (twelve for phase 2, fourteen for phase 9), with no lease, command or retiring buffer left;
  - with field and controllers both on Metal, 200 ticks, the same;
  - `recycled_removals_reduce_on_metal_as_on_the_reference`: 60 ticks with every organism within 12 world units
    of a wandering place removed and recycled every eight ticks (856 recycled), so phase 9 reduces both deposit
    sets; the digests are equal at every tick and all 60 reductions came from the device;
  - the planted negatives flip one bit of one moment value after the device's seventh step, and after its
    seventh reduction, and the digests must differ at exactly tick 7;
  - a backend failing at its fifth step (after leaving a damaged value in the candidate), or at its fifth
    reduction, hands that phase to the reference and is dropped: the world stays the reference's, with 4 device
    steps, 1 reference step and 4 device reductions, or 5 device steps, 4 device reductions and 1 reference
    reduction, and the failure recorded.
- `tests/reduce.rs` (FL-032, 2 tests), `MetalField::reduce` against `flife_field::reduce_field_on` from the
  same field, grants and deposits, every field value and both summaries compared as bits, and the grants and
  both sets applied:
  - `the_metal_reduction_is_the_reference_reduction_bit_for_bit`: a `32 × 32` torus with scattered grants,
    emissions and recycling over energies from subnormal to 1e30 with zeros of both signs, and moments signed,
    subnormal and to 5e5; oversubscribed cells the grants drain; a hotspot of 4,000 deposits in and around one
    cell over 500 demands; tiny amounts onto subnormal cells beside amounts to 1e30; three deposits of 1e38
    into a cell holding the largest F32 (rounded down to it, nothing received); sums on either side of an F32
    boundary, rounded down and to nearest (`1 − 2⁻⁵³` then `3·2⁻⁵⁵` into an empty cell, and `4 − 2⁻⁵¹` into a
    cell of 4, which the reference must take to the F32 below 1 and below 8); grants alone, each deposit set
    alone, and nothing; a stretched `48 × 16` torus; and the first grid again on buffers grown for 6,000
    deposits. In all, 54,306 packets, 4,529 debits that granted energy (2,663 drained) and 5,844 cells with more
    than one packet.
  - `a_reduction_the_device_cannot_finish_leaves_everything_as_it_was`: a NaN moment in a cell that receives a
    deposit makes the device fault (`NumericalInvalid`) after it has run every kernel; the host's field bits
    are unchanged and the grants and both sets are not applied, and the reference then reduces them exactly as
    it reduces the untouched inputs. Infinite moments in cells the grants drain become zero as on the
    reference. Grants already applied, deposits already applied, deposits staged for another grid and one
    organism depositing twice are refused with the reference's error, nothing written or applied.

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
- `tests/uptake.rs` (FL-031, 4 tests), every value compared as bits:
  - `the_metal_grants_are_the_reference_grants_bit_for_bit`: every packet, debit, total and summary of
    `resolve_uptake_on` against `resolve_uptake`. The cases: scattered organisms over mixed energies (zeros of both
    signs, subnormals, 1e-30 to 1e30); 4,000 organisms oversubscribing every cell; a hotspot of 4,000 organisms,
    3,000 of them at one cell center, contended, uncontended and at an irrational amount, with tied remainders;
    subnormal cells asked for a few quanta and for 1e9 times more; zero cells and requests below half a quantum;
    one organism at each cell center asking exactly `k + ½` quanta, exactly its energy, just above it, or a random
    multiple of it; and every regime at once on a stretched torus, then on buffers grown and shrunk. 87,774
    packets: 5,179 contended, 5,253 deficit, 1,470 exact and 957 empty cells.
  - `the_metal_demands_are_the_reference_demands_bit_for_bit`: 3,000 organisms on each of three grids and rules,
    at cell centers, knots, edges, the seams and far away, then three of them again.
  - `the_metal_uptake_of_the_running_preset_is_the_reference_uptake`: the running preset's field and full
    population at ticks 1, 31, 61 and 91: 16,384 demands and 65,304 packets.
  - `the_metal_uptake_refuses_what_the_reference_refuses`: a NaN, an infinity and a negative energy under a
    footprint (a footprint away from it is served); a negative or NaN amount, a NaN position, and a NaN or
    negative cell energy (the grants left empty); cells that do not partition the packets; a cell whose demand
    overflows; a request that overflows (the reference's infinity, the device's refusal); a stencil from a larger
    grid. The device matches the reference again afterwards.
- `tests/uptake_world.rs` (FL-031, 4 tests), the engine path, on the real preset:
  - with its uptake on Metal, the world's digest equals the reference's at every one of 400 ticks (42 births).
    Every tick's phase 5 came from the device. Installing it reserves 2,752,512 bytes of host lists (FL-043),
    stepping never grows them, and removing the backend releases them. The owner holds the eleven buffers, with no
    lease, command or retiring buffer left;
  - with field, sensors, uptake and controllers on Metal (the first three on one owner), 200 ticks, the same;
  - the planted negative moves one quantum between two packets of one cell at the seventh tick, which the host's
    sum check cannot see: the digests must differ at exactly tick 7;
  - a backend failing at its fifth tick (leaving damaged quanta), and one granting a cell one quantum too many at
    its sixth, each hand that tick to the reference and are dropped: the world stays the reference's, and the
    failure is recorded.
- `tests/chemistry.rs` (FL-032, 3 tests), every value compared as bits:
  - `the_settlement_arithmetic_is_the_references`: 262,400 operand pairs (random finite patterns, near pairs,
    subnormals, zeros of both signs, tanh's active range, and 256 edge pairs) against flife-core's `numerics::tanh`,
    `add_down`, `sub_down`, `f64::next_down`, `max`, `min` and a `CompensatedSum`. Each operation is checked on more
    than 200,000 pairs.
  - `chemistry_on_the_device_is_the_references_settlement`: 4,096 organisms under each of three constant sets (the
    preset's; saturating, `k = 1e6`; gainless, `k = 0`) against `Chemistry::settle`. The field has zero, subnormal
    and 1e-30 to 1e30 energies and wide moments. Each organism has 0 to 8 packets, past the reference's stack of
    four. Budgets run from zero to 1e4. The draws are asserted to include toxicity exhaustion, gains, harms, and
    empty and long packet lists.
  - `chemistry_the_reference_refuses_is_refused`: a NaN affinity, one past the limit, a negative grant, a NaN grant,
    an infinite moment, a gain for a zero budget, and a cell outside the field. Each is placed beside a valid
    organism: the reference refuses it, the device refuses the call and leaves no outcomes, and the valid organism
    alone settles as the reference does. Packets past the list and a zero `κ` are refused before any command.
- `tests/motion.rs` (FL-032, 3 tests), every value compared as bits:
  - `the_motion_arithmetic_is_the_references`: 262,636 operand pairs: 262,144 random patterns (infinities and
    NaNs, near pairs, exponents up to 64 apart, subnormals, zeros of both signs), 168 coordinates around six
    periods, and 324 edge pairs. They are checked against `f64::sqrt`, `%`, `rem_euclid` and flife-field's `wrap_coordinate`. More than
    100,000 positive square roots, 50,000 dividing remainders, 20,000 Euclidean corrections and 100,000 wraps are
    checked; NaN results must be NaN on both sides. The one difference is stated: softfloat64 has no infinities,
    so `rem_euclid` of a negative value on an infinite period is a NaN on the device.
  - `motion_on_the_device_is_the_references`: 4,096 organisms under each of three rule sets (the preset's, which
    the test asserts equal to the engine's `from_config`; an expensive, drag-free set on a 3.7 × 1 torus; and a
    set whose every cost is −0, with no uptake capacity and a drag that underflows the decay) against the
    reference's calls in `settle_stages`' order. Budgets run from zero and subnormal to 1e4, some drawn between
    the bill and the bill plus the largest motion cost. The draws are asserted to include deaths at chemistry,
    maintenance and motion, scaled motion, proposals at the speed limit, wraps across an edge, and both clamps'
    shrink loops (recomputed on the host).
  - `motion_the_reference_refuses_is_refused`: zero, negative, NaN and infinite sensing scales, a negative and a NaN
    absorbed uptake, a NaN response, one past the limit, an infinite gradient, a NaN velocity, an infinite
    position, and a sensor share that overflows, each beside a valid organism. Also refused: a velocity of 1e200
    and a motion cost that overflows, which the reference settles. Not refused: an organism chemistry exhausted
    with NaN everything else, and one maintenance exhausts with a NaN response and infinite gradients, whose
    outcomes are the reference's. A huge finite sensor share and a huge finite motion cost settle as the
    reference does. Constants out of range are refused before any command.
- `tests/emission.rs` (FL-032, 3 tests), every value compared as bits:
  - `the_emission_arithmetic_is_the_references`: 262,164 operands (random patterns, values near integers and
    half-integers of either sign, zeros, subnormals, and 20 edges) against `f64::floor`, `as i64`, and
    `-add_down(-a, -b)`. More than 50,000 fractional floors, 100,000 integers and 200,000 upward sums are checked.
  - `emission_on_the_device_is_the_references`: 2,048 organisms for each of four grids (the preset's 128 × 128,
    1 × 1, 1 × 7 and 3 × 2, with non-integer sides) and three efficiencies (the preset's 0.9, 1 and 0) against
    `EmissionPolicy::emit` and the entry and packets it staged. Positions include zero, the last value below each
    side, cell centres and cell edges; fractions include 0, 1 and subnormals. The draws are asserted to include
    deaths at emission, staged deposits, deposits without packets, and deposits of one and of four packets.
  - `emission_the_reference_refuses_is_refused`: NaN, negative and above-one fractions, a direction past the unit
    ball, a NaN direction, and NaN and infinite positions, each beside a valid organism. Not refused, and settled
    as the reference settles them: an organism that does not emit with a NaN position, and one with a zero
    fraction, NaN direction and infinite position. Refused before any command: zero, negative and NaN balances,
    and an efficiency of 1.5.
- `tests/proposal.rs` (FL-032, 3 tests), every value compared as bits:
  - `the_random_blocks_are_numpys_and_flife_cores`: the device's Philox4x64-10 blocks against FrankenNumPy's
    NumPy-exact `PhiloxRng` on random keys and counters and on eight carry counters (each word wrapping in turn,
    and the whole counter wrapping to zero), and against flife-core's `SemanticRng::block` on semantic addresses
    of every stream and lane kind; each word's half-open uniform, and that of 58 edge words (zero, the largest,
    the leading 52 bits' smallest and largest, and each bit alone), against flife-core's.
  - `proposals_on_the_device_are_the_references`: 4,096 organisms under each of four rule sets (the preset's; no
    birth cost or attempt minimum with the smallest positive minimums; a birth cost of 60; a child's minimum of
    0.4 beside a parent's of 0.2), with the constants from `Reproduction::proposal_constants`, against
    `Reproduction::propose`. Balances run from zero to 1e300, some a few ulps above the birth cost, some at the
    attempt minimum and one ulp below it; deadlines fall on, before and after the tick; probabilities include
    ±0, 1, the smallest subnormal, the largest below one, and the attempt's own uniform and one ulp above it;
    fractions include the smallest subnormal and the largest below one; identities reach `u64::MAX`. Every
    outcome is seen (exhausted 6,547, cooling down 1,675, no attempt 4,378, the four unviable reasons 784, 401,
    657 and 213, viable 1,729).
  - `proposals_the_reference_refuses_are_refused`: probabilities of NaN, the smallest negative, one ulp above one
    and infinity; balances of -1, NaN and infinity; fractions of zero, one, NaN and -0.5, each beside a valid
    organism.
    Not refused, and proposed as the reference proposes: a cooling-down or exhausted parent with NaN everything
    else, and a parent that makes no attempt with a NaN fraction. Refused before any command: a NaN or infinite
    constant.
- `tests/birth.rs` (FL-032, 3 tests), every value compared as bits:
  - `the_arithmetic_is_flife_cores`: 69,632 rows through `MetalBirth::birth_ops` against flife-core's `exp`,
    `ln`, `sin_cos_turns`, `open_unit`, `half_open_unit`, `box_muller`, `F16::from_f64`, `F16::to_f32` and
    flife-genome's `fan_in_draw` (binary64 and F32). Edge rows include ±0, the series' reduction bound and its
    neighbours, exp's overflow at 710 and its neighbours, the subnormal range, ln at one, √2 and their
    neighbours, the quarter turns and their neighbours, words whose leading 52 bits are zero or all ones,
    binary16's largest finite, its overflow tie and its subnormal ties, infinities and NaN, and fan-ins from 1
    to `u32::MAX`; every binary16 pattern is widened at least once. exp is undefined in 4,921 rows, and 23,002
    logarithms are of subnormals.
  - `mutations_on_the_device_are_the_references`: children proposed by `Mutator::propose` from six founders at
    ticks 0, 7, 1,000 and `u64::MAX - 1`, and a second generation from them, under seven rate sets, in F32 and
    F16. Each child's parameters before mutation (its parent's, or `map_parameters` onto its member under the
    map lanes' words) go through `MetalBirth::mutate` and must come back as the reference child's, with its
    counts. The sets: the preset's; every parameter perturbed; every parameter replaced; topology changes with
    1.5 standard deviations on 1% of weights (32 children mapped and 34 on a later candidate in each dtype); a
    replacement probability, and then a weight probability, exactly at a parameter's own uniform; and
    perturbations below half a binary16 step. 1,008 children in all, 72 per set in each dtype. Each dtype has
    946,205 replacements and 2,128,621 perturbations.
  - `rates_the_reference_refuses_are_refused`: replacement probabilities of NaN and one ulp above one, a weight
    probability of the smallest negative, and deviations of -1, infinity and NaN, each refused with its
    quantity's name, values unchanged and no counts. So are values one short of the members' parameters and one
    over, and births of no layers (and no parameters), of four, with a layer's weights one past the last
    layer's end, with biases inside the weights, and with one parameter more than the layers hold. An empty
    call succeeds.
- `tests/settle_world.rs` (FL-032, 8 tests): a world with `MetalSettle` raced against the reference world, the
  first organism watched, comparing digests and watched transactions every tick, and each tick's birth records
  and newborn genomes (the digest names genomes, not their parameters).
  - `the_preset_with_phase_6_on_metal_is_the_reference_world_at_every_tick`: the preset, 300 ticks, every tick
    on the device (report 300 and 0), with deposits (552,539), births (22, each first candidate mutated on the
    device) and deaths at chemistry (146) and maintenance (1,894). Installing the backend reserves its lists (scratch 101,233,808 to 135,050,384 bytes),
    which the run does not grow; the owner holds no leases or retiring buffers afterwards, and removing the
    backend releases its lists.
  - `worlds_whose_organisms_die_at_every_stage_are_the_reference_worlds`: the preset with motion costing 2.0 and
    emission capped at 1,000 per second (deaths 1,191, 1,154 and 935 at chemistry, maintenance and motion), and
    with emission capped at 10⁷ (3,356 at emission), 60 ticks each, every tick on the device.
  - `the_preset_with_every_metal_backend_on_one_device_is_the_reference_world`: the field, sensors, uptake and
    phase 6 on one `DeviceOwner` and the controllers on `MetalBackend`, 120 ticks.
  - `a_change_the_host_cannot_see_changes_the_world`: an organism moved one ulp further, two packets' amounts
    swapped, and a viable attempt reported as none; the worlds diverge at that tick and the backend serves every
    tick.
  - `a_failing_or_refused_settle_backend_hands_its_tick_to_the_reference_and_is_dropped`: 24 planted changes,
    one per host check: a failure after emission; one ulp more toxicity paid, or less left after chemistry; a
    gain reported as a loss; a death at motion the host does not see; a death at chemistry reported at
    maintenance; one ulp more of a maintenance bill paid, or paid to the controller; a scaled flag flipped; one
    ulp less paid for motion; a position on the torus's edge; a deposit for an organism that does not emit; one
    ulp less offered or paid for a deposit; a packet twice the offer, or holding nothing; two packets out of
    order, or in one cell; a cell one past the grid; no attempt reported as cooling down, and cooling down as no
    attempt; a viable child one ulp short; an attempt below the attempt energy reported as unaffordable, or one
    ulp short. Each world is the reference world at every tick, the backend served the ticks before and is then
    dropped, and its report names the cause. A death at chemistry reported as none is refused by the emission
    kernel itself, which takes only organisms alive after motion. The preset's founders cool down for their
    first 64 ticks, so the proposal's changes are planted in the preset with a cooldown of 0.01 s (two ticks),
    and the viable ones with an attempt minimum of 1.0 too.
  - `births_mutated_on_metal_are_the_reference_births`: the preset with that cooldown and attempt minimum, an
    architecture change in half the births, chemistry and sensing that do not mutate, and one weight in 10,000
    perturbed, so a first candidate often changes member and is often an effective no-op, in F32 and F16,
    12 ticks on the device: 150 births in each, 86 of them onto another member and 16 from a later candidate,
    every first candidate mutated on the device.
  - `a_mutation_the_host_cannot_see_changes_the_world`: the first mutated parameter's last bit flipped, and
    one replacement more counted for the first birth; the worlds diverge at the first births, and the backend
    serves every tick.
  - `a_failing_or_refused_mutation_hands_its_births_to_the_reference_and_is_dropped`: a failure, one count
    fewer than births, and more counted than a birth has parameters. Each world is the reference world at
    every tick, the backend served the ticks before the first births and is then dropped (one reference tick,
    no births mutated), and its report names the cause.

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
- FL-031's uptake: 22 planted against `tests/uptake.rs` and `tests/uptake_world.rs`, on the final code, 21 caught:
  - on the device: the demand's compensation dropped; Neumaier's branch always taken; ties rounded away from
    zero; a deficit's ties going to the higher identity; its remainders ranked smallest first; its whole passes
    dropped; the cell area not divided; the saturation dropped; the subnormal quantum's exponent not clamped; a
    floor keeping a value below one; the remainder taken as the ideal share;
  - on the host: a non-finite request, or demand, accepted; the cell partition, or the footprint's bounds,
    unchecked;
  - in flife-field: the sum check dropped; a device density not checked;
  - in the engine: a failing backend kept; its lists not reserved; the backend never called; its lists left out
    of the footprint.

  One survived, and it is equivalent: `A_c > e_c` made `A_c ≥ e_c`. At equality both branches give the target
  `n` and the scale `1/Q` exactly, and the contended flag recorded is the host's.
- FL-032's chemistry: 14 planted against `tests/chemistry.rs`, 14 caught:
  - in `softfloat64.metal`: `max` keeping −0, `min` keeping +0, `next_down` of a zero going up, `add_down` stepping
    down on a positive error, Neumaier's other branch always taken, and `tanh` losing its sign;
  - in `chemistry.metal`: a zero-energy cell composed as if it held energy, the harm summing positive
    efficiencies, the gain credited rounded to nearest, the affinity check inverted, an exhausted budget credited,
    `a·z` summed from the last channel, the toxicity never exhausting, and a negative grant admitted.
- FL-032's maintenance and motion: 22 planted against `tests/motion.rs`, 22 caught, each by a failed assertion
  (none by a shader that does not compile):
  - in `softfloat64.metal`: `sqrt` dropping its sticky bit or never evening an odd power; `fmod`'s subnormal
    result shifted one too few, equal magnitudes returning `a`, and the sign dropped; `rem_euclid` adding the signed
    period; and the wrap keeping a remainder that rounded up to the period, or a −0;
  - in `motion.metal`: a chemistry death still billed; the controller billed when not evaluated; the bill's total
    keeping a −0; the controller share rounded to nearest; the uptake ratio not clamped at one; the response norm
    unchecked; the acceleration unclamped; the clamp skipping its shrink loop; drag not applied; the motion cost
    missing `Δt`; an unaffordable motion not scaled; the displacement using the proposed velocity; `y` wrapping
    on the width; and a motion death not recorded.
- FL-032's emission: 16 planted against `tests/emission.rs`, 16 caught, each by a failed assertion:
  - in `softfloat64.metal`: `floor` truncating a negative fraction, or taking a small negative to zero; the
    integer dropping its sign;
  - in `emission.metal`: `add_up` rounding down; a zero request still staging; the offer ignoring the efficiency;
    a direction outside the unit ball admitted; a cell met twice not merged; the stencil not sorted; the largest
    weight taken as the last of equals; the remainder rounded to nearest; packets of zero amount kept; the
    payment never exhausting; the column taking the row's offset; `y` losing the half-cell offset; and the cell
    index striding by the rows.

  Four candidates were not planted, being equivalent: the request not capped at the balance (`f ≤ 1`), zero
  weights kept (their shares are zero and dropped), the remainder not clamped at zero (the largest weight is at
  least ¼), and the payment not capped at the balance (the request never exceeds the energy available, which is
  the balance).
- FL-032's phase 6 in a world: 21 planted in flife-engine's `world/settle.rs` and flife-field's `push_split`,
  against `tests/settle_world.rs`, 21 caught:
  - the backend: a failing backend kept; the replay's error ignored; rows written before the replay is known
    to succeed (the world then refuses its own tick, since the reference settles those rows again); the
    settlements never written;
  - the host's checks: chemistry's amounts, payment and balance; maintenance's payment and controller share; a
    scaled motion; motion's payment; a position on the torus's edge; a deposit for an organism that does not
    emit; emission's offer, unchecked or ignoring the efficiency; emission's payment; the backend's fate;
  - the split: cells repeating, a cell at the grid's end, a packet holding nothing, packets above the offer.

  A repeated cell survived the first round, as no planted change gave one; the repeat, the grid's end and the
  empty packet were planted after it, and the second round caught all three.
- `Settled::apply`, 7 planted against flife-engine's own tests (the race cannot see them, since both worlds
  write their rows through it): budget, velocity, position, emitted and birth intent caught. Vitality survived
  because `tests/world.rs` tallied the ecology's deaths by stage but asserted nothing about them. Its assertion
  of deaths at maintenance now catches it: without the vitality, each death is recorded at the split, its
  budget's next check. Absorbed was equivalent: nothing read it after phase 6, so the field was removed.
- FL-032's reproduction proposal: 38 planted in `philox.metal`, `proposal.metal`, `proposal.rs` and
  `Reproduction::proposal_constants`, against `tests/proposal.rs`, 38 caught:
  - Philox: a multiplier and a Weyl increment one bit off; no carry into the counter's second or last word, or
    a carry past a third word that did not wrap; nine rounds; the first new word under the second key; the
    block's second and fourth words swapped; the counter not incremented;
  - the uniform: a zero word not special-cased; the exponent off by one; 53 bits kept; the fraction shifted
    one too few;
  - the kernel: an exhausted budget not refused; the cooldown including its deadline; a probability above one,
    or below zero, admitted; an attempt when the uniform equals the probability; the draw from the block's
    second word, or from draw one; a negative balance, or a fraction of one, admitted; a balance at the attempt
    minimum below it; the remainder rounded to nearest; a zero remainder affordable; the parent's share taken
    from the balance; the parent's minimum checked first; a child below its minimum reported as a parent;
    below the attempt energy reporting the minimum;
  - the port: the attempt lane's index, or the stream's code, off by one; the child's and parent's minimums in
    each other's slots, or swapped by the engine; a viable split's shares read swapped; an unknown row read as
    no attempt; an infinite constant admitted; the exhausted flag not sent; the seed not the world's.

  The uniform equal to the probability and the balance at the attempt minimum survived the first round, as
  the generator had neither edge; it now proposes at the attempt's own uniform and one ulp above it, and at the
  attempt minimum and one ulp below it, and the second round caught both. The swapped minimums are caught only
  under the fourth rule set, whose child's minimum differs from the parent's.
- FL-032's proposal in a world: 11 planted in flife-engine's `Device::propose` and the rows it sends, against
  `tests/settle_world.rs`, 10 caught: the cooldown not checked; any outcome passing during a cooldown; a
  cooldown outside one read as no attempt; the split not compared; a viable child's share not compared; an
  unviable attempt's value not compared; the host splitting with another fraction; the backend proposing for
  the next tick, from half the balance, or with no cooldown. The host's probability check survived: no world
  produces a probability outside [0, 1] (the controller decodes it into that range), and the kernel refuses
  one itself. It is kept because it mirrors the order in which `propose` refuses.
- FL-032's parameter mutation: 46 planted in `numerics.metal`, `birth.metal` and `birth.rs`, against
  `tests/birth.rs`, 45 caught:
  - the arithmetic:
    - exp: overflowing from 709; +0 negated to -0; the short series of x, not -x; the reduction without ln 2's
      low part;
    - ln: a subnormal's exponent off by one; the mantissa folded at √2 itself; the series one term short; the
      parts summed in another order;
    - an integer's sign dropped, or its exponent off by one;
    - the second quadrant's cosine keeping its sign; a half quarter taking the complement; the quarter-turn
      series one term short;
    - open_unit keeping 53 bits; Box–Muller's components swapped, or its radius from +2 ln u;
    - binary16: ties away from zero; the subnormal shift, a subnormal's exponent or the rebias off by one;
      another NaN pattern;
  - the kernel: a child's first parameter taken as the previous child's; a weight's output and input swapped;
    a bias drawn on the weight lane, or with its fan-out as its fan-in; the candidate shifted into the kind;
    the perturbation from draw 2, decided by word 1, or taking Box–Muller's component 1; the replacement drawn
    from word 0; a limit of √(2/fan-in); a replacement, or a perturbation, when the uniform equals the
    probability; the last layer never mutated; the first bias taken as a weight; an F16 read as F32 bits; every
    result rounded to F32;
  - the host: the birth stream for the mutation stream; every dtype sent as F32; every child's first parameter
    at zero; the two probabilities in each other's slots; a negative deviation, or a probability outside
    [0, 1], admitted; the counts swapped; values longer than the members' parameters admitted.

  binary16 overflowing one binade late survived. It is equivalent: a value of exponent 16 that passes the
  first check reaches the check after rounding's carry, which also returns infinity. flife-core's
  `F16::from_f64` makes both checks, and the kernel mirrors it.
- FL-032's parameter mutation in a world: 28 planted, against flife-genome's `tests/mutation.rs`, this crate's
  `tests/birth.rs` and `tests/settle_world.rs`'s mutation tests:
  - flife-genome's `Mutator` (9, 9 caught): `propose_prepared` proposing as `propose`; a prepared candidate of
    another member admitted; its counts swapped; a mapped candidate recording no transition; every candidate
    taken as prepared; `unmutated` laying out candidate 1; values one short admitted in F32, or in F16; values
    of another dtype admitted;
  - the port's checks here (6, 6 caught): births of no layers, or of more than fit; a layer's weights, or its
    biases, not checked; the layers' end not checked; F16 values not mutated;
  - flife-engine's phase 8 (13, 12 caught): the rates' skip inverted; a count per birth, or the counts' bound,
    not checked; a failed backend kept; the device's counts not taken; the rows naming candidate 1; the
    prepared candidates ignored, or the first given to every parent; the mutated births, or a failed
    mutation's reference tick, not counted; another seed; every world staged as F32.

  The first round had 28 and 5 survived. A prepared candidate's length and dtype were checked twice, before
  its buffer was allocated and when its values were copied; the copy abandons the buffer on a mismatch, so
  nothing is charged either way, and each check hid the other's mutants. The first check is gone, and an F16
  parent's short values and an F32 parent's F16 values are now refused in the test too. A birth of no layers
  was refused only by the layers' end, as the test's had parameters; the test's now has none. The prepared
  candidates not sorted by parent survives and is equivalent: rows are in identity order, so a tick's requests
  arrive sorted, and `resolve_births_with`'s binary search could only miss a parent, whose birth the reference
  would then propose bit for bit. The sort is kept because `resolve_births_with` documents sorted input.
- FL-032's phase 9: 23 planted against `tests/reduce.rs` and `tests/field_world.rs`, 22 caught after one round
  of fixes:
  - `reduce.metal` (12, 12 caught): rounding to an F32 never stepping down; an overflow left infinite; a
    drained cell keeping its moments; the debit's ratio inverted; the debit's energy, or the deposits' energy,
    or what a cell received, not written; the total, or `before + total`, rounded to nearest; additions of zero
    applied; the direction stride halved; a packet's deposit index misread;
  - `reduce.rs` (6, 5 caught): debits that granted nothing sent; the fault word ignored; the grants, or the
    recycling, left unapplied; the field not read back. Dropping the check that debits arrive in strictly
    ascending cells survives: flife-field sorts them, so no input reaches it. It stays as the guard that no two
    threads write one cell;
  - flife-field (3, 3 caught): empty cells counted in a summary; the debit never marking the grants; the
    reference reducing the first set twice;
  - flife-engine (2, 2 caught): a failed reduction keeping the backend; reductions not counted.

  The first round had 23 and 3 survived. Rounding the total, or `before + total`, to nearest instead of down
  changes an F64 by an ulp, which random cases never carried across an F32; the two boundary sums above now
  do. The zero-total guard in `deposits_reduce` was not planted, since only positive amounts are staged, so an
  occupied cell's total is positive.
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

**Shard size (FL-026).** Whole ticks of the preset, `tick_throughput -- W 20 8 40 <choices>`, each row one
process, M5 Ultra under other load (load average 28–106), 2026-11-10. Every digest was equal, nothing was deferred,
and every number shown varied by at most 5% (cv in parentheses). `+slots=N` caps a shard at `N` genomes; the
default holds the preset's population in one shard.

| W | reference | `torch` | 64 slots | 256 slots | 4,096 slots | default (`SHARD_BYTES`) |
|---|---|---|---|---|---|---|
| 4 | 51.9 ticks/s (2.8%) | 66.5 (1.2%) | 45.1 (2.1%) | 56.9 (2.8%) | 69.7 (3.7%) | |
| 4 | 49.4 (2.3%) | 61.3 (1.9%) | | 56.7 (2.0%) | | 64.0 (3.9%) |
| 8 | 69.3 (3.8%) | 83.3 (3.6%) | | 58.7 (3.3%) | | 78.6 (4.5%) |

Each extra kernel cost 0.1–0.2 ms a tick. FrankenTorch encodes a command's kernels in one serial encoder, so each
waits for the last. Against 256-genome shards (16 kernels a tick), the default is 1.13× at four workers and 1.34× at
eight. At one worker the 256-slot samples varied by more than 5% in both runs, so that ratio is not claimed.

**Resident bandwidth (FL-030, plan §19.2).** `examples/resident_bandwidth.rs` gives the engine path
(`MetalBackend::evaluate`) `N` distinct genomes resident in its shards. Every output is compared with the
reference, bit for bit. Each warm batch's device time (`last_command`, the command buffer's GPU time) becomes the
weight bytes it streamed per second. A plain streaming read of the same bytes on the same device, alternated with
it round by round, gives the ceiling. 21 rounds each, M5 Ultra, 2026-11-10. Other GPU work shared the device (load
average about 20, device times varying by 27–74%), so the fastest round is the better estimate of capability,
and the medians show contention. Every output was identical and none was deferred:

| Topology (parameters) | Genomes | Weights | Kernels | Weight stream, fastest / median | Plain read, fastest / median | Fastest fraction |
|---|---|---|---|---|---|---|
| `[64, 32]` (9,988) | 5,000 | 0.20 GB | 1 | 449 / 441 GB/s | 956 / 911 GB/s | 0.47 |
| | 20,000 | 0.80 GB | 3 | 512 / 473 | 980 / 906 | 0.52 |
| | 80,000 | 3.20 GB | 12 | 564 / 276 | 1,049 / 359 | 0.54 |
| | 160,000 | 6.39 GB | 24 | 748 / 572 | 1,082 / 812 | 0.69 |
| `[128, 96]` (29,316) | 2,000 | 0.23 GB | 1 | 426 / 414 | 969 / 909 | 0.44 |
| | 10,000 | 1.17 GB | 5 | 449 / 378 | 994 / 934 | 0.45 |
| | 40,000 | 4.69 GB | 18 | 719 / 493 | 1,082 / 921 | 0.66 |

Against §19.2's bank (four billion F32 parameters, 16 GB a sweep), 450–750 GB/s is 28–47 full sweeps a second in
the fastest rounds, and 17–36 at the medians. So the F32 bandwidth of the 15- and 30-sweep rows (240 and 480 GB/s)
is reached. The 60-sweep row's 960 GB/s is about this device's plain read ceiling, out of reach for F32 weights
with this kernel; F16 weights would halve it. A batch's wall time is 1.5–2.9 times its device time: the host
stages inputs and job words and reads the outputs back (160,000 genomes: 30.7 ms wall, 11.2 ms on the device).
That is the controllers alone; §19.2's sensors, fields, births and deaths are additional.

**F16 shards (FL-030).** `resident_bandwidth -- 20000,80000 64,32 21 f16` holds the same binary16 genomes on two
backends. One widens them into F32 shards, which is how F16 genomes ran before half shards; the other keeps them in
half shards. The two alternate round by round, each going first in every other round, beside the plain read. Every
output of both equalled the reference in every round, and none was deferred. The half shards are half as many (2
against 3, 6 against 12), so a batch runs half the kernels. The gate, declared before the half kernel was written,
was a cv of at most 5% on both sides. The device was shared (load average 20–22), and the cv was 41% and 56% at
20,000 genomes and 17% and 20% at 80,000, so there is NO ADMISSIBLE RATIO (2026-11-12). For orientation only, not a
claim: the median batches took 1.82 → 1.22 ms and 7.41 → 4.82 ms (1.49× and 1.54×). The medians of the paired
rounds' ratios were 1.32× and 1.45×, and the fastest rounds 1.31× and 1.41×. The half shards streamed 326–332 GB/s
of weight bytes, against 431–439 GB/s for F32, so halving the bytes did not halve the time. At these sizes the
kernel is not bound by its weight bytes alone (see the unexplained remainder under the known gaps).

Negative evidence: storing each layer's weights `[in][out]`, so that a threadgroup's lanes read adjacent words, was
measured in one process against the kernel's `[out][in]`. One template instantiated both orders; the two backends
and the stream alternated over 21 paired rounds. Every output was identical. The paired `[out][in]`/`[in][out]`
device-time ratio's median was 1.02, 0.93 and 1.04 at 5,000, 20,000 and 80,000 genomes (cv 32–100%), and the
fastest rounds differed by at most 5%. That is neutral, so it was not kept. Each lane reads its own row
sequentially, which the caches already serve well.

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

**Uptake's frontier (FL-031).** Whole ticks of the preset (`tick_throughput -- 1 20 8 40 cpu,cpu+uptake` and
`-- 16 20 8 40 metal,metal+uptake,metal+sensors+field+uptake`, in rotating 20-tick blocks in one process, medians
of 8 blocks after 40 warm-up ticks, the same digest after every block), on an M5 Ultra under other load (load
average 27 at the end), 2026-11-10:

| workers | incumbent | uptake on Metal | field, sensors and uptake on Metal |
|---|---|---|---|
| 1 | reference 24.9 ticks/s (cv 1.9%) | 23.8 (cv 2.4%), 0.95× | |
| 16 | Metal controllers 83.1 (cv 3.6%) | 74.6 (cv 4.4%), 0.90× | 58.3 (cv 3.1%), 0.70× |

Every row is admissible (cv ≤ 5%), and the device path is slower in each. It was not profiled, so the causes are
hypotheses. The device does the cells' arithmetic, but the host still takes every footprint and stages and groups
every packet, as the reference does. Two synchronous commands are added, with the energy plane, 4,096 footprints
and about 16,000 packets uploaded and the results read back. So nothing selects it. Like phases 2 and 3, it pays
only once the field stays on the device between phases (FL-032).

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
  decoding, chemistry, uptake and the field step stay on the CPU (FL-032 moves them; a world given `MetalSettle`
  runs chemistry, maintenance, motion, emission and the reproduction proposal on the device, its host
  replaying each payment and recomputing each attempt's split, and phase 8's mutation of each birth's first
  candidate). On
  many CPU workers the tick
  is bound by those, so Metal does not shorten it there.
- FL-030: the resident-bandwidth benchmark against §19.2's weight-stream bound is above, for F32 weights. Per-dtype
  parity covers F32 and F16; the quantized I8 profile does not exist. The kernel
  reaches 44–69% of a plain read in its fastest rounds, and nothing measured explains the rest: it is not
  profiled. Each per-product subnormal check is a candidate, as are the 64-lane threadgroup that the 32- and
  36-wide layers half fill, and one barrier per layer.
- FL-043: the budget bounds the device's peak, but shards are still not freed when their genomes die, so a
  crash keeps the room they took.
- Parameter shards are never freed before `forget`: after a population crash, device memory stays at the peak in
  whole shards. A topology's first genome allocates a whole 256 MiB shard, so the five catalog members can hold
  1.25 GiB however few their genomes. Backend memory is not yet part of admission (FL-043).
- A command's kernels run serially, so a population past one shard per topology pays 0.1–0.2 ms a tick per extra
  shard. Dispatching shards concurrently, since they write disjoint rows, would remove that; FrankenTorch's batch has
  only a serial encoder.
- `auto`'s Metal ceiling (8 workers) was measured against the reference, not against `torch`. On the FL-026 tree at
  eight workers, `torch` ran 1.06× faster than Metal in one admissible row. The ceiling against `torch` is not
  re-measured. FL-029's batched submission is now in place.
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
- FL-031 gaps:
  - Phases 2, 3 and 5 run on the device, bit for bit. A surplus is never apportioned there (no valid input makes
    one; see the uptake invariants).
  - The field is not resident: phase 2 uploads and reads it back every step, in two synchronous commands; phase 3
    uploads it again and reads every sample back (3.2 MB on the preset) in a third; and phase 5 uploads the energy
    plane again, with the footprints and packets, in two more. Residency needs the field kept on the device between
    them, and the observation encoding on the device to keep the samples there (FL-032).
  - Measured slower than the CPU on the preset (above), so nothing selects it by default: no CLI or Studio
    choice exists yet, unlike `ControllerChoice`. The engine ports, these tests and `tick_throughput` are its only
    users.
  - The host sums stay serial on the CPU by design. A device reduction would need a fixed tree, which is a
    different numerical profile (§11.3). Phase 5's per-cell compensated sums run on the device in the reference's
    order, one thread per cell, so a hotspot cell is one thread's work.
  - The world's invariant checks (about 10% of a one-worker tick in a profile) run on the CPU over the read-back
    field.
- FL-032's phase 9 gaps:
  - The field is not resident here either: each reduction uploads the whole candidate field and reads it back.
  - Its packet, direction and debit buffers grow during a tick when a reduction needs more (doubling), rather
    than being reserved with the population's capacity (FL-043).
  - The kernels fault on a non-finite value the reference would carry through (a NaN or infinite moment in a
    cell that is debited without draining, or that receives a deposit), handing that reduction to the
    reference. Admissible fields hold none, by INV-01.
  - Two checks cannot be reached through the public API: debits out of ascending cell order (flife-field
    sorts them; a GPU race if two threads shared a cell) and a cell with packets whose total is zero (only
    positive amounts are staged). Both are kept as defenses, and their mutants are not planted or survive.
- No CUDA backend exists: FrankenTorch has no CUDA crate, and no host here has an NVIDIA GPU.
