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
still FL-022's. FL-022 also adds `Intervention::InjectMaterial`, the material brush (§14.3): declared energy into
one cell with a signed composition direction `a`, `||a||₂ ≤ 1`, so the packet `(Δe, Δe a)` is admissible and the
brush cannot write an `m` that breaks `||m||₂ ≤ e`. And it adds `Intervention::IntroduceOrganisms`, seeded
introduction (§12.3, §14.3): `count` new organisms, each endowed with declared energy, placed in one cell at the end
of the tick, each drawn exactly as a founder with its identity would be.

FL-016 (`flife-gh0`) writes and restores the whole world as ten checkpoint chunks (plan §16.1, §16.3, INV-14,
Appendix B.2, C.13).

FL-041 (`flife-rln`) adds two ablation models (plan §15.3, §21.6), each `field-ecology-v1` with one rule changed
and its own registered model name, so an ablated run never shares a model identity with the baseline:
`field-ecology-v1-chemistry-only` (decisions decode from an all-zero output) and `field-ecology-v1-fresh-offspring`
(a newborn's genome is the one a founder would have at its identity).

FL-043 (`flife-8gy`), first slice, gives the world a footprint (plan §19.4): the bytes its state, fields and tick
scratch hold, each buffer charged its capacity, and the most they hold while the population stays within
`population.maximum_count`. The fields and the tick's scratch are reserved for that capacity, so stepping never
grows them. `flife-cli`'s preflight admits these terms against `[execution] maximum_total_bytes`.

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
  amounts, every proposed parent in stable order, and how many births were covered. `resolve_births_with`
  (FL-032) is the same, with some parents' first candidates already mutated elsewhere (`flife_genome::Prepared`,
  sorted by parent): those births are proposed by `Mutator::propose_prepared`, the rest by `Mutator::propose`.
- `Vitality` (FL-012): per organism, the first `BudgetStage` that exhausted it, and its `Fate` (`Survives` or
  `Dies(stage)`) once the tiny-reservoir stage has been checked.
- `TinyEnergyPolicy::clear`: the cleared energy, for the `tiny-reservoir` flow.
- `Death` (identity, genome, lineage, generation, tick, resulting position, `DeathCause`, residual), and
  `Recycling::settle`'s `DeathSettlement` (removed, dissipated, recycled and offered; `record` books the first two).
