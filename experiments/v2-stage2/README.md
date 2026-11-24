# V2-17 Stage-2 Verification & Ablation Harness

**Status:** **Observed (`76f47f9`, 2026-11-24 UTC)** — full `104 / 104` Stage-2 multi-seed campaign (`44` Bundle A + `44` Bundle B + `16` `chemistry-only`/`fresh-offspring` controls on unmodified `320×320` presets across Discovery `[42, 43]` & Held-Out `[101, 102]`, `2,048` ticks + `1280×1280` Gate G5 & `V2-16` renders) completed with `0` faults and `0` extinctions. Full Four-Claims analysis and tables are published in [`experiments/v2-emergence/README.md`](../v2-emergence/README.md).

This directory implements the reproducible Stage-2 campaign for `V2-17`, exercising the two authored `field-ecology-v2` showcase presets alongside modular single-mechanism CLI ablations, `v1-compat` baselines, and `fixed-controller` controls:

- **Bundle A (`presets/morphogenesis-tissue-v2.toml`):** 80×80 periodic torus, 320×320×33 field (`h = D = 0.25`), 10,954 founders, `inv08_pure_field = false`, 14 active tissue/differentiation/trophallaxis features.
- **Bundle B (`presets/supercolony-v2.toml`):** 80×80 periodic torus, 320×320×34 hybrid field, 16,384 founders, `inv08_pure_field = true`, 11 active pure-field oasis/excitable/vascular/dual-carrier features.

---

## Files

| File | Role |
|---|---|
| `run_stage2.sh` | Reproducible bash runner calling real `flife validate`, `flife run --metrics-every`, `flife inspect --field`, and optional `flife render --run-dir`. Keeps every run (complete, extinct, paused, or failed) under `--out` outside git. |
| `diagnostics.py` | Pure Python stdlib evaluator consuming `frankenlife.inspect.v1` and `frankenlife.run.v1` metrics NDJSON streams to emit one `frankenlife.v2-stage2-diagnostics.v1` JSON record per run. Reports `available: false` and `null` whenever an optional plane or state is absent. |
| `test_diagnostics.py` | Independent hand-computed known-answer unit tests (`unittest`) covering all five Stage-2 diagnostics and the CLI pipeline with zero mocks and zero third-party packages. |

---

## Ablation Matrices

### Bundle A (`presets/morphogenesis-tissue-v2.toml`)

| Condition | Mechanism Isolated | CLI / Model Override | Mode |
|---|---|---|---|
| `treatment` | Full Bundle A (14 features) | *(preset default)* | smoke, full |
| `no-epigenetic-notch` | Epigenetic recurrence + Notch-Delta (`V2-12`) | `--disable epigenetic-recurrence,notch-delta` | smoke, full |
| `no-notch-delta` | Lateral inhibition coupling (`V2-12`) | `--disable notch-delta` | smoke, full |
| `no-trophallaxis` | Kin energy sharing (`V2-13`) | `--disable trophallaxis` | smoke, full |
| `v1-compat` | Full v1 baseline (`INV-14`) | `--v1-compat` | smoke, full |
| `fixed-controller` | Zero-agency neural control (`H2`) | `model = "field-ecology-v2-fixed-controller"` | smoke, full |
| `no-cadherin-4d` | Decoupled 4-D adhesion tag (`V2-13`) | `--disable cadherin-4d` | full |
| `no-lysis-recycling` | Viability-floor corpse lysis (`V2-11`) | `--disable lysis-recycling` | full |
| `no-symplectic-waste` | Obligate byproduct recycling (`V2-11`) | `--disable symplectic-waste` | full |
| `no-pcp-polarity` | Apical-basal PCP polarity & lateral mitosis (`V2-08`) | `--disable pcp-polarity` | full |
| `with-velocity-alignment` | Contact kin flocking comparison (`V2-13`) | `--enable velocity-alignment` | full |

`--validate-only` also verifies the required `V2-12` dependency refusal: `flife validate presets/morphogenesis-tissue-v2.toml --disable epigenetic-recurrence` (without disabling `notch-delta`) must exit non-zero, while `--disable epigenetic-recurrence,notch-delta` must succeed.

### Bundle B (`presets/supercolony-v2.toml`, `INV-08` pure-field)

| Condition | Mechanism Isolated | CLI / Model Override | Mode |
|---|---|---|---|
| `treatment` | Full Bundle B (11 showcase features) | *(preset default)* | smoke, full |
| `no-vascular-field` | Murray/Physarum flux-reinforced arteries (`V2-14`) | `--disable vascular-field` | smoke, full |
| `no-dual-carrier-trails` | Carrier-1 persistent pheromone trails (`V2-15`) | `--disable dual-carrier-trails` | smoke, full |
| `v1-compat` | Full v1 baseline (`INV-14`) | `--v1-compat` | smoke, full |
| `fixed-controller` | Zero-agency neural control (`H2`) | `model = "field-ecology-v2-fixed-controller"` | smoke, full |
| `no-excitable-relay` | Vesicle-readiness pulse relay (`V2-04`) | `--disable excitable-relay` | full |
| `no-oases` | Ephemeral oasis bloom landscape (`V2-02`) | `--disable oases` | full |
| `no-so2-motor` | SO(2)-equivariant chemotaxis/propulsion (`V2-05`) | `--disable so2-motor` | full |
| `no-lysis-recycling` | Viability-floor corpse lysis (`V2-11`) | `--disable lysis-recycling` | full |
| `no-tight-junctions` | Passive flux-form diffusion barrier (`V2-09`) | `--disable tight-junctions` | full |
| `with-so2-budding-waste` | Full 13-feature Bundle B default closure | `--enable so2-budding,symplectic-waste` | full |

