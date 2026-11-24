# V2-10: Preregistered `128²` Quickie Verification Campaign (`⤴-1` Neural Agency `H2` & `⤴-2` Tissue Morphogenesis + `H4`)

> **Status:** **Observed (`62f16ac`, 2026-11-23/24 UTC)** — preregistered `--validate-only`, `--smoke` (`64²`), and `--full` (`128²`, 60 experiment runs + 3 parent checkpoints + 12 `flife branch --remove` perturbation assays + 6 rendered PNGs) executed end-to-end against the release `flife` binary (`62f16ac`, Apple M5 Ultra, `--controllers auto --cpu-workers 8`). All 60 runs—including the 1 real `INV-02` numerical fault on Arm 1 discovery seed `101`—are retained and reported honestly under the Four Claims (`AGENTS.md`). Targets below are explicitly separated from observed measurements.

## Purpose

Before calibrating Stage-2 showcase presets (`V2-17`: `presets/morphogenesis-tissue-v2.toml` and `presets/supercolony-v2.toml`), this preregistered harness empirically tests FrankenLife v2's two highest-stakes hypotheses on `128²` multi-seed runs using exact implemented `field-ecology-v2` controls and modular `--disable`/`--enable` toggles:

1. **Arm `⤴-1` — Neural Agency (`H2`, `inv08_pure_field = true`):**
   - **Base:** `arm1-neural-agency-128.toml` (`128 × 128`, `h = 1.0`, `Δt = 1/128 s`, 4,096 founders, cap `16,384` under `pause-before-commit`, F32 weights, `emission.maximum_fractional_rate_per_second = 0.0` so motility is isolated from emission; active features: `oases`, `senescence`, `so2-motor`, `v2-visual-mappings`; ABI `104/40/40/33`).
   - **Controls (`arm1/spec.toml`):**
     - `fixed-controller` (`field-ecology-v2-fixed-controller`, traits mutate, neural weights frozen),
     - `chemistry-only` (`field-ecology-v2-chemistry-only`, zero-output actions, identical neural cost),
     - `no-oases` (`v2.oases.enabled = false`, uniform source power).
   - **Seeds & Horizon:** Discovery `[101, 102, 103, 104]`, held-out `[2101, 2102]`; `4,096` ticks (`32.0 s`), metrics every `512` ticks.
   - **Preregistered Target Pass Gate:** Treatment beats `fixed-controller` by `≥ 15–20%` steady-state biomass or `≥ 25%` harvest efficiency (`G / C_motion`) **and** evolves `|corr(m_e, dE/dt)| ≥ 0.25` (or `|corr(m_e, E)| ≥ 0.25`) on `≥ 3/4` discovery seeds and holds in the same direction on held-out seeds.

2. **Arm `⤴-2` — Tissue Morphogenesis & `H4` Boundary Sealing (`inv08_pure_field = false`):**
   - **Base:** `arm2-tissue-morphogenesis-128.toml` (`32 × 32` torus on `128 × 128` grid so `h = D = 0.25`, `Δt = 1/32 s`, 1,750 founders, cap `32,768` under `refuse-excess-births-v1`, F16 weights, `[contact]` with `k = 0.75, k_a = 0.35`, active features: `pcp-polarity`, `hippo-crowding` with `c_max = 4.5`, `tight-junctions` with `φ_min = 0.04`, `v2-visual-mappings`; ABI `104/36/36/33`).
   - **Controls (`arm2/spec.toml`):**
     - `v1-compat` (all v2 features disabled — exact isotropic `[contact]` baseline),
     - `no-hippo-crowding` (`v2.crowding.enabled = false` — tests the 201% jamming prediction),
     - `frozen-polarity` (`v2.polarity.torque_mobility = 0.0, v2.polarity.oriented_lateral_mitosis = false`),
     - `no-tight-junctions` (`v2.barrier.enabled = false`),
     - `uniform-low-diffusion` (`v2.barrier.enabled = false, world.diffusion = 0.001194868`, `0.04×` uniform diffusion).
   - **Seeds & Horizon:** Discovery `[201, 202, 203, 204]`, held-out `[2201, 2202]`; `3,200` ticks (`100.0 s`), metrics every `400` ticks.
   - **Perturbation Assays (`flife branch --remove`):** At tick `2,048` (`128` in `--smoke`), `run_quickies.sh` branches `treatment`, `--v1-compat`, and `--disable tight-junctions` four ways (`control`, `boundary`, `interior`, `distant`) using area-matched removal disks, resumes to `+256` ticks (`+64` in `--smoke`), and measures wound-rim vs far-field proliferation and boundary-vs-interior field leakage via `flife inspect --field`.
   - **Preregistered Target Pass Gate:** `mean_overlaps_within_1D < 1.25`, `⟨|ψ_6|⟩ < 0.55`, chain fraction `≥ 2.5×` baseline, wound-rim birth rate `≥ 3×` far-field, `H4` boundary-vs-interior leak ratio `> 3×`, plus raw & sphere frame inspection.

