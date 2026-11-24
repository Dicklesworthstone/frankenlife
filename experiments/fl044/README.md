# FL-044 & FL-046: Independent-Weight Capacity Frontier and Multi-Rate Profile Measurements

> **Status:** **Observed (`07180f1`, 2026-11-24 UTC)** — real release-binary capacity frontier across populations (`4k` → `400k`, up to `4.00 billion` independent F16 parameters on a `1024² × 33` field), all five catalog topologies (`[8]` through `[128, 96]`), precisions (`F32` vs `F16`), controller routes (`cpu`, `torch`, `metal`), and the separately named multi-rate Desktop profile (`presets/desktop-multirate-4t.toml`, `evaluation_interval_ticks = 4`).
>
> **Four-Claims discipline (`AGENTS.md`, plan §19.5–§19.6):** A performance number on named hardware establishes **none** of the four scientific claims (Rules, Implementation, Visual, Ecological). Every row here also reports `|INV-03|` energy-ledger residual and exact `WorldDigest` parity across backends (Implementation/Rules cross-check) while clearly marking the short founding-transient windows (Limits §6).

---

## 1. Provenance & Hardware Record (plan §19.6, Appendix B.4)

| Item | Observed Value |
|---|---|
| Host CPU | Apple M5 Ultra — `36` logical cores (`12` P-cores + `24` E-cores) |
| Host Memory | `256 GiB` unified memory (`274,877,906,944` bytes) |
| Host GPU | Apple M5 Ultra Metal (`metal (Apple M5 Ultra)`, verified by `flife capabilities`) |
| OS | macOS `27.0.1` (Build `26A434`, `aarch64`) |
| Compiler | `rustc 1.100.0-nightly (908501772 2026-08-30)` |
| Repository Commit | `07180f1` (`main` == `master`) |
| Release Binary | `flife` (`release`, `sha256: 752d594f9dfe7b8a3a2eb2dc8fecb32c185784b1f41e44c02e6b143b63ae7503`) |
| Host Load During Runs | Shared workstation (`vm.loadavg` `33.8–47.0` during runs; exact numbers reflect real loaded-host behavior, not an isolated benchmark box) |
| `flife capabilities` (`w=16`) | `cpu`, `torch`, and `metal` all `verified` against pinned `PRESET_DIGEST` (`82e0720d…`) |
| Preset SHA-256s | `reference-small-world.toml` `064f583f…`, `interactive-world.toml` `f395fe29…`, `desktop-world.toml` `129c134e…`, `desktop-multirate-4t.toml` `08334202…` |

All derived scale/precision/topology TOMLs (`25k_f16`, 5 catalog topologies, `262k_f16`, `400k_f16_k1`, `400k_f16_k4`) vary only their declared scale, dtype, hidden widths, interval, and memory quotas from the committed presets above (no synthetic GEMMs, no shared weights, no disabled physics).

---

## 2. Steady-State Scale Frontier (`4k` → `400k`, Apple M5 Ultra Metal)

Per plan §19.6, steady-state tick rate is measured across `progress` timestamps **after** the initial warmup block (which absorbs the one-time tick-1 GPU weight-shard upload) and **before** the post-run single-threaded `Survey::of` genome hash pass (`survey_s`, reported separately so startup/shutdown work never obscures or inflates tick throughput). Timestep is `Δt = 1/128 s` (`sim_x = steady_tps / 128`).

