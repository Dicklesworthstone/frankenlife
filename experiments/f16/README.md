# F16-E1: compact F16 genomes against F32, long-run ecology (plan §24.2)

Plan §24.2 leaves "F16 canonical-genome suitability" open, to be settled by mutation fidelity, short-run parity
and a long-run ecological comparison. This directory is the record of the third: the reference small world with
compact F16 genomes (`controller.canonical_weight_dtype = "f16"`, plan §10.3: binary16 parameters rounded once
from F64, a distinct quantized evolutionary model) against the same world in F32. Mutation fidelity is measured
by flife-genome's `f16_mutation_fidelity_under_appendix_a_rates` (0d76dd3). Short-run parity is not measured
here. The decision stays with the model lane.

## Result

By the declared rule, F16 differs from F32 on **no primary measure**. As the specification says, that does not
show the two are equivalent: each effect is given below beside the seeds' spread.

| Primary measure | F32 mean (sd), discovery / held-out | F16 − F32, discovery | F16 − F32, held-out |
|---|---|---|---|
| final live | 4,534 (296) / 4,746 (374) | +26.9, 4 higher of 8, p=0.80 | +18.3, 3 of 8, p=0.95 |
| births | 19,198 (518) / 19,140 (1,350) | +207, 6 of 8, p=0.10 | +218, 5 of 8, p=0.23 |
| lineages alive | 40.6 (7.4) / 44.3 (10.6) | +1.25, 4 of 8 (1 equal), p=0.59 | −2.1, 2 of 8 (1 equal), p=0.36 |
| organism Morisita | 2.476 (0.280) / 2.411 (0.216) | +0.059, 7 of 8, p=0.023 | −0.019, 3 of 8, p=0.60 |
| field Gini | 0.5795 (0.018) / 0.584 (0.030) | −0.0023, 2 of 8, p=0.34 | −0.0070, 2 of 8, p=0.070 |
| topologies alive | 5 (0) / 5 (0) | 0, 8 equal | 0, 8 equal |

Effects are mean paired differences by seed; "k of 8" counts seeds where F16 is higher; p is the exact two-sided
sign-flip p from the report.

- Organism Morisita is the one primary with a discovery effect at p < 0.05 (F16 more clustered on 7 of 8
  seeds); held out, the sign reverses (3 of 8, p=0.60), so by the rule it does not replicate.
- Field Gini is lower under F16 in both splits (2 of 8 higher in each), but neither split reaches p < 0.05. The
  effects (0.0023, 0.0070) are a small fraction of the seeds' spread (sd 0.018 and 0.030).
- Births are higher under F16 in both splits (+207 and +218; 6 and 5 of 8), at p=0.10 and 0.23.
- Topologies alive is 5 on every run of both conditions, so on this preset it cannot show a difference.

## Provenance

- Specification `spec.toml` (SHA-256 `6950f23e…c32dff5`), written before any run, with the primary measures, the
  rule and the cross-check in it. Its hash was logged at launch (2026-11-12 19:55:37 EST). It was committed in
  0d76dd3 at 19:58:55, while the runs were in progress. No result was read until the end record (21:01:48).
- Base preset `../fl048/reference-small-world.toml` (SHA-256 `064f583f…406b`): 4,096 founders, a 128 × 128
  grid, ticks of 1/128 s, 8,192 ticks (64 simulated seconds). Seeds are FL-048 E1's: discovery 1–8 and held-out
  1001–1008.
- `flife experiment`, release binary SHA-256 `ecb19706…d67a8a6fc`, built from the 0b5a3eb tree. The tree's
  uncommitted changes at build time were the flife-genome CONTRACT, a test file and this specification, so no
  product code was involved. The controllers were CPU-reference, on 16 workers. Reports do not depend on the
  worker count (FL-041's tests).
- Hardware: Apple M5 Ultra, macOS.
- Cross-check, declared with the rule: all 16 F32 runs reached the digest that FL-048 E1's control run had reached
  on the same seed at tick 8,192. F32 semantics on this preset are therefore unchanged between 372cbc8 and
  0b5a3eb. The F16 runs' digests differ from F32's on all 16 seeds, as expected for a distinct model.

## Endings and the colony criterion

All 32 runs reached tick 8,192. None went extinct, paused at the population cap or failed. FL-048's H1 colony
criterion (organism Morisita > 1 and Moran's I > 0 at the last sample) holds on all 32. Both conditions show the
preset's early bottleneck to the same depth: least live was 434–615 under F16 and 434–613 under F32.

## Secondary measures (exploratory)

Among the 18 secondary measures, one has p < 0.05 in either split: organism Moran's I held-out (+0.0099, 7 of
8, p=0.016). Its discovery effect has the opposite mean sign (−0.0018, 5 of 8 higher, p=0.47). Distinct genomes
equals final live on every run, so its effects repeat final live's. Every secondary effect is in the report
and recomputable from `records.ndjson`.

## Limits

- Eight seeds a split allow p down to 0.0078. The rule is a consistency rule, not an equivalence test: it
  detects only effects whose sign is consistent across seeds. For final live, the paired differences' sd is 258
  (discovery) and 150 (held-out), so mean effects of the size seen (+27 and +18) are well inside the noise.
  There is no multiple-comparison correction across the six primaries.
- This is one preset, at 4,096 founders, over 64 simulated seconds. Plan §19.5 makes F16 the default for the
  Desktop (100k) and Target (400k) profiles, and neither their scale nor longer horizons are tested here.
- These measures are ecological aggregates at one sample, so they say nothing about behaviour that leaves the
  aggregates unchanged.

## Files

- `spec.toml`: the specification as run. Its `base` resolves to FL-048's frozen preset.
- `records.ndjson`: the `flife experiment` records, verbatim from its output: the start record, 32 run records
  and the end record. The end record's report path is shortened to `out/report.json`; its hash is the report's.

Outside git, in session scratch storage on the build SSD (not archived), by SHA-256:

| Artifact | SHA-256 |
|---|---|
| Report (`flife experiment`) | `2e918e4578473354c539155fd8a45d4fb31c108da55306416ae26e51edda09b3` |
| Binary | `ecb19706861541713e9915a13e43d32f1dd5cbb8ef8a29243d50683d67a8a6fc` |