## Files

- `arm1-neural-agency-128.toml`, `arm1-neural-agency-smoke64.toml` — Arm `⤴-1` base configs (`128²` full & `64²` smoke).
- `arm1/spec.toml`, `arm1/spec-smoke.toml` — Arm `⤴-1` `flife experiment` specifications.
- `arm2-tissue-morphogenesis-128.toml`, `arm2-tissue-morphogenesis-smoke64.toml` — Arm `⤴-2` base configs (`128²` full & `64²` smoke).
- `arm2/spec.toml`, `arm2/spec-smoke.toml` — Arm `⤴-2` `flife experiment` specifications.
- `run_quickies.sh` — Reproducible CLI runner (`--validate-only`, `--smoke`, `--full`, `--render`).

## Commands

```bash
# 1. Schema & CLI toggle validation (all 6 V2-10/V2-17 TOMLs + 14 toggle matrices)
./experiments/v2-quickies/run_quickies.sh --validate-only --flife target/release/flife

# 2. Fast 64x64 CI smoke mode (writes artifacts outside git)
./experiments/v2-quickies/run_quickies.sh --smoke --flife target/release/flife --render

# 3. Full preregistered 128x128 multi-seed campaign (optional --render for raw/sphere PNGs)
./experiments/v2-quickies/run_quickies.sh --full --flife target/release/flife --render
```

All run directories, SQLite experiment catalogs, `flife inspect` dumps, and rendered PNGs live outside git (e.g. `/Volumes/SANDISK_SSD/build/v2-quickies-62f16ac/{validate,smoke,full}`).

---

## Observed Results (`62f16ac`, 2026-11-23/24 UTC — Four Claims)

### 1. Rules (`INV-01`–`INV-15`)

