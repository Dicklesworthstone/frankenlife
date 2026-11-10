# flife-genome — contract

**Purpose.** FL-009 (`flife-4wf`) covers immutable genomes and the topology catalog (plan §6.4, §6.5, §7.1, §7.2, §7.5,
§9.2, A.2, A.3 items 2, 5 and 6, Appendix C.16):
- `dense-104-36-catalog-v1`, the five dense topologies a controller may take, under the run's parameter cap;
- immutable genomes, each holding one organism's own controller parameters and heritable traits;
- the fixed logical genome quota and the allocated quota, with checked, fallible allocation;
- the deterministic part of `fan-in-scaled-symmetric-v1`, from raw words to founders' parameters;
- the transition maps `overlap-plus-zero-output-v1` and `prefix-and-identity-bridge-v1`.

FL-010 (`flife-rf8`) adds founders and mutation (plan §7.2, §7.3, §16.2, A.3 items 1–4, C.16, INV-11):
- founders' genomes from `fnp-philox-semantic-v1`, by stable organism identity;
- bounded mutation proposals: at most eight candidates per attempted birth, each with its own semantic lanes,
  topology change first (`uniform-other-catalog-member`), then parameters, then traits;
- `reject-birth-no-charge` when every candidate is invalid or an effective no-op.

Births and deaths are FL-011 and FL-012. FL-026 (`flife-fsy`) adds `GenomeArena`, the allocator of arena slots
(§9.2): per topology, the lowest free slot first, each reuse at the next generation, so a dead genome's name is
stale. A slot whose generation reaches `u32::MAX` is retired, not reused. `reserve` before a commit makes the
commit's inserts and releases allocation-free. The slots are physical, not scientific state.

**Inputs.**
- A `ValidatedConfig` (`[architecture_mutation]`, `[controller]`, `[chemistry]`, `[sensors]`, `[execution]`), or
  the constants directly.
- Raw 64-bit words, one per `WeightAddress` (layer, receiving neuron, sending neuron), for `initialize` and
  `map_parameters` called directly.
- A parent's parameters and topology, for a transition.
- A `SemanticRng` (FL-010), and a `BirthAttempt` (parent organism, event tick) for each proposal.

**Outputs.**
- `TopologyCatalog`: members in catalog order with stable `TopologyId`s, `find`, `admits`, the founders' default
  topology and `transition_targets` (the other admitted members).
- `GenomeQuota::allocate` returns a charged, zeroed `ParameterBuffer`. `Genome::seal` turns a filled buffer into a
  `Genome` or returns it as `Unsealed` with the reason. `abandon` and `retire` return charges, and `capacity_for`
  reports how many more genomes of a topology fit.
- `Genome`: identity, topology, parameters (with a `SharedParameters` read-only handle), `Traits` (affinity
  direction, 33-term motion response, sensing scale), parent identity, logical and allocated bytes, and its
  `ReferenceController`.
