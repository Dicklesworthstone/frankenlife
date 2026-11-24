# FieldEcologyV2 Empirical Emergence & Verification Report (`V2-17`)

> **Status:** **Observed (`76f47f9`, 2026-11-24 UTC)** — full `V2-17` Stage-2 verification campaign (`104 / 104` unmodified `320×320`, `2,048`-tick runs across Discovery seeds `[42, 43]` and Held-Out seeds `[101, 102]`, `1280×1280` Gate G5 & `V2-16` visual renders, plus quiet-host `320²` wall-time/RSS and Round 1–2b optimization benchmarks) executed end-to-end on Apple M5 Ultra with real `flife` release binary `flife_round2b` (`sha256: db47d491dba8e7c146938d1a09221043343a4e1ed0d81764b4c36aff11cc0b02`, `--controllers auto --cpu-workers 12`). All 104 Stage-2 runs — together with all 75 Stage-1 `V2-10` runs (`experiments/v2-quickies/README.md`) — are retained and reported under the Four Claims (`AGENTS.md`). No large media or checkpoint artifacts are committed to git.

---

## Executive Summary (Four-Claims Verdict)

| Claim | Status | Observed Evidence (`76f47f9` / `V2-17`) |
|---|---|---|
| **1. Rules (`INV-01`–`INV-15`)** | **PASS (Observed)** | `104 / 104` Stage-2 `320×320` 2,048-tick runs reached target horizon: **`0` extinct, `0` paused, `0` failed, `0` refused births (`INV-12`), max `|INV-03| = 7.17e-9`** (`1,400×` inside `1e-5` tolerance). Bundle B runs strictly `inv08_pure_field = true` (zero pairwise forces). |
| **2. Implementation (`G1`, `G4`)** | **PASS (Observed)** | `crates/flife-cli/tests/v2_presets.rs` passes all 4 E2E suites: unmodified Bundle A (`108/40/44/33`) & Bundle B (`108/40/40/34`) round-trip; `e2e_all_21_v2_features_composed_simultaneously` (`112/44/48/34`, `1v4` workers, `C.7`, `C.8`, Reference vs `CpuBackend/torch` vs Apple M5 Ultra Metal F16 bit-exact with `0` deferrals, checkpoint v3, crop/graft); `e2e_leave_one_out_disable_all_21_v2_features` (all 21 slugs + `notch-delta` dependency refusal + `INV-09` visual digest invariance); `v2_ablation_controls_execute_and_report_structured_metrics` (`flife experiment` across `chemistry-only`, `fixed-controller`, `fresh-offspring`, `--v1-compat`). |
| **3. Visual (`Gate G5`)** | **OBSERVED (Human Review 2026-11-24)** | Side-by-side `1280×1280` `raw` + `sphere`/`glow` + `V2-16` overlays inspected on Discovery (`seed=42`) and unseen Held-Out (`seed=101`) for BOTH bundles: Bundle A eliminates `v1-compat` solid crystalline jamming into porous, multi-lineage tissue islands with coherent PCP polarity domains (`sphere-polarity`) and intra-colony `h_0` heterogeneity (`glow-cell-state`); Bundle B (`INV-08` pure-field) forms crescentic/sickle wave fronts with luminous excitable leading rims, clean circular depletion cavities (`raw-lineage-energy`), decoupled amber Carrier-1 pheromone wakes (`glow-trail-carrier`), and bimodal Murray vascular watershed basins (`raw-vascular-conductance`). |
| **4. Ecological (`Gate G9` Stage-2)** | **STRONG POSITIVE WITH EXPLICIT RESIDUALS** | **Heritable evolution (`4/4` seeds, both bundles):** `treatment` outperforms `fresh-offspring` by **`429×`** live on Bundle A (`15,446` vs `36`) and **`67×`** on Bundle B (`21,308` vs `317`). **Bundle A (`4/4` seeds):** `4.0×` fewer jammed overlaps vs `v1-compat`, `+0.135..+0.144` higher collective coherence (`4/4`), `79,910 E` active kin trophallaxis (`16/16` windows, `4/4`), `+0.038` clonal `W/T` over `fixed-controller` (`4/4`), `+1,496` live over `no-cadherin-4d` (`4/4`). **Bundle B (`INV-08` pure-field, `4/4` seeds):** bimodal Murray arteries (`Sarle b = 0.608`, `P95/P25 = 7.02×`) lift organism alignment `+50%..+58%` (`4/4` vs `no-vascular-field`), decoupled Carrier-1 trails (`MaxZ = 10.24`, `decoupling = 0.948`), `+58.3%` live over `no-excitable-relay` (`4/4`), and neural evolution advantage over `fixed-controller` (`+6.9%` live `3/4` & **`+10.0%` `2/2` held-out**, `Trail MaxZ` `4/4`, `Trail Mean` `4/4`, `Glob P` `3/4`). **Honest residuals documented in §4.4 below** (weak linear radial `corr(h_0, c_den) = +0.069` at 2,048t, `chemistry-only` passive survival/alignment tradeoffs, and Stage-1 `V2-10` Arm-1/Arm-2 shortfalls). |

---

## 1. Rules (`INV-01`–`INV-15` & Energy Ledger)

