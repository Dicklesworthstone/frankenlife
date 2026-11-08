# flife-genome — contract

**Purpose.** FL-009 (`flife-4wf`) covers immutable genomes and the topology catalog (plan §6.4, §6.5, §7.1, §7.2, §7.5,
§9.2, A.2, A.3 items 2, 5 and 6, Appendix C.16):
- `dense-104-36-catalog-v1`, the five dense topologies a controller may take, under the run's parameter cap;
- immutable genomes, each holding one organism's own controller parameters and heritable traits;
- the fixed logical genome quota and the allocated quota, with checked, fallible allocation;
- the deterministic part of `fan-in-scaled-symmetric-v1`, from raw words to founders' parameters;
- the transition maps `overlap-plus-zero-output-v1` and `prefix-and-identity-bridge-v1`.

Random words, mutation and target selection are FL-010. Births and deaths are FL-011 and FL-012. Packed
per-topology arenas are FL-026.

**Inputs.**
- A `ValidatedConfig` (`[architecture_mutation]`, `[controller]`, `[chemistry]`, `[sensors]`, `[execution]`), or
  the constants directly.
- Raw 64-bit words, one per `WeightAddress` (layer, receiving neuron, sending neuron), supplied by the caller.
- A parent's parameters and topology, for a transition.

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

**Refusal cases.**
- A default topology that is not a member, or is above the cap. Another catalog, inheritance map or depth-change
  map (`UnknownAlgorithm`).
- A member above the parameter cap, or a quota that cannot cover a genome (`BudgetExhausted`, with what was
  required and what was available). Quotas out of order.
- An unrepresentable or host-refused buffer (`AllocationFailed`).
- A parameter or trait outside its bound, a nonfinite value, or a sensing scale out of range. Nonpositive or
  nonfinite bounds.
- Releasing a charge the quota does not hold (`StateConflict`).
- A buffer of the wrong length, a zero fan-in, or a depth change other than one-to-two or two-to-one.

**Determinism.** Pure functions of their inputs, in fixed (layer, output, input) order, using only IEEE-754
arithmetic and the correctly rounded `sqrt`. No hash maps and no draws of its own.

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

**Validity domain.** `dense-104-36-catalog-v1` with `overlap-plus-zero-output-v1`, `prefix-and-identity-bridge-v1`,
F32 canonical weights and `independent-dense-mlp-v1` controllers, with traits for the 32-channel field of
`field-ecology-v1`.

**Known gaps.**
- Founders' and added neurons' words come from the caller. FL-010's frozen Philox semantic addressing
  (`fnp-philox-semantic-v1`) completes `fan-in-scaled-symmetric-v1`, which stays `Specified` under FL-010.
- Mutation (weight perturbation and replacement, trait mutation, sensing-scale mutation), topology-change
  selection (`uniform-other-catalog-member`) and the mutation record in provenance are FL-010.
- Genomes are separate heap buffers. Packed immutable slabs by topology, generation-tagged handles and device
  sharding are FL-026. A store with an identity map, epoch-aware reclamation and checkpointed genome identity
  belongs to the tick and the store (FL-013, FL-014).
- Research-mode pause and exhibition-mode birth rejection on quota pressure (§7.5) are policies of the birth
  transaction, FL-011. This crate reports the refusal.
- A backend-compatible materialization of each transition (C.16) is FL-025 and FL-030.
