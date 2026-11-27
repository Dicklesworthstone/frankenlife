# FL-034 E1 and E2: the Interactive and Desktop ecology profiles' source power

This directory records FrankenLife's scale-up experiments (plan §19.5, FL-034): E1 for the Interactive profile
(25,000 organisms) and, [below](#fl-034-e2-the-desktop-ecology-profiles-source-power), E2 for the Desktop
ecology profile (100,000). Each covers what was run, the rule declared before the runs, every run, and the
decision. The directory holds each experiment's base configuration, its specifications verbatim and its
records. The experiment directories (catalogs, per-run logs) are kept outside git.

## Why

The Interactive operating profile (§19.5) is a workload definition: 25,000 organisms on a 256 × 256 field with
33 planes, controllers every tick. §19.5 asks each qualifying profile to supply its own ecology "through
declared small-to-large experiments". It says copying the small world's energy budget is not a valid scale-up,
and that the performance gate uses an evolving population in its declared band. Before E1, no 25,000-organism
configuration existed. The 25k load configuration used for timing is not one: it loses about 22,000 of its
founders within 450 ticks and then holds about 3,000 to 4,500. Every "25k" timing before E1 therefore measured
a founding transient.

## Summary

| Source power | Discovery (seeds 11–14) | Held-out (seeds 2011–2014) | Verdict |
|---:|---|---|---|
| 960 (control) | 0 of 4 in band: 16,728–19,697 | 0 of 4: 17,040–18,399 | below the band |
| **1248** (1.3×) | **4 of 4: 22,211–25,079** | **4 of 4: 22,537–25,007** | **adopted** |
| 1536 (1.6×) | 2 of 4: 26,883–31,020 | 3 of 4: 28,525–30,602 | does not qualify |

By the declared rule, 1248 per source was adopted: `presets/interactive-world.toml` is this directory's base
with that power (`crates/flife-cli/tests/interactive_preset.rs` keeps it so).

## Provenance

- Base: `interactive-base.toml` (SHA-256 `9bbd732f…12cad2`). It is `presets/reference-small-world.toml`
  (SHA-256 `064f583f…23406b`) scaled to the profile, changing only the name, the world and grid to 256 (4× the
  area at the same cell size), founders 4,096 → 25,000, the population cap 16,384 → 40,000, sources 8 → 32
  (the reference's energy per area), and the genome and total memory budgets (2, 2.25 and 8 GiB). Its header
  lists each change.
- Specifications: `e1/spec-11.toml` … `e1/spec-14.toml`, one seed pair each (discovery `d`, held-out
  `2000 + d`), identical except for their name and seeds. SHA-256: 11 `fd555c01…475ffc`, 12 `803c22e8…2ad57`,
  13 `8e700829…74eb1c`, 14 `fa3926bc…a52666`.
- Binary: `flife experiment` (FL-041) built at 1869984, with CPU-reference controllers and 16 workers per run. On
  the Mac (Apple M5 Ultra, macOS 27.0.1, binary SHA-256 `7d37375c…915869`), pairs 11 and 12 ran one after the
  other. On hz4 (AMD EPYC 7282, Linux 7.0.0 x86_64, binary SHA-256 `223f739a…b744b3`), pairs 13 and 14 ran
  concurrently. A run's result depends on neither its host nor its worker count (world digests are pinned on
  both hosts), so splitting the seeds across hosts changes no result. hz4 was unreachable on the network for
  about five minutes during the runs; its runs continued, and every one completed.
- Date: 2026-11-13, 00:02 to 01:50 EST.
- Records: `e1/records.ndjson` (SHA-256 `91302d53…f566f`), the four experiments' NDJSON output in pair order.
  Each has a start record, six run records and an end record. Every experiment's stderr was empty.

## Declared before the runs

The rule, the band and the conditions were written into the specifications and the working TODO at
2026-11-13 00:01:52 EST by the clock, before any run. Nothing was changed after.

- Conditions: the control (960 per source) and a sweep of `sources.power_per_source` over 1248 (1.3×) and 1536
  (1.6×). 8,192 ticks (64 simulated seconds) per run, metrics every 1,024 ticks.
- Band: 20,000 to 30,000 live (25,000 ± 20%).
- A condition qualifies on a split when every run of it on that split, over all four seed pairs, reaches tick
  8,192 (no extinction, no capacity pause) with `final_live` in the band. The candidate is the lowest source
  power whose discovery runs qualify. It is adopted for an Interactive preset only if its held-out runs qualify
  too. If none qualifies, nothing is adopted, and the next experiment is declared from these results.
- Prediction (scaling FL-048 E2's control by area): the control ends near 17,000–21,000, 1.3× near
  22,000–27,000, 1.6× near 27,000–34,000, perhaps pausing at the cap.

## Results

All 24 runs reached tick 8,192. None went extinct, paused at the population cap or failed.

| Power | Split | Seed | End | Final live | In band | Peak | Least | Births | Deaths | Lineages | Morisita | Moran's I | Digest |
|---:|---|---:|---|---:|---|---:|---:|---:|---:|---:|---:|---:|---|
| 960 | discovery | 11 | reached 8192 | 17,880 | no | 25,015 | 3,108 | 75,749 | 82,869 | 212 | 2.19 | 0.45 | `92ec78bfec22` |
| 960 | discovery | 12 | reached 8192 | 19,347 | no | 25,014 | 3,123 | 76,959 | 82,612 | 217 | 2.36 | 0.51 | `65cf8067895f` |
| 960 | discovery | 13 | reached 8192 | 19,697 | no | 25,013 | 2,799 | 78,816 | 84,119 | 200 | 2.32 | 0.42 | `761525a29c34` |
| 960 | discovery | 14 | reached 8192 | 16,728 | no | 25,016 | 3,032 | 72,659 | 80,931 | 187 | 2.74 | 0.50 | `309a70b3065a` |
| 960 | held-out | 2011 | reached 8192 | 17,040 | no | 25,011 | 2,883 | 72,919 | 80,879 | 181 | 2.31 | 0.49 | `25f68af4c6fb` |
| 960 | held-out | 2012 | reached 8192 | 17,165 | no | 25,013 | 2,613 | 70,114 | 77,949 | 177 | 2.43 | 0.52 | `52fefbb92f8c` |
| 960 | held-out | 2013 | reached 8192 | 18,303 | no | 25,009 | 2,946 | 73,469 | 80,166 | 179 | 2.32 | 0.47 | `ccdd01043166` |
| 960 | held-out | 2014 | reached 8192 | 18,399 | no | 25,014 | 3,382 | 76,972 | 83,573 | 220 | 2.20 | 0.46 | `59dca9509da0` |
| 1248 | discovery | 11 | reached 8192 | 24,006 | yes | 30,003 | 3,831 | 99,996 | 100,990 | 231 | 2.15 | 0.47 | `bee65e38cbc5` |
| 1248 | discovery | 12 | reached 8192 | 25,079 | yes | 30,814 | 3,816 | 97,418 | 97,339 | 214 | 2.39 | 0.51 | `08fe3b432af9` |
| 1248 | discovery | 13 | reached 8192 | 24,661 | yes | 30,982 | 3,443 | 100,396 | 100,735 | 223 | 2.28 | 0.44 | `3346a3b552aa` |
| 1248 | discovery | 14 | reached 8192 | 22,211 | yes | 29,901 | 3,744 | 94,871 | 97,660 | 184 | 2.67 | 0.51 | `35b676a08d03` |
| 1248 | held-out | 2011 | reached 8192 | 22,537 | yes | 28,822 | 3,563 | 92,766 | 95,229 | 219 | 2.32 | 0.51 | `bc15e6021c84` |
| 1248 | held-out | 2012 | reached 8192 | 22,577 | yes | 30,616 | 3,163 | 91,517 | 93,940 | 205 | 2.47 | 0.52 | `114b05ee799f` |
| 1248 | held-out | 2013 | reached 8192 | 24,299 | yes | 29,779 | 3,649 | 96,311 | 97,012 | 201 | 2.19 | 0.49 | `b4919db0f239` |
| 1248 | held-out | 2014 | reached 8192 | 25,007 | yes | 30,717 | 4,178 | 100,356 | 100,349 | 251 | 2.16 | 0.46 | `b16d82af6eda` |
| 1536 | discovery | 11 | reached 8192 | 28,689 | yes | 38,044 | 4,396 | 119,393 | 115,704 | 271 | 2.19 | 0.45 | `29bda772e138` |
| 1536 | discovery | 12 | reached 8192 | 31,020 | no | 38,439 | 4,395 | 118,938 | 112,918 | 246 | 2.42 | 0.51 | `02e896d993b0` |
| 1536 | discovery | 13 | reached 8192 | 30,521 | no | 38,375 | 3,923 | 123,537 | 118,016 | 230 | 2.32 | 0.42 | `c1e1848f4be8` |
| 1536 | discovery | 14 | reached 8192 | 26,883 | yes | 36,959 | 4,310 | 115,181 | 113,298 | 231 | 2.71 | 0.50 | `64c6e993445f` |
| 1536 | held-out | 2011 | reached 8192 | 29,176 | yes | 35,600 | 4,070 | 116,704 | 112,528 | 209 | 2.27 | 0.51 | `dcc8c8376fcd` |
| 1536 | held-out | 2012 | reached 8192 | 28,525 | yes | 37,552 | 3,590 | 111,410 | 107,885 | 214 | 2.40 | 0.54 | `0fd93c4d059f` |
| 1536 | held-out | 2013 | reached 8192 | 29,495 | yes | 37,006 | 4,169 | 117,992 | 113,497 | 233 | 2.25 | 0.49 | `e66d7aa6d77a` |
| 1536 | held-out | 2014 | reached 8192 | 30,602 | no | 38,134 | 4,732 | 122,841 | 117,239 | 276 | 2.14 | 0.45 | `2f2c57bfae82` |

Morisita and Moran's I are the organisms' at the last sample (tick 8,192), and the digest is the committed
world's at tick 8,192.

- Every run first lost most of its 25,000 founders. Its least live (2,613–4,732) fell within the first 1,024
  ticks in all 24 runs, and it then recovered. The control's peak (25,009–25,016) is its founding count and
  the first births. At 1248 and 1536 the recovery overshot to 28,822–38,439 before settling. FL-048's E2 saw the same founding bottleneck at the
  reference scale, so the populations at tick 8,192 descend from the survivors (§21.6's survival bias).
- The predictions held: the control ended at 16,728–19,697, 1.3× at 22,211–25,079 and 1.6× at 26,883–31,020.
- At 1536 the peaks came within 1,600 of the 40,000 cap, and three runs ended above the band.
- Every run kept colonies by FL-048's H1 criterion (organism Morisita > 1 and Moran's I > 0 at the last
  sample), with 177–276 founder lineages alive. Five catalog topologies were alive at the end of every run.

## Energy throughput (§19.5)

| World | Total source power (energy/s) | Per unit area (energy/s) | Per organism (energy/s) |
|---|---:|---:|---:|
| Reference preset: 8 × 960 on 128² | 7,680 | 0.469 | 1.655 |
| E1 control: 32 × 960 on 256² | 30,720 | 0.469 | 1.700 |
| 32 × 1248 (adopted) | 39,936 | 0.609 | 1.678 |
| 32 × 1536 | 49,152 | 0.750 | 1.674 |

Per organism is total source power over the mean final live of the condition's eight runs (for the reference,
FL-048 E1's sixteen control runs, 4,147–5,216). The final population tracks source power closely, at about 1.7
energy/s per organism in every condition and at both scales.

Where it went, in each run's last metrics window (ticks 7,168–8,192, mean over the condition's runs): uptake
inefficiency 49.9–50.2% of the source power, maintenance 47.1–47.7%, field decay 0.5–0.8%, and emission loss,
birth loss, controllers, sensors and motion under 0.5% each.

## Decision

1248 per source is adopted. Its discovery runs qualified (4 of 4 in band at tick 8,192), it is the lowest
power that did, and its held-out runs qualified too (4 of 4). The control did not qualify on either split. 1536
did not qualify on either split.

`presets/interactive-world.toml` is `interactive-base.toml` with the name `interactive-world` and
`power_per_source = 1248.0`. Its header lists its changes from the reference preset.

After adoption, outside the rule: the preset on its own seed (42) reached tick 8,192 with 22,779 live (peak
29,784, least 3,581), 92,363 births and 94,584 deaths, 213 lineages, and a ledger residual of 3.3e-9 (digest
`a8b7f6b3…5eb34`, the same binary on the Mac). That evolved world is what FL-034's profiles and its offscreen
G6 measurement start from.

## Limits

- Four seed pairs per condition. The band is wide (±20%), and 1248's final populations (22,211–25,079) sit
  at and below its middle.
- 8,192 ticks is 64 simulated seconds. Nothing here shows the population staying in the band beyond that.
  G8's endurance run is where that is tested.
- The rule tests only `final_live`. 1248's peaks (28,822–30,982) reach the band's top and slightly beyond it,
  within the 40,000 cap.
- Per-organism throughput is a ratio of totals, and the partition is one window's ledger flows. Neither
  measures what an individual organism takes.

# FL-034 E2: the Desktop ecology profile's source power

## Why

The Desktop ecology profile (§19.5) is 100,000 organisms on 512 × 512 × 33 with F16 canonical weights,
controllers every tick. No 100,000-organism configuration existed. E2 tests whether the Interactive preset's
energy per area, which E1 selected at 256 × 256, holds the Desktop profile's population at 4× the area.

## Summary

| Source power | Discovery (seeds 21–24) | Held-out (seeds 2021–2024) | Verdict |
|---:|---|---|---|
| **1248 (control)** | **4 of 4: 91,664–96,063** | **4 of 4: 92,381–99,361** | **adopted** |
| 1435.2 (1.15×) | 4 of 4: 107,253–110,919 | 4 of 4: 106,513–113,532 | qualifies, not the lowest |

By the declared rule, the control's power was adopted: `presets/desktop-world.toml` is this experiment's base
with only its name changed (`crates/flife-cli/tests/desktop_preset.rs` keeps it so).

## Provenance

- Base: `desktop-base.toml` (SHA-256 `25b83953…40c86`). It is `presets/interactive-world.toml` scaled to the
  profile, changing only the name, the world and grid to 512 (4× the area at the same cell size), founders
  25,000 → 100,000, the population cap 40,000 → 160,000, sources 32 → 128 (the Interactive preset's energy per
  area), the canonical weights F32 → F16, and the genome and total memory budgets (4, 4.5 and 16 GiB). Its
  header lists each change.
- Specifications: `e2/spec-21.toml` … `e2/spec-24.toml`, one seed pair each (discovery `d`, held-out `2000 + d`),
  identical except for their name and seeds. SHA-256: 21 `792a1a5b…25687`, 22 `ec032adc…61f72`, 23
  `f3153015…295d47`, 24 `b9f5fe13…cdddd0`.
- Binary: `flife experiment` built at a5e73e3 (SHA-256 `96cc1fa4…4f19`), CPU-reference controllers and 16
  workers per run, two pairs concurrently per host: pairs 21 and 22 on hz4 (AMD EPYC 7282, Linux 7.0.0
  x86_64), pairs 23 and 24 on ts1 (AMD Ryzen Threadripper PRO 5975WX, Linux 7.0.0 x86_64). The specifications'
  amendment (82fd4ba) changed no code. A run's result depends on neither its host nor its worker count, so
  splitting the seeds across hosts changes no result.
- Date: 2026-11-15, 00:14 to 04:49 EST. Each run took 3,029–4,449 s of wall time. A first attempt at 00:12 was
  refused at validation (a specification with no condition beside its control) and ran nothing.
- Records: `e2/records.ndjson` (SHA-256 `25ad57bc…31e8`), the four experiments' NDJSON output in pair order.
  Each has a start record, four run records and an end record. Every experiment's stderr was empty.

## Declared before the runs

Written into the specifications at 2026-11-15 00:04:31 EST, and amended at 00:13:31 EST, still before any run,
to add the 1.15× sweep when `flife experiment` refused a specification with only its control (commit 82fd4ba).

- Conditions: the control (1248 per source) and a sweep of `sources.power_per_source` over 1435.2 (1.15×).
  8,192 ticks (64 simulated seconds) per run, metrics every 1,024 ticks.
- Band: 80,000 to 120,000 live.
- A condition qualifies on a split when every run of it on that split, over all four seed pairs, reaches tick
  8,192 (no extinction, no capacity pause) with `final_live` in the band. The candidate is the lowest source
  power whose discovery runs qualify. It is adopted for a Desktop ecology preset only if its held-out runs
  qualify too. If none qualifies, nothing is adopted and the next experiment is declared from these results.
- Prediction (from E1's 1.655–1.700 energy/s of source power per organism): the control near 88,000–101,000
  live at tick 8,192, 1.15× near 101,000–116,000.

## Results

All 16 runs reached tick 8,192. None went extinct, paused at the population cap or failed.

| Power | Split | Seed | Final live | In band | Peak | Least | Births | Deaths | Lineages | Morisita | Moran's I | Digest |
|---:|---|---:|---:|---|---:|---:|---:|---:|---:|---:|---:|---|
| 1248 | discovery | 21 | 94,391 | yes | 119,501 | 15,507 | 379,009 | 384,618 | 907 | 1.82 | 0.18 | `7082822b4b4a` |
| 1248 | discovery | 22 | 91,664 | yes | 120,993 | 15,442 | 374,991 | 383,327 | 952 | 1.80 | 0.23 | `1f891384b92d` |
| 1248 | discovery | 23 | 95,172 | yes | 121,867 | 14,906 | 387,466 | 392,294 | 897 | 1.72 | 0.10 | `74bc6e4cb53d` |
| 1248 | discovery | 24 | 96,063 | yes | 120,342 | 14,805 | 380,744 | 384,681 | 917 | 1.78 | 0.15 | `fc731efe159a` |
| 1248 | held-out | 2021 | 92,381 | yes | 119,439 | 14,791 | 380,065 | 387,684 | 852 | 1.85 | 0.17 | `89fae6e36480` |
| 1248 | held-out | 2022 | 99,361 | yes | 119,331 | 15,252 | 389,349 | 389,988 | 884 | 1.61 | 0.20 | `31e5c97f9158` |
| 1248 | held-out | 2023 | 92,393 | yes | 117,891 | 15,575 | 379,413 | 387,020 | 893 | 1.84 | 0.18 | `03168c22af7b` |
| 1248 | held-out | 2024 | 94,777 | yes | 119,405 | 15,689 | 390,394 | 395,617 | 928 | 1.76 | 0.26 | `311859ae6ec0` |
| 1435.2 | discovery | 21 | 110,919 | yes | 138,180 | 17,104 | 436,023 | 425,104 | 978 | 1.83 | 0.19 | `8347a4ec2ec2` |
| 1435.2 | discovery | 22 | 107,253 | yes | 140,905 | 17,080 | 430,825 | 423,572 | 979 | 1.80 | 0.23 | `038d4b4e6f94` |
| 1435.2 | discovery | 23 | 110,345 | yes | 140,573 | 16,405 | 444,493 | 434,148 | 981 | 1.69 | 0.11 | `b2109fce1a29` |
| 1435.2 | discovery | 24 | 110,584 | yes | 138,416 | 16,328 | 435,210 | 424,626 | 955 | 1.80 | 0.16 | `89ec18f0e836` |
| 1435.2 | held-out | 2021 | 109,117 | yes | 140,051 | 16,292 | 439,388 | 430,271 | 942 | 1.85 | 0.19 | `aee707f62c2c` |
| 1435.2 | held-out | 2022 | 113,532 | yes | 138,893 | 16,734 | 444,502 | 430,970 | 961 | 1.61 | 0.21 | `9241932b46b7` |
| 1435.2 | held-out | 2023 | 106,513 | yes | 134,856 | 17,046 | 433,280 | 426,767 | 965 | 1.83 | 0.17 | `9d2e7c780bce` |
| 1435.2 | held-out | 2024 | 109,079 | yes | 138,317 | 17,410 | 447,139 | 438,060 | 938 | 1.76 | 0.24 | `6497d7a37d33` |

Morisita and Moran's I are the organisms' at the last sample (tick 8,192), and the digest is the committed
world's at tick 8,192.

- As at both smaller scales, every run first lost most of its 100,000 founders: its least live (14,791–17,410)
  fell within the first 1,024 ticks, and it then recovered and overshot (peaks 117,891–140,905, below the
  160,000 cap) before settling. The populations at tick 8,192 descend from the survivors (§21.6).
- The predictions held: the control ended at 91,664–99,361, 1.15× at 106,513–113,532.
- Every run kept colonies by FL-048's H1 criterion (organism Morisita > 1 and Moran's I > 0 at the last
  sample), with 852–981 founder lineages alive and all five catalog topologies alive. The default topology
  (`[64, 32]`) held 91–94% of the live organisms in the last window.
- The largest normalized ledger residual of any metrics window in any run was 1.19e-8.

## Energy throughput (§19.5)

| World | Total source power (energy/s) | Per unit area (energy/s) | Per organism (energy/s) |
|---|---:|---:|---:|
| Interactive preset: 32 × 1248 on 256² | 39,936 | 0.609 | 1.678 |
| E2 control: 128 × 1248 on 512² (adopted) | 159,744 | 0.609 | 1.690 |
| 128 × 1435.2 | 183,706 | 0.701 | 1.675 |

Per organism is total source power over the mean final live of the condition's eight runs (94,525 and
109,668). The population still tracks source power at about 1.7 energy/s per organism, now at three scales.