- **`--validate-only`:** **PASS** (6/6 TOMLs + 14/14 CLI toggle matrices validated cleanly).
- **`--smoke` (`64²`, 256 ticks, 56 s wall):** **PASS** (`8/8` Arm 1 + `12/12` Arm 2 + `12/12` `flife branch --remove` assays reached target horizon with `0` invariant faults and `0` extinctions).
- **`--full` (`128²`, 60 experiment runs + 3 assay parents + 12 resumed branches = 75 runs):**
  - **74 / 75** runs reached their target horizon with **zero invariant faults** and **zero extinctions**.
  - **1 / 75 failed with a real `INV-02` cone fault (kept, not suppressed):** Arm 1 `treatment`, discovery seed `101`, halted cleanly under `stop-and-preserve` at tick `2,657`:
    `numerical-invalid: invalid chemical moment cone: cell (103, 87): ||m|| exceeds e = 0.1690865457057953 by 1.6962981073120886e-6 (INV-02)` (`1.0032e-5 · e`, crossing the `1.0e-5` `reference_f32` cone tolerance by `0.32%`; `INV-03 normalized_residual = 2.09e-9`).
    - **Root cause (proved independently by `CopperBay`, Agent Mail `#37800`):** Coherent F32 scalar-vs-32-vector rounding phase-locking on the neutrally-stable cone boundary (`||m||_2 / e = 1`) under zero-emission, zero-recycling pulsing single-oasis patches (primarily `transport.rs::stencil_rows`, amplified by `uptake.rs::apply_grants_on` and `sources.rs`). Plan §11.5 explicitly provides for a declared, logged, bounded, non-energy-creating contraction of `m` (with zero tolerance widening); that surgical fix is queued after the clean-tree 4K60 render.
  - **`INV-03` energy conservation across all 75 runs:**
    - Arm 1 max `|normalized_residual|` = **`8.353e-8`** (`discovery / chemistry-only / seed 102`, tick `4,096`).
    - Arm 2 max `|normalized_residual|` = **`6.164e-9`** (`discovery / uniform-low-diffusion / seed 202`, tick `3,200`).
    - Arm 2 assays max `|normalized_residual|` = **`2.808e-9`** (`treatment / resume-control`, tick `2,304`).
  - **`INV-12` explicit capacity selection disclosure:**
    - Arm 1: **`0` refused births** across all 24 runs (`pause-before-commit` never approached the `16,384` cap).
    - Arm 2: `treatment`, `frozen-polarity`, `no-tight-junctions`, and `uniform-low-diffusion` recorded **`0` refused births across all 24 / 24 runs**. By contrast, BOTH `v1-compat` and `no-hippo-crowding` hit the `32,768` cap on **6 / 6 seeds** and incurred massive externally imposed selection under `refuse-excess-births-v1`:
      - `v1-compat`: **`1,892,038` refused births/seed** (discovery mean; `7,568,150` total), **`2,009,014` refused births/seed** (held-out mean; `4,018,027` total); plus `1,123,310` refused births in the tick-2,048 assay parent and `~200k` per `+256`-tick branch.
      - `no-hippo-crowding`: **`1,895,423` refused births/seed** (discovery mean; `7,581,690` total), **`1,998,701` refused births/seed** (held-out mean; `3,997,402` total).

### 2. Implementation

- **Synthetic geometry oracle (`run_quickies.sh`):** **PASS** on analytical lattices (`1-cell ring`: `P(z=2) = 1.0`, `⟨|ψ_6|⟩ = 0.2357`; `periodic hex crystal`: `P(z=6) = 1.0`, `|ψ_6| = 1.0`).
- **Perturbation lesion planner bug caught & fixed during `--smoke`:** `plan_h4_lesion_disks` in `run_quickies.sh` initially picked `boundary == interior` on small clusters; fixed with multi-source inward BFS depth so `interior` is strictly deepest interior and `boundary` is a distinct outer rim (verified `boundary ≠ interior` across all 3 full `128²` assays; `bash -n` and `shellcheck -x` pass with 0 warnings).
- **Hardware / execution record (performance measurement, not an ecological claim):** Apple M5 Ultra, `--controllers auto --cpu-workers 8`, Metal controller backend (`metal (Apple M5 Ultra)`):
  - Arm 1 (`F32` weights): `892.4 s` cumulative wall across 24 runs; `490,129` rare-topology evaluations routed to CPU reference fallback (`0` skipped controllers, `INV-13` exact).
  - Arm 2 (`F16` weights): `4,037.5 s` cumulative wall across 36 runs; **`0` deferred controllers**.
- **Explicit remaining scope disclosures (honest gaps while `crates/` is frozen for 4K60):**
  1. Geometry, wound-rim, and `H4` leak diagnostics ran in Python (`run_quickies.sh` + `experiments/v2-stage2/diagnostics.py`) against real `flife inspect --field` NDJSON dumps rather than a new Rust `v2_diagnostics` library module in `crates/`.
  2. Per-organism instantaneous motor correlations (`corr(m_e, dE/dt)`, `corr(m_e, E)`, `⟨|v̂ · ∇q_⊥|⟩`) are not emitted in `flife experiment` `run-*.ndjson` and therefore remain **unmeasured** in this campaign.

### 3. Visual (Gate G5 — Raw + Sphere Inspection)

