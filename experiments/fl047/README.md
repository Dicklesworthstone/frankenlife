# FL-047: Endurance Turnover, Checkpoint/Restart Recovery, and Fault-Injection Campaign (Gate G8)

> **Status:** **Observed (`5ee847f`, 2026-11-27)** — automated controlled-churn endurance, bounded post-warmup footprint, multi-stage checkpoint/restart, corrupted-checkpoint skip/recovery, explicit capacity-pause (`INV-12`), and numerical-fault (`stop-and-preserve`) verification suite (`crates/flife-cli/tests/endurance_and_faults.rs`).
>
> **Honest Scope Disclosure (plan §21.3 Gate G8):** Functional endurance mechanics (real birth/death turnover, `INV-07` non-resurrection, post-warmup memory stability, multi-stage checkpoint/restart bit-exactness, corrupted-chunk recovery, capacity-pause recovery, and NaN fault preservation) are verified here. The **8-wall-clock-hour continuous soak** at a qualifying workload (864,000 ticks at 30 t/s = 1.875 simulated hours at `Δt = 1/128 s`) remains an explicit operator-scheduled soak run and is NOT claimed complete by these short integration subruns.

---

## Automated G8 Subruns (`crates/flife-cli/tests/endurance_and_faults.rs`)

Observed (`5ee847f`, Apple M5 Ultra, `cargo test --locked -p flife-cli --test endurance_and_faults -- --nocapture`, **3/3 passed in 1.28s**):

| Subrun Test | What It Exercises | Observed Evidence & Invariants |
|---|---|---|
| `controlled_churn_endurance_turnover_bounded_memory_and_inv07` | 512 ticks on a high-turnover ecology with zero automatic reseeding (`automatic_reseed = false`), tracking genuine births, deaths, arena slot reclamation, and distinct wall vs simulated clocks | `512` ticks (`4.00` sim s in `0.31` wall s = `1,636.0 t/s`), `84` births, `171` deaths, `41` live at end, `max |INV-03| = 3.372e-8`, **0-byte post-warmup footprint drift (`[6,431,912, 6,431,912]` bytes)**, `INV-07` dead identities never resurrected |
| `multi_stage_checkpoint_restart_and_corrupted_chunk_recovery` | Uninterrupted 192-tick control vs two-stage `pause-and-stream` run (`0 → 96`, drop, `resume 96 → 192`) + fault injection corrupting the newest persisted checkpoint on disk before resuming | Exact bit-for-bit `World::digest()` identity on normal restart AND after detecting/skipping a corrupted newest checkpoint (`skipped >= 1`) |
| `capacity_pause_and_numerical_fault_subruns_preserve_committed_state` | (a) Tight population ceiling (`128`, `pause-before-commit`) triggered by natural biological births, followed by explicit `RemoveOrganisms(Recycle)` and clean resumption; (b) `f64::NAN` energy injection under `stop-and-preserve` | `INV-12` explicit capacity pause with zero lost state; `CoreError::NumericalInvalid` preserves exact committed world digest & live count (fault never masquerades as organism death, plan §7.6) |

## Verification Command

```bash
cargo test --locked -p flife-cli --test endurance_and_faults -- --nocapture
```
