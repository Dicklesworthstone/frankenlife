# flife-backend-cpu contract

**Purpose.** FL-025 (`flife-ar6`): the qualified CPU route for controller inference (plan §10.1, §10.2, §12.5,
§22.4). Every due organism's own dense controller is evaluated through FrankenTorch's serial population kernel,
`ft_kernel_cpu::population_affine_f32_into`, bit-identical to `flife_neural::ReferenceController`, on threads the
world budgets. The kernel is the upstream deliverable §22.4 names ("controlled-executor CPU slice kernels"). It is
in frankentorch, added for this item. This crate is `forbid(unsafe_code)`.

**Inputs.**
- A batch of `flife_neural::ControllerJob`s from the engine (`ControllerBackend::evaluate`): each organism's genome
  id, its arena slot (FL-026: topology bucket, slot, generation), its admitted controller (topology and parameters)
  and its 104 F64 observation inputs.
- A lane count, at construction: the world's `cpu_workers`.
- The composition root selects it with `--controllers torch` (`flife run`, `render`, `resume`, `experiment` and
  `flife-studio`): `flife_backend_metal::ControllerChoice::Torch`, whose `backend(cpu_workers, budget)` gives this
  backend on that many lanes, on any host. `auto` does not take it (see Measurements).
- FL-043: a `flife_neural::BackendBudget` (bytes, and the organisms the world can hold), at construction:
  `CpuBackend::with_budget(lanes, shard_slots, budget)`. `new` and `with_shard_slots` are unbounded. The CLI grants
  what its preflight leaves (`flife-cli`'s `memory`).

**Outputs.** One entry per job, in batch order: its 36 raw outputs, or `None` for a deferred job, which the world
evaluates on the reference.

**The arithmetic.**
- Each input is rounded once to F32.
- Each layer is one kernel call. Output `o` is the bias, then each input's product in ascending index, in F32.
  Every product and every sum is rounded, and there is no fused multiply-add: the reference's declared order.
- The hidden activation is `flife_core::numerics::tanh` of the F32 pre-activation in F64, rounded once to F32.
- The output layer is linear.
- The CPU keeps subnormal results as the reference does, so, unlike the Metal backend, nothing is deferred for
  being small.

**Deferral.** A job is `None` when the reference would refuse it:
- an input that is not finite in F32;
- a pre-activation `tanh` refuses;
- an output that is not finite.

It is also `None` when its shape is not one this backend takes:
- not 104 inputs in, 36 outputs out, and each layer feeding the next;
- more than `MAXIMUM_HIDDEN_LAYERS` = 2 hidden layers;
- a parameter count other than its layers'.

The catalog's topologies all qualify.

**Executor** (plan §3.4, §12.5). The kernel runs on its caller's thread. It touches no rayon pool, spawns nothing
and allocates nothing. `CpuBackend` splits a batch into at most `lanes` contiguous runs of at least
`MINIMUM_JOBS_PER_LANE` = 128 jobs. The first runs on the calling thread; the others run on scoped threads it joins
before returning. The controllers of a tick therefore use at most `lanes` threads, the world's `cpu_workers`, and
`threads_used` reports how many the last batch ran on. The engine calls the backend once a tick from its own
thread, between its lane phases, so the backend's threads never run alongside the engine's. The result does not
depend on the split.

**Residency.** A genome's parameters are packed once, each layer's `weight[out][in]` transposed to
`[in][out]`: the kernel's `[B, I, O]` with `B = 1`, so its output loop reads contiguous memory. FL-026: each
topology has a bucket of fixed-size shards, `SHARD_SLOTS` (256) genomes to a shard unless `with_shard_slots` says
otherwise, and at most `MAXIMUM_SHARD_ELEMENTS` (2^28) values. A job's `ArenaSlot` locates its packed set
(`ShardGeometry::locate`: shard, offset and extent, checked in 64 bits). The set is used only while its slot holds
the same generation and genome, so a newborn at the next generation of a dead genome's slot is packed in its
place. A batch drops nothing: a genome due one tick in K is not packed again when it is next due. A shard is
reserved at its full size when first touched, so memory follows the peak population in whole shards until
`forget`.

FL-043: a shard, its parameters and its occupants' records alike, is reserved whole when it is made, and is made
only if it fits in what the budget leaves after the shards already held and the lanes' activation scratch. A job
whose shard would not fit is deferred, and the world evaluates it on the reference, which gives the same bits. So
a small budget moves work to the reference and never changes a result. `resident_bytes` is the shards' and the
lanes' scratch's capacities.

**Independent tests.**
- In frankentorch, `crates/ft-kernel-cpu/tests/population_affine.rs` tests the kernel itself.
  - Every output of FrankenLife's layer shapes (`[37, 104, 64]` to `[1, 1, 1]`) equals an independent scalar loop
    in the declared order, bit for bit. The same data tells a fused multiply-add and a bias added last apart.
  - Each member uses only its own weights, and any split of the batch agrees.
  - Shapes are checked, and empty shapes give the bias.
  - It finishes a 512-member batch while every rayon worker is held. In the same test, a rayon parallel loop cannot
    finish, which shows the pool really was held.
- `tests/forward.rs`:
  - 2,000 jobs over all five catalog topologies. They are typical, saturating, subnormal, hidden-overflow,
    output-overflow and wide-input (an input finite in F64 and infinite in F32).
  - Every reference success is the backend's output bit for bit, and every reference refusal is deferred. All
    three refusals are exercised, and subnormal organisms are evaluated, not deferred.
  - On 2, 4 and 16 lanes, the outputs are the same bits.
  - Residency by slot (FL-026), in shards of 16: 300 genomes over five topologies are packed once. Batches
    of a few pack nothing and drop nothing. A slot at its next generation, holding another genome with other
    values, is packed in its place and gives that genome's reference outputs, the other slots unchanged. In one
    batch naming a slot at both generations, the last one packed is served and the other deferred, never given
    its outputs. Another genome at the same slot and generation is another occupant. A job naming a bucket made
    for another topology is deferred. `forget` empties it.
  - Shard sizes 1, 2 and 3 and slots from `u32::MAX - 7` and from `2^40` give the same bits as the reference.
  - A batch runs on at most its lanes and on no more than one thread per 128 jobs (`threads_used`, counted from
    the threads it actually started): 2,000 jobs on 1, 4, 16 and 64 lanes use 1, 4, 16 and 16; 300, 128 and 129
    jobs on 16 lanes use 3, 1 and 2. Every split gives the same bits.
  - It evaluates 1,024 jobs on four lanes while every rayon worker is held, with the same blocked control.
- `tests/world.rs`:
  - The real 4,096-organism preset runs 400 ticks with its controllers on this backend, on the world's lanes (up
    to 16). Its digest equals the reference world's at every tick, through the founder crash, births and mutation.
  - No tick fell back to the reference, and no job was deferred.
  - One flipped output bit at one tick changes that tick's digest.
  - At an evaluation interval of four ticks (a batch holds about a quarter of the live genomes), 400 ticks equal
    the reference world at every tick, and every genome, newborns included, is packed exactly once while it lives
    (4,139 genomes, 4,139 packs), in shards of 256 and of 3.
  - FL-043, `under_half_its_bytes_the_backend_defers_and_the_world_is_the_reference`: an unbounded backend holds
    163.7 MB after 40 ticks of the preset; under half of that (81.9 MB), 400 ticks equal the reference world at
    every tick, with 42 births, 553,606 jobs batched and 701,150 deferred, no tick redone and no failure, and the
    backend's resident peak at 71.6 MB. Planted mutant: a shard made whatever the budget leaves fails it.
