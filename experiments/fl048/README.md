# FL-048: first hypothesis results (H1–H5)

This directory is the record of FrankenLife's first hypothesis experiments (plan §15.4, §21.6, §23.1 FL-048):
what was run, the rules declared before the runs, every run including the paused ones, and the results stated as
"effect e over k of n seeds versus control", discovery and held-out seeds separately. It holds the specifications
verbatim, the frozen base preset, and compact result records. Run directories, checkpoints and the rendered video
are artifacts kept outside git (AGENTS.md, plan §18.5); their hashes are below.

## Summary

| Hypothesis | Test | Declared rule | Result |
|---|---|---|---|
| H1 persistent heterogeneous colonies | E1 and E2 control runs, E2 sweep | survives to tick 8,192 with organism Morisita > 1 and Moran's I > 0 at its last sample | Met on every run that reached tick 8,192 (E2: 32 of 32; E1 control: 16 of 16). A low bar: see Limits |
| H2 controllers add behaviour beyond chemistry | E1 chemistry-only, fixed-controller | held-out effect in the direction of the discovery effect, on a primary measure | Not supported. The rule is met in form by chemistry-only (final live +448.5, 8 of 8, p=0.0078; held-out +414.8, 7 of 8, p=0.023), but chemistry-only also stops emission, and no-emission alone has the same effects; post hoc, chemistry-only against no-emission (the controller's decisions alone) differs inconsistently on every measure. Fixed-controller: no effect on any primary measure |
| H3 paid emission supports niche construction | E1 no-emission | as H2, on organism Morisita and field Gini | Met narrowly, on field Gini only: emission evens the field (Gini 0.0085 lower with emission, 6 of 8, p=0.047; held-out 0.0098 lower, 7 of 8, p=0.031); organism Morisita unchanged (4 of 8 in both). Environment modification at a population cost, not demonstrated niche construction |
| H4 visual structures are functional | H4 boundary perturbation | a primary measure boundary-specific on ≥ 3 of 4 discovery and ≥ 3 of 4 held-out seeds | Declared rule met on P3 (colony field at T+256), 3 of 4 and 3 of 4; not on P1 (survival). Post hoc, the P3 effect is mostly the emptied disk refilling, and the boundary disk empties more colony than the interior disk; a functional boundary is not established |
| H5 architecture variation affects ecology | E1 fixed-topology | as H2, on topologies alive and the common primaries | Not supported: final live and births reverse sign between discovery and held-out. The declared primary topologies alive is fixed at 1 by the ablation itself, so it cannot be an outcome |

## Provenance

- Model `field-ecology-v1` and its two registered ablation models (FL-041), the preset
  `presets/reference-small-world.toml` (4,096 founders, 128 × 128 grid, 1/128 s tick; SHA-256 `064f583f…406b`,
  frozen here as `reference-small-world.toml`, byte-identical).
- E1 and E2: `flife experiment` (FL-041), binary SHA-256 `cad7fe0f5229b9a9…7d1b6`, built at 372cbc8 (FL-048 part 1,
  the sampled measures), CPU-reference controllers, 16 workers (E1) and 12 workers (E2). Reports do not depend on
  the worker count (FL-041's tests).
- H4: binary SHA-256 `2ee7faefe4ffbff6…ae307a`, built from the be7acdb tree (`flife inspect`, `RemoveOrganisms`);
  its embedded `FLIFE_REVISION` reads 4488b54 because the build script had not rerun (a known gap, FL-041's
  notes). 8 CPU workers.
- Hardware: Apple M5 Ultra, macOS. Dates: 2026-11-09 and 2026-11-10.
- Cross-check: `flife render` of the control preset on held-out seed 1001 with Metal controllers reached digest
  `9cce84e2615e579e…664c3e` at tick 8,192, the digest E1's control run on that seed reached with CPU-reference
  controllers. The rendered video (129 frames, SHA-256 `efa86a29…b647619`) is outside git.

## Declared before the runs, and what changed

- **E2** (`e2/spec.toml`): the H1 criterion, primary measures and the refinement rule were written into the
  specification before its first run. Nothing was changed after.
- **E1** (`e1/spec.toml`): its first launch was stopped within a minute, with no run cataloged, and the
  specification was amended to write the H1 criterion and the H3/H5 primary measures into it. No result had been
  seen. The stopped launch's catalog (one start record) is kept with the artifacts.
- **H4**: a pilot on the preset's own seed (42) set the smoothing, density factor, boundary radius and horizons. It
  showed that a colony region fixed at T is a poor persistence measure, so the design was changed to track
  members, add horizons and a distant noise floor, and use a magnitude criterion. The pilot is excluded from every
  claim. The declaration (rules 1–7 below) was frozen with its scripts' SHA-256s at 2026-11-10T05:35:43Z before
  any declared seed ran. The first launch failed before any `flife` ran (a wrong default binary path); the
  relaunch used the frozen scripts unchanged.

## E2: H1 and the preset's calibration (`e2/`)

Source power {480, 1920} × basal maintenance {0.4, 1.2} around the preset (960, 0.8, the control); discovery
seeds 11–14, held-out 2011–2014; 8,192 ticks (64 simulated seconds); 40 runs.

- Endings: 32 reached tick 8,192; 8 paused at the population cap (16,384), all in basal 0.4 / power 1920, on every
  seed. Paused runs are reported as pauses, never as persistence. No run went extinct or failed.
- H1: the colony criterion holds on every run that reached tick 8,192: control 4 of 4 discovery (Morisita
  2.17–2.84, Moran's I 0.75–0.80) and 4 of 4 held-out (2.24–2.95, 0.75–0.79); each of the three sweep cells that
  reached the end, 4 of 4 and 4 of 4.
- Refinement rule: the candidate was basal 1.2 / power 480 (the highest discovery mean Morisita among cells
  whose discovery runs all reached the end, 2.62 against the control's 2.52; mean effect +0.101, higher on 4 of 4
  discovery seeds). Its held-out runs all reached the end, but its held-out effect was negative (mean −0.030,
  higher on 2 of 4 seeds), against the discovery effect's sign, so by the declared rule it was not adopted and the
  preset stays as calibrated.
- Bottleneck: every control run lost about 85% of its 4,096 founders early (least live 505–665) before recovering
  to 4,177–5,267; basal 1.2 / power 480 fell to 120–155. The colonies seen at tick 8,192 descend from those
  survivors (§21.6's survival bias).

## E1: H2, H3, H5 and the controls (`e1/`)

Seven conditions (the control and six ablations, FL-041) × discovery seeds 1–8 and held-out seeds 1001–1008;
8,192 ticks (64 simulated seconds); 112 runs. Primary measures, declared before any run: final live, births and
lineages alive for every condition; organism Morisita and field Gini for H3; topologies alive for H5. Emission
paid, topology changes and mutated parameters are manipulation checks. A hypothesis is supported only by a
held-out effect in the same direction as its discovery effect. The numbers below are the report's
(`e1/out/report.json`, SHA-256 `c0ba9030…dd142f5a`, outside git; the same comparisons are recomputable from
`e1/records.ndjson`): mean paired difference against the control, seeds higher and lower of 8, and the exact
two-sided sign-flip p.

- Endings: all 112 runs reached tick 8,192. None went extinct, paused at the cap, or failed.
- Manipulation checks: chemistry-only and no-emission paid no emission (emitter ticks 0 on every run);
  fixed-controller and fresh-offspring mutated no parameter; those two and fixed-topology changed no topology
  (topologies alive 1 on every run). Each ablation did what it names.
- **H1** (control runs): the colony criterion held on 8 of 8 discovery runs (Morisita 2.07–3.02, Moran's I
  0.72–0.83) and 8 of 8 held-out runs (2.00–2.70, 0.74–0.81), with 29–60 founder lineages alive at the end. It
  also held on every run of every ablation, 112 of 112, so it does not depend on controllers, emission, topology
  variation or inheritance. As in E2, every control run first lost most of its founders (least live 434–613)
  before recovering to 4,147–5,216.
- **H2, chemistry-only** (controllers evaluated and charged as before, their actions decoded from an all-zero
  output, which also stops emission): final live +448.5 (8 of 8, p=0.0078) in discovery and +414.8 (7 of 8,
  p=0.023) held-out; births +2,125 and +1,543 (8 of 8 in both); lineages alive +2.4 (6 of 8, p=0.20) and +1.3
  (5 of 8, p=0.57). The declared rule is met in form on final live and births: the world holds more organisms
  without the controllers' decisions. But no-emission alone moves the same measures by the same amounts (final
  live +493.5 and +326.9, births +1,958 and +1,629, all 8 of 8), and mean speed falls equally in both (0.567 to
  0.465 for chemistry-only and to 0.470 for no-emission in discovery, 8 of 8). Post hoc, not declared: paired by
  seed, chemistry-only minus no-emission (the controllers' decisions other than emission) is inconsistent on
  every primary and behavioural measure (final live −45, higher on 5 of 8, and +88, 4 of 8; births +168, 7 of 8,
  and −86, 5 of 8; mean speed −0.004, 3 of 8, and −0.007, 4 of 8).
- **H2, fixed-controller** (controllers present, never mutated): no primary measure moved in either split (every
  p ≥ 0.20; final live −10 and −13), nor did speed or emission. Heritable variation in controller parameters
  made no detectable difference within 64 seconds.
- **H2 reading:** not supported. On this preset, the only detectable ecological effect of the controllers'
  decisions is through emission. Plan §15.4's consequence applies: simplify or redesign the sensor/action
  contract.
- **H3, no-emission**: organism Morisita +0.035 (4 of 8, p=0.19) and +0.002 (4 of 8, p=0.93), no effect. Field
  Gini +0.0085 (6 of 8, p=0.047) and +0.0098 (7 of 8, p=0.031): the field is less even without emission, in both
  splits, so the declared rule is met on field Gini. The organisms do measurably reshape their environment by
  paying to emit. The change is small (about 1.5% of a Gini near 0.58), it does not change how organisms cluster,
  and it costs population (+493.5 and +326.9 live without emission, 8 of 8 in both). Niche construction in the
  evolutionary sense, the modified field feeding back on selection, was not tested.
- **H5, fixed-topology**: final live +138.9 (7 of 8, p=0.031) in discovery but −79.3 (2 of 8, p=0.031) held-out;
  births +255 (7 of 8, p=0.016) and −63 (2 of 8, p=0.38). The discovery effects reverse on held-out seeds.
  Lineages alive is lower in both (−1.6, higher on 2 of 8, p=0.17; −1.3, 3 of 8 with one tie, p=0.36), which
  meets the letter of the rule but is indistinguishable from no effect in either split; it is not counted as
  support. Topologies alive, declared as H5's primary, is 1 under fixed-topology by construction against 5 in
  the control, so it measures the manipulation, not an outcome: a flaw in the declaration. Not supported. Plan
  §15.4's consequence: no expensive unrestricted topology work without a demonstrated benefit.
- **Inheritance control, fresh-offspring** (each newborn gets the genome a founder would have at its identity,
  with the default topology and no mutation, instead of its parent's; it keeps its parent's lineage): final live −2,407 and −2,524, births −5,959
  and −5,493 (0 of 8 higher in both, p=0.0078); lineages alive +113.8 and +110.9 (8 of 8 in both, p=0.0078);
  Morisita +0.50 (7 of 8) and +0.51 (7 of 8); field Gini +0.090 (7 of 8) and +0.093 (8 of 8). Without
  inheritance the world supports about half the organisms, and far more founder lineages persist, because no
  lineage carries a heritable advantage. The strongest result in E1: selection is acting on inherited
  variation, the premise of the whole design (plan §15).
- **Complexity-cost control, no-complexity-cost**: no primary measure moved consistently (every p ≥ 0.18;
  lineages alive −0.8 and −3.0).

## H4: boundary perturbation (`h4/`)

Rules declared before any declared seed (verbatim from the driver's docstring):

1. Run reference-small-world with SEED to tick T = 8192, keeping a run directory with a checkpoint at T. Inspect
   it at T.
2. Colonies at T: organisms per field cell smoothed by a periodic Gaussian (sigma 2 cells); cells at or above
   twice the mean density; periodic 4-connected components ranked by mass. The focal colony is the largest.
3. Disks, from the state at T alone: boundary, radius 6 centred on the focal colony's boundary cell whose disk
   holds the most organisms (ties: lowest y, then x); interior, centred on the focal colony's density peak, its
   radius the smallest multiple of 0.25 whose disk holds at least as many organisms as the boundary disk
   (count-matched); distant, the same, centred on the second colony's density peak (the noise floor). Matching is
   on organisms removed; how many were focal members is reported.
4. Branch at T four ways (control: no command; boundary, interior and distant: `RemoveOrganisms` with disposition
   `remove` at tick T), resume each to T + 256 and T + 1024 and inspect it there. The removal's count is checked
   against the geometry.
5. Measures at each horizon, over sets fixed at T ("members" are the focal colony's organisms at T; "remaining"
   are the members a treatment did not remove), each treatment compared with the control on the same set: P1
   survival, the fraction of remaining members alive; P2 cohesion, of the surviving remaining members the share in
   their most common colony at the horizon; P3 colony field, field energy over the focal colony's cells at T.
   Effect e = the treatment's value minus the control's.
6. Primary: P1 at T + 1024 and P3 at T + 256. A seed counts for a primary measure when |e_boundary| >
   |e_interior| and |e_boundary| > |e_distant|. H4 is supported only if one primary measure counts on at least 3
   of 4 discovery seeds and at least 3 of 4 held-out seeds. Otherwise the structures are presented as visual
   patterns, not functional membranes (plan §15.4). Everything else is secondary and cannot establish support.
7. Seeds: discovery 21–24; held-out 3021–3024. Every seed and branch is reported.

Results (`h4/result.txt`, from `h4/measures.ndjson`): all 8 seeds and 32 branches completed; every removal's count
matched its geometry (83–108 organisms per disk).

- P3 at T + 256: the boundary effect was the largest in magnitude on 3 of 4 discovery seeds (21, 22, 24) and 3 of
  4 held-out seeds (3021, 3022, 3023). Every boundary and interior effect was positive (field over the colony rose
  by 43–241 energy units against the control); the distant effect was within ±2.1. The declared rule is met.
- P1 at T + 1024: boundary-specific on 1 of 4 discovery and 1 of 4 held-out seeds. Interior removal raised the
  remaining members' survival more than boundary removal on 6 of 8 seeds.
- Secondary: P3 at T + 1024 had mostly faded (within ±53, mixed signs); P2 cohesion effects were within ±0.03.

Post hoc, not declared and unable to establish support (`h4/posthoc-area.txt`): the P3 effect at T + 256 split
into the part inside the emptied disk's share of the colony and the rest of the colony.

- 85–98% of every boundary and interior effect lay inside the emptied disk: the hole refilling with field no one
  was taking up.
- The count-matched boundary disk (radius 6) covered more focal-colony cells than the interior disk (radius
  3.5–5.25) on 7 of 8 seeds (69–110 against 37–88). Where it covered fewer (seed 23), the interior effect was the
  larger; on seed 3024 the interior effect was larger although its disk covered fewer colony cells. Boundary
  specificity on P3 therefore follows largely, not entirely, from how much colony each disk emptied.
- Field gained in the rest of the colony was larger after the boundary removal on 4 of 4 discovery and 2 of 4
  held-out seeds (2.15–30.65 against 2.03–15.52 energy units).

Reading: the declared rule is met, but the evidence does not show that colony boundaries regulate resource flow.
Until an area-matched test, declared in advance, shows a boundary-specific effect, the structures should be
presented as visual patterns (plan §15.4's consequence).

## Statistics and limits

- Effects are paired by seed against the control. Four seeds per split (E2, H4) can give a two-sided sign-flip p
  no smaller than 0.125, so those splits cannot reach conventional significance on their own; the declared rules
  are consistency rules (discovery and held-out agreeing), not significance tests. E1's eight seeds per split
  allow p down to 0.0078; the exact p values quoted are E1's report's, whose SHA-256 its `experiment-end` record
  carries. There is no
  multiple-comparison correction; primary measures were declared, everything else is exploratory.
- One preset, 64 simulated seconds. H1's criterion is clustering at one sample, not colony identity persisting
  through time; colony tracking across samples is not yet a measure.
- The chemistry-only ablation also stops emission, so chemistry-only against control mixes the loss of the
  controllers' decisions with the loss of emission; no-emission alone isolates the second. The comparison that
  isolates the first (chemistry-only against no-emission) was not declared, so it is reported as post hoc.
- H5's declared primary, topologies alive, is fixed by the fixed-topology ablation itself; a usable H5 test needs
  an outcome the manipulation does not set. E1's measures are ecological aggregates at one sample; they say
  nothing about behaviour that leaves those aggregates unchanged.
- The H4 driver and its post hoc analysis are Python with NumPy, kept outside the repository (AGENTS.md routes
  analysis through FrankenPandas); their SHA-256s are below. A native perturbation experiment (removal conditions
  in `flife experiment`, or `flife analyze`) would bring it in-tree.

## Files

- `reference-small-world.toml`: the base preset, byte-identical to `presets/reference-small-world.toml`.
- `e1/spec.toml`, `e2/spec.toml`: the specifications verbatim (their `base` resolves to the preset above).
- `e1/records.ndjson`, `e2/records.ndjson`: every run's `flife experiment` record, the start record first.
- `h4/measures.ndjson`: each seed's declared measures (the commands run, geometry, the state at T, every branch at
  both horizons), one line per seed, the driver's `measures.json` files compacted. `h4/result.txt`: the declared
  analysis; `analyze_h4.py` run over these measures (one `seed-N/measures.json` per line) reproduces it byte for
  byte. `h4/posthoc-area.txt`: the post hoc split, which also reads the inspected states at T and T + 256 (about
  4.7 MB each, outside git). `h4/DECLARED.txt`: the freeze record, with the declared scripts' and binary's
  SHA-256s and the relaunch note.

Outside git (artifact storage), by SHA-256:

| Artifact | SHA-256 |
|---|---|
| H4 driver `h4.py` (declared) | `e89bacaef9f20ae7fc3ca865321d35daf86f471f0d3533dd2a6882abc9863614` |
| H4 colony rules `colony.py` (declared) | `9c59b55eec8b4573b72642def6f133f3821550131089035a7271a8a34593ad2f` |
| H4 analysis `analyze_h4.py` | `d8c7cf5263dd5e7d6ed1b35c17b8a4f3008633cc3ea8d48356973692c22f0f3b` |
| H4 post hoc `posthoc_area.py` | `a2b93a05b107530893bd1362dbc9258e560fc1a737cc653e2a432b4ae87cb33a` |
| E1/E2 analysis `analyze.py` | `50dcae2bf6a28575148a1a4eec52e28185d389fbf046b57a4cb320f4327fc032` |
| H4 binary | `2ee7faefe4ffbff656c0231f1a6d978d50e9c121966157624c30aa9132ae307a` |
| E1/E2 binary | `cad7fe0f5229b9a9eb2bab7329dd2dc519f6b18f3750bc1e3aa34f510fd7d1b6` |
| E1 report (`flife experiment`) | `c0ba9030737ead5dfffbb0a200b91cef86a44743d3329bddcaa547c0dd142f5a` |
| Render, control seed 1001 | `efa86a2945dab3985efdc3dc63a2360870278a74de4180a7f413d49e6b647619` |

H4's run directories (about 2 GB per seed) and the experiment catalogs are held with these artifacts.