---

## Stage-2 Diagnostics (`diagnostics.py`)

1. **Field spatial contrast (`field_spatial_contrast`):** Computes Carrier-0 `field_gini`, `mean`, `median`, `p95`, `max`, and `dark_fraction_below_10pct_mean` directly from `field.energy` to verify dark inter-oasis / inter-tissue medium (`field_gini >= 0.45` target).
2. **Clonal differentiation (`V2-12`):** Reads `held.recurrent_state[0]` (`h_0`) per live organism and computes local neighbor crowding `c_den` on the periodic torus within `r_c = 0.375` world units. Outputs member-weighted within-lineage variance, Bessel-corrected pooled within-lineage variance, between-lineage variance, global `corr(h_0, c_den)`, and within-clone centered `corr(h_0, c_den)`.
3. **Murray vascular bimodality (`V2-14`):** Reads `field.vascular_conductance` from `flife inspect --field`. Outputs high-band (`>= P90`) to mid-band (`[P25, P75]`) conductance ratio, `P90/P50`, `P95/P50`, midpoint-exceedance fraction, and finite-sample Sarle's bimodality coefficient `b` (uniform = `5/9 ≈ 0.556`).
4. **Trail-carrier SNR vs Carrier-0 (`V2-15`):** Reads `field.trail_energy` (`e_1`) alongside `field.energy` (`e_0`). Outputs `(P95 - median) / std`, `(max - median) / std`, `P95 / median`, top-decile energy share, Carrier-1-to-Carrier-0 SNR ratios, and spatial decoupling `1 - |corr(e_1, e_0)|`.
5. **Trophallaxis flux (`V2-13`):** Sums `window.transfers["trophallaxis"]` and `window.flows["trophallaxis-loss"]` across `flife run --metrics-every` windows, reporting total credited/debited energy, mean flux per tick, and live-weighted flux per organism-tick.
6. **Flock polarization (`V2-13`):** Computes global Vicsek polarization `P = ||(1/N) Σ (v_i / ||v_i||)||` over moving organisms (`||v_i|| > 1e-9`) alongside local neighborhood Vicsek polarization, local pairwise cosine alignment, and same-lineage kin local alignment on the periodic torus (`r_align = 0.375`).

---

## Visual Review Hooks (`--render`, `V2-16` & Gate G5)

When `--render` is passed, both `treatment` and `v1-compat` checkpoints are rendered through `flife render --run-dir` into `$OUT_DIR/renders/bundle-{a,b}/{treatment,v1-compat}/seed-<S>/` using bundle-appropriate mechanism overlays paired with shared fair-comparison (`lineage-energy`) and Gate G5 linear-grey (`raw`) views:

- **Bundle A (`treatment` + `v1-compat`):** shared `sphere-lineage-energy` (`--no-field`) + `raw-lineage-energy`, plus `treatment`-only `glow-cell-state`, `raw-cell-state`, and high-contrast dark-backdrop `sphere-polarity` (`--no-field`).
- **Bundle B (`treatment` + `v1-compat`):** shared `glow-lineage-energy` + `raw-lineage-energy`, plus `treatment`-only `glow-vascular-conductance`, `glow-trail-carrier`, `raw-vascular-conductance`, and `raw-trail-carrier`.

---

## Running

```bash
# 1. Run independent known-answer analytical oracle tests (no compiled binary needed):
python3 -m unittest discover -s experiments/v2-stage2 -p 'test_*.py' -v

# 2. Validate Bundle A/B presets, fixed-controller variants, CLI matrices, and notch-delta dependency refusal:
experiments/v2-stage2/run_stage2.sh --validate-only --flife target/release/flife

# 3. Fast scaled 64x64 smoke run (clearly tagged mode="smoke", writes to artifacts/v2-stage2/smoke):
experiments/v2-stage2/run_stage2.sh --smoke --flife target/release/flife --render

# 4. Full Stage-2 multi-seed campaign on unmodified 320x320 presets (writes to artifacts/v2-stage2/full):
experiments/v2-stage2/run_stage2.sh --full --flife target/release/flife --render
```

---

## Four-Claims Ledger

| Claim | Status | Notes |
|---|---|---|
| **Rules** (`INV-01..15`, ledger closure) | **Observed (PASS)** | `104 / 104` full `320×320` 2,048-tick runs reached (`0` extinct, `0` paused, `0` failed, `0` refused births, max `\|INV-03\| = 7.17e-9`). See [`experiments/v2-emergence/README.md`](../v2-emergence/README.md). |
| **Implementation** (`diagnostics.py` oracles + E2E) | **Observed (PASS)** | 16 analytical oracle cases in `test_diagnostics.py` + 4/4 E2E suites in `crates/flife-cli/tests/v2_presets.rs` (all 21 features, leave-one-out, Metal F16 bit-exact `0` deferrals, `flife experiment` controls). |
| **Visual** (Gate G5 raw + sphere + `V2-16`) | **Observed (2026-11-24)** | `1280×1280` `raw`, `sphere`/`glow`, and all four `V2-16` overlays reviewed on Discovery (`42`) and Held-Out (`101`) for both Bundle A and Bundle B (`experiments/v2-emergence/README.md` §3). |
| **Ecological** (paired multi-seed ablations) | **Observed (Positive + Residuals)** | Full 4-seed (`[42, 43]` discovery + `[101, 102]` held-out) tables, paired win counts, and honest residual tradeoffs reported in [`experiments/v2-emergence/README.md`](../v2-emergence/README.md) §4. |