| Workload | Grid × Planes | Dtype / `K` | Warmup / Steady Ticks | Steady `ticks/s` | `sim_s / wall_s` | Steady `Gparam-evals/s` | Metal `ms/tick` | Metal Weight Read `GB/s` | Warmup Block (`s`) | End `Survey` (`s`) | End `live` | Digest (12 hex) |
|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| `4k-f32-torch-w24` | `128² × 33` | `F32` / `1` | `128` / `384` | `202.64` | `1.583×` | `5.18` | — | — | `1.82` | `0.01` | `481` *(bottleneck)* | `1cb0623a9205` |
| `4k-f32-metal-w24` | `128² × 33` | `F32` / `1` | `128` / `384` | `199.99` | `1.562×` | `5.12` | `0.76` | `134.4` | `0.92` | `0.01` | `481` *(bottleneck)* | `1cb0623a9205` |
| `25k-f32-metal-w24` | `256² × 33` | `F32` / `1` | `64` / `128` | `47.92` | `0.374×` | `11.96` | `2.35` | `425.3` | `1.56` | `0.59` | `24,862` | `72956e9668ae` |
| `25k-f16-metal-w24` | `256² × 33` | `F16` / `1` | `64` / `128` | `55.79` | `0.436×` | `13.93` | `1.17` | `426.9` | `1.30` | `0.44` | `24,862` | `8d6c81c3fe7a` |
| `100k-f16-k1-metal-w32` | `512² × 33` | `F16` / `1` | `32` / `96` | **`17.88`** | `0.140×` | **`17.86`** | `4.77` | `418.5` | `2.19` | `1.77` | `100,032` | `9584bef6d6fa` |
| `100k-f16-k4-metal-w32` | `512² × 33` | `F16` / `4` | `32` / `96` | **`19.63`** | `0.153×` | `5.02` | `2.33` | `219.5` | `2.12` | `1.77` | `100,032` | `e39ce1b4e5d1` |
| `262k-f16-k1-metal-w32` | `512² × 33` | `F16` / `1` | `16` / `32` | **`8.71`** | `0.068×` | **`22.80`** | `11.61` | `450.9` | `2.89` | `4.56` | `262,144` | `d32c4ad09019` |
| **`400k-f16-k1-metal-w32`** | **`1024² × 33`** | **`F16` / `1`** | `8` / `24` | **`5.42`** | `0.042×` | **`21.66`** | **`17.89`** | **`446.7`** | `3.27` | `7.11` | **`400,000`** | `6aa0adbd69fd` |
| `400k-f16-k4-metal-w32` | `1024² × 33` | `F16` / `4` | `16` / `32` | `5.40` | `0.042×` | `5.74` | `8.03` | `264.4` | `4.77` | `7.27` | `400,000` | `baee86d6da0b` |

### Key Finding — Where the `400k` (`4.00B`-Parameter) Wall Lives

At `400,000` live organisms (`3.995 billion` independent F16 parameters = `7.990 GB` of unique weights read every tick, `1024² × 33` field, `0` Metal deferrals, `21.26 GiB` peak RSS, `7.80 GiB` resident GPU weight shards, `|INV-03| = 2.71e-12`):

- **Fused independent-weight Metal inference alone runs at `17.89 ms/tick` (`446.7 GB/s` sustained weight-read bandwidth) = `55.9 full-population sweeps/s` (`223.3 Gparam-evals/s` GPU-only)** — already **1.86× above** Gate G7's `30 t/s` threshold!
- **Host CPU tick phases** (`1024² × 33` transport/sources/pyramid + 400k sensor sampling/encoding/uptake/settle/deposit) consume **`~166.6 ms/tick` (`90.3%` of the `184.5 ms` tick)**, capping end-to-end full-evaluation throughput on this architecture at **`5.42 steady t/s`** (`17.88 steady t/s` at `100k`, `55.79 steady t/s` at `25k`).
- Consequently, reducing Metal controller cost from `17.89 → 8.03 ms/tick` via `K = 4` changes total `400k` Metal wall time by only `~5%` (within shared-host variance), whereas on CPU-only (`100k`), `K = 4` delivers a **`2.10×`** speedup (`7.37 → 15.45 t/s`). Reaching `≥ 30 t/s` end-to-end at `400k` requires moving the remaining host field/sensing/uptake/settle phases onto a GPU-resident tick, not further neural weight compression.

---

## 3. End-to-End Backend & Precision Parity (`cpu` vs `torch` vs `metal`)

Every run below reports full `flife run` `wall_seconds` (including tick-1 weight upload and end-of-run `Survey::of`), exact peak RSS (`/usr/bin/time -l`), resident GPU bytes, `|INV-03|` normalized ledger residual, and SHA-256 world digest (`0` deferrals on every `torch` and `metal` run).