- `initialize` fills founders' parameters. `fan_in_weight` gives one weight from a word.
- `map_parameters` writes a child's parameters from a parent's and reports the `Transition` kind.
- `founder` and `founder_traits` (FL-010): a founder's charged, sealed genome and its traits.
- FL-043: `GenomeArena::allocated_bytes` (its buckets' capacities) and `GenomeArena::bound_bytes(topologies,
  live)`, the most it holds for `live` genomes over at most `topologies` topologies: three slots' records for each
  genome in each topology, since a bucket grown by pushing holds at most twice its length and three times while it
  moves.
- `MutationParameters` (from `[mutation]` and `[architecture_mutation]`) and `Mutator::propose`, which returns
  either `Proposal::Child` with its `MutationRecord` or `Proposal::Rejected` with every candidate's
  `RejectionReason`. Also `perturb` and `reflect_log_scale`.

**Invariants.**
- The catalog is exactly §6.4's five rows, in order: `104 → 8 → 36` (1,164 parameters), `104 → 32 → 36` (4,548),
  `104 → 64 → 32 → 36` (9,988), `104 → 96 → 64 → 36` (18,628) and `104 → 128 → 96 → 36` (29,316). §6.4 calls them
  examples; this catalog version fixes them, and another membership is another catalog name. A member above the
  parameter cap stays listed, so identities never shift, but no genome may take it and no transition may target it.
- A genome is validated once, when sealed: the parameter count matches its member, every parameter is finite and
  within `±maximum_absolute_parameter`, `||a||₂ ≤ affinity_norm_limit`, `||χ||₂ ≤ motion_response_norm_limit`
  (both in F64, as `flife-engine` checks them), and the sensing scale lies in the admitted range. After that it
  never changes. `Genome` is not `Clone`; it is the live record whose charge is returned exactly once.
- Independence (§6.5, INV-13). Every genome has its own buffer, even when two have identical content. Readers hold
  `SharedParameters`, so a retired genome's parameters stay readable without a copy, and they count against
  observer budgets rather than the genome quota (§7.5).
- Logical quota (§7.5, A.2). Every live genome is charged `P × 4` bytes, its full canonical F32 size.
  Deduplication never lowers that charge. The allocated quota counts the bytes actually reserved. Both are
  checked before allocation, and a refusal changes nothing. Logical is at most allocated.
- Allocation is fallible. An unrepresentable or refused buffer is `AllocationFailed`, never an abort (INV-15).
- `fan-in-scaled-symmetric-v1`. A weight of fan-in `I` is `(2u − 1) √(3/I)` with `u = k 2^-52`, where `k` is the
  word's leading 52 bits. That is computed in F64 (`2u − 1` exact; `3/I` and `√` correctly rounded) and rounded
  once to F32. Biases are zero. Each weight depends only on its own address's word.
- `overlap-plus-zero-output-v1` (same depth). The leading overlapping subnetwork is copied, biases and output biases
  included. An added neuron gets fan-in-scaled incoming weights from inherited inputs and a zero bias, and its
  outgoing weights are zero, so widening preserves the parent's function exactly. Shrinking keeps the leading
  neurons; it equals the parent with the dropped neurons' outgoing weights zeroed.
- `prefix-and-identity-bridge-v1` (depth change). The overlapping first affine map is copied, added first-layer
  neurons are initialized as above, and output biases are copied.
  - Deepening `h → (a, b)` inserts a rectangular identity bridge with zero bias. It joins first-layer neuron `j`
    to final neuron `j` only when `j` was inherited (`j < h`), and the old output weights from neuron `j` become
    those from final neuron `j`.
  - Flattening `(a, b) → h` discards the middle layer. The old output weights from final neuron `j` become those
    from neuron `j` when `j < min(a, b)`.
  - Every other output weight is zero. Neither map preserves the function, since `tanh` is not an identity.
- Zeroing an added neuron's outgoing influence takes precedence over every other entry, the bridge included.
  This reading of A.3 item 6 ("initialize missing first-layer neurons as above") is a decision of this crate.
- A map writes every child value explicitly, and requests words only for added neurons' incoming weights from
  inherited inputs, in ascending address order.
- Founders (FL-010).
  - The default member.
  - Weight `(layer, output, input)` from word 0 of `(founders, organism, 0, lane (0, weight, neuron), 0)`;
    zero biases.
  - An affinity normalized from 32 Box–Muller normals to the sphere of radius `min(1, affinity_norm_limit)`.
  - A motion response normalized from 33 normals to radius `motion_response_norm_limit`.
  - The configured initial sensing scale.
  - Pair `j` of lane `(0, kind, j)` gives components `2j` and `2j + 1`.
- Mutation (FL-010). Every word of candidate `c` is addressed `(mutation, parent organism, tick, lane (c, kind,
  index), draw)`. An operational retry of a tick repeats its proposals, a later tick draws fresh ones, and no two
  candidates share a word. Within a candidate, in A.3 item 4's order:
  1. Topology. Word 0 of lane `(c, topology, 0)` decides against `probability_per_birth` (zero when architecture
     mutation is disabled), and word 1 chooses uniformly among the other admitted members. The map's added
     weights take lane `(c, map weight, neuron)`.
  2. Parameters, named by their neuron in the child's topology. Draw 0 of lane `(c, weight or bias, neuron)`
     works as follows:
     - word 0 against `replacement_probability` replaces the value with the initializer of word 1, at the
       layer's fan-in;
     - otherwise word 2 against `weight_probability` perturbs it by `weight_standard_deviation` times the
       Box–Muller cosine of draw 1.
     The arithmetic is F64, rounded once to F32 with ties to even. Biases follow the same rule.
  3. Traits.
     - The affinity adds `chemistry_standard_deviation` normals and renormalizes.
     - The motion response adds the same deviation and projects radially into its ball when outside it.
     - The sensing scale adds `sensing_log_scale_standard_deviation` times a normal to its natural log, reflects
       at the log bounds, and is kept within the range against the last bit of `exp`.
     - An operator whose rate or deviation is zero changes nothing, bit for bit.
  - A candidate with a nonfinite, out-of-bound or zero-norm value (any `NumericalInvalid`) is rejected, never
    reinterpreted. Under `reject_effective_noop`, a candidate equal to its parent bit for bit is rejected too.
  - Each rejected candidate's buffer is abandoned at once. A birth whose candidates are all rejected leaves the
    quota exactly as it was.
  - A quota or allocation refusal is returned as an error, not as a mutation outcome.
  - Only the controller (topology, weights, biases), the affinity, the motion response and the sensing scale are
    heritable. Every fixed coefficient lives in the configuration, not in the genome (§7.1).

**Refusal cases.**
- A default topology that is not a member, or is above the cap. Another catalog, inheritance map or depth-change
  map (`UnknownAlgorithm`).
- A member above the parameter cap, or a quota that cannot cover a genome (`BudgetExhausted`, with what was
  required and what was available). Quotas out of order. The budget is named by `LOGICAL_GENOME_BYTES` or
  `ALLOCATED_GENOME_BYTES`, so the birth transaction can tell a fixed logical shortfall (a pause) from a host
  allocation limit (an error).
- An unrepresentable or host-refused buffer (`AllocationFailed`).
- A parameter or trait outside its bound, a nonfinite value, or a sensing scale out of range. Nonpositive or
  nonfinite bounds.
- Releasing a charge the quota does not hold (`StateConflict`).
- A buffer of the wrong length, a zero fan-in, or a depth change other than one-to-two or two-to-one.
- Mutation parameters with a probability outside `[0, 1]`, a negative or nonfinite deviation, a candidate count
  outside `1..=8`, or a strict no-op rule with no operator able to change a genome. Another target selection or
  exhausted-proposals policy (`UnknownAlgorithm`).
- A log-scale step that cannot be reflected into range, or a nonpositive scale.

**Determinism.** Pure functions of their inputs, in fixed (layer, output, input) order, using only IEEE-754
arithmetic, the correctly rounded `sqrt` and `flife_core::numerics`. No hash maps. Every draw is a semantic block,
so results never depend on evaluation order, worker or quota state.

**Independent tests.** `tests/genomes.rs`. Each directed transition is checked against an expectation the map does
not compute:
- The catalog: §6.4's counts in order, `find`, the default, four targets per member, and a cap that lists but does
  not admit the largest member.
- Logical quota:
  - members' charges of 4,656 to 117,264 bytes;
  - A.2's arithmetic: 16,384 genomes of 29,316 parameters need 1,832.25 MiB, above the 1 GiB quota; 9,156 fit,
    and 26,875 of the default;
  - a quota one byte short of a fourth large genome, which refuses it with the exact shortfall and changes
    nothing, still admits a small one, and readmits after a release.
- Allocation refusals: the allocated quota binding alone, the parameter cap refused before allocating, an
  unrepresentable buffer as `AllocationFailed`, and a foreign charge refused.
- The initializer: edge words against independently computed F32 bits; one word per weight (9,856 for the default)
  in ascending address order; zero biases; the bound; variance `1/I` to within sampling error; order independence.
- All 20 directed transitions, classified as 4 widenings, 4 shrinkings, 6 deepenings and 6 flattenings, each mapped
  into a child buffer prefilled with NaN:
  - widenings reproduce the parent's outputs exactly over 16 inputs, and draw exactly `added × inherited` words;
  - shrinkings match the parent by index and equal the pruned parent exactly, but not the parent;
  - deepenings and flattenings match the declared map entry by entry and an F64 evaluation of it within `1e-5`;
  - C.16 through the map: `tanh(1) = 0x3f42f7d6` becomes `tanh(tanh(1)) = 0x3f245b18`.
- Sealing: an out-of-bound parameter returned unsealed and its charge abandoned; identical founders as two buffers
  and two charges; a mapped child recording its parent and reproducing the parent's outputs; readers keeping a
  retired genome's parameters; and every charge returned.
- Trait bounds and limit validation.

A manual mutation check during FL-009 planted two map bugs: a bridge that also joins added neurons, and added
neurons that keep their outgoing weights. These tests caught each of them.

`tests/mutation.rs` (FL-010). The full-path fixtures take their expected bits from an oracle that uses NumPy
2.5.4's `Philox`, Python `Decimal` and `numpy.float32`, and imports no FrankenLife or FrankenNumPy code.
- Founders:
  - the same content in forward and reverse order;
  - a founder weight pinned to the oracle's `0xbb83c278`;
  - zero biases, distinct organisms and seeds, and unit or limit norms;
  - isotropic affinity moments over 512 founders.
- Zero rates give a bit-identical child that records its parent in a new buffer.
- Each operator alone changes only its own heritable component:
  - perturbation changes every parameter;
  - replacement equals the initializer of each parameter's own word 1;
  - chemistry changes affinity and motion response within their norms;
  - sensing changes only the scale, within range;
  - a topology change equals the declared map with its own lanes.
- The same children in forward, reverse, interleaved and four-worker order. A retry of a tick repeats its child,
  and a later tick or another seed changes it.
- C.16: a parent whose every perturbation rounds back to it in F32. Exactly 8, 3 or 1 candidates are tried, all
  rejected as effective no-ops in order, and the quota is unchanged. Without the strict rule the same proposal is
  an exact clone.
- Out-of-bound candidates are rejected with `NumericalInvalid` and nothing is charged. A child accepted after
  earlier rejections records them in order, and a retry repeats it.
- Canonical rounding:
  - ties to even;
  - a replaced weight `0xbe2c8834`, a replaced bias `0xbd3628c2` and a perturbed weight `0x3e81b044`, all from the
    oracle.
- Topology targets are uniform among the other admitted members (400 attempts, five standard deviations), with a
  capped member never chosen.
- Perturbation and replacement counts over 200 children of Appendix A's rates lie within five standard
  deviations of their binomial means, and the perturbations have unit variance in units of 0.02.
- Log-scale reflection at both bounds, repeated reflection, a one-scale range, and refusals.
- Rates read from Appendix A, every invalid rate refused, and architecture mutation switched off.

Five planted mutation bugs were each caught by these tests:
- the candidate index left out of parameter lanes;
- Box–Muller from the wrong draw;
- a no-op buffer not abandoned;
- the sine component taken instead of the cosine;
- the wrong stream.

**Validity domain.** `dense-104-36-catalog-v1` with `overlap-plus-zero-output-v1`, `prefix-and-identity-bridge-v1`,
F32 canonical weights and `independent-dense-mlp-v1` controllers, with traits for the 32-channel field of
`field-ecology-v1`.

**Known gaps.**
- The birth transaction is FL-011. It owns energy and cooldown preservation on a rejected birth, the identity of
  the child, and storing the `MutationRecord` with the child's provenance.
- Only F32 canonical genomes exist. C.16's F16 rounding case, and the strict-distinct profile's choice of a
  representable neighbouring value, wait for an F16 weight dtype.
- A strict globally unique benchmark's content-hash check at birth (§7.3) is not implemented. Parent-relative
  nonidentity is.
- `MAXIMUM_CANDIDATES` is §7.3's eight. Device paths must reuse the same addresses (FL-032).
- Genomes are separate immutable heap buffers, the canonical genotype. §9.2 asks for the weights in packed slabs
  by topology; FL-026 gives each genome a generation-tagged arena slot, and the packed slabs are the backends'
  execution copies in per-topology shards (`flife-backend-cpu`, `flife-backend-metal`), not the host's canonical
  store. A store with an identity map, epoch-aware reclamation and checkpointed genome identity belongs to the tick
  and the store (FL-013, FL-014).
- Pausing on quota pressure (§7.5) belongs to the birth transaction. FL-011's `flife_engine::resolve_births`
  turns a logical refusal into a `CapacityShortfall` and rolls back; FL-013 pauses the tick on it. Exhibition-mode
  birth rejection has no registered name yet. This crate reports the refusal.
- A backend-compatible materialization of each transition (C.16) is FL-025 and FL-030.