- In `flife-backend-metal`, `tests/world.rs::auto_takes_metal_only_up_to_its_worker_ceiling`: `torch` gives this
  backend on the world's workers on any host, and parses from `torch`.

**Measurements.** `cargo run --release -p flife-backend-metal --example tick_throughput -- W 20 8 40
cpu,torch,metal` steps the preset under each choice in one process: 40 warm-up ticks, then eight 20-tick blocks per
world in rotating order, the digests compared after every block. On an M5 Ultra under other load (load average
about 27), every digest was equal, and nothing was deferred. Whole ticks against the reference:

| W | `torch` | `metal` |
|---|---|---|
| 1 | 1.62× (24.5 → 39.6 ticks/s) | 2.22× |
| 4 | 1.26× | 1.32× |
| 8 | 1.10× | 1.04× |
| 16 | 1.01× | 0.94× |

At 8 and 16 workers the samples varied by more than 5%, so those rows show no admissible difference. A second
two-way run gave 1.61×, 1.29×, 1.10× and 1.04×/0.99× at 1, 4, 8 and 16. None of this is profiled. The likely
reasons: with weights packed `[in][out]`, the kernel's output loop can run in SIMD lanes across independent outputs,
which the reference's per-output dot product cannot do without reassociating; and with more workers the controllers
are a smaller part of a tick, while the batched path adds passes (three lane splits a tick instead of one, a serial
pass building the jobs). On one worker Metal is clearly faster than `torch`. On four it led by 5% while
its own samples varied by 5.1%. So `auto` keeps Metal where the host has it.

On Linux x86-64 (RCH workers, 2026-11-10, `tick_throughput -- W 20 8 40 cpu,torch` on the clean tree of commit
974eb02), each row is one invocation on one worker. All digests were equal and nothing was deferred:

| W | reference | `torch` | worker |
|---|---|---|---|
| 1 | 10.96 ticks/s (cv 0.33%) | 15.46 (cv 0.36%), 1.41× | hz4 (64 CPUs) |
| 4 | 24.10 (cv 3.28%) | 26.75 (cv 4.48%), 1.11× | ts1 (64 CPUs) |
| 8 | 28.97 (cv 3.47%) | 26.31 (cv 2.89%), 0.91× | hz3 (64 CPUs) |

The rows ran on different workers, so only the ratio within a row compares. Every row is admissible. `torch` is
faster at one and four workers on both hosts and slower at eight on Linux, so `auto` takes it where it does not
take Metal, at up to `AUTO_TORCH_MAX_WORKERS` = 4 workers (`flife-backend-metal`). Two and three workers were not
measured. The ceiling assumes that they lie between one and four, where the ratio fell from 1.41× to 1.11× on
Linux and from 1.62× to 1.26× on macOS.

**Validity domain.** The `dense-104-36-v1` catalog's shapes, the reference F32 profile, any CPU Rust targets.

**Known gaps.**
- The kernel is called once per organism over that organism's packed weights. FL-026's shards hold a topology's
  genomes side by side, but a batch's due organisms are not adjacent slots (some are not due, some slots are
  free), so one call over several needs a gather or a whole-shard call. The kernel already takes `[B, I, O]`.
- Shards are never freed before `forget`: after a population crash, memory stays at the peak in whole shards.
  FL-043's budget bounds that peak, but a crash keeps the room the dead genomes' shards took, so later shards of
  other topologies may be deferred.
- Packing a newborn's parameters is a copy and a transpose, once per genome.
- `auto` takes this backend only on a host without Metal, at up to four workers. The crossover between four and
  eight workers is not measured more finely. The loss at eight is not profiled; the next gap gives the hypothesis
  for many workers.
- At 16 workers the batched path is no faster than the reference's own lanes. Observing on the lanes, building the
  jobs, evaluating and deciding each split the batch again, so a tick pays for three sets of threads.