| Case | Ticks / `w` | End-to-End `wall_s` | `ticks/s` | Metal `ms/t` | Metal `GB/s` | Resident GPU (`MiB`) | Peak RSS (`MiB`) | `|INV-03|` | Digest (12 hex) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| `4k-f32-cpu` | `256` / `24` | `1.72` | `149.21` | — | — | `0.0` | `271.9` | `9.92e-10` | `03cfbb3a72f6` |
| `4k-f32-torch` | `256` / `24` | `1.58` | `162.09` | — | — | `156.2` | `432.5` | `9.92e-10` | `03cfbb3a72f6` |
| `4k-f32-metal` | `256` / `24` | `1.75` | `146.07` | `0.76` | `214.1` | `211.1` | `494.4` | `9.92e-10` | `03cfbb3a72f6` |
| `25k-f32-cpu` | `128` / `24` | `3.50` | `36.57` | — | — | `0.0` | `1,524.3` | `5.36e-10` | `b6a19258cf23` |
| `25k-f32-torch` | `128` / `24` | `3.26` | `39.30` | — | — | `956.5` | `2,485.8` | `5.36e-10` | `b6a19258cf23` |
| `25k-f32-metal` | `128` / `24` | `3.17` | `40.32` | `1.91` | `524.0` | `1,043.5` | `2,578.2` | `5.36e-10` | `b6a19258cf23` |
| `25k-f16-cpu` | `128` / `24` | `3.58` | `35.74` | — | — | `0.0` | `878.0` | `5.04e-10` | `3f0f661285d8` |
| `25k-f16-torch` | `128` / `24` | `2.98` | `42.90` | — | — | `956.5` | `1,833.2` | `5.04e-10` | `3f0f661285d8` |
| `25k-f16-metal` | `128` / `24` | `2.79` | `45.82` | `1.16` | `430.1` | `531.7` | `1,411.7` | `5.04e-10` | `3f0f661285d8` |
| `100k-f16-k1-cpu` | `64` / `24` | `8.68` | `7.37` | — | — | `0.0` | `3,471.8` | `1.39e-11` | `9c090a08418d` |
| `100k-f16-k1-torch` | `64` / `24` | `6.03` | `10.61` | — | — | `3,816.1` | `7,276.9` | `1.39e-11` | `9c090a08418d` |
| `100k-f16-k1-metal` | `64` / `24` | `5.40` | `11.86` | `4.52` | `441.9` | `2,125.9` | `5,576.4` | `1.39e-11` | `9c090a08418d` |
| `100k-f16-k1-metal-w32` | `128` / `32` | `9.16` | `13.98` | `4.27` | `468.0` | `2,382.5` | `5,854.2` | `7.37e-11` | `9584bef6d6fa` |
| `262k-f16-k1-metal-w32` | `32` / `32` | `9.13` | `3.51` | `11.12` | `470.8` | `5,280.0` | `14,020.4` | `8.75e-12` | `6e8627e90dfd` |
| `400k-f16-k1-metal-w32` | `16` / `32` | `12.66` | `1.26` | `17.85` | `447.6` | `7,991.0` | `21,767.1` | `2.71e-12` | `82868217e265` |

- **Bit-exact parity (`INV-11`, `G4`):** `cpu`, `torch`, and `metal` produced **identical SHA-256 digests** on every single scale and precision tested (`4k F32`, `25k F32`, `25k F16`, `100k F16 K=1`, `100k F16 K=4`).
- **Precision effect (`F16` vs `F32` at `25k`):** Switching `F32 → F16` cuts Metal GPU time `1.65×` (`1.91 → 1.16 ms/tick`), cuts Metal resident weight memory `1.96×` (`1,043.5 → 531.7 MiB`), cuts peak RSS `1.83×` (`2,578.2 → 1,411.7 MiB`), and raises steady-state Metal throughput from `47.92 → 55.79 t/s` (`+16.4%`). F16 is a distinct quantized model (`3f0f661285d8 ≠ b6a19258cf23`).

---

## 4. Topology Catalog Frontier (`25,000` Founders, `256² × 33`, `F16`, `64` Ticks, `w = 24`)

All five members of `dense-104-36-catalog-v1` evaluated end-to-end with `0` deferrals and **100% bit-exact `torch == metal` SHA-256 parity**:

| Hidden Widths | Params / Org | Total F16 Weights (`MB`) | `torch` `t/s` | `metal` `t/s` | Metal `ms/tick` | Metal `GB/s` | `metal` `Gp/s` | `torch` RSS (`MiB`) | `metal` RSS (`MiB`) | `|INV-03|` | Digest (12 hex) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| `[8]` | `1,164` | `58.2` | `55.45` | `53.53` | `0.39` | `150.3` | `1.56` | `533.5` | `696.9` | `3.68e-10` | `ffeaf22343c3` |
| `[32]` | `4,548` | `227.4` | `49.02` | `47.91` | `0.60` | `375.8` | `5.45` | `1,041.4` | `896.6` | `2.63e-10` | `5d28358b780e` |
| `[64, 32]` | `9,988` | `499.4` | `37.74` | `35.60` | `1.08` | `461.3` | `8.89` | `1,820.5` | `1,411.8` | `2.41e-10` | `8b19e53771b1` |
| `[96, 64]` | `18,628` | `931.4` | `26.54` | `30.20` | `2.06` | `452.9` | `14.06` | `3,301.2` | `2,577.0` | `3.94e-10` | `c42242374b1a` |
| `[128, 96]` | `29,284` | `1,464.2` | `22.78` | `24.60` | `3.40` | `430.9` | `18.01` | `4,719.6` | `3,475.8` | `2.67e-10` | `133171f3b6b5` |