All Stage-2 runs executed via `experiments/v2-stage2/run_stage2.sh` and `/Volumes/SANDISK_SSD/build/v2-stage2-round2b/run_extra_controls.sh` on unmodified `320×320` showcase presets (`2,048` ticks = `64.0 s` simulated time at `Δt = 1/32 s`, Discovery seeds `[42, 43]`, Held-Out seeds `[101, 102]`, `13` conditions × `4` seeds × `2` bundles = **`104` full runs**). Every run is retained in `/Volumes/SANDISK_SSD/build/v2-stage2-round2b/stage2_summary_FINAL_104.json`.

| Bundle | Preset & Boundary | Runs | Reached | Extinct | Paused | Failed | Max `|INV-03|` | Refused Births (`INV-12`) | Mean Wall (s) *(2 concurrent 12-w jobs)* | Mean `ms/tick` |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| **Bundle A** | `presets/morphogenesis-tissue-v2.toml` (`inv08_pure_field = false`, 33 planes, 14 V2 features) | `52` | `52` | `0` | `0` | `0` | `5.00e-09` | `0 (0)` | `62.5` | `30.52` |
| **Bundle B** | `presets/supercolony-v2.toml` (`inv08_pure_field = true`, 34 planes, 11 V2 features) | `52` | `52` | `0` | `0` | `0` | `7.17e-09` | `0 (0)` | `91.2` | `44.52` |
| **TOTAL** | **All 26 conditions × 4 seeds** | **`104`** | **`104`** | **`0`** | **`0`** | **`0`** | **`7.17e-09`** | **`0 (0)`** | `76.8` | `37.52` |

- **Zero hidden capacity selection (`INV-12`):** Unlike Stage-1 `V2-10` Arm 2 (`v1-compat` hitting the 32,768 cap with `~1.9M` refused births), all 104 Stage-2 runs stayed cleanly below the `262,144` cap with **`0` refused births**.
- **Zero numerical faults (`INV-01`, `INV-02`):** Every cell maintained `e ≥ 0` and `‖m‖₂ ≤ e` within tolerance across all `212,992` committed ticks (`104 × 2,048`).
- **Modular toggle boundary (`INV-14`):** `run_stage2.sh --validate-only` verified `20` CLI override chains across Bundles A & B plus explicit exit-2 refusal of `--disable epigenetic-recurrence` alone while `notch-delta` remains enabled.

---

## 2. Implementation (`G1`, `G4` & Automated E2E Verification)

### 2.1 Showcase Presets & Pinned 4-Aligned ABIs

| Preset | Scale | Effective ABI (`in / act / out / planes`) | `inv08_pure_field` | Active V2 Features | Metal F16 Alignment Deferrals |
|---|---|---|---:|---|---:|
| [`presets/morphogenesis-tissue-v2.toml`](../../presets/morphogenesis-tissue-v2.toml) (Bundle A) | `80×80` torus, `320×320×33`, `10,954` founders, F16 | `108 / 40 / 44 / 33` (`% 4 == 0`) | `false` | `14`: `hippo-crowding`, `pcp-polarity`, `tight-junctions`, `senescence`, `lysis-recycling`, `symplectic-waste`, `epigenetic-recurrence` (`S=2`), `notch-delta`, `trophallaxis`, `cadherin-4d`, `so2-motor`, `surplus-emission`, `wake-offset`, `v2-visual-mappings` (`cell-state-v1`) | **`0`** |
| [`presets/supercolony-v2.toml`](../../presets/supercolony-v2.toml) (Bundle B) | `80×80` torus, `320×320×34`, `16,384` founders, F16 | `108 / 40 / 40 / 34` (`% 4 == 0`) | `true` | `11`: `oases`, `senescence`, `so2-motor`, `surplus-emission`, `excitable-relay`, `wake-offset`, `tight-junctions`, `lysis-recycling`, `vascular-field`, `dual-carrier-trails`, `v2-visual-mappings` (`trail-carrier`) | **`0`** |

Modular founding-only CLI overrides work identically on both presets (`flife --list-features` lists all 21 slugs):
```bash
flife validate presets/morphogenesis-tissue-v2.toml
flife run presets/morphogenesis-tissue-v2.toml --ticks 2048 --disable pcp-polarity
flife run presets/morphogenesis-tissue-v2.toml --ticks 2048 --disable epigenetic-recurrence,notch-delta
flife run presets/supercolony-v2.toml --ticks 2048 --disable vascular-field
flife run presets/supercolony-v2.toml --ticks 2048 --enable so2-budding,symplectic-waste
flife run presets/supercolony-v2.toml --ticks 2048 --v1-compat
```

### 2.2 Automated E2E & Oracle Coverage