Inspected full `128²` tick-`2,048` frames (`/Volumes/SANDISK_SSD/build/v2-quickies-62f16ac/full/renders/{treatment,v1-compat,no-tight-junctions}/{raw,sphere}/frame-000000.png`) alongside `64²` smoke tick-`192` frames (cross-audited with `BrownMill`, Agent Mail `#37796`):

- **`treatment/sphere` & `treatment/raw` (`128²`, `live = 16,749`):** Crisp non-jammed monolayer domains cleanly colored by OKLab planar-cell-polarity angle (teal, coral, gold, lavender, magenta domains with coherent local orientation), legible white apical orientation ticks in `sphere`, and open intercellular clearings. On lower-density `64²` smoke (`live = 528`), `treatment` formed 1-cell-wide curved epithelial arcs and bilayers.
- **`v1-compat/raw` & `v1-compat/sphere` (`128²`, `live = 32,768`):** Dramatically distinct — uniform grey unpolarized jammed sheet pressed against the capacity cap with dense multi-layer dark overlap knots.
- **`no-tight-junctions/sphere` (`128²`, `live = 16,449`):** Visually comparable polarized monolayer domains with outward apical ticks, confirming `pcp-polarity` + `hippo-crowding` establish the polarized non-jammed architecture while `tight-junctions` modulates local field permeability and wound-rim response.
- **Visual-honesty caveats carried into `V2-17`:**
  1. Because uniform high source power floods the `32 × 32` torus (`field_gini = 0.109`), the bright linear-grey energy background partially washes out white apical ticks in `sphere`.
  2. Rendering `v1-compat` under `polarity-v1` maps all organisms to monochrome slate-grey (`POLARITY_UNPOLARIZED`). Stage-2 proof sheets must always pair a shared fair mapping (`--color lineage-energy` on BOTH treatment and `v1-compat`) alongside dark-backdrop `--look sphere --color polarity --no-field` and Gate G5 `--look raw --field-mapping energy`.

### 4. Ecological — Arm `⤴-1` (Neural Agency `H2`, `128²`, 4,096 ticks = 32 s)

**Preregistered Pass Gate Verdict: NOT MET** (honest null on `treatment` vs `fixed-controller`; `chemistry-only` outperformed `treatment`; strong positive effect for `V2-02` `oases` vs `no-oases`).

#### Arm 1 Means (Paired Reached Seeds)

Discovery paired means cover `n = 3` seeds `[102, 103, 104]` because discovery `treatment` seed `101` halted at tick `2,657` (`INV-02`, `live = 4,573`, `biomass = 16,282.0`, `births = 6,912`, `deaths = 6,435`); held-out covers `n = 2` seeds `[2101, 2102]`. Biomass is `holdings.organisms` at tick `4,096`; `G / C_motion` is `transfers.uptake / flows.motion-cost` (both final 512-tick window and cumulative over all 8 windows).

| Split | Condition | Reached | Mean `final_live` | Mean Biomass (`holdings.organisms`) | Mean `births` | Final-Window `G / C_motion` | Cumulative `G / C_motion` | Mean `speed` | Mean `morisita` | Mean `field_gini` |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Discovery (`[102..104]`) | `treatment` | 3/4 *(101 failed t=2657)* | `6,536.7` | `20,959.8` | `16,503.0` | `598.1` | `638.8` | `0.552` | `3.693` | `0.764` |
| Discovery (`[102..104]`) | `fixed-controller` | 4/4 | `6,503.0` | `20,973.3` | `16,381.0` | `584.4` | `632.5` | `0.567` | `3.659` | `0.755` |
| Discovery (`[102..104]`) | `chemistry-only` | 4/4 | `6,918.7` | `21,469.4` | `17,290.3` | `593.7` | `649.0` | `0.488` | `3.626` | `0.760` |
| Discovery (`[102..104]`) | `no-oases` | 4/4 | `4,201.7` | `8,026.7` | `12,977.0` | `766.2` | `648.7` | `0.578` | `2.314` | `0.591` |
| Held-out (`[2101, 2102]`) | `treatment` | 2/2 | `6,393.5` | `19,233.0` | `16,400.5` | `558.3` | `668.0` | `0.466` | `4.671` | `0.818` |
| Held-out (`[2101, 2102]`) | `fixed-controller` | 2/2 | `6,356.5` | `19,555.1` | `15,852.5` | `544.1` | `638.8` | `0.506` | `4.971` | `0.816` |
| Held-out (`[2101, 2102]`) | `chemistry-only` | 2/2 | `6,933.5` | `21,601.5` | `16,708.0` | `570.2` | `663.1` | `0.513` | `4.841` | `0.817` |
| Held-out (`[2101, 2102]`) | `no-oases` | 2/2 | `4,372.0` | `8,125.5` | `13,316.0` | `771.9` | `702.5` | `0.555` | `2.527` | `0.607` |