Metal overtakes CPU FrankenTorch once independent weight volume exceeds `~0.6–0.9 GB` (`[96, 64]` and `[128, 96]`, or `100k × [64, 32]`) while using **`1.24–1.70 GiB` less peak RSS** because Metal stores canonical F16 weights directly on GPU shards without Torch's F32 host widening copy.

---

## 5. Multi-Rate Profile Measurements (`FL-046`: `presets/desktop-multirate-4t.toml`)

Per plan §6.6 and §19.5, `presets/desktop-multirate-4t.toml` is a **separately named** operating profile (`desktop-world.toml` with only `name` and `controller.evaluation_interval_ticks = 4` changed; verified by `crates/flife-cli/tests/multirate_preset.rs`, 4/4 passed). Nominal full-population-equivalent sweeps/s (`ticks/s / 4`) is reported alongside **measured** individual evaluations/s and parameter evaluations/s (which include forced tick-1 founder and newborn evaluations):

| Run (`128` ticks, `100,000` founders, `live_end = 100,032`) | Route / `w` | End-to-End `ticks/s` | Steady `ticks/s` | Nominal `equiv sweeps/s` (`t/s ÷ 4`) | Total Measured Evals (`128t`) | Measured `evals/s` (e2e) | Measured `Gparam-evals/s` (e2e) | Metal `ms/t` | `|INV-03|` | Digest (12 hex) |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| `100k-f16-k4-cpu-w24` | `cpu` / `24` | `15.45` | — | `3.86` | `3,275,155` | `395,550` | `3.95` | — | `1.63e-10` | `e39ce1b4e5d1` |
| `100k-f16-k4-torch-w24` | `torch` / `24` | `15.70` | — | `3.92` | `3,275,155` | `401,659` | `4.01` | — | `1.63e-10` | `e39ce1b4e5d1` |
| `100k-f16-k4-metal-w24` | `metal` / `24` | `15.53` | — | `3.88` | `3,275,155` | `397,477` | `3.97` | `1.98` | `1.63e-10` | `e39ce1b4e5d1` |
| `100k-f16-k4-metal-w32` | `metal` / `32` | `14.78` | **`19.63`** | `3.70` *(steady `4.91`)* | `3,275,155` | `378,270` *(steady `502,250`)* | `3.78` *(steady `5.02`)* | `1.99` | `1.63e-10` | `e39ce1b4e5d1` |

Note the exact §19.5 discrepancy between nominal and measured work: over `128` ticks, `100,000 × 128 / 4 = 3,200,000` is the nominal quarter-rate baseline, whereas measured evaluations are **`3,275,155`** (`+75,155` extra evaluations from forced tick-1 founder evaluations of phases `0, 2, 3` and newborn forced first evaluations!).

---

## 6. Limits & Open Blockers

- **Founding-transient windows:** All runs here are short (`16–512` ticks) starting from uniform founders rather than long-run evolved checkpoints (`8,192+` ticks). In particular, `4k` drops to `481` live by tick `512` (the Reference preset's known founding bottleneck), whereas `25k`, `100k`, `262k`, and `400k` remain at `24,862`, `100,032`, `262,144`, and `400,000` live across their measured windows.
- **Single-host, shared-load measurements:** Only Apple M5 Ultra (`macOS 27.0.1`) was measured here, under background workstation load (`loadavg 33.8–47.0`). Quiet-host rates and Linux x86-64 CPU reference points remain additive measurements.
- **No Gate G7 or ecological claim:** `400k` steady-state full-eval throughput is `5.42 t/s` (`0.042×` real-time), below Gate G7's `30 t/s` target. `FL-044` (`flife-jy1`) and `FL-046` (`flife-9as`) remain open strictly because upstream blocker `FL-034` (`flife-2je`) is still open.