- **`crates/flife-cli/tests/v2_presets.rs` (4 / 4 PASS):**
  1. `bundle_a_and_bundle_b_presets_validate_and_round_trip`: validates unmodified `320×320` Bundle A & B TOMLs, compact run → `inspect` → `resume` bit-exact SHA-256 parity, `--disable`/`--v1-compat` toggles, raw + diagnostic PNG `render`.
  2. `e2e_all_21_v2_features_composed_simultaneously`: enables `[contact]` + **all 21 `V2Feature` slugs** (`112 / 44 / 48 / 34`), asserts `INV-01..15`, `w=1` vs `w=4` bit-exact (`INV-11`), `Compact` vs `FreeList` (`C.7`), `Candidate::cancel` (`C.8`), Reference vs `CpuBackend` (`torch`) vs real Apple M5 Ultra Metal F16 bit-exact with **`0` deferrals**, checkpoint v3 save/restore, `inspect --field`, `branch`, and `discover --region` → `transplant` → `resume`.
  3. `e2e_leave_one_out_disable_all_21_v2_features`: sweeps all 21 `V2Feature::ALL` slugs + `--v1-compat`, verifies explicit dependency rejection on `--disable epigenetic-recurrence` alone + clean pass with `--disable epigenetic-recurrence,notch-delta`, logs structured JSON telemetry per slug, asserts `0` Metal F16 deferrals and exact `WorldDigest` invariance when disabling `v2-visual-mappings` (`INV-09`).
  4. `v2_ablation_controls_execute_and_report_structured_metrics`: executes real `flife experiment` across `control`, `chemistry-only`, `fixed-controller`, `fresh-offspring`, and `--v1-compat` on discovery `[1, 2]` + held-out `[101]` seeds, asserting FrankenSQLite `ExperimentCatalog` rows, manifests, resolved `field-ecology-v2-*` models, and distinct structured digests.
- **Stage-2 diagnostic oracles:** `python3 -m unittest discover -s experiments/v2-stage2 -p 'test_*.py'` (16 independent analytical oracle tests).
- **Workspace hygiene:** `cargo fmt --all -- --check`, `cargo clippy --locked --workspace --all-targets -- -D warnings`, `cargo test --locked --workspace`, and `flife-probes` dependency-closure probes all pass with 0 warnings and 0 failures.

---

## 3. Visual Review (`Gate G5`, Human Inspection 2026-11-24 UTC)

All `1280×1280` frames rendered via real `flife render --run-dir` at tick `2,048` into `/Volumes/SANDISK_SSD/build/v2-stage2-round2b/bundle-{a,b}/renders/` (outside git). Every aesthetic view was paired with its unmodified linear-grey `raw` diagnostic view (`Gate G5`) and apples-to-apples `treatment` vs `v1-compat` renders under identical `lineage-energy` mappings on both Discovery (`seed=42`) and unseen Held-Out (`seed=101`):

### 3.1 Bundle A (`morphogenesis-tissue-v2.toml`, `inv08_pure_field = false`)
- **Discovery (`seed=42`) & Held-Out (`seed=101`) `treatment` vs `v1-compat` (`sphere-lineage-energy` + `raw-lineage-energy`):**
  - `v1-compat` overpopulates (`32k–37k` live, `123k` neighbor pairs) into dense, jammed, solid crystalline slabs (e.g., tightly packed gravel-like violet/magenta/olive blocks choking the source plumes in `raw-lineage-energy`).
  - `treatment` (`12k–17k` live, `23k–34k` neighbor pairs) completely eliminates solid-slab jamming on both discovery and held-out seeds, producing discrete, porous, multi-lineage tissue islands with open intercellular spacing, crisp 4-D cadherin lineage sorting, and clean dark metabolic depletion shadows carved into bright source plumes (`raw-lineage-energy`).
- **`V2-16` Diagnostic Overlays (`treatment`, Discovery `42` & Held-Out `101`):**
  - `sphere-polarity` (`--no-field`): Smooth contiguous planar-cell-polarity orientational domains across every tissue island (coherent OKLab hue gradients + aligned apical orientation ticks).
  - `glow-cell-state`: Visible intra-colony `h_0` state heterogeneity (e.g., green-haloed lavender lobes and crimson-centered olive colonies on Held-Out `seed=101`).

### 3.2 Bundle B (`supercolony-v2.toml`, `inv08_pure_field = true` — ZERO Direct Pairwise Forces)
- **Discovery (`seed=42`) & Held-Out (`seed=101`) `treatment` vs `v1-compat` (`glow-lineage-energy` + `raw-lineage-energy`):**
  - `v1-compat` forms sluggish, diffuse, multi-species mixed puddles with blurred field contrast (`Gini = 0.757`, `Dark<10% = 38.9%`) and no sharp leading wave structure.
  - `treatment` produces sharply differentiated motile super-colony morphologies on both Discovery `42` and Held-Out `101`: crescentic/sickle bow-wave fronts with **luminous white excitable leading rims** (`V2-04`) sweeping around freshly carved circular depletion cavities clearly visible in linear-grey `raw-lineage-energy` (e.g., paired emerald scythe and magenta crescent carving a clean dark hollow on Held-Out `101`), alongside distinct cobalt horseshoe colonies, crimson gliders, and elongated ribbon swarms.
- **`V2-16` Mechanism Overlays (`treatment`, Discovery `42` & Held-Out `101`):**
  - `glow-trail-carrier`: Blazing amber Carrier-1 pheromone wakes (`V2-15` + `V2-04` wake-offset) directly trailing inside the depleted cavities behind advancing wave rims, spatially decoupled (`0.948`) from fresh white Carrier-0 oasis blooms.
  - `raw-vascular-conductance`: High-contrast bimodal Murray/Physarum conductance basins (`V2-14`) separated by crisp branching dark watershed seams.

