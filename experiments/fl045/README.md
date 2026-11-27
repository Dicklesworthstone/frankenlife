# FL-045: Full-Evaluation 400k Screenshot-Scale Operating Profile (`presets/screenshot-world.toml`)

> **Status:** **Observed (`5ee847f`, 2026-11-27)** — complete energy/environment preset (`presets/screenshot-world.toml`), preflight memory admission, compact CPU/Metal parity tests (`crates/flife-cli/tests/screenshot_preset.rs`), and real Apple M5 Ultra Metal full-evaluation 400k measurements with explicit capacity-limit accounting per M4's exit decision (*"Publish full-profile measurements or a precise capacity limit"*, plan §22.1).
>
> **Four-Claims discipline:** Performance measurements on named hardware establish none of the four scientific claims (Rules, Implementation, Visual, Ecological). Ledger residual `|INV-03|` and genome distinctness are reported alongside distinct simulation, wall, and GPU clocks.

---

## 1. Preset Specification & Scale Derivation

`presets/screenshot-world.toml` is `presets/desktop-world.toml` scaled cleanly by `4×` area (`512² → 1024²`) with exactly 11 declared changes verified by `crates/flife-cli/tests/screenshot_preset.rs`:
- **World:** `1024.0 × 1024.0` torus, `1024 × 1024 × 33` F32 field (`Δt = 1/128 s`).
- **Population:** `400,000` founders (`640,000` ceiling, `1.6×` headroom so capacity does not silently bind inside Gate G7's `≥ 380,000` band; `capacity_policy = "pause-before-commit"`, `automatic_reseed = false`).
- **Energy Budget:** `512` moving Gaussian sources × `1,248.0` power = `638,976` energy/s (`0.609` energy/unit²/s, matching `interactive-world` E1 and `desktop-world` E2).
- **Neural Controllers (Full Evaluation `K = 1`):** `400,000` independent `F16` MLPs (`[64, 32]`, `9,988` parameters/organism — inside Gate G7's `9,000–11,000` band; `3,995,200,000` active independent parameters = `7.990 GB` canonical weights read every single tick).
- **Memory Quotas:** `16 GiB` logical genome quota, `18 GiB` allocated genome quota, `64 GiB` total admitted budget (`pause-and-stream` checkpoints admitted cleanly).

---

## 2. Observed 400k Full-Evaluation Measurement (Apple M5 Ultra Metal)

| Field | Observed Value |
|---|---|
| Host Hardware | Apple M5 Ultra (`36` logical cores, `256 GiB` unified memory, Metal `Apple M5 Ultra`, macOS `27.0.1`) |
| Commit / Binary | `5ee847f` (`release` `flife`, `sha256: a0ed3bfa2982…`) |
| Preset SHA-256 | `a19a0ae86eb2647fe3cd3fd405baf0f35d6c48f500c26fea38ff3d1fdc9c7079` (`config_hash: 56b1aa3ec825…`) |
| Host Load During Run | Shared workstation (`vm.loadavg = 110.57 / 107.32 / 156.51`) |
| Command | `flife run presets/screenshot-world.toml --ticks 64 --every 16 --metrics-every 16 --controllers metal --cpu-workers 32` |
| Live Population (ticks `0..=64`) | `400,000` constant (`distinct_genomes = 400,000`, `distinct_parameter_sets = 400,000`, `0` refused births) |
| Total Controller Evaluations | `25,600,000` batched on Metal, **`0` deferred**, `0` reference fallback ticks |
| Metal GPU Compute Time | `930.7 ms` across 64 ticks = **`14.54 ms/tick`** (`68.8 full-population sweeps/s` GPU-only; **`549.4 GB/s`** sustained unique-weight bandwidth) |
| Steady-State Tick Rate (ticks `16 → 64`) | **`6.46 ticks/s`** (`48` ticks in `7.4345 s`: `6.83`, `6.40`, `6.17 t/s` per 16-tick block; **`25.81 Gparam-evals/s`** end-to-end) |
| Distinct Clocks (`Δt = 1/128 s`) | `64` ticks = `0.500` simulated seconds in `17.06 s` stepping wall time (`17.69 s` incl. `Survey::of`; `0.050×` real-time steady) |
| Memory Footprint | Peak RSS `19.71 GiB` (`21,160,935,424` bytes), Metal resident weight shards peak `7.80 GiB` (`8,379,187,408` bytes), admitted ceiling `64 GiB` |
| Energy Conservation (`|INV-03|`) | `1.772 × 10⁻¹²` normalized residual at tick `64` |
| Committed World Digest (tick `64`) | `27bb8907c3efb08ed5ed340f54afccdd81dda2cc654535911446d04d677f492a` |

---

## 3. Honest Capacity Limit vs Gate G7 Target (M4 Exit Decision)

Per plan §21.3 and §22.1, FrankenLife states the precise measured capacity limit without lowering cadence, enlarging `Δt`, or sharing weights:
- **What passes today:** `presets/screenshot-world.toml` validates, admits within `64 GiB`, runs `400,000` independent `9,988`-parameter `F16` controllers on `1024² × 33` with `0` Metal deferrals, `|INV-03| = 1.77e-12`, and **Metal neural inference alone already runs at `68.8 full sweeps/s` (`14.54 ms/tick`, `549.4 GB/s`) — `2.29×` faster than Gate G7's `30 t/s` threshold**.
- **Where the honest wall sits:** End-to-end full-evaluation throughput on the current hybrid host-CPU-tick + Metal-controller architecture is **`6.46 steady ticks/s`** on a loaded 36-core M5 Ultra (`~140 ms/tick` spent in host CPU field/pyramid/sensing/uptake/deposit phases vs `14.5 ms/tick` on Metal GPU), below Gate G7's `30 t/s` sustained 10-minute target. Reaching `≥ 30 t/s` end-to-end at 400k requires moving the remaining host field/sensing/uptake phases onto a GPU-resident tick.
- **Short-window limit:** The `64`-tick measurement above verifies founding admission, steady bandwidth, and `INV-03` conservation, not a 10-minute evolved-population ecological band check (`≥ 380k` live over `10` wall minutes).