Where it went, in each run's last metrics window (ticks 7,168–8,192): uptake inefficiency 48.2–51.5% of the
source power, maintenance 45.5–49.4%, field decay 0.5–0.7%, and emission loss, birth loss, controllers,
sensors, motion and toxicity under 0.6% each.

## Decision

The control's power, 1248 per source, is adopted. Its discovery runs qualified (4 of 4 in band at tick
8,192), it is the lowest power that did, and its held-out runs qualified too (4 of 4). 1435.2 also qualified on
both splits but is not the lowest.

`presets/desktop-world.toml` is `desktop-base.toml` with the name `desktop-world`. Its header lists its changes
from the Interactive preset.

## Limits

- Four seed pairs per condition, as in E1. Every control run ended in the lower half of the band.
- 8,192 ticks is 64 simulated seconds. Nothing here shows the population staying in the band beyond that.
- The rule tests only `final_live`.
- These runs say nothing about speed: they ran on the CPU reference on x86-64 hosts, two at a time. The
  Desktop profile's G4/G6 qualification is FL-034's remaining work.

# FL-034 G6: the two profiles in a window (to run)

G6 (plan §21.3) asks a qualified profile for 30 fps display and a p95 input/camera latency of at most 100 ms
during live inference. It is shown in a real window. A window takes the operator's focus, so agent sessions
measure only offscreen, and those numbers are not G6 evidence. This is the run that shows it.