*(Unpaired discovery seed `101` reached on all 3 controls: `fixed-controller` live `6,696` / bio `19,200.6`; `chemistry-only` live `6,339` / bio `15,759.0`; `no-oases` live `4,122` / bio `8,018.9`.)*

#### Arm 1 Paired Contrasts (`treatment` vs Controls)

- **`treatment` vs `fixed-controller`:**
  - `final_live`: `+0.52%` discovery (`1/3` treat > fixed), `+0.58%` held-out (`2/2` treat > fixed).
  - Biomass: `-0.06%` discovery (`2/3`), `-1.65%` held-out (`0/2`).
  - Final-window `G / C_motion`: `+2.34%` discovery (`1/3`), `+2.61%` held-out (`1/2`); cumulative `+1.01%` discovery, `+4.57%` held-out.
  - **Gate status:** Far below the preregistered `+15–20%` biomass or `+25%` `G / C_motion` threshold.
- **`treatment` vs `chemistry-only`:**
  - `chemistry-only` beat `treatment` on both live count (`+5.84%` discovery `2/3`, `+8.45%` held-out `2/2`) and biomass (`+2.43%` discovery `2/3`, `+12.31%` held-out `2/2`), replicating the earlier `FL-048` `H2` pattern.
  - **Mechanism exposed by the energy ledger:** `[chemistry].motion_response_norm_limit = 256.0` already gives passive `chemistry-only` organisms strong gradient-riding (`mean_speed = 0.49–0.51`) with zero neural motor noise (`toxicity` in the final window on seed `2101`: `27.8` for `chemistry-only` vs `82.2` for `treatment`). Meanwhile `4,096` ticks at `Δt = 1/128 s` spans only `32.0 s` (`1.33` oasis bloom cycles, `max_generation = 13–15`, `weight_probability = 0.002`) — too few generations for neural chemotaxis to out-evolve passive gradient-riding.
- **`treatment` vs `no-oases` (`V2-02` validation):**
  - Ephemeral pulsing oases increased `final_live` by **`+55.6%`** (discovery, `3/3`) and **`+46.2%`** (held-out, `2/2`), increased biomass by **`+161.1%`** (discovery, `3/3`) and **`+136.7%`** (held-out, `2/2`), and sharply increased spatial patchiness (`organism_morisita`: `3.69` vs `2.31` discovery, `4.67` vs `2.53` held-out; `field_gini`: `0.764` vs `0.591` discovery, `0.818` vs `0.607` held-out).

### 5. Ecological — Arm `⤴-2` (Tissue Morphogenesis + `H4`, `128²`, 3,200 ticks = 100 s)

**Preregistered Pass Gate Verdict: PARTIAL** (anti-jamming and chain-fraction gates **MET** across 6/6 seeds; real `1.98×` selective wound-rim proliferation signal observed on `treatment` vs `0.99×` `v1-compat` and `0.89×` `no-tight-junctions`; hollow-cyst stacking, `⟨|ψ_6|⟩`, and `H4` sealing gates **NOT YET MET** because uniform over-feeding produced a confluent torus-filling monolayer rather than discrete hollow cysts).

#### Arm 2 Multi-Seed Summary (`36 / 36` Reached, `0` Faults, `0` Extinctions)

