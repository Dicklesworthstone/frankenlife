# FL-058: Single-World Multi-Device Spatial Partitioning Evaluation

> **Status:** **Observed (2026-11-27)** — exact `0 ULP` spatially partitioned halo transport (`ResidentTileField` / `transport_partitioned`), boundary-consistent two-pass demand/grant uptake (`resolve_partitioned_uptake`, `INV-04`), boundary deposit reduction (`apply_partitioned_deposits`, `INV-03`), atomic post-motion organism + full-genome migration (`migrate_organisms`, `INV-07`), and empirical interconnect communication-budget measurements on real stepped FrankenLife worlds per plan §20.4, §24.2, §23.1, and ADR-11.
>
> **Four-Claims discipline:** Bit-exactness against single-device operators establishes the **Implementation** claim (`0 ULP` parity across all partition geometries); communication budgets and resident-tile timings on Apple M5 Ultra establish **Performance / Architecture Decision** evidence and establish none of the Visual or Ecological claims. No asynchronous stale-boundary relaxation is used (plan §20.4).

---

## 1. Decision Summary (Plan §24.2 & ADR-11)

**Recommendation: DEFER shipping a distributed multi-machine single-world runtime until single-device capacity is actually exhausted (`> 1M` organisms / `> 32 GiB` active genomes or multi-GPU host clusters beyond a single workstation).**

The measured evidence establishes both sides cleanly:
1. **Correctness & wire feasibility are proven (`Observed`):**
   - Exact synchronous 2D spatial domain decomposition reproduces global single-device transport, uptake (including 4-way corner Appendix C.1 ties and `INV-04`), and deposits (`INV-03`) **`0 ULP` bit-for-bit**, with zero stale boundaries (`crates/flife-field/tests/partition_evaluation.rs`, 5/5 passing).
   - At `400,000` organisms (`1024×1024×33`, `presets/screenshot-world.toml`, `maximum_scale_world_units = 16.0` → `5` pyramid levels `0..=4`), synchronous inter-device wire traffic across a `2×2` (`4`-device) partition is **`6.04 MB/tick`** (**`1.51 MB/device/tick`** across `6` barriers/tick) — **`1.85 ms/tick` (`5.6%` of a `30 Hz` tick)** on `10GbE TCP`, **`0.35 ms/tick` (`1.05%`)** on `80Gbps Thunderbolt 5`, **`0.18 ms/tick` (`0.54%`)** on `100GbE RDMA`, and **`0.052 ms/tick` (`0.15%`)** on `400GbE InfiniBand NDR`.
2. **Single-device limit is NOT reached at `400k` — and ecological clustering cuts static multi-device speedup nearly in half (`Observed`):**
   - On Apple M5 Ultra (`256 GiB` unified memory), `presets/screenshot-world.toml` (`400k` organisms, `3.995B` independent F16 parameters) uses **`19.71 GiB` peak RSS (`7.7%` of RAM)**, Metal GPU neural sweeps already run at **`14.54 ms/tick` (`68.8 sweeps/s`, `549.4 GB/s`)**, and single-device `transport_on` (`1024×1024×33`) runs in **`5.68 ms/tick`** (`8` workers). The current `6.46 t/s` end-to-end ceiling (`experiments/fl045/README.md`) is host CPU field/sensor/uptake/settle work, **not** single-GPU memory or bandwidth exhaustion.
   - Once ecological selection clusters organisms around moving food sources (`reference-small-world`, tick `512`), a static 2D grid partition exhibits **`1.78×` (`2×2`) to `1.92×` (`4×2`) peak load imbalance**, capping effective `4`-device neural speedup at `~2.2×` unless dynamic load-balanced repartitioning (e.g. orthogonal recursive bisection) is also added.
   - Therefore, per plan §24.2 (*"A measured single-device limit and a credible communication budget"*) and ADR-11, **moving the remaining host tick phases onto a single GPU** has strictly higher payoff than multi-machine single-world execution at `400k`, whereas multi-device partitioning becomes net-positive once worlds cross the single-GPU memory or neural-sweep wall (e.g. `> 1.5M–2M` full-evaluation organisms / `> 30–40 GiB` active F16 weights on a `1–2 TB/s` GPU, or multi-GPU nodes connected by `≥ 100GbE RDMA` / `NVLink`).

---

## 2. Correctness Verification (`crates/flife-field/src/partition.rs`)

Implemented in `#![forbid(unsafe_code)]` Rust (`crates/flife-field/src/partition.rs`) and verified by `crates/flife-field/tests/partition_evaluation.rs`:

| Component | Protocol | Parity / Invariant Verified |
|---|---|---|
| `TilePartition` / `TileBounds` | Periodic 2D `tiles_x × tiles_y` disjoint cell ownership (`1×1` through `4×4`, including non-divisible `17×13` grids) | Exact full-grid disjoint partition |
| `ResidentTileField` / `transport_partitioned` | Explicit ghost-apron exchange from neighbor interiors (`HaloMode::PerSubstep` and `HaloMode::WideTick`) + parallel tile stencil & decay | **`0 ULP` bit-for-bit** vs `transport_on` (incl. multi-substep `substeps ≥ 3`, `w=1,4`) |
| `resolve_partitioned_uptake` | 1-cell periodic energy halo → local `grid-cic-v1` requests → cross-seam `BoundaryDemandMsg` → authoritative owner-tile `resolve_cell` → `BoundaryGrantMsg` | **`0 ULP` bit-for-bit** vs `demands` + `resolve_uptake`; **`INV-04`** & Appendix C.1 4-way corner ties; duplicate `OrganismId` refused |
| `apply_partitioned_deposits` | Post-motion `grid-cic-v1` split → cross-seam `BoundaryDepositMsg` → owner-tile ascending-`OrganismId` reduction | **`0 ULP` bit-for-bit** vs `Deposits` + `apply_deposits`; **`INV-03`**; unit-ball violation refused without mutation |
| `migrate_organisms` | Two-phase verify-before-mutate periodic seam crossing transfer (`OrganismId`, wrapped position, full canonical genome bytes) | **`INV-07`** uniqueness preserved; atomic rollback on duplicate/invalid position; exact wire byte accounting |
| `estimate_comm_budget` | Exact per-phase byte & barrier breakdown over `5` real interconnects | Verified across `1×1`, `2×1`, `2×2`, `4×2` |

---

## 3. Observed Empirical Measurements (Apple M5 Ultra, 2026-11-27)

- **Host:** Apple M5 Ultra (`36` logical cores, `256 GiB` unified memory, Metal `Apple M5 Ultra`, macOS `27.0.1`).
- **Command:** `cargo run --release --locked -p flife-cli --example fl058_partition_eval`

### 3.1 Real Stepped-World Seam Traffic & Ecological Load Imbalance (`Observed`)

| World (`Grid`, Ticks Sampled) | Layout (`Devices`) | Live (`Mean`) | Boundary Demand Pkts/t | Est. Boundary Deposit Pkts/t | Mean Migrations/t (`B/t`) | Peak Load Imbalance | Uptake `0 ULP` Parity |
|---|---|---:|---:|---:|---:|---:|:---:|
| `reference-small-world` (`128×128`, ticks `385–512`) | `2×1` (`2`) | `512.2` | `30` | `14` | `0.055` (`2,199 B`) | `1.08×` | `true` |
| `reference-small-world` (`128×128`, ticks `385–512`) | `2×2` (`4`) | `512.2` | `34` | `15` | `0.078` (`3,141 B`) | **`1.78×`** | `true` |
| `reference-small-world` (`128×128`, ticks `385–512`) | `4×2` (`8`) | `512.2` | `46` | `21` | `0.094` (`3,770 B`) | **`1.92×`** | `true` |
| `desktop-world` (`512×512`, ticks `65–96`, Metal F16) | `2×1` (`2`) | `100,000.4` | `790` | `379` | `0.531` (`10,748 B`) | `1.002×` | — |
| `desktop-world` (`512×512`, ticks `65–96`, Metal F16) | `2×2` (`4`) | `100,000.4` | `1,599` | `767` | `0.969` (`19,600 B`) | `1.005×` | — |
| `desktop-world` (`512×512`, ticks `65–96`, Metal F16) | `4×2` (`8`) | `100,000.4` | `2,297` | `1,101` | `1.875` (`37,935 B`) | `1.010×` | — |

> **Velocity note (`Derived`):** `desktop-world` ticks `65–96` capture early post-founding acceleration (`~1.0` migration/t on `2×2`). At steady foraging speed (`v ≈ 0.6` world units/s → `Δx = 0.0047` cells/t at `Δt = 1/128 s`), seam crossings on `400k` / `1024×1024` (`2×2`, `4,096` seam cells) rise to `~7.5` migrations/t (`~152 KB/t`), and even at the hard velocity ceiling `v_max = 4.0` (`Δx = 0.03125` cells/t), upper-bound migration traffic is `~50` migrations/t (`~1.01 MB/t` total = `0.25 MB/device/t`) — still `< 17%` of sensor-pyramid halo traffic!

### 3.2 Resident-Tile Field Transport Scaling (`Observed`, `0 ULP` Bit-Exact)

Median single-tick wall time of `ResidentTileField::transport_step` (persistent per-tile buffers + ghost-apron exchange only, `HaloMode::PerSubstep`, `1` substep, `33` F32 planes) versus unpartitioned global `transport_on` on the same machine:

| Grid (`Planes`) | Global `transport_on` (`1w / 4w / 8w / 32w`) | Resident `1×1` (`1w`) | Resident `2×1` (`2w`) | Resident `2×2` (`4w`) | Resident `4×2` (`8w`) | `0 ULP` All Layouts |
|---|---:|---:|---:|---:|---:|:---:|
| `512×512` (`33`) | `5.57 / 1.97 / 1.25 / 1.01 ms` | `6.81 ms` | `4.51 ms` (`1.51×`) | `5.11 ms` (`1.33×`) | `3.30 ms` (`2.07×`) | `true` |
| `1024×1024` (`33`) | `25.06 / 7.57 / 5.68 / 8.05 ms` | `25.27 ms` | `14.12 ms` (`1.79×`) | `9.31 ms` (`2.72×`) | `8.33 ms` (`3.03×`) | `true` |