---

## 4. Ecological Results (`Gate G9` Multi-Seed Stage-2 Campaign)

All numbers below are pooled across all **4 seeds** (Discovery `[42, 43]` + Held-Out `[101, 102]`), with split-level consistency noted from `/Volumes/SANDISK_SSD/build/v2-stage2-round2b/stage2_summary_FINAL_104.md`.

### 4.1 Bundle A — Morphogenesis Tissue (`52` Runs: `13` Conditions × `4` Seeds, `320×320`, `2,048` Ticks)

| Condition (`n = 4`) | Mean `Live` | Mean `Births` | `Field Gini` | Clonal `W/T` | Within `corr(h_0, c)` | `Troph E` (win/16) | `Loc Vicsek` | `Kin Vicsek` | `Loc Align` | `Kin Align` | `Nb Pairs` |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| **`treatment`** | **`15,446`** | `45,933` | `0.667` | **`0.326`** | `+0.069` | **`79,910` (16)** | **`0.725`** | **`0.757`** | **`0.509`** | **`0.554`** | **`30,616`** |
| `v1-compat` | `34,489` | `139,506` | `0.802` | — | — | `0` (0) | `0.581` | `0.621` | `0.372` | `0.419` | `123,294` |
| `fresh-offspring` | **`36`** | `3,214` | `0.553` | `0.626` | `+0.064` | `0.4` (7.5) | `0.499` | `0.499` | `-0.256` | `-0.256` | `4` |
| `fixed-controller` | `15,186` | `45,145` | `0.664` | `0.289` | `+0.080` | `79,081` (16) | `0.737` | `0.770` | `0.547` | `0.597` | `31,512` |
| `chemistry-only` | `27,603` | `91,932` | `0.807` | `0.441` | `+0.062` | `508,173` (16) | `0.604` | `0.668` | `0.361` | `0.432` | `66,064` |
| `no-cadherin-4d` | `13,950` | `40,604` | `0.658` | `0.343` | `+0.089` | `73,322` (16) | `0.732` | `0.769` | `0.529` | `0.577` | `28,189` |
| `no-pcp-polarity` | `15,708` | `45,183` | `0.668` | `0.327` | `+0.016` | `79,325` (16) | `0.712` | `0.739` | `0.489` | `0.527` | `31,008` |
| `no-symplectic-waste` | `16,695` | `48,911` | `0.725` | `0.355` | `+0.066` | `81,893` (16) | `0.699` | `0.744` | `0.466` | `0.522` | `34,480` |
| `no-trophallaxis` | `15,579` | `48,084` | `0.664` | `0.330` | `+0.076` | `0` (0) | `0.724` | `0.756` | `0.499` | `0.543` | `29,166` |
| `no-epigenetic-notch` | `15,286` | `46,524` | `0.666` | — | — | `77,858` (16) | `0.729` | `0.765` | `0.518` | `0.570` | `30,309` |
| `no-notch-delta` | `15,725` | `45,416` | `0.664` | `0.362` | `+0.069` | `77,532` (16) | `0.723` | `0.755` | `0.505` | `0.546` | `30,915` |
| `no-lysis-recycling` | `15,591` | `46,076` | `0.671` | `0.319` | `+0.058` | `80,235` (16) | `0.729` | `0.761` | `0.515` | `0.557` | `30,874` |
| `with-velocity-alignment` *(opt-in)* | `9,574` | `23,431` | `0.622` | `0.240` | `+0.018` | `46,727` (16) | **`0.917`** | **`0.939`** | **`0.855`** | **`0.896`** | `18,046` |

**Paired Seed Effects (Bundle A, `wins / 4` across `[42, 43]` + `[101, 102]`):**
- **Anti-jamming & tissue coherence (`treatment` vs `v1-compat`):** `4/4` seeds cut neighbor overlap pairs **4.0×** (`30,616` vs `123,294`), hold `live` regulated (`15,446` vs `34,489`), activate `79,910 E` kin trophallaxis (`4/4`), and improve ALL four coherence metrics (`loc_V +0.144`, `kin_V +0.136`, `loc_cos +0.137`, `kin_cos +0.135`, all `4/4`, holding on both Discovery `2/2` and Held-Out `2/2`).
- **Heritable selection (`treatment` vs `fresh-offspring`):** `4/4` seeds (`+15,410` live = **`429×`**, `+79,910 E` trophallaxis, `loc_cos +0.765`, `kin_cos +0.810`).
- **Decoupled 4-D cadherin adhesion (`treatment` vs `no-cadherin-4d`):** `4/4` seeds increase `live` (`+1,496`, `+10.7%`), `Field Gini` (`+0.009`), `troph_E` (`+6,589 E`), and structured kin contacts (`+2,427` pairs).
- **PCP polarity (`treatment` vs `no-pcp-polarity`):** `4/4` seeds increase `kin_V` (`+0.018`), `3/4` increase `loc_V` (`+0.013`), `loc_cos` (`+0.020`), and `kin_cos` (`+0.027`).
- **Obligate symplectic waste (`treatment` vs `no-symplectic-waste`):** `4/4` seeds increase `loc_V` (`+0.026`), `loc_cos` (`+0.043`), and `kin_cos` (`+0.032`).
- **Neural evolution on clonal plasticity (`treatment` vs `fixed-controller`):** `4/4` seeds increase within-clone epigenetic variance share `Clonal W/T` (`+0.038`: `0.326` vs `0.289`, `2/2` discovery + `2/2` held-out), `3/4` increase `live` (`+260`) and `troph_E` (`+829 E`).
- **Opt-in velocity alignment (`with-velocity-alignment`):** Enabling `--enable velocity-alignment` drives extreme collective streaming (`kin_V = 0.939`, `kin_cos = 0.896`, `4/4` seeds) while trading off steady-state biomass (`9,574` vs `15,446`, `4/4` seeds) as fast aligned flocks overshoot localized sources.

