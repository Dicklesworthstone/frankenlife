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

The ordered tick that calls them is FL-013. The crate computes settlements, proposals and staged births, and stores
the committed organisms; the tick decides when each runs. It does not own the field.

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
  `HeldOutput` hides an invalid output entirely.

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

**Determinism.** Pure functions of their inputs. Sums run in channel or packet order and the packet sums are
compensated. The only elementary functions are `flife_core::numerics` (`exp_neg`, `expm1_neg`, `tanh`) and the
correctly rounded `sqrt`, so every IEEE-754 host computes the same bits. The integrated fixture pins a digest of
its final organisms and field. FL-011's randomness is `fnp-philox-semantic-v1` on the frozen `birthing` stream,
addressed by parent identity and attempt tick, plus the mutation stream. Staging order is ascending parent
identity, so the births do not depend on request order, worker or storage slot. FL-012 draws nothing. The
population's index is a `BTreeMap` and its free list a `BTreeSet`, so every order it exposes is identity order or
a deterministic function of the commit history.

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

**Validity domain.** `field-ecology-v1` with `bounded-affinity-v1` and `per-granted-cell-packet`, at sensing-scale
cost exponent 1; FL-011 at `no-emission-no-charge`, `post-motion`, `next-tick` visibility and activation, and
`pause-before-commit` at the birth stage; FL-012 at `dissipate-and-record` and `genetic-affinity`.

**Known gaps.**
- C.11's near-zero emission fractions and birth hazards are decoded and tested in `flife-neural` (FL-007). FL-011
  settles them, including the exact zero limits.
- The ordered tick (FL-013) has to do several things with these pieces:
  - string the stages together per organism and record each flow;
  - reduce emission deposits before recycling, both after the uptake debits;
  - commit survivors' new state (including `(E_next − E_previous)/Δt`), births and deaths with publication;
  - pause the whole candidate on a `CapacityShortfall`;
  - stop the run and preserve the candidate on a numerical fault (`stop-and-preserve`, now owned by FL-013).

  The exhibition-mode stable-ID birth rejection policy has no registered name yet.
- Explicit removal needs a command to carry it, and the erase brush is a later command item. FL-012 supplies only
  the settlement. Environmental hazards are not part of `field-ecology-v1`.
- `Population` stores each genome as its own heap value. Packed slabs, generation-tagged handles,
  epoch-aware reclamation after readers finish, and checkpointing the store are FL-013, FL-016 and FL-026.
  The caller retires a collected genome's quota charge at once; readers holding its shared parameters keep them.
- Field reservoirs are not cleared by the tiny-energy policy: A.1's "clears its moment consistently" has no
  organism moment to act on in this model, and F32 subnormal cells are a numerical-profile matter (the field
  contract's cone-tolerance gap).
- Sensor gradients come from FL-007.
- A Metal implementation is FL-032; its parity with this reference is a separate numerical profile.