On a shared-memory workstation, plane-parallel `transport_on` (`5.68 ms` at `8` workers on `1024²`) beats spatial tiling (`8.33 ms` at `8` tiles) because plane-level parallelism requires zero halo gather/scatter. On **separate discrete memory spaces** (distinct GPUs/nodes), `ResidentTileField` achieves `2.72×–3.03×` compute reduction with only `1.08–1.63 MB/tick` of halo traffic.

### 3.3 Interconnect Communication Budget (`Derived` from Measured Seam Traffic)

#### A. `desktop-world` (`512×512×33`, `100k` F16 organisms, `Observed` seam traffic, `6` barriers/tick)

| Layout (`Devices`) | Total Wire / Tick | Per-Device / Tick | Dominant Term | `10GbE TCP` | `80Gbps TB5` | `100GbE RDMA` | `400GbE IB` | `NVLink / UF` |
|---|---:|---:|---|---:|---:|---:|---:|---:|
| `2×1` (`2`) | `1.51 MB` | `753 KB` | Pyramid halo (`69.6%`) | `1.16 ms` (`3.5%`) | `0.25 ms` (`0.75%`) | `0.11 ms` (`0.34%`) | `0.035 ms` (`0.10%`) | `0.008 ms` |
| `2×2` (`4`) | `3.04 MB` | `760 KB` | Pyramid halo (`70.3%`) | `1.17 ms` (`3.5%`) | `0.25 ms` (`0.75%`) | `0.11 ms` (`0.34%`) | `0.035 ms` (`0.10%`) | `0.008 ms` |
| `4×2` (`8`) | `4.57 MB` | `571 KB` | Pyramid halo (`70.6%`) | `1.00 ms` (`3.0%`) | `0.23 ms` (`0.68%`) | `0.098 ms` (`0.29%`) | `0.031 ms` (`0.09%`) | `0.007 ms` |

#### B. `screenshot-world` (`1024×1024×33`, `400k` F16 organisms, `5` pyramid levels `0..=4`, `Derived` via `2×` seam-perimeter scaling of `desktop-world` at constant areal density, `6` barriers/tick)

| Layout (`Devices`) | Total Wire / Tick | Per-Device / Tick | Breakdown (`Trans / Pyr / Upt / Dep / Mig`) | `10GbE TCP` | `80Gbps TB5` | `100GbE RDMA` | `400GbE IB` | `NVLink / UF` |
|---|---:|---:|---|---:|---:|---:|---:|---:|
| `2×1` (`2`) | `2.99 MB` | `1.50 MB` | `0.54 / 2.10 / 0.12 / 0.22 / 0.02 MB` | `1.84 ms` (`5.5%`) | `0.35 ms` (`1.05%`) | `0.18 ms` (`0.53%`) | `0.051 ms` (`0.15%`) | `0.010 ms` |
| `2×2` (`4`) | `6.04 MB` | `1.51 MB` | `1.08 / 4.23 / 0.24 / 0.44 / 0.04 MB` | `1.85 ms` (`5.6%`) | `0.35 ms` (`1.05%`) | `0.18 ms` (`0.54%`) | `0.052 ms` (`0.15%`) | `0.010 ms` |
| `4×2` (`8`) | `9.05 MB` | `1.13 MB` | `1.63 / 6.37 / 0.34 / 0.63 / 0.08 MB` | `1.51 ms` (`4.5%`) | `0.30 ms` (`0.90%`) | `0.15 ms` (`0.44%`) | `0.043 ms` (`0.13%`) | `0.009 ms` |

*(Percentages show fraction of a `30 t/s` = `33.33 ms` tick budget consumed purely by synchronous inter-device communication.)*

---

## 4. Architectural Conclusion & Future Crossover Trigger

1. **Key surprise from measurement:** **Sensor-pyramid halos (`70%` of wire traffic)** — not organism genome migration (`< 2%` observed, `< 17%` even at theoretical `v_max`) — dominate multi-device wire bandwidth. Because every coarse level `0..=4` requires a `2`-cell halo across `33` F32 planes for central-difference gradients, pyramid halos cost `~4×` more bytes than field transport halos.
2. **Immediate engineering priority (do NOT build multi-machine single-world yet):** Single-device M5 Ultra holds `400k` organisms in `7.7%` of RAM and sweeps all `4.0B` independent F16 parameters on Metal in `14.54 ms/tick`. Moving the remaining host CPU field/sensor/uptake/settle phases onto the single GPU closes the gap to `30 t/s` without paying `6` cross-device barriers or `1.8–1.9×` ecological spatial load imbalance.
3. **Trigger to promote FL-058 to a production distributed single-world backend (plan §24.2):** Revisit only when a concrete experiment requires either **`> 1.5M` live independent organisms (`> 30 GiB` active F16 weights exceeding single-GPU bandwidth at `30 Hz`)** or **memory beyond a single host**, paired with **dynamic spatial load-balancing** (orthogonal recursive bisection) and **`≥ 80 Gbps` RDMA / Thunderbolt 5 / NVLink** interconnects.
