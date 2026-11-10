# flife-engine — contract

**Purpose.** FL-006 (`flife-h6c`) covers the organism phase's reference operators (plan §5.3, §5.4, §5.6, §8.2,
Appendix C.2, C.3, C.9, C.11, C.12):
- `bounded-affinity-v1` chemistry evaluated `per-granted-cell-packet`, with toxicity clipped to what the
  organism holds;
- the mandatory maintenance of one tick;
- field-mediated motion: drag integrated with a stable `expm1`, speed and acceleration limits, affordability
  scaling of the whole velocity, and the wrapped position step.

FL-011 (`flife-t24`) adds paid emission and the reproduction transaction (plan §5.5, §7.4, §7.5, §8.1 phases 7
and 8, §8.2, INV-05, INV-06, A.3 item 10, Appendix C.4, C.5):
- emission paid from the post-motion budget and staged as a next-tick deposit at the post-motion position
  (`no-emission-no-charge`, `post-motion`, `next-tick`). The deposit reduction itself is
  `flife_field::deposit`;
- phase 7's reproduction proposal: cooldown eligibility, one semantic Bernoulli attempt per parent and tick, and
  viability on the post-emission budget with an exact split;
- phase 8's birth transaction: an early live-population slot and logical genome quota check, the mutated
  child from `flife_genome::Mutator`, identities issued in stable parent order, a uniform-by-area child offset,
  and all-or-nothing rollback (`next-tick` newborn activation).

FL-012 (`flife-3zm`) adds death, recycling, live storage and stable identity (plan §4.4, §7.6, §7.7, §8.2,
§9.1, INV-01, INV-06, INV-07, A.1, Appendix C.7, C.12):
- `lifecycle`: exhaustion rechecked after every §8.2 budget stage, `dissipate-and-record` tiny-reservoir clearing
  as the last stage, deaths collected once with their cause at the resulting position, and `genetic-affinity`
  recycling through `flife_field::Deposits` under `Transfer::Recycling`;
- `population`: the live organisms as a structure of arrays behind an ordered stable-identity index, in a
  `Compact` or a `FreeList` slot arrangement, with transactional commits of one tick's deaths and births;
- `costs`: the mandatory cost split by ledger flow (`maintenance`, `sensor-cost`, `controller-cost`), with a
  declared order for a partial payment.

FL-013 (`flife-x3o`) adds `world`, the ordered candidate tick and epoch publication (plan §7.5, §7.6, §8.1–§8.4,
§9.4, §9.5, INV-01–INV-08, A.3 item 1, Appendix C.7, C.8):
- `World::found`: a world at tick zero from a validated configuration, with its founders (`uniform` positions,
  `zero` velocity) and seeded sources;
- `World::advance`: phases 2–10 of §8.1 against the committed state, giving a checked `Candidate`, or a pause
  under `pause-before-commit`;
- `Candidate::publish`: the population's whole-tick commit (`Population::commit_tick`), then the field, ledger,
  quota, identities, source paths, tick, epoch and event cursor together. Dropping or cancelling a candidate
  discards it;
- `stop-and-preserve`: a numerical fault stops the run with the committed state kept.

FL-014 (`flife-fp2`) adds `intervention` and `World::advance_with`: phase 1's sealed, admission-ordered
interventions, applied with phase 2's external sources and recorded in the ledger. The session owner that admits
and seals them is `flife-runtime`.

FL-022 (`flife-2jj`, the world-space half) adds `Intervention::RemoveOrganisms`: every organism in a disk on the
torus dies at the boundary, its energy removed, dissipated or recycled as the command says (plan §7.6, §12.3,
§14.3). It is the perturbation FL-048's H4 boundary test needs. The native brush input that would produce it is
still FL-022's.

FL-016 (`flife-gh0`) writes and restores the whole world as ten checkpoint chunks (plan §16.1, §16.3, INV-14,
Appendix B.2, C.13).

