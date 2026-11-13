# FL-034 E1: the Interactive profile's source power

This directory records FrankenLife's first scale-up experiment (plan §19.5, FL-034). It covers what was run,
the rule declared before the runs, every run, and the decision. It holds the base configuration, the four
specifications verbatim and the experiments' records. The experiment directories (catalogs, per-run logs)
are kept outside git.

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