| Split | Condition | Mean `final_live` | Mean Biomass | Mean `births` | Mean `refused_births` (`INV-12`) | Mean `max_gen` | Mean `speed` | Mean `field_gini` |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| Discovery (`n=4`) | `treatment` | `16,670.0` | `41,136.2` | `21,266.5` | **`0.0`** | `22.8` | `0.150` | `0.109` |
| Discovery (`n=4`) | `v1-compat` | `32,768.0` *(cap)* | `84,636.9` | `62,319.0` | **`1,892,037.5`** | `15.5` | `0.174` | `0.189` |
| Discovery (`n=4`) | `no-hippo-crowding` | `32,768.0` *(cap)* | `84,748.1` | `62,247.8` | **`1,895,422.5`** | `14.8` | `0.212` | `0.187` |
| Discovery (`n=4`) | `frozen-polarity` | `16,734.0` | `41,178.3` | `21,293.2` | `0.0` | `24.8` | `0.152` | `0.109` |
| Discovery (`n=4`) | `no-tight-junctions` | `16,296.8` | `40,163.6` | `20,723.2` | `0.0` | `25.2` | `0.121` | `0.107` |
| Discovery (`n=4`) | `uniform-low-diffusion` | `17,412.8` | `43,108.1` | `22,498.0` | `0.0` | `20.2` | `0.203` | `0.112` |
| Held-out (`n=2`) | `treatment` | `19,282.0` | `45,894.9` | `22,749.0` | **`0.0`** | `23.5` | `0.163` | `0.117` |
| Held-out (`n=2`) | `v1-compat` | `32,768.0` *(cap)* | `84,116.2` | `48,312.5` | **`2,009,013.5`** | `13.5` | `0.196` | `0.166` |
| Held-out (`n=2`) | `no-hippo-crowding` | `32,768.0` *(cap)* | `84,280.8` | `49,237.5` | **`1,998,701.0`** | `12.5` | `0.244` | `0.170` |
| Held-out (`n=2`) | `frozen-polarity` | `19,082.0` | `45,404.7` | `22,848.5` | `0.0` | `23.0` | `0.165` | `0.118` |
| Held-out (`n=2`) | `no-tight-junctions` | `19,014.5` | `45,306.8` | `22,396.5` | `0.0` | `25.5` | `0.131` | `0.115` |
| Held-out (`n=2`) | `uniform-low-diffusion` | `19,723.5` | `47,396.8` | `23,820.5` | `0.0` | `20.0` | `0.224` | `0.121` |

#### Arm 2 Geometry & `flife branch --remove` Perturbation Assays (Seed `201`, Fork Tick `2,048` → `2,304`)

| Metric | Target Gate | `treatment` | `v1-compat` | `no-tight-junctions` | Verdict |
|---|---:|---:|---:|---:|---|
| Pre-lesion `live` (tick `2,048`) | Below `32,768` cap | `16,749` (`0` refused) | `32,768` (`1.12M` refused) | `16,449` (`0` refused) | **MET** (`V2-07` stops jamming on 6/6 seeds) |
| `median_d1_over_D` | `≈ 1.0` | **`0.930`** | `0.580` | `0.928` | **MET** (`+60%` inter-cell separation) |
| `mean_overlaps_within_1D` (stacking) | `< 1.25` | `3.372` | `6.504` | `3.425` | **NOT MET** (`-48%` vs baseline, still confluent) |
| Jammed tail `P(z ≥ 8)` | Low | **`4.72%`** | `70.72%` | `5.40%` | **Strong reduction** (`15×` lower) |
| Hexatic `⟨|ψ_6|⟩` (`P(z=6)`) | `< 0.55` | `0.647` (`48.3%`) | `0.332` (`14.6%`) | `0.673` (`51.7%`) | **NOT MET** (hexagonal confluent sheet) |
| Chain fraction `P(z=1)+P(z=2)` | `≥ 2.5×` baseline | **`0.0127`** (`69.5×`) | `0.000183` | `0.0120` (`65.4×`) | **MET** (`69.5× >> 2.5×`) |
| `wound_rim_to_far_ratio` (rim / far birth rate) | `≥ 3.0×` | **`1.982×`** (`0.1468 / 0.0741`) | `0.989×` (`0.0630 / 0.0636`) | `0.888×` (`0.0767 / 0.0864`) | **PARTIAL** (real `~2×` rim surge; vanishes without `tight-junctions` or `hippo-crowding`) |
| `h4_boundary_to_interior_field_delta_ratio` | `> 3.0×` | `1.004×` (`5479 / 5458`) | `1.004×` (`22755 / 22676`) | `1.043×` (`5303 / 5084`) | **NOT MET** (confluent lawn had no hollow lumen; `boundary`/`interior` only `0.79` units apart) |