- `Population`: `admit`, `update`, `commit` (the `Collected` deaths, each with its last row and genome, for the
  caller to retire), `get`, `iter` and `ids` in identity order, `iter_with_genomes` (each organism with its
  genome, read from its one slot), `slot` and `slots`, `genome`, `arena_slot` and
  `arena` (FL-026: each live genome's generation-tagged slot, assigned at admission or birth and released at
  death, kept when the compact arrangement moves the organism, and reassigned in identity order on restore),
  `latest` and
  `digest`, a SHA-256 of the canonical state in identity order without slots (`frankenlife.population-state.v1`).
  `HeldOutput` hides an invalid output entirely. `check_tick` and `commit_tick` (FL-013) check and commit a whole
  tick: survivors, deaths with their genome charges returned to the quota, and births. Each finds the survivors'
  slots in one ordered walk of the identity index (the survivors are checked ascending first), not a search per
  survivor (FL-034's C13); the refusals and their order are unchanged. In the 25k evolved world's profile those
  searches, with the render projection's second search per genome, were about 7.7% of the main thread, which
  bounds the tick. Measured on csd (2 x EPYC 7282, quiet): 10.36 to 10.86 ticks/s, paired median 1.047, 6 of 6,
  every digest one value. On the shared Mac both runs were inadmissible (other agents' load), so no claim there.
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
- FL-022: `Intervention::InjectMaterial { cell, amount, direction }` and `MATERIAL_CHANNELS` (32, one component
  per moment channel). `flife_field::sources::squared_norm` is now public, the one norm rule for a source's and a
  material's direction.
- FL-022: `Intervention::IntroduceOrganisms { cell, count, energy }` and `MAXIMUM_INTRODUCTION` (4096, the reference
  preset's founder count); `AppliedIntervention::introduced`, the organisms an introduction introduced (zero for
  anything else); `TickReport::introductions`, an `IntroductionEvent { organism, genome, topology }` per introduced
  organism in identity order, after the births (the event range counts them last).
  `Intervention::declared_energy` is no longer `const` (it multiplies the count).
- FL-041: `ModelVariant` (`Baseline`, `ChemistryOnly`, `FreshOffspring`) and `ModelVariant::of(model)`;
  `reproduction::Offspring` (`Inherited`, or `Fresh { catalog, limits, initial_scale }`), the new `offspring`
  field of `BirthContext`.
- FL-043: `Footprint { state, field, scratch, staging }` with `total()`; `World::allocated()` (what the world holds
  now, staging 0), `World::footprint()` (the most it holds within its capacity) and `World::batched_bytes()` (what
  installing a controller backend adds). `Population::allocated_bytes()`, `Population::bound_bytes(topologies)`
  and `PER_ORGANISM_BYTES` (an organism's index, free-list and genome-header entries). `ControllerReport` gains
  `resident_peak_bytes`, the most `ControllerBackend::resident_bytes` reported after any batch.
- FL-043, second slice: `World::admit_memory(batched, checkpoint, render)`, the peak-memory preflight every host
  calls (moved here from `flife-cli` so the Studio can call it): the allocated genome quota, the footprint's state,
  field, scratch (plus `batched_bytes()` when `batched`) and staging, the host's `render` and `checkpoint` bytes,
  and `RESERVE_BYTES`, against `maximum_total_bytes`. Over it: `BudgetExhausted` ("peak memory
  (maximum_total_bytes)"). Otherwise the backend is granted the rest when `batched`, or nothing.
- FL-021 (`flife-zyc`, plan §14.2): `World::organism_snapshot(id)`, `Ok(None)` for an identity the population does
  not hold, or an `OrganismSnapshot`:
  - `tick`, the world's committed tick;
  - `organism`, its row: identity, genome, parent, lineage, generation, birth tick, position, velocity, energy and
    its change rate over the last committed tick, held output, phase and reproduction cooldown;
  - `age_ticks`, from its birth tick to `tick`;
  - `topology`, its catalog index, and `hidden_widths`, that topology's;
  - `parameters`, its controller's parameter count;
  - `traits`: its affinity direction, motion response and sensing scale;
  - `local` (plan §14.2's Chemistry): a `LocalChemistry`, what the committed field offers it where it stands. Its
    `grid-cic-v1` exchange footprint at its position, as `ExchangeCell`s in ascending index order (the cell, its
    weight, its F32 energy and the efficiency `η(z) = η_max tanh(k a·z − b)` at its composition `z = m/e` for the
    organism's affinity, neutral for an empty cell); the footprint's density `r` and A.1's request
    `Δt U_max r/(K + r)` there; and the gain `Σ w_c max(η_c, 0)` and toxicity `κ Σ w_c max(−η_c, 0)` per unit
    taken in the footprint's proportions. It is the committed field, which the Studio draws, not a forecast: a
    step's uptake reads the field after its transport and sources, and grants less than the request where
    organisms contend or quanta round;
  - `decoded`, its held output decoded as the world decodes it for the next tick unless it is evaluated again
    (from zero under `field-ecology-v1-chemistry-only`), or `None` while it holds no output;
  - `sensed` (plan §14.2's Inputs), while the world watches it: a `Sensed` with the newest published step's start
    tick, its sensing scale (the source scale its sample blends at), the raw sample (`R, M_0, …, M_31` and their
    x and y derivatives), the encoded `Observation` (the 104 inputs in ABI order and the unsaturated gradients
    motion follows) and whether it evaluated its controller on it; or `None`;
  - `activations` (plan §14.2's Brain), when the world recorded the observation of the evaluation its held output
    came from (that step started one tick before `held.evaluated_at()`): the hidden activations
    `ReferenceController::trace` gives on those inputs, with the output's tick. The trace must reproduce the held
    output bit for bit;
  - `transactions` (plan §14.2's Transactions), while the world watches it: one `Transactions` per step the world
    recorded, oldest first, at most `RECENT_STEPS` (128, a second at the reference 1/128 s tick), the newest the
    step to `tick`. Each holds the step's start tick, its opening energy, and the settlements its stages returned
    as the ledger was given them, in §8.2's order: chemistry's `UptakeSettlement` (absorbed, gain, toxicity
    requested and paid), then, unless an earlier stage exhausted it, maintenance's `MaintenanceSettlement`,
    motion's `MotionSettlement` (proposed and applied velocity, cost, whether scaled), emission's
    `EmissionSettlement` (budget, cost, offered) and a `ReproductionOutcome` (`CoolingDown { deadline }`,
    `NoAttempt`, `Unviable`, `Born { child, split }` or `Rejected { split, candidates }`); then what the tiny
    reservoir took and its closing energy (zero if the step exhausted it);
  - `descendants` (plan §14.2's Descendants), while the world traces its ancestry: a `Descendants` with the tick
    its counts run from (`since`), the children it bore and how many of them live, the descendants born below it
    and how many of them live, and its `path`: the ancestors its line passes through that are alive or where two
    or more lines with live members meet, nearest first, at most `ANCESTRY_PATH` (8), each an `Ancestor` with its
    identity, generation, birth tick, whether it lives, and its descendants born and living; and how many more
    lie `beyond` them. A dead ancestor through which only one line with live members passes is not on the path;
    a founder's or an introduced organism's path ends with it. `None` while the world traces nothing.
- `World::organism_weights(id)` (plan §14.2's on-demand weight dump): `Ok(None)` for an identity the population does
  not hold, or a `ControllerWeights`: the tick, the organism, its genome and topology index, its topology's
  `LayerShape`s in evaluation order (fan-in, fan-out, and where the weights and biases sit), the weight dtype its
  genome stores, and its genome's parameters copied whole as F32 (a binary16 widened exactly), in the controller's
  layout (layers in evaluation order, each `weight[out][in]` row-major then `bias[out]`, torch's
  `parameters_to_vector` order). `ControllerWeights::layer(index)` gives a layer's weights
  and biases. The copy's room is reserved first (`AllocationFailed`), and a genome whose parameter count is not its
  topology's is refused (`StateConflict`, "dump organism weights").
- `World::measure_region(center, radius)` (plan §14.3's temporary measurement region, `measure`): a
  `RegionMeasurement` of the committed world, with:
  - the tick, centre and radius;
  - the organisms in the disk a removal of that centre and radius would take (`Intervention::removes_at`), so
    a removal applied alone at this boundary removes exactly `organisms`, holding exactly `organism_energy` bit for
    bit (summed in identity order, as the removal sums it);
  - their distinct founder `lineages`;
  - the field `cells` whose centres lie in the disk by the same minimum-image test, the circle included, and their
    `field_energy` summed in cell-index order.

  A disk a removal would refuse is refused (`NumericalInvalid`, `measurement centre` or `measurement radius`), and
  half the shorter world side is admitted. It reads each organism's row and the committed energy plane once, copies
  no genome and changes nothing. Its cell test is `measure::cell_in_disk`, which the crop and the graft share.
- `World::cropped(center, radius)` (FL-042, plan §14.5's local package, `world/crop.rs`): the committed world with
  everything outside the disk emptied, and the disk's `RegionMeasurement`. It keeps the configuration, tick,
  resource epoch, event count, source paths and identity allocators. Of the matter it keeps the organisms
  `measure_region` counts, each with its whole row and its genome resealed through a fresh quota (the same identity
  and parent, the traits checked again against the limits), and the cells `measure_region` counts, every plane; the
  rest of the field is zero. Its ledger opens at its own holdings. A fault refuses it (`StateConflict`, "crop
  world"), and so does any disk a measurement refuses.
- `World::graft(package, center, radius, offset)` (FL-042, `world/graft.rs`): put a crop of the disk about
  `center` into this world, moved by `offset` whole cells, and return a `Graft` (where the disk went; host cells
  replaced and their energy; host organisms displaced and their energy; organisms grafted and their new lineages;
  the holdings before, the package's endowment, and after).
- `World::watch(Some(id) | None)` names the one organism whose observation and transactions each step records, from
  the next step on. Watching another organism drops the records of the one before; watching the same one again
  keeps them. `World::watched()` names it, and `World::watched_transactions()` gives its records, oldest first,
  whether or not it still lives (the newest of an organism that died is the step that exhausted it). Each step's
  preparation reserves the records' room, so publishing allocates nothing; a cancelled or paused candidate records
  nothing.
- `World::trace_ancestry()` (FL-021, plan §14.2's Descendants) begins the world's ancestry index (the `ancestry`
  module) at its committed tick: from the founding every birth counts; later, it starts from the live population,
  each organism under its parent when that parent lives too. It reserves the index's room, `ROOM_PER_ORGANISM`
  (3) entries for each organism `population.maximum_count` allows, so call it before `admit_memory`; tracing again
  keeps what the index holds. `World::ancestry_held()` gives an `AncestryHeld`: `since`, the `members` (live
  organisms and dead ancestors where lines meet), how many are `dead`, the `entries` held (members and leavers
  not yet cleared out), the `room` and the index's `bytes`. A restored world traces nothing until told to.
- `World::follow(organism)` (plan §2.2, "Follow descendants of a selected organism") begins a line of descent
  from a live organism at the committed tick, or with `None` follows nothing; `World::followed()` gives the
  `Followed` line: its `origin`, the `organism` that has it, the tick `since` which it has had it, its
  `handoffs` and whether that organism is `alive`. When the organism that has the line dies, the line passes in
  that tick to its heir: its descendant alive once the tick's deaths are all folded, fewest generations below it
  and then the eldest (lowest identity), among the descendants the index reaches through organisms alive since
  tracing began. With none alive the line ends, naming the organism that died, until another is followed.
  Following an organism always begins a fresh line, even from the one that has the line now. It needs the
  ancestry index.

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
  all into the next field, so new deposits are first sensed next tick. Each runs on the field's workers. The
  debit (`flife_field::apply_grants_on`) writes each moment plane as one thread would. Each reduction
  (`flife_field::apply_deposits_on`) reduces and writes every cell as on one thread, and sums what the cells
  received in cell order. The world is therefore bit-identical at any worker count
  (`the_cpu_worker_count_changes_no_result`, pinned digests). With a field backend, the three go to its
  `reduce` instead (`reduce_field_with`, below).
- Phase 10 checks the next field (INV-01, INV-02 at the profile's cone tolerance), closes the tick against holdings
  measured from the next field and the survivors and newborns in identity order (INV-03 per tick), commits the
  closure to a copy of the run's ledger (INV-03 cumulative), and checks the population's whole commit (INV-01,
  INV-06, INV-07). The next tick opens at exactly those holdings, so energy that moved without a flow is refused.
  The field check runs on the field's workers (`FieldState::check_invariants_on`), which reports the same summary
  and the same first offending cell at any worker count. With a field backend it runs there instead
  (`check_field_with`, below).
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
- FL-022: an `InjectMaterial` applies where and when an injection does, in sealed order with them. Its energy is
  raised exactly as `InjectEnergy` raises it (rounded down, never above the request), to the change `Δe` actually
  made. Each moment channel `k` with `a_k ≠ 0` then becomes `m_k + Δe · a_k` (F64, rounded to the nearest `f32`),
  in channel order; channels with `a_k = 0` are not written, so a direction of zeros makes exactly the world the
  energy brush makes. The composition follows `Δe`, not the request, so `||Δe a||₂ ≤ Δe` holds even when the cell
  rounds down. The `command-injection` flow is `Δe`. A direction whose squares, summed in channel order in F64,
  exceed one, or with a non-finite component (its sum is then NaN or infinite), is refused like a bad cell or
  amount; a norm of exactly one is admitted.
- FL-022: an `IntroduceOrganisms` adds its organisms in phase 8, after the births are staged and every fate is
  decided, as the tick's last entrants; they first sense and act in tick `n + 1`, and a removal at the same
  boundary cannot touch them. Introductions go in sealed order, each one's organisms in turn, each taking the next
  organism and genome identities after the births', so an introduction never changes a birth's identity, genome or
  randomness. Each organism's genome is `flife_genome::founder` for its identity, its position the founder
  position words (`founder_position`'s, now shared through `founder_draws`) scaled into the cell, uniform there; it
  founds its own lineage (lineage = identity, generation zero, no parent), is born at `n + 1` with the endowment,
  zero velocity and change rate, an invalid held output, its stable phase and the newborn cooldown deadline
  `n + 1 + ⌈cooldown/Δt⌉`. The endowments, `count × energy` rounded once, are `command-injection`, and the organisms'
  energies join the closing holdings. An introduction is all or none: if its organisms do not all fit under
  `population.maximum_count` beside the survivors, the births and the introductions before it, or the genome quota
  refuses any of their genomes, it introduces nothing, spends no identity or quota and applies zero. It never pauses
  the run. A count of zero or above `MAXIMUM_INTRODUCTION`, an endowment that is not a finite positive energy at
  most `f32::MAX`, or a cell outside the grid is refused before anything changes. No new model identity: a world
  never sent the command is the world it was.

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
- Scratch buffers and the controller, field and sensor backends are not state. A restored world runs on the
  reference path until its owner calls `set_controller_backend`, `set_field_backend` or `set_sensor_backend`, and
  no result depends on any of them.

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

**Invariants — the footprint (FL-043; §7.5, §19.4).**
- Every term charges a buffer its capacity times its element size (`flife_core::memory`). Genome parameters are
  apart: the genome quota charges them.
- `state` is the population (its columns at capacity, the genome arena, and each organism's index entries), the
  sources and their paths. `allocated` counts the arena and the entries as they are; `footprint` counts them at
  their bounds for a full population of the catalog's members.
- `field` is both fields, the field step's per-worker transport scratch and one source support per source, and the
  sensor pyramid. Each support is reserved at founding for its kernel's most candidate cells on the grid (per axis
  at most `⌈2r/h⌉ + 5` cells, and at most the axis), so a step never grows one.
- `scratch` is the tick's rows, decision lanes, demands, grants, packets, each packet's composition, deposits,
  survivors and deaths, and the sensor backend's queries and samples. They are reserved for the capacity when the
  world is founded, its workers are set or a sensor backend is installed. Each lane's share of the rows follows
  the worker count, and lanes removed by a smaller count are released. The compositions take room for four
  packets a row (256 bytes each), so the footprint grows by 1 KiB per organism of capacity. Each lane also holds
  room for phases 6 and 7 of the most rows it can take: six 24-byte ledger entries, one 296-byte staged emission
  and its four 32-byte packets a row, 568 bytes per organism of capacity in all (measured: one worker at 16,384
  rows adds 9,306,680 bytes), and one organism's grant packets.
- Phase 6 computes every granted packet's composition before any organism settles. It runs on the world's lanes,
  each lane taking a contiguous run of at least 1,024 packets in packet order, and a packet whose cell repeats the
  previous packet's in its run copies that composition. A composition is a function of the observed field and its
  cell alone, so the result does not depend on the worker count: the world is bit-identical at any worker count
  (`the_cpu_worker_count_changes_no_result`, pinned digests).
- Phases 6 and 7 then settle on the lanes, each lane taking a contiguous identity-ordered run of at least 128
  organisms. A lane records its run's ledger flows in a `flife_core::LedgerLog`, which accepts or refuses each
  exactly as the tick's ledger would, and stages its run's emissions in deposits of its own. The tick then replays
  each lane's log into its ledger and appends each lane's deposits to its own (`Deposits::append`), in lane order:
  the same terms in the same order as one thread settling every organism in identity order, so the ledger's
  compensated sums and the staged deposits are the same to the bit. The first error in identity order wins, and
  a lane's buffers are emptied before each tick's settlement. `a_replayed_log_sums_the_same_bits_as_recording_directly`
  and `appended_deposits_reduce_as_if_pushed_in_order` check the two halves directly. Known gap: compensated sums
  of nonnegative terms almost never depend on their order, and deposits reduce in identity order, so taking the
  lanes in the wrong order changes no result the tests can reach (a planted reversal survived); the order is kept
  by construction, and only the log test's constructed tie sees it. No test reaches an error inside a lane either,
  so planted defects in the first-error rule and in emptying a lane's buffers survived too.
- Installing a controller backend reserves the batched path's outputs, a slot per row, and each lane's observations
  for the most rows it can take: exactly `batched_bytes()`. Removing it releases them. If the host refuses them,
  no backend is installed and the report says why.
- `staging` is what a step holds only while it runs, charged per organism of capacity: a birth request and its
  place in the resolution order and in a pause's proposal; a place in both the births and the rejected births; one
  boxed genome and at most `MAXIMUM_CANDIDATES` candidate rejections; two entrants (one while an introduction's
  are staged); one birth or introduction event, one death in the report and one controller job: 3,448 bytes on
  this build. Each of those lists is reserved for its count before it is filled, so none grows past it.
- `footprint()` does not change as the world steps. No result depends on any of this.

**Invariants — inspection (FL-021; §14.2, §21.2 INV-09).**
- A snapshot reads one row through the identity index, that organism's own genome, the topology catalog and the
  committed field at its exchange footprint (at most four cells). It copies no controller parameter and nothing of
  another organism or genome, so its cost does not grow with the population or the genome bank.
- Its local chemistry is computed by the functions phase 5 and chemistry use (`ExchangeStencil::at`, `density`,
  `RequestRule::request`, `Affinity::new`, `composition`, `Chemistry::efficiency`), so the panel and the tick
  cannot disagree on the law.
- It takes `&self` and changes nothing: a world inspected in full before every tick commits the digests of one
  never inspected.
- The held output is decoded by the function the tick uses (`raw_outputs` then `decode`), so the snapshot and the
  tick cannot disagree on the variant's raw outputs.
- The watch is not scientific state. No digest, checkpoint or report holds it, a restored world watches nothing,
  and no step reads it. A step records the watched organism's observation after its decisions, by the functions
  they use (`blended`, then `encode`), from the sensor backend's sample for its row when the decisions used the
  backend's samples. The record becomes visible when the step publishes, so a cancelled or paused candidate
  records nothing. A step costs one binary search of its identity-ordered rows and one sample more; the
  activations are evaluated at inspection, once, from the recorded inputs and the organism's own genome.
- The ancestry index is not scientific state either. No digest, checkpoint or report holds it, and no step reads
  it. Publishing a tick first checks that the index can take its report (`admits`: each birth's parent a live
  member, newcomers' identities above every one it holds and ascending, each death a live member or one of the
  tick's births), then commits the tick, then folds the report into the index, which cannot fail and allocates
  nothing: the tick's births, deaths and introductions in the report's order. It holds at most `2n − 1` members
  for `n` live organisms, and clears out leavers before adding newcomers when its room would not hold them. A
  birth or death costs a step for each index member between it and its line's first ancestor; a `Descendants`
  costs a step for each ancestor above the organism in the index, at most the path's eight named. Its bytes are
  part of `footprint()`'s and `allocated()`'s state, so `admit_memory` charges them, and they do not change as
  the world steps.
- A followed line is the index's, and like it is not scientific state: no digest, checkpoint or report holds it,
  and no step reads it. Folding a tick in which the line's organism dies marks, as that organism's death is
  folded and while every link below it still reaches it, each entry above it that descends from it, and chooses
  the heir among the marked once the tick's deaths are all folded, so the heir does not depend on the order the
  report lists them in, and before the room is cleared out for the introductions. That costs one pass over the
  entries above the dead organism, with a binary search each, and allocates nothing: the mark is a field of
  each entry, within the entry's existing size.

**Invariants — crops and grafts (FL-042; §14.5).**
- A crop is the disk and nothing else: rows, genomes and cell planes bit for bit, every other cell zero, and its
  ledger's initial and current holdings exactly its field's and organisms' energy. Its world checkpoint restores to
  the same digest and runs on identically.
- A graft applies only to a founding world (tick 0, epoch 0, no event, no fault, no trace), with the package's
  cell size (bit for bit), topology catalog and parameter type. The package must be a crop of its disk: an
  organism outside it, or a cell outside it that is not all zero, is refused.
- Cells map to cells: each package cell in the disk goes to the host cell under `target + d`, where `target` is
  the disk's centre moved by `offset` cells and wrapped on the host's torus and `d` is the cell centre's
  minimum-image displacement from the package's centre on the package's torus; it replaces every plane. A host
  may differ in size from the source; two package cells landing on one host cell (a host narrower than the disk)
  are refused.
- The host's organisms in the host's disk are displaced; the others keep their rows and genomes, in order. The
  package's organisms follow in identity order with the host's next organism and genome identities, as founders:
  no parent, generation 0, born at tick 0, held output invalid, energy change rate 0, the stable phase of the new
  identity, the founders' cooldown deadline. They keep velocity, energy, topology, traits and parameters, and
  their position's displacement from the disk's centre. Each source lineage becomes one new lineage named by its
  first member's new identity.
- The ledger reopens at the grafted world's holdings: the host's, less the displaced cells' and organisms' energy,
  plus the package's endowment (to rounding).
- All or none: the field, population, quota and identity allocators are built aside and assigned only once every
  check and allocation has passed. A refused graft leaves the host's digest, ledger and population as they were.

**Refusal cases.**
- FL-042: `cropped` of a stopped world (`StateConflict`, "crop world"); `graft` into a world that is not
  founding, of another cell size, topology catalog or parameter type, of a package holding matter outside its
  disk, or landing two cells on one (`StateConflict`, "graft package"); a disk either world's measurement refuses
  (`NumericalInvalid`); a host whose population or genome quota cannot take the organisms (`BudgetExhausted`).
- FL-021: a live organism whose genome or topology is missing (`StateConflict`, "inspect organism"); a held output
  that does not decode (as `decode`); a watched organism whose held output the reference does not reproduce from
  the inputs recorded for it (`StateConflict`, "inspect organism"), which only a controller backend that broke its
  bit-for-bit contract can cause. The ancestry index's room not reserved (`AllocationFailed`, "ancestry index"),
  after which the world traces nothing; a tick report the index cannot take (`StateConflict`, "ancestry index"),
  refused before the tick commits, which the world's own reports never cause. Following an organism in a world
  that does not trace its ancestry, or one its index does not hold alive (`StateConflict`, "follow a line"),
  keeping the line followed before.
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
  - `tests/lifecycle.rs::a_whole_tick_commit_finds_survivors_by_identity_across_gaps`: with gaps the dead leave and
    a reused slot, in both arrangements, `commit_tick` writes each survivor's state on its own identity; a dead
    identity between live ones is refused as not alive by `check_tick` and `commit_tick`, nothing changed; and
    `iter_with_genomes` gives `iter`'s organisms with `genome`'s genomes. Of five planted defects in the walk and
    in `iter_with_genomes`, each was caught; the one that drops the walk's equality check is caught by this test
    alone.
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

`tests/material.rs` (5 tests, FL-022), on the 32 × 32 Appendix A ecology:
- `a_material_with_no_composition_is_the_energy_brush`: the metamorphic oracle. A zero direction gives the energy
  brush's applied energy and flow bit for bit and the same digest, then and 20 ticks on.
- `a_material_adds_the_energy_brush_energy_and_its_composition`: a composition changes the digest but not the
  applied energy or the flow, and the tick closes within its allowance; a material removes nothing, and
  `removes_at` says so even at its own cell's centre.
- `materials_outside_the_unit_ball_are_refused_unchanged`: norms one ulp and 1e-14 above one, NaN and infinite
  components, a cell outside, a zero and a NaN amount, each refused with the tick and digest unchanged; then a unit
  direction (`0.6, 0.8`, exactly one in channel order) and `-1` in the last channel admitted.
- `a_material_world_is_the_same_at_any_worker_count`: 1, 3 and 8 workers, 20 ticks on.
- `a_material_and_an_injection_at_one_cell_apply_in_sealed_order`: each sees the cell the one before it left; a
  0.01 material after a 10⁶ injection adds nothing (below one `f32` ulp), before it adds energy.

`intervention.rs`'s unit tests check the packet on a bare 4 × 3 field, where nothing else touches the cell, against
`f32` values written out: channels 0 and 3 become exactly `2.3` and `-2.65` from `(0.5, -0.25)` with `Δe = 3` and
`a = (0.6, -0.8)`, every other plane value keeps its bits (a `-0.0` moment included); a cell that rounds `1 + 0.1`
down takes composition for the energy it took, not `0.1`; unit material keeps the cone into an empty cell and onto
a cell already on it, exactly.

Thirteen planted mutants (with `flife-runtime`'s and `flife-cli`'s) are listed in `flife-cli`'s contract under
FL-022.

`tests/introduction.rs` (7 tests, FL-022), on the 32 × 32 Appendix A ecology. The oracle is a second founding:
- `introduced_organisms_are_the_founders_a_larger_founding_draws`: five organisms introduced into the 48-founder
  world at tick 0 carry the genomes (parameters bit for bit), traits and phases of founders 48–52 of the same world
  founded with 53; each position is that founder's position words scaled into cell (15, 17) within 1e-12, inside the
  cell; each founds its own lineage, is born at tick 1 with the endowment, zero velocity, an invalid held output and
  that founder's cooldown deadline one tick later. Applied 12.5, `introduced` 5, the flow 12.5, five more live and
  five more events, and the report's introductions name them with the larger founding's genomes and topologies; on
  the next advance they are evaluated, and 30 ticks follow.
- `introduced_phases_follow_identity_under_a_longer_interval`: under a controller interval of 4 the introduced
  organisms take the founders' stable phases, not all zero (the ecology's interval of 1 makes every phase zero).
- `the_report_lists_an_introduction_in_its_sealed_position`: an introduction, an injection, a removal and another
  introduction are reported in sealed order although they apply in phases 8, 2, 1 and 8.
- `an_introduction_is_all_or_none_under_the_cap_and_the_quota`: with 48 live under a cap of 51, introductions of
  2, 2 and 1 apply 2, none and 1 in sealed order, and the world equals one sent only the two that fit; with a
  logical genome quota of exactly 50 genomes, an introduction of 3 introduces none and one of 2 both, leaving 50
  live genomes and identity 50 next.
- `introductions_enter_after_the_births_and_never_change_them`: at the first tick with births, the births, rejected
  births and deaths equal the plain world's, and every introduced identity is above the newest child's.
- `introductions_outside_the_rules_are_refused_unchanged`: a count of 0 or 4097, endowments 0, −1, NaN, ∞ and
  2 × `f32::MAX`, and two cells outside, each refused naming its quantity with the tick and digest unchanged; the
  limit itself is admitted, `declared_energy` is every endowment, `removes_at` is false, and 4096 over the cap
  introduces nothing.
- `an_introduced_world_is_the_same_at_any_worker_count`: 1, 3 and 8 workers, 40 ticks on.

Twenty planted mutants for the introduction (with `flife-runtime`'s and `flife-cli`'s) are listed in
`flife-cli`'s contract under FL-022.

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

`tests/footprint.rs` (2 tests, FL-043), on the preset at 512 founders:
- `a_founded_world_holds_its_footprint_and_stepping_never_grows_it`: at founding the allocated fields and scratch
  are the footprint's, and the state is below it. For 300 ticks with births and deaths the fields and scratch hold
  exactly their founding bytes, the state stays within the footprint's, and no staging outlives a step. The
  footprint itself does not move.
- `changing_workers_keeps_the_footprint_reserved`: at four workers the scratch and fields are larger and again
  fixed for 300 ticks with births; back at one worker the scratch is exactly the one-worker scratch, and fixed.

`tests/controller_backend.rs`'s `installing_a_backend_reserves_exactly_its_batched_bytes`: installing a backend
adds exactly `batched_bytes()` to the scratch and nothing to the state or fields, 60 ticks grow nothing, and
removing it returns the scratch to its bytes before.

`tests/inspect.rs` (5 tests, FL-021), on the 32 x 32 Appendix A world with FL-013's small ecology, which breeds.
The oracle reads the same world through `Population::get`, `Population::genome`, the configuration's topology
catalog and action decoding:
- `an_organism_snapshot_is_its_committed_row_and_genome`: three ticks after children of two birth ticks live
  beside the founders, every live organism against the oracle (row, age, topology and widths, parameter count,
  traits, decoded output, with some organism holding one); each child's age is from its own birth tick, a
  founder's is the tick; an unknown identity gives `None`.
- `a_chemistry_only_snapshot_decodes_from_zero`: under the chemistry-only ablation every held output decodes from
  zero, as the world decodes it.
- `inspecting_changes_nothing`: every organism and an unknown identity inspected before each of 20 ticks; the
  digest is unchanged by it, and equal to an uninspected world's after every tick.
- `a_snapshot_holds_what_the_committed_field_offers_where_it_stands`: every organism's local chemistry at each of
  12 ticks against §4.3's footprint, C.3's composition, §5.3's efficiency and A.1's request written out over the
  committed field's planes and the configuration (the standard library's `tanh`): the footprint's cells, weights
  and energies exactly, the efficiencies, density, request, gain and toxicity within 1e-12. It runs on the small
  world's unit cells and on 2 x 2 cells (a 16 x 16 grid), so a density is an amount over the cell's area. On each,
  the footprints read include useful and harmful cells, wrapped footprints and footprints of four cells.
- `a_weight_dump_is_the_organisms_own_controller`: once children of two birth ticks live beside the founders,
  every organism's dump is its genome's parameters whole and bit for bit, split by its catalog topology's widths
  (104, its hidden widths, 36) into consecutive layers of weights then biases covering every parameter; its
  layers, evaluated by the declared F32 arithmetic written out in the test, give `ReferenceController::sealed`'s
  outputs bit for bit on three input vectors; an unknown identity gives `None`; the digest is unchanged. Planted
  bugs, each caught: the copy one parameter short, every dump naming the first organism's genome, the layers in
  reverse, the tick one late, a layer's weights taking its biases, its biases one along, and an unknown identity
  refused rather than `None`.

`tests/measure.rs` (5 tests, plan §14.3), on the removal tests' 32 x 32 ecology. The oracle reads the committed
population and energy plane directly, tests distance with its own `rem_euclid` minimum image, and sums in identity
order and cell-index order:
- `a_measured_disk_holds_what_a_removal_of_it_takes`: at ticks 0, 40 and 75, a disk across the seam, two inside
  and one half the world wide, each equal to the oracle (both sums bit for bit). A removal of the same disk
  applied at that boundary to a world founded alike removes exactly the measured count and, bit for bit, the
  measured energy; the seam disk holds at least two organisms.
- `the_cells_on_the_circle_count_across_both_seams`: centred on cell (0, 0)'s centre with radius 3, the 29 cells
  with i² + j² ≤ 9 (18 across a seam), the four on the circle included; a radius 1e-9 smaller holds 25.
- `measuring_changes_nothing`: two disks measured before each of 30 ticks leave the digest unchanged, and the
  world is the never-measured world's at tick 30.
- `a_disk_off_the_world_or_wider_than_half_of_it_is_refused`: the removal's ten refused disks, each refused as a
  `measurement centre` or `measurement radius`; half the world is admitted and equals the oracle.
- `an_empty_disk_measures_no_organism`: no organisms, zero energy and no lineage, and the field under it.

Planted bugs, 10 with the runtime's (`flife-runtime`'s `tests/measure.rs`), each caught: the circle's cells left
out; a cell measured at its corner; lineages not made distinct; a radius refused under a removal's name; no check
of the disk; the organisms' energy one short; the next field measured instead of the committed one; the cells'
energy summed in reverse order; a running owner answering for half the radius; and a closing owner dropping the
measurement.

`tests/crop.rs` (3 tests, FL-042) and `tests/graft.rs` (3 tests, FL-042), on the measurement tests' 32 × 32 ecology
(controllers every 4 ticks in `graft.rs`). The oracles are the files' own: a `rem_euclid` minimum image on each
torus, positions and cells placed on the host's torus the same way, and the source's and host's populations,
genomes and fields read directly before the operation:
- `a_crop_holds_exactly_the_disk_and_nothing_else`: three disks at tick 75 (one across the seam, one centred, one
  at a corner): the measurement is `measure_region`'s, tick and epoch kept; rows and genomes equal the source's in
  the disk; every cell in it equal plane for plane and every other zero; the holdings are the crop's own (field bit
  for bit); the digest differs from the source's.
- `a_crop_restores_from_its_checkpoint_and_runs_on`: the crop's checkpoint restores to its digest and holdings, and
  both run 40 ticks with equal digests, its organisms living on.
- `a_crop_of_a_disk_a_measurement_refuses_is_refused`: three disks, each refused as the measurement refuses it.
- `a_crop_grafts_into_a_host_of_its_size` (moved 8 cells east and 4 north) and
  `a_crop_across_the_seams_grafts_into_a_wider_host` (a disk on the source's corner into a 48 × 48 host): each field
  cell is the package's where the oracle lands it (one per place) and the host's elsewhere, and the displaced energy
  is theirs; the host's founders outside the disk keep their rows and genomes, in order; the grafted organisms have
  the host's next identities, kin as kin, no parent, generation 0, birth tick 0, the founders' cooldown, the stable
  phase of the new identity, invalid held output, the package's velocity and energy bit for bit, the oracle's
  position, and the package's genomes with no parent; the holdings identity holds; the grafted world's checkpoint
  restores and runs 30 ticks identically.
- `a_refused_graft_leaves_the_host_as_it_was`: a host stepped once, a host of half-unit cells, an F16 host, the
  whole source as a "package", a 17-cell disk into a 16-cell host, and a host whose population cap is its founders:
  each refused for its own reason (the refusal's detail is checked), the host's digest, holdings and rows unchanged.

Planted bugs, 18, run with `tests/measure.rs`: 17 caught: the crop copying every cell or keeping every organism,
dropping genome parents, opening its ledger at the source field's energy or resetting the epoch; the rim cells left
out of a disk (`<` for `≤`: caught by the seam graft, whose radius-5 disk has rim cells 3 and 4 cells off its
centre, and by the 17-cell refusal, whose disk then fits; `tests/measure.rs` did not run for it, as cargo stops at
the first failing test binary); the cell-size, parameter-type,
outside-the-disk and landing checks removed; host founders in the disk kept; a lineage not named by its first
member; the source phase or cooldown kept; the ledger not reopened; the disk moved the wrong way; and the whole
founding check removed. One survived: removing only the founding check's tick clause, because a stepped host is
still refused by a remaining clause (its epoch or its event count; which one was not isolated). The whole check
removed is caught.

`tests/watch.rs` (7 tests, FL-021, plan §14.2's Inputs, Brain and Transactions), on the same small world. Its
oracles are not the snapshot's path: a controller backend that records every job's inputs and defers each to the
reference; `observe` of the recorded raw sample and the organism's row at the step's start; the declared F32
arithmetic (the bias, then each input's product in ascending index, `tanh` in F64 rounded once) written out over the
genome's parameters; and, for the transactions, a fresh `EnergyBudget` replaying each record, the organism's next
row, the report's births, rejections and deaths, and the tick's ledger closure:
- `a_watched_snapshot_holds_what_its_controller_read_and_its_activations`: nothing recorded before the first
  watched step; then for 48 ticks, through births, the record's tick is the step's start, it evaluated, its scale
  is its trait's, its inputs are its job's bit for bit, `observe` of its raw sample and its row gives them and its
  gradients, and the written-out arithmetic on them gives its activations and its held output bit for bit; no
  other organism's snapshot records anything.
- `the_records_follow_the_watch`: kept when the same organism is watched again (two steps' transactions, ticks 0
  and 1, the snapshot's and `watched_transactions()` alike), unchanged by a cancelled candidate, dropped for both
  organisms when the watch moves, made for the new one at its next step (one transaction), and none once nothing
  is watched.
- `at_a_slower_cadence_the_activations_are_the_held_outputs`: at an evaluation every three ticks, nine steps give
  three records that evaluated, each its job's inputs, and six that held, with no job; the activations are those
  of the newest evaluation, checked as above, and none before the first watched one.
- `watching_changes_nothing_a_run_computes`: at cadences 1 and 3, a world watching a new organism every ten ticks
  and inspecting it before each of 40 ticks publishes every tick's report and digest as one that watches nothing,
  and both breed.
- `the_inspector_refuses_an_output_the_reference_does_not_reproduce`: under a backend that flips one bit of every
  output, the watched organism's snapshot is `StateConflict`; an unwatched one's is answered.
- `every_organisms_transactions_replay_on_its_budget_and_add_up_to_the_ledger`: one world watching each live
  organism, in lockstep with an unwatched reference for 40 ticks (a world whose organism died watches a newborn;
  a newborn with no free world gets one stepped to the tick), on the small world, a starving variant (basal upkeep
  30/s, motion 10^8 per speed² second, a tiny reservoir below 0.2) and a rejecting one (no weight or architecture
  mutation, trait steps of 10^-30, so most candidates are effective no-ops). Each world's reports are the
  reference's. Each step, every organism's newest record has the step's tick and opens at its row's energy;
  replayed on a fresh budget (the gain credited, the toxicity paid, then maintenance, motion, the scaled motion's
  whole balance, emission, the parent's share kept, the tiny reservoir), every payment pays what the record says,
  each stage is recorded exactly when the budget reached it alive, a scaled motion's velocity is its proposal
  times `sqrt(E_available / cost)` (Appendix A), emission offers `ρ` times its cost, and the balance left is its
  closing energy; a survivor's next row has that energy and the recorded velocity, an
  organism that died has a death and closes at zero; a birth is the report's (its child, whose energy is the
  split's), a rejection is the report's with its candidate count, and any other outcome has neither. The records
  of every organism, summed in the ledger's order (the tiny reservoir's non-parents before the parents), are the
  closure's toxicity, maintenance, sensor and controller costs, motion and tiny-reservoir flows and its uptake,
  child-endowment and emission transfers, bit for bit. The small world bred 8 times, with cooling down and
  emissions; the starving one had 48 deaths, 39 tiny-reservoir clears and 5 scaled motions; the rejecting one
  rejected 6 births and bred 2.
- `the_watch_keeps_its_last_steps`: over 131 steps, the snapshot carries one record a step up to 128, oldest first
  and consecutive, the newest the step just published.
- Planted bugs, each run against the tests that should catch it, all caught (with the trace's, the session's, the
  panel's and the record's, 18 of 18): every record marked evaluated, the record named by the step's end tick or
  encoded at its age then, no evaluated or no observed record kept on publish, the records kept when the watch
  moves or carried by every organism's snapshot, no reproduction check, and the next row's organism recorded.
- Planted bugs in the transactions, each run against the tests that should catch it, all caught (with the panel's
  and the record's, 22 of 22): the record opened after chemistry; maintenance not recorded; motion recorded at its
  proposed velocity; emission recorded only when it emitted; cooling down recorded as no attempt; a birth naming
  its parent as the child; a non-parent's tiny reservoir not recorded; the closing energy the opening; one step
  more than the bound kept; the old transactions kept when the watch moves, or carried by every organism's
  snapshot; every organism but the watched one recorded; recording that changes the step; a rejected birth's
  candidate count off by one; and the step's transactions dropped on publish. The proposed velocity and the
  rejected count survived the first run: no scaled motion leaves a survivor whose next row could show its
  velocity, and no step had rejected a birth. The scaled-motion check and the rejecting variant catch them.
- Planted bugs in the local chemistry, each run against the tests that should catch it, all caught (with the
  panel's and the record's, 20 of 20): the footprint taken at the origin; each efficiency the next cell's; the gain
  counting harm; the toxicity without `κ`; the request at twice the density; the density without the cell's area;
  a cell's energy its first moment; the footprint's first cell dropped; every cell weighing a quarter; the snapshot
  reading the candidate's field; and the harm counting gain. The density without the area survived the first run,
  whose cells were 1 x 1; the 2 x 2 grid catches it.

`src/ancestry.rs` unit tests (8, FL-021, plan §14.2's Descendants and §2.2's following), the index against a whole genealogy that
forgets nothing, in random ticks of at most three births (half of them to one of the three newest organisms, so
lines grow deep), deaths and introductions in a population of at most 24:
- `the_index_says_what_the_whole_genealogy_says`: for four seeds, 3,000 ticks from six founders; after every tick
  every live organism's `Descendants` (counts, path and `beyond`) equals the genealogy's, a dead one has none, the
  members less their dead are the live, the dead fewer than the live, and the room never grows. Each seed cleared
  out leavers more than 20 times, named a path past eight ancestors, and put a dead ancestor on a path.
- `an_index_begun_later_starts_from_the_live_population`: for two seeds, an index begun at tick 400 from the live
  population, with at least one organism whose parent died, equals a genealogy rebased there over 1,500 ticks.
- `events_the_index_cannot_hold_are_refused`: a birth to a parent unknown or dead, a child or introduction at or
  below an identity taken, and a death unknown, already dead, or above every identity and not born that tick;
  a birth whose child dies in the same tick, another death and an introduction above them are taken.
- `a_followed_line_passes_to_the_nearest_eldest_living_descendant`: for four seeds, 3,000 ticks, a line followed
  from a random live organism, and again whenever it ends; after every tick it
  equals the genealogy's, whose heir is the live descendant, by whole parent chains, fewest generations below
  the dead organism and then the lowest identity, chosen after the tick's deaths, with the tick it passed and its
  hand-offs; a line with no live descendant ends naming the organism that died. Over the seeds more than 30
  hand-offs, more than 10 lines ended, heirs below a dead child, and heirs nearer that were not the eldest
  descendant alive; the room never grows.
- `a_line_followed_in_an_index_begun_later_passes_through_its_lines`: for two seeds, an index begun at tick 400,
  following likewise against the genealogy rebased there: at least 10 hand-offs, a line ended and a deeper heir.
- `only_a_live_organism_is_followed`: a dead and an unknown organism are refused, keeping the line followed;
  following the heir that has the line begins a fresh one from it; `None` clears it.
- `the_heir_is_chosen_once_the_ticks_deaths_are_folded`: a parent and its elder child die in one tick, listed in
  either order; the line passes to the younger child, not the elder child's child.
- `a_child_born_in_the_tick_its_parent_dies_is_its_heir`: a parent with one child born in the tick it dies passes
  the line to it; that child's death later ends the line.

`tests/ancestry.rs` (4 tests, FL-021, plan §2.2), the 32 x 32 Appendix A world at a population of 512 with 48 founders and
eight 240-power sources, a removal brush (radius 5, at one of four places in turn) at tick 50 of every hundred and
six organisms introduced at tick 150; the genealogy is built from the reports alone, each birth's row checked for
its parent, generation and birth tick:
- `a_snapshot_says_what_the_whole_genealogy_says`: traced from the founding, every 25 ticks to 1,200 each live
  organism's snapshot `descendants` equals the genealogy's (6,592 organisms checked); 262 births, 97 deaths, at
  most 38 living below one organism, a path of 6, and dead ancestors on paths.
- `a_world_traced_later_starts_from_its_live_population`: untraced, a snapshot has no descendants and the world
  holds no index; traced at the first tick from 300 on at which a live organism's parent is dead (300), it equals
  the genealogy begun there, every 25 ticks for 500, with `since` kept when traced again.
- `tracing_changes_nothing_and_stays_in_its_room`: the room is 512 x 3 and the founders its 48 members, and the
  footprint's and the admission's state are an untraced world's plus the index's bytes; for 1,200 ticks with the
  brushes, every tick's report and digest equal an untraced world's, the members less their dead are the live,
  fewer than twice them, the index's bytes do not change, and a dead ancestor is held.
- `a_followed_line_passes_to_the_heir_the_genealogy_names`: an untraced world refuses to follow an organism and
  follows `None`; a traced world, told by a world stepped ahead which organisms die soonest, follows the live
  organism with a live descendant that dies soonest (with none, the one that dies soonest), again whenever its
  line ends, and to 1,200 ticks its line
  equals the genealogy's heir after every tick, while every tick's report and digest equal an untraced world's.
  In one run 8 lines, 3 hand-offs and 7 ended; at least 3 hand-offs and one ended are asserted (the brushes'
  deaths are mostly of organisms with no descendant alive).
- Planted bugs in the ancestry index, each run against the tests that should catch it, all caught (with the
  Studio's host, panel and record, 23 of 23): a dead organism with one line kept on the path; a dropped line not
  taken from its ancestor; a birth counted born for its parent alone; a child's death leaving its parent's living
  children; a death not taken from its ancestors' living; clearing out without carrying links past leavers;
  leavers never cleared out; a child at its parent's generation; an introduction born a tick late; no ancestor
  past the path counted `beyond`; every ancestor named alive; a later trace counting no living above a seed; a
  birth to a parent not held alive, and a death above every identity not born that tick, admitted; the index
  never folding a tick; a later trace counted from the founding; and the footprint not charging the index.
- Planted bugs in following (plan §2.2), each run against the unit tests and `tests/ancestry.rs`, all caught:
  the heir the furthest generation, the youngest of its generation, or the eldest whatever its generation; the
  heir chosen as the organism's death is folded; descendants marked after every death; only children marked; a
  dead descendant made heir; a line that never ends; the heir's tick not kept; hand-offs not counted; a dead
  organism followed; and an untraced world following silently. With the session, epoch and Studio, 24 of 25
  (the Studio's contract).

**Validity domain.** `field-ecology-v1` with `bounded-affinity-v1` and `per-granted-cell-packet`, at sensing-scale
cost exponent 1; FL-011 at `no-emission-no-charge`, `post-motion`, `next-tick` visibility and activation, and
`pause-before-commit` at the birth stage; FL-012 at `dissipate-and-record` and `genetic-affinity`; FL-013 at
`cpu-reference`, `qualified-reference`, `stop-and-preserve`, `pause-before-commit`, `uniform` positions and `zero`
velocity, first-order splitting at the configured fixed timestep.

**Known gaps.**
- FL-042: a crop is a disk; a graft moves it by whole cells only (no rotation, no sub-cell move) and only into a
  founding world, so a mid-run transplant with ledger flows is not built. Grafted organisms are founders; their
  ancestry, ages and cooldowns are not carried. `graft`'s `numerical-invalid` and `budget-exhausted` genome-quota
  paths are not tested (the population's is). The crop's and graft's memory is not charged to `admit_memory`.
- C.11's near-zero emission fractions and birth hazards are decoded and tested in `flife-neural` (FL-007). FL-011
  settles them, including the exact zero limits.
- FL-021's snapshot covers §14.2's identity, state, chemistry and brain architecture, and, for the watched
  organism, its inputs, sampled activations and the transactions of its last 128 steps. A snapshot taken right
  after the watch is set, or in a world not stepped since, has none of them: they exist only inside a step. The
  transactions are the watched organism's alone, from when it was watched; an organism's earlier history is not
  kept. Its local chemistry is the committed field's, not the next step's settlement. Its descendants exist only
  while the world traces its ancestry, and count from when tracing began: a checkpoint holds no ancestry, so a
  resumed or branched world traced again starts from its live population, and an organism whose parent died
  before then starts a line of its own. Only the Studio traces; `flife run` and `flife inspect` do not. The path
  leaves out dead ancestors through which a single living line passes, and names no ancestor's identity once it
  is dropped. A weight dump is on demand (`World::organism_weights`), through `flife inspect --weights`, a
  session's `SessionHandle::organism_weights` and the Studio's W key.
- Phase 1 applies what `flife-runtime` seals: single-cell `InjectEnergy` (FL-014) and `InjectMaterial` (FL-022),
  disk `RemoveOrganisms` (FL-022), and single-cell `IntroduceOrganisms` (FL-022, entering in phase 8). Area
  brushes, introducing a chosen genome rather than founder draws, source edits and the native input that maps a
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
  allocates nothing, a hotspot included. FL-043 reserves the rest of the scratch for the capacity and charges each
  tick's own lists at the staging bound. That bound is analytic: no test makes every organism reproduce in one
  tick.
- The live-population quota counts every parent with a viable attempt as live. Its kept share is at least
  `minimum_parent_energy_after_split`, so only a minimum below the tiny threshold could make that conservative.
- Resource epochs here are the committed world's: there are no leases, observers or render projections yet
  (FL-017), and the reference has no device fences. A candidate is exclusive by borrowing, not by a lease.
- `execution.cpu_workers` (or `World::set_cpu_workers`) sets the threads for organism decisions, phase 5's
  demands, phases 6 and 7, and the field's step, reductions and check; no result depends on it
  (`the_cpu_worker_count_changes_no_result`: 1, 2, 3, 6 and 64 workers, 96 ticks with births). Decisions read only
  committed state and split into contiguous identity-ordered chunks of at least 128, the first error in identity
  order winning; phases 6 and 7 split the same way and keep the ledger's and the deposits' order (see the scratch
  section). The field splits transport by plane and the source supports by source; the energy plane and each
  moment plane keep their per-source deposit order. The
  threads are scoped `std` threads spawned per tick: a bounded, reported reference adapter (§3.4, §12.5), not
  Asupersync-scheduled work. Phase 5's demands and phase 6's packet compositions run on the lanes too, and phase
  9's grant debit and deposit reductions and phase 10's field check on the field's workers (see phases 9 and 10).
  Phase 5's grant resolution, phase 10's holdings closure and population checks, and the commit stay on one
  thread, and a persistent pool is later work. Parallel phases must keep these reduction orders.
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
  it is not state: a restored world steps on the reference until its owner sets one. FL-032: the same backend's
  `reduce` runs phase 9 (the grants' debit, then the emission and recycling deposits) into the candidate, and
  its two summaries are recorded in the tick's ledger as the reference's are. The port guarantees that a
  backend that errs has written and applied nothing, so the reference then reduces the same grants and deposits
  into the same candidate; the backend is dropped and the failure recorded. `FieldReport::reductions` counts
  the device's reductions and `reference_reductions` the ones handed back. Phase 10's field check goes to the
  same backend's `check`. The host builds the tick's `FieldSummary` from its scan with
  `FieldState::summarize_scan`. When the scan names a failing cell, the reference's check gives the tick's
  error. If the backend errs, its scan is refused, or it names a failing cell the reference passes, the
  reference's check is the tick's, the backend is dropped and the failure recorded. `FieldReport::checks` and
  `reference_checks` count each path. The tests are in `flife-backend-metal`'s `tests/field_world.rs`,
  `tests/reduce.rs` and `tests/check.rs`.
- `World::set_sensor_backend` (FL-031) hands phase 3's raw sampling to a `flife_field::SensorBackend`, which must
  equal `PyramidView::sample`. The world still builds the reference pyramid. It then gives the backend every live
  organism's query (its position and its genome's sensing scale) in row order, once per tick, before the lanes
  run. Each lane's observations read the backend's sample at the organism's row, `start + index`. The
  encoding, controllers and decoding are unchanged. If the backend errs, or returns a different number of
  samples, the lanes sample on the reference instead, and whatever the backend left in its samples is not read.
  Once the reference has made the tick's decisions without an error, the backend is dropped and
  `World::sensor_report` (`SensorReport`: backend, device and reference ticks, failure) records why. If the
  reference errs too, its error is the tick's. No result depends on the backend, and it is not state. The tests
  are in `flife-backend-metal`'s `tests/sensor_world.rs`.
- Phase 5's demands are `flife_field::demands` with the world's `RequestRule`, over the working organisms in
  identity order: the same expressions as before. On the reference they are computed on the lanes, each lane
  taking a contiguous identity-ordered run of at least 128 organisms and computing each one's `flife_field::demand`
  into its own slot; a demand reads only the observed field, so the demands are the same at any worker count, and
  the first refusal in identity order is the tick's. `World::set_uptake_backend` (FL-031) hands phase 5's
  densities, requests, cell demands and quanta to a `flife_field::UptakeBackend`, through `demands_on` and
  `resolve_uptake_on`. If either errs, the tick's phase 5 is redone on the reference from the
  same observed field, and the backend is dropped, with `World::uptake_report` (`UptakeReport`: backend, device
  and reference ticks, failure) recording why. If the reference errs too, its error is the tick's. Installing a
  backend reserves its `UptakeWork` for the population's capacity on the grid (FL-043, counted in the footprint's
  scratch). If the host refuses that, the backend is not installed and the report says why. No result depends on
  the backend, and it is not state. The tests are in `flife-backend-metal`'s `tests/uptake_world.rs`.
- Phase 6 runs each organism's stages through one function (`settle_stages`) over a `Stages` source: the
  reference's operators, or a backend's outcomes. It returns the organism's settlement (`Settled`: budget,
  vitality, velocity, position, emitted, birth intent), which is written to the row afterwards; the stage
  order, exhaustion checks, ledger flows, watched record and reproduction proposal are unchanged.
  `World::set_settle_backend` (FL-032) hands phase 6's chemistry, maintenance, motion and emission, and phase
  7's reproduction proposal, to a `flife_field::SettleBackend`. The host replays the backend's settlements on each organism's own budget with
  the reference's payments, on the lanes as the reference runs them, and refuses (`StateConflict`, "replay a
  settle backend") a payment, balance, exhaustion flag, maintenance split, scaled flag, emission request or offer
  that differs by a bit, a position off the torus, a deposit for an organism that does not emit or for a zero
  request, a first death at another stage, or a deposit split `push_split` refuses. It recomputes emission's
  request and offer; it takes from the backend what chemistry absorbed and gained, the toxicity, maintenance and
  motion requests, the velocity, the position and the split, which racing the worlds shows to be the
  reference's. No row is written until every organism has settled. If the backend or the replay errs, the
  tick's phase 6 is redone on the reference from the same rows, and the backend is dropped, with
  `World::settle_report` (`SettleReport`: backend, device and reference ticks, failure) recording why; if the
  reference errs too, its error is the tick's. For the proposal, the backend gets every organism alive after
  emission with its post-emission balance, the attempt probability and child fraction, and the constants of
  `Reproduction::proposal_constants` (the world seed and the reproduction rules). The host checks the cooldown
  itself and refuses an outcome that disagrees ("proposed"), checks the probability as `attempts` does, and
  recomputes an attempt's split with `Reproduction::attempted` (`propose`'s tail, factored out with its
  arithmetic unchanged), refusing a split or an unviable reason or value that differs by a bit ("split as").
  It takes from the backend whether the Bernoulli draw made an attempt. Phase 8's step 2 for each birth's first
  candidate goes to the same backend, in an inherited world whose replacement or weight rate is above zero: the
  host has `Mutator::unmutated` choose each first candidate's member and lay the parent's parameters onto it
  (copied, or mapped for another member) in one buffer of the run's dtype, and hands the backend a row per birth
  (parent, tick, candidate 0, the member's layers and parameter count) with the world seed and the two rates. It
  takes back the mutated values and each birth's replaced and perturbed counts, checking only that there is one
  count per birth and that a birth's replaced and perturbed do not exceed its parameters ("counted",
  "parameters"); the values and counts are otherwise taken as given, and racing the worlds shows them to be the
  reference's, F32 and F16. `resolve_births_with` hands them to `Mutator::propose_prepared`, whose traits, no-op
  check, seal and later candidates are the reference's own. A birth whose candidate the host cannot lay out is
  proposed by the reference. If the backend errs or a check refuses, the tick's births are all proposed by the
  reference, the backend is dropped, and the report records why, counting the tick in `reference_ticks`;
  `SettleReport::mutated` counts the births whose first candidate the backend mutated. Installing a backend
  reserves its lists for the population's capacity (FL-043, counted in the footprint's scratch); if the host
  refuses that, the backend is not installed and the report says why. No result depends on the backend, and it
  is not state. The tests are in `flife-backend-metal`'s `tests/settle_world.rs`.
- First-order splitting only. Strang splitting and timestep-refinement studies are later items with their own
  scheme identity (§8.4).
- INV-08 is tested by isolation: an organism outside the field's reach evolves bit for bit as if alone.
- FL-016 encodes the world (INV-14) and restores it to the same trajectory, and FL-035's `flife-store` publishes
  it atomically and recovers the last valid one. Resuming a session from the store is FL-037. Genomes are stored whole, one copy per
  live genome: 1.4–3.6 MB for the 48-founder ecology and 161.3 MiB for the 4,096-organism preset, with no page
  sharing, deltas or compression. Each parameter is its bits in the run's weight dtype: four bytes for `f32`, two
  for `f16` (FL-030, read back through the configured dtype; an `f32` checkpoint's bytes are unchanged). An F16
  world resumed mid-run follows its original bit for bit, genomes included (`tests/f16_world.rs`). A restored store is compacted in identity order, so `Population::latest`
  names the highest live identity rather than a since-dead later one; new identities come from the restored
  allocators either way. The checkpoint carries no toolchain or source fingerprint (B.1's run manifest).
- Explicit removal is carried by `RemoveOrganisms` (FL-022), a disk. Other region shapes and the erase brush's
  native input are later. Environmental hazards are not part of `field-ecology-v1`.
- `Population` stores each genome as its own immutable heap value, and FL-026's arena slots name them to the
  controller backends, which keep packed copies by slot. A slot is released in the commit that removes its
  organism, and a backend serves a job only while the slot holds that generation and genome; there is no
  epoch-based reclamation beyond that. FL-016 checkpoints the store's contents. The ordered
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