## The worlds

Each profile is measured on an evolved world, not a founding one (see [Why](#why)). Run each world to tick 8,192
once on the Mac, from the repository, with a release `flife`, keeping it in a run directory:

```text
flife run presets/interactive-world.toml --ticks 8192 --every 1024 --cpu-workers 16 \
  --run-dir INTERACTIVE_RUN --checkpoint-every 0
sed 's/^seed = 42$/seed = 2022/' presets/desktop-world.toml > desktop-2022.toml
flife run desktop-2022.toml --ticks 8192 --every 1024 --cpu-workers 16 \
  --run-dir DESKTOP_RUN --checkpoint-every 0
```

The Interactive world is the preset on its own seed, as E1 ran it above. The Desktop world is on E2's held-out
seed 2022. Neither reaches E1's or E2's end state on today's tree. On the Mac at 7b4a4e5, the Interactive world
ends at tick 8,192 with 22,559 live (digest `595e2d90…3167b`, the same with CPU and with Metal controllers),
where E1 had 22,779 (`a8b7f6b3…5eb34`). A Mac run of the Desktop world earlier on 2026-11-27 ended with 99,065
live and 389,045 births, where E2 had 99,361 and 389,349.

The model changed after E1 and E2 ran. commit:d8a88aa (2026-11-23) added plan §11.5's bounded moment-only
cone contraction. It contracts a field cell's moments, never its energy, back inside `|m| ≤ E` when they exceed
the cone by more than 10⁻⁶ relative, one tenth of `INV-02`'s 10⁻⁵. The Interactive world shows where the
contraction first acts. These were observed on the Mac on 2026-11-27 with
the release builds of 62f16ac (before d8a88aa) and 147c18d (after it), both branched from one tick-4,096
checkpoint:

- The two worlds are bit-identical through tick 4,135.
- At tick 4,136 exactly one field cell differs: cell (226, 92). Its `|m| / E` is 1 + 1.01 × 10⁻⁶ before the
  change and 1 − 4 × 10⁻⁸ after it. All 32 of its moments are scaled by the same factor, and its energy is
  unchanged.
- Its four neighbours, 0.4 to 3.8 × 10⁻⁷ outside the cone, are the same in both.
- At tick 4,137 the five organisms within about 5 world units of that cell move or decide differently.

The adopted powers' rule runs were not rerun under the contraction, and their digests no longer reproduce.
The Nov 20 build reproduces E1's world exactly (`a8b7f6b3…5eb34`). Today's tree gives the same world with CPU
and Metal controllers and on 7 or 16 workers.

A window
run resumes each world and keeps it on in its directory, so every run starts where the last one stopped,
near the same population.

## The run

With nothing else heavy on the Mac (note the load average before and after):

```text
FLIFE_G6_INTERACTIVE_RUN=INTERACTIVE_RUN FLIFE_G6_DESKTOP_RUN=DESKTOP_RUN \
  cargo test --release --locked -p flife-studio --features synthetic-input --test window_g6
```

Where cargo is the rch shim, keep this build on the Mac as `scripts/studio.sh` does
(`env -u RCH_REQUIRE_REMOTE RCH_SHIM_LOCAL_IDE=1`, with `CARGO_TARGET_DIR` on the build disk).

`crates/flife-studio/tests/window_g6.rs` runs FL-024's three small-world cases first. It then resumes each world
in a window at its `[presentation]` size (1920 × 1080 points, 3840 × 2160 pixels at 2× Retina), with Metal
controllers on 16 workers. Each world runs for 20 s, paced to 30 fps, with the HUD drawn. The test posts a real
AppKit scroll at the content's centre in every third frame interval, zooming in and out in turn.

## Declared before the run

A profile shows G6 when its case prints a `G6 [...]` line and passes every check:

- it presents at least 98% of the paced rate, 29.4 fps;
- the `MoveLatencyBound` p95 is at most 100 ms;
- no input is dropped or refused, and every move posted moves the view;
- no frame is read back.

One run is made on each world, and the Mac's load is recorded with it. Its `G6 [...]` lines and the commit go on
FL-034 (`flife-2je`). A failing case leaves G6 unshown for that profile. It is not retried until something has
changed.