---

## Concrete Parameter Calibrations for `V2-17` (Stage-2 Showcase Presets)

These observed breakpoints directly specify what must be calibrated before claiming Stage-2 `H2`, hollow-cyst morphogenesis, or `H4` boundary sealing:

1. **Field F32 Cone Boundary Stability (prerequisite for zero-emission oasis runs):**
   - Land `CopperBay`'s Plan §11.5 bounded, logged, non-energy-creating contraction of `m` (plus `sources.rs` `delta_e`-scaled moment deposition) before re-running long zero-emission pulsing-oasis sweeps so neutral-boundary F32 rounding drift never trips `INV-02` at `~2,600+` ticks.
2. **Neural Foraging Agency (`H2` / Bundle B `supercolony-v2.toml`):**
   - **Attenuate passive chemical gradient-riding:** Reduce `[chemistry].motion_response_norm_limit` from `256.0` to `<= 8.0` (or `0.0` in pure neural-agency tests) so `chemistry-only` cannot coast up source gradients for free without neural motor control.
   - **Extend evolutionary time & oasis cycles:** Switch `Δt` from `1/128 s` to `1/32 s` (4× more simulated seconds per tick → `128 s` = `5.3` oasis bloom cycles in `4,096` ticks, or run `8,192–16,384` ticks) and raise `mutation.weight_probability` (`0.002 → 0.005`) and `weight_standard_deviation` (`0.02 → 0.04`) so selection sees `30–60+` generations across multiple boom-bust oasis cycles.
   - **Add per-organism motor-correlation logging** (`corr(m_e, dE/dt)`, `corr(m_e, E)`, `⟨|v̂ · ∇q_⊥|⟩`) so the full preregistered `H2` correlation gate is directly measured.
3. **Hollow-Cyst Morphogenesis & `H4` Sealing (Bundle A `morphogenesis-tissue-v2.toml`):**
   - **Eliminate torus-wide over-feeding (`field_gini = 0.109`):** Lower `sources.power_per_source` (`19,000 → ~3,500–5,500`) and localize sources (`sigma_world_units` `6.0 → 2.5–3.5`, optionally with `oases` or fewer sources) so `field_gini >= 0.45` and colonies grow as discrete, bounded structures with real free boundaries and enclosed lumens rather than a torus-filling confluent lawn (`~16.7k` cells).
   - **Tighten Hippo crowding threshold for 1-cell arches:** Lower `v2.crowding.arrest_threshold` from `4.5` to `2.5–3.0` (`c_max`) so lateral sheets arrest before closing into dense `z = 6` hexagonal pavements (`P(z=6) = 48.3%`), targeting `mean_overlaps_within_1D < 1.25` and `⟨|ψ_6|⟩ < 0.55` (matching the 1-cell curved arcs observed at lower density in `--smoke`).
   - **Verify `H4` on true enclosed lumens:** Once colonies are discrete cysts/rings (`focal_cells` spanning an enclosed low-density cavity rather than an 8-cell patch inside a confluent lawn), re-run the 4-branch `flife branch --remove` assay (`control`, `boundary`, `interior`, `distant`) and pair with `BrownMill`'s shared `lineage-energy` + dark-backdrop `polarity` + `raw` visual proof sheet.