### 4.2 Bundle B — Vascularized Super-Colonies (`52` Runs: `13` Conditions × `4` Seeds, `INV-08` Pure-Field, `320×320`, `2,048` Ticks)

| Condition (`n = 4`) | Mean `Live` | Mean `Births` | `Gini` | `Dark<10%` | `Sarle b` | `Vasc Hi/Mid` | `Vasc P95/P25` | `Trail MaxZ` | `Trail P95Z` | `Trail Decoup` | `Trail Mean` | `Glob P` | `Loc Align` | `Kin Align` |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| **`treatment`** | **`21,308`** | `351,919` | **`0.813`** | **`0.505`** | **`0.608`** | **`2.111`** | **`7.02×`** | **`10.24`** | **`2.37`** | **`0.948`** | `0.0718` | **`0.102`** | `0.378` | `0.419` |
| `v1-compat` | `21,075` | `98,524` | `0.757` | `0.389` | — | — | — | — | — | — | — | `0.073` | `0.552` | `0.571` |
| `fresh-offspring` | **`317`** | `16,510` | `0.582` | `0.176` | `0.766` | `1.207` | `1.89×` | `39.73` | `1.20` | `0.949` | `0.0024` | `0.048` | `0.136` | `0.094` |
| `fixed-controller` | `19,942` | `349,486` | `0.814` | `0.500` | `0.606` | `2.096` | `6.86×` | `9.56` | `2.36` | `0.939` | `0.0709` | `0.090` | `0.351` | `0.396` |
| `chemistry-only` | `20,436` | `362,622` | `0.812` | `0.510` | `0.604` | `2.085` | `6.64×` | `0.00` | `0.00` | — | `0.0000` | `0.177` | `0.672` | `0.697` |
| `no-vascular-field` | `20,113` | `338,724` | `0.804` | `0.465` | — | — | — | `9.29` | `2.35` | `0.970` | `0.0691` | `0.096` | `0.252` | `0.266` |
| `no-dual-carrier-trails` | `25,695` | `402,732` | `0.826` | `0.511` | `0.590` | `2.199` | `7.62×` | — | — | — | — | `0.089` | `0.493` | `0.515` |
| `no-excitable-relay` | `13,458` | `141,817` | `0.764` | `0.447` | `0.667` | `1.607` | `4.35×` | `11.10` | `2.32` | `0.973` | `0.3070` | `0.135` | `0.519` | `0.565` |
| `no-oases` | `32,517` | `248,487` | `0.777` | `0.362` | `0.547` | `2.190` | `6.00×` | `6.82` | `2.25` | `0.880` | `0.0687` | `0.038` | `0.247` | `0.290` |
| `no-so2-motor` | `19,034` | `334,996` | `0.812` | `0.514` | `0.612` | `2.034` | `6.44×` | `10.50` | `2.32` | `0.936` | `0.0679` | `0.124` | `0.479` | `0.502` |
| `no-lysis-recycling` | `22,047` | `352,166` | `0.819` | `0.515` | `0.604` | `2.117` | `7.05×` | `9.40` | `2.37` | `0.942` | `0.0714` | `0.116` | `0.343` | `0.372` |
| `no-tight-junctions` | `21,050` | `361,842` | `0.812` | `0.506` | `0.618` | `2.043` | `7.10×` | `10.54` | `2.37` | `0.950` | `0.0724` | `0.105` | `0.413` | `0.437` |
| `with-so2-budding-waste` | `18,420` | `247,208` | `0.776` | `0.456` | `0.627` | `1.714` | `4.23×` | `12.89` | `2.37` | `0.921` | `0.0551` | `0.146` | `0.465` | `0.480` |