FL-041 (`flife-rln`) adds two ablation models (plan §15.3, §21.6), each `field-ecology-v1` with one rule changed
and its own registered model name, so an ablated run never shares a model identity with the baseline:
`field-ecology-v1-chemistry-only` (decisions decode from an all-zero output) and `field-ecology-v1-fresh-offspring`
(a newborn's genome is the one a founder would have at its identity).

It runs on the scalar CPU reference (`cpu-reference`) in the `qualified-reference` determinism class. The other
modules compute settlements, proposals and staged births and store the committed organisms; `world` decides when
each runs and owns the field.

**Inputs.**
- A `ValidatedConfig` (`[chemistry]`, `[costs]`, the timestep), or the constants directly.
- An organism's `Affinity` and `MotionResponse`, each within its run's norm limit.
- The granted cell packets of one tick: FL-005's `GrantPacket` amounts and each cell's composition, read from the
  observation snapshot with `composition` before the field is debited.
- The organism's `flife_core::EnergyBudget`.
- The organism's normalized sensor gradients (33 two-vectors: energy, then 32 channels). Sensing is FL-007; the
  tests supply fixed gradients.
- FL-011: `[emission]`, `[reproduction]`, `costs.birth_energy`, `population.maximum_count` and the controller
  interval; one tick's `flife_neural::DecodedActions`; the world's `SemanticRng`; each parent's
  `ParentState` (identity, lineage, generation, post-motion position, cooldown deadline), post-emission budget and
  genome; the run's `GenomeQuota`, `Identities` and `Mutator`.
- FL-012: `world.tiny_energy_policy` and its threshold, `[death]`, `population.maximum_count`; each organism's
  budget after each stage; its resulting position and genetic affinity at death; an explicit removal's residual
  and `Disposition`; founders and staged births as `Entrant`s (an `Organism` row and its sealed `Genome`).
- FL-013: a whole validated configuration; the `Arrangement`; a `NumericalProfile` (the ledger's
  `ResidualBudget` and the field's `ConeTolerance`). `NumericalProfile::reference_f32` is §21.4's initial F32 gate:
  `1e-5` per tick and cumulatively, and `1e-5` of a cell's energy for the cone, each above the smallest normal F32.
- FL-041: the configuration's `model`, read by `ModelVariant::of`: `field-ecology-v1`, or one of the two
  ablation models.

**Outputs.**
- `Chemistry::efficiency`: `η(z) = η_max tanh(k a·z − b)`.
- `Chemistry::settle` and `settle_efficiencies`: an `UptakeSettlement` with absorbed `U`, useful gain `G`, and a
  toxicity `Payment` holding `T_req` and the paid `T`. `net() = G − T` and `dissipated() = U − G + T`.
  `record` enters an uptake transfer of `U` crediting `G` (its loss charged to `uptake-inefficiency`) and the
  paid toxicity.
- `MaintenanceCosts::mandatory`: `Δt (c_basal + c_sensor s / s_unit) + c_neural P × evaluated`, the total of
  `MaintenanceCosts::bill` (FL-012). A `MaintenanceBill` holds the basal, sensor and controller terms.
  `MaintenanceBill::settle` pays the total once and attributes it to them, and `MaintenanceSettlement::record`
  books them as `maintenance`, `sensor-cost` and `controller-cost`.
- `Motion`: `decay = e^(−γΔt)`, `force_coefficient = (1 − e^(−γΔt))/γ` (or `Δt` at `γ = 0`), `acceleration`,
  `propose`, `settle` (a `MotionSettlement` with the proposal, the applied velocity, the cost `Payment` and
  whether it was scaled) and `displace`.
- `uptake_ratio`: `g_u = U / (Δt U_max)`, clamped to `[0, 1]`, zero at zero capacity.
- `EmissionPolicy::emit`: an `EmissionSettlement` with the budget at the emission stage, the cost `w = f E` (always
  paid in full) and the offer `ρ w`, staged in a `flife_field::Deposits`.
- `Reproduction`: `attempts` (the Bernoulli draw), `child_offset`, `split` (a `Viability`: a `Split` or the
  `Unviable` reason) and `propose` (a `ReproductionProposal`: `Exhausted`, `CoolingDown`, `NoAttempt`, `Unviable`
  or a `BirthIntent`).
- `resolve_births`: a `BirthResolution`, either `Staged(Births)` or `Paused(CapacityShortfall)`. `Births` holds
  each `Birth` (parent, split, the parent's new deadline, the `Newborn`, its sealed genome and `MutationRecord`) and
  each `RejectedBirth`. `Births::settle` returns the budgets the parents keep and records one `child-endowment`
  transfer per birth. `Births::abandon` returns every staged charge. A `CapacityShortfall` names the quota
  (`PopulationSlots` or `LogicalGenomeBytes`), the first parent it could not cover, the required and available
  amounts, every proposed parent in stable order, and how many births were covered.
- `Vitality` (FL-012): per organism, the first `BudgetStage` that exhausted it, and its `Fate` (`Survives` or
  `Dies(stage)`) once the tiny-reservoir stage has been checked.
- `TinyEnergyPolicy::clear`: the cleared energy, for the `tiny-reservoir` flow.
- `Death` (identity, genome, lineage, generation, tick, resulting position, `DeathCause`, residual), and
  `Recycling::settle`'s `DeathSettlement` (removed, dissipated, recycled and offered; `record` books the first two).
- `Population`: `admit`, `update`, `commit` (the `Collected` deaths, each with its last row and genome, for the
  caller to retire), `get`, `iter` and `ids` in identity order, `slot` and `slots`, `genome`, `latest` and
  `digest`, a SHA-256 of the canonical state in identity order without slots (`frankenlife.population-state.v1`).
  `HeldOutput` hides an invalid output entirely. `check_tick` and `commit_tick` (FL-013) check and commit a whole
  tick: survivors, deaths with their genome charges returned to the quota, and births.
- `World` (FL-013): the committed `tick`, `epoch` (a `ResourceEpoch`, zero at founding and one more per
  publication), event cursor `events`, `field`, `population`, source `paths`, `identities`, `quota`, `ledger`,
  `fault`, and `digest`, a SHA-256 of the scientific state (`frankenlife.world-state.v1`: tick, every field value,
  the population digest, the identity sequences, the quota, the path ordinals and the ledger holdings; slots,
  epoch and cursor excluded). `advance` returns an `Advance` (`Ready(Candidate)` or `Paused(CapacityShortfall)`),
  and `step` advances and publishes.
- `Candidate`: `report`, `field` (the next field), `base` (the committed tick and epoch), `cancel` and `publish`.
- `TickReport`: the input and published ticks, the epoch, the event range (deaths, then births, each in identity
  order), live count, evaluations, emitters, viable attempts, `BirthEvent`s (child, parent, genome, mutation
  record), rejected births, deaths, the uptake summary, phase 2's `FieldStepReport`, the next field's
  `FieldSummary`, the tick's `TickClosure` and the run's `LedgerReport`. `NumericalFault` holds the input tick and
  error of the advance that stopped the run.
- `founder_position`: words 0 and 1 of `(founders, organism, 0, lane (0, position, 0), 0)` as half-open uniforms
  scaled to the world.
- FL-014: `Intervention` (`InjectEnergy { cell, amount }`) with `check` and `declared_energy`;
  `World::advance_with` and `World::step_with`; and each `TickReport`'s `interventions`, the
  `AppliedIntervention`s (input tick, sealed position, intervention, applied energy) in sealed order.
- FL-022: `Intervention::RemoveOrganisms { center, radius, disposition }` and `Intervention::removes_at(grid,
  position)`; `AppliedIntervention::removed`, the organisms a removal removed (zero for an injection), with
  `applied` the energy they held; `Disposition::ALL`, `name` (`remove`, `dissipate`, `recycle`) and `from_name`.
- FL-041: `ModelVariant` (`Baseline`, `ChemistryOnly`, `FreshOffspring`) and `ModelVariant::of(model)`;
  `reproduction::Offspring` (`Inherited`, or `Fresh { catalog, limits, initial_scale }`), the new `offspring`
  field of `BirthContext`.

**Invariants.**
- Chemistry runs on each granted packet, never on the request or on the mixture of packets (C.9).
- The gain is credited before toxicity is paid, so `T = min(T_req, E + G)` (§8.2, C.2). The ledger records the
  paid toxicity, never the requested one.
- An empty cell, or one whose moments cancel, has composition `z = 0`. With `b > 0` that is mildly harmful,
  and it is not special-cased (C.3).
- Maintenance comes from canonical model work only: sensing scale, parameter count and whether the controller
  ran. It never depends on wall-clock time, cache behavior, kernel padding or the backend.
- `||a||₂ ≤ affinity_norm_limit` and `||χ||₂ ≤ motion_response_norm_limit`, both checked in F64.
- `||f||² ≤ a_max²` and `||v*||² ≤ v_max²` hold as computed in F64. A vector that rounding leaves just outside is
  shrunk one ulp at a time.
- Motion is affordable. An unaffordable proposal is scaled as a whole by `sqrt(E_available / cost)`, and all
  available energy is paid, so the organism is exhausted. An exhausted organism still moves by what it paid
  for (C.12). Later stages pay nothing, and credits are refused.
- The force coefficient uses `expm1_neg`. At `γ = 1e-6`, `Δt = 1/128` it is `0.00781249996948`, where an F32
  `1 − exp(−γΔt)` is exactly zero (C.11). `γ = 0` gives exactly `Δt`, and so does a vanishing `γ`.
- An organism's outcome does not depend on the order in which organisms are presented. The integrated fixture's
  organisms and field agree bit for bit under three orders.

**Invariants — emission (FL-011).**
- Under `no-emission-no-charge`, a closed gate, a direction at or below the zero threshold, a zero fraction or a
  dead emitter emits nothing and pays nothing.
- `w = f E`, where `E` is the budget at the emission stage, after motion. A held output is decoded against
  each tick's own budget. `f ≤ 1`, so emission is always affordable and pays in full (INV-05). An emission that
  takes `E` exhausts the emitter, which dies; its deposit stands.
- The organism pays `w` and the field is offered `ρ w` at the post-motion position. Energy and every moment
  component share each cell's packet. The field never receives more than the offer, and the offer never
  exceeds the payment. The ledger debits `w`, credits what the cells received, and charges the rest to
  `emission-loss`. C.4: `w = 4`, `ρ = 0.75` puts 3 in the field and 1 in dissipation, exactly.
- Deposits are first sensed and first absorbable on the next tick, the emitter's own included.
- A refused emission (a fraction outside `[0, 1]`, a direction outside the unit ball, a nonfinite position)
  changes neither the budget nor the deposits.

**Invariants — reproduction (FL-011).**
- Phase 7 changes nothing. A dead parent reports `Exhausted`, and a parent before its deadline reports
  `CoolingDown` without a draw. An eligible parent draws word 0 of `(birth, parent, n, lane (attempt, 0), 0)`
  against `p` (`u < p`), so `p = 0` never attempts and `p = 1` always does. The draw depends on the seed,
  the parent's identity and the attempt tick only.
- Viability is checked on the post-emission budget `E`: `E` at least the attempt minimum, a positive remainder
  `R = E − c_b` (rounded down), child `β R` (to nearest) and parent `R − β R` (rounded down), each at least its
  minimum. `child + parent ≤ E − c_b` holds exactly, and what the birth dissipates exceeds `c_b` by at most
  `4 E ε`. C.5: 10, 1 and 1/3 give exactly 3, 6 and 1.
- Births are staged in ascending parent identity, whatever order the requests arrive in. Each birth reserves a
  live-population slot before any genome work, then the logical genome quota is charged before any buffer is
  allocated. Children take consecutive organism and genome identities in that order.
- A birth whose candidates are all rejected charges nothing, takes no slot, issues no identity, and leaves the
  parent's energy and cooldown alone.
- A fixed logical quota shortfall (`pause-before-commit`) or any error rolls back the whole stage. Every staged
  genome is retired, the identities are unchanged and no budget is touched. Retrying the unchanged requests gives
  the same result. A physical refusal (`allocated genome bytes`, `AllocationFailed`) is an error, never a birth
  rejection.
- A `Birth` carries identity, genome, slot and energy together (INV-06). `Births::settle` checks every parent's
  budget bit for bit against its proposal before keeping exactly the split's parent share and recording anything.
- Newborns: birth tick `n + 1`; position the parent's post-motion position plus an offset uniform by area in the
  disk, wrapped; zero velocity; zero energy-change history; an invalid held output; phase `id mod interval`;
  the parent's lineage at the next generation. Both deadlines become `n + 1 + ⌈cooldown/Δt⌉`, and nothing else
  changes a deadline.

**Invariants — death and stable identity (FL-012).**
- `Vitality` checks every stage once, in §8.2 order (chemistry, maintenance, motion, emission, split, tiny
  reservoir). The first stage that finds the budget exhausted is where the organism died, and nothing after it
  pays or acts. Payments already made keep their effect. A survivor's fate needs the tiny-reservoir stage, so no
  organism is published before it (INV-01).
- `dissipate-and-record` clears a reservoir strictly below the threshold: the whole remainder is paid and
  recorded as `tiny-reservoir`, and the organism dies of it.
- The split never exhausts a parent: its kept share has a positive minimum, and rules without one are refused.
- Natural death here is exhaustion and leaves no residual, so it recycles nothing. An explicit removal's residual
  goes to `command-removal` (`Remove`), `unrecycled-death` (`Dissipate`) or the field (`Recycle`). Recycling
  offers `f R` at the death position with the organism's own affinity as its chemistry, through a separate deposit
  reduction booked as `recycling`. The field never receives more than the offer, and the rest is
  `unrecycled-death`.
- A death is committed once. A commit refuses a death of an organism that is not alive, a second death in one
  commit, a death at another tick, and a death whose genome, lineage or generation is not the stored organism's.
- Identities are admitted once, in strictly increasing order, so a dead identity is never resurrected (INV-07). A
  reused slot gets every column rewritten. A newborn's held output is invalid and unreadable, so no stale
  controller state reaches it.
- A commit checks every death and birth before changing anything, and applies deaths and then births in identity
  order. Each birth brings the genome its row names (INV-06). A refused commit or admission changes nothing and
  hands the entrants back, so their quota charges can be returned. Columns are reserved to capacity, so a checked
  commit neither fails nor allocates.
- `update` changes only position, velocity, energy, energy-change rate, held output and cooldown deadline. A
  stored organism always has finite state and strictly positive energy, and a fault is refused by identity, never
  stored or collected as a death.
- The two arrangements give the same rows, iteration order and digest after every commit. Only `slot` and
  `slots` differ (C.7).
- Maintenance is one payment of the bill's total. A partial payment covers basal, then sensor, then controller,
  and the attribution rounds down, so the recorded flows never exceed the payment.

**Invariants — the ordered tick (FL-013).**
- Phases run in §8.1 order, once each per tick. Phase 1 admits no commands yet. Phase 2 transports the committed
  field into the spare buffer and applies each source at its mid-tick center, `(n + ½)Δt` on the segment that
  contains tick `n`. That buffer is the tick's one observation field: every organism senses it, demands from it
  and reads its packets' compositions from it before anything debits it. The ledger records `source-power` as
  the declared `Σ P Δt` and `decay` as transport's removal.
- Every organism is observed every tick. Its controller is evaluated when `(n + 1) mod K` equals its phase or when
  its held output is invalid (a newborn's or founder's first advance), stored with evaluation tick `n + 1`, and
  charged as controller cost. A forced evaluation does not shift the phase. Every tick decodes the held raw output
  for that tick and decides against that tick's budget. Age and eligibility use the input tick `n`.
- Per organism, in identity order: chemistry, maintenance, motion, emission, then the reproduction proposal at
  attempt tick `n`, with exhaustion rechecked after each. Organisms without a viable attempt then run the split
  (unchanged) and tiny-reservoir stages. The live-population quota counts them, if alive, plus every parent with a
  viable attempt. The births are staged as one transaction, the parents keep exactly their split shares and take
  the new deadline, and the parents' last two stages follow. A paused birth stage pauses the whole tick.
- Fates follow in identity order. A survivor commits its position, velocity, energy, held output, deadline and
  `(E_next − E_previous)/Δt`. A death is collected exhausted at its resulting position at tick `n + 1`, and its
  residual (zero) is settled under the death policy. Newborns join at `n + 1` and are first advanced from it.
- Phase 9 debits the grants first, then reduces the emission deposits (`emission`), then recycling (`recycling`),
  all into the next field, so new deposits are first sensed next tick.
- Phase 10 checks the next field (INV-01, INV-02 at the profile's cone tolerance), closes the tick against holdings
  measured from the next field and the survivors and newborns in identity order (INV-03 per tick), commits the
  closure to a copy of the run's ledger (INV-03 cumulative), and checks the population's whole commit (INV-01,
  INV-06, INV-07). The next tick opens at exactly those holdings, so energy that moved without a flow is refused.
- A candidate reads only committed state and writes only the spare field buffer, reusable scratch and its own
  copies of the ledger, quota, identities and paths. Until publication the committed world is authoritative and
  unchanged. Cancelling, dropping, pausing or failing leaves every committed byte as it was. A retry prepares the
  same candidate, report for report. A candidate borrows the world mutably, so it is the only writer and its base
  cannot move (§9.5).
- Publication commits the population first (check-then-apply, with the dying genomes' charges returned to the
  candidate's quota). Then it swaps the field buffers and replaces the ledger, quota, identities and paths, and
  advances the tick, epoch and event cursor. None of that can fail. A published tick's state, events and accounting
  appear together (INV-10's precondition). Every live organism has exactly one genome charge.
- `stop-and-preserve`: a `NumericalInvalid` while preparing records a `NumericalFault` with the input tick and
  stops the run. Every later advance is refused, and the committed state is the last published epoch, bit for bit.
- Organisms interact only through the field (INV-08). Each stage reads the shared observation field and the
  organism's own state, and writes only its own budget, its grants and its deposits. The only population-wide
  coupling is the declared capacity policy. Storage slots never decide an order.
- Founders (A.3 item 1): identities, genomes and lineages `0, 1, …`, uniform positions on the torus, zero velocity
  and history, the initial energy, birth tick zero, cooldown deadline `⌈cooldown/Δt⌉`, invalid held outputs and
  stable phases. The ledger's initial holdings include their endowment.

**Invariants — interventions (FL-014).**
- The interventions effective at tick `n` apply in the candidate from `n` to `n + 1`. They go into the observation
  buffer after transport and the sources, in their sealed order, so every organism senses them during tick `n`.
  `World::advance` is `advance_with` an empty set.
- `InjectEnergy` raises one cell's energy and leaves its moments unchanged, so `||m||₂ ≤ e` still holds. The cell
  is rounded down to the largest `f32` not above `e + amount`. The ledger records the change actually made as
  `command-injection`: never more than declared, and within one `f32` ulp of the cell.
- The interventions are checked before anything is prepared. A refused one changes nothing and does not stop the
  run. A non-finite cell after an admitted injection is a numerical fault like any other.
- FL-022: a `RemoveOrganisms` applies at the boundary itself, before transport. It takes out every organism whose
  committed position lies within `radius` of `center` by the minimum-image distance, the circle included. The
  removed sense nothing, ask for nothing, pay nothing and leave no offspring in tick `n`. Each dies once, at its
  committed position, with its whole energy as the residual and `Removed(disposition)` as its cause. `remove`
  records the residual as `command-removal`. `dissipate` records it as `unrecycled-death`. `recycle` offers the
  configured fraction to the field in phase 9 like any recycled residual, first sensed in tick `n + 1`, and the
  rest is the recycling transfer's `unrecycled-death`.
- An organism in two removals' disks at one boundary dies by the first in sealed order. A disk that holds no
  organism removes nothing, still applies, and leaves the tick exactly as it would be without it. The tick's
  deaths list phase 1's removals first, in sealed and then identity order. Its `interventions` list every
  intervention in its sealed position, though removals apply before injections.
- A centre outside `[0, L_x) × [0, L_y)`, or a radius that is not finite, not strictly positive or above half the
  shorter world side, is refused before anything changes. Half the world is admitted.

**Invariants — checkpoints (FL-016; §16.3, INV-14, B.2).**
- `World::write_checkpoint` writes the ten world chunks of `WORLD_CHECKPOINT_CHUNKS` at version 1, each declaring
  what it is decoded against:
  - configuration: the canonical TOML, its hash, the slot arrangement and the numerical profile's five values;
  - random (on configuration): the algorithm and seed;
  - topology (on configuration): the catalog's name and every member's widths and parameter count;
  - environment (on configuration): the tick, the resource epoch, and each seeded path's source, ordinal and
    anchor;
  - lifecycle (on configuration): the next organism, lineage and genome identities, the event count and the
    quota's logical, allocated and live counts;
  - field (on configuration): the shape and every plane's F32 values;
  - genomes (on topology): every live genome in identity order, with its topology, parent, traits and
    parameters;
  - particles (on genomes): identity, genome, parent, lineage, generation, position, velocity and energy;
  - held (on particles): each organism's held outputs with the tick they were evaluated at, its phase, cooldown
    deadline, birth tick and energy change rate;
  - ledger (on field and particles): every bit of `LedgerState`.
- No generator state is saved because there is none: every random word is addressed by seed, stream and
  counter (FL-010), so the seed and the tick determine every later draw. `SeededPath::resume` rebuilds a seeded
  path from its source, ordinal and anchor.
- `World::read_checkpoint` builds the world only from the chunks and checks each against what it determines.
  The configuration must validate to the recorded hash, and the model, profile, algorithm, seed and catalog must
  match. Paths come in source order, and the field has the configured shape and finite values. Each genome is
  re-sealed through a fresh quota, which must end equal to the recorded one. Each organism must name its own
  genome, have the phase its identity determines and a birth tick no later than the tick, and lie inside the
  torus. Every manifest item count must match its chunk. The identity allocators must be past every live
  identity. The ledger's last holdings must equal the measured field and organism holdings bit for bit, and its
  next tick must be the world's tick.
- Off-phase state survives (C.13): an organism between evaluations keeps its held outputs and their evaluation
  tick, so a world restored mid-interval evaluates on each organism's own phase, and a newborn's forced first
  evaluation stays due.
- A faulted world is refused (`StateConflict`). `stop-and-preserve`'s fault is a diagnosis, not a state to
  resume.
- Scratch buffers and the controller and field backends are not state. A restored world runs on the reference
  path until its owner calls `set_controller_backend` or `set_field_backend`, and no result depends on either.

**Invariants — ablation models (FL-041; §15.3, §21.6).**
- `field-ecology-v1-chemistry-only`: every due controller is evaluated and its cost charged exactly as in the
  baseline (same evaluations, same `controller-cost`, `sensor-cost`, `motion-cost` and `maintenance` bits on the
  first tick from one founding), and the held output is kept, but `finish` decodes every decision from
  `[0.0; ACTION_WIDTH]`. With Appendix A's decoding that is σ(0) = 1/2 everywhere: the emission gate sits at its
  threshold of 1/2 and never opens, and every birth gives the child the share β = 0.1 + 0.8 / 2 = 1/2 of what
  remains after the birth cost. Chemistry, motion physics, uptake, costs and reproduction thresholds are unchanged.
- `field-ecology-v1-fresh-offspring`: a birth stages `flife_genome::founder(rng, child, genome_id, catalog,
  quota, limits, initial_scale)`, the genome a founder would draw at the child's organism identity (A.3 items
  2–3), with an empty mutation record. Its energy, position, lineage, generation and quota charge follow the
  baseline's birth transaction. Nothing of the parent's parameters is carried except by chance (founders' zero
  biases).
- The founding is the same under all three models: the model name changes no founder.

**Refusal cases.**
- Chemistry constants outside `η_max ∈ (0, 1]`, `k ≥ 0`, `b > 0`, `κ > 0`, limit `∈ (0, 1]`, or nonfinite.
- An affinity or motion response above its norm limit, or with a nonfinite component.
- A negative or nonfinite grant, an efficiency outside `[−1, 1]`, or a nonfinite composition.
- A gain offered to an exhausted budget (`StateConflict`).
- Negative or nonfinite costs, a nonpositive sensor unit scale, or a nonpositive sensing scale.
- A sensing-scale cost exponent other than `1`: another exponent needs a host-independent power function, which
  the reference profile does not have (`MissingCapability`).
- Negative drag, nonpositive speed, acceleration or response limits, or a negative motion cost.
- An uptake ratio outside `[0, 1]`, a nonfinite gradient, velocity or acceleration.
- A configuration naming another chemistry family or evaluation (`UnknownAlgorithm`).
- FL-011: an efficiency outside `[0, 1]`; another emission position, visibility or newborn activation
  (`UnknownAlgorithm`); a nonfinite or negative birth cost, attempt minimum or offset radius; a nonpositive child or
  parent minimum; a zero controller interval; an attempt probability outside `[0, 1]`; a child share outside
  `(0, 1)`; two requests from one parent, or requests of different ticks (`StateConflict`); an exhausted identity
  sequence or a generation past `u32::MAX` (`Overflow`); a population quota with more live organisms than its
  maximum; and settling against a missing, dead or changed parent budget (`StateConflict`, nothing recorded).
- FL-012: a stage checked out of order or twice, or a survivor's fate before the tiny-reservoir stage
  (`StateConflict`); a negative or nonfinite tiny threshold, a recycle fraction outside `[0, 1]`, a nonfinite
  death position, or a negative or nonfinite residual; an exhaustion death claiming a residual (`StateConflict`);
  a recycled affinity outside the unit ball; every commit and admission refusal above; a population above its
  capacity (`BudgetExhausted`); a host-refused column (`AllocationFailed`); a nonfinite held output; and a negative
  or nonfinite maintenance term.
- FL-013: a backend, determinism class, fault policy, capacity policy, initial positions or velocity, chemical
  directions or source path other than the ones implemented (`UnknownAlgorithm`); automatic reseeding
  (`MissingCapability`); a whole-tick commit whose survivors are out of identity order, repeated, not alive, also
  dying, changing a lifelong field or failing INV-01, that leaves a live organism unaccounted for, or whose quota
  does not hold the dying genomes' charges (`StateConflict`, `NumericalInvalid`); every phase's own refusal; and any
  advance once the run has stopped (`StateConflict`). Nothing committed changes on any of them.

**Determinism.** Pure functions of their inputs. Sums run in channel or packet order and the packet sums are
compensated. The only elementary functions are `flife_core::numerics` (`exp_neg`, `expm1_neg`, `tanh`) and the
correctly rounded `sqrt`, so every IEEE-754 host computes the same bits. The integrated fixture pins a digest of
its final organisms and field. FL-011's randomness is `fnp-philox-semantic-v1` on the frozen `birthing` stream,
addressed by parent identity and attempt tick, plus the mutation stream. Staging order is ascending parent
identity, so the births do not depend on request order, worker or storage slot. FL-012 draws nothing. The
population's index is a `BTreeMap` and its free list a `BTreeSet`, so every order it exposes is identity order or
a deterministic function of the commit history. FL-013 visits organisms in identity order and runs every reduction
in one fixed order (`qualified-reference`): demand order for grants, identity order for deposits, holdings and
births. Packets are grouped by demand in cell order. It draws only on the founders', sources', birth and mutation
streams. The small ecology pins its world digest, checked on Linux x86_64.

**Independent tests.** `tests/organism_phase.rs`:
- C.2 exactly: absorbed 3, `T_req = 5`, `T = 2`, dissipation 5 closing at `ε = 0`; recording `T_req` cannot close.
  Gain bounding toxicity at `E + G`.
- C.9 to 12 digits: `+0.639355310069` per packet and `−0.047643210759` on the mixture, through both the stack
  and the heap packet paths.
- C.3: `z = −0.25a` for `(8, −2a)`, the neutral composition for cancelled and empty cells, and its mild harm.
- C.11: the drag coefficient, the F32 cancellation, the exact zero-rate limit, monotonicity in `γ`, and Appendix
  A's `γ = 2`.
- C.12: the last unit spent on motion, the step taken, one dissipated unit, and every later stage refused.
- Affordability: an opposing force cannot rescue an unaffordable residual velocity; the whole velocity scales.
- Both radial bounds over 20,000 random cases, and the response and affinity norm refusals.
- Maintenance at Appendix A: `0.008556928` per simulated second at unit sensing scale with 9,988 parameters.
- §8.2 ordering: gain before maintenance, unpaid maintenance not dissipated, and no motion after death.
- An integrated simulated second: five organisms on a 16² field with contended cells, real FL-005 grants,
  per-packet chemistry, maintenance and motion. The ledger closes every tick (cumulative residual about 1e-14,
  6.6e-16 of turnover at 48 ticks). The opposed organism dies at tick 29 and never absorbs again.

- `tests/emission_births.rs` (FL-011, 23 tests):
  - C.4 exactly against `ResidualBudget::EXACT`: 4 paid, 3 to the field, 1 to `emission-loss`, the energy and all
    32 moment components at the same four 1/4 weights. Off a corner, each cell holds its bilinear weight of the
    offer to F32 rounding, and the moment follows the energy each cell received.
  - Emission spends the decoded fraction of the post-motion budget, decoded against each tick's budget. Closed
    gates, zero directions, the dead and a zero fraction emit and pay nothing. Malformed proposals change
    nothing. Spending everything exhausts the emitter and the deposit stands.
  - Next-tick visibility: the same tick's observation and grants are bit-identical with and without an
    emission, the emitter's own included, and the next tick sees it. Reducing deposits before the uptake
    debits is refused.
  - Deposit reduction is bit-identical under three push orders, including an adversarial cell where the sum's
    order decides an F32 boundary. It closes the ledger to rounding, keeps the cone, and never credits more than
    was paid, even at efficiency 1 into cells that cannot hold the packet.
  - C.5 exactly through the whole transaction (3, 6 and 1 against an exact ledger). The split is exact over
    4,000 random cases. Viability is checked on the post-emission budget.
  - Attempts: the NumPy-pinned word for parent 7 at tick 1234, binomial rates over 10,000 draws, `p = 0` and
    `p = 1`, and reseeding. Offsets: the oracle-pinned offset within 4 ulp, uniform by area within 5σ over
    8,192 draws, and wrapping at the seam.
  - Cooldown, newborn state and identities; six births bit-identical under three request orders, with the
    ledger closing; settling refused on a changed budget with nothing recorded.
  - C.5 rollback: a logical genome shortfall leaves the quota, identities and parents unchanged, and a retry
    gives the same report. An identity overflow mid-stage rolls back too. A full world is refused before any
    genome work. Rejected births charge nothing and take no identity. Malformed requests are refused.
  - Eight planted bugs were each caught by at least one of these tests: nearest instead of downward cell
    rounding, a moment that ignores the received fraction, push-order cell sums, a deadline one tick early, no
    rollback on shortfall, rejected births consuming identities, a nearest-rounded parent share, and paying
    before staging.

- `tests/lifecycle.rs` (FL-012, 14 tests):
  - C.12 in both arrangements. Motion spends the last unit and the organism still moves. It dies at motion, then
    neither emits nor splits, and is collected at its resulting position with nothing to recycle. The ledger closes
    exactly with one dissipated unit, and the genome charge comes back. Resumed with field energy, the dead identity
    demands nothing and absorbs nothing, and readmission, update and a second death are all refused.
  - Exhaustion at chemistry, maintenance, motion, emission and the tiny reservoir, each against an exact ledger.
    Every later stage pays nothing, and what was paid keeps its effect. A survivor runs every stage.
  - Stage order and single checks; tiny clearing strictly below the threshold, including its `next_down`; recycling
    with the genetic affinity in every channel, removal to `O`, dissipation, a zero fraction and the refusals.
  - Single deaths, with refusals for duplicates, other ticks, never-born identities and mismatched genome,
    lineage or generation. A refused commit returns the births.
  - INV-07 readmission refusals, and capacity, order, tick, genome and INV-01 refusals, with nothing changed.
  - Stale held outputs never reach a newborn in a reused slot; compacted survivors keep their own state.
  - C.7: 48 ticks of random deaths, births and updates in both arrangements, with the same digest and rows after
    every tick and different slot layouts.
  - Numerical faults refused by identity, and updates that keep lifelong fields fixed. The digest sees each
    changing field.
  - Births and deaths committed together with FL-011's birth transaction. The parent keeps its share and deadline
    and the ledger closes. Both arrangements agree.
  - The maintenance bill against `mandatory` bit for bit at Appendix A, with partial payments covering basal
    first, exactly.
  - Sixteen planted bugs were each caught by at least one of these tests: keeping the last exhausting stage,
    allowing a repeated stage, a fate without the tiny stage, clearing at the threshold, recycling the whole
    residual, removal booked as dissipation, updates changing lineage fields, deaths not checked against the stored
    identity, duplicate deaths collapsing, readmitting the latest identity, compaction not moving the index, a
    reused slot keeping its held output, a free list that never reuses slots, a digest without the cooldown
    deadline, maintenance covering sensor first, and controller cost booked as basal.

- `tests/world.rs` (FL-013, 10 tests). Every world is Appendix A with a few values changed, so that a 32 × 32
  world breeds within a few hundred ticks. The oracles recount the published state from the state itself:
  - G1's small headless ecology: 48 founders over 384 ticks, with 41 births, 57 deaths (chemistry and
    maintenance), about 12,000 emissions, perturbed children and dead descendants. Every tick checks INV-01–INV-07
    independently: one epoch per tick, contiguous events, field and organism holdings measured bit for bit, the
    cumulative residual within the gate, each survivor's change equal to its recorded rate, every child's
    endowment equal to the recorded transfer, one genome charge per organism, the dead never returning, and an
    organism that died before motion collected where it stood. Motion paid is recounted from the survivors'
    velocities. At about `1e-7` per tick it is far below the transport residual, so closure alone cannot see it.
    The world digest is pinned.
  - C.7: compact and free-list worlds publish the same epochs, reports, rows and digests for 288 ticks.
  - C.8: a candidate cancelled, dropped or retried leaves the prior epoch authoritative, and its retry publishes
    exactly the reference run's state.
  - Controller cadence at `K = 4` against an independent due-tick oracle, with every newborn forced on its first
    advance and its phase unchanged.
  - Population and genome-quota shortfalls pause the tick with every committed byte unchanged.
  - A numerical fault stops the run, preserves the committed state and refuses every later advance.
  - Founders follow A.3 item 1 (identities, quadrant coverage, a pinned founder position, the cooldown deadline).
  - INV-08: an organism out of the field's reach evolves bit for bit as if alone.
  - `Population::check_tick` refusals: a forgotten, repeated, reordered, surviving-and-dying, starved or renamed
    organism, and a quota short of the dying genomes' bytes or count, each checked on its own.
  - Twenty-two planted bugs were each caught by at least one of these tests: regular evaluation keyed to the input
    tick, newborns not forced, packet scratch leaking between organisms, every demand at one position, identities
    or the quota advanced in place, the field, epoch or event cursor not committed, deposits before the grants'
    debits, parents keeping their whole budget, a cooldown not reset, the energy change rate not recorded, source
    paths a tick late, founder positions on another lane, faults not stopping the run, a pause becoming an error,
    motion cost unrecorded, a tick forgetting an organism, dying genomes keeping their charges, survivors' state not
    written, and the quota's count not checked.
- FL-016's world chunks are tested through the session, in `flife-runtime`'s `tests/checkpoint.rs`.

`tests/removal.rs` (8 tests, FL-022), on the 32 × 32 Appendix A ecology at tick 40. The oracle for which
organisms a disk holds is the test's own minimum-image distance (`rem_euclid`), not `FieldGrid::displacement`:
- `a_removal_takes_out_exactly_the_organisms_in_its_disk_across_the_seam`: a disk placed so that one organism is
  inside only by wrapping; exactly the organisms the oracle names die, in identity order and first among the
  tick's deaths, at their committed positions, with their committed energies' bits as residuals and their genome,
  lineage and generation; none of them remains; `command-removal` is their energy.
- `each_disposition_sends_the_residual_where_it_says_and_nowhere_else`: one disk, three worlds. `remove`,
  `dissipate` and `recycle` put the residual in `command-removal`, `unrecycled-death`, or the recycling transfer
  (credit at most half, loss as `unrecycled-death`). The recycling world's field exceeds the dissipating world's
  by exactly the credit (1e-9), the other two fields are bit-identical, and the survivors are the same in all
  three.
- `overlapping_removals_kill_each_organism_once_and_the_removed_take_no_part`: four radius-16 disks cover the
  torus; each organism dies once, by the first disk holding it, with that disk's disposition and count; nothing
  is evaluated, requested, granted, emitted or born, and the empty world advances.
- `a_disk_that_holds_no_organism_changes_nothing`: the same digest and deaths as a tick without it, and an applied
  record of zero.
- `removals_outside_the_world_or_wider_than_half_of_it_are_refused_unchanged`: ten refusals with the tick and
  digest unchanged, then half the world admitted.
- `a_removal_world_is_the_same_at_any_worker_count`: 1, 3 and 8 workers, 20 ticks on.
- `the_disk_includes_its_circle_and_wraps_both_seams`: `[3, 4]` and `[29, 28]` exactly 5 from the origin are
  inside a radius of 5; a millionth farther is not.
- `an_injection_and_a_removal_at_one_boundary_are_reported_in_sealed_order`.

Eleven planted mutants were each caught: the circle excluded, no minimum image, the removed kept in the tick, an
organism dying twice, half the residual, every disposition dissipating, half the world refused, the report left
in application order, removal deaths unreported, and in `flife-runtime` the disposition unencoded or decoded as
`recycle` always.

`tests/ablation.rs` (3 tests, FL-041), on the 32 × 32 Appendix A ecology:
- `the_model_key_names_each_variant_and_refuses_others`: each name maps to its variant and validates; another name
  (`field-ecology-v2`, `field-ecology-v1-shuffled`) is refused.
- `a_chemistry_only_world_pays_the_same_evaluations_and_decides_from_zero`: the same founders as the baseline; on
  the first tick the same evaluations and the same cost flows bit for bit; over 240 ticks no organism emits (the
  baseline does) and every birth's child share is 1/2 within 1e-12, where the baseline's vary.
- `a_fresh_offspring_child_holds_the_founder_genome_of_its_own_identity`: every child equals, in topology,
  parameter bits and traits, `flife_genome::founder` at its identity, with an empty mutation record; under
  `field-ecology-v1` none does. Under 1/4 of the parameters a fresh child shares with its parent are identical,
  against over 9/10 when inherited.

**Validity domain.** `field-ecology-v1` with `bounded-affinity-v1` and `per-granted-cell-packet`, at sensing-scale
cost exponent 1; FL-011 at `no-emission-no-charge`, `post-motion`, `next-tick` visibility and activation, and
`pause-before-commit` at the birth stage; FL-012 at `dissipate-and-record` and `genetic-affinity`; FL-013 at
`cpu-reference`, `qualified-reference`, `stop-and-preserve`, `pause-before-commit`, `uniform` positions and `zero`
velocity, first-order splitting at the configured fixed timestep.

**Known gaps.**
- C.11's near-zero emission fractions and birth hazards are decoded and tested in `flife-neural` (FL-007). FL-011
  settles them, including the exact zero limits.
- Phase 1 applies what `flife-runtime` seals: single-cell `InjectEnergy` (FL-014) and disk `RemoveOrganisms`
  (FL-022). Area energy brushes, material, seeded introduction, source edits and the native input that maps a
  brush stroke to these payloads are still FL-022. The exhibition-mode stable-ID birth rejection policy has no
  registered name yet.
- `stop-and-preserve` keeps the committed state and the fault's description, which names the quantity and,
  where the operator knows it, the organism. It does not yet retain the faulted candidate's buffers for
  inspection, and there is no exhibition containment mode.
- Steady state reuses the field buffers, the pyramid, transport and controller scratch, and the per-organism,
  demand, grant, packet, deposit, survivor and death buffers. Each tick still allocates its boxed candidate, the
  birth requests, the staged births and the report's event lists, which scale with births and deaths, not with the
  population. FL-027 declares the contribution scratch when the world is built, from `population.maximum_count`,
  and admits it against `BufferLimits`: the grants, both deposit reductions, and `Bins` grouping each organism's
  grant packets. The tick groups packets by cell and by organism with `flife_field::Bins` counting sorts instead
  of comparison sorts, in the same order, so the world is bit-identical. A reduction within the declaration
  allocates nothing, a hotspot included. Admitting the rest of the scratch, with the CPU worker budget, comes
  later (§12.5, FL-043).
- The live-population quota counts every parent with a viable attempt as live. Its kept share is at least
  `minimum_parent_energy_after_split`, so only a minimum below the tiny threshold could make that conservative.
- Resource epochs here are the committed world's: there are no leases, observers or render projections yet
  (FL-017), and the reference has no device fences. A candidate is exclusive by borrowing, not by a lease.
- `execution.cpu_workers` (or `World::set_cpu_workers`) sets the threads for organism decisions and the field step;
  no result depends on it (`the_cpu_worker_count_changes_no_result`: 1, 2, 3, 6 and 64 workers, 96 ticks with
  births). Decisions read only committed state and split into contiguous identity-ordered chunks of at least 128,
  the first error in identity order winning. The field splits transport by plane, and each moment plane keeps its
  per-source deposit order. The threads are scoped `std` threads spawned per tick: a bounded, reported reference
  adapter (§3.4, §12.5), not Asupersync-scheduled work. Invariant checks, chemistry composition, uptake and the
  commit stay on one thread, and a persistent pool is later work. Parallel phases must keep these reduction orders.
- `World::set_controller_backend` (FL-030) hands each tick's due evaluations to a `flife_neural::ControllerBackend`
  in one batch, in identity order. Observations are made on the lanes first, then the backend is called once, then
  each lane decodes its run, evaluating what the backend deferred on the reference. Any error on that path hands
  the tick to the reference path, which reproduces the reference's own error. A backend that fails is dropped, with
  the reason in `World::controller_report`. No result depends on the backend. `tests/controller_backend.rs` runs the
  preset at 512 founders for 300 ticks through a CPU batch backend that defers one job in seven, and the digests
  match the reference at every tick. Two planted negatives: one flipped output bit must change that tick's digest,
  and a planted device failure must leave the world unchanged and be reported.
- `World::set_field_backend` (FL-031) hands phase 2 to a `flife_field::FieldBackend`, which must equal
  `advance_field_on`. If it errs, the step is redone on the reference from the same committed field: the
  backend writes only the candidate, so nothing it left there survives. The backend is then dropped, and
  `World::field_report` (`FieldReport`: backend, device and reference steps, failure) records why. If the
  reference errs too, its error is the tick's. No result depends on the backend. Like the controller backend,
  it is not state: a restored world steps on the reference until its owner sets one. The tests are in
  `flife-backend-metal`'s `tests/field_world.rs`.
- First-order splitting only. Strang splitting and timestep-refinement studies are later items with their own
  scheme identity (§8.4).
- INV-08 is tested by isolation: an organism outside the field's reach evolves bit for bit as if alone.
- FL-016 encodes the world (INV-14) and restores it to the same trajectory, and FL-035's `flife-store` publishes
  it atomically and recovers the last valid one. Resuming a session from the store is FL-037. Genomes are stored whole, one copy per
  live genome: 1.4–3.6 MB for the 48-founder ecology and 161.3 MiB for the 4,096-organism preset, with no page
  sharing, deltas or compression. A restored store is compacted in identity order, so `Population::latest`
  names the highest live identity rather than a since-dead later one; new identities come from the restored
  allocators either way. The checkpoint carries no toolchain or source fingerprint (B.1's run manifest).
- Explicit removal is carried by `RemoveOrganisms` (FL-022), a disk. Other region shapes and the erase brush's
  native input are later. Environmental hazards are not part of `field-ecology-v1`.
- `Population` stores each genome as its own heap value. Packed slabs, generation-tagged handles and
  epoch-aware reclamation after readers finish are FL-026. FL-016 checkpoints the store's contents. The ordered
  tick returns a dead genome's quota charge in the publishing commit; readers holding its shared parameters keep
  them.
- Field reservoirs are not cleared by the tiny-energy policy: A.1's "clears its moment consistently" has no
  organism moment to act on in this model, and F32 subnormal cells are a numerical-profile matter (the field
  contract's cone-tolerance gap).
- Sensor gradients come from FL-007.
- A Metal implementation is FL-032; its parity with this reference is a separate numerical profile.
- FL-041: §21.6's shuffled-inheritance and shuffled- or frozen-controller variants are not models here.
  `chemistry-only` fixes every decision at the zero output rather than at a random or frozen one, and keeps the
  controller's cost, so it isolates the decisions, not the cost of having a controller.
