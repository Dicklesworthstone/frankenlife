# V2-10: Preregistered `128²` Quickie Verification Campaign (`⤴-1` Neural Agency `H2` & `⤴-2` Tissue Morphogenesis + `H4`)

> **Status:** **Preregistered / Target** — configs, experiment specs, and runner are frozen and schema-validated against `crates/flife-core/src/v2.rs`. No empirical campaign run has been executed yet (pending completion of Wave-1/Wave-2 mechanics `V2-02`–`V2-09` and central `flife` release build). No target value below is an observed result (`AGENTS.md` Honesty Rules).

## Purpose

Before calibrating Stage-2 showcase presets (`V2-17`: `presets/morphogenesis-tissue-v2.toml` and `presets/supercolony-v2.toml`), this preregistered harness empirically tests FrankenLife v2's two highest-stakes hypotheses on fast `128²` multi-seed runs using exact implemented `field-ecology-v2` controls and modular `--disable`/`--enable` toggles:

1. **Arm `⤴-1` — Neural Agency (`H2`, `inv08_pure_field = true`):**
   - **Base:** `arm1-neural-agency-128.toml` (`128 × 128`, 4,096 founders, `emission.maximum_fractional_rate_per_second = 0.0` so motility is strictly isolated from emission, active features: `oases`, `senescence`, `so2-motor`, `v2-visual-mappings`; ABI `104/40/40/33`).
   - **Controls (`arm1/spec.toml`):**
     - `fixed-controller` (`field-ecology-v2-fixed-controller`, traits mutate, neural weights frozen),
     - `chemistry-only` (`field-ecology-v2-chemistry-only`, zero-output actions, identical neural cost),
     - `no-oases` (`v2.oases.enabled = false`, uniform source power).
   - **Seeds & Horizon:** Discovery `[101, 102, 103, 104]`, held-out `[2101, 2102]`; `4,096` ticks (32 s), metrics every 512 ticks. Every run (complete, extinct, paused, or failed) is cataloged and reported.
   - **Target Pass Gate (preregistered):** Treatment beats `fixed-controller` by `≥ 15–20%` steady-state biomass or `≥ 25%` harvest efficiency (`G / C_motion`) **and** evolves `|corr(m_e, dE/dt)| ≥ 0.25` (or `|corr(m_e, E)| ≥ 0.25`) on `≥ 3/4` discovery seeds and holds in the same direction on held-out seeds.

2. **Arm `⤴-2` — Tissue Morphogenesis & `H4` Boundary Sealing (`inv08_pure_field = false`):**
   - **Base:** `arm2-tissue-morphogenesis-128.toml` (`32 × 32` torus on `128 × 128` grid so `h = D = 0.25`, `[contact]` with `k = 0.75, k_a = 0.35`, active features: `pcp-polarity`, `hippo-crowding` with `c_max = 4.5`, `tight-junctions` with `φ_min = 0.04`, `v2-visual-mappings`; ABI `104/36/36/33`).
   - **Controls (`arm2/spec.toml`):**
     - `v1-compat` (all v2 features disabled — exact isotropic `[contact]` baseline),
     - `no-hippo-crowding` (`v2.crowding.enabled = false` — tests the 201% jamming prediction),
     - `frozen-polarity` (`v2.polarity.torque_mobility = 0.0, v2.polarity.oriented_lateral_mitosis = false`),
     - `no-tight-junctions` (`v2.barrier.enabled = false`),
     - `uniform-low-diffusion` (`v2.barrier.enabled = false, world.diffusion = 0.001194868`, `0.04×` uniform diffusion).
   - **Seeds & Horizon:** Discovery `[201, 202, 203, 204]`, held-out `[2201, 2202]`; `3,200` ticks (100 s), metrics every 400 ticks.
   - **Perturbation Assays (`flife branch --remove`):** At tick `2048` (`128` in `--smoke`), `run_quickies.sh` branches `treatment`, `--v1-compat`, and `--disable tight-junctions` four ways (`control`, `boundary`, `interior`, `distant`) using area-matched removal disks, resumes to `+256` ticks (`+64` in `--smoke`), and measures wound-rim vs far-field proliferation and boundary-vs-interior field leakage via `flife inspect --field`.
   - **Target Pass Gate (preregistered):** `stacking < 1.25`, `⟨|ψ_6|⟩ < 0.55`, chain fraction `≥ 2.5×` baseline, wound-rim birth rate `≥ 3×` interior, `H4` boundary-vs-interior leak ratio `> 3×`, plus raw & sphere frame inspection.

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

# 2. Fast 64x64 CI smoke mode (< 30 s target; writes artifacts outside git)
./experiments/v2-quickies/run_quickies.sh --smoke --flife target/release/flife

# 3. Full preregistered 128x128 multi-seed campaign (optional --render for raw/sphere PNGs)
./experiments/v2-quickies/run_quickies.sh --full --flife target/release/flife --render
```

All run directories, SQLite experiment catalogs, `flife inspect` dumps, and rendered PNGs default to `artifacts/v2-quickies/<mode>/` (outside git, per `AGENTS.md`).

## Results (Four Claims — To Be Filled After Execution)

- **Rules (`INV-01`–`INV-15`):** *Not yet measured.*
- **Implementation (toggle parity, `v2_diagnostics` synthetic crystal/ring oracle):** *Not yet measured.*
- **Visual (raw + sphere + `polarity-v1` / `cell-state-v1` review):** *Not yet measured.*
- **Ecological (Arm `⤴-1` `H2` & Arm `⤴-2` Tissue + `H4`, discovery & held-out seeds, all failed/extinct runs kept):** *Not yet measured.*
- **Parameter Calibrations for `V2-17`:** *Pending campaign execution.*