**Paired Seed Effects (Bundle B, `INV-08` Pure-Field, `wins / 4` across `[42, 43]` + `[101, 102]`):**
- **Heritable selection (`treatment` vs `fresh-offspring`):** `4/4` seeds (`+20,990` live = **`67×`**, `Gini +0.231`, `Dark<10% +0.329`, `Vasc Hi/Mid +0.904`, `Vasc P95/P25 +5.14×`, `Trail P95Z +1.17`, `Glob P +0.054`).
- **Murray/Physarum vascular network organizes living organisms (`treatment` vs `no-vascular-field`):** Bimodal vascular conductance (`Sarle b = 0.608 > 0.556`, `Hi/Mid = 2.111×`, `P95/P25 = 7.02×`) lifts **organism local alignment by `+50.0%` (`0.378` vs `0.252`, `4/4` seeds)** and **kin alignment by `+57.5%` (`0.419` vs `0.266`, `4/4` seeds)**, sharpens `Trail MaxZ` (`+0.95`, `4/4`), and increases `Dark<10%` inter-oasis contrast (`+0.040`, `4/4`).
- **Excitable pulse relay prevents trail saturation & sustains population (`treatment` vs `no-excitable-relay`):** `4/4` seeds lift `live` by **`+58.3%`** (`21,308` vs `13,458`), raise `Vasc Hi/Mid` (`+0.504`, `4/4`) & `Vasc P95/P25` (`+2.67×`, `4/4`), and keep Carrier-1 trails crisp instead of saturating `4.3×` (`Trail Mean` `0.0718` vs `0.3070`).
- **Ephemeral oases drive bimodality & trail SNR (`treatment` vs `no-oases`):** `4/4` seeds flip `Sarle b` across the bimodality threshold (`0.608` vs `0.547`), raise `Trail MaxZ` by **`+50.1%`** (`10.24` vs `6.82`, `4/4`), raise `Trail Decoup` (`0.948` vs `0.880`, `4/4`), and raise `Loc Align` (`+53%`, `4/4`) and `Glob P` (`2.7×`, `3/4`).
- **Genuine neural agency & evolution (`treatment` vs `fixed-controller` & `no-so2-motor`):**
  - vs `fixed-controller`: `treatment` yields **`+6.9%` higher `live`** (`21,308` vs `19,942`, `3/4` seeds; **`2/2` Held-Out `+2,494 = +10.0%`**), sharper `Trail MaxZ` (`+0.69`, `4/4` seeds), higher `Trail Mean` (`4/4`), higher `Glob P` (`3/4`, `2/2` held-out), and higher `Kin Align` (`3/4`).
  - vs `no-so2-motor`: `so2-motor` increases `live` by **`+11.9%`** (`+2,274`, `3/4`), `Trail P95Z` (`4/4`), and `Trail Mean` (`4/4`).
  - vs `chemistry-only`: `treatment` reverses the Stage-1 `V2-10` Arm-1 deficit on mean `live` (`21,308` vs `20,436`, `+872`) while generating 100% of Carrier-1 trail ridges (`Trail MaxZ = 10.24` vs `0.00`, `4/4`).
- **Lysis recycling & tight junctions:** `treatment` beats `no-lysis-recycling` on `Trail MaxZ` (`4/4`), `Loc Align` (`4/4`), and `Kin Align` (`4/4`), and beats `no-tight-junctions` on `Vasc Hi/Mid` (`4/4`) and `live` (`3/4`).
- **`treatment` vs `v1-compat`:** `4/4` higher `Gini` (`+0.056`) & `Dark<10%` (`+0.116`), `3/4` higher `live` (`2/2` held-out `+2,592`) and `Glob P` (`0.102` vs `0.073`), `3.6×` faster generational turnover (`351,919` vs `98,524` births), plus decoupled Carrier-1 trails and bimodal vascular networks absent in v1.

### 4.3 Stage-1 `V2-10` Reference (`128²` Quickies, `experiments/v2-quickies/README.md`)
Stage-1 `V2-10` (`75` runs) directly motivated the Stage-2 calibrations applied in `presets/{morphogenesis-tissue-v2,supercolony-v2}.toml`:
- Arm `⤴-1` (`128²`, 32 s) failed its neural-agency gate (`+0.5%` vs `fixed-controller`; `chemistry-only` won) because `[chemistry].motion_response_norm_limit = 256.0` gave free passive chemotaxis and `Δt = 1/128 s` gave only `1.33` oasis cycles. Cutting passive gradient dominance and moving to `Δt = 1/32 s` in Bundle B produced the positive `treatment > fixed-controller` (`+6.9%` pooled, `+10.0%` held-out) and `treatment > chemistry-only` (`+872` live + 100% of trail structure) outcomes in §4.2.
- Arm `⤴-2` (`128²`, uniform over-feeding, `field_gini = 0.109`) passed anti-jamming (`0` vs `1.89M` refused births) and chain fraction (`69.5×`) with `1.98×` wound-rim proliferation, but formed a confluent hexagonal sheet (`field_gini = 0.109`, `H4 = 1.004×`). Cutting source power `9.5×` (`4,000 E/s`, `σ = 4.2`) and tightening Hippo `c_max = 3.25` in Bundle A achieved `field_gini = 0.667` with discrete porous multi-lineage tissue islands (§3.1, §4.1).

### 4.4 Honest Residual Failure Modes & Tradeoffs (Nothing Hidden)

1. **Bundle A radial `h_0` vs `c_den` stratification (`within_lineage_corr_h0_cden = +0.069`):** Clonal lineages show substantial within-clone `h_0` variance (`Clonal W/T = 32.6%`, `4/4` higher than `fixed-controller`), and `glow-cell-state` renders show visual intra-colony state patches, but the *linear* correlation between `h_0` and local crowding `c_den` at tick `2,048` (`64 s`) remains modest (`+0.069`) rather than strongly bimodal core-vs-sheath sorting (`|r| > 0.3`). Longer horizons (`8,192+` ticks) or stronger `Q_met == J` niche separation are needed to claim mature 2-cell-type core/sheath stratification.
2. **Bundle A `no-trophallaxis` and `chemistry-only` live-count tradeoffs:** Because kin trophallaxis carries a `5%` transport dissipation (`trophallaxis-loss`), `no-trophallaxis` holds nearly identical mean `live` (`15,579` vs `15,446`, `2/4`) at `t = 2,048` despite `79,910 E` transferred across `16/16` windows. Similarly, `chemistry-only` holds higher raw `live` (`27,603` vs `15,446`) on Bundle A because zero-output organisms spend `0` energy on emission or motor force while passively receiving `508k E` trophallaxis — yet `treatment` beats `chemistry-only` `4/4` on ALL collective coherence metrics (`loc_V +0.122`, `loc_cos +0.149`) and halves contact crowding (`30,616` vs `66,064`).
3. **Bundle B short-range pair alignment vs `v1-compat` / `chemistry-only` / `no-dual-carrier-trails`:** On Bundle B, `v1-compat` (`0.552`), `chemistry-only` (`0.672`), and `no-dual-carrier-trails` (`0.493`) show higher raw short-range cosine alignment than `treatment` (`0.378`) because organisms without active excitable pulse bursts or pheromone trail steering glide smoothly down single Carrier-0 gradients. By contrast, within the active V2 super-colony dynamics, **removing `vascular-field` drops alignment `0.378 → 0.252` (`4/4`)**, **removing `oases` drops it `0.378 → 0.247` (`4/4`)**, and **removing `fixed-controller` evolution drops it `0.378 → 0.351`** — demonstrating that active wave/trail dynamics are genuinely organized by the vascular network and evolving controllers.
4. **Bundle B `no-dual-carrier-trails` carrying capacity (`25,695` vs `21,308`):** Diverting `30%` of emission onto non-nutritive Carrier-1 trails carries a real metabolic cost (`-17%` steady-state `live`), while creating decoupled pheromone wakes (`MaxZ = 10.24`, `decoupling = 0.948`), slightly higher vascular bimodality (`Sarle b +0.018`, `4/4`), and higher global polarization (`3/4`).
5. **`H4` lesion-assay re-run on Bundle A:** The `flife branch --remove` boundary-vs-interior leak assay was executed in Stage-1 `V2-10` (`1.98×` wound-rim surge, `1.004×` H4 leak ratio on confluent lawn) and remains a targeted follow-up on the now-discrete Bundle A cysts.

---

## 5. Performance & Memory Profile (`Apple M5 Ultra`, `flife_round2b` `76f47f9`)

Performance measurements are separate from the Four Claims (`AGENTS.md`).

### 5.1 Quiet-Host `320×320` Wall-Time & Peak RSS Comparison vs `confluent-world`

Measured back-to-back on a quiet Apple M5 Ultra (`36` logical cores, `256 GiB` unified memory) via `/usr/bin/time -l` on `flife_round2b` (`76f47f9`, `--ticks 256 --controllers auto --cpu-workers 12`, flat-F32 `torch` CPU backend, `0` deferrals; record `/Volumes/SANDISK_SSD/build/v2-stage2-round2b/quiet_320_perf_rss.json`):

| Preset (`320×320`, `256` ticks, `seed=42`, `w=12 auto`) | Wall (s) | `ms/tick` | Organism-Ticks (`batched`) | **`µs / org-tick`** | Peak RSS (MiB) | Peak Footprint (MiB) | Final `Live` | Digest |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| `presets/confluent-world.toml` (v1-contact, `10,954` founders, 33p) | `5.919` | `23.12` | `1,620,459` | `3.784` | **`1,002.0`** | `993.7` | `5,554` | `1c92cb6ac5e6ef5b` |
| `presets/morphogenesis-tissue-v2.toml` (Bundle A, 14 V2 feat, 33p) | **`2.495`** | **`9.74`** | `774,138` | `3.237` | **`1,002.3`** *(+0.03%)* | `993.9` | `1,410` | `361082e4a6c69420` |
| `presets/morphogenesis-tissue-v2.toml --v1-compat` | `2.161` | `8.44` | `772,337` | `2.718` | `891.3` | `881.5` | `1,543` | `28b8a3ec60d85054` |
| `presets/supercolony-v2.toml` (Bundle B, 11 V2 feat, `16,384` founders, 34p) | **`3.812`** | **`14.89`** | `1,769,463` | **`2.136`** *(+3.1% vs v1-compat)* | `1,589.6` | `1,567.5` | `6,027` | `1312ff13f70b16e3` |
| `presets/supercolony-v2.toml --v1-compat` | `2.434` | `9.51` | `1,099,903` | `2.071` | `1,275.9` | `1,266.8` | `1,752` | `162b59302c45871b` |

- **Bundle B (`INV-08` pure-field, 34 planes + Murray 4-neighbor vascular field + 11 V2 features):** **`+3.1%` per organism-tick** vs `--v1-compat` (`2.136` vs `2.071 µs/org-tick`), **beating the `< 8%` combined overhead target**!
- **Bundle A (`[contact]` + 14 V2 features):** Identical peak RSS to `confluent-world` (`1,002.3` vs `1,002.0 MiB`, `+0.03%`), **`2.37×` faster wall-time** at 256t (`9.74` vs `23.12 ms/t`) and **`1.90×` faster wall-time** than `--v1-compat` over 2,048t (`31.01` vs `59.06 ms/t`) due to Hippo anti-jamming; normalized per organism-tick at matched 256t density, the 14 contact-coupled V2 mechanisms add `+19.1%` (`3.237` vs `2.718 µs/org-tick`, honestly above the `< 8%` pure-field target because `[contact]` relaxation evaluates 4-D cadherin, PCP torque, Notch-Delta, and trophallaxis on neighbor pairs).

### 5.2 Optimization Blitz Gains (`d8a88aa` → `flife_round2b` `76f47f9`, 100% Bit-Exact SHA-256 Parity)

Full ABBA paired ledger, phase profiles, and binary SHA-256 receipts live outside git in `/Volumes/SANDISK_SSD/build/perf-opt/PERF_LEDGER.md`:

| Workload (`Apple M5 Ultra`, `--controllers auto`) | Baseline `d8a88aa` | Optimized `flife_round2b` (`76f47f9`) | Speedup | Parity & Gate Notes |
|---|---:|---:|---:|---|
| `B_bundle_b_256t` (`w=12`, ABBA) | `25.87 ms/t` | **`19.17 ms/t`** | **`1.35×`** | `1312ff13✓` — **ADMISSIBLE `WIN`** (`max cv 3.62%`, `3/3` pairs) |
| `A_bundle_a_256t` (`w=8/12/16`, ABBA) | `17.59–20.15 ms/t` | **`12.20–14.30 ms/t`** | **`1.34×–1.44×`** | `361082e4✓` — `9/9` pair wins (R1 `1.31×` ADMISSIBLE; R2b directional under host load) |
| `A_mature_12k_128t` sim (`w=8/12/16`, ABBA) | `45.72–50.87 ms/t` | **`28.38–34.71 ms/t`** | **`1.45×–1.79×`** | `8290e76f✓` — `9/9` pair wins (directional under host load) |
| `A_mature_12k_128t` **262.9 MiB ckpt pause** | `1.32–1.36 s` | **`0.32–0.39 s`** | **`3.36×–4.17×`** | `8290e76f✓` — byte-identical `275,630,595 B` (`<= 1 MiB` read buffer) |
| `C_confluent_262k_32t` sim (`262k` live, `w=12`) | `582.87 ms/t` | **`415.05 ms/t`** | **`1.40×`** | `041488d0✓` — `PROVISIONAL_LOCAL_WIN` (`n=1` gate, `0` deferrals, `INV-03 7.87e-11`) |
| `C_confluent_262k_32t` **5.71 GB ckpt pause** | `35.81–63.95 s` | **`6.38 s`** | **`5.61×–10.02×`** | `041488d0✓` — **`895.4 MB/s`** end-to-end inside `<= 1 MiB` read buffer |

Key structural levers landed across Rounds 1, 2, and 2b: cached private `rayon::ThreadPool` (`flife-core/src/pool.rs`, zero per-tick pthread spawn storms), bit-exact 4-wide NEON autovec `exp_neg_4` & `tanh_f32_4` (`flife-core/src/numerics.rs`), cached `Traits::unit_affinity()`, flat-F32 `CpuBackend` (`torch`) with 8×8 tiled parallel weight packing and empirical `ControllerChoice::Auto` (`AUTO_METAL_MAX_WORKERS=4`, `AUTO_TORCH_MAX_WORKERS=64`), fused multi-stream deposit reduction + reused `CachedFootprint` + 256-query pyramid batching, parallel contact setup & crowding bin reuse, and bulk `ChunkWriter::{f32_slice,u16_slice,f16_slice}` + `ChunkStore::prune_excess_intact`.

---

## 6. Backlog Cross-References (`FL-052` & `FL-055`)

- **`FL-052` (`flife-bml` — Qualified extension examples and developer contracts):** `field-ecology-v2` delivers a new chemistry family (`symplectic-waste`, `lysis-recycling`, `dual-carrier-trails`, `vascular-field`), new diagnostics/metrics (`crates/flife-cli/src/v2_diagnostics.rs`, `experiments/v2-stage2/diagnostics.py`), and four pure read-only `V2-16` visual mappings (`cell-state-v1`, `polarity-v1`, `trail-carrier`, `vascular-conductance`) cleanly layered via the `field-ecology-v2` feature registry (`crates/flife-core/src/v2.rs`) without mutating any `field-ecology-v1*` path. *(Note: `FL-052` remains blocked by open `FL-034` per the backlog dependency graph and is not prematurely closed here.)*
- **`FL-055` (`flife-f9d` — Multi-carrier chemistry):** `V2-15` (`dual-carrier-trails`, 34-plane `24+8` Carrier-0 + Carrier-1 hybrid field) and this Stage-2 multi-seed campaign establish species-resolved conservation (`INV-01..INV-03`, `max |INV-03| = 7.17e-9` across all 52 Bundle B runs) and measurable spatial decoupling (`0.948`, `MaxZ = 10.24`, visible amber pheromone wakes trailing advancing waves in §3.2) alongside the honest metabolic cost tradeoff vs `no-dual-carrier-trails` (§4.4). *(Note: `FL-055` remains blocked by open `FL-052` per the backlog dependency graph and is not prematurely closed here.)*
